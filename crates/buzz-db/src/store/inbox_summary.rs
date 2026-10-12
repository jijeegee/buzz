//! One-statement inbox summary: the reader's member rooms and their active
//! threads mixed newest first, each with its latest message and unread count.

use buzz_core::kind::{
    KIND_FORUM_COMMENT, KIND_FORUM_POST, KIND_STREAM_MESSAGE, KIND_STREAM_MESSAGE_V2,
};
use buzz_core::CommunityId;
use buzz_datastore_tracing::datastore_span;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::{observability, Db, DbError, Result};

/// Largest page a caller may request.
pub const MAX_INBOX_SUMMARY_LIMIT: u32 = 100;
/// Unread counts stop at this value; callers render it as "99+".
pub const INBOX_UNREAD_CAP: i64 = 99;

/// Kinds that make a room or thread row "active" and count as unread.
const MESSAGE_KINDS: [i32; 4] = [
    KIND_STREAM_MESSAGE as i32,
    KIND_STREAM_MESSAGE_V2 as i32,
    KIND_FORUM_POST as i32,
    KIND_FORUM_COMMENT as i32,
];

/// Whether a row is a room's main timeline or one thread inside a room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxRowKind {
    /// A channel or DM main timeline.
    Room,
    /// A thread root with at least one reply.
    Thread,
}

/// Keyset cursor: rows strictly older than `(at, key)` in `(at DESC, key DESC)` order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxCursor {
    /// Activity time of the last row already served.
    pub at: DateTime<Utc>,
    /// Stable row key of the last row already served.
    pub key: String,
}

/// Caller-supplied read markers, used only to count unread rows in this statement.
#[derive(Debug, Default, Clone)]
pub struct InboxReadMarkers {
    /// Per-room main-timeline markers.
    pub channels: Vec<(Uuid, DateTime<Utc>)>,
    /// Per-thread markers keyed by root event id.
    pub threads: Vec<(Vec<u8>, DateTime<Utc>)>,
}

/// One inbox row.
#[derive(Debug, Clone)]
pub struct InboxSummaryRow {
    /// Room or thread.
    pub kind: InboxRowKind,
    /// Room the row belongs to.
    pub channel_id: Uuid,
    /// `stream`, `forum`, `dm`, ... as stored on the channel.
    pub channel_type: String,
    /// Thread root id for thread rows.
    pub root_id: Option<Vec<u8>>,
    /// Sort time: latest message, latest reply, or join time for an empty room.
    pub at: DateTime<Utc>,
    /// Stable key used in the keyset cursor.
    pub key: String,
    /// Reply count for thread rows.
    pub reply_count: Option<i32>,
    /// Latest visible message (room) or reply (thread), as event JSON.
    pub latest: Option<serde_json::Value>,
    /// Thread root event JSON, `None` for rooms or a deleted root.
    pub root: Option<serde_json::Value>,
    /// Unread messages from others after the supplied marker, capped at
    /// [`INBOX_UNREAD_CAP`] + 1; `None` when no marker was supplied.
    pub unread: Option<i64>,
}

