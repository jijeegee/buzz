use std::collections::HashMap;

use kuzz_core_pkg::PresenceStatus;
use serde::Deserialize;
use serde_json::Value;
use tauri::State;

use crate::{
    app_state::AppState,
    events,
    managed_agents::persona_events::monotonic_created_at,
    models::{ProfileInfo, SearchUsersResponse, UserNotesResponse, UsersBatchResponse},
    nostr_convert,
    relay::{
        query_relay, query_relay_at_with_keys, relay_http_base_url, submit_event,
        submit_event_at_with_keys,
    },
};

#[tauri::command]
pub async fn get_profile(state: State<'_, AppState>) -> Result<ProfileInfo, String> {
    let my_pubkey = current_pubkey_hex(&state)?;
    let events = query_relay(
        &state,
        &[serde_json::json!({
            "kinds": [0],
            "authors": [my_pubkey],
            "limit": 1
        })],
    )
    .await?;

    Ok(events
        .first()
        .map(nostr_convert::profile_info_from_event)
        .transpose()?
        .unwrap_or_else(|| empty_profile_info(&current_pubkey_hex_unwrap(&state))))
}

#[tauri::command]
pub async fn update_profile(
    display_name: Option<String>,
    avatar_url: Option<String>,
    about: Option<String>,
    nip05_handle: Option<String>,
    state: State<'_, AppState>,
) -> Result<ProfileInfo, String> {
    // Token mode: the profile is the account's global profile, edited through
    // `PATCH /auth/profile`; a client kind:0 is refused by the relay there.
    if let crate::auth::CredentialMode::Token(_) = state.current_credential_mode() {
        crate::auth::commands::patch_global_profile(
            &state,
            display_name.clone(),
            avatar_url.clone(),
        )
        .await?;
        let mut info = empty_profile_info(&current_pubkey_hex(&state)?);
        info.display_name = display_name;
        info.avatar_url = avatar_url;
        info.has_profile_event = true;
        return Ok(info);
    }
    // Read-merge-write: kind 0 is a full profile snapshot.
    let my_pubkey = current_pubkey_hex(&state)?;
    let prior_events = query_relay(
        &state,
        &[serde_json::json!({
            "kinds": [0],
            "authors": [my_pubkey],
            "limit": 1
        })],
    )
    .await?;

    // Pull the current content as a JSON object so we can merge with
    // the caller's overrides.
    let current: Value = prior_events
        .first()
        .and_then(|ev| serde_json::from_str::<Value>(&ev.content).ok())
        .unwrap_or(Value::Null);

    let dn = display_name
        .as_deref()
        .or_else(|| current.get("display_name").and_then(Value::as_str));
    let name = current.get("name").and_then(Value::as_str);
    let picture = avatar_url
        .as_deref()
        .or_else(|| current.get("picture").and_then(Value::as_str));
    let ab = about
        .as_deref()
        .or_else(|| current.get("about").and_then(Value::as_str));
    let nip05 = nip05_handle
        .as_deref()
        .or_else(|| current.get("nip05").and_then(Value::as_str));

    let builder = events::build_profile(dn, name, picture, ab, nip05)?;
    submit_event(builder, &state).await?;

    // Re-fetch to return canonical profile.
    let events = query_relay(
        &state,
        &[serde_json::json!({
            "kinds": [0],
            "authors": [current_pubkey_hex(&state)?],
            "limit": 1
        })],
    )
    .await?;

    Ok(events
        .first()
        .map(nostr_convert::profile_info_from_event)
        .transpose()?
        .unwrap_or_else(|| empty_profile_info(&current_pubkey_hex_unwrap(&state))))
}

