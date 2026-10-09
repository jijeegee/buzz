//! `buzz goals` — read and edit a channel's or DM's goal tree (kind 40110).
//!
//! Every edit is read-modify-write against the live head with an
//! `expected-revision` precondition. On a conflict the command re-reads the
//! head and re-applies the same edit, so concurrent editors never silently
//! drop each other's changes.

use buzz_core::goal_tree::{new_node_id, GoalNode, GoalOp, GoalStatus, GoalTree};
use buzz_core::kind::KIND_GOAL_TREE;
use serde_json::{json, Value};

use crate::client::BuzzClient;
use crate::error::CliError;
use crate::validate::{parse_uuid, read_or_stdin, validate_hex64};
use crate::GoalsCmd;

/// Attempts at a conflicting write before giving up.
const MAX_WRITE_ATTEMPTS: usize = 4;

pub(crate) struct Head {
    id: String,
    created_at: u64,
    pubkey: String,
    pub(crate) tree: GoalTree,
}

fn parse_head(event: &Value) -> Result<Head, CliError> {
    let field = |name: &str| event.get(name).and_then(Value::as_str).unwrap_or_default();
    let tree = GoalTree::parse(field("content"))
        .map_err(|e| CliError::Other(format!("goal tree head is unreadable: {e}")))?;
    Ok(Head {
        id: field("id").to_string(),
        created_at: event.get("created_at").and_then(Value::as_u64).unwrap_or(0),
        pubkey: field("pubkey").to_string(),
        tree,
    })
}

async fn query_events(client: &BuzzClient, filter: Value) -> Result<Vec<Value>, CliError> {
    let resp = client.query(&filter).await?;
    serde_json::from_str(&resp)
        .map_err(|e| CliError::Other(format!("malformed relay response querying goal tree: {e}")))
}

/// The live head. `strong` pins the read to the writer pool so a write is
/// always composed against the newest head.
pub(crate) async fn fetch_head(
    client: &BuzzClient,
    channel: &str,
    strong: bool,
) -> Result<Option<Head>, CliError> {
    let mut filter = json!({ "kinds": [KIND_GOAL_TREE], "#h": [channel], "limit": 1 });
    if strong {
        filter["consistency"] = json!("strong");
    }
    query_events(client, filter)
        .await?
        .first()
        .map(parse_head)
        .transpose()
}

async fn event_exists(client: &BuzzClient, channel: &str, id: &str) -> Result<bool, CliError> {
    let filter = json!({
        "ids": [id], "kinds": [KIND_GOAL_TREE], "#h": [channel], "limit": 1,
        "consistency": "strong",
    });
    Ok(!query_events(client, filter).await?.is_empty())
}

pub(crate) enum Edit {
    /// Operations applied in order and published as one revision.
    Ops(Vec<GoalOp>),
    Replace(GoalTree),
}

pub(crate) fn status_op(id: &str, status: GoalStatus) -> GoalOp {
    status_note_op(id, status, None)
}

pub(crate) fn status_note_op(id: &str, status: GoalStatus, note: Option<String>) -> GoalOp {
    GoalOp::Update {
        id: id.to_string(),
        title: None,
        note,
        status: Some(status),
        add_assignees: vec![],
        remove_assignees: vec![],
    }
}

/// What finishing `id` means for the tree around it: sub-goals still open,
/// and whether every sibling under its parent is now finished. The parent is
/// never completed automatically; people decide that.
pub(crate) fn done_report(tree: &GoalTree, id: &str) -> Value {
    let unfinished = |n: &GoalNode| matches!(n.status, GoalStatus::Open | GoalStatus::InProgress);
    let open_sub_goals = tree
        .subtree(id)
        .iter()
        .filter(|(_, n)| n.id != id && unfinished(n))
        .count();
    let mut out = json!({});
    if open_sub_goals > 0 {
        out["open_sub_goals"] = json!(open_sub_goals);
        out["warning"] = json!(format!(
            "goal {id} is done but {open_sub_goals} of its sub-goals are still open"
        ));
    }
    let parent = tree.node(id).and_then(|n| n.parent.clone());
    if let Some(parent) = parent {
        let siblings = tree.children(&parent);
        if siblings.iter().all(|n| !unfinished(n)) && tree.node(&parent).is_some_and(unfinished) {
            out["parent_all_done"] = json!(parent);
        }
    }
    out
}

