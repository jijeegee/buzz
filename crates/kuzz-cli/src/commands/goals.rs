//! `kuzz goals` — read and edit a channel's or DM's goals.
//!
//! Edits go to the relay as kind 40111 ops, which it applies to the stored
//! goals, so there is nothing to compose against and nothing to retry. Reads
//! are `{"goals":1}` queries; finished and quiet goals only come with `--all`.

use kuzz_core::goal_store::{GoalOps, GoalOutcome, GoalRead, GoalReadRow};
use kuzz_core::goal_tree::{new_node_id, GoalOp, GoalStatus};
use kuzz_core::kind::KIND_GOAL_OPS;
use serde_json::{json, Value};

use crate::client::KuzzClient;
use crate::error::CliError;
use crate::validate::{parse_uuid, read_or_stdin, validate_hex64};
use crate::GoalsCmd;

async fn read(client: &KuzzClient, filter: Value) -> Result<GoalRead, CliError> {
    let resp = client.query(&filter).await?;
    serde_json::from_str(&resp)
        .map_err(|e| CliError::Other(format!("malformed relay response reading goals: {e}")))
}

/// One whole list.
async fn read_view(client: &KuzzClient, channel: &str, view: &str) -> Result<GoalRead, CliError> {
    let filter = json!({ "goals": 1, "channel": channel, "view": view });
    let mut all = read(client, filter.clone()).await?;
    while let Some(cursor) = all.next.take() {
        let mut filter = filter.clone();
        filter["cursor"] = json!(cursor);
        all.absorb(read(client, filter).await?);
    }
    Ok(all)
}

/// What a thread works on: its goal, if linked, with the goal's context.
pub(crate) async fn read_thread(
    client: &KuzzClient,
    channel: &str,
    thread: &str,
) -> Result<GoalRead, CliError> {
    let filter = json!({ "goals": 1, "channel": channel, "threads": [thread] });
    read(client, filter).await
}

