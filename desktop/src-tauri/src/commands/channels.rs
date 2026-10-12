use tauri::State;

use crate::{
    app_state::AppState,
    events,
    models::{ChannelDetailInfo, ChannelInfo, ChannelMembersResponse, GetChannelsPayload},
    nostr_convert,
    relay::{
        assert_expected_relay_scope, assert_expected_signer, query_relay,
        relay_api_base_url_with_override, submit_event, submit_event_at_with_keys,
        submit_event_with_keys,
    },
};

// ── Reads (pure-nostr via /query) ────────────────────────────────────────────

// The relay-backed channel list computation (fetch_channels, DirectoryScope,
// the directory cursor, the not-modified hash, and member-count collection)
// lives in the `fetch` submodule to keep this file under the per-file line cap.
mod fetch;
use fetch::{compute_channels_hash, fetch_channels, DirectoryScope};

// ── Tauri commands ────────────────────────────────────────────────────────────

/// Return the channels the active identity belongs to (plus its own
/// not-yet-propagated creations). This is the 60s poll path: it performs no
/// all-open directory scan, so its phase-2 fan-out is bounded by membership.
/// Joinable open channels are served separately by
/// [`get_open_channel_directory`].
///
/// `known_hash` is a previously returned `hash` value. When it matches the
/// computed stable hash (which excludes `last_message_at`), the response
/// carries `channels: null` so the multi-MB list is not serialized across IPC.
/// `last_messages` is always included because it is cheap and changes with
/// every new message.
#[tauri::command]
pub async fn get_channels(
    known_hash: Option<String>,
    state: State<'_, AppState>,
) -> Result<GetChannelsPayload, String> {
    let channels = fetch_channels(&state, DirectoryScope::MemberOnly).await?;

    let last_messages: std::collections::HashMap<String, String> = channels
        .iter()
        .filter_map(|c| {
            c.last_message_at
                .as_ref()
                .map(|ts| (c.id.clone(), ts.clone()))
        })
        .collect();

    let hash = compute_channels_hash(&channels);

    // Not-modified short-circuit: skip the multi-MB IPC payload when the
    // caller's hash matches. `last_messages` still ships so the TS side can
    // update sidebar timestamps without re-rendering the full list.
    if known_hash.as_deref() == Some(hash.as_str()) {
        return Ok(GetChannelsPayload {
            hash,
            channels: None,
            last_messages,
        });
    }

    Ok(GetChannelsPayload {
        hash,
        channels: Some(channels),
        last_messages,
    })
}

/// Return the open-channel directory: every joinable open channel plus the
/// identity's own channels, marked with `is_member`. This is the discovery
/// superset that `get_channels` intentionally omits from the 60s poll — the
/// channel browser and global search fetch it on demand (browse open / search
/// active) with a generous staleTime, so the expensive all-open scan runs only
/// when a user is actually looking for channels to join.
#[tauri::command]
pub async fn get_open_channel_directory(
    state: State<'_, AppState>,
) -> Result<Vec<ChannelInfo>, String> {
    fetch_channels(&state, DirectoryScope::IncludeOpenDirectory).await
}

#[tauri::command]
pub async fn get_channel_details(
    channel_id: String,
    state: State<'_, AppState>,
) -> Result<ChannelDetailInfo, String> {
    let events = query_relay(
        &state,
        &[serde_json::json!({
            "kinds": [39000],
            "#d": [channel_id],
            "limit": 1
        })],
    )
    .await?;

    events
        .first()
        .map(nostr_convert::channel_detail_from_event)
        .transpose()?
        .ok_or_else(|| "channel not found".to_string())
}

/// Cap for the kind:0 profile join in `get_channel_members`. Enriching a
/// huge roster required an `authors` filter carrying every member pubkey — a
/// query whose size and relay cost grow linearly with membership and which
/// dominated channel-open latency on large channels. Members past the cap
/// keep `display_name: None` (the UI falls back to pubkey-derived labels and
/// resolves visible names through its profile caches); `role == "bot"` agent
/// flags are roster-derived and unaffected by the cap.
const MEMBER_PROFILE_JOIN_LIMIT: usize = 500;

/// The pubkeys eligible for the kind:0 profile join: roster order, capped.
fn profile_join_pubkeys(members: &[crate::models::ChannelMemberInfo], limit: usize) -> Vec<String> {
    members
        .iter()
        .take(limit)
        .map(|member| member.pubkey.clone())
        .collect()
}

