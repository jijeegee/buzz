//! Goal tree (kind 40110) commands — the layered goals of a channel or DM.
//!
//! Edits are sent as operations, not whole trees: the backend re-reads the
//! live head, applies the operation, and publishes with an
//! `expected-revision` precondition. A conflict re-applies the same operation
//! to the newer head, so concurrent editors never drop each other's changes.

use buzz_core_pkg::goal_tree::{GoalOp, GoalTree};
use buzz_core_pkg::kind::KIND_GOAL_TREE;
use tauri::State;

use crate::{
    app_state::AppState,
    relay::{query_relay, submit_event},
};

const MAX_WRITE_ATTEMPTS: usize = 4;

struct Head {
    id: String,
    created_at: u64,
    author: String,
    tree: GoalTree,
}

/// `strong` pins the read to the writer pool. Every write is composed against
/// a strong read so a lagging replica cannot hand it a stale revision.
fn head_filter(channel_id: &str, strong: bool) -> serde_json::Value {
    let mut filter =
        serde_json::json!({ "kinds": [KIND_GOAL_TREE], "#h": [channel_id], "limit": 1 });
    if strong {
        filter["consistency"] = serde_json::json!("strong");
    }
    filter
}

async fn fetch_head(
    state: &AppState,
    channel_id: &str,
    strong: bool,
) -> Result<Option<Head>, String> {
    let events = query_relay(state, &[head_filter(channel_id, strong)]).await?;
    let Some(event) = events.first() else {
        return Ok(None);
    };
    let tree = GoalTree::parse(&event.content).map_err(|e| e.to_string())?;
    Ok(Some(Head {
        id: event.id.to_hex(),
        created_at: event.created_at.as_secs(),
        author: event.pubkey.to_hex(),
        tree,
    }))
}

fn head_json(head: Option<Head>) -> serde_json::Value {
    match head {
        Some(head) => serde_json::json!({
            "revision": head.id,
            "updated_at": head.created_at,
            "author": head.author,
            "tree": head.tree,
        }),
        None => serde_json::json!({
            "revision": null,
            "updated_at": null,
            "author": null,
            "tree": GoalTree::empty(),
        }),
    }
}

/// Read the live goal tree of a channel or DM.
#[tauri::command]
pub async fn get_goal_tree(
    channel_id: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    Ok(head_json(fetch_head(&state, &channel_id, false).await?))
}

enum Edit {
    Op(GoalOp),
    Replace(GoalTree),
}

async fn write(
    state: &AppState,
    channel_id: &str,
    edit: Edit,
) -> Result<serde_json::Value, String> {
    let uuid = uuid::Uuid::parse_str(channel_id)
        .map_err(|_| format!("invalid channel UUID: {channel_id}"))?;
    let editor = state.signing_keys()?.public_key().to_hex();
    for attempt in 0..MAX_WRITE_ATTEMPTS {
        if attempt > 0 {
            // Spread out writers that collided on the same head.
            tokio::time::sleep(std::time::Duration::from_millis(150 << attempt)).await;
        }
        let head = fetch_head(state, channel_id, true).await?;
        let (revision, head_created_at, mut tree) = match head {
            Some(h) => (h.id, Some(h.created_at), h.tree),
            None => ("none".to_string(), None, GoalTree::empty()),
        };
        match &edit {
            Edit::Op(op) => {
                let now = chrono::Utc::now().timestamp().max(0) as u64;
                tree.apply(op, &editor, now).map_err(|e| e.to_string())?;
            }
            Edit::Replace(replacement) => tree = replacement.clone(),
        }
        let builder = buzz_sdk_pkg::build_set_goal_tree(uuid, &tree, &revision, head_created_at)
            .map_err(|e| e.to_string())?;
        match submit_event(builder, state).await {
            Ok(result) => {
                return Ok(serde_json::json!({ "event_id": result.event_id, "tree": tree }));
            }
            Err(e) if e.contains("conflict: goal tree") => continue,
            Err(e) => return Err(e),
        }
    }
    Err("GOAL_TREE_BUSY: the goals kept changing while saving — try again".into())
}

/// Apply one edit operation to the live goal tree.
#[tauri::command]
pub async fn apply_goal_op(
    channel_id: String,
    op: GoalOp,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    write(&state, &channel_id, Edit::Op(op)).await
}

/// List previous goal tree revisions, newest first.
#[tauri::command]
pub async fn get_goal_tree_history(
    channel_id: String,
    limit: Option<u32>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = serde_json::json!({
        "kinds": [KIND_GOAL_TREE],
        "#h": [channel_id],
        "limit": limit.unwrap_or(30).clamp(1, 200),
    });
    let events = query_relay(&state, &[filter]).await?;
    let revisions: Vec<serde_json::Value> = events
        .iter()
        .map(|event| {
            let goals = GoalTree::parse(&event.content).map(|t| t.nodes.len()).ok();
            serde_json::json!({
                "revision": event.id.to_hex(),
                "created_at": event.created_at.as_secs(),
                "author": event.pubkey.to_hex(),
                "goals": goals,
            })
        })
        .collect();
    Ok(serde_json::json!({ "revisions": revisions }))
}

/// Restore a previous revision as the new head.
#[tauri::command]
pub async fn restore_goal_tree(
    channel_id: String,
    revision: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = serde_json::json!({
        "ids": [revision],
        "kinds": [KIND_GOAL_TREE],
        "#h": [channel_id],
        "limit": 1,
    });
    let events = query_relay(&state, &[filter]).await?;
    let event = events
        .first()
        .ok_or_else(|| format!("revision {revision} not found"))?;
    let tree = GoalTree::parse(&event.content).map_err(|e| e.to_string())?;
    write(&state, &channel_id, Edit::Replace(tree)).await
}