/// These goals with their context. An unknown id reads as nothing.
async fn read_nodes(
    client: &KuzzClient,
    channel: &str,
    ids: &[&str],
) -> Result<GoalRead, CliError> {
    let filter = json!({ "goals": 1, "channel": channel, "nodes": ids });
    read(client, filter).await
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

fn unfinished(row: &GoalReadRow) -> bool {
    matches!(row.status, GoalStatus::Open | GoalStatus::InProgress)
}

/// What finishing `id` means for the goals around it: sub-goals still active,
/// and whether nothing active is left under its parent. `read` is a read of
/// `id` and its parent after the write; `id` is gone from it if it was
/// deleted. The parent is never completed automatically; people decide that.
fn done_report(read: &GoalRead, id: &str, parent: Option<&str>) -> Value {
    let open_sub_goals = read
        .subtree(id)
        .iter()
        .filter(|(_, r)| r.id != id && unfinished(r) && !r.inactive)
        .count();
    let mut out = json!({});
    if open_sub_goals > 0 {
        out["open_sub_goals"] = json!(open_sub_goals);
        out["warning"] = json!(format!(
            "goal {id} is done but {open_sub_goals} of its sub-goals are still open"
        ));
    }
    if let Some(parent) = parent.and_then(|p| read.row(p)) {
        if unfinished(parent) && parent.open_children == 0 {
            out["parent_all_done"] = json!(parent.id);
        }
    }
    out
}

/// The one status/title/note change path behind `update`, `start`, `done`,
/// and `threads close --goal-done`.
pub(crate) async fn update_goal(
    client: &KuzzClient,
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
    let (mut out, outcome) = write(client, channel, vec![op]).await?;
    let Some(id) = done else {
        return Ok(out);
    };
    let mut ids = vec![id.as_str()];
    ids.extend(outcome.parent.as_deref());
    // The edit is saved by now, so a failed read only loses the report.
    match read_nodes(client, channel, &ids).await {
        Ok(read) => {
            if let (Some(out), Value::Object(report)) = (
                out.as_object_mut(),
                done_report(&read, &id, outcome.parent.as_deref()),
            ) {
                out.extend(report);
            }
        }
        Err(e) => out["report_error"] = json!(e.to_string()),
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
    // Linking starts the goal, so no separate status op.
    match thread {
        Some(thread) => vec![add, GoalOp::Link { id, thread }],
        None => vec![add],
    }
}

/// The relay's answer to a goal edit. A retry of a stored event answers
/// `duplicate:`, with nothing to report.
fn parse_outcome(ack: &str) -> GoalOutcome {
    serde_json::from_str::<Value>(ack)
        .ok()
        .and_then(|ack| serde_json::from_str(ack.get("message")?.as_str()?).ok())
        .unwrap_or_default()
}

/// What the writer sees of an edit. Missing ids are the writer's cue: a wrong
/// id must not read as a finished goal.
fn write_json(event_id: &str, outcome: &GoalOutcome) -> Value {
    let mut out = json!({ "event_id": event_id, "accepted": true });
    if !outcome.deleted.is_empty() {
        out["deleted"] = json!(outcome.deleted);
    }
    if !outcome.missing.is_empty() {
        out["missing"] = json!(outcome.missing);
        out["warning"] = json!(format!(
            "no goal with id {}; nothing changed for it",
            outcome.missing.join(", ")
        ));
    }
    out
}

/// The relay enforces the goal rules, so a broken one is the caller's to fix,
/// not a relay failure.
fn refused_edit(error: CliError) -> CliError {
    match error {
        CliError::Relay { status: 400, body } if body.contains("invalid: ") => {
            CliError::Usage(body)
        }
        other => other,
    }
}

/// Send one edit; the ops apply in order, all or none.
pub(crate) async fn write(
    client: &KuzzClient,
    channel: &str,
    ops: Vec<GoalOp>,
) -> Result<(Value, GoalOutcome), CliError> {
    let builder =
        kuzz_sdk::build_goal_ops(parse_uuid(channel)?, &ops).map_err(crate::validate::sdk_err)?;
    let event = client.sign_event(builder)?;
    let event_id = event.id.to_hex();
    let ack = client.submit_event(event).await.map_err(refused_edit)?;
    let outcome = parse_outcome(&ack);
    Ok((write_json(&event_id, &outcome), outcome))
}

/// [`write`] when the outcome only needs printing.
async fn write_print(client: &KuzzClient, channel: &str, ops: Vec<GoalOp>) -> Result<(), CliError> {
    print(&write(client, channel, ops).await?.0);
    Ok(())
}

fn row_json(layer: usize, row: &GoalReadRow) -> Value {
    json!({
        "id": row.id,
        "layer": layer,
        "parent": row.parent,
        "title": row.title,
        "note": row.note,
        "status": row.status.as_str(),
        "inactive": row.inactive,
        "assignees": row.assignees,
        "threads": row.links,
        "progress": row.progress,
        "updated_by": row.updated_by,
        "updated_at": row.updated_at,
    })
}

/// A goal's working context: the path above it, its open sub-goals, and the
/// active goals beside it (to avoid overlap and spot gaps).
fn focus_json(read: &GoalRead, id: &str) -> Result<Value, CliError> {
    let path = read.path(id);
    let layer = path.len();
    if layer == 0 {
        return Err(CliError::NotFound(format!("goal {id} not found")));
    }
    let path: Vec<Value> = path
        .iter()
        .enumerate()
        .map(|(i, r)| row_json(i + 1, r))
        .collect();
    let subtree: Vec<Value> = read
        .subtree(id)
        .iter()
        .map(|(l, r)| row_json(*l, r))
        .collect();
    let same_layer: Vec<Value> = read
        .outline()
        .iter()
        .filter(|(l, r)| *l == layer && r.id != id)
        .map(|(l, r)| row_json(*l, r))
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

pub async fn dispatch(cmd: GoalsCmd, client: &KuzzClient) -> Result<(), CliError> {
    match cmd {
        GoalsCmd::Get {
            channel,
            node,
            thread,
            all,
        } => {
            parse_uuid(&channel)?;
            if let Some(thread) = &thread {
                validate_hex64(thread)?;
            }
            let mut goals = read_view(client, &channel, "active").await?;
            if all {
                goals.absorb(read_view(client, &channel, "inactive").await?);
                goals.absorb(read_view(client, &channel, "done").await?);
            }
            let this_thread_goal = match &thread {
                Some(thread) => read_thread(client, &channel, thread)
                    .await?
                    .threads
                    .into_iter()
                    .find_map(|t| t.goal),
                None => None,
            };
            let focus_id = node.or_else(|| this_thread_goal.clone());
            let outline: Vec<Value> = goals
                .outline()
                .iter()
                .map(|(l, r)| row_json(*l, r))
                .collect();
            let mut out = json!({
                "channel": channel,
                "counts": goals.counts,
                "outline": outline,
            });
            if thread.is_some() {
                // The goal the asked-about thread works on, or null.
                out["this_thread_goal"] = json!(this_thread_goal);
            }
            if let Some(id) = focus_id {
                let context = read_nodes(client, &channel, &[&id]).await?;
                out["focus"] = focus_json(&context, &id)?;
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
            write_print(client, &channel, vec![op]).await?;
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
            let (mut out, _) = write(client, &channel, add_ops(op, thread)).await?;
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
            write_print(client, &channel, vec![op]).await?;
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
            write_print(client, &channel, vec![op]).await?;
        }
        GoalsCmd::Link {
            channel,
            node,
            thread,
            message,
        } => {
            // A message link shares the goal's thread list: a message is the
            // root of its thread once someone replies.
            let thread = thread
                .or(message)
                .ok_or_else(|| CliError::Usage("pass --thread or --message".into()))?;
            validate_hex64(&thread)?;
            let op = GoalOp::Link { id: node, thread };
            write_print(client, &channel, vec![op]).await?;
        }
        GoalsCmd::Unlink { channel, thread } => {
            validate_hex64(&thread)?;
            let op = GoalOp::Unlink { thread };
            write_print(client, &channel, vec![op]).await?;
        }
        GoalsCmd::History { channel, limit } => {
            parse_uuid(&channel)?;
            let filter = json!({
                "kinds": [KIND_GOAL_OPS], "#h": [channel], "limit": limit.clamp(1, 200),
            });
            let resp = client.query(&filter).await?;
            let events: Vec<Value> = serde_json::from_str(&resp).map_err(|e| {
                CliError::Other(format!("malformed relay response listing goal edits: {e}"))
            })?;
            let edits: Vec<Value> = events
                .iter()
                .map(|event| {
                    let ops = event
                        .get("content")
                        .and_then(Value::as_str)
                        .and_then(|c| serde_json::from_str::<GoalOps>(c).ok())
                        .map(|c| c.ops);
                    json!({
                        "event_id": event.get("id"),
                        "created_at": event.get("created_at"),
                        "author": event.get("pubkey"),
                        "ops": ops,
                    })
                })
                .collect();
            print(&json!({ "channel": channel, "edits": edits }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuzz_core::goal_store::{GoalProgress, GoalReadRow};

    const ED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn row(id: &str, parent: Option<&str>, status: GoalStatus) -> GoalReadRow {
        GoalReadRow {
            id: id.into(),
            parent: parent.map(Into::into),
            title: id.into(),
            note: String::new(),
            status,
            order: 0,
            assignees: vec![],
            links: vec![],
            updated_by: None,
            updated_at: 0,
            matched: true,
            inactive: false,
            is_thread: false,
            progress: GoalProgress::default(),
            open_children: 0,
        }
    }

    fn goals(rows: Vec<GoalReadRow>) -> GoalRead {
        GoalRead {
            rows,
            ..GoalRead::default()
        }
    }

    #[test]
    fn focus_includes_path_subtree_and_same_layer() {
        let read = goals(vec![
            row("r", None, GoalStatus::Open),
            row("a", Some("r"), GoalStatus::Open),
            row("b", Some("r"), GoalStatus::Open),
            row("a1", Some("a"), GoalStatus::Open),
        ]);
        let focus = focus_json(&read, "a").unwrap();
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
        assert!(matches!(
            focus_json(&read, "gone"),
            Err(CliError::NotFound(_))
        ));
    }

    #[test]
    fn add_with_thread_is_one_write() {
        let add = GoalOp::Add {
            id: "n".into(),
            parent: "a".into(),
            title: "New".into(),
            note: None,
            assignees: vec![],
        };
        assert_eq!(add_ops(add.clone(), None).len(), 1);
        let thread = "e".repeat(64);
        let ops = add_ops(add, Some(thread.clone()));
        assert_eq!(ops.len(), 2);
        assert_eq!(
            ops[1],
            GoalOp::Link {
                id: "n".into(),
                thread
            }
        );
    }

    #[test]
    fn done_reports_open_sub_goals_and_a_finished_level_without_closing_the_parent() {
        let mut quiet = row("a2", Some("a"), GoalStatus::Open);
        quiet.inactive = true;
        let read = goals(vec![
            row("r", None, GoalStatus::Open),
            row("a", Some("r"), GoalStatus::Done),
            row("a1", Some("a"), GoalStatus::Open),
            quiet,
        ]);
        let report = done_report(&read, "a", Some("r"));
        assert_eq!(report["open_sub_goals"], 1, "the quiet one does not count");
        assert!(report["warning"].as_str().unwrap().contains("still open"));
        assert_eq!(report["parent_all_done"], "r");

        let mut busy = row("r", None, GoalStatus::Open);
        busy.open_children = 1;
        let report = done_report(&goals(vec![busy]), "gone", Some("r"));
        assert_eq!(
            report,
            json!({}),
            "a deleted goal has no sub-goals to report"
        );

        let finished = goals(vec![row("r", None, GoalStatus::Done)]);
        assert_eq!(done_report(&finished, "gone", Some("r")), json!({}));
    }

    #[test]
    fn the_relay_outcome_reaches_the_writer() {
        let ack = json!({
            "event_id": "e", "accepted": true,
            "message": r#"{"parent":"p","deleted":["t"],"missing":["x"]}"#,
        })
        .to_string();
        let outcome = parse_outcome(&ack);
        assert_eq!(outcome.parent.as_deref(), Some("p"));
        let out = write_json("e", &outcome);
        assert_eq!(out["deleted"], json!(["t"]));
        assert_eq!(out["missing"], json!(["x"]));
        assert!(out["warning"].as_str().unwrap().contains('x'));

        let replay = json!({ "accepted": true, "message": "duplicate:" }).to_string();
        let out = write_json("e", &parse_outcome(&replay));
        assert_eq!(out, json!({ "event_id": "e", "accepted": true }));
    }

    #[test]
    fn a_refused_edit_is_the_callers_error() {
        let refused = CliError::Relay {
            status: 400,
            body: r#"{"error":"invalid: goal not found: x"}"#.into(),
        };
        assert!(matches!(refused_edit(refused), CliError::Usage(_)));
        let other = CliError::Relay {
            status: 400,
            body: "bad request".into(),
        };
        assert!(matches!(refused_edit(other), CliError::Relay { .. }));
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
    fn link_takes_a_thread_or_a_message_but_not_both() {
        use clap::Parser;
        let id = "e".repeat(64);
        let parse = |extra: &[&str]| {
            let mut args = vec!["kuzz", "goals", "link", "--channel", "c", "--node", "g"];
            args.extend_from_slice(extra);
            crate::Cli::try_parse_from(args).map(|cli| cli.command)
        };
        let linked = |extra: &[&str]| match parse(extra) {
            Ok(crate::Cmd::Goals(GoalsCmd::Link {
                thread, message, ..
            })) => thread.or(message),
            _ => None,
        };
        assert_eq!(linked(&["--message", &id]), Some(id.clone()));
        assert_eq!(linked(&["--thread", &id]), Some(id.clone()));
        assert!(parse(&[]).is_err());
        assert!(parse(&["--thread", &id, "--message", &id]).is_err());
    }

    #[test]
    fn status_and_pubkey_parsing() {
        assert_eq!(parse_status("in_progress").unwrap(), GoalStatus::InProgress);
        assert!(parse_status("finished").is_err());
        assert!(parse_pubkey("nothex").is_err());
        assert_eq!(parse_pubkey(ED).unwrap(), ED);
    }
}
