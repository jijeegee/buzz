//! Chats server state (`chat_list`, `mark_read`, `thread_follow`) over the
//! relay's `/query` bridge.
//!
//! A relay with the chat-state switch off answers
//! `404 {"error": "chat_state_disabled"}`; a relay without the envelopes
//! ignores the unknown keys and answers a plain event array. Both become the
//! single error [`CHAT_STATE_UNAVAILABLE`] so the app keeps today's
//! device-local behavior.
//!
//! Read positions and follows are private per-user state: never log them.

use tauri::State;

use crate::{app_state::AppState, relay::query_relay_value};

/// Error returned when the relay keeps no chat state for this user.
pub const CHAT_STATE_UNAVAILABLE: &str = "chat_state_unavailable";
/// Largest `chat_list` page the relay serves.
const MAX_CHAT_LIST_LIMIT: u32 = 200;
/// Most items one `mark_read` request may carry.
const MAX_MARK_READ_ITEMS: usize = 100;
/// Most pinned thread ids one `chat_list` request may name.
const MAX_PINNED_THREADS: usize = 200;

/// Keyset cursor returned by the relay as `next`.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct ChatListCursor {
    at_us: i64,
    key: String,
}

/// One read position to store: a room's main timeline (`channel`) or a
/// thread (`thread`, by root id), read up to `at` (unix seconds).
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMarkReadItem {
    kind: ChatMarkReadKind,
    id: String,
    at: u64,
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

impl ChatFollowState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Followed => "followed",
            Self::Unfollowed => "unfollowed",
        }
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

/// The wire shape of one `mark_read` item. Kept in one place: the relay
/// takes `{"channel": uuid, "at": secs}` / `{"thread": root, "at": secs}`.
fn mark_read_item_json(item: &ChatMarkReadItem) -> serde_json::Value {
    let target = match item.kind {
        ChatMarkReadKind::Channel => "channel",
        ChatMarkReadKind::Thread => "thread",
    };
    serde_json::json!({ target: item.id, "at": item.at })
}

fn build_mark_read_filter(items: &[ChatMarkReadItem]) -> Result<serde_json::Value, String> {
    if items.is_empty() || items.len() > MAX_MARK_READ_ITEMS {
        return Err(format!("mark_read takes 1..={MAX_MARK_READ_ITEMS} items"));
    }
    Ok(serde_json::json!({
        "mark_read": 1,
        "items": items.iter().map(mark_read_item_json).collect::<Vec<_>>(),
    }))
}

fn build_thread_follow_filter(root: &str, state: ChatFollowState) -> serde_json::Value {
    serde_json::json!({
        "thread_follow": 1,
        "root": root,
        "state": state.as_str(),
    })
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
    filter: serde_json::Value,
    envelope: &str,
    array_key: Option<&str>,
) -> Result<serde_json::Value, String> {
    let response = query_relay_value(state, &[filter])
        .await
        .map_err(map_query_error)?;
    validated_envelope(response, envelope, array_key)
}

/// Fetch one page of the user's server-side chats list: rooms and live
/// threads, newest first, with unread counts. Pass the same `pinned_threads`
/// on every page.
#[tauri::command]
pub async fn get_chat_list(
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
    query_envelope(&state, filter, "chat_list", Some("rows")).await
}

/// Store read positions for rooms and threads. Positions only move forward.
#[tauri::command]
pub async fn chat_mark_read(
    items: Vec<ChatMarkReadItem>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_mark_read_filter(&items)?;
    query_envelope(&state, filter, "mark_read", Some("items")).await
}

/// Follow or unfollow one thread by root id.
#[tauri::command]
pub async fn chat_thread_follow(
    root: String,
    follow_state: ChatFollowState,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_thread_follow_filter(&root, follow_state);
    query_envelope(&state, filter, "thread_follow", None).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn mark_read_items_name_a_room_or_thread_with_a_time() {
        let items = vec![
            ChatMarkReadItem {
                kind: ChatMarkReadKind::Channel,
                id: "11111111-1111-4111-8111-111111111111".to_string(),
                at: 1_760_000_000,
            },
            ChatMarkReadItem {
                kind: ChatMarkReadKind::Thread,
                id: "cd".repeat(32),
                at: 1_760_000_001,
            },
        ];
        let filter = build_mark_read_filter(&items).expect("valid");
        assert_eq!(
            filter,
            serde_json::json!({
                "mark_read": 1,
                "items": [
                    {"channel": "11111111-1111-4111-8111-111111111111", "at": 1_760_000_000u64},
                    {"thread": "cd".repeat(32), "at": 1_760_000_001u64},
                ],
            })
        );
        assert!(build_mark_read_filter(&[]).is_err());
        let too_many: Vec<ChatMarkReadItem> = (0..=MAX_MARK_READ_ITEMS)
            .map(|index| ChatMarkReadItem {
                kind: ChatMarkReadKind::Channel,
                id: index.to_string(),
                at: 1,
            })
            .collect();
        assert!(build_mark_read_filter(&too_many).is_err());
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
        let state: ChatFollowState =
            serde_json::from_value(serde_json::json!("followed")).expect("state");
        assert_eq!(state, ChatFollowState::Followed);
    }
}
