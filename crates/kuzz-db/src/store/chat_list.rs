//! Chats list for one reader (Chats stage S2), read from the server-side chat
//! state that the S1 write path keeps.
//!
//! One statement returns a page of the reader's rooms and live threads mixed
//! newest first. Unread counts are computed for the page only, from the
//! channel sequence index, and stop at [`CHAT_UNREAD_CAP`] + 1.
//!
//! - Rooms: current memberships of live channels, minus hidden DMs. Archived
//!   rooms stay only while they have something unread or an unread mark.
//! - Threads: rows in `thread_follows` that are not `unfollowed`, in a room the
//!   reader is still in (not hidden), not closed, and either unread, marked
//!   unread, or active within the quiet window. In archived rooms only unread
//!   or marked threads stay.
//! - Pinned threads named by the caller are added whatever their follow
//!   state, quietness, or close state; `closed` tells the caller which are
//!   closed.
//!
//! Read positions and follows are private per-user state: never log them.

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;
use uuid::Uuid;

use crate::chat_state::{THREAD_CLOSED_TAG, THREAD_REOPENED_TAG};
use crate::inbox_summary::MESSAGE_KINDS;
use crate::{observability, Db, DbError, Result};

/// Largest page a caller may request.
pub const MAX_CHAT_LIST_LIMIT: u32 = 200;
/// Unread and mention counts stop at this value; callers render it as "99+".
pub const CHAT_UNREAD_CAP: i64 = 99;
/// Most pinned thread ids one request may name.
pub const MAX_CHAT_LIST_PINNED: usize = 200;
/// DM rows list at most this many other members.
pub const MAX_DM_MEMBERS: i64 = 16;

/// Whether a row is a room's main timeline or one thread inside a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRowKind {
    /// A channel or DM main timeline.
    Room,
    /// A followed or pinned thread.
    Thread,
}

/// Keyset cursor: rows strictly older than `(at, key)` in `(at DESC, key DESC)` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatListCursor {
    /// Activity time of the last row already served.
    pub at: DateTime<Utc>,
    /// Stable row key of the last row already served.
    pub key: String,
}

/// One request for a page of the reader's chats list.
#[derive(Debug, Clone)]
pub struct ChatListQuery {
    /// Page size, `1..=MAX_CHAT_LIST_LIMIT`.
    pub limit: u32,
    /// Resume after this row; `None` for the first page.
    pub before: Option<ChatListCursor>,
    /// Pinned thread root ids (32 bytes each) to include even when quiet,
    /// unfollowed, or closed. Pass the same list on every page.
    pub pinned_threads: Vec<Vec<u8>>,
    /// Threads with no reply since this time (and nothing unread) are quiet
    /// and left out.
    pub quiet_before: DateTime<Utc>,
}

/// One chats list row.
#[derive(Debug, Clone)]
pub struct ChatListRow {
    /// Room or thread.
    pub kind: ChatRowKind,
    /// Room the row belongs to.
    pub channel_id: Uuid,
    /// `stream`, `forum`, `dm`, ... as stored on the channel.
    pub channel_type: String,
    /// Channel name (DM names are built by the client from `dm_members`).
    pub channel_name: String,
    /// Whether the room is archived.
    pub archived: bool,
    /// For DM rooms: the other current members (raw 32-byte pubkeys, at most
    /// [`MAX_DM_MEMBERS`]). Empty otherwise.
    pub dm_members: Vec<Vec<u8>>,
    /// Thread root id for thread rows.
    pub root_id: Option<Vec<u8>>,
    /// Newest non-empty thread name (kind 40009) for thread rows.
    pub thread_name: Option<String>,
    /// Thread root event JSON, `None` for rooms or a deleted root.
    pub root: Option<serde_json::Value>,
    /// The reader's follow state for thread rows (`None` for a pinned thread
    /// the reader has no row for).
    pub follow_state: Option<String>,
    /// Sort time: latest main message (room), latest reply (thread), or the
    /// join / root time when there is none.
    pub at: DateTime<Utc>,
    /// Stable key used in the keyset cursor.
    pub key: String,
    /// Channel sequence of the latest main message (room) or reply (thread).
    pub last_seq: Option<i64>,
    /// The reader's stored read position in the same sequence space.
    pub read_seq: Option<i64>,
    /// Latest visible main message (room) or reply (thread), as event JSON.
    pub latest: Option<serde_json::Value>,
    /// Messages from others after `read_seq`, capped at [`CHAT_UNREAD_CAP`] + 1.
    pub unread: i64,
    /// Of those, messages that mention the reader, capped the same way.
    pub unread_mentions: i64,
    /// The reader marked this row unread.
    pub unread_mark: bool,
    /// Thread rows: the thread is closed (only pinned threads can be).
    pub closed: bool,
}

