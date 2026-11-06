//! Goal reads over `/query`. The NIP-98 signer is the reader and needs the
//! same access as reading the conversation's messages.
//!
//! ```json
//! {"goals": 1, "channel": "<uuid>", "view": "active" | "inactive" | "done",
//!  "cursor": {"order": 12, "id": "g_1"}}
//! {"goals": 1, "channel": "<uuid>", "threads": ["<event-hex>", ..]}
//! ```
//!
//! `view` reads one list in pages, `threads` reads what those threads work
//! on; a request names one of them. The reply is the selected rows plus the
//! goals above them, with everything computed on the server:
//!
//! ```json
//! {"goals": 1, "counts": {"active": 3, "inactive": 1, "done": 7},
//!  "rows": [{"id": "g_1", "parent": "root", "title": "..", "note": "..",
//!            "status": "open", "order": 12, "assignees": [], "links": [],
//!            "updated_by": "<hex>", "updated_at": 1760000000, "matched": true,
//!            "inactive": false, "is_thread": false,
//!            "progress": {"done": 1, "total": 2}, "open_children": 1}],
//!  "threads": [{"thread": "<event-hex>", "goal": "g_1"}], "next": null}
//! ```
//!
//! Reads always come from the writer, so `consistency` is accepted and has
//! nothing to change. A conversation with goals off reads as empty. While `KUZZ_GOAL_STORE` is
//! off the request answers `404 {"error": "goal_store_disabled"}`.

use std::sync::Arc;

use axum::{http::StatusCode, Json};
use kuzz_core::goal_store::{GoalProgress, GoalRead, GoalReadRow, GoalReadThread, GoalView};
use kuzz_core::TenantContext;
use kuzz_db::goals::{
    GoalCounts, GoalCursor, GoalPage, GoalPageRow, GoalSelection, MAX_GOAL_THREADS,
};
use kuzz_db::DbError;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{api_error, db_read_error, internal_error};
use crate::state::AppState;

type QueryError = (StatusCode, Json<Value>);

/// Error body while the goal store switch is off.
const DISABLED: &str = "goal_store_disabled";

pub(super) fn requested(filters: &[Value]) -> bool {
    filters.iter().any(|f| f.get("goals").is_some())
}

fn bad(message: &str) -> QueryError {
    api_error(StatusCode::BAD_REQUEST, message)
}

fn no_cursor(object: &serde_json::Map<String, Value>) -> Result<(), QueryError> {
    if object.get("cursor").is_some_and(|c| !c.is_null()) {
        return Err(bad("goals threads and nodes take no cursor"));
    }
    Ok(())
}

fn parse_nodes(nodes: &Value) -> Result<Vec<String>, QueryError> {
    let nodes = nodes
        .as_array()
        .filter(|n| !n.is_empty() && n.len() <= MAX_GOAL_THREADS)
        .ok_or_else(|| bad(&format!("goals nodes must list 1..={MAX_GOAL_THREADS} ids")))?;
    nodes
        .iter()
        .map(|n| n.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| bad("goals nodes must be strings"))
}

fn parse(filters: &[Value]) -> Result<(Uuid, GoalSelection), QueryError> {
    let [filter] = filters else {
        return Err(bad("goals requires exactly one filter"));
    };
    let object = filter
        .as_object()
        .ok_or_else(|| bad("goals filter must be an object"))?;
    if let Some(key) = object.keys().find(|key| {
        !matches!(
            key.as_str(),
            "goals" | "channel" | "view" | "threads" | "nodes" | "cursor" | "consistency"
        )
    }) {
        return Err(bad(&format!("unknown goals field {key}")));
    }
    super::resolve_read_route(filter)
        .map_err(|()| bad("consistency must be \"strong\" when present"))?;
    if object.get("goals").and_then(Value::as_u64) != Some(1) {
        return Err(bad("goals version must be 1"));
    }
    let channel = object
        .get("channel")
        .and_then(Value::as_str)
        .and_then(|c| Uuid::parse_str(c).ok())
        .ok_or_else(|| bad("goals channel must be a uuid"))?;
    let selection = match (
        object.get("view"),
        object.get("threads"),
        object.get("nodes"),
    ) {
        (Some(view), None, None) => {
            let view = view
                .as_str()
                .and_then(GoalView::parse)
                .ok_or_else(|| bad("goals view must be active, inactive, or done"))?;
            let after = object
                .get("cursor")
                .filter(|c| !c.is_null())
                .map(|c| {
                    serde_json::from_value::<GoalCursor>(c.clone())
                        .map_err(|_| bad("goals cursor is not valid"))
                })
                .transpose()?;
            GoalSelection::View { view, after }
        }
        (None, Some(threads), None) => {
            no_cursor(object)?;
            GoalSelection::Threads(parse_threads(threads)?)
        }
        (None, None, Some(nodes)) => {
            no_cursor(object)?;
            GoalSelection::Goals(parse_nodes(nodes)?)
        }
        _ => return Err(bad("goals needs one of view, threads, or nodes")),
    };
    Ok((channel, selection))
}

