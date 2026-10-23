//! Chats server state (`chat_list`, `mark_read`, `thread_follow`,
//! `chat_import`, `thread_search`) over the relay's `/query` bridge.
//!
//! A relay with the chat-state switch off answers
//! `404 {"error": "chat_state_disabled"}`; a relay without the envelopes
//! ignores the unknown keys and answers a plain event array. Both become the
//! single error [`CHAT_STATE_UNAVAILABLE`] so the app keeps today's
//! device-local behavior.
//!
//! Every command names the account it was queued for ([`ChatStateBinding`]).
//! The relay and the signer are each read once and checked against it; on a
//! mismatch nothing is sent and the command fails with
//! [`CHAT_STATE_ACCOUNT_CHANGED`], so writes queued for one account can never
//! land under another account or on another community's relay.
//!
//! Read positions and follows are private per-user state: never log them.

use tauri::State;

use crate::{
    app_state::AppState,
    relay::{
        assert_expected_relay_scope, assert_expected_signer, query_relay_value_as,
        relay_api_base_url_with_override,
    },
};

/// Error returned when the relay keeps no chat state for this user.
pub const CHAT_STATE_UNAVAILABLE: &str = "chat_state_unavailable";
/// Error returned, before any request, when the active relay or signer is no
/// longer the account the request was queued for.
pub const CHAT_STATE_ACCOUNT_CHANGED: &str = "chat_state_account_changed";
/// Largest `chat_list` page the relay serves.
const MAX_CHAT_LIST_LIMIT: u32 = 200;
/// Most items one `mark_read` request may carry.
const MAX_MARK_READ_ITEMS: usize = 100;
/// Most pinned thread ids one `chat_list` request may name.
const MAX_PINNED_THREADS: usize = 200;
/// Most reads plus follows one `chat_import` request may carry.
const MAX_CHAT_IMPORT_ITEMS: usize = 500;
/// Most threads one `thread_search` request may ask for.
const MAX_THREAD_SEARCH_LIMIT: u32 = 50;
/// Longest `thread_search` query the relay accepts, in characters.
const MAX_THREAD_SEARCH_QUERY_CHARS: usize = 100;

/// The account a chat-state request belongs to: the community relay and the
/// user pubkey the caller captured when it queued the request.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStateBinding {
    relay_url: String,
    pubkey: String,
}

/// Keyset cursor returned by the relay as `next`.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct ChatListCursor {
    at_us: i64,
    key: String,
}

/// One room main timeline (`channel`) or thread (`thread`, by root id), read
/// up to `at` (unix seconds). Without `at` only the unread mark changes.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMarkReadItem {
    kind: ChatMarkReadKind,
    id: String,
    #[serde(default)]
    at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatMarkReadKind {
    Channel,
    Thread,
}

/// Explicit follow state for `chat_thread_follow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatFollowState {
    Followed,
    Unfollowed,
}

/// One device-local read position uploaded by `chat_import`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatImportRead {
    kind: ChatMarkReadKind,
    id: String,
    at: u64,
}

/// One device-local follow choice uploaded by `chat_import`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatImportFollow {
    root: String,
    state: ChatFollowState,
}

impl ChatFollowState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Followed => "followed",
            Self::Unfollowed => "unfollowed",
        }
    }
}

/// Refuse a request whose account is not the active relay and signer. A blank
/// binding never matches.
fn check_binding(
    binding: &ChatStateBinding,
    relay_base: &str,
    signer_hex: &str,
) -> Result<(), String> {
    let relay = binding.relay_url.trim();
    let pubkey = binding.pubkey.trim();
    let matches = !relay.is_empty()
        && !pubkey.is_empty()
        && assert_expected_relay_scope(Some(relay), relay_base).is_ok()
        && assert_expected_signer(Some(pubkey), signer_hex).is_ok();
    if matches {
        Ok(())
    } else {
        Err(CHAT_STATE_ACCOUNT_CHANGED.to_string())
    }
}

fn build_chat_list_filter(
    limit: u32,
    before: Option<&ChatListCursor>,
    pinned_threads: &[String],
) -> serde_json::Value {
    let mut filter = serde_json::json!({
        "chat_list": 1,
        "limit": limit.clamp(1, MAX_CHAT_LIST_LIMIT),
    });
    if let Some(before) = before {
        filter["before"] = serde_json::json!(before);
    }
    if !pinned_threads.is_empty() {
        let pinned: Vec<&String> = pinned_threads.iter().take(MAX_PINNED_THREADS).collect();
        filter["pinned_threads"] = serde_json::json!(pinned);
    }
    filter
}

