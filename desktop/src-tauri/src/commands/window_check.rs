//! Store-first windows checked against the relay
//! (docs/message-sync.md#window-check): the relay's check of one window
//! (its oldest row id and row count, no rows), the same window as the store
//! holds it, and replacing the stored rows with a page fetched after a
//! mismatch.

use tauri::State;

use crate::{
    app_state::AppState,
    models::{ChannelPageCursor, ThreadCursor},
    relay::{query_relay, relay_api_base_url_with_override},
};

use super::channel_window::{build_channel_window_filter, TIMELINE_KINDS};
use super::message_store_coverage::{off_main, paint_scope};
use super::thread_window::{thread_window_filter, THREAD_ROW_KINDS};

/// Bounds kinds that close a room window and a thread window.
const KIND_ROOM_BOUNDS: u16 = 39006;
const KIND_THREAD_BOUNDS: u16 = 39007;

fn window_kinds(root: Option<&str>) -> Vec<u64> {
    let kinds: &[u32] = if root.is_some() {
        &THREAD_ROW_KINDS
    } else {
        &TIMELINE_KINDS
    };
    kinds.iter().map(|&kind| u64::from(kind)).collect()
}

fn window_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(50).clamp(1, 200)
}

/// The relay's check of one window, from its bounds' `check` object.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct WindowCheckAnswer {
    oldest_id: Option<String>,
    oldest_created_at: Option<i64>,
    count: u64,
    room_seq: Option<i64>,
    mode: String,
}

/// The filter of a check: the page filter it checks plus `"check": true`.
fn check_filter(
    channel_id: &str,
    root_id: Option<&str>,
    cursor: Option<&ThreadCursor>,
    limit: u32,
) -> serde_json::Value {
    let mut filter = match root_id {
        None => build_channel_window_filter(
            channel_id,
            limit,
            cursor
                .map(|cursor| ChannelPageCursor {
                    created_at: cursor.created_at,
                    event_id: cursor.event_id.clone(),
                })
                .as_ref(),
        ),
        Some(root_id) => thread_window_filter(channel_id, root_id, cursor, limit, true),
    };
    filter["check"] = serde_json::json!(true);
    filter
}

/// The `check` object of the one bounds event of the answer, if any: a relay
/// without the check answers a page (or an error) and has none.
fn check_answer(events: &[nostr::Event], thread: bool) -> Option<WindowCheckAnswer> {
    let bounds_kind = if thread {
        KIND_THREAD_BOUNDS
    } else {
        KIND_ROOM_BOUNDS
    };
    let mut bounds = events
        .iter()
        .filter(|event| event.kind.as_u16() == bounds_kind);
    let event = bounds.next()?;
    if bounds.next().is_some() {
        return None;
    }
    let content: serde_json::Value = serde_json::from_str(&event.content).ok()?;
    let check = content.get("check")?;
    let mode = check.get("mode")?.as_str()?;
    if !matches!(mode, "off" | "shadow" | "on") {
        return None;
    }
    Some(WindowCheckAnswer {
        oldest_id: check
            .get("oldest_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_ascii_lowercase),
        oldest_created_at: check
            .get("oldest_created_at")
            .and_then(serde_json::Value::as_i64),
        count: check.get("count")?.as_u64()?,
        room_seq: check.get("room_seq").and_then(serde_json::Value::as_i64),
        mode: mode.to_owned(),
    })
}

/// What a check got back: the check, or, from a relay without the check
/// that answered a room check with the whole page, that page.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct WindowCheckResult {
    check: Option<WindowCheckAnswer>,
    page: Option<Vec<serde_json::Value>>,
}

fn check_result(events: &[nostr::Event], thread: bool) -> WindowCheckResult {
    let check = check_answer(events, thread);
    let one_bounds = events
        .iter()
        .filter(|event| event.kind.as_u16() == KIND_ROOM_BOUNDS)
        .count()
        == 1;
    let page = (check.is_none() && !thread && one_bounds).then(|| {
        events
            .iter()
            .filter_map(|event| serde_json::to_value(event).ok())
            .collect()
    });
    WindowCheckResult { check, page }
}

/// Asks the relay to check one window: the same page query, answered with
/// its bounds only. No check when the relay has none.
#[tauri::command]
pub async fn check_window(
    channel_id: String,
    root_id: Option<String>,
    cursor: Option<ThreadCursor>,
    limit_rows: Option<u32>,
    state: State<'_, AppState>,
) -> Result<WindowCheckResult, String> {
    let filter = check_filter(
        &channel_id,
        root_id.as_deref(),
        cursor.as_ref(),
        window_limit(limit_rows),
    );
    let events = query_relay(&state, &[filter]).await?;
    Ok(check_result(&events, root_id.is_some()))
}

/// A window as the store holds it.
#[derive(serde::Serialize)]
pub struct StoredWindowAnswer {
    /// A room: a channel-window response; a thread: its replies.
    events: Vec<serde_json::Value>,
    count: usize,
    oldest_id: Option<String>,
    oldest_created_at: Option<i64>,
    has_more: bool,
    fully_stored: bool,
}

