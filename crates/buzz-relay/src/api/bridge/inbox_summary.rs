//! Opt-in inbox summary envelope: one request returns the reader's member
//! rooms and active threads mixed newest first, each with its latest message.
//!
//! Request (exactly one filter object):
//! `{"inbox_summary": 1, "limit": 30, "before": {"at_us": .., "key": ".."},
//!   "read": {"<channel-uuid>": unix_secs, "thread:<root-hex>": unix_secs}}`
//!
//! `read` is used only to count unread messages for the returned rows; it is
//! never stored.

use axum::{http::StatusCode, Json};
use buzz_core::TenantContext;
use buzz_db::inbox_summary::{
    InboxCursor, InboxReadMarkers, InboxRowKind, InboxSummaryRow, INBOX_UNREAD_CAP,
    MAX_INBOX_SUMMARY_LIMIT,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{api_error, internal_error};
use crate::state::AppState;

type QueryError = (StatusCode, Json<Value>);

/// Default page size when the request omits `limit`.
const DEFAULT_LIMIT: u32 = 30;
/// Upper bound on supplied read markers per request.
const MAX_READ_MARKERS: usize = 4096;

pub(super) fn requested(filters: &[Value]) -> bool {
    filters.iter().any(|f| f.get("inbox_summary").is_some())
}

#[derive(Debug)]
struct Request {
    limit: u32,
    before: Option<InboxCursor>,
    markers: InboxReadMarkers,
}

fn bad(message: &str) -> QueryError {
    api_error(StatusCode::BAD_REQUEST, message)
}

fn unix_secs(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(i64::try_from(value.as_u64()?).ok()?, 0)
}

fn parse(filters: &[Value]) -> Result<Request, QueryError> {
    let [filter] = filters else {
        return Err(bad("inbox_summary requires exactly one filter"));
    };
    let object = filter
        .as_object()
        .ok_or_else(|| bad("inbox_summary filter must be an object"))?;
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "inbox_summary" | "limit" | "before" | "read"))
    {
        return Err(bad(&format!("inbox_summary does not accept `{key}`")));
    }
    if object.get("inbox_summary") != Some(&json!(1)) {
        return Err(bad("unsupported inbox_summary version"));
    }
    let limit = match object.get("limit") {
        None => DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=MAX_INBOX_SUMMARY_LIMIT).contains(n))
            .ok_or_else(|| bad("inbox_summary limit must be 1..=100"))?,
    };
    let before = match object.get("before") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let at = value
                .get("at_us")
                .and_then(Value::as_i64)
                .and_then(DateTime::from_timestamp_micros);
            let key = value.get("key").and_then(Value::as_str);
            match (at, key) {
                (Some(at), Some(key)) if key.len() <= 64 => Some(InboxCursor {
                    at,
                    key: key.to_owned(),
                }),
                _ => return Err(bad("inbox_summary before requires at_us and key")),
            }
        }
    };
    let mut markers = InboxReadMarkers::default();
    if let Some(read) = object.get("read") {
        let read = read
            .as_object()
            .ok_or_else(|| bad("inbox_summary read must be an object"))?;
        if read.len() > MAX_READ_MARKERS {
            return Err(bad("inbox_summary read has too many markers"));
        }
        for (context, value) in read {
            let at = unix_secs(value)
                .ok_or_else(|| bad("inbox_summary read values must be unix seconds"))?;
            // Other context schemes (msg:, client-private ids) are ignored.
            if let Some(root) = context.strip_prefix("thread:") {
                if let Ok(root) = hex::decode(root) {
                    if root.len() == 32 {
                        markers.threads.push((root, at));
                    }
                }
            } else if let Ok(channel) = Uuid::parse_str(context) {
                markers.channels.push((channel, at));
            }
        }
    }
    Ok(Request {
        limit,
        before,
        markers,
    })
}

/// Drop an event the reader may not see (gated or author-only kinds).
fn visible_event(value: Option<Value>, reader: &[u8]) -> Option<Value> {
    let value = value?;
    let event: nostr::Event = serde_json::from_value(value.clone()).ok()?;
    crate::handlers::req::event_visible_to_reader(&event, reader).then_some(value)
}

fn row_json(row: InboxSummaryRow, reader: &[u8]) -> Value {
    let unread = row.unread.map(|n| n.min(INBOX_UNREAD_CAP));
    json!({
        "type": match row.kind {
            InboxRowKind::Room => "room",
            InboxRowKind::Thread => "thread",
        },
        "channel_id": row.channel_id,
        "channel_type": row.channel_type,
        "root_id": row.root_id.map(hex::encode),
        "at": row.at.timestamp(),
        "at_us": row.at.timestamp_micros(),
        "key": row.key,
        "reply_count": row.reply_count,
        "unread": unread,
        "unread_overflow": row.unread.is_some_and(|n| n > INBOX_UNREAD_CAP),
        "latest": visible_event(row.latest, reader),
        "root": visible_event(row.root, reader),
    })
}

