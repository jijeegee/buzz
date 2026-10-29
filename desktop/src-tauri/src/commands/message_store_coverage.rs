//! Coverage of the device message store for the active account's relay
//! (docs/message-sync.md#coverage).

use std::collections::HashMap;

use tauri::State;

use crate::{app_state::AppState, relay::relay_api_base_url_with_override};

fn scope(state: &AppState) -> Result<(String, String), String> {
    let account = state.user_credential()?.public_key().to_hex();
    Ok((relay_api_base_url_with_override(state), account))
}

/// The end of each room's latest coverage span begun before `before`.
#[tauri::command]
pub fn message_store_resume_points(
    room_ids: Vec<String>,
    before: i64,
    state: State<'_, AppState>,
) -> Result<HashMap<String, i64>, String> {
    let (relay, account) = scope(&state)?;
    Ok(crate::message_store::resume_points(
        &relay, &account, &room_ids, before,
    ))
}

/// Records that every event of `room_ids` in `[from, to]` has been received.
#[tauri::command]
pub fn message_store_extend_coverage(
    room_ids: Vec<String>,
    from: i64,
    to: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let (relay, account) = scope(&state)?;
    crate::message_store::extend_coverage(&relay, &account, room_ids, from, to);
    Ok(())
}
