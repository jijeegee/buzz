//! `chat_import` v1: a new app's first-run upload of its device-local read
//! positions and follows (Chats design §5).
//!
//! ```json
//! {"chat_import": 1,
//!  "reads": [{"channel": "<uuid>", "at": 1760000000},
//!            {"thread": "<root-hex>", "at": 1760000000}],
//!  "follows": [{"root": "<root-hex>", "state": "followed" | "unfollowed"}]}
//! ```
//!
//! `reads` and `follows` may each be omitted; together they carry 1..=500
//! items, each room, thread, and follow root at most once. `at` is whole Unix
//! seconds, at most 300 s past the relay's clock (milliseconds are refused),
//! compared with each message's `created_at` (the signer's time). The reply
//! has one result per item, in request order:
//!
//! ```json
//! {"chat_import": 1,
//!  "reads": [{"channel": "<uuid>", "applied": true, "reason": null,
//!             "channel_id": "<uuid>", "read_seq": 12}],
//!  "follows": [{"root": "<root-hex>", "state": "unfollowed", "applied": false,
//!               "reason": "already_set", "channel_id": "<uuid>"}]}
//! ```
//!
//! `reason` is null when applied, else `not_member`, `unknown_thread`,
//! `not_thread_root`, `not_followed` (thread read with no follow row), or
//! `already_set` (the server already holds the reader's own state). Skipped
//! items never fail the request.

use std::collections::HashSet;

use kuzz_db::chat_import::{
    ChatImportOutcome, ImportFollow, ImportItemResult, ImportRead, MAX_CHAT_IMPORT_ITEMS,
};
use kuzz_db::chat_user_state::{FollowState, MarkReadTarget};
use serde_json::{json, Map, Value};

use super::{bad, envelope_object, event_id, read_target, unix_seconds, Envelope, QueryError};

/// Parsed upload.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ImportRequest {
    pub(super) reads: Vec<ImportRead>,
    pub(super) follows: Vec<ImportFollow>,
}

fn items<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a [Value], QueryError> {
    match object.get(key) {
        None => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(bad(&format!("chat_import {key} must be an array"))),
    }
}