/// A page of rows plus whether older rows exist.
#[derive(Debug, Clone)]
pub struct ChatListPage {
    /// Rows newest first.
    pub rows: Vec<ChatListRow>,
    /// True when another page exists after the last row.
    pub has_more: bool,
}

/// Event JSON built in SQL; a NULL signature stays NULL for the caller to fill.
macro_rules! event_json {
    () => {
        "jsonb_build_object(
            'id', encode(e.id, 'hex'), 'pubkey', encode(e.pubkey, 'hex'),
            'created_at', extract(epoch FROM e.created_at)::bigint, 'kind', e.kind,
            'tags', e.tags, 'content', e.content,
            'sig', CASE WHEN e.sig IS NULL THEN NULL ELSE encode(e.sig, 'hex') END)"
    };
}

/// A reply that only closes or reopens its thread (`$9`/`$10` are the tags).
macro_rules! not_marker {
    () => {
        "NOT EXISTS (
            SELECT 1 FROM jsonb_array_elements(e.tags) mt
            WHERE mt->>0 IN ($9, $10) AND lower(mt->>1) = encode(p.root_id, 'hex'))"
    };
}

/// Main-timeline messages: not a reply, or a broadcast reply.
macro_rules! main_timeline {
    () => {
        "NOT EXISTS (
            SELECT 1 FROM thread_metadata r
            WHERE r.community_id = $1 AND r.event_id = e.id AND r.depth > 0 AND NOT r.broadcast)"
    };
}

/// The reader is mentioned (`$11` is the reader's hex pubkey).
macro_rules! mentions_reader {
    () => {
        "EXISTS (
            SELECT 1 FROM event_mentions em
            WHERE em.community_id = $1 AND em.pubkey_hex = $11 AND em.event_id = e.id)"
    };
}

/// Room unread candidates for page row `p`.
macro_rules! room_unread_from {
    () => {
        concat!(
            " FROM events e
            WHERE e.community_id = $1 AND e.channel_id = p.channel_id
              AND e.channel_sequence > COALESCE(p.read_seq, 0)
              AND e.kind = ANY($3) AND e.deleted_at IS NULL AND e.pubkey <> $2
              AND ",
            main_timeline!()
        )
    };
}

/// Thread unread candidates for page row `p`.
macro_rules! thread_unread_from {
    () => {
        concat!(
            " FROM thread_metadata r
            JOIN events e ON e.community_id = r.community_id
                AND e.created_at = r.event_created_at AND e.id = r.event_id
            WHERE r.community_id = $1 AND r.root_event_id = p.root_id AND r.depth > 0
              AND e.channel_sequence > COALESCE(p.read_seq, 0)
              AND e.kind = ANY($3) AND e.deleted_at IS NULL AND e.pubkey <> $2
              AND ",
            not_marker!()
        )
    };
}

