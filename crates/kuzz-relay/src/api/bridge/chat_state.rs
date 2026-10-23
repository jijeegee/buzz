//! Chats S2 `/query` envelopes over the server-side chat state. The NIP-98
//! signer is the reader; every request reads or changes only that reader's
//! own state in the host's community.
//!
//! Exactly one filter object per request, carrying one of:
//!
//! - `{"chat_list": 1, "limit": 100, "before": {"at_us": .., "key": ".."},
//!   "pinned_threads": ["<root-hex>", ..]}` → `{"chat_list": 1, "rows": [..],
//!   "next": cursor | null}`. Pass the same `pinned_threads` on every page.
//! - `{"mark_read": 1, "items": [{"channel": "<uuid>", "seq": 12},
//!   {"thread": "<root-hex>", "event": "<event-hex>"}, ..], "unread_mark": false}`
//!   → `{"mark_read": 1, "items": [..]}`. Each item names a `channel` or a
//!   `thread` and optionally one position: a `seq` from `chat_list`, the id
//!   of the newest `event` the reader has seen, or `at`, whole Unix seconds
//!   the reader has read up to (matched against message `created_at`, the
//!   signer's time; at most 300 s ahead). Without a position only the unread mark
//!   changes. All items apply or none do.
//! - `{"thread_follow": 1, "root": "<root-hex>", "state": "followed" |
//!   "unfollowed"}` → `{"thread_follow": 1, "root": .., "channel_id": ..,
//!   "state": ..}`.
//! - `{"chat_import": 1, "reads": [..], "follows": [..]}`: a new app's
//!   first-run upload of device-local state; see the `import` module.
//! - `{"thread_search": 1, "q": "..", "limit": 20}` → `{"thread_search": 1,
//!   "rows": [..]}`: threads in the reader's rooms by name, closed and quiet
//!   ones included; see the `thread_search` module.
//!
//! After a change, the reader's own devices get a kind:24210 chat-state ping.
//!
//! Off switch: while `KUZZ_CHAT_STATE_WRITES` is off the server keeps no chat
//! state, so all of them return `404 {"error": "chat_state_disabled"}` before
//! parsing or touching the database. Clients treat that as "use today's
//! device-local behavior"; nothing else on `/query` changes.

use std::sync::Arc;

use axum::{http::StatusCode, Json};
use chrono::{DateTime, Duration, Utc};
use kuzz_core::TenantContext;
use kuzz_db::chat_list::{
    ChatListCursor, ChatListQuery, ChatListRow, ChatRowKind, CHAT_UNREAD_CAP, MAX_CHAT_LIST_LIMIT,
    MAX_CHAT_LIST_PINNED,
};
use kuzz_db::chat_user_state::{
    ChatStateDenied, FollowState, MarkReadItem, MarkReadResult, MarkReadTarget, ReadPosition,
    MAX_MARK_READ_ITEMS,
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::{api_error, db_read_error, internal_error};
use crate::state::AppState;

mod import;
mod thread_search;

type QueryError = (StatusCode, Json<Value>);

/// Error body while the chat-state switch is off.
pub(super) const DISABLED: &str = "chat_state_disabled";
/// Default `chat_list` page size.
const DEFAULT_LIMIT: u32 = 100;
/// Threads with no reply for this long (and nothing unread) are quiet.
const QUIET_DAYS: i64 = 7;

/// Which chat-state envelope a request carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Envelope {
    ChatList,
    MarkRead,
    ThreadFollow,
    ChatImport,
    ThreadSearch,
}

impl Envelope {
    const ALL: [Self; 5] = [
        Self::ChatList,
        Self::MarkRead,
        Self::ThreadFollow,
        Self::ChatImport,
        Self::ThreadSearch,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::ChatList => "chat_list",
            Self::MarkRead => "mark_read",
            Self::ThreadFollow => "thread_follow",
            Self::ChatImport => "chat_import",
            Self::ThreadSearch => "thread_search",
        }
    }
}

/// The chat-state envelope named by any filter, if one is.
pub(super) fn requested(filters: &[Value]) -> Option<Envelope> {
    Envelope::ALL
        .into_iter()
        .find(|envelope| filters.iter().any(|f| f.get(envelope.key()).is_some()))
}

fn bad(message: &str) -> QueryError {
    api_error(StatusCode::BAD_REQUEST, message)
}

/// The single filter object, version 1, with only `allowed` keys.
fn envelope_object<'a>(
    filters: &'a [Value],
    envelope: Envelope,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, QueryError> {
    let name = envelope.key();
    let [filter] = filters else {
        return Err(bad(&format!("{name} requires exactly one filter")));
    };
    let object = filter
        .as_object()
        .ok_or_else(|| bad(&format!("{name} filter must be an object")))?;
    if let Some(key) = object
        .keys()
        .find(|key| key.as_str() != name && !allowed.contains(&key.as_str()))
    {
        return Err(bad(&format!("{name} does not accept `{key}`")));
    }
    if object.get(name) != Some(&json!(1)) {
        return Err(bad(&format!("unsupported {name} version")));
    }
    Ok(object)
}

fn event_id(value: &Value) -> Option<Vec<u8>> {
    let text = value.as_str()?;
    let bytes = hex::decode(text).ok()?;
    (bytes.len() == 32).then_some(bytes)
}

/// The optional `as_agent` pubkey: list that agent's chats for its owner.
fn parse_as_agent(filters: &[Value]) -> Result<Option<nostr::PublicKey>, QueryError> {
    let object = envelope_object(
        filters,
        Envelope::ChatList,
        &["limit", "before", "pinned_threads", "as_agent"],
    )?;
    match object.get("as_agent") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_str()
            .and_then(|hex| nostr::PublicKey::from_hex(hex).ok())
            .map(Some)
            .ok_or_else(|| bad("chat_list as_agent must be a 64-hex pubkey")),
    }
}