/// `channel_id`'s window (`root_id`: that thread's) below `cursor` as the
/// store holds it, read in the account and server the caller opened it
/// under; `None` when nothing can be read.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn message_store_window(
    channel_id: String,
    root_id: Option<String>,
    cursor: Option<ThreadCursor>,
    limit: Option<u32>,
    account: String,
    relay_url: String,
    state: State<'_, AppState>,
) -> Result<Option<StoredWindowAnswer>, String> {
    let Some((relay, account)) = paint_scope(&state, &account, &relay_url)? else {
        return Ok(None);
    };
    let kinds = window_kinds(root_id.as_deref());
    let limit = window_limit(limit) as usize;
    off_main(move || {
        let before = cursor
            .as_ref()
            .map(|cursor| (cursor.created_at, cursor.event_id.as_str()));
        crate::message_store::stored_window(
            &relay,
            &account,
            &channel_id,
            root_id.as_deref(),
            &kinds,
            limit,
            before,
        )
        .map(|window| StoredWindowAnswer {
            oldest_id: window.oldest.as_ref().map(|(_, id)| id.clone()),
            oldest_created_at: window.oldest.as_ref().map(|(at, _)| *at),
            events: window.events,
            count: window.count,
            has_more: window.has_more,
            fully_stored: window.fully_stored,
        })
    })
    .await
}

/// Replaces a window's stored rows with the page the relay answered to the
/// request marked `scope` (`message_store_head_sent`): `cursor` is the
/// page's, `oldest` its oldest row (`None` when it had no more), `ids` every
/// event it held.
#[tauri::command]
pub fn message_store_replace_window(
    scope: Option<String>,
    channel_id: String,
    root_id: Option<String>,
    cursor: Option<ThreadCursor>,
    oldest: Option<ThreadCursor>,
    ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let account = state.user_credential()?.public_key().to_hex();
    let relay = relay_api_base_url_with_override(&state);
    let key = |cursor: ThreadCursor| (cursor.created_at, cursor.event_id.to_ascii_lowercase());
    let page = crate::message_store::WindowReplace {
        kinds: window_kinds(root_id.as_deref()),
        room: channel_id,
        root: root_id,
        before: cursor.map(key),
        oldest: oldest.map(key),
        ids: ids.into_iter().map(|id| id.to_ascii_lowercase()).collect(),
    };
    crate::message_store::replace_window(&relay, &account, scope.as_deref(), page)
}

/// Drops the head and page spans of `room_ids` (their catch-up was skipped
/// or cut short), so their windows are fetched again.
#[tauri::command]
pub fn message_store_forget_windows(
    room_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let account = state.user_credential()?.public_key().to_hex();
    let relay = relay_api_base_url_with_override(&state);
    crate::message_store::forget_windows(&relay, &account, room_ids);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn bounds(kind: u16, content: &str) -> nostr::Event {
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(vec![Tag::parse(["h", "room"]).unwrap()])
            .sign_with_keys(&Keys::generate())
            .unwrap()
    }

    #[test]
    fn a_check_is_the_page_filter_plus_check() {
        let room = check_filter("room", None, None, 50);
        assert_eq!(room["check"], true);
        assert_eq!(room["top_level"], true);
        assert_eq!(
            room["kinds"].as_array().unwrap().len(),
            TIMELINE_KINDS.len()
        );
        let cursor = ThreadCursor {
            created_at: 7,
            event_id: "ab".into(),
        };
        let thread = check_filter("room", Some("root"), Some(&cursor), 50);
        assert_eq!(thread["check"], true);
        assert_eq!(thread["thread_window"], true);
        assert_eq!(thread["include_aux"], true);
        assert_eq!(thread["until"], 7);
        assert_eq!(thread["before_id"], "ab");
    }

    #[test]
    fn reads_the_check_from_the_bounds() {
        let answer = check_answer(
            &[bounds(
                KIND_ROOM_BOUNDS,
                r#"{"has_more":true,"check":{"oldest_id":"AB","oldest_created_at":5,"count":50,"room_seq":9,"mode":"shadow"}}"#,
            )],
            false,
        );
        assert_eq!(
            answer,
            Some(WindowCheckAnswer {
                oldest_id: Some("ab".into()),
                oldest_created_at: Some(5),
                count: 50,
                room_seq: Some(9),
                mode: "shadow".into(),
            })
        );
        let empty = check_answer(
            &[bounds(
                KIND_THREAD_BOUNDS,
                r#"{"check":{"oldest_id":null,"oldest_created_at":null,"count":0,"room_seq":3,"mode":"on"}}"#,
            )],
            true,
        )
        .unwrap();
        assert_eq!(empty.oldest_id, None);
        assert_eq!(empty.count, 0);
    }

    #[test]
    fn a_relay_without_the_check_answers_none() {
        let page = bounds(KIND_ROOM_BOUNDS, r#"{"has_more":false,"next_cursor":null}"#);
        assert_eq!(check_answer(&[page.clone()], false), None);
        assert_eq!(check_answer(&[], false), None);
        // An old relay's room answer is the page itself: used as the page.
        let result = check_result(&[page.clone()], false);
        assert_eq!(result.check, None);
        assert_eq!(result.page.as_ref().map(Vec::len), Some(1));
        // Never for a thread, nor for an answer that is no page.
        assert_eq!(check_result(&[page], true).page, None);
        assert_eq!(check_result(&[], false).page, None);
    }
}