/// A page of rows plus whether older rows exist.
#[derive(Debug, Clone)]
pub struct InboxSummaryPage {
    /// Rows newest first.
    pub rows: Vec<InboxSummaryRow>,
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

const SUMMARY_SQL: &str = concat!(
    r#"
WITH my AS MATERIALIZED (
    SELECT cm.channel_id, c.channel_type::text AS channel_type, cm.joined_at
    FROM channel_members cm
    JOIN channels c ON c.community_id = cm.community_id AND c.id = cm.channel_id
    WHERE cm.community_id = $1 AND cm.pubkey = $2 AND cm.removed_at IS NULL
      AND c.deleted_at IS NULL AND c.archived_at IS NULL
),
rooms AS (
    SELECT 'room'::text AS row_kind, m.channel_id, m.channel_type,
           NULL::bytea AS root_id, NULL::timestamptz AS root_created_at,
           COALESCE(l.created_at, m.joined_at) AS at, m.channel_id::text AS row_key,
           NULL::int AS reply_count, l.id AS latest_id, l.created_at AS latest_created_at
    FROM my m
    LEFT JOIN LATERAL (
        SELECT e.id, e.created_at FROM events e
        WHERE e.community_id = $1 AND e.channel_id = m.channel_id
          AND e.kind = ANY($3) AND e.deleted_at IS NULL
          AND NOT EXISTS (
              SELECT 1 FROM thread_metadata r
              WHERE r.community_id = $1 AND r.event_id = e.id
                AND r.depth > 0 AND NOT r.broadcast)
        ORDER BY e.created_at DESC, e.id DESC LIMIT 1
    ) l ON true
),
threads AS (
    -- No room can contribute more than one page to the global top-N, so each
    -- room reads at most $6 roots from idx_thread_metadata_inbox.
    SELECT 'thread'::text, m.channel_id, m.channel_type,
           t.event_id, t.event_created_at, t.last_reply_at, encode(t.event_id, 'hex'),
           t.reply_count, NULL::bytea, NULL::timestamptz
    FROM my m
    CROSS JOIN LATERAL (
        SELECT t.event_id, t.event_created_at, t.last_reply_at, t.reply_count
        FROM thread_metadata t
        WHERE t.community_id = $1 AND t.channel_id = m.channel_id
          AND t.depth = 0 AND t.reply_count > 0 AND t.last_reply_at IS NOT NULL
          AND ($4::timestamptz IS NULL
               OR (t.last_reply_at, encode(t.event_id, 'hex')) < ($4, $5))
        ORDER BY t.last_reply_at DESC, t.event_id DESC
        LIMIT $6
    ) t
),
page AS (
    SELECT * FROM (SELECT * FROM rooms UNION ALL SELECT * FROM threads) x
    WHERE $4::timestamptz IS NULL OR (x.at, x.row_key) < ($4, $5)
    ORDER BY x.at DESC, x.row_key DESC
    LIMIT $6
),
channel_marks AS (
    SELECT * FROM unnest($7::uuid[], $8::timestamptz[]) AS cr(channel_id, read_at)
),
thread_marks AS (
    SELECT * FROM unnest($9::bytea[], $10::timestamptz[]) AS tr(root_id, read_at)
)
SELECT p.row_kind, p.channel_id, p.channel_type, p.root_id, p.at, p.row_key, p.reply_count,
    CASE WHEN p.row_kind = 'room' THEN (
        SELECT "#,
    event_json!(),
    r#" FROM events e
        WHERE e.community_id = $1 AND e.created_at = p.latest_created_at AND e.id = p.latest_id
    ) ELSE (
        SELECT "#,
    event_json!(),
    r#" FROM thread_metadata r
        JOIN events e ON e.community_id = r.community_id
            AND e.created_at = r.event_created_at AND e.id = r.event_id
        WHERE r.community_id = $1 AND r.root_event_id = p.root_id AND r.depth > 0
          AND e.kind = ANY($3) AND e.deleted_at IS NULL
        ORDER BY r.event_created_at DESC, r.event_id DESC LIMIT 1
    ) END AS latest,
    CASE WHEN p.row_kind = 'thread' THEN (
        SELECT "#,
    event_json!(),
    r#" FROM events e
        WHERE e.community_id = $1 AND e.created_at = p.root_created_at AND e.id = p.root_id
          AND e.deleted_at IS NULL
    ) END AS root,
    CASE WHEN p.row_kind = 'room' THEN (
        SELECT count(*) FROM (
            SELECT 1 FROM events e
            WHERE e.community_id = $1 AND e.channel_id = p.channel_id
              AND e.kind = ANY($3) AND e.deleted_at IS NULL AND e.pubkey <> $2
              AND e.created_at > cm.read_at
              AND NOT EXISTS (
                  SELECT 1 FROM thread_metadata r
                  WHERE r.community_id = $1 AND r.event_id = e.id
                    AND r.depth > 0 AND NOT r.broadcast)
            LIMIT $11
        ) n WHERE cm.read_at IS NOT NULL
    ) ELSE (
        SELECT count(*) FROM (
            SELECT 1 FROM thread_metadata r
            JOIN events e ON e.community_id = r.community_id
                AND e.created_at = r.event_created_at AND e.id = r.event_id
            WHERE r.community_id = $1 AND r.root_event_id = p.root_id AND r.depth > 0
              AND e.kind = ANY($3) AND e.deleted_at IS NULL AND e.pubkey <> $2
              AND e.created_at > GREATEST(tm.read_at, cm.read_at)
            LIMIT $11
        ) n WHERE COALESCE(tm.read_at, cm.read_at) IS NOT NULL
    ) END AS unread,
    (cm.read_at IS NOT NULL OR tm.read_at IS NOT NULL) AS has_marker
FROM page p
LEFT JOIN channel_marks cm ON cm.channel_id = p.channel_id
LEFT JOIN thread_marks tm ON p.row_kind = 'thread' AND tm.root_id = p.root_id
ORDER BY p.at DESC, p.row_key DESC
"#,
);

/// Restore the all-zero sentinel signature for server-stamped rows, matching
/// the generic read path.
fn event_value(value: Option<serde_json::Value>) -> Option<serde_json::Value> {
    let mut value = value?;
    if value.get("sig").is_some_and(serde_json::Value::is_null) {
        value["sig"] = serde_json::Value::String(hex::encode(buzz_core::draft::SENTINEL_SIG));
    }
    Some(value)
}

impl Db {
    /// Load one inbox page for `reader` in a single statement.
    ///
    /// Rooms are the reader's current, non-archived memberships; threads are
    /// replied-to roots in those rooms. Read markers are used only for the
    /// unread counts of this page and are never stored.
    #[datastore_span(name = "inbox_summary", system = "postgresql")]
    pub async fn inbox_summary(
        &self,
        community: CommunityId,
        reader: &nostr::PublicKey,
        limit: u32,
        before: Option<&InboxCursor>,
        markers: &InboxReadMarkers,
    ) -> Result<InboxSummaryPage> {
        if limit == 0 || limit > MAX_INBOX_SUMMARY_LIMIT {
            return Err(DbError::InvalidData(format!(
                "inbox summary limit must be 1..={MAX_INBOX_SUMMARY_LIMIT}"
            )));
        }
        let (channel_ids, channel_reads): (Vec<Uuid>, Vec<DateTime<Utc>>) =
            markers.channels.iter().cloned().unzip();
        let (thread_ids, thread_reads): (Vec<Vec<u8>>, Vec<DateTime<Utc>>) =
            markers.threads.iter().cloned().unzip();
        let mut conn = observability::acquire_writer(
            &self.pool,
            observability::WriterOperation::SubscriptionHistory,
        )
        .await?;
        let rows = sqlx::query(SUMMARY_SQL)
            .bind(community.as_uuid())
            .bind(reader.to_bytes().to_vec())
            .bind(&MESSAGE_KINDS[..])
            .bind(before.map(|c| c.at))
            .bind(before.map(|c| c.key.as_str()))
            .bind(i64::from(limit) + 1)
            .bind(&channel_ids)
            .bind(&channel_reads)
            .bind(&thread_ids)
            .bind(&thread_reads)
            .bind(INBOX_UNREAD_CAP + 1)
            .fetch_all(&mut *conn)
            .await?;
        let has_more = rows.len() > limit as usize;
        let rows = rows
            .into_iter()
            .take(limit as usize)
            .map(|row| {
                let kind = match row.try_get::<String, _>("row_kind")?.as_str() {
                    "room" => InboxRowKind::Room,
                    _ => InboxRowKind::Thread,
                };
                let has_marker: bool = row.try_get("has_marker")?;
                Ok(InboxSummaryRow {
                    kind,
                    channel_id: row.try_get("channel_id")?,
                    channel_type: row.try_get("channel_type")?,
                    root_id: row.try_get("root_id")?,
                    at: row.try_get("at")?,
                    key: row.try_get("row_key")?,
                    reply_count: row.try_get("reply_count")?,
                    latest: event_value(row.try_get("latest")?),
                    root: event_value(row.try_get("root")?),
                    unread: if has_marker {
                        row.try_get("unread")?
                    } else {
                        None
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(InboxSummaryPage { rows, has_more })
    }
}