fn parse_chat_list(filters: &[Value], now: DateTime<Utc>) -> Result<ChatListQuery, QueryError> {
    let object = envelope_object(
        filters,
        Envelope::ChatList,
        &["limit", "before", "pinned_threads", "as_agent"],
    )?;
    let limit = match object.get("limit") {
        None => DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=MAX_CHAT_LIST_LIMIT).contains(n))
            .ok_or_else(|| {
                bad(&format!(
                    "chat_list limit must be 1..={MAX_CHAT_LIST_LIMIT}"
                ))
            })?,
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
                (Some(at), Some(key)) if key.len() <= 64 => Some(ChatListCursor {
                    at,
                    key: key.to_owned(),
                }),
                _ => return Err(bad("chat_list before requires at_us and key")),
            }
        }
    };
    let pinned_threads = match object.get("pinned_threads") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(ids)) if ids.len() <= MAX_CHAT_LIST_PINNED => ids
            .iter()
            .map(event_id)
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| bad("chat_list pinned_threads must be 64-hex event ids"))?,
        Some(_) => {
            return Err(bad(&format!(
                "chat_list pinned_threads must be an array of at most {MAX_CHAT_LIST_PINNED} ids"
            )))
        }
    };
    Ok(ChatListQuery {
        limit,
        before,
        pinned_threads,
        quiet_before: now - Duration::days(QUIET_DAYS),
    })
}

fn parse_mark_read(filters: &[Value]) -> Result<(Vec<MarkReadItem>, bool), QueryError> {
    let object = envelope_object(filters, Envelope::MarkRead, &["items", "unread_mark"])?;
    let unread_mark = match object.get("unread_mark") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| bad("mark_read unread_mark must be a boolean"))?,
    };
    let items = object
        .get("items")
        .and_then(Value::as_array)
        .filter(|items| (1..=MAX_MARK_READ_ITEMS).contains(&items.len()))
        .ok_or_else(|| {
            bad(&format!(
                "mark_read items must be an array of 1..={MAX_MARK_READ_ITEMS} items"
            ))
        })?;
    let items = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            parse_mark_read_item(item)
                .map_err(|message| bad(&format!("mark_read item {index}: {message}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((items, unread_mark))
}

fn parse_mark_read_item(item: &Value) -> Result<MarkReadItem, &'static str> {
    let object = item.as_object().ok_or("must be an object")?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "channel" | "thread" | "seq" | "event" | "at"))
    {
        return Err("accepts only channel, thread, seq, event, at");
    }
    let target = read_target(object)?;
    let position = match (object.get("seq"), object.get("event"), object.get("at")) {
        (None, None, None) => None,
        (Some(seq), None, None) => Some(ReadPosition::Seq(
            seq.as_u64()
                .and_then(|n| i64::try_from(n).ok())
                .ok_or("seq must be a non-negative integer")?,
        )),
        (None, Some(event), None) => Some(ReadPosition::Event(
            event_id(event).ok_or("event must be a 64-hex event id")?,
        )),
        (None, None, Some(at)) => Some(ReadPosition::At(unix_seconds(at)?)),
        _ => return Err("takes one of seq, event, at"),
    };
    Ok(MarkReadItem { target, position })
}

/// The item's `channel` (UUID) or `thread` (root id), exactly one.
fn read_target(object: &Map<String, Value>) -> Result<MarkReadTarget, &'static str> {
    Ok(match (object.get("channel"), object.get("thread")) {
        (Some(channel), None) => MarkReadTarget::Channel(
            channel
                .as_str()
                .and_then(|text| Uuid::parse_str(text).ok())
                .ok_or("channel must be a UUID")?,
        ),
        (None, Some(thread)) => {
            MarkReadTarget::Thread(event_id(thread).ok_or("thread must be a 64-hex event id")?)
        }
        _ => return Err("needs exactly one of channel or thread"),
    })
}

/// How far past the relay's clock a read time may be (clock skew).
const MAX_AT_AHEAD_SECS: i64 = 300;

/// A non-negative whole number of Unix seconds, at most
/// [`MAX_AT_AHEAD_SECS`] past now. The bound also catches milliseconds sent
/// by mistake.
fn unix_seconds(value: &Value) -> Result<DateTime<Utc>, &'static str> {
    let secs = value
        .as_u64()
        .and_then(|secs| i64::try_from(secs).ok())
        .ok_or("at must be non-negative whole Unix seconds")?;
    if secs > Utc::now().timestamp() + MAX_AT_AHEAD_SECS {
        return Err("at is in the future (must be Unix seconds, not milliseconds)");
    }
    DateTime::from_timestamp(secs, 0).ok_or("at is out of range")
}

fn parse_thread_follow(filters: &[Value]) -> Result<(Vec<u8>, FollowState), QueryError> {
    let object = envelope_object(filters, Envelope::ThreadFollow, &["root", "state"])?;
    let root = object
        .get("root")
        .and_then(event_id)
        .ok_or_else(|| bad("thread_follow root must be a 64-hex event id"))?;
    let state = match object.get("state").and_then(Value::as_str) {
        Some("followed") => FollowState::Followed,
        Some("unfollowed") => FollowState::Unfollowed,
        _ => return Err(bad("thread_follow state must be followed or unfollowed")),
    };
    Ok((root, state))
}

fn denied(envelope: Envelope, denial: ChatStateDenied) -> QueryError {
    let name = envelope.key();
    match denial {
        ChatStateDenied::NotMember { index } if envelope == Envelope::MarkRead => api_error(
            StatusCode::FORBIDDEN,
            &format!("{name} item {index}: not a member of that room"),
        ),
        ChatStateDenied::NotMember { .. } => api_error(
            StatusCode::FORBIDDEN,
            &format!("{name}: not a member of that room"),
        ),
        ChatStateDenied::UnknownEvent { index } => api_error(
            StatusCode::NOT_FOUND,
            &format!("{name} item {index}: event is not in that room or thread"),
        ),
        ChatStateDenied::NotThreadRoot => {
            bad(&format!("{name}: root is a reply, not a thread root"))
        }
    }
}

/// Drop an event the reader may not see (gated or author-only kinds).
fn visible_event(value: Option<Value>, reader: &[u8]) -> Option<Value> {
    let value = value?;
    let event: nostr::Event = serde_json::from_value(value.clone()).ok()?;
    crate::handlers::req::event_visible_to_reader(&event, reader).then_some(value)
}