pub(super) fn parse(filters: &[Value]) -> Result<ImportRequest, QueryError> {
    let object = envelope_object(filters, Envelope::ChatImport, &["reads", "follows"])?;
    let (reads, follows) = (items(object, "reads")?, items(object, "follows")?);
    let total = reads.len() + follows.len();
    if !(1..=MAX_CHAT_IMPORT_ITEMS).contains(&total) {
        return Err(bad(&format!(
            "chat_import carries 1..={MAX_CHAT_IMPORT_ITEMS} reads and follows"
        )));
    }
    let mut seen = HashSet::new();
    let reads = reads
        .iter()
        .enumerate()
        .map(|(index, item)| {
            parse_read(item)
                .and_then(|read| {
                    seen.insert(read.target.clone())
                        .then_some(read)
                        .ok_or("names a room or thread twice")
                })
                .map_err(|message| bad(&format!("chat_import read {index}: {message}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = HashSet::new();
    let follows = follows
        .iter()
        .enumerate()
        .map(|(index, item)| {
            parse_follow(item)
                .and_then(|follow| {
                    seen.insert(follow.root.clone())
                        .then_some(follow)
                        .ok_or("names a root twice")
                })
                .map_err(|message| bad(&format!("chat_import follow {index}: {message}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ImportRequest { reads, follows })
}

fn parse_read(item: &Value) -> Result<ImportRead, &'static str> {
    let object = item.as_object().ok_or("must be an object")?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "channel" | "thread" | "at"))
    {
        return Err("accepts only channel, thread, at");
    }
    let target = read_target(object)?;
    let at = unix_seconds(object.get("at").ok_or("needs at")?)?;
    Ok(ImportRead { target, at })
}

fn parse_follow(item: &Value) -> Result<ImportFollow, &'static str> {
    let object = item.as_object().ok_or("must be an object")?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "root" | "state"))
    {
        return Err("accepts only root, state");
    }
    let root = object
        .get("root")
        .and_then(event_id)
        .ok_or("root must be a 64-hex event id")?;
    let state = match object.get("state").and_then(Value::as_str) {
        Some("followed") => FollowState::Followed,
        Some("unfollowed") => FollowState::Unfollowed,
        _ => return Err("state must be followed or unfollowed"),
    };
    Ok(ImportFollow { root, state })
}

fn result_json(mut value: Value, result: &ImportItemResult) -> Value {
    value["applied"] = json!(result.is_applied());
    value["reason"] = json!(result.skipped.map(|reason| reason.as_str()));
    value["channel_id"] = json!(result.channel_id);
    value
}

/// The reply body for `request` and its per-item `outcome`.
pub(super) fn response(request: &ImportRequest, outcome: &ChatImportOutcome) -> Value {
    let reads = request
        .reads
        .iter()
        .zip(&outcome.reads)
        .map(|(read, result)| {
            let target = match &read.target {
                MarkReadTarget::Channel(channel) => json!({ "channel": channel }),
                MarkReadTarget::Thread(root) => json!({ "thread": hex::encode(root) }),
            };
            let mut value = result_json(target, result);
            value["read_seq"] = json!(result.read_seq);
            value
        })
        .collect::<Vec<_>>();
    let follows = request
        .follows
        .iter()
        .zip(&outcome.follows)
        .map(|(follow, result)| {
            result_json(
                json!({ "root": hex::encode(&follow.root), "state": follow.state.as_str() }),
                result,
            )
        })
        .collect::<Vec<_>>();
    json!({ "chat_import": 1, "reads": reads, "follows": follows })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use kuzz_db::chat_import::ImportSkip;
    use uuid::Uuid;

    use super::*;

    const CHANNEL: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn parses_reads_and_follows() {
        let root = "ab".repeat(32);
        let request = parse(&[json!({
            "chat_import": 1,
            "reads": [{"channel": CHANNEL, "at": 1_760_000_000}, {"thread": root, "at": 5}],
            "follows": [{"root": root, "state": "unfollowed"}]
        })])
        .expect("valid");
        assert_eq!(
            request.reads,
            vec![
                ImportRead {
                    target: MarkReadTarget::Channel(Uuid::parse_str(CHANNEL).expect("uuid")),
                    at: DateTime::from_timestamp(1_760_000_000, 0).expect("time"),
                },
                ImportRead {
                    target: MarkReadTarget::Thread(vec![0xab; 32]),
                    at: DateTime::from_timestamp(5, 0).expect("time"),
                },
            ]
        );
        assert_eq!(
            request.follows,
            vec![ImportFollow {
                root: vec![0xab; 32],
                state: FollowState::Unfollowed
            }]
        );
        let only_follows = parse(&[json!({
            "chat_import": 1, "follows": [{"root": root, "state": "followed"}]
        })])
        .expect("reads may be omitted");
        assert!(only_follows.reads.is_empty());
    }

    #[test]
    fn rejects_malformed_imports() {
        let root = "ab".repeat(32);
        let read = json!({"channel": CHANNEL, "at": 1});
        let follow = json!({"root": root, "state": "followed"});
        let now = chrono::Utc::now().timestamp();
        for filter in [
            json!({"chat_import": 1, "reads": [{"channel": CHANNEL, "at": now * 1000}]}),
            json!({"chat_import": 1, "reads": [{"thread": root, "at": now + 3600}]}),
            json!({"chat_import": 2, "reads": [read]}),
            json!({"chat_import": 1}),
            json!({"chat_import": 1, "reads": [], "follows": []}),
            json!({"chat_import": 1, "reads": [read], "extra": 1}),
            json!({"chat_import": 1, "reads": read}),
            json!({"chat_import": 1, "reads": [{"channel": CHANNEL}]}),
            json!({"chat_import": 1, "reads": [{"channel": CHANNEL, "at": -1}]}),
            json!({"chat_import": 1, "reads": [{"channel": CHANNEL, "at": 1.5}]}),
            json!({"chat_import": 1, "reads": [{"channel": CHANNEL, "at": "1"}]}),
            json!({"chat_import": 1, "reads": [{"channel": CHANNEL, "at": 1, "seq": 2}]}),
            json!({"chat_import": 1, "reads": [{"channel": CHANNEL, "thread": root, "at": 1}]}),
            json!({"chat_import": 1, "reads": [{"thread": "ab", "at": 1}]}),
            json!({"chat_import": 1, "reads": [read, read]}),
            json!({"chat_import": 1, "follows": [{"root": root, "state": "auto"}]}),
            json!({"chat_import": 1, "follows": [{"root": root}]}),
            json!({"chat_import": 1, "follows": [{"root": root, "state": "followed", "x": 1}]}),
            json!({"chat_import": 1, "follows": [follow, follow]}),
        ] {
            assert!(parse(std::slice::from_ref(&filter)).is_err(), "{filter}");
        }
        assert!(
            parse(&[json!({"chat_import": 1, "reads": [{"channel": CHANNEL, "at": now}]})]).is_ok(),
            "now is accepted"
        );
        assert!(parse(&[]).is_err());
        assert!(parse(&[
            json!({"chat_import": 1, "follows": [follow]}),
            json!({"kinds": [9]})
        ])
        .is_err());
    }

    #[test]
    fn caps_items_across_reads_and_follows() {
        let reads = (0..300u32)
            .map(|n| json!({"channel": Uuid::from_u128(u128::from(n) + 1), "at": n}))
            .collect::<Vec<_>>();
        let follows = |count: u32| {
            (0..count)
                .map(|n| json!({"root": format!("{n:064x}"), "state": "followed"}))
                .collect::<Vec<_>>()
        };
        let at_cap = json!({"chat_import": 1, "reads": reads, "follows": follows(200)});
        assert_eq!(
            parse(&[at_cap]).expect("500 items").follows.len(),
            200,
            "exactly the cap is accepted"
        );
        let over = json!({"chat_import": 1, "reads": reads, "follows": follows(201)});
        assert!(parse(&[over]).is_err());
    }

    #[test]
    fn response_keeps_request_order_and_reasons() {
        let root = vec![0xcd; 32];
        let channel = Uuid::parse_str(CHANNEL).expect("uuid");
        let request = ImportRequest {
            reads: vec![ImportRead {
                target: MarkReadTarget::Thread(root.clone()),
                at: DateTime::from_timestamp(9, 0).expect("time"),
            }],
            follows: vec![ImportFollow {
                root: root.clone(),
                state: FollowState::Followed,
            }],
        };
        let outcome = ChatImportOutcome {
            reads: vec![ImportItemResult {
                channel_id: Some(channel),
                skipped: None,
                read_seq: Some(4),
            }],
            follows: vec![ImportItemResult {
                channel_id: None,
                skipped: Some(ImportSkip::UnknownThread),
                read_seq: None,
            }],
        };
        assert_eq!(
            response(&request, &outcome),
            json!({
                "chat_import": 1,
                "reads": [{"thread": hex::encode(&root), "applied": true, "reason": null,
                           "channel_id": channel, "read_seq": 4}],
                "follows": [{"root": hex::encode(&root), "state": "followed", "applied": false,
                             "reason": "unknown_thread", "channel_id": null}]
            })
        );
    }
}
