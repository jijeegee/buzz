//! The user stream's part of the device message store: the `room_seq`
//! watermark, its epoch and the stream's `sync_state` values
//! (docs/message-sync.md#watermark-room_seq).

use std::collections::HashMap;

use tauri::State;

use super::message_store_coverage::off_main;
use crate::{app_state::AppState, relay::relay_api_base_url_with_override};

fn scope(state: &AppState) -> Result<(String, String), String> {
    let account = state.user_credential()?.public_key().to_hex();
    Ok((relay_api_base_url_with_override(state), account))
}

fn lease(state: &AppState) -> Result<Option<crate::message_store::Lease>, String> {
    let (relay, account) = scope(state)?;
    Ok(crate::message_store::global().and_then(|store| store.lease(&relay, &account)))
}

/// Queues an accepted part 1's epoch (a different one clears every row).
/// `false` when it is invalid or could not be queued.
#[tauri::command]
pub fn message_store_stream_epoch(
    epoch: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let Some(lease) = lease(&state)? else {
        return Ok(false);
    };
    Ok(crate::message_store::global().is_some_and(|store| store.stream_epoch(&lease, epoch)))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSeqs {
    /// Everything queued was written and nothing was lost when read.
    vouched: bool,
    /// `None` until a part 1 was accepted for this file.
    epoch: Option<String>,
    /// `[seq, at]` per room that has a row.
    rows: HashMap<String, (i64, i64)>,
    /// Resume points from coverage begun before `before` (when given).
    resume: HashMap<String, i64>,
}

/// The stored epoch, the rows of `room_ids` and (with `before`) their resume
/// points; none unless everything queued is written and nothing was lost.
#[tauri::command]
pub async fn message_store_stream_seqs(
    room_ids: Vec<String>,
    before: Option<i64>,
    state: State<'_, AppState>,
) -> Result<StreamSeqs, String> {
    let lease = lease(&state)?;
    off_main(move || {
        let stored = lease
            .zip(crate::message_store::global())
            .map(|(lease, store)| store.stream_seqs(&lease, &room_ids, before))
            .unwrap_or_default();
        StreamSeqs {
            vouched: stored.vouched,
            epoch: stored.epoch,
            rows: stored.rows,
            resume: stored.resume,
        }
    })
    .await
}

/// Deletes the rows of `room_ids`, whose stored seq is ahead of the
/// snapshot (the relay DB was restored); `false` when not queued.
#[tauri::command]
pub fn message_store_clear_stream_rows(
    room_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let Some(lease) = lease(&state)? else {
        return Ok(false);
    };
    Ok(crate::message_store::global()
        .is_some_and(|store| store.clear_stream_rows(&lease, room_ids)))
}

/// Records a catch-up's span with its rule-W rows (`seqs`: each room's
/// snapshot seq, `epoch`: the one the catch-up started under). Refused with
/// the span when `scope` (from `message_store_scope`) is no longer current.
#[tauri::command]
pub fn message_store_extend_stream_coverage(
    room_ids: Vec<String>,
    from: i64,
    to: i64,
    scope: Option<String>,
    epoch: String,
    seqs: HashMap<String, i64>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (relay, account) = self::scope(&state)?;
    let Some(store) = crate::message_store::global() else {
        return Ok(());
    };
    let seq = crate::message_store::SeqWrite { epoch, seqs };
    store.extend_stream_coverage(&relay, &account, scope.as_deref(), room_ids, from, to, seq)
}

/// Each room's coverage segments, taken before a catch-up so its new rows
/// can be counted; `None` when the store cannot vouch for what it holds.
#[tauri::command]
pub async fn message_store_walk_baseline(
    room_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Option<HashMap<String, Vec<(i64, i64)>>>, String> {
    let lease = lease(&state)?;
    off_main(move || {
        let (lease, store) = lease.zip(crate::message_store::global())?;
        store.walk_segments(&lease, &room_ids)
    })
    .await
}

/// Which of `ids` (a walk's candidate new rows) were not stored before
/// `since_ms`, the connection's start on this device's clock; `None` when
/// the store cannot vouch for what it holds (docs/message-sync.md#counters).
#[tauri::command]
pub async fn message_store_fresh_ids(
    ids: Vec<String>,
    since_ms: i64,
    state: State<'_, AppState>,
) -> Result<Option<Vec<String>>, String> {
    let lease = lease(&state)?;
    off_main(move || {
        let (lease, store) = lease.zip(crate::message_store::global())?;
        store.fresh_ids(&lease, &ids, since_ms)
    })
    .await
}

/// A stream `sync_state` value: `stream_backoff`, `stream_counters` or
/// `stream_prev_live`.
#[tauri::command]
pub async fn message_store_stream_state(
    key: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let lease = lease(&state)?;
    off_main(move || {
        let (lease, store) = lease.zip(crate::message_store::global())?;
        store.stream_state(&lease, &key)
    })
    .await
}

/// Queues a stream `sync_state` value; `false` for another key, a value
/// too long, or a full queue.
#[tauri::command]
pub fn message_store_set_stream_state(
    key: String,
    value: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let Some(lease) = lease(&state)? else {
        return Ok(false);
    };
    Ok(crate::message_store::global()
        .is_some_and(|store| store.set_stream_state(&lease, &key, value)))
}