fn row_json(row: ChatListRow, reader: &[u8]) -> Value {
    json!({
        "type": match row.kind {
            ChatRowKind::Room => "room",
            ChatRowKind::Thread => "thread",
        },
        "key": row.key,
        "at": row.at.timestamp(),
        "at_us": row.at.timestamp_micros(),
        "channel_id": row.channel_id,
        "channel_type": row.channel_type,
        "channel_name": row.channel_name,
        "archived": row.archived,
        "group_chat": row.group_chat,
        "member_count": row.member_count,
        "dm_members": row.dm_members.iter().map(hex::encode).collect::<Vec<_>>(),
        "root_id": row.root_id.map(hex::encode),
        "thread_name": row.thread_name,
        "follow_state": row.follow_state,
        "closed": row.closed,
        "last_seq": row.last_seq,
        "read_seq": row.read_seq,
        "unread": row.unread.min(CHAT_UNREAD_CAP),
        "unread_overflow": row.unread > CHAT_UNREAD_CAP,
        "unread_mentions": row.unread_mentions.min(CHAT_UNREAD_CAP),
        "unread_mentions_overflow": row.unread_mentions > CHAT_UNREAD_CAP,
        "unread_mark": row.unread_mark,
        "latest": visible_event(row.latest, reader),
        "root": visible_event(row.root, reader),
    })
}

fn mark_result_json(result: MarkReadResult) -> Value {
    let mut value = match result.target {
        MarkReadTarget::Channel(channel) => json!({ "channel": channel }),
        MarkReadTarget::Thread(root) => json!({ "thread": hex::encode(root) }),
    };
    value["channel_id"] = json!(result.channel_id);
    value["read_seq"] = json!(result.read_seq);
    value["unread_mark"] = json!(result.unread_mark);
    value["stored"] = json!(result.stored);
    value
}

/// Tell the reader's own devices to refetch; failures are only logged.
async fn ping(state: &Arc<AppState>, tenant: &TenantContext, reader: &nostr::PublicKey) {
    if let Err(e) =
        crate::handlers::side_effects::publish_chat_state_ping(tenant, state, &reader.to_bytes())
            .await
    {
        tracing::warn!("chat-state ping failed: {e}");
    }
}

/// Serve one chat-state envelope for the authenticated reader.
pub(super) async fn query(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    reader: &nostr::PublicKey,
    filters: &[Value],
    envelope: Envelope,
) -> Result<Json<Value>, QueryError> {
    if !state.db.chat_state_writes() {
        return Err(api_error(StatusCode::NOT_FOUND, DISABLED));
    }
    match envelope {
        Envelope::ChatList => chat_list(state, tenant, reader, filters).await,
        Envelope::MarkRead => {
            let (items, unread_mark) = parse_mark_read(filters)?;
            let results = state
                .db
                .chat_mark_read(tenant.community(), reader, &items, unread_mark)
                .await
                .map_err(|e| internal_error(&format!("mark_read: {e}")))?
                .map_err(|denial| denied(envelope, denial))?;
            if results.iter().any(|result| result.stored) {
                ping(state, tenant, reader).await;
            }
            Ok(Json(json!({
                "mark_read": 1,
                "items": results.into_iter().map(mark_result_json).collect::<Vec<_>>(),
            })))
        }
        Envelope::ThreadFollow => {
            let (root, follow) = parse_thread_follow(filters)?;
            let channel_id = state
                .db
                .chat_thread_follow(tenant.community(), reader, &root, follow)
                .await
                .map_err(|e| internal_error(&format!("thread_follow: {e}")))?
                .map_err(|denial| denied(envelope, denial))?;
            ping(state, tenant, reader).await;
            Ok(Json(json!({
                "thread_follow": 1,
                "root": hex::encode(root),
                "channel_id": channel_id,
                "state": follow.as_str(),
            })))
        }
        Envelope::ChatImport => {
            let request = import::parse(filters)?;
            let outcome = state
                .db
                .chat_import(tenant.community(), reader, &request.reads, &request.follows)
                .await
                .map_err(|e| internal_error(&format!("chat_import: {e}")))?;
            if outcome.any_applied() {
                ping(state, tenant, reader).await;
            }
            Ok(Json(import::response(&request, &outcome)))
        }
        Envelope::ThreadSearch => thread_search(state, tenant, reader, filters).await,
    }
}

async fn thread_search(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    reader: &nostr::PublicKey,
    filters: &[Value],
) -> Result<Json<Value>, QueryError> {
    let request = thread_search::parse(filters)?;
    let accessible = state
        .get_accessible_channel_ids_cached(tenant.community(), &reader.to_bytes())
        .await
        .map_err(|e| internal_error(&format!("channel access lookup: {e}")))?;
    let rows = state
        .db
        .thread_search(tenant.community(), reader, &request.query, request.limit)
        .await
        .map_err(|e| db_read_error("thread_search", &e))?;
    let rows: Vec<Value> = rows
        .into_iter()
        // Membership is the source; access is re-checked as defense in depth.
        .filter(|row| accessible.contains(&row.channel_id))
        .map(thread_search::row_json)
        .collect();
    Ok(Json(json!({ "thread_search": 1, "rows": rows })))
}

async fn chat_list(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    reader: &nostr::PublicKey,
    filters: &[Value],
) -> Result<Json<Value>, QueryError> {
    let query = parse_chat_list(filters, Utc::now())?;
    if let Some(agent) = parse_as_agent(filters)? {
        return agent_chat_list(state, tenant, reader, &agent, query).await;
    }
    let reader_bytes = reader.to_bytes();
    let accessible = state
        .get_accessible_channel_ids_cached(tenant.community(), &reader_bytes)
        .await
        .map_err(|e| internal_error(&format!("channel access lookup: {e}")))?;
    let page = state
        .db
        .chat_list(tenant.community(), reader, &query)
        .await
        .map_err(|e| internal_error(&format!("chat list: {e}")))?;
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
        "chat_list": 1,
        "rows": rows,
        "next": next,
    })))
}

/// Pages of the agent's chat list read per request in the owner view.
const AGENT_LIST_MAX_PAGES: usize = 5;

