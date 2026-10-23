//! `thread_search` v1: threads in the reader's rooms whose name contains a
//! query, closed and quiet ones included (the Chats list leaves those out).
//!
//! ```json
//! {"thread_search": 1, "q": "launch", "limit": 20}
//! ```
//!
//! `q` is matched case-insensitively anywhere in the thread's newest name; it
//! is trimmed and must be 1..=100 characters. `limit` is 1..=50 (default 20).
//! The reply lists threads best match first (exact, prefix, word start,
//! middle; then most recent activity):
//!
//! ```json
//! {"thread_search": 1,
//!  "rows": [{"type": "thread", "key": "<root-hex>", "root_id": "<root-hex>",
//!            "at": 1760000000, "at_us": 1760000000000000,
//!            "channel_id": "<uuid>", "channel_type": "stream",
//!            "channel_name": "general", "archived": false,
//!            "group_chat": false, "member_count": null, "dm_members": [],
//!            "thread_name": "Launch plan", "closed": true, "match": 1}]}
//! ```
//!
//! Rows carry the `chat_list` thread-row fields a client needs to show and
//! open a thread (no counts or events). Only rooms the reader is a current,
//! non-hidden member of are searched; see `kuzz_db::thread_search`.

use kuzz_db::thread_search::{
    ThreadSearchRow, MAX_THREAD_SEARCH_LIMIT, MAX_THREAD_SEARCH_QUERY_CHARS,
};
use serde_json::{json, Value};

use super::{bad, envelope_object, Envelope, QueryError};

/// Default result count.
const DEFAULT_LIMIT: u32 = 20;

/// Parsed request: the trimmed query and the result count.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ThreadSearchRequest {
    pub(super) query: String,
    pub(super) limit: u32,
}

pub(super) fn parse(filters: &[Value]) -> Result<ThreadSearchRequest, QueryError> {
    let object = envelope_object(filters, Envelope::ThreadSearch, &["q", "limit"])?;
    let query = object
        .get("q")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|q| {
            !q.is_empty() && q.chars().count() <= MAX_THREAD_SEARCH_QUERY_CHARS && !q.contains('\0')
        })
        .ok_or_else(|| {
            bad(&format!(
                "thread_search q must be 1..={MAX_THREAD_SEARCH_QUERY_CHARS} characters"
            ))
        })?;
    let limit = match object.get("limit") {
        None => DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=MAX_THREAD_SEARCH_LIMIT).contains(n))
            .ok_or_else(|| {
                bad(&format!(
                    "thread_search limit must be 1..={MAX_THREAD_SEARCH_LIMIT}"
                ))
            })?,
    };
    Ok(ThreadSearchRequest {
        query: query.to_owned(),
        limit,
    })
}

pub(super) fn row_json(row: ThreadSearchRow) -> Value {
    let root = hex::encode(&row.root_id);
    json!({
        "type": "thread",
        "key": root,
        "root_id": root,
        "at": row.at.timestamp(),
        "at_us": row.at.timestamp_micros(),
        "channel_id": row.channel_id,
        "channel_type": row.channel_type,
        "channel_name": row.channel_name,
        "archived": row.archived,
        "group_chat": row.group_chat,
        "member_count": row.member_count,
        "dm_members": row.dm_members.iter().map(hex::encode).collect::<Vec<_>>(),
        "thread_name": row.name,
        "closed": row.closed,
        "match": row.match_rank,
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn parses_query_and_limit() {
        let request = parse(&[json!({"thread_search": 1, "q": "  Launch plan "})]).expect("ok");
        assert_eq!(
            request,
            ThreadSearchRequest {
                query: "Launch plan".into(),
                limit: DEFAULT_LIMIT
            }
        );
        let request = parse(&[json!({"thread_search": 1, "q": "검색", "limit": 50})]).expect("ok");
        assert_eq!(request.limit, 50);
        // 100 characters, more than 100 bytes.
        let long = "가".repeat(MAX_THREAD_SEARCH_QUERY_CHARS);
        assert!(parse(&[json!({"thread_search": 1, "q": long})]).is_ok());
    }

    #[test]
    fn rejects_malformed_requests() {
        for filters in [
            vec![],
            vec![json!({"thread_search": 1, "q": "a"}); 2],
            vec![json!({"thread_search": 2, "q": "a"})],
            vec![json!({"thread_search": 1})],
            vec![json!({"thread_search": 1, "q": "   "})],
            vec![json!({"thread_search": 1, "q": 5})],
            vec![json!({"thread_search": 1, "q": "a\u{0}b"})],
            vec![json!({"thread_search": 1, "q": "x".repeat(101)})],
            vec![json!({"thread_search": 1, "q": "a", "limit": 0})],
            vec![json!({"thread_search": 1, "q": "a", "limit": 51})],
            vec![json!({"thread_search": 1, "q": "a", "limit": "5"})],
            vec![json!({"thread_search": 1, "q": "a", "kinds": [9]})],
        ] {
            assert!(parse(&filters).is_err(), "{filters:?}");
        }
    }

    #[test]
    fn rows_use_the_chat_list_thread_shape() {
        let channel = Uuid::new_v4();
        let value = row_json(ThreadSearchRow {
            root_id: vec![0xab; 32],
            channel_id: channel,
            channel_type: "dm".into(),
            channel_name: String::new(),
            archived: false,
            group_chat: false,
            member_count: None,
            dm_members: vec![vec![0xcd; 32]],
            name: "Trip".into(),
            at: DateTime::from_timestamp_micros(1_760_000_000_123_456).expect("time"),
            closed: true,
            match_rank: 0,
        });
        assert_eq!(value["type"], "thread");
        assert_eq!(value["key"], "ab".repeat(32));
        assert_eq!(value["root_id"], "ab".repeat(32));
        assert_eq!(value["at"], 1_760_000_000);
        assert_eq!(value["at_us"], 1_760_000_000_123_456i64);
        assert_eq!(value["channel_id"], channel.to_string());
        assert_eq!(value["dm_members"], json!(["cd".repeat(32)]));
        assert_eq!(value["thread_name"], "Trip");
        assert_eq!(value["closed"], true);
        assert_eq!(value["match"], 0);
    }
}
