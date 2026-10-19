//! Chats list for one reader (Chats stage S2), read from the server-side chat
//! state that the S1 write path keeps.
//!
//! One statement returns a page of the reader's rooms and live threads mixed
//! newest first. Latest message, thread name, and unread counts are the
//! stored values migration 0072 added (Telegram's dialog model), so a row
//! costs a few key lookups. Two cases still search messages as before: the
//! stored latest message is deleted or (in a DM) from someone the reader
//! blocked, and a pinned thread the reader has no follow row for. In a DM
//! with someone the reader blocked, the sort time and latest sequence are
//! searched too, so the blocked person's messages neither move the row up
//! nor bring back an archived room. Counts stop at [`CHAT_UNREAD_CAP`] + 1.
//! Close and reopen markers count nowhere.
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

/// Main-timeline messages: not a reply, or a broadcast reply that is not a
/// close or reopen marker.
macro_rules! main_timeline {
    () => {
        "NOT EXISTS (
            SELECT 1 FROM thread_metadata r
            WHERE r.community_id = $1 AND r.event_id = e.id AND r.depth > 0
              AND (NOT r.broadcast OR chat_is_thread_marker(e.tags, r.root_event_id)))"
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

/// Not a DM message from someone the reader blocked (silent block). `m` is
/// the reader's membership row for the same room.
macro_rules! not_blocked_dm {
    () => {
        "NOT (m.channel_type = 'dm' AND EXISTS (
            SELECT 1 FROM user_blocks ub
            WHERE ub.community_id = $1 AND ub.blocker = $2 AND ub.blocked = e.pubkey))"
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
            not_marker!(),
            " AND ",
            not_blocked_dm!()
        )
    };
}