#[tauri::command]
pub async fn get_channel_members(
    channel_id: String,
    read_your_writes: Option<bool>,
    state: State<'_, AppState>,
) -> Result<ChannelMembersResponse, String> {
    let events = query_relay(
        &state,
        &[channel_members_filter(
            &channel_id,
            read_your_writes.unwrap_or(false),
        )],
    )
    .await?;

    let mut response = events
        .first()
        .map(nostr_convert::channel_members_from_event)
        .transpose()?
        .ok_or_else(|| "channel members not found".to_string())?;

    // Batch-fetch kind:0 profiles to populate display names, capped so the
    // query cost is bounded on large rosters (see MEMBER_PROFILE_JOIN_LIMIT).
    let pubkeys = profile_join_pubkeys(&response.members, MEMBER_PROFILE_JOIN_LIMIT);
    if !pubkeys.is_empty() {
        let profile_events = query_relay(
            &state,
            &[serde_json::json!({
                "kinds": [0],
                "authors": pubkeys,
                "limit": pubkeys.len()
            })],
        )
        .await
        .unwrap_or_default();

        // Build pubkey → profile display metadata from kind:0 events.
        let mut profile_map = std::collections::HashMap::new();
        for ev in &profile_events {
            let pk = ev.pubkey.to_hex();
            if let Ok(profile) = nostr_convert::profile_info_from_event(ev) {
                profile_map.insert(
                    pk,
                    (
                        profile.display_name,
                        nostr_convert::profile_has_valid_oa_owner(ev),
                    ),
                );
            }
        }

        // Populate profile-derived fields on each member.
        for member in &mut response.members {
            if member.role == "bot" {
                member.is_agent = true;
            }
            if let Some((display_name, is_agent)) = profile_map.get(&member.pubkey) {
                if member.display_name.is_none() {
                    member.display_name = display_name.clone();
                }
                member.is_agent = member.is_agent || *is_agent;
            }
        }
    }

    Ok(response)
}

// ── Writes (signed events) ──────────────────────────────────────────────────

fn parse_channel_uuid(channel_id: &str) -> Result<uuid::Uuid, String> {
    uuid::Uuid::parse_str(channel_id).map_err(|_| format!("invalid channel UUID: {channel_id}"))
}

/// Build the kind:39002 member-list filter. Display reads stay replica-eligible;
/// `read_your_writes` is for callers acting on state this client just wrote.
fn channel_members_filter(channel_id: &str, read_your_writes: bool) -> serde_json::Value {
    let mut filter = serde_json::json!({
        "kinds": [39002],
        "#d": [channel_id],
        "limit": 1,
    });
    if read_your_writes {
        // Read-your-writes: pin to the writer so a lagging replica cannot hide
        // the member list the relay wrote for a just-created channel.
        filter["consistency"] = serde_json::json!("strong");
    }
    filter
}

/// Build the kind:39000 filter that reads back channel metadata by ID right
/// after this client created or edited those channels.
pub(crate) fn channel_metadata_filter(channel_ids: &[impl serde::Serialize]) -> serde_json::Value {
    serde_json::json!({
        "kinds": [39000],
        "#d": channel_ids,
        "limit": channel_ids.len(),
        // Read-your-writes: a lagging read replica would miss the metadata the
        // relay just wrote, so pin this read to the writer.
        "consistency": "strong",
    })
}

