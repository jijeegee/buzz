//! Ownerless multi-person chats ("Chat" in the UI).
//!
//! One kind:9007 carries the `["group_chat"]` marker and the initial people as
//! `p` tags; the relay adds them in the same transaction. People the relay
//! could not add directly (not friends, blocked, over the invitation cap) are
//! reported back as `invited` by comparing the picked list with the resulting
//! kind:39002 member list, which is the authoritative outcome.

use tauri::State;

use super::fetch::collect_members_by_channel;
use super::{channel_members_filter, channel_metadata_filter};
use crate::{
    app_state::AppState,
    events,
    models::ChannelInfo,
    nostr_convert,
    relay::{query_relay, submit_event_with_keys},
};

#[derive(serde::Serialize)]
pub struct CreateGroupChatResult {
    pub channel: ChannelInfo,
    /// Picked people who did not become members (invitation or skipped).
    pub invited: Vec<String>,
}

/// Picked people missing from the member list, in pick order.
pub(crate) fn not_yet_members(picked: &[String], members: &[String]) -> Vec<String> {
    let members: std::collections::HashSet<String> =
        members.iter().map(|m| m.to_ascii_lowercase()).collect();
    let mut seen = std::collections::HashSet::new();
    picked
        .iter()
        .map(|p| p.to_ascii_lowercase())
        .filter(|p| !members.contains(p) && seen.insert(p.clone()))
        .collect()
}

#[tauri::command]
pub async fn create_group_chat(
    name: Option<String>,
    pubkeys: Vec<String>,
    state: State<'_, AppState>,
) -> Result<CreateGroupChatResult, String> {
    let channel_uuid = uuid::Uuid::new_v4();
    let creator_keys = state.user_credential()?;
    let creator_pubkey = creator_keys.public_key().to_hex();
    // Google-session community: a local agent joins as its server bot.
    let pubkeys = crate::auth::bots::adopt_named_agents(&state, pubkeys).await?;
    let picked: Vec<String> = pubkeys
        .iter()
        .filter(|p| !p.eq_ignore_ascii_case(&creator_pubkey))
        .cloned()
        .collect();
    let builder =
        events::build_create_group_chat(channel_uuid, name.as_deref(), &picked, &creator_pubkey)?;
    submit_event_with_keys(builder, &state, &creator_keys, None).await?;

    let channel_id = channel_uuid.to_string();
    state.mark_pending_owned_channel(&creator_pubkey, &channel_id);

    let events = query_relay(
        &state,
        &[
            channel_metadata_filter(&[&channel_id]),
            channel_members_filter(&channel_id, true),
        ],
    )
    .await?;
    let metadata = events
        .iter()
        .find(|ev| ev.kind.as_u16() == 39000)
        .ok_or_else(|| "chat created but not yet available".to_string())?;
    let mut channel = nostr_convert::channel_info_from_event(metadata, None, Some(true))?;
    let member_events: Vec<nostr::Event> = events
        .iter()
        .filter(|ev| ev.kind.as_u16() == 39002)
        .cloned()
        .collect();
    if let Some(info) = collect_members_by_channel(&member_events).remove(&channel_id) {
        channel.member_count = info.count;
        channel.member_pubkeys = info.pubkeys;
    }
    let invited = not_yet_members(&picked, &channel.member_pubkeys);
    Ok(CreateGroupChatResult { channel, invited })
}

#[cfg(test)]
mod tests {
    use super::not_yet_members;

    #[test]
    fn invited_are_picked_people_missing_from_members() {
        let picked = vec!["AA".to_string(), "bb".into(), "cc".into(), "aa".into()];
        let members = vec!["me".to_string(), "aa".into(), "cc".into()];
        assert_eq!(not_yet_members(&picked, &members), vec!["bb".to_string()]);
        assert!(not_yet_members(&picked, &["aa".into(), "bb".into(), "cc".into()]).is_empty());
    }
}