fn parse_threads(value: &Value) -> Result<Vec<[u8; 32]>, QueryError> {
    let threads = value
        .as_array()
        .filter(|t| !t.is_empty() && t.len() <= MAX_GOAL_THREADS)
        .ok_or_else(|| {
            bad(&format!(
                "goals threads must list 1..={MAX_GOAL_THREADS} ids"
            ))
        })?;
    threads
        .iter()
        .map(|thread| {
            thread
                .as_str()
                .and_then(|t| hex::decode(t).ok())
                .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                .ok_or_else(|| bad("goals thread id must be 64 hex characters"))
        })
        .collect()
}

pub(super) async fn query(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    reader: &nostr::PublicKey,
    filters: &[Value],
) -> Result<Json<Value>, QueryError> {
    if !state.config.goal_store {
        return Err(api_error(StatusCode::NOT_FOUND, DISABLED));
    }
    let (channel_id, selection) = parse(filters)?;
    let reader_bytes = reader.to_bytes();
    let accessible = state
        .get_accessible_channel_ids_cached(tenant.community(), &reader_bytes)
        .await
        .map_err(|e| internal_error(&format!("channel access lookup: {e}")))?;
    let can_read = accessible.contains(&channel_id)
        || state
            .db
            .can_read_requested_channel(tenant.community(), channel_id, &reader_bytes)
            .await
            .map_err(|e| internal_error(&format!("channel access lookup: {e}")))?;
    if !can_read {
        return Err(api_error(StatusCode::FORBIDDEN, "channel_not_readable"));
    }
    let channel = match state.db.get_channel(tenant.community(), channel_id).await {
        Ok(channel) => channel,
        Err(DbError::ChannelNotFound(_)) => {
            return Err(api_error(StatusCode::NOT_FOUND, "channel_not_found"))
        }
        Err(e) => return Err(db_read_error("channel lookup error", &e)),
    };
    if !channel.goals_enabled {
        return respond(&empty_page());
    }
    let page = state
        .db
        .read_goals(tenant.community(), channel_id, &selection)
        .await
        .map_err(|e| db_read_error("goal read error", &e))?;
    respond(&page)
}

fn empty_page() -> GoalPage {
    GoalPage {
        rows: Vec::new(),
        counts: GoalCounts::default(),
        threads: Vec::new(),
        next: None,
    }
}

fn respond(page: &GoalPage) -> Result<Json<Value>, QueryError> {
    page_json(page)
        .map(Json)
        .map_err(|e| internal_error(&format!("goal response: {e}")))
}

fn read_row(row: &GoalPageRow) -> GoalReadRow {
    let goal = &row.goal;
    GoalReadRow {
        id: goal.id.clone(),
        parent: goal.parent.clone(),
        title: goal.title.clone(),
        note: goal.note.clone(),
        status: goal.status,
        order: goal.order,
        assignees: goal.assignees.clone(),
        links: goal.links.clone(),
        updated_by: goal.updated_by.clone(),
        updated_at: goal.updated_at,
        matched: row.matched,
        inactive: goal.inactive,
        is_thread: goal.is_thread,
        progress: GoalProgress {
            done: row.progress.0,
            total: row.progress.1,
        },
        open_children: row.open_children,
    }
}