/// Owner view: one page of `agent`'s DMs and group chats the owner is not
/// in. The relay checks that the signer owns the agent.
fn build_agent_chat_list_filter(
    agent: &str,
    limit: u32,
    before: Option<&ChatListCursor>,
) -> Result<serde_json::Value, String> {
    if agent.len() != 64 || !agent.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("agent must be a 64-hex pubkey".to_string());
    }
    let mut filter = build_chat_list_filter(limit, before, &[]);
    filter["as_agent"] = serde_json::json!(agent.to_ascii_lowercase());
    Ok(filter)
}

/// The wire shape of one `mark_read` item. Kept in one place: the relay
/// takes `{"channel": uuid, "at": secs}` / `{"thread": root, "at": secs}`,
/// and no position when only the unread mark changes.
fn mark_read_item_json(item: &ChatMarkReadItem) -> serde_json::Value {
    let target = match item.kind {
        ChatMarkReadKind::Channel => "channel",
        ChatMarkReadKind::Thread => "thread",
    };
    let mut value = serde_json::json!({ target: item.id });
    if let Some(at) = item.at {
        value["at"] = serde_json::json!(at);
    }
    value
}

fn build_mark_read_filter(
    items: &[ChatMarkReadItem],
    unread_mark: bool,
) -> Result<serde_json::Value, String> {
    if items.is_empty() || items.len() > MAX_MARK_READ_ITEMS {
        return Err(format!("mark_read takes 1..={MAX_MARK_READ_ITEMS} items"));
    }
    Ok(serde_json::json!({
        "mark_read": 1,
        "items": items.iter().map(mark_read_item_json).collect::<Vec<_>>(),
        "unread_mark": unread_mark,
    }))
}

fn build_thread_follow_filter(root: &str, state: ChatFollowState) -> serde_json::Value {
    serde_json::json!({
        "thread_follow": 1,
        "root": root,
        "state": state.as_str(),
    })
}

/// `{"chat_import": 1, "reads": [{"channel"|"thread": id, "at": secs}],
/// "follows": [{"root": id, "state": ..}]}`, 1..=500 items in total.
fn build_chat_import_filter(
    reads: &[ChatImportRead],
    follows: &[ChatImportFollow],
) -> Result<serde_json::Value, String> {
    let total = reads.len() + follows.len();
    if total == 0 || total > MAX_CHAT_IMPORT_ITEMS {
        return Err(format!(
            "chat_import takes 1..={MAX_CHAT_IMPORT_ITEMS} reads and follows"
        ));
    }
    let reads: Vec<serde_json::Value> = reads
        .iter()
        .map(|read| {
            let target = match read.kind {
                ChatMarkReadKind::Channel => "channel",
                ChatMarkReadKind::Thread => "thread",
            };
            serde_json::json!({ target: read.id, "at": read.at })
        })
        .collect();
    let follows: Vec<serde_json::Value> = follows
        .iter()
        .map(|follow| serde_json::json!({ "root": follow.root, "state": follow.state.as_str() }))
        .collect();
    Ok(serde_json::json!({ "chat_import": 1, "reads": reads, "follows": follows }))
}

/// `{"thread_search": 1, "q": query, "limit": n}`. The query is trimmed and
/// must be 1..=100 characters.
fn build_thread_search_filter(query: &str, limit: u32) -> Result<serde_json::Value, String> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > MAX_THREAD_SEARCH_QUERY_CHARS {
        return Err(format!(
            "thread_search query must be 1..={MAX_THREAD_SEARCH_QUERY_CHARS} characters"
        ));
    }
    Ok(serde_json::json!({
        "thread_search": 1,
        "q": query,
        "limit": limit.clamp(1, MAX_THREAD_SEARCH_LIMIT),
    }))
}

/// `404 chat_state_disabled` means the switch is off: no chat state here.
fn map_query_error(error: String) -> String {
    if error.starts_with("relay returned 404") && error.contains("chat_state_disabled") {
        CHAT_STATE_UNAVAILABLE.to_string()
    } else {
        error
    }
}