/// Read-only owner view: the DMs and group chats of `agent` that `owner` is
/// not in. Only the agent's owner may ask. The agent's own read state is
/// stripped; the owner has none for these rooms.
async fn agent_chat_list(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    owner: &nostr::PublicKey,
    agent: &nostr::PublicKey,
    mut query: ChatListQuery,
) -> Result<Json<Value>, QueryError> {
    let owner_bytes = owner.to_bytes();
    let owns = state
        .db
        .is_agent_owner(tenant.community(), &agent.to_bytes(), &owner_bytes)
        .await
        .map_err(|e| internal_error(&format!("agent owner lookup: {e}")))?;
    if !owns {
        return Err(api_error(
            StatusCode::FORBIDDEN,
            "chat_list as_agent: not your agent",
        ));
    }
    query.pinned_threads.clear();
    let owner_rooms = state
        .get_accessible_channel_ids_cached(tenant.community(), &owner_bytes)
        .await
        .map_err(|e| internal_error(&format!("channel access lookup: {e}")))?;
    // Regular channels are filtered out after the query, so keep reading
    // pages until this one is full (bounded) instead of handing back a short
    // or empty page with a cursor.
    let mut rows: Vec<Value> = Vec::new();
    let mut next = None;
    for _ in 0..AGENT_LIST_MAX_PAGES {
        let page = state
            .db
            .chat_list(tenant.community(), agent, &query)
            .await
            .map_err(|e| internal_error(&format!("chat list: {e}")))?;
        let cursor = page
            .rows
            .last()
            .filter(|_| page.has_more)
            .map(|row| ChatListCursor {
                at: row.at,
                key: row.key.clone(),
            });
        rows.extend(
            page.rows
                .into_iter()
                .filter(|row| owner_view_row(row, &owner_rooms))
                .map(|row| owner_view_json(row_json(row, &owner_bytes))),
        );
        next = cursor
            .as_ref()
            .map(|c| json!({ "at_us": c.at.timestamp_micros(), "key": c.key }));
        match cursor {
            Some(cursor) if rows.len() < query.limit as usize => query.before = Some(cursor),
            _ => break,
        }
    }
    Ok(Json(json!({
        "chat_list": 1,
        "as_agent": agent.to_hex(),
        "rows": rows,
        "next": next,
    })))
}

/// A DM or group chat the owner is not already in (those are in Chats).
fn owner_view_row(row: &ChatListRow, owner_rooms: &[Uuid]) -> bool {
    (row.channel_type == "dm" || row.group_chat) && !owner_rooms.contains(&row.channel_id)
}