const CHAT_LIST_SQL: &str = concat!(
    r#"
WITH my AS MATERIALIZED (
    SELECT cm.channel_id, c.channel_type::text AS channel_type, c.name AS channel_name,
           c.archived_at, cm.joined_at, cm.hidden_at, cm.read_seq, cm.unread_mark,
           c.main_last_seq, c.main_last_at
    FROM channel_members cm
    JOIN channels c ON c.community_id = cm.community_id AND c.id = cm.channel_id
    WHERE cm.community_id = $1 AND cm.pubkey = $2 AND cm.removed_at IS NULL
      AND c.deleted_at IS NULL
),
rooms AS (
    SELECT 'room'::text AS row_kind, m.channel_id, NULL::bytea AS root_id,
           COALESCE(m.main_last_at, m.joined_at) AS at, m.channel_id::text AS row_key,
           m.main_last_seq AS last_seq, m.read_seq, m.unread_mark,
           NULL::text AS follow_state, FALSE AS closed
    FROM my m
    WHERE m.hidden_at IS NULL
      AND (m.archived_at IS NULL OR m.unread_mark
           OR COALESCE(m.main_last_seq, 0) > COALESCE(m.read_seq, 0))
),
pinned AS (
    SELECT DISTINCT root_id FROM unnest($7::bytea[]) AS pr(root_id)
),
candidates AS (
    SELECT tf.root_event_id AS root_id, tf.channel_id, tf.follow_state, tf.read_seq,
           tf.unread_mark, NULL::timestamptz AS root_at
    FROM thread_follows tf
    WHERE tf.community_id = $1 AND tf.pubkey = $2 AND tf.follow_state <> 'unfollowed'
    UNION ALL
    -- Pinned threads the reader does not follow (no row, or unfollowed).
    SELECT pr.root_id, root.channel_id, tf.follow_state, tf.read_seq,
           COALESCE(tf.unread_mark, FALSE), root.created_at
    FROM pinned pr
    CROSS JOIN LATERAL (
        SELECT e.channel_id, e.created_at FROM events e
        WHERE e.community_id = $1 AND e.id = pr.root_id AND e.channel_id IS NOT NULL
        ORDER BY e.created_at DESC LIMIT 1
    ) root
    LEFT JOIN thread_follows tf ON tf.community_id = $1 AND tf.root_event_id = pr.root_id
        AND tf.pubkey = $2
    WHERE tf.follow_state IS NULL OR tf.follow_state = 'unfollowed'
),
threads AS (
    SELECT 'thread'::text, t.channel_id, t.root_id,
           COALESCE(ts.last_at, t.root_at, m.joined_at), encode(t.root_id, 'hex'),
           ts.last_seq, t.read_seq, t.unread_mark, t.follow_state,
           ts.closed_at IS NOT NULL
    FROM candidates t
    JOIN my m ON m.channel_id = t.channel_id AND m.hidden_at IS NULL
    LEFT JOIN thread_state ts ON ts.community_id = $1 AND ts.root_event_id = t.root_id
        AND ts.channel_id = t.channel_id
    WHERE t.root_id = ANY($7)
       OR (ts.closed_at IS NULL
           AND (t.unread_mark
                OR COALESCE(ts.last_seq, 0) > COALESCE(t.read_seq, 0)
                OR (m.archived_at IS NULL AND ts.last_at >= $8)))
),
page AS (
    SELECT * FROM (SELECT * FROM rooms UNION ALL SELECT * FROM threads) x
    WHERE $4::timestamptz IS NULL OR (x.at, x.row_key) < ($4, $5)
    ORDER BY x.at DESC, x.row_key DESC
    LIMIT $6
)
SELECT p.row_kind, p.channel_id, p.root_id, p.at, p.row_key, p.last_seq, p.read_seq,
    p.unread_mark, p.follow_state, p.closed,
    m.channel_type, m.channel_name, m.archived_at IS NOT NULL AS archived,
    CASE WHEN p.row_kind = 'room' AND m.channel_type = 'dm' THEN ARRAY(
        SELECT o.pubkey FROM channel_members o
        WHERE o.community_id = $1 AND o.channel_id = p.channel_id
          AND o.removed_at IS NULL AND o.pubkey <> $2
        ORDER BY o.joined_at, o.pubkey LIMIT $13
    ) ELSE ARRAY[]::bytea[] END AS dm_members,
    CASE WHEN p.row_kind = 'room' THEN (
        SELECT "#,
    event_json!(),
    r#" FROM events e
        WHERE e.community_id = $1 AND e.channel_id = p.channel_id
          AND e.kind = ANY($3) AND e.deleted_at IS NULL AND "#,
    main_timeline!(),
    r#"
        ORDER BY e.channel_sequence DESC LIMIT 1
    ) ELSE (
        SELECT "#,
    event_json!(),
    r#" FROM thread_metadata r
        JOIN events e ON e.community_id = r.community_id
            AND e.created_at = r.event_created_at AND e.id = r.event_id
        WHERE r.community_id = $1 AND r.root_event_id = p.root_id AND r.depth > 0
          AND e.kind = ANY($3) AND e.deleted_at IS NULL AND "#,
    not_marker!(),
    r#"
        ORDER BY e.channel_sequence DESC LIMIT 1
    ) END AS latest,
    CASE WHEN p.row_kind = 'thread' THEN (
        SELECT "#,
    event_json!(),
    r#" FROM events e
        WHERE e.community_id = $1 AND e.id = p.root_id AND e.deleted_at IS NULL
        ORDER BY e.created_at DESC LIMIT 1
    ) END AS root,
    CASE WHEN p.row_kind = 'thread' THEN (
        SELECT NULLIF(e.content, '') FROM events e
        WHERE e.community_id = $1 AND e.channel_id = p.channel_id AND e.kind = $14
          AND e.deleted_at IS NULL
          AND e.tags @> jsonb_build_array(jsonb_build_array('e', encode(p.root_id, 'hex')))
        ORDER BY e.created_at DESC, e.id DESC LIMIT 1
    ) END AS thread_name,
    CASE WHEN p.row_kind = 'room' THEN (
        SELECT count(*) FROM (SELECT 1"#,
    room_unread_from!(),
    r#" LIMIT $12) n
    ) ELSE (
        SELECT count(*) FROM (SELECT 1"#,
    thread_unread_from!(),
    r#" LIMIT $12) n
    ) END AS unread,
    CASE WHEN p.row_kind = 'room' THEN (
        SELECT count(*) FROM (SELECT 1"#,
    room_unread_from!(),
    " AND ",
    mentions_reader!(),
    r#" LIMIT $12) n
    ) ELSE (
        SELECT count(*) FROM (SELECT 1"#,
    thread_unread_from!(),
    " AND ",
    mentions_reader!(),
    r#" LIMIT $12) n
    ) END AS unread_mentions
FROM page p
JOIN my m ON m.channel_id = p.channel_id
ORDER BY p.at DESC, p.row_key DESC
"#,
);

/// Restore the all-zero sentinel signature for server-stamped rows, matching
/// the generic read path.
fn event_value(value: Option<serde_json::Value>) -> Option<serde_json::Value> {
    let mut value = value?;
    if value.get("sig").is_some_and(serde_json::Value::is_null) {
        value["sig"] = serde_json::Value::String(hex::encode(kuzz_core::draft::SENTINEL_SIG));
    }
    Some(value)
}

impl Db {
    /// Load one page of `reader`'s chats list in a single statement.
    ///
    /// Reads only server-side chat state; callers gate it on the chat-state
    /// switch ([`Db::chat_state_writes`]).
    #[datastore_span(name = "chat_list", system = "postgresql")]
    pub async fn chat_list(
        &self,
        community: CommunityId,
        reader: &nostr::PublicKey,
        query: &ChatListQuery,
    ) -> Result<ChatListPage> {
        if query.limit == 0 || query.limit > MAX_CHAT_LIST_LIMIT {
            return Err(DbError::InvalidData(format!(
                "chat list limit must be 1..={MAX_CHAT_LIST_LIMIT}"
            )));
        }
        if query.pinned_threads.len() > MAX_CHAT_LIST_PINNED
            || query.pinned_threads.iter().any(|id| id.len() != 32)
        {
            return Err(DbError::InvalidData(format!(
                "chat list takes at most {MAX_CHAT_LIST_PINNED} 32-byte pinned thread ids"
            )));
        }
        let mut conn = observability::acquire_writer(
            &self.pool,
            observability::WriterOperation::SubscriptionHistory,
        )
        .await?;
        let rows = sqlx::query(CHAT_LIST_SQL)
            .bind(community.as_uuid())
            .bind(reader.to_bytes().to_vec())
            .bind(&MESSAGE_KINDS[..])
            .bind(query.before.as_ref().map(|c| c.at))
            .bind(query.before.as_ref().map(|c| c.key.as_str()))
            .bind(i64::from(query.limit) + 1)
            .bind(&query.pinned_threads)
            .bind(query.quiet_before)
            .bind(THREAD_CLOSED_TAG)
            .bind(THREAD_REOPENED_TAG)
            .bind(reader.to_hex())
            .bind(CHAT_UNREAD_CAP + 1)
            .bind(MAX_DM_MEMBERS)
            .bind(kuzz_core::kind::KIND_THREAD_NAME as i32)
            .fetch_all(&mut *conn)
            .await?;
        let has_more = rows.len() > query.limit as usize;
        let rows = rows
            .into_iter()
            .take(query.limit as usize)
            .map(|row| {
                let kind = match row.try_get::<String, _>("row_kind")?.as_str() {
                    "room" => ChatRowKind::Room,
                    _ => ChatRowKind::Thread,
                };
                Ok(ChatListRow {
                    kind,
                    channel_id: row.try_get("channel_id")?,
                    channel_type: row.try_get("channel_type")?,
                    channel_name: row.try_get("channel_name")?,
                    archived: row.try_get("archived")?,
                    dm_members: row.try_get("dm_members")?,
                    root_id: row.try_get("root_id")?,
                    thread_name: row.try_get("thread_name")?,
                    root: event_value(row.try_get("root")?),
                    follow_state: row.try_get("follow_state")?,
                    at: row.try_get("at")?,
                    key: row.try_get("row_key")?,
                    last_seq: row.try_get("last_seq")?,
                    read_seq: row.try_get("read_seq")?,
                    latest: event_value(row.try_get("latest")?),
                    unread: row.try_get("unread")?,
                    unread_mentions: row.try_get("unread_mentions")?,
                    unread_mark: row.try_get("unread_mark")?,
                    closed: row.try_get("closed")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ChatListPage { rows, has_more })
    }
}