fn page_json(page: &GoalPage) -> Result<Value, serde_json::Error> {
    let threads = page
        .threads
        .iter()
        .map(|(thread, goal)| GoalReadThread {
            thread: hex::encode(thread),
            goal: goal.clone(),
        })
        .collect();
    let mut body = serde_json::to_value(GoalRead {
        counts: page.counts,
        rows: page.rows.iter().map(read_row).collect(),
        threads,
        next: page.next.clone(),
    })?;
    body["goals"] = json!(1);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuzz_db::goals::GoalRow;

    const CHANNEL: &str = "3bc405e6-3f14-44fa-a0cb-20f8ba8d879a";

    fn message(result: Result<(Uuid, GoalSelection), QueryError>) -> String {
        let (_, Json(body)) = result.unwrap_err();
        body.to_string()
    }

    #[test]
    fn a_view_reads_with_or_without_a_cursor() {
        let (channel, selection) =
            parse(&[json!({"goals": 1, "channel": CHANNEL, "view": "inactive"})]).unwrap();
        assert_eq!(channel.to_string(), CHANNEL);
        assert_eq!(
            selection,
            GoalSelection::View {
                view: GoalView::Inactive,
                after: None
            }
        );
        assert!(parse(&[
            json!({"goals": 1, "channel": CHANNEL, "view": "done", "consistency": "weak"})
        ])
        .is_err());
        // `next` is null on the last page, and clients send it straight back.
        parse(&[
            json!({"goals": 1, "channel": CHANNEL, "view": "done", "cursor": null,
            "consistency": "strong"}),
        ])
        .unwrap();
        let (_, selection) = parse(&[json!({"goals": 1, "channel": CHANNEL, "view": "done",
            "cursor": {"order": 3, "id": "g"}})])
        .unwrap();
        assert_eq!(
            selection,
            GoalSelection::View {
                view: GoalView::Done,
                after: Some(GoalCursor {
                    order: 3,
                    id: "g".into()
                })
            }
        );
    }

    #[test]
    fn threads_read_by_event_id() {
        let id = "ab".repeat(32);
        let (_, selection) =
            parse(&[json!({"goals": 1, "channel": CHANNEL, "threads": [id]})]).unwrap();
        assert_eq!(selection, GoalSelection::Threads(vec![[0xab; 32]]));
    }

    #[test]
    fn bad_requests_are_refused() {
        let cases = [
            (
                json!({"goals": 2, "channel": CHANNEL, "view": "active"}),
                "version",
            ),
            (
                json!({"goals": 1, "channel": "nope", "view": "active"}),
                "uuid",
            ),
            (json!({"goals": 1, "channel": CHANNEL}), "one of"),
            (
                json!({"goals": 1, "channel": CHANNEL, "view": "active", "threads": ["a"]}),
                "one of",
            ),
            (
                json!({"goals": 1, "channel": CHANNEL, "view": "gone"}),
                "view",
            ),
            (
                json!({"goals": 1, "channel": CHANNEL, "view": "active", "x": 1}),
                "unknown",
            ),
            (
                json!({"goals": 1, "channel": CHANNEL, "threads": []}),
                "threads",
            ),
            (
                json!({"goals": 1, "channel": CHANNEL, "threads": ["zz"]}),
                "64 hex",
            ),
            (
                json!({"goals": 1, "channel": CHANNEL, "threads": ["ab"], "cursor": {}}),
                "no cursor",
            ),
        ];
        for (filter, expected) in cases {
            let text = message(parse(std::slice::from_ref(&filter)));
            assert!(text.contains(expected), "{filter}: {text}");
        }
        let too_many = vec!["ab".repeat(32); MAX_GOAL_THREADS + 1];
        let filter = json!({"goals": 1, "channel": CHANNEL, "threads": too_many});
        assert!(message(parse(&[filter])).contains("threads"));
        let one = json!({"goals": 1, "channel": CHANNEL, "view": "active"});
        assert!(message(parse(&[one.clone(), one])).contains("exactly one"));
    }

    #[test]
    fn rows_carry_the_computed_values() {
        let page = GoalPage {
            rows: vec![GoalPageRow {
                goal: GoalRow {
                    id: "g".into(),
                    parent: Some("root".into()),
                    title: "t".into(),
                    note: String::new(),
                    status: kuzz_core::goal_tree::GoalStatus::InProgress,
                    order: 4,
                    assignees: Vec::new(),
                    links: vec!["aa".into()],
                    updated_by: None,
                    updated_at: 9,
                    is_thread: true,
                    inactive: false,
                },
                matched: true,
                progress: (1, 2),
                open_children: 1,
            }],
            counts: GoalCounts {
                active: 1,
                inactive: 0,
                done: 0,
            },
            threads: vec![([1; 32], None)],
            next: Some(GoalCursor {
                order: 4,
                id: "g".into(),
            }),
        };
        let value = page_json(&page).unwrap();
        assert_eq!(value["counts"]["active"], 1);
        assert_eq!(value["rows"][0]["status"], "in_progress");
        assert_eq!(value["rows"][0]["progress"]["total"], 2);
        assert_eq!(value["rows"][0]["is_thread"], true);
        assert_eq!(value["threads"][0]["goal"], Value::Null);
        assert_eq!(value["next"]["order"], 4);
    }

    #[test]
    fn a_conversation_with_goals_off_reads_as_empty() {
        let value = page_json(&empty_page()).unwrap();
        assert_eq!(value["rows"], json!([]));
        assert_eq!(value["counts"]["active"], 0);
    }
}