/// The one status/title/note change path behind `update`, `start`, `done`,
/// and `threads close --goal-done`.
pub(crate) async fn update_goal(
    client: &BuzzClient,
    channel: &str,
    op: GoalOp,
) -> Result<Value, CliError> {
    let done = match &op {
        GoalOp::Update {
            id,
            status: Some(GoalStatus::Done),
            ..
        } => Some(id.clone()),
        _ => None,
    };
    let (mut out, tree) = write_tree(client, channel, Edit::Ops(vec![op])).await?;
    if let Some(id) = done {
        if let (Some(out), Value::Object(report)) = (out.as_object_mut(), done_report(&tree, &id)) {
            out.extend(report);
        }
    }
    Ok(out)
}

/// `add --thread`: create the goal and link the thread (which starts it) —
/// one write.
fn add_ops(add: GoalOp, thread: Option<String>) -> Vec<GoalOp> {
    let GoalOp::Add { id, .. } = &add else {
        return vec![add];
    };
    let id = id.clone();
    // Linking starts the goal (`GoalTree::apply`), so no separate status op.
    match thread {
        Some(thread) => vec![add, GoalOp::Link { id, thread }],
        None => vec![add],
    }
}

fn is_goal_conflict(err: &CliError) -> bool {
    matches!(err, CliError::Relay { status: 409, body } if body.contains("conflict: goal tree"))
}

/// Apply `edit` to the live head and publish it, re-applying after conflicts.
pub(crate) async fn write(
    client: &BuzzClient,
    channel: &str,
    edit: Edit,
) -> Result<Value, CliError> {
    write_tree(client, channel, edit).await.map(|(out, _)| out)
}