/// Drop the agent's read and follow state; the owner sees new activity by
/// `last_seq`.
fn owner_view_json(mut value: Value) -> Value {
    for key in ["read_seq", "unread_mark", "follow_state"] {
        value[key] = Value::Null;
    }
    for key in ["unread", "unread_mentions"] {
        value[key] = json!(0);
    }
    for key in ["unread_overflow", "unread_mentions_overflow"] {
        value[key] = json!(false);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_envelope_by_key() {
        assert_eq!(
            requested(&[json!({"chat_list": 1})]),
            Some(Envelope::ChatList)
        );
        assert_eq!(
            requested(&[json!({"mark_read": 1})]),
            Some(Envelope::MarkRead)
        );
        assert_eq!(
            requested(&[json!({"kinds": [9]}), json!({"thread_follow": 1})]),
            Some(Envelope::ThreadFollow)
        );
        assert_eq!(
            requested(&[json!({"chat_import": 1})]),
            Some(Envelope::ChatImport)
        );
        assert_eq!(
            requested(&[json!({"thread_search": 1, "q": "x"})]),
            Some(Envelope::ThreadSearch)
        );
        assert_eq!(requested(&[json!({"kinds": [9]})]), None);
    }

    #[test]
    fn parses_chat_list_defaults_cursor_and_pins() {
        let now = Utc::now();
        let query = parse_chat_list(&[json!({"chat_list": 1})], now).expect("defaults");
        assert_eq!(query.limit, DEFAULT_LIMIT);
        assert!(query.before.is_none() && query.pinned_threads.is_empty());
        assert_eq!(query.quiet_before, now - Duration::days(QUIET_DAYS));

        let root = "ab".repeat(32);
        let query = parse_chat_list(
            &[json!({
                "chat_list": 1, "limit": 5, "pinned_threads": [root],
                "before": {"at_us": 1_760_000_000_123_456i64, "key": "k"}
            })],
            now,
        )
        .expect("full request");
        assert_eq!(query.limit, 5);
        assert_eq!(query.pinned_threads, vec![vec![0xab; 32]]);
        assert_eq!(
            query.before.expect("cursor").at.timestamp_micros(),
            1_760_000_000_123_456
        );
    }

    #[test]
    fn rejects_malformed_chat_lists() {
        let now = Utc::now();
        for filters in [
            vec![],
            vec![json!({"chat_list": 1}), json!({"chat_list": 1})],
            vec![json!({"chat_list": 2})],
            vec![json!({"chat_list": 1, "limit": 0})],
            vec![json!({"chat_list": 1, "limit": 201})],
            vec![json!({"chat_list": 1, "kinds": [9]})],
            vec![json!({"chat_list": 1, "before": {"key": "x"}})],
            vec![json!({"chat_list": 1, "pinned_threads": ["zz"]})],
            vec![json!({"chat_list": 1, "pinned_threads": vec!["ab".repeat(32); 201]})],
        ] {
            assert!(parse_chat_list(&filters, now).is_err(), "{filters:?}");
        }
    }

    #[test]
    fn parses_mark_read_items() {
        let channel = "11111111-1111-4111-8111-111111111111";
        let root = "cd".repeat(32);
        let (items, unread_mark) = parse_mark_read(&[json!({
            "mark_read": 1, "unread_mark": true,
            "items": [
                {"channel": channel, "seq": 7},
                {"thread": root, "event": root},
                {"channel": channel},
                {"thread": root, "at": 1_760_000_000}
            ]
        })])
        .expect("valid");
        assert!(unread_mark);
        assert_eq!(items.len(), 4);
        assert_eq!(
            items[3].position,
            Some(ReadPosition::At(
                DateTime::from_timestamp(1_760_000_000, 0).expect("time")
            ))
        );
        assert_eq!(items[0].position, Some(ReadPosition::Seq(7)));
        assert_eq!(items[1].target, MarkReadTarget::Thread(vec![0xcd; 32]));
        assert_eq!(items[1].position, Some(ReadPosition::Event(vec![0xcd; 32])));
        assert_eq!(items[2].position, None);
    }

    #[test]
    fn rejects_malformed_mark_reads() {
        let channel = "11111111-1111-4111-8111-111111111111";
        let root = "cd".repeat(32);
        let now = Utc::now().timestamp();
        for item in [
            json!({}),
            // Milliseconds by mistake, and more than the allowed skew ahead.
            json!({"channel": channel, "at": now * 1000}),
            json!({"channel": channel, "at": now + 3600}),
            json!({"channel": channel, "thread": root}),
            json!({"channel": "nope"}),
            json!({"thread": "abcd"}),
            json!({"channel": channel, "seq": -1}),
            json!({"channel": channel, "seq": 1, "event": root}),
            json!({"channel": channel, "seq": 1, "at": 1}),
            json!({"channel": channel, "event": root, "at": 1}),
            json!({"channel": channel, "at": -1}),
            json!({"channel": channel, "at": 1.5}),
            json!({"channel": channel, "extra": 1}),
        ] {
            assert!(
                parse_mark_read(&[json!({"mark_read": 1, "items": [item.clone()]})]).is_err(),
                "{item}"
            );
        }
        // Within the skew is accepted.
        assert!(parse_mark_read(&[
            json!({"mark_read": 1, "items": [{"channel": channel, "at": now + 60}]})
        ])
        .is_ok());
        assert!(parse_mark_read(&[json!({"mark_read": 1, "items": []})]).is_err());
        let too_many = vec![json!({"channel": channel}); MAX_MARK_READ_ITEMS + 1];
        assert!(parse_mark_read(&[json!({"mark_read": 1, "items": too_many})]).is_err());
        assert!(parse_mark_read(&[
            json!({"mark_read": 1, "items": [{"channel": channel}], "unread_mark": "yes"})
        ])
        .is_err());
    }

    #[test]
    fn parses_thread_follow() {
        let root = "ef".repeat(32);
        let (parsed, state) = parse_thread_follow(&[json!({
            "thread_follow": 1, "root": root, "state": "unfollowed"
        })])
        .expect("valid");
        assert_eq!(parsed, vec![0xef; 32]);
        assert_eq!(state, FollowState::Unfollowed);
        for bad in [
            json!({"thread_follow": 1, "root": root, "state": "auto"}),
            json!({"thread_follow": 1, "root": "ef", "state": "followed"}),
            json!({"thread_follow": 1, "root": root}),
            json!({"thread_follow": 1, "root": root, "state": "followed", "x": 1}),
        ] {
            assert!(
                parse_thread_follow(std::slice::from_ref(&bad)).is_err(),
                "{bad}"
            );
        }
    }
}

#[cfg(test)]
mod postgres_tests {
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU8;

    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use kuzz_core::kind::KIND_CHAT_STATE_PING;
    use kuzz_core::CommunityId;
    use kuzz_db::channel::{ChannelType, ChannelVisibility, MemberRole};
    use nostr::{EventBuilder, Filter, Keys, Kind, Tag, Timestamp};
    use tokio::sync::{mpsc, Mutex};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use super::*;

    struct Fixture {
        state: Arc<AppState>,
        host: String,
        community: CommunityId,
    }

    impl Fixture {
        async fn new(chat_state: bool) -> Self {
            let base = super::super::postgres_tests::bridge_handler_test_state()
                .await
                .expect("isolated Postgres/Redis fixture");
            let mut state = (*base).clone();
            state.db = state.db.clone().with_chat_state_writes(chat_state);
            let host = format!("chats-{}.local", Uuid::new_v4().simple());
            let community = state
                .db
                .ensure_configured_community(&host)
                .await
                .expect("community")
                .id;
            Self {
                state: Arc::new(state),
                host,
                community,
            }
        }

        async fn send(&self, uri: &str, key: &Keys, body: &Value) -> (StatusCode, Value) {
            let response = crate::router::build_router(self.state.clone())
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("host", &self.host)
                        .header("x-pubkey", key.public_key().to_hex())
                        .body(Body::from(serde_json::to_vec(body).expect("body")))
                        .expect("request"),
                )
                .await
                .expect("router");
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .expect("body bytes");
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }

        async fn query(&self, key: &Keys, filter: Value) -> (StatusCode, Value) {
            self.send("/query", key, &json!([filter])).await
        }

        async fn room(&self, owner: &Keys, members: &[&Keys]) -> Uuid {
            let id = self
                .state
                .db
                .create_channel(
                    self.community,
                    "room",
                    ChannelType::Stream,
                    ChannelVisibility::Private,
                    None,
                    &owner.public_key().to_bytes(),
                    None,
                )
                .await
                .expect("channel")
                .id;
            for member in members {
                self.state
                    .db
                    .add_member(
                        self.community,
                        id,
                        &member.public_key().to_bytes(),
                        MemberRole::Member,
                        Some(&owner.public_key().to_bytes()),
                    )
                    .await
                    .expect("member");
            }
            id
        }

        /// `ts` orders messages within the test (100 and up); the event time
        /// sits a few minutes in the past so ingest accepts it.
        fn sign(&self, room: Uuid, key: &Keys, ts: u64, tags: Vec<Tag>) -> nostr::Event {
            let mut all = vec![Tag::parse(["h", &room.to_string()]).expect("h")];
            all.extend(tags);
            let created_at = Timestamp::now().as_secs() - 200 + (ts - 100);
            EventBuilder::new(Kind::Custom(9), format!("message at {ts}"))
                .tags(all)
                .custom_created_at(Timestamp::from(created_at))
                .sign_with_keys(key)
                .expect("sign")
        }

        /// A message through the real HTTP ingest path.
        async fn publish(&self, event: &nostr::Event, key: &Keys) {
            let (status, body) = self
                .send("/events", key, &serde_json::to_value(event).expect("event"))
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["accepted"], true, "{body}");
        }

        /// A live connection authenticated as `key`, subscribed to its pings.
        fn device(&self, key: &Keys) -> mpsc::Receiver<axum::extract::ws::Message> {
            let conn_id = Uuid::new_v4();
            let (tx, rx) = mpsc::channel(16);
            let (ctrl_tx, _ctrl_rx) = mpsc::channel(4);
            self.state.conn_manager.register(
                conn_id,
                tx,
                ctrl_tx,
                mpsc::channel(1).0,
                None,
                CancellationToken::new(),
                self.community,
                Arc::new(AtomicU8::new(0)),
                Arc::new(Mutex::new(HashMap::new())),
                3,
                crate::state::CommunityConnectionControl::new(CancellationToken::new()),
            );
            self.state
                .conn_manager
                .set_authenticated_pubkey(conn_id, key.public_key().to_bytes().to_vec());
            self.state.sub_registry.register_scoped(
                self.community,
                conn_id,
                "pings".into(),
                vec![Filter::new()
                    .kind(Kind::Custom(KIND_CHAT_STATE_PING as u16))
                    .pubkey(key.public_key())],
                None,
            );
            rx
        }
    }

    fn pings(rx: &mut mpsc::Receiver<axum::extract::ws::Message>) -> usize {
        let mut count = 0;
        while let Ok(message) = rx.try_recv() {
            let axum::extract::ws::Message::Text(text) = message else {
                panic!("expected a text frame");
            };
            let frame: Value = serde_json::from_str(&text).expect("frame");
            assert_eq!(frame[0], "EVENT");
            assert_eq!(frame[2]["kind"], KIND_CHAT_STATE_PING);
            count += 1;
        }
        count
    }

    fn row<'a>(body: &'a Value, key: &str) -> Option<&'a Value> {
        body["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["key"] == key)
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn switch_off_refuses_every_envelope() {
        let f = Fixture::new(false).await;
        let reader = Keys::generate();
        let room = f.room(&reader, &[]).await;
        let mut device = f.device(&reader);
        for filter in [
            json!({"chat_list": 1}),
            json!({"mark_read": 1, "items": [{"channel": room}]}),
            json!({"thread_follow": 1, "root": "ab".repeat(32), "state": "followed"}),
            json!({"chat_import": 1, "reads": [{"channel": room, "at": 1}]}),
            json!({"thread_search": 1, "q": "plan"}),
            // Even malformed requests: the switch is checked first.
            json!({"mark_read": 1}),
        ] {
            let (status, body) = f.query(&reader, filter.clone()).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{filter}: {body}");
            assert_eq!(body["error"], DISABLED);
        }
        assert_eq!(pings(&mut device), 0);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn list_read_and_follow_round_trip_pinging_only_own_devices() {
        let f = Fixture::new(true).await;
        let (reader, other) = (Keys::generate(), Keys::generate());
        let room = f.room(&other, &[&reader]).await;
        let foreign = f.room(&other, &[]).await;
        let (mut phone, mut laptop, mut others) =
            (f.device(&reader), f.device(&reader), f.device(&other));

        let root = f.sign(room, &reader, 100, Vec::new());
        f.publish(&root, &reader).await;
        let reply = f.sign(
            room,
            &other,
            101,
            vec![
                Tag::parse(["e", &root.id.to_hex(), "", "reply"]).expect("e"),
                Tag::parse(["p", &reader.public_key().to_hex()]).expect("p"),
            ],
        );
        f.publish(&reply, &other).await;
        let mut last = root.clone();
        for ts in 102..106 {
            last = f.sign(room, &other, ts, Vec::new());
            f.publish(&last, &other).await;
        }
        f.publish(&f.sign(foreign, &other, 107, Vec::new()), &other)
            .await;

        let (status, body) = f.query(&reader, json!({"chat_list": 1})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["chat_list"], 1);
        assert!(body["next"].is_null());
        assert_eq!(body["rows"].as_array().expect("rows").len(), 2, "{body}");
        let room_row = row(&body, &room.to_string()).expect("room row");
        assert_eq!(room_row["type"], "room");
        assert_eq!(room_row["unread"], 4);
        assert_eq!(room_row["unread_overflow"], false);
        assert_eq!(room_row["latest"]["id"], last.id.to_hex());
        let thread_row = row(&body, &root.id.to_hex()).expect("thread row");
        assert_eq!(thread_row["type"], "thread");
        assert_eq!(thread_row["unread"], 1);
        assert_eq!(thread_row["unread_mentions"], 1);
        assert_eq!(thread_row["root"]["id"], root.id.to_hex());
        assert_eq!(thread_row["follow_state"], "auto");
        assert_eq!(pings(&mut phone), 0, "reading the list changes nothing");

        // Paging hands back a cursor.
        let (_, first) = f.query(&reader, json!({"chat_list": 1, "limit": 1})).await;
        assert_eq!(first["rows"].as_array().expect("rows").len(), 1);
        let (_, second) = f
            .query(
                &reader,
                json!({"chat_list": 1, "limit": 1, "before": first["next"]}),
            )
            .await;
        assert_ne!(first["rows"][0]["key"], second["rows"][0]["key"]);

        let (status, body) = f
            .query(
                &reader,
                json!({"mark_read": 1, "items": [
                    {"channel": room, "event": last.id.to_hex()},
                    {"thread": root.id.to_hex(), "seq": thread_row["last_seq"]}
                ]}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["items"][0]["read_seq"], room_row["last_seq"]);
        assert_eq!(body["items"][1]["stored"], true);
        assert_eq!((pings(&mut phone), pings(&mut laptop)), (1, 1));
        assert_eq!(pings(&mut others), 0, "never pings other people");

        let (_, body) = f.query(&reader, json!({"chat_list": 1})).await;
        assert_eq!(row(&body, &room.to_string()).expect("room")["unread"], 0);
        assert_eq!(row(&body, &root.id.to_hex()).expect("thread")["unread"], 0);

        let (status, body) = f
            .query(
                &reader,
                json!({"thread_follow": 1, "root": root.id.to_hex(), "state": "unfollowed"}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["state"], "unfollowed");
        assert_eq!(body["channel_id"], room.to_string());
        assert_eq!((pings(&mut phone), pings(&mut laptop)), (1, 1));
        let (_, body) = f.query(&reader, json!({"chat_list": 1})).await;
        assert!(row(&body, &root.id.to_hex()).is_none(), "{body}");

        // Refused and malformed requests change nothing and ping no one.
        for (filter, expected) in [
            (
                json!({"mark_read": 1, "items": [{"channel": foreign}]}),
                StatusCode::FORBIDDEN,
            ),
            (
                json!({"mark_read": 1, "items": [{"channel": room, "event": "00".repeat(32)}]}),
                StatusCode::NOT_FOUND,
            ),
            (
                json!({"thread_follow": 1, "root": reply.id.to_hex(), "state": "followed"}),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({"mark_read": 1, "items": [{"channel": room}], "extra": 1}),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let (status, body) = f.query(&reader, filter.clone()).await;
            assert_eq!(status, expected, "{filter}: {body}");
        }
        let (status, _) = f
            .send(
                "/query",
                &reader,
                &json!([{"chat_list": 1}, {"kinds": [9]}]),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            pings(&mut phone) + pings(&mut laptop) + pings(&mut others),
            0
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn closing_a_thread_pings_only_the_closer() {
        let f = Fixture::new(true).await;
        let (closer, other) = (Keys::generate(), Keys::generate());
        let room = f.room(&other, &[&closer]).await;
        let (mut closer_device, mut other_device) = (f.device(&closer), f.device(&other));
        let root = f.sign(room, &other, 100, Vec::new());
        f.publish(&root, &other).await;
        let reply = f.sign(
            room,
            &other,
            101,
            vec![Tag::parse(["e", &root.id.to_hex(), "", "reply"]).expect("e")],
        );
        f.publish(&reply, &other).await;
        assert_eq!(pings(&mut closer_device) + pings(&mut other_device), 0);

        let close = f.sign(
            room,
            &closer,
            102,
            vec![
                Tag::parse(["e", &root.id.to_hex(), "", "reply"]).expect("e"),
                Tag::parse([kuzz_db::chat_state::THREAD_CLOSED_TAG, &root.id.to_hex()])
                    .expect("close tag"),
            ],
        );
        f.publish(&close, &closer).await;
        assert_eq!(pings(&mut closer_device), 1);
        assert_eq!(pings(&mut other_device), 0);

        let (_, body) = f
            .query(
                &other,
                json!({"chat_list": 1, "pinned_threads": [root.id.to_hex()]}),
            )
            .await;
        assert_eq!(
            row(&body, &root.id.to_hex()).expect("pinned")["closed"],
            true
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn thread_search_finds_closed_threads_only_for_members() {
        let f = Fixture::new(true).await;
        let (reader, other, outsider) = (Keys::generate(), Keys::generate(), Keys::generate());
        let room = f.room(&other, &[&reader]).await;
        let root = f.sign(room, &other, 100, Vec::new());
        f.publish(&root, &other).await;
        let name = EventBuilder::new(Kind::Custom(40009), "Quarterly launch review")
            .tags([
                Tag::parse(["h", &room.to_string()]).expect("h"),
                Tag::parse(["e", &root.id.to_hex()]).expect("e"),
            ])
            .custom_created_at(Timestamp::from(Timestamp::now().as_secs() - 100))
            .sign_with_keys(&other)
            .expect("name");
        f.state
            .db
            .insert_event(f.community, &name, Some(room))
            .await
            .expect("thread name");
        let close = f.sign(
            room,
            &other,
            102,
            vec![
                Tag::parse(["e", &root.id.to_hex(), "", "reply"]).expect("e"),
                Tag::parse([kuzz_db::chat_state::THREAD_CLOSED_TAG, &root.id.to_hex()])
                    .expect("close tag"),
            ],
        );
        f.publish(&close, &other).await;

        // Closed: the Chats list leaves it out, search still finds it.
        let (_, body) = f.query(&reader, json!({"chat_list": 1})).await;
        assert!(row(&body, &root.id.to_hex()).is_none(), "{body}");
        let (status, body) = f
            .query(&reader, json!({"thread_search": 1, "q": "LAUNCH"}))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["thread_search"], 1);
        let hit = row(&body, &root.id.to_hex()).expect("closed thread found");
        assert_eq!(hit["type"], "thread");
        assert_eq!(hit["root_id"], root.id.to_hex());
        assert_eq!(hit["channel_id"], room.to_string());
        assert_eq!(hit["thread_name"], "Quarterly launch review");
        assert_eq!(hit["closed"], true);
        assert_eq!(hit["match"], 2);

        // Someone outside the private room sees nothing.
        let (status, body) = f
            .query(&outsider, json!({"thread_search": 1, "q": "launch"}))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["rows"], json!([]));

        let (status, _) = f
            .query(&reader, json!({"thread_search": 1, "q": " "}))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn real_nip98_auth_binds_the_reader_and_rejects_replay() {
        use base64::Engine;
        use sha2::{Digest, Sha256};

        let f = Fixture::new(true).await;
        let mut state = (*f.state).clone();
        Arc::make_mut(&mut state.config).require_auth_token = true;
        state.nip98_replay = Arc::new(kuzz_pubsub::RedisNip98ReplayGuard::new(
            state.redis_pool.clone(),
        ));
        let state = Arc::new(state);
        let reader = Keys::generate();
        let room = f.room(&reader, &[]).await;
        let body = serde_json::to_vec(&json!([{"chat_list": 1}])).expect("body");
        let proof = |signed_host: &str, payload: &[u8]| {
            let auth = EventBuilder::new(Kind::Custom(27235), "")
                .tags([
                    Tag::parse(["u", &format!("https://{signed_host}/query")]).expect("u"),
                    Tag::parse(["method", "POST"]).expect("method"),
                    Tag::parse(["payload", &hex::encode(Sha256::digest(payload))])
                        .expect("payload"),
                    Tag::parse(["nonce", &Uuid::new_v4().to_string()]).expect("nonce"),
                ])
                .sign_with_keys(&reader)
                .expect("sign");
            format!(
                "Nostr {}",
                base64::engine::general_purpose::STANDARD
                    .encode(serde_json::to_vec(&auth).expect("auth json"))
            )
        };
        let post = |headers: Vec<(&'static str, String)>| {
            let router = crate::router::build_router(state.clone());
            let mut request = Request::builder()
                .method("POST")
                .uri("/query")
                .header("host", &f.host);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let request = request.body(Body::from(body.clone())).expect("request");
            async move {
                let response = router.oneshot(request).await.expect("router");
                let status = response.status();
                let bytes = to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .expect("bytes");
                (
                    status,
                    serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                )
            }
        };

        // The dev header is not accepted once real auth is required.
        let (status, _) = post(vec![("x-pubkey", reader.public_key().to_hex())]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = post(vec![("authorization", proof("wrong.invalid", &body))]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = post(vec![("authorization", proof(&f.host, b"[]"))]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let auth = proof(&f.host, &body);
        let (status, value) = post(vec![("authorization", auth.clone())]).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        assert_eq!(value["rows"][0]["channel_id"], room.to_string(), "{value}");
        let (status, _) = post(vec![("authorization", auth)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "replayed proof");
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn chat_import_applies_once_and_skips_per_item() {
        let f = Fixture::new(true).await;
        let (reader, other) = (Keys::generate(), Keys::generate());
        let room = f.room(&other, &[&reader]).await;
        let foreign = f.room(&other, &[]).await;
        let (mut phone, mut others) = (f.device(&reader), f.device(&other));
        let first = f.sign(room, &other, 100, Vec::new());
        f.publish(&first, &other).await;
        for ts in 101..104 {
            f.publish(&f.sign(room, &other, ts, Vec::new()), &other)
                .await;
        }
        f.state
            .db
            .backfill_chat_state_channel(
                f.community,
                room,
                &kuzz_db::chat_backfill::ChatBackfillOptions::from_now(false),
            )
            .await
            .expect("backfill");
        let (_, list) = f.query(&reader, json!({"chat_list": 1})).await;
        assert_eq!(row(&list, &room.to_string()).expect("room")["unread"], 0);

        // Read up to the first message: the three after it are unread again.
        let request = json!({
            "chat_import": 1,
            "reads": [
                {"channel": room, "at": first.created_at.as_secs()},
                {"channel": foreign, "at": first.created_at.as_secs()},
                {"thread": "ab".repeat(32), "at": 1}
            ],
            "follows": [{"root": first.id.to_hex(), "state": "followed"}]
        });
        let (status, body) = f.query(&reader, request.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["chat_import"], 1);
        assert_eq!(body["reads"][0]["applied"], true, "{body}");
        assert_eq!(body["reads"][0]["channel_id"], room.to_string());
        assert_eq!(body["reads"][1]["reason"], "not_member");
        assert_eq!(body["reads"][2]["reason"], "unknown_thread");
        assert_eq!(body["follows"][0]["applied"], true, "{body}");
        assert_eq!(body["follows"][0]["state"], "followed");
        assert_eq!(pings(&mut phone), 1);
        assert_eq!(pings(&mut others), 0);
        let (_, list) = f.query(&reader, json!({"chat_list": 1})).await;
        assert_eq!(row(&list, &room.to_string()).expect("room")["unread"], 3);

        // The upload applies once: a repeat changes nothing and pings no one.
        let (status, body) = f.query(&reader, request).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["reads"][0]["reason"], "already_set");
        assert_eq!(body["follows"][0]["reason"], "already_set");
        assert_eq!(pings(&mut phone), 0);

        let (status, _) = f
            .query(&reader, json!({"chat_import": 1, "reads": []}))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// Signed NIP-29 remove-user (9001) of `target` from `room`.
    fn remove_user(room: Uuid, actor: &Keys, target: &Keys) -> nostr::Event {
        EventBuilder::new(Kind::Custom(9001), "")
            .tags(vec![
                Tag::parse(["h", &room.to_string()]).expect("h"),
                Tag::parse(["p", &target.public_key().to_hex()]).expect("p"),
            ])
            .sign_with_keys(actor)
            .expect("sign")
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn owner_reads_and_leaves_agent_chats_read_only() {
        let f = Fixture::new(true).await;
        let (owner, agent, person, stranger) = (
            Keys::generate(),
            Keys::generate(),
            Keys::generate(),
            Keys::generate(),
        );
        for key in [&owner, &agent, &person, &stranger] {
            f.state
                .db
                .ensure_user(f.community, &key.public_key().to_bytes())
                .await
                .expect("user");
        }
        f.state
            .db
            .set_agent_owner(
                f.community,
                &agent.public_key().to_bytes(),
                &owner.public_key().to_bytes(),
            )
            .await
            .expect("agent owner");

        // Group chat of the agent and a person (owner not in it), a group chat
        // the owner is also in, and a regular channel.
        let watched = f.room(&person, &[&agent]).await;
        let shared = f.room(&person, &[&agent, &owner]).await;
        let channel = f.room(&person, &[&agent]).await;
        for room in [watched, shared] {
            sqlx::query("UPDATE channels SET group_chat = true WHERE id = $1")
                .bind(room)
                .execute(f.state.db.pool())
                .await
                .expect("group chat");
        }
        let said = f.sign(watched, &agent, 100, Vec::new());
        f.publish(&said, &agent).await;
        f.publish(&f.sign(watched, &person, 101, Vec::new()), &person)
            .await;
        let hidden = f.sign(channel, &agent, 102, Vec::new());
        f.publish(&hidden, &agent).await;

        // The list holds only the watched chat, without the agent's read state.
        let as_agent = json!({"chat_list": 1, "as_agent": agent.public_key().to_hex()});
        let (status, body) = f.query(&owner, as_agent.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["as_agent"], agent.public_key().to_hex());
        let rows = body["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "{body}");
        assert_eq!(rows[0]["channel_id"], watched.to_string());
        assert_eq!(rows[0]["unread"], 0);
        assert!(rows[0]["read_seq"].is_null());
        let (status, _) = f.query(&stranger, as_agent.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = f.query(&agent, as_agent.clone()).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "an agent is not its own owner"
        );

        // Explicitly requested history: the owner reads the watched chat only.
        let history = |room: Uuid| json!({"kinds": [9], "#h": [room]});
        let (_, body) = f.query(&owner, history(watched)).await;
        assert!(body.to_string().contains(&said.id.to_hex()), "{body}");
        for (key, room, event) in [(&stranger, watched, &said), (&owner, channel, &hidden)] {
            let (_, body) = f.query(key, history(room)).await;
            assert!(!body.to_string().contains(&event.id.to_hex()), "{body}");
        }
        // Nothing leaks into the owner's own unscoped reads.
        let (_, body) = f.query(&owner, json!({"kinds": [9]})).await;
        assert!(!body.to_string().contains(&said.id.to_hex()), "{body}");
        let (_, body) = f.query(&owner, json!({"chat_list": 1})).await;
        assert!(row(&body, &watched.to_string()).is_none(), "{body}");
        // The owner cannot write there.
        let (_, body) = f
            .send(
                "/events",
                &owner,
                &serde_json::to_value(f.sign(watched, &owner, 103, Vec::new())).expect("event"),
            )
            .await;
        assert_ne!(body["accepted"], true, "{body}");

        // The owner may remove the agent from the chat, not from a channel,
        // and nobody else's agent.
        let (_, body) = f
            .send(
                "/events",
                &owner,
                &serde_json::to_value(remove_user(channel, &owner, &agent)).expect("event"),
            )
            .await;
        assert_ne!(body["accepted"], true, "{body}");
        let (_, body) = f
            .send(
                "/events",
                &stranger,
                &serde_json::to_value(remove_user(watched, &stranger, &agent)).expect("event"),
            )
            .await;
        assert_ne!(body["accepted"], true, "{body}");
        f.publish(&remove_user(watched, &owner, &agent), &owner)
            .await;

        // With the agent gone the owner loses the chat too.
        let (_, body) = f.query(&owner, as_agent).await;
        assert!(body["rows"].as_array().expect("rows").is_empty(), "{body}");
        let (_, body) = f.query(&owner, history(watched)).await;
        assert!(!body.to_string().contains(&said.id.to_hex()), "{body}");
    }
}