/// Accept only a version-1 `envelope` response carrying `array_key` as an
/// array (when given); anything else (an old relay's event array) means the
/// relay keeps no chat state.
fn validated_envelope(
    response: serde_json::Value,
    envelope: &str,
    array_key: Option<&str>,
) -> Result<serde_json::Value, String> {
    let versioned = response.get(envelope) == Some(&serde_json::json!(1));
    let has_array =
        array_key.is_none_or(|key| response.get(key).is_some_and(serde_json::Value::is_array));
    if versioned && has_array {
        Ok(response)
    } else {
        Err(CHAT_STATE_UNAVAILABLE.to_string())
    }
}

async fn query_envelope(
    state: &AppState,
    binding: &ChatStateBinding,
    filter: serde_json::Value,
    envelope: &str,
    array_key: Option<&str>,
) -> Result<serde_json::Value, String> {
    // One read of each, checked, then used for the request itself.
    let relay_base = relay_api_base_url_with_override(state);
    let credential = state.user_credential()?;
    check_binding(binding, &relay_base, &credential.public_key().to_hex())?;
    let response = query_relay_value_as(state, &relay_base, &credential, &[filter])
        .await
        .map_err(map_query_error)?;
    validated_envelope(response, envelope, array_key)
}

/// Fetch one page of the user's server-side chats list: rooms and live
/// threads, newest first, with unread counts. Pass the same `pinned_threads`
/// on every page.
#[tauri::command]
pub async fn get_chat_list(
    binding: ChatStateBinding,
    limit: Option<u32>,
    before: Option<ChatListCursor>,
    pinned_threads: Option<Vec<String>>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_chat_list_filter(
        limit.unwrap_or(100),
        before.as_ref(),
        &pinned_threads.unwrap_or_default(),
    );
    query_envelope(&state, &binding, filter, "chat_list", Some("rows")).await
}

/// Owner view: one page of an agent's DMs and group chats that the signed-in
/// owner is not in, read-only (no read state).
#[tauri::command]
pub async fn get_agent_chat_list(
    binding: ChatStateBinding,
    agent: String,
    limit: Option<u32>,
    before: Option<ChatListCursor>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_agent_chat_list_filter(&agent, limit.unwrap_or(100), before.as_ref())?;
    query_envelope(&state, &binding, filter, "chat_list", Some("rows")).await
}

/// Store read positions for rooms and threads (positions only move forward)
/// and set every item's unread mark to `unread_mark`.
#[tauri::command]
pub async fn chat_mark_read(
    binding: ChatStateBinding,
    items: Vec<ChatMarkReadItem>,
    unread_mark: Option<bool>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_mark_read_filter(&items, unread_mark.unwrap_or(false))?;
    query_envelope(&state, &binding, filter, "mark_read", Some("items")).await
}

/// Follow or unfollow one thread by root id.
#[tauri::command]
pub async fn chat_thread_follow(
    binding: ChatStateBinding,
    root: String,
    follow_state: ChatFollowState,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_thread_follow_filter(&root, follow_state);
    query_envelope(&state, &binding, filter, "thread_follow", None).await
}

/// First-run upload of this device's read positions and follows. The relay
/// applies each item only over its backfilled state and answers per item.
#[tauri::command]
pub async fn chat_import(
    binding: ChatStateBinding,
    reads: Vec<ChatImportRead>,
    follows: Vec<ChatImportFollow>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_chat_import_filter(&reads, &follows)?;
    query_envelope(&state, &binding, filter, "chat_import", Some("reads")).await
}