/// [`write`], also returning the tree as published.
pub(crate) async fn write_tree(
    client: &BuzzClient,
    channel: &str,
    edit: Edit,
) -> Result<(Value, GoalTree), CliError> {
    let channel_uuid = parse_uuid(channel)?;
    let editor = client.pubkey().to_hex();
    for _ in 0..MAX_WRITE_ATTEMPTS {
        let head = fetch_head(client, channel, true).await?;
        let (revision, head_created_at, mut tree) = match head {
            Some(h) => (h.id, Some(h.created_at), h.tree),
            None => ("none".to_string(), None, GoalTree::empty()),
        };
        match &edit {
            Edit::Replace(replacement) => tree = replacement.clone(),
            Edit::Ops(ops) => {
                let now = chrono::Utc::now().timestamp().max(0) as u64;
                for op in ops {
                    tree.apply(op, &editor, now)
                        .map_err(|e| CliError::Usage(e.to_string()))?;
                }
            }
        }
        let builder =
            buzz_sdk::build_set_goal_tree(channel_uuid, &tree, &revision, head_created_at)
                .map_err(crate::validate::sdk_err)?;
        let event = client.sign_event(builder)?;
        let event_id = event.id.to_hex();
        match client.submit_event(event).await {
            Ok(_) => return Ok((json!({ "event_id": event_id, "accepted": true }), tree)),
            // A lost response can surface the retry of our own stored write as
            // a conflict; only re-apply when our event is really absent.
            Err(e) if is_goal_conflict(&e) => {
                if event_exists(client, channel, &event_id).await? {
                    return Ok((json!({ "event_id": event_id, "accepted": true }), tree));
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(CliError::Conflict(
        "the goal tree kept changing while saving; run the command again".into(),
    ))
}

fn node_json(tree: &GoalTree, layer: usize, node: &GoalNode) -> Value {
    let (done, total) = tree.progress(&node.id);
    json!({
        "id": node.id,
        "layer": layer,
        "parent": node.parent,
        "title": node.title,
        "note": node.note,
        "status": node.status.as_str(),
        "assignees": node.assignees,
        "threads": node.threads,
        "progress": { "done": done, "total": total },
        "updated_by": node.updated_by,
        "updated_at": node.updated_at,
    })
}

/// A goal's working context: the path above it, its subtree, and the other
/// goals on its layer (to avoid overlap and spot gaps).
fn focus_json(tree: &GoalTree, id: &str) -> Result<Value, CliError> {
    let layer = tree
        .layer(id)
        .ok_or_else(|| CliError::NotFound(format!("goal {id} not found")))?;
    let path: Vec<Value> = tree
        .path(id)
        .iter()
        .enumerate()
        .map(|(i, n)| node_json(tree, i + 1, n))
        .collect();
    let subtree: Vec<Value> = tree
        .subtree(id)
        .iter()
        .map(|(l, n)| node_json(tree, *l, n))
        .collect();
    let same_layer: Vec<Value> = tree
        .layer_nodes(layer)
        .iter()
        .filter(|n| n.id != id)
        .map(|n| node_json(tree, layer, n))
        .collect();
    Ok(
        json!({ "goal_id": id, "layer": layer, "path": path, "subtree": subtree, "same_layer": same_layer }),
    )
}

fn parse_pubkey(value: &str) -> Result<String, CliError> {
    nostr::PublicKey::parse(value)
        .map(|p| p.to_hex())
        .map_err(|_| CliError::Usage(format!("invalid pubkey {value:?} (hex or npub)")))
}

fn parse_status(value: &str) -> Result<GoalStatus, CliError> {
    GoalStatus::parse(value).ok_or_else(|| {
        CliError::Usage(format!(
            "invalid status {value:?} (open, in_progress, done, dropped)"
        ))
    })
}

fn optional_text(value: Option<String>) -> Result<Option<String>, CliError> {
    value.map(|v| read_or_stdin(&v)).transpose()
}

fn print(value: &Value) {
    println!("{value}");
}

pub async fn dispatch(cmd: GoalsCmd, client: &BuzzClient) -> Result<(), CliError> {
    match cmd {
        GoalsCmd::Get {
            channel,
            node,
            thread,
        } => {
            parse_uuid(&channel)?;
            let head = fetch_head(client, &channel, false).await?;
            let Some(head) = head else {
                print(&json!({ "channel": channel, "revision": null, "outline": [] }));
                return Ok(());
            };
            let tree = &head.tree;
            if let Some(thread) = &thread {
                validate_hex64(thread)?;
            }
            let this_thread_goal = thread
                .as_deref()
                .and_then(|t| tree.node_for_thread(t))
                .map(|n| n.id.clone());
            let focus_id = node.or_else(|| this_thread_goal.clone());
            let outline: Vec<Value> = tree
                .outline()
                .iter()
                .map(|(l, n)| node_json(tree, *l, n))
                .collect();
            let mut out = json!({
                "channel": channel,
                "revision": head.id,
                "updated_at": head.created_at,
                "updated_by": head.pubkey,
                "outline": outline,
            });
            if thread.is_some() {
                // The goal the asked-about thread works on, or null.
                out["this_thread_goal"] = json!(this_thread_goal);
            }
            if let Some(id) = focus_id {
                out["focus"] = focus_json(tree, &id)?;
            }
            print(&out);
        }
        GoalsCmd::SetRoot {
            channel,
            title,
            note,
        } => {
            let op = GoalOp::SetRoot {
                id: new_node_id(),
                title,
                note: optional_text(note)?,
            };
            print(&write(client, &channel, Edit::Ops(vec![op])).await?);
        }
        GoalsCmd::Add {
            channel,
            parent,
            title,
            note,
            assignee,
            thread,
        } => {
            if let Some(thread) = &thread {
                validate_hex64(thread)?;
            }
            let id = new_node_id();
            let op = GoalOp::Add {
                id: id.clone(),
                parent,
                title,
                note: optional_text(note)?,
                assignees: assignee
                    .iter()
                    .map(|a| parse_pubkey(a))
                    .collect::<Result<_, _>>()?,
            };
            let mut out = write(client, &channel, Edit::Ops(add_ops(op, thread))).await?;
            out["goal_id"] = json!(id);
            print(&out);
        }
        GoalsCmd::Update {
            channel,
            node,
            title,
            note,
            status,
            assign,
            unassign,
        } => {
            let op = GoalOp::Update {
                id: node,
                title,
                note: optional_text(note)?,
                status: status.as_deref().map(parse_status).transpose()?,
                add_assignees: assign
                    .iter()
                    .map(|a| parse_pubkey(a))
                    .collect::<Result<_, _>>()?,
                remove_assignees: unassign
                    .iter()
                    .map(|a| parse_pubkey(a))
                    .collect::<Result<_, _>>()?,
            };
            print(&update_goal(client, &channel, op).await?);
        }
        GoalsCmd::Start { channel, node } => {
            let op = status_op(&node, GoalStatus::InProgress);
            print(&update_goal(client, &channel, op).await?);
        }
        GoalsCmd::Done {
            channel,
            node,
            note,
        } => {
            let op = status_note_op(&node, GoalStatus::Done, optional_text(note)?);
            print(&update_goal(client, &channel, op).await?);
        }
        GoalsCmd::Move {
            channel,
            node,
            parent,
            order,
        } => {
            let op = GoalOp::Move {
                id: node,
                parent,
                order,
            };
            print(&write(client, &channel, Edit::Ops(vec![op])).await?);
        }
        GoalsCmd::Remove {
            channel,
            node,
            recursive,
        } => {
            let op = GoalOp::Remove {
                id: node,
                recursive,
            };
            print(&write(client, &channel, Edit::Ops(vec![op])).await?);
        }
        GoalsCmd::Link {
            channel,
            node,
            thread,
        } => {
            validate_hex64(&thread)?;
            let op = GoalOp::Link { id: node, thread };
            print(&write(client, &channel, Edit::Ops(vec![op])).await?);
        }
        GoalsCmd::Unlink { channel, thread } => {
            validate_hex64(&thread)?;
            let op = GoalOp::Unlink { thread };
            print(&write(client, &channel, Edit::Ops(vec![op])).await?);
        }
        GoalsCmd::History { channel, limit } => {
            parse_uuid(&channel)?;
            let filter = json!({
                "kinds": [KIND_GOAL_TREE], "#h": [channel], "limit": limit.clamp(1, 200),
            });
            let revisions: Vec<Value> = query_events(client, filter)
                .await?
                .iter()
                .map(|event| {
                    let nodes = parse_head(event).map(|h| h.tree.nodes.len()).ok();
                    json!({
                        "revision": event.get("id"),
                        "created_at": event.get("created_at"),
                        "author": event.get("pubkey"),
                        "goals": nodes,
                    })
                })
                .collect();
            print(&json!({ "channel": channel, "revisions": revisions }));
        }
        GoalsCmd::Restore { channel, revision } => {
            parse_uuid(&channel)?;
            validate_hex64(&revision)?;
            let filter = json!({
                "ids": [revision], "kinds": [KIND_GOAL_TREE], "#h": [channel], "limit": 1,
            });
            let events = query_events(client, filter).await?;
            let old = events.first().ok_or_else(|| {
                CliError::NotFound(format!(
                    "revision {revision} not found for channel {channel}"
                ))
            })?;
            let tree = parse_head(old)?.tree;
            print(&write(client, &channel, Edit::Replace(tree)).await?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn tree() -> GoalTree {
        let mut tree = GoalTree::empty();
        let ops = [
            GoalOp::SetRoot {
                id: "r".into(),
                title: "Root".into(),
                note: None,
            },
            GoalOp::Add {
                id: "a".into(),
                parent: "r".into(),
                title: "A".into(),
                note: None,
                assignees: vec![],
            },
            GoalOp::Add {
                id: "b".into(),
                parent: "r".into(),
                title: "B".into(),
                note: None,
                assignees: vec![],
            },
            GoalOp::Add {
                id: "a1".into(),
                parent: "a".into(),
                title: "A1".into(),
                note: None,
                assignees: vec![],
            },
        ];
        for op in &ops {
            tree.apply(op, ED, 1).unwrap();
        }
        tree
    }

    #[test]
    fn focus_includes_path_subtree_and_same_layer() {
        let focus = focus_json(&tree(), "a").unwrap();
        let ids = |key: &str| -> Vec<String> {
            focus[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n["id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(ids("path"), ["r", "a"]);
        assert_eq!(ids("subtree"), ["a", "a1"]);
        assert_eq!(ids("same_layer"), ["b"]);
        assert_eq!(focus["layer"], 2);
    }

    #[test]
    fn add_with_thread_links_and_starts_in_one_write() {
        let thread = "e".repeat(64);
        let add = GoalOp::Add {
            id: "n".into(),
            parent: "a".into(),
            title: "New".into(),
            note: None,
            assignees: vec![],
        };
        assert_eq!(add_ops(add.clone(), None).len(), 1);
        let ops = add_ops(add, Some(thread.clone()));
        assert_eq!(ops.len(), 2);
        let mut tree = tree();
        for op in &ops {
            tree.apply(op, ED, 2).unwrap();
        }
        let node = tree.node_for_thread(&thread).unwrap();
        assert_eq!(
            (node.id.as_str(), node.status),
            ("n", GoalStatus::InProgress)
        );
    }

    #[test]
    fn done_reports_open_sub_goals_and_finished_siblings_without_closing_the_parent() {
        let mut tree = tree();
        // a has an open sub-goal a1; b is still open.
        tree.apply(&status_op("a", GoalStatus::Done), ED, 2)
            .unwrap();
        let report = done_report(&tree, "a");
        assert_eq!(report["open_sub_goals"], 1);
        assert!(report["warning"].as_str().unwrap().contains("still open"));
        assert!(report.get("parent_all_done").is_none());

        tree.apply(&status_op("b", GoalStatus::Done), ED, 3)
            .unwrap();
        let report = done_report(&tree, "b");
        assert_eq!(report["parent_all_done"], "r");
        assert!(report.get("open_sub_goals").is_none());
        assert_eq!(
            tree.node("r").unwrap().status,
            GoalStatus::Open,
            "parent untouched"
        );

        tree.apply(&status_op("a1", GoalStatus::Done), ED, 4)
            .unwrap();
        let report = done_report(&tree, "a1");
        assert_eq!(report["parent_all_done"], json!(null), "a is already done");
    }

    #[test]
    fn start_and_done_are_status_updates() {
        assert_eq!(
            status_note_op("a", GoalStatus::Done, Some("shipped".into())),
            GoalOp::Update {
                id: "a".into(),
                title: None,
                note: Some("shipped".into()),
                status: Some(GoalStatus::Done),
                add_assignees: vec![],
                remove_assignees: vec![],
            }
        );
    }

    #[test]
    fn conflict_detection_matches_goal_tree_only() {
        let goal = CliError::Relay {
            status: 409,
            body: "conflict: goal tree changed since it was loaded".into(),
        };
        let canvas = CliError::Relay {
            status: 409,
            body: "conflict: canvas changed since it was loaded".into(),
        };
        assert!(is_goal_conflict(&goal));
        assert!(!is_goal_conflict(&canvas));
    }

    #[test]
    fn status_and_pubkey_parsing() {
        assert_eq!(parse_status("in_progress").unwrap(), GoalStatus::InProgress);
        assert!(parse_status("finished").is_err());
        assert!(parse_pubkey("nothex").is_err());
        assert_eq!(parse_pubkey(ED).unwrap(), ED);
    }
}
