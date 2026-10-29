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

/// The scope (server file and sign-in) coverage recorded from now on is for.
#[tauri::command]
pub fn message_store_scope(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let (relay, account) = scope(&state)?;
    Ok(crate::message_store::scope_token(&relay, &account))
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
