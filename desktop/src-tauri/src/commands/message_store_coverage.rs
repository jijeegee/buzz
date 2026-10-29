//! Coverage of the device message store for the active account's relay, and
//! opening rooms from it (docs/message-sync.md#coverage).

use std::collections::HashMap;

use tauri::State;

use crate::{app_state::AppState, relay::relay_api_base_url_with_override};

fn scope(state: &AppState) -> Result<(String, String), String> {
    let account = state.user_credential()?.public_key().to_hex();
    Ok((relay_api_base_url_with_override(state), account))
}

/// Store reads run on a blocking thread: a sync command would hold the main
/// thread (and every other IPC call) while SQLite reads.
async fn off_main<T: Send + 'static>(
    read: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(read)
        .await
        .map_err(|error| format!("message store read failed: {error}"))
}

/// The end of each room's latest coverage span begun before `before`.
#[tauri::command]
pub async fn message_store_resume_points(
    room_ids: Vec<String>,
    before: i64,
    state: State<'_, AppState>,
) -> Result<HashMap<String, i64>, String> {
    let (relay, account) = scope(&state)?;
    off_main(move || crate::message_store::resume_points(&relay, &account, &room_ids, before)).await
}

/// The capture (server file, sign-in, sequence) coverage is recorded with:
/// the one taken when subscription `sub_id` reached EOSE on the socket, or
/// one taken now.
#[tauri::command]
pub fn message_store_scope(
    sub_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let (relay, account) = scope(&state)?;
    Ok(match sub_id {
        Some(sub) => crate::message_store::eose_scope_token(&relay, &account, &sub),
        None => crate::message_store::scope_token(&relay, &account),
    })
}

/// Records that every event of `room_ids` in `[from, to]` has been received.
/// Refused when `scope` (from `message_store_scope`) is no longer current.
#[tauri::command]
pub fn message_store_extend_coverage(
    room_ids: Vec<String>,
    from: i64,
    to: i64,
    scope: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (relay, account) = self::scope(&state)?;
    crate::message_store::extend_coverage(&relay, &account, scope.as_deref(), room_ids, from, to)
}

/// Applies `room_ids`, the account's membership list from a successful
/// fetch (archived, hidden and huddle rooms included), to the stored rooms.
/// Refused when `scope` (the capture `get_channels` took before that fetch)
/// is no longer current, or the list is empty. Also learns the relay's
/// signing key, so its notices can forget rooms
/// (docs/message-sync.md#forgetting-a-room).
#[tauri::command]
pub async fn message_store_forget_rooms_except(
    room_ids: Vec<String>,
    scope: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (relay, account) = self::scope(&state)?;
    crate::message_store::forget_rooms_except(&relay, &account, scope.as_deref(), room_ids)?;
    // Noted by the lookup itself; a failed one leaves the notices unacted.
    let _ = super::identity_archive::fetch_relay_self(&state).await;
    Ok(())
}

/// `room_id`'s newest window from the store, in the shape of
/// `get_channel_window`, or `None` unless it is certain. `live_at` is when
/// the room's live REQ reached EOSE on the current connection.
#[tauri::command]
pub async fn message_store_room_head(
    room_id: String,
    live_at: i64,
    state: State<'_, AppState>,
) -> Result<Option<Vec<serde_json::Value>>, String> {
    let (relay, account) = scope(&state)?;
    let kinds: Vec<u64> = super::channel_window::TIMELINE_KINDS
        .iter()
        .map(|&kind| u64::from(kind))
        .collect();
    off_main(move || {
        crate::message_store::certain_room_head(
            &relay,
            &account,
            &room_id,
            &kinds,
            ROOM_HEAD_ROWS,
            live_at,
        )
    })
    .await
}

/// `root_id`'s newest replies from the store, in the shape of a
/// `get_thread_windows` page, or `None` unless it is certain.
#[tauri::command]
pub async fn message_store_thread_head(
    channel_id: String,
    root_id: String,
    limit: usize,
    live_at: i64,
    state: State<'_, AppState>,
) -> Result<Option<super::thread_window::ThreadWindowPage>, String> {
    let (relay, account) = scope(&state)?;
    let kinds: Vec<u64> = super::thread_window::THREAD_ROW_KINDS
        .iter()
        .map(|&kind| u64::from(kind))
        .collect();
    off_main(move || {
        crate::message_store::certain_thread_head(
            &relay,
            &account,
            &channel_id,
            &root_id,
            &kinds,
            limit.clamp(1, 200),
            live_at,
        )
        .map(|(events, next_cursor)| {
            super::thread_window::ThreadWindowPage::from_store(
                channel_id,
                root_id,
                events,
                next_cursor,
            )
        })
    })
    .await
}

/// Rows of a room's first window, as `get_channel_window` asks by default.
const ROOM_HEAD_ROWS: usize = 50;

/// Marks a newest-window request for `room_ids` (each carried by a live REQ)
/// going out: the capture to record its head spans with, or `None`
/// (docs/message-sync.md#head-spans).
#[tauri::command]
pub async fn message_store_head_sent(
    room_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let (relay, account) = scope(&state)?;
    Ok(crate::message_store::head_sent(&relay, &account, room_ids))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreHeadCursor {
    created_at: i64,
    event_id: String,
}

/// One newest window as the relay answered it, while the room's live REQs
/// stayed live from before the request (L = `live_at`).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreHeadArg {
    room_id: String,
    /// `None`: the room's window; else this thread root's.
    root_id: Option<String>,
    live_at: i64,
    /// The answer's oldest row; `None` when it had no more.
    next_cursor: Option<StoreHeadCursor>,
    ids: Vec<String>,
}

/// Records the head spans of the request marked `scope` (from
/// `message_store_head_sent`); an empty list only releases the mark.
#[tauri::command]
pub async fn message_store_record_heads(
    scope: Option<String>,
    heads: Vec<StoreHeadArg>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (relay, account) = self::scope(&state)?;
    let kinds = |list: &[u32]| list.iter().map(|&kind| u64::from(kind)).collect();
    let heads = heads
        .into_iter()
        .map(|head| crate::message_store::HeadSpan {
            kinds: if head.root_id.is_some() {
                kinds(&super::thread_window::THREAD_ROW_KINDS)
            } else {
                kinds(&super::channel_window::TIMELINE_KINDS)
            },
            room: head.room_id,
            root: head.root_id,
            live_at: head.live_at,
            oldest: head
                .next_cursor
                .map(|cursor| (cursor.created_at, cursor.event_id)),
            ids: head.ids.into_iter().collect(),
        })
        .collect();
    crate::message_store::record_heads(&relay, &account, scope.as_deref(), heads)
}