const CHAT_LIST_SQL: &str = concat!(
    r#"
WITH blk AS MATERIALIZED (
    SELECT ub.blocked FROM user_blocks ub WHERE ub.community_id = $1 AND ub.blocker = $2
),
my AS MATERIALIZED (
    SELECT cm.channel_id, c.channel_type::text AS channel_type, c.name AS channel_name,
           c.archived_at, cm.joined_at, cm.hidden_at, cm.read_seq, cm.unread_mark,
           c.main_last_seq, c.main_last_at, c.main_last_event_id,
           cm.unread_count, cm.unread_mentions,
           -- A DM someone the reader blocked is or was in: its stored latest
           -- may be theirs, so ordering is searched below.
           (c.channel_type = 'dm' AND EXISTS (
               SELECT 1 FROM blk JOIN channel_members bm ON bm.community_id = $1
                   AND bm.channel_id = cm.channel_id AND bm.pubkey = blk.blocked
           )) AS dm_blocked
    FROM channel_members cm
    JOIN channels c ON c.community_id = cm.community_id AND c.id = cm.channel_id
    WHERE cm.community_id = $1 AND cm.pubkey = $2 AND cm.removed_at IS NULL
      AND c.deleted_at IS NULL
),
rooms AS (
    SELECT 'room'::text AS row_kind, m.channel_id, NULL::bytea AS root_id,
           COALESCE(v.last_at, m.joined_at) AS at, m.channel_id::text AS row_key,
           v.last_seq, m.read_seq, m.unread_mark,
           NULL::text AS follow_state, FALSE AS closed,
           v.last_event_id, NULL::text AS stored_name,
           m.unread_count AS stored_unread, m.unread_mentions AS stored_mentions
    FROM my m
    -- In a DM with someone the reader blocked, their messages neither sort
    -- nor revive an archived room: use the latest main message from anyone
    -- else (deleted ones too, as the stored values never move back).
    LEFT JOIN LATERAL (
        SELECT e.channel_sequence, e.received_at, e.id FROM events e
        WHERE m.dm_blocked AND e.community_id = $1 AND e.channel_id = m.channel_id
          AND e.kind = ANY($3) AND "#,
    main_timeline!(),
    r#"
          AND NOT EXISTS (SELECT 1 FROM blk WHERE blk.blocked = e.pubkey)
        ORDER BY e.channel_sequence DESC LIMIT 1
    ) l ON TRUE
    CROSS JOIN LATERAL (
        SELECT CASE WHEN m.dm_blocked THEN l.channel_sequence ELSE m.main_last_seq END AS last_seq,
               CASE WHEN m.dm_blocked THEN l.received_at ELSE m.main_last_at END AS last_at,
               CASE WHEN m.dm_blocked THEN l.id ELSE m.main_last_event_id END AS last_event_id
    ) v
    WHERE m.hidden_at IS NULL
      AND (m.archived_at IS NULL OR m.unread_mark
           OR COALESCE(v.last_seq, 0) > COALESCE(m.read_seq, 0))
),
pinned AS (
    SELECT DISTINCT root_id FROM unnest($7::bytea[]) AS pr(root_id)
),
candidates AS (
    SELECT tf.root_event_id AS root_id, tf.channel_id, tf.follow_state, tf.read_seq,
           tf.unread_mark, NULL::timestamptz AS root_at,
           tf.unread_count AS stored_unread, tf.unread_mentions AS stored_mentions
    FROM thread_follows tf
    WHERE tf.community_id = $1 AND tf.pubkey = $2 AND tf.follow_state <> 'unfollowed'
    UNION ALL
    -- Pinned threads the reader does not follow (no row, or unfollowed):
    -- counted from messages below.
    SELECT pr.root_id, root.channel_id, tf.follow_state, tf.read_seq,
           COALESCE(tf.unread_mark, FALSE), root.created_at,
           NULL::integer, NULL::integer
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
           COALESCE(v.last_at, t.root_at, m.joined_at), encode(t.root_id, 'hex'),
           v.last_seq, t.read_seq, t.unread_mark, t.follow_state,
           ts.closed_at IS NOT NULL,
           v.last_event_id, ts.name, t.stored_unread, t.stored_mentions
    FROM candidates t
    JOIN my m ON m.channel_id = t.channel_id AND m.hidden_at IS NULL
    LEFT JOIN thread_state ts ON ts.community_id = $1 AND ts.root_event_id = t.root_id
        AND ts.channel_id = t.channel_id
    -- Same as rooms: replies from someone the reader blocked do not count.
    LEFT JOIN LATERAL (
        SELECT e.channel_sequence, e.received_at, e.id FROM thread_metadata r
        JOIN events e ON e.community_id = r.community_id
            AND e.created_at = r.event_created_at AND e.id = r.event_id
        WHERE m.dm_blocked AND r.community_id = $1 AND r.root_event_id = t.root_id
          AND r.depth > 0 AND e.kind = ANY($3)
          AND NOT chat_is_thread_marker(e.tags, t.root_id)
          AND NOT EXISTS (SELECT 1 FROM blk WHERE blk.blocked = e.pubkey)
        ORDER BY e.channel_sequence DESC LIMIT 1
    ) l ON TRUE
    CROSS JOIN LATERAL (
        SELECT CASE WHEN m.dm_blocked THEN l.channel_sequence ELSE ts.last_seq END AS last_seq,
               CASE WHEN m.dm_blocked THEN l.received_at ELSE ts.last_at END AS last_at,
               CASE WHEN m.dm_blocked THEN l.id ELSE ts.last_event_id END AS last_event_id
    ) v
    WHERE t.root_id = ANY($7)
       OR (ts.closed_at IS NULL
           AND (t.unread_mark
                OR COALESCE(v.last_seq, 0) > COALESCE(t.read_seq, 0)
                OR (m.archived_at IS NULL AND v.last_at >= $8)))
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
    CASE WHEN p.last_event_id IS NULL THEN NULL ELSE COALESCE((
        -- The stored latest message, unless it is gone or (in a DM) from
        -- someone the reader blocked; then search as before.
        SELECT "#,
    event_json!(),
    r#" FROM events e
        WHERE e.community_id = $1 AND e.id = p.last_event_id AND e.deleted_at IS NULL
          AND "#,
    not_blocked_dm!(),
    r#"
        ORDER BY e.created_at DESC LIMIT 1
    ), CASE WHEN p.row_kind = 'room' THEN (
        SELECT "#,
    event_json!(),
    r#" FROM events e
        WHERE e.community_id = $1 AND e.channel_id = p.channel_id
          AND e.kind = ANY($3) AND e.deleted_at IS NULL AND "#,
    main_timeline!(),
    " AND ",
    not_blocked_dm!(),
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
    " AND ",
    not_blocked_dm!(),
    r#"
        ORDER BY e.channel_sequence DESC LIMIT 1
    ) END) END AS latest,
    CASE WHEN p.row_kind = 'thread' THEN (
        SELECT "#,
    event_json!(),
    r#" FROM events e
        WHERE e.community_id = $1 AND e.id = p.root_id AND e.deleted_at IS NULL
        ORDER BY e.created_at DESC LIMIT 1
    ) END AS root,
    p.stored_name AS thread_name,
    -- Stored counts for rooms and follow rows; pinned threads without a
    -- follow row are counted from messages.
    CASE WHEN p.stored_unread IS NOT NULL THEN p.stored_unread::bigint ELSE (
        SELECT count(*) FROM (SELECT 1"#,
    thread_unread_from!(),
    r#" LIMIT $12) n
    ) END AS unread,
    CASE WHEN p.stored_mentions IS NOT NULL THEN p.stored_mentions::bigint ELSE (
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