#[tauri::command]
pub async fn create_channel(
    name: String,
    channel_type: String,
    visibility: String,
    description: Option<String>,
    ttl_seconds: Option<i32>,
    state: State<'_, AppState>,
) -> Result<ChannelInfo, String> {
    let channel_uuid = uuid::Uuid::new_v4();

    let vis = match visibility.as_str() {
        "open" | "private" => visibility.as_str(),
        other => return Err(format!("invalid visibility: {other}")),
    };
    let ct = match channel_type.as_str() {
        "stream" | "forum" => channel_type.as_str(),
        other => return Err(format!("invalid channel_type: {other}")),
    };

    let builder = events::build_create_channel(
        channel_uuid,
        &name,
        vis,
        ct,
        description.as_deref(),
        ttl_seconds,
    )?;

    // Capture the signing identity before submission so the pending-owner
    // mark below is bound to whoever actually signed this create — not
    // whoever `state.keys` holds once the network round-trip completes. An
    // in-process identity swap while the request is in flight must not be
    // able to retarget the mark onto the new identity.
    let creator_keys = state.user_credential()?;
    let creator_pubkey = creator_keys.public_key().to_hex();
    submit_event_with_keys(builder, &state, &creator_keys, None).await?;

    // Mark this channel pending-owner: we just created it, so we know we're
    // the owner, but the relay's kind:39002 membership entry (#1761) is
    // provisioned asynchronously. `get_channels` consults this overlay to
    // classify us as `is_member=true` until that entry is observable. Bound
    // to the identity that signed the create above, so an in-process
    // identity swap can neither inherit nor retarget this entry.
    let channel_uuid_string = channel_uuid.to_string();
    state.mark_pending_owned_channel(&creator_pubkey, &channel_uuid_string);

    // Re-fetch the canonical metadata event to return ChannelInfo.
    let events = query_relay(&state, &[channel_metadata_filter(&[&channel_uuid_string])]).await?;

    events
        .first()
        .map(|ev| nostr_convert::channel_info_from_event(ev, None, None))
        .transpose()?
        .ok_or_else(|| "channel created but metadata not yet available".to_string())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateChannelInput {
    pub channel_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    /// Absent = leave unchanged, `null` = clear (permanent), seconds = set.
    #[serde(default, deserialize_with = "crate::util::double_option")]
    pub ttl_seconds: Option<Option<i32>>,
}

#[tauri::command]
pub async fn update_channel(
    input: UpdateChannelInput,
    state: State<'_, AppState>,
) -> Result<ChannelDetailInfo, String> {
    let uuid = parse_channel_uuid(&input.channel_id)?;
    let builder = events::build_update_channel(
        uuid,
        input.name.as_deref(),
        input.description.as_deref(),
        input.visibility.as_deref(),
        input.ttl_seconds,
    )?;
    submit_event(builder, &state).await?;

    let events = query_relay(&state, &[channel_metadata_filter(&[&input.channel_id])]).await?;

    events
        .first()
        .map(nostr_convert::channel_detail_from_event)
        .transpose()?
        .ok_or_else(|| "channel updated but metadata not yet available".to_string())
}

#[tauri::command]
pub async fn set_channel_topic(
    channel_id: String,
    topic: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let builder = events::build_set_topic(uuid, &topic)?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[tauri::command]
pub async fn set_channel_purpose(
    channel_id: String,
    purpose: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let builder = events::build_set_purpose(uuid, &purpose)?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[tauri::command]
pub async fn archive_channel(channel_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let builder = events::build_archive(uuid)?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[tauri::command]
pub async fn unarchive_channel(
    channel_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let builder = events::build_unarchive(uuid)?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[tauri::command]
pub async fn delete_channel(channel_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let builder = events::build_delete_channel(uuid)?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[tauri::command]
pub async fn add_channel_members(
    channel_id: String,
    pubkeys: Vec<String>,
    role: Option<String>,
    expected_relay_url: Option<String>,
    expected_signer_pubkey: Option<String>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let relay_base = relay_api_base_url_with_override(&state);
    assert_expected_relay_scope(expected_relay_url.as_deref(), &relay_base)?;
    let signing_keys = state.user_credential()?;
    assert_expected_signer(
        expected_signer_pubkey.as_deref(),
        &signing_keys.public_key().to_hex(),
    )?;
    let role_str = match role.as_deref() {
        Some("admin") => Some("admin"),
        Some("bot") => Some("bot"),
        Some("guest") => Some("guest"),
        Some("member") | None => None,
        Some(other) => return Err(format!("invalid role: {other}")),
    };

    let mut added = Vec::new();
    let mut errors = Vec::<serde_json::Value>::new();
    // Google-session community: a local agent joins as its server bot (its
    // record moves onto the bot first; key mode passes through).
    let pubkeys = crate::auth::bots::adopt_named_agents(&state, pubkeys).await?;

    for pubkey in &pubkeys {
        let builder = match events::build_add_member(uuid, pubkey, role_str) {
            Ok(b) => b,
            Err(e) => {
                errors.push(serde_json::json!({"pubkey": pubkey, "error": e}));
                continue;
            }
        };
        match submit_event_at_with_keys(builder, &state, &relay_base, &signing_keys).await {
            Ok(_) => added.push(pubkey.clone()),
            Err(e) => errors.push(serde_json::json!({"pubkey": pubkey, "error": e})),
        }
    }

    Ok(serde_json::json!({ "added": added, "errors": errors }))
}

#[tauri::command]
pub async fn remove_channel_member(
    channel_id: String,
    pubkey: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let builder = events::build_remove_member(uuid, &pubkey)?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[tauri::command]
pub async fn change_channel_member_role(
    channel_id: String,
    pubkey: String,
    role: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    // Only allow permission-tier roles for humans and bot/guest for bots.
    // Owner changes require a dedicated transfer-ownership flow.
    let role_str = match role.as_str() {
        "admin" | "member" | "guest" | "bot" => role.as_str(),
        "owner" => return Err("cannot assign owner role — use transfer ownership".into()),
        other => return Err(format!("invalid role: {other}")),
    };
    let builder = events::build_add_member(uuid, &pubkey, Some(role_str))?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[tauri::command]
pub async fn join_channel(channel_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let builder = events::build_join(uuid)?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[tauri::command]
pub async fn leave_channel(channel_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let uuid = parse_channel_uuid(&channel_id)?;
    let builder = events::build_leave(uuid)?;
    submit_event(builder, &state).await?;
    Ok(())
}

#[cfg(test)]
#[path = "channels_tests.rs"]
mod tests;