#[tauri::command]
pub async fn update_profile_at_relay(
    relay_url: String,
    expected_pubkey: String,
    expected_avatar_url: Option<String>,
    avatar_url: String,
    state: State<'_, AppState>,
) -> Result<ProfileInfo, String> {
    crate::relay::ensure_relay_allowed("update_profile_at_relay", &relay_url)?;
    let signer = capture_expected_signer(&state, &expected_pubkey)?;

    let api_base_url = relay_http_base_url(&relay_url);
    let filter = serde_json::json!({
        "kinds": [0],
        "authors": [expected_pubkey],
        "limit": 1
    });
    let prior_events = query_relay_at_with_keys(
        &state,
        &api_base_url,
        std::slice::from_ref(&filter),
        &signer,
        None,
    )
    .await?;
    let prior_event = prior_events.first();
    let current: Value = prior_event
        .and_then(|event| serde_json::from_str::<Value>(&event.content).ok())
        .unwrap_or(Value::Null);
    let current_avatar_url = current
        .get("picture")
        .and_then(Value::as_str)
        .map(str::to_string);
    if normalized_avatar_url(current_avatar_url.as_deref())
        != normalized_avatar_url(expected_avatar_url.as_deref())
    {
        return Err("profile avatar changed before deferred save".to_string());
    }

    let builder = build_deferred_profile_event(&current, &avatar_url, prior_event)?;
    submit_event_at_with_keys(builder, &state, &api_base_url, &signer).await?;

    let events = query_relay_at_with_keys(&state, &api_base_url, &[filter], &signer, None).await?;
    Ok(events
        .first()
        .map(nostr_convert::profile_info_from_event)
        .transpose()?
        .unwrap_or_else(|| empty_profile_info(&expected_pubkey)))
}

fn build_deferred_profile_event(
    current: &Value,
    avatar_url: &str,
    prior_event: Option<&nostr::Event>,
) -> Result<nostr::EventBuilder, String> {
    let display_name = current.get("display_name").and_then(Value::as_str);
    let name = current.get("name").and_then(Value::as_str);
    let about = current.get("about").and_then(Value::as_str);
    let nip05 = current.get("nip05").and_then(Value::as_str);

    Ok(
        events::build_profile(display_name, name, Some(avatar_url), about, nip05)?
            .custom_created_at(monotonic_created_at(
                prior_event.map(|event| event.created_at.as_secs() as i64),
            )),
    )
}

fn capture_expected_signer(state: &AppState, expected_pubkey: &str) -> Result<nostr::Keys, String> {
    let signer = state.signing_keys()?;
    if signer.public_key().to_hex() != expected_pubkey {
        return Err("profile identity changed before avatar save".to_string());
    }
    Ok(signer)
}

fn normalized_avatar_url(avatar_url: Option<&str>) -> Option<&str> {
    avatar_url.map(str::trim).filter(|value| !value.is_empty())
}

#[tauri::command]
pub async fn get_user_profile(
    pubkey: Option<String>,
    state: State<'_, AppState>,
) -> Result<ProfileInfo, String> {
    let target = match pubkey {
        Some(pk) => pk,
        None => current_pubkey_hex(&state)?,
    };

    let events = query_relay(
        &state,
        &[serde_json::json!({
            "kinds": [0],
            "authors": [target.clone()],
            "limit": 1
        })],
    )
    .await?;

    Ok(events
        .first()
        .map(nostr_convert::profile_info_from_event)
        .transpose()?
        .unwrap_or_else(|| empty_profile_info(&target)))
}

/// Most filters one batch profile read sends: the relay's REQ limit
/// (`MAX_FILTERS_PER_REQ`).
const USERS_BATCH_MAX_FILTERS: usize = 10;

/// How far before a profile's last check a read looks again (seconds):
/// covers clock skew between the device that checked and the one that
/// edited. Profiles edited in that window come back and are dropped unless
/// newer than the one held.
const USERS_BATCH_VERIFY_MARGIN_SECS: i64 = 86_400;

/// A profile the caller already holds.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownProfile {
    /// `created_at` of the kind:0 event it came from.
    pub created_at: i64,
    /// This device's clock (unix seconds) when the relay last answered a
    /// read covering it.
    pub verified_at: i64,
    /// Id (hex) of that kind:0 event, for the relay's same-second tie-break.
    pub event_id: String,
}

