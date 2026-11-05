//! Goal commands — the layered goals of a channel or DM.
//!
//! Reads are `{"goals":1}` `/query` extensions; the relay computes everything
//! a row shows. Edits are kind 40111 events the relay applies to its rows in
//! order, all or none, so there is nothing to re-read or retry here.

use kuzz_core_pkg::goal_tree::GoalOp;
use kuzz_core_pkg::kind::KIND_GOAL_OPS;
use tauri::State;

use crate::{
    app_state::AppState,
    relay::{query_relay, query_relay_value, submit_event},
};

fn view_filter(
    channel_id: &str,
    view: &str,
    cursor: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut filter = serde_json::json!({ "goals": 1, "channel": channel_id, "view": view });
    if let Some(cursor) = cursor {
        filter["cursor"] = cursor;
    }
    filter
}

/// One page of a goal list (`active`, `inactive`, or `done`).
#[tauri::command]
pub async fn get_goals(
    channel_id: String,
    view: String,
    cursor: Option<serde_json::Value>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    query_relay_value(&state, &[view_filter(&channel_id, &view, cursor)]).await
}

/// What these threads or messages work on, whatever list their goals are in.
#[tauri::command]
pub async fn get_thread_goals(
    channel_id: String,
    threads: Vec<String>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = serde_json::json!({ "goals": 1, "channel": channel_id, "threads": threads });
    query_relay_value(&state, &[filter]).await
}

/// Apply edits as one event: all of them or none, e.g. "add a sub-goal and
/// link this thread to it" from a thread.
#[tauri::command]
pub async fn apply_goal_ops(
    channel_id: String,
    ops: Vec<GoalOp>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    if ops.is_empty() {
        return Err("no goal changes to save".into());
    }
    let uuid = uuid::Uuid::parse_str(&channel_id)
        .map_err(|_| format!("invalid channel UUID: {channel_id}"))?;
    let builder = kuzz_sdk_pkg::build_goal_ops(uuid, &ops).map_err(|e| e.to_string())?;
    let result = submit_event(builder, &state).await?;
    Ok(serde_json::json!({ "event_id": result.event_id }))
}

/// List goal edits, newest first.
#[tauri::command]
pub async fn get_goal_history(
    channel_id: String,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = serde_json::json!({
        "kinds": [KIND_GOAL_OPS],
        "#h": [channel_id],
        "limit": limit.unwrap_or(30).clamp(1, 200),
    });
    let events = query_relay(&state, &[filter]).await?;
    let edits: Vec<serde_json::Value> = events
        .iter()
        .map(|event| {
            let ops = serde_json::from_str::<serde_json::Value>(&event.content)
                .ok()
                .and_then(|content| content.get("ops").cloned());
            serde_json::json!({
                "event_id": event.id.to_hex(),
                "created_at": event.created_at.as_secs(),
                "author": event.pubkey.to_hex(),
                "ops": ops,
            })
        })
        .collect();
    Ok(serde_json::json!({ "edits": edits }))
}