/// Threads in the user's rooms whose name contains `query`, closed and
/// quiet ones included, best match first. Rows use the `chat_list` thread
/// row fields without counts or events.
#[tauri::command]
pub async fn search_thread_names(
    binding: ChatStateBinding,
    query: String,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_thread_search_filter(&query, limit.unwrap_or(20))?;
    query_envelope(&state, &binding, filter, "thread_search", Some("rows")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_search_filter_trims_and_bounds() {
        assert_eq!(
            build_thread_search_filter("  launch ", 500).expect("ok"),
            serde_json::json!({"thread_search": 1, "q": "launch", "limit": 50})
        );
        assert_eq!(
            build_thread_search_filter("검색", 0).expect("ok")["limit"],
            1
        );
        assert!(build_thread_search_filter("   ", 20).is_err());
        assert!(build_thread_search_filter(&"x".repeat(101), 20).is_err());
        assert!(build_thread_search_filter(&"가".repeat(100), 20).is_ok());
    }

    const SIGNER: &str = "8cf9a7e7deebdc7d1fe172b9af4c6796b224e43e8f03e80dbd357f6c61fdeff9";

    fn binding(relay_url: &str, pubkey: &str) -> ChatStateBinding {
        ChatStateBinding {
            relay_url: relay_url.to_string(),
            pubkey: pubkey.to_string(),
        }
    }

    #[test]
    fn requests_for_another_account_or_relay_are_refused() {
        let base = "https://tenant-a.example";
        assert!(check_binding(&binding("wss://tenant-a.example", SIGNER), base, SIGNER).is_ok());
        assert!(check_binding(
            &binding("wss://tenant-a.example", &SIGNER.to_uppercase()),
            base,
            SIGNER
        )
        .is_ok());
        for refused in [
            binding("wss://tenant-b.example", SIGNER),
            binding("wss://tenant-a.example", &"ab".repeat(32)),
            binding("", SIGNER),
            binding("wss://tenant-a.example", " "),
        ] {
            assert_eq!(
                check_binding(&refused, base, SIGNER),
                Err(CHAT_STATE_ACCOUNT_CHANGED.to_string())
            );
        }
        // Distinct from the 4xx errors the app treats as permanent.
        assert!(!CHAT_STATE_ACCOUNT_CHANGED.starts_with("relay returned"));
        let item: ChatStateBinding = serde_json::from_value(
            serde_json::json!({"relayUrl": "wss://tenant-a.example", "pubkey": SIGNER}),
        )
        .expect("camelCase binding");
        assert!(check_binding(&item, base, SIGNER).is_ok());
    }

    #[test]
    fn disabled_switch_and_old_relays_are_unavailable() {
        assert_eq!(
            map_query_error("relay returned 404 Not Found: chat_state_disabled".to_string()),
            CHAT_STATE_UNAVAILABLE
        );
        let other = "relay returned 403 Forbidden: mark_read item 1: not a member of that room";
        assert_eq!(map_query_error(other.to_string()), other);
        assert_eq!(
            map_query_error("relay unreachable: request timed out".to_string()),
            "relay unreachable: request timed out"
        );

        let list = serde_json::json!({"chat_list": 1, "rows": [], "next": null});
        assert!(validated_envelope(list, "chat_list", Some("rows")).is_ok());
        for response in [
            serde_json::json!([]),
            serde_json::json!({"chat_list": 2, "rows": []}),
            serde_json::json!({"chat_list": 1}),
            serde_json::json!({"mark_read": 1, "items": []}),
        ] {
            assert_eq!(
                validated_envelope(response, "chat_list", Some("rows")),
                Err(CHAT_STATE_UNAVAILABLE.to_string())
            );
        }
        let follow = serde_json::json!({"thread_follow": 1, "root": "ab", "state": "followed"});
        assert!(validated_envelope(follow, "thread_follow", None).is_ok());
        assert!(validated_envelope(serde_json::json!([]), "thread_follow", None).is_err());
    }

    #[test]
    fn chat_list_filter_carries_cursor_pins_and_bounded_limit() {
        let cursor = ChatListCursor {
            at_us: 42,
            key: "k".to_string(),
        };
        let filter = build_chat_list_filter(500, Some(&cursor), &["ab".repeat(32)]);
        assert_eq!(filter["chat_list"], 1);
        assert_eq!(filter["limit"], MAX_CHAT_LIST_LIMIT);
        assert_eq!(filter["before"]["at_us"], 42);
        assert_eq!(filter["before"]["key"], "k");
        assert_eq!(filter["pinned_threads"][0], "ab".repeat(32));

        let first = build_chat_list_filter(0, None, &[]);
        assert_eq!(first["limit"], 1);
        assert!(first.get("before").is_none());
        assert!(first.get("pinned_threads").is_none());

        let many = vec!["cd".repeat(32); MAX_PINNED_THREADS + 5];
        let capped = build_chat_list_filter(10, None, &many);
        assert_eq!(
            capped["pinned_threads"].as_array().map(Vec::len),
            Some(MAX_PINNED_THREADS)
        );
    }

    #[test]
    fn agent_chat_list_filter_names_the_agent_without_pins() {
        let agent = "AB".repeat(32);
        let filter = build_agent_chat_list_filter(&agent, 50, None).expect("filter");
        assert_eq!(filter["chat_list"], 1);
        assert_eq!(filter["as_agent"], "ab".repeat(32));
        assert_eq!(filter["limit"], 50);
        assert!(filter.get("pinned_threads").is_none());
        assert!(build_agent_chat_list_filter("zz", 50, None).is_err());
        assert!(build_agent_chat_list_filter(&"g".repeat(64), 50, None).is_err());
    }

    #[test]
    fn mark_read_items_name_a_room_or_thread_with_a_time() {
        let items = vec![
            ChatMarkReadItem {
                kind: ChatMarkReadKind::Channel,
                id: "11111111-1111-4111-8111-111111111111".to_string(),
                at: Some(1_760_000_000),
            },
            ChatMarkReadItem {
                kind: ChatMarkReadKind::Thread,
                id: "cd".repeat(32),
                at: Some(1_760_000_001),
            },
        ];
        let filter = build_mark_read_filter(&items, false).expect("valid");
        assert_eq!(
            filter,
            serde_json::json!({
                "mark_read": 1,
                "items": [
                    {"channel": "11111111-1111-4111-8111-111111111111", "at": 1_760_000_000u64},
                    {"thread": "cd".repeat(32), "at": 1_760_000_001u64},
                ],
                "unread_mark": false,
            })
        );
        assert!(build_mark_read_filter(&[], false).is_err());
        let too_many: Vec<ChatMarkReadItem> = (0..=MAX_MARK_READ_ITEMS)
            .map(|index| ChatMarkReadItem {
                kind: ChatMarkReadKind::Channel,
                id: index.to_string(),
                at: Some(1),
            })
            .collect();
        assert!(build_mark_read_filter(&too_many, false).is_err());
    }

    #[test]
    fn mark_unread_sends_no_position() {
        let item: ChatMarkReadItem = serde_json::from_value(serde_json::json!({
            "kind": "channel",
            "id": "11111111-1111-4111-8111-111111111111",
        }))
        .expect("item without at");
        let filter = build_mark_read_filter(&[item], true).expect("valid");
        assert_eq!(
            filter,
            serde_json::json!({
                "mark_read": 1,
                "items": [{"channel": "11111111-1111-4111-8111-111111111111"}],
                "unread_mark": true,
            })
        );
    }

    #[test]
    fn chat_import_filter_carries_reads_and_follows_up_to_the_cap() {
        let reads: Vec<ChatImportRead> = serde_json::from_value(serde_json::json!([
            {"kind": "channel", "id": "11111111-1111-4111-8111-111111111111", "at": 7},
            {"kind": "thread", "id": "ab".repeat(32), "at": 8},
        ]))
        .expect("camelCase reads");
        let follows: Vec<ChatImportFollow> = serde_json::from_value(serde_json::json!([
            {"root": "ab".repeat(32), "state": "unfollowed"},
        ]))
        .expect("follows");
        assert_eq!(
            build_chat_import_filter(&reads, &follows).expect("valid"),
            serde_json::json!({
                "chat_import": 1,
                "reads": [
                    {"channel": "11111111-1111-4111-8111-111111111111", "at": 7},
                    {"thread": "ab".repeat(32), "at": 8},
                ],
                "follows": [{"root": "ab".repeat(32), "state": "unfollowed"}],
            })
        );
        assert!(build_chat_import_filter(&[], &[]).is_err());
        let follow = |n: usize| ChatImportFollow {
            root: format!("{n:064x}"),
            state: ChatFollowState::Followed,
        };
        let at_cap: Vec<ChatImportFollow> = (0..MAX_CHAT_IMPORT_ITEMS).map(follow).collect();
        assert!(build_chat_import_filter(&[], &at_cap).is_ok());
        assert!(build_chat_import_filter(&reads, &at_cap).is_err());
        let missing_at = serde_json::from_value::<ChatImportRead>(
            serde_json::json!({"kind": "channel", "id": "x"}),
        );
        assert!(missing_at.is_err(), "an import read always has a time");
    }

    #[test]
    fn thread_follow_filter_names_root_and_state() {
        let filter = build_thread_follow_filter("ef", ChatFollowState::Unfollowed);
        assert_eq!(
            filter,
            serde_json::json!({"thread_follow": 1, "root": "ef", "state": "unfollowed"})
        );
        let item: ChatMarkReadItem =
            serde_json::from_value(serde_json::json!({"kind": "thread", "id": "ab", "at": 3}))
                .expect("camelCase item");
        assert_eq!(item.kind, ChatMarkReadKind::Thread);
        assert_eq!(item.at, Some(3));
        let state: ChatFollowState =
            serde_json::from_value(serde_json::json!("followed")).expect("state");
        assert_eq!(state, ChatFollowState::Followed);
    }
}
