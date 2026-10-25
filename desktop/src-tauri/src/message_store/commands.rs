//! Tauri commands. Every read names its scope explicitly, so a scope change
//! in the webview can never read another account's rows. There is no insert
//! command: Rust writes everything.

use serde::Deserialize;
use tauri::{AppHandle, State};

use super::{
    coverage::Cursor,
    ingest,
    read::{self, AroundPage, RoomCoverage, RoomHead, ThreadHeadPage, ThreadPage, WindowPage},
    store_dir, MessageStore, StoreScope,
};

const MAX_PAGE_ROWS: u32 = 500;
const MAX_HEAD_ROOMS: u32 = 256;
const MAX_CHANNELS: usize = 512;

fn page_limit(limit: u32) -> usize {
    limit.clamp(1, MAX_PAGE_ROWS) as usize
}

fn check_channels(channel_ids: &[String]) -> Result<(), String> {
    if channel_ids.len() > MAX_CHANNELS {
        return Err(format!("at most {MAX_CHANNELS} channels per call"));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Anchor {
    created_at: i64,
    event_id: Option<String>,
}

/// Head pages of the most recently used rooms (boot prefill).
#[tauri::command]
pub(crate) async fn message_store_load_heads(
    scope: StoreScope,
    rooms: Option<u32>,
    rows: Option<u32>,
    app: AppHandle,
    store: State<'_, MessageStore>,
) -> Result<Vec<RoomHead>, String> {
    let rooms = rooms.unwrap_or(64).clamp(1, MAX_HEAD_ROOMS) as usize;
    let rows = page_limit(rows.unwrap_or(50));
    let dir = store_dir(&app)?;
    store
        .call(&dir, &scope, move |conn| read::heads(conn, rooms, rows))
        .await
}

/// One page of a room below `before` (or the head); never crosses a gap.
#[tauri::command]
pub(crate) async fn message_store_read_window(
    scope: StoreScope,
    channel_id: String,
    before: Option<Cursor>,
    limit: u32,
    app: AppHandle,
    store: State<'_, MessageStore>,
) -> Result<WindowPage, String> {
    let limit = page_limit(limit);
    let dir = store_dir(&app)?;
    store
        .call(&dir, &scope, move |conn| {
            read::window(conn, &channel_id, before.as_ref(), limit)
        })
        .await
}

/// Rows around an anchor (unread marker, search jump, deep link).
#[tauri::command]
pub(crate) async fn message_store_read_around(
    scope: StoreScope,
    channel_id: String,
    anchor: Anchor,
    before: u32,
    after: u32,
    app: AppHandle,
    store: State<'_, MessageStore>,
) -> Result<AroundPage, String> {
    let (before, after) = (page_limit(before), page_limit(after));
    let anchor = read::anchor(anchor.created_at, anchor.event_id);
    let dir = store_dir(&app)?;
    store
        .call(&dir, &scope, move |conn| {
            read::around(conn, &channel_id, &anchor, before, after)
        })
        .await
}

/// Replies of one thread, oldest first.
#[tauri::command]
pub(crate) async fn message_store_read_thread(
    scope: StoreScope,
    root_id: String,
    cursor: Option<Cursor>,
    limit: u32,
    app: AppHandle,
    store: State<'_, MessageStore>,
) -> Result<ThreadPage, String> {
    let limit = page_limit(limit);
    let dir = store_dir(&app)?;
    store
        .call(&dir, &scope, move |conn| {
            read::thread(conn, &root_id.to_ascii_lowercase(), cursor.as_ref(), limit)
        })
        .await
}

/// Replies of one thread below `before` (or the newest), newest first.
#[tauri::command]
pub(crate) async fn message_store_read_thread_head(
    scope: StoreScope,
    root_id: String,
    before: Option<Cursor>,
    limit: u32,
    app: AppHandle,
    store: State<'_, MessageStore>,
) -> Result<ThreadHeadPage, String> {
    let limit = page_limit(limit);
    let dir = store_dir(&app)?;
    store
        .call(&dir, &scope, move |conn| {
            read::thread_head(conn, &root_id.to_ascii_lowercase(), before.as_ref(), limit)
        })
        .await
}

/// What is proven locally, per room.
#[tauri::command]
pub(crate) async fn message_store_coverage(
    scope: StoreScope,
    channel_ids: Vec<String>,
    app: AppHandle,
    store: State<'_, MessageStore>,
) -> Result<Vec<RoomCoverage>, String> {
    check_channels(&channel_ids)?;
    let dir = store_dir(&app)?;
    store
        .call(&dir, &scope, move |conn| {
            channel_ids
                .iter()
                .map(|channel_id| read::coverage(conn, channel_id))
                .collect()
        })
        .await
}

/// Registers a live subscription that reached EOSE. Returns the segment id;
/// the segment closes when its socket exits (or at the next launch).
#[tauri::command]
pub(crate) async fn message_store_begin_live(
    scope: StoreScope,
    ws_id: i64,
    channel_ids: Vec<String>,
    since: i64,
    app: AppHandle,
    store: State<'_, MessageStore>,
) -> Result<i64, String> {
    check_channels(&channel_ids)?;
    let dir = store_dir(&app)?;
    store
        .call(&dir, &scope, move |conn| {
            ingest::begin_live(conn, ws_id, &channel_ids, since)
        })
        .await
}

/// Deletes one scope's stored messages.
#[tauri::command]
pub(crate) async fn message_store_clear_scope(
    scope: StoreScope,
    app: AppHandle,
) -> Result<(), String> {
    super::purge_blocking(&app, scope.file_stem()?).await
}