/// Serve one inbox page for the authenticated reader.
pub(super) async fn query(
    state: &AppState,
    tenant: &TenantContext,
    reader: &nostr::PublicKey,
    filters: &[Value],
) -> Result<Json<Value>, QueryError> {
    let request = parse(filters)?;
    let reader_bytes = reader.to_bytes();
    let accessible = state
        .get_accessible_channel_ids_cached(tenant.community(), &reader_bytes)
        .await
        .map_err(|e| internal_error(&format!("channel access lookup: {e}")))?;
    let page = state
        .db
        .inbox_summary(
            tenant.community(),
            reader,
            request.limit,
            request.before.as_ref(),
            &request.markers,
        )
        .await
        .map_err(|e| internal_error(&format!("inbox summary: {e}")))?;
    let next = page
        .rows
        .last()
        .filter(|_| page.has_more)
        .map(|row| json!({ "at_us": row.at.timestamp_micros(), "key": row.key }));
    let rows: Vec<Value> = page
        .rows
        .into_iter()
        // Membership is the source; access is re-checked as defense in depth.
        .filter(|row| accessible.contains(&row.channel_id))
        .map(|row| row_json(row, &reader_bytes))
        .collect();
    Ok(Json(json!({
        "inbox_summary": 1,
        "rows": rows,
        "next": next,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_markers() {
        let channel = "11111111-1111-4111-8111-111111111111";
        let root = "ab".repeat(32);
        let request = parse(&[json!({
            "inbox_summary": 1,
            "read": { channel: 100, format!("thread:{root}"): 200, "msg:zz": 5 }
        })])
        .expect("valid request");
        assert_eq!(request.limit, DEFAULT_LIMIT);
        assert!(request.before.is_none());
        assert_eq!(request.markers.channels.len(), 1);
        assert_eq!(request.markers.threads.len(), 1);
        assert_eq!(request.markers.threads[0].0, vec![0xab; 32]);
    }

    #[test]
    fn rejects_malformed_requests() {
        assert!(parse(&[]).is_err());
        assert!(parse(&[json!({"inbox_summary": 1}), json!({"inbox_summary": 1})]).is_err());
        assert!(parse(&[json!({"inbox_summary": 2})]).is_err());
        assert!(parse(&[json!({"inbox_summary": 1, "limit": 0})]).is_err());
        assert!(parse(&[json!({"inbox_summary": 1, "limit": 101})]).is_err());
        assert!(parse(&[json!({"inbox_summary": 1, "kinds": [9]})]).is_err());
        assert!(parse(&[json!({"inbox_summary": 1, "before": {"key": "x"}})]).is_err());
        assert!(parse(&[json!({"inbox_summary": 1, "read": {"x": -1}})]).is_err());
    }

    #[test]
    fn parses_cursor() {
        let request = parse(&[json!({
            "inbox_summary": 1, "limit": 30,
            "before": {"at_us": 1_760_000_000_123_456i64, "key": "abc"}
        })])
        .expect("valid cursor");
        let before = request.before.expect("cursor");
        assert_eq!(before.at.timestamp_micros(), 1_760_000_000_123_456);
        assert_eq!(before.key, "abc");
    }
}

#[cfg(test)]
mod postgres_tests {
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use buzz_db::channel::{ChannelType, ChannelVisibility};
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn query_route_serves_member_rooms_with_unread_counts() {
        let state = super::super::postgres_tests::bridge_handler_test_state()
            .await
            .expect("isolated Postgres/Redis fixture");
        let host = format!("inbox-{}.local", uuid::Uuid::new_v4().simple());
        let community = state
            .db
            .ensure_configured_community(&host)
            .await
            .unwrap()
            .id;
        let reader = Keys::generate();
        let other = Keys::generate();
        let mine = state
            .db
            .create_channel(
                community,
                "mine",
                ChannelType::Stream,
                ChannelVisibility::Private,
                None,
                &reader.public_key().to_bytes(),
                None,
            )
            .await
            .unwrap()
            .id;
        let foreign = state
            .db
            .create_channel(
                community,
                "foreign",
                ChannelType::Stream,
                ChannelVisibility::Private,
                None,
                &other.public_key().to_bytes(),
                None,
            )
            .await
            .unwrap()
            .id;
        for (room, content, ts) in [(mine, "old", 100), (mine, "new", 200), (foreign, "x", 300)] {
            let event = EventBuilder::new(Kind::Custom(9), content)
                .tags([Tag::parse(["h", &room.to_string()]).unwrap()])
                .custom_created_at(Timestamp::from(ts))
                .sign_with_keys(&other)
                .unwrap();
            state
                .db
                .insert_event(community, &event, Some(room))
                .await
                .unwrap();
        }

        let body = json!([{
            "inbox_summary": 1,
            "limit": 30,
            "read": { mine.to_string(): 150 }
        }]);
        let response = crate::router::build_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/query")
                    .header("host", &host)
                    .header("x-pubkey", reader.public_key().to_hex())
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status, StatusCode::OK, "{value}");
        let rows = value["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "only the reader's own room: {value}");
        assert_eq!(rows[0]["type"], "room");
        assert_eq!(rows[0]["channel_id"], mine.to_string());
        assert_eq!(rows[0]["latest"]["content"], "new");
        assert_eq!(rows[0]["unread"], 1);
        assert_eq!(rows[0]["unread_overflow"], false);
        assert!(value["next"].is_null());
    }
}