impl KnownProfile {
    fn is_valid(&self) -> bool {
        self.created_at >= 0
            && self.verified_at >= 0
            && self.event_id.len() == 64
            && self
                .event_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}

/// Filters for one batch profile read. Pubkeys with no known profile are
/// asked for in full, in one filter. Known pubkeys are asked only for what
/// changed since their last check: sorted by `verified_at`, split into the
/// remaining filter slots at the largest gaps (so pubkeys checked in the
/// same launch share a group), each group asking `since` its oldest check
/// minus [`USERS_BATCH_VERIFY_MARGIN_SECS`].
fn users_batch_filters(pubkeys: &[String], known: &HashMap<String, KnownProfile>) -> Vec<Value> {
    let mut unknown = Vec::new();
    let mut held = Vec::new();
    for pubkey in pubkeys {
        match known.get(pubkey) {
            Some(profile) => held.push((profile.verified_at, pubkey)),
            None => unknown.push(pubkey),
        }
    }
    let mut filters = Vec::new();
    if !unknown.is_empty() {
        filters.push(serde_json::json!({ "kinds": [0], "authors": unknown }));
    }
    held.sort();
    let slots = USERS_BATCH_MAX_FILTERS - filters.len();
    for group in known_profile_groups(&held, slots) {
        let authors: Vec<&String> = group.iter().map(|(_, pubkey)| *pubkey).collect();
        filters.push(serde_json::json!({
            "kinds": [0],
            "authors": authors,
            "since": (group[0].0 - USERS_BATCH_VERIFY_MARGIN_SECS).max(0),
        }));
    }
    filters
}

/// Splits `held` (sorted by `verified_at`) into at most `slots` contiguous
/// groups, cutting at the largest gaps between consecutive values.
fn known_profile_groups<T>(held: &[(i64, T)], slots: usize) -> Vec<&[(i64, T)]> {
    if held.is_empty() || slots == 0 {
        return Vec::new();
    }
    // (gap, index the next group would start at)
    let mut cuts: Vec<(i64, usize)> = (1..held.len())
        .map(|index| (held[index].0 - held[index - 1].0, index))
        .filter(|(gap, _)| *gap > 0)
        .collect();
    cuts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut starts: Vec<usize> = std::iter::once(0)
        .chain(cuts.iter().take(slots - 1).map(|(_, start)| *start))
        .collect();
    starts.sort_unstable();
    starts
        .iter()
        .enumerate()
        .map(|(i, start)| &held[*start..starts.get(i + 1).copied().unwrap_or(held.len())])
        .collect()
}

/// Reads other people's profiles. `known` holds the profiles the caller
/// already has: only what changed since their last check is fetched, an
/// answer not newer than the held `created_at` is dropped, and a known
/// pubkey with nothing newer comes back `unchanged`, not `missing`.
#[tauri::command]
pub async fn get_users_batch(
    pubkeys: Vec<String>,
    known: Option<HashMap<String, KnownProfile>>,
    state: State<'_, AppState>,
) -> Result<UsersBatchResponse, String> {
    if pubkeys.is_empty() {
        return Ok(UsersBatchResponse {
            profiles: HashMap::new(),
            missing: Vec::new(),
            unchanged: Vec::new(),
        });
    }
    let requested: std::collections::HashSet<&String> = pubkeys.iter().collect();
    let known: HashMap<String, KnownProfile> = known
        .unwrap_or_default()
        .into_iter()
        .filter(|(pubkey, profile)| profile.is_valid() && requested.contains(pubkey))
        .collect();
    let events = query_relay(&state, &users_batch_filters(&pubkeys, &known)).await?;
    let known_created_at: HashMap<String, (i64, String)> = known
        .into_iter()
        .map(|(pubkey, profile)| (pubkey, (profile.created_at, profile.event_id)))
        .collect();

    Ok(nostr_convert::users_batch_from_events(
        &events,
        &pubkeys,
        &known_created_at,
    ))
}

#[tauri::command]
pub async fn get_user_notes(
    pubkey: String,
    limit: Option<u32>,
    before: Option<i64>,
    before_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<UserNotesResponse, String> {
    let _ = before_id; // pure-nostr filter does not use the id-based cursor
    let mut filter = serde_json::Map::new();
    filter.insert("kinds".to_string(), serde_json::json!([1]));
    filter.insert("authors".to_string(), serde_json::json!([pubkey]));
    filter.insert(
        "limit".to_string(),
        serde_json::json!(limit.unwrap_or(20).min(100)),
    );
    if let Some(t) = before {
        filter.insert("until".to_string(), serde_json::json!(t));
    }

    let events = query_relay(&state, &[Value::Object(filter)]).await?;
    Ok(nostr_convert::user_notes_from_events(&events))
}

fn build_user_search_filter(query: &str, limit: usize, page: u32) -> serde_json::Value {
    serde_json::json!({
        "kinds": [0],
        "search": query,
        "search_mode": "prefix",
        "limit": limit,
        "page": page,
    })
}

#[tauri::command]
pub async fn search_users(
    query: String,
    limit: Option<u32>,
    cursor: Option<String>,
    state: State<'_, AppState>,
) -> Result<SearchUsersResponse, String> {
    let trimmed = query.trim();
    let max = limit.unwrap_or(8).min(500) as usize;
    let page = cursor
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);

    if max == 0 {
        return Ok(SearchUsersResponse {
            users: Vec::new(),
            next_cursor: None,
        });
    }

    if trimmed.is_empty() {
        let events = query_relay(
            &state,
            &[serde_json::json!({
                "kinds": [0],
                "limit": max,
                "page": page,
            })],
        )
        .await?;

        // Emit a real next page cursor when the relay returned a full page, so
        // the empty-query people directory can page past its first page (the
        // relay honors `page`→offset for this non-search kind:0 listing). The
        // raw `events.len()` is the correct fullness signal — `list_user_search_results`
        // dedupes/truncates, so its output length can undercount a full page.
        let mut response = nostr_convert::list_user_search_results(&events, max);
        if events.len() >= max {
            response.next_cursor = Some((page + 1).to_string());
        }
        return Ok(response);
    }

    // NIP-50 full-text search on kind:0 profiles. The relay's HTTP bridge
    // intercepts the `search` field on POST /query and routes to Postgres FTS
    // (see `crates/kuzz-relay/src/api/bridge.rs::handle_bridge_search`),
    // so we get indexed, server-side search instead of fetching every kind:0
    // and scanning client-side. The old path was capped at 2000 kind:0 events
    // by the relay's HTTP bridge limit, which silently hid users on busy relays.
    //
    // We fetch one bounded page (bridge accepts up to 500) and re-rank that page
    // locally because the relay scores FTS rank against the whole kind:0 JSON
    // `content` blob, where a hit in `display_name` is not weighted any higher
    // than a substring hit in `about`. The caller can request later pages via the
    // cursor so the UI cap is only a page size, not a terminal directory ceiling.
    //
    // `search_mode: "prefix"` matters: every caller of this command is a
    // typeahead surface (member picker, @mention popup, DM recipient search,
    // topbar people results), so a partially typed name must match. Without it
    // the relay runs whole-word `websearch_to_tsquery` matching and "tyl"
    // returns zero results for "Tyler". Same bridge-only extension the topbar
    // message search uses (see `build_search_messages_filter`).
    let events = query_relay(&state, &[build_user_search_filter(trimmed, max, page)]).await?;

    let mut response = nostr_convert::rank_user_search_results(&events, trimmed, max);
    if events.len() >= max {
        response.next_cursor = Some((page + 1).to_string());
    }
    Ok(response)
}

#[tauri::command]
pub async fn get_presence(
    pubkeys: Vec<String>,
    state: State<'_, AppState>,
) -> Result<HashMap<String, PresenceStatus>, String> {
    if pubkeys.is_empty() {
        return Ok(HashMap::new());
    }

    // Presence is published as kind:20001 ephemeral events. Query the most
    // recent per author. Only a successful empty snapshot establishes absence;
    // transport/auth/storage failures must reject so consumers remain unknown.
    let events = query_relay(
        &state,
        &[serde_json::json!({
            "kinds": [20001],
            "authors": pubkeys,
        })],
    )
    .await?;

    let mut latest: HashMap<String, (u64, PresenceStatus)> = HashMap::new();
    for ev in &events {
        // Relay-synthesized presence events use a p-tag to identify the subject.
        // Self-signed presence events (live WS) use the event author directly.
        let pk = ev
            .tags
            .iter()
            .find_map(|t| {
                let s = t.as_slice();
                if s.len() >= 2 && s[0] == "p" {
                    Some(s[1].clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| ev.pubkey.to_hex());
        let ts = ev.created_at.as_secs();
        let status = match ev.content.trim() {
            "online" => PresenceStatus::Online,
            "away" => PresenceStatus::Away,
            "offline" => PresenceStatus::Offline,
            _ => continue,
        };
        match latest.get(&pk) {
            Some((prev_ts, _)) if *prev_ts >= ts => {}
            _ => {
                latest.insert(pk, (ts, status));
            }
        }
    }

    Ok(latest
        .into_iter()
        .map(|(pk, (_, status))| (pk, status))
        .collect())
}

fn current_pubkey_hex(state: &AppState) -> Result<String, String> {
    Ok(state.current_identity_pubkey()?.to_hex())
}

fn current_pubkey_hex_unwrap(state: &AppState) -> String {
    current_pubkey_hex(state).unwrap_or_default()
}

fn empty_profile_info(pubkey: &str) -> ProfileInfo {
    ProfileInfo {
        pubkey: pubkey.to_string(),
        display_name: None,
        avatar_url: None,
        about: None,
        nip05_handle: None,
        owner_pubkey: None,
        has_profile_event: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_profile_signer_is_captured_and_rejects_wrong_identity() {
        let state = crate::app_state::build_app_state();
        let original = state.signing_keys().expect("signable identity");
        let original_pubkey = original.public_key().to_hex();

        let captured = capture_expected_signer(&state, &original_pubkey)
            .expect("matching identity should be captured");
        *state.keys.lock().expect("lock keys") = nostr::Keys::generate();

        assert_eq!(captured.public_key().to_hex(), original_pubkey);
        assert_ne!(
            state.keys.lock().expect("lock keys").public_key().to_hex(),
            original_pubkey
        );
        assert_eq!(
            capture_expected_signer(&state, &original_pubkey).unwrap_err(),
            "profile identity changed before avatar save"
        );
    }

    #[test]
    fn deferred_profile_event_is_strictly_newer_than_prior_head() {
        let keys = nostr::Keys::generate();
        let prior_created_at = nostr::Timestamp::now().as_secs() + 60;
        let prior_event = nostr::EventBuilder::new(
            nostr::Kind::Metadata,
            serde_json::json!({"display_name": "Larry"}).to_string(),
        )
        .custom_created_at(nostr::Timestamp::from(prior_created_at))
        .sign_with_keys(&keys)
        .expect("sign prior profile");

        let builder = build_deferred_profile_event(
            &serde_json::json!({"display_name": "Larry"}),
            "https://example.com/avatar.png",
            Some(&prior_event),
        )
        .expect("build deferred profile");
        let event = builder
            .sign_with_keys(&keys)
            .expect("sign deferred profile");

        assert_eq!(event.created_at.as_secs(), prior_created_at + 1);
        assert_eq!(
            serde_json::from_str::<Value>(&event.content).unwrap()["picture"],
            "https://example.com/avatar.png"
        );
    }

    #[test]
    fn user_search_filter_requests_prefix_mode_for_typeahead() {
        // Every caller of `search_users` is a typeahead surface. Whole-word
        // FTS matching returns zero results for a partially typed name
        // ("tyl" for "Tyler"), which reads as "user doesn't exist" in the
        // member picker and @mention popup. Pin the mode so it can't drift.
        let filter = build_user_search_filter("tyl", 25, 1);

        assert_eq!(filter["search"], serde_json::json!("tyl"));
        assert_eq!(filter["search_mode"], serde_json::json!("prefix"));
        assert_eq!(filter["limit"], serde_json::json!(25));
        assert_eq!(filter["page"], serde_json::json!(1));
    }

    fn pubkeys(count: usize) -> Vec<String> {
        (0..count).map(|i| format!("{i:064x}")).collect()
    }

    fn known_at(verified_at: i64) -> KnownProfile {
        KnownProfile {
            created_at: 1,
            verified_at,
            event_id: "a".repeat(64),
        }
    }

    #[test]
    fn known_profiles_need_a_hex_event_id() {
        assert!(known_at(1).is_valid());
        for event_id in [String::new(), "A".repeat(64), "a".repeat(63)] {
            assert!(!KnownProfile {
                event_id,
                ..known_at(1)
            }
            .is_valid());
        }
    }

    #[test]
    fn users_batch_filters_ask_unknown_pubkeys_in_full() {
        let all = pubkeys(3);
        let filters = users_batch_filters(&all, &HashMap::new());

        assert_eq!(
            filters,
            vec![serde_json::json!({ "kinds": [0], "authors": all })]
        );
    }

    #[test]
    fn users_batch_filters_ask_known_pubkeys_since_a_day_before_their_check() {
        let all = pubkeys(5);
        let launch = 1_800_000_000;
        let known = HashMap::from([
            (all[1].clone(), known_at(launch + 2)),
            (all[2].clone(), known_at(launch)),
            (all[3].clone(), known_at(1_000)),
        ]);
        let filters = users_batch_filters(&all, &known);

        assert_eq!(
            filters,
            vec![
                serde_json::json!({ "kinds": [0], "authors": [all[0], all[4]] }),
                serde_json::json!({ "kinds": [0], "authors": [all[3]], "since": 0 }),
                serde_json::json!({
                    "kinds": [0],
                    "authors": [all[2]],
                    "since": launch - 86_400,
                }),
                serde_json::json!({
                    "kinds": [0],
                    "authors": [all[1]],
                    "since": launch + 2 - 86_400,
                }),
            ]
        );
    }

    #[test]
    fn users_batch_filters_ask_a_launch_since_a_day_before_it() {
        let all = pubkeys(200);
        let launch = 1_800_000_000;
        // Everyone checked within a few seconds at the last launch.
        let known: HashMap<String, KnownProfile> = all
            .iter()
            .enumerate()
            .map(|(i, pubkey)| (pubkey.clone(), known_at(launch + (i % 3) as i64)))
            .collect();
        let filters = users_batch_filters(&all, &known);

        assert!(filters.len() <= USERS_BATCH_MAX_FILTERS);
        for filter in &filters {
            let since = filter["since"].as_i64().unwrap();
            assert!((launch - 86_400..=launch + 2 - 86_400).contains(&since));
        }
    }

    #[test]
    fn users_batch_filters_stay_within_the_relay_filter_limit() {
        let all = pubkeys(200);
        let verified_at = |i: usize| 1_000_000 + (i as i64) * 7_919 % 100_003;
        let some_known: HashMap<String, KnownProfile> = all[..150]
            .iter()
            .enumerate()
            .map(|(i, pubkey)| (pubkey.clone(), known_at(verified_at(i))))
            .collect();
        let all_known: HashMap<String, KnownProfile> = all
            .iter()
            .enumerate()
            .map(|(i, pubkey)| (pubkey.clone(), known_at(verified_at(i))))
            .collect();

        for (known, full_filters, filter_count) in [
            (HashMap::new(), 1, 1),
            (some_known, 1, USERS_BATCH_MAX_FILTERS),
            (all_known, 0, USERS_BATCH_MAX_FILTERS),
        ] {
            let filters = users_batch_filters(&all, &known);
            assert_eq!(filters.len(), filter_count);
            let asked: usize = filters
                .iter()
                .map(|filter| filter["authors"].as_array().unwrap().len())
                .sum();
            assert_eq!(asked, all.len());
            assert_eq!(
                filters.iter().filter(|f| f.get("since").is_none()).count(),
                full_filters
            );
            for filter in filters.iter().filter(|f| f.get("since").is_some()) {
                let oldest = filter["authors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pubkey| known[pubkey.as_str().unwrap()].verified_at)
                    .min()
                    .unwrap();
                assert_eq!(filter["since"], serde_json::json!(oldest - 86_400));
            }
        }
    }

    #[test]
    fn known_profile_groups_cut_at_the_largest_gaps() {
        let held: Vec<(i64, ())> = [10, 11, 12, 500, 501, 900, 2_000, 2_001]
            .into_iter()
            .map(|at| (at, ()))
            .collect();
        let groups: Vec<Vec<i64>> = known_profile_groups(&held, 3)
            .into_iter()
            .map(|group| group.iter().map(|(at, _)| *at).collect())
            .collect();

        assert_eq!(
            groups,
            vec![vec![10, 11, 12], vec![500, 501, 900], vec![2_000, 2_001]]
        );
        // Equal values are never split.
        let same: Vec<(i64, ())> = vec![(5, ()); 4];
        assert_eq!(known_profile_groups(&same, 10).len(), 1);
    }

    #[tokio::test]
    async fn update_profile_at_relay_refuses_another_server() {
        use tauri::Manager as _;
        let app = crate::relay::server::refusal_test_app();
        let error = update_profile_at_relay(
            "wss://other.example".into(),
            "a".repeat(64),
            None,
            String::new(),
            app.state(),
        )
        .await
        .err()
        .expect("refused");
        assert_eq!(error, crate::relay::server::REFUSED);
    }
}

#[cfg(test)]
#[path = "profile_presence_tests.rs"]
mod presence_tests;
