//! One-off backfill of server-side chat state from message history (Chats
//! stage S3).
//!
//! Fills, one channel per transaction, the state the S1 write path keeps for
//! new messages, so the chats list is right from the moment the chat-state
//! switch is turned on:
//!
//! - `channels.main_last_*`: the latest top-level or broadcast message.
//! - `thread_state`: the latest reply (`last_*`) and, for threads that have
//!   no row yet, the close state, which follows the latest reply: a close
//!   marker closes, a reopen marker or any other reply reopens.
//! - `channel_members.read_seq`: the room's current sequence for current
//!   members that have no read position yet (`read_origin = 'backfill'`).
//! - `thread_follows`: `auto` rows (`follow_origin = 'backfill'`) for current
//!   members who started, replied in, or were mentioned in (root or reply) a
//!   thread with a reply inside the follow window; `read_seq` = the thread's
//!   latest reply. The same people's existing, not unfollowed rows without a
//!   read position get that `read_seq` too. Both carry `read_origin =
//!   'backfill'`, so one first-run client upload (`chat_import`) may still
//!   move them.
//!
//! Rules shared with the write path: only `MESSAGE_KINDS` count; close and
//! reopen marker replies never move ordering, follows, or read positions; a
//! thread exists only while its root is a live event in the same channel.
//! Unlike the write path, which never undoes state on delete, the backfill
//! reads only live (not deleted) messages.
//!
//! Runs after the chat-state switch (`KUZZ_CHAT_STATE_WRITES`) is on: the
//! live write path records new activity, the backfill fills in what is
//! older. Rows the write path (or an earlier run) wrote win: the backfill
//! never lowers `main_last_seq` or a thread's `last_seq`, never changes an
//! existing thread's close state, never replaces a read position, and only
//! fills a missing follow `read_seq` (never state, origin, or unread mark).
//! So a re-run changes nothing unless new members or follow rows appeared.
//! Each channel is processed under the same advisory lock the message insert
//! and membership paths take, so its history cannot move underneath it, and
//! the community write fence is checked first.
//!
//! Read positions and follows are private per-user state: never log them.

use std::ops::AddAssign;

use chrono::{DateTime, Duration, Utc};
use kuzz_core::CommunityId;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::chat_state::{THREAD_CLOSED_TAG, THREAD_REOPENED_TAG};
use crate::inbox_summary::MESSAGE_KINDS;
use crate::{observability, Db, Result};

/// Threads with a reply in this many days get backfilled auto follows.
pub const CHAT_BACKFILL_FOLLOW_DAYS: i64 = 30;

/// How one backfill run behaves.
#[derive(Debug, Clone, Copy)]
pub struct ChatBackfillOptions {
    /// Reference time: follow window end and `thread_follows.updated_at`.
    pub now: DateTime<Utc>,
    /// Threads whose latest reply is older than `now - follow_window` get no
    /// backfilled follows.
    pub follow_window: Duration,
    /// Compute everything, report the counts, and roll back.
    pub dry_run: bool,
}

impl ChatBackfillOptions {
    /// Default window ([`CHAT_BACKFILL_FOLLOW_DAYS`]) ending at `now`.
    pub fn new(now: DateTime<Utc>, dry_run: bool) -> Self {
        Self {
            now,
            follow_window: Duration::days(CHAT_BACKFILL_FOLLOW_DAYS),
            dry_run,
        }
    }

    /// Default window ending at the current time.
    pub fn from_now(dry_run: bool) -> Self {
        Self::new(Utc::now(), dry_run)
    }
}

/// Rows the backfill wrote (or would write, in a dry run).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ChatBackfillCounts {
    /// Channels processed.
    pub channels: u64,
    /// Channels whose `main_last_*` moved forward.
    pub main_last_updated: u64,
    /// New `thread_state` rows.
    pub thread_state_inserted: u64,
    /// Existing `thread_state` rows that changed.
    pub thread_state_updated: u64,
    /// Of the written `thread_state` rows, those left closed.
    pub threads_closed: u64,
    /// Members given a backfilled room read position.
    pub member_reads_filled: u64,
    /// New backfilled `auto` follow rows.
    pub follows_inserted: u64,
    /// Existing, not unfollowed, follow rows without a read position given
    /// the thread's latest reply.
    pub follow_reads_filled: u64,
}

impl ChatBackfillCounts {
    /// Whether the run wrote nothing besides visiting channels.
    pub fn is_unchanged(&self) -> bool {
        Self {
            channels: 0,
            ..*self
        } == Self::default()
    }
}

impl AddAssign for ChatBackfillCounts {
    fn add_assign(&mut self, other: Self) {
        self.channels += other.channels;
        self.main_last_updated += other.main_last_updated;
        self.thread_state_inserted += other.thread_state_inserted;
        self.thread_state_updated += other.thread_state_updated;
        self.threads_closed += other.threads_closed;
        self.member_reads_filled += other.member_reads_filled;
        self.follows_inserted += other.follows_inserted;
        self.follow_reads_filled += other.follow_reads_filled;
    }
}

/// Live replies of channel `$2`, keyed by thread root (`$3` message kinds).
macro_rules! channel_replies {
    () => {
        "SELECT COALESCE(tm.root_event_id, tm.parent_event_id) AS root_id,
                e.channel_sequence AS seq, e.received_at, e.pubkey, e.tags
         FROM thread_metadata tm
         JOIN events e ON e.community_id = tm.community_id
             AND e.created_at = tm.event_created_at AND e.id = tm.event_id
         WHERE tm.community_id = $1 AND tm.channel_id = $2
           AND tm.depth > 0 AND tm.parent_event_id IS NOT NULL
           AND e.channel_id = $2 AND e.kind = ANY($3) AND e.deleted_at IS NULL
           AND e.channel_sequence > 0"
    };
}

/// A marker tag on reply `r` naming its own root (`$4`/`$5` are the tags).
macro_rules! has_marker {
    ($tag:literal) => {
        concat!(
            "EXISTS (SELECT 1 FROM jsonb_array_elements(r.tags) mt
                     WHERE mt->>0 = ",
            $tag,
            " AND lower(mt->>1) = encode(r.root_id, 'hex'))"
        )
    };
}

/// The thread root `x.root_id` is a live event in channel `$2`.
macro_rules! live_root {
    () => {
        "EXISTS (SELECT 1 FROM events root
                 WHERE root.community_id = $1 AND root.id = x.root_id
                   AND root.channel_id = $2 AND root.deleted_at IS NULL)"
    };
}

/// Latest main-timeline message: not a reply, or a broadcast reply that is not
/// a close/reopen marker. Moves `main_last_*` forward only.
const MAIN_LAST_SQL: &str = concat!(
    "WITH r AS (
        SELECT e.channel_sequence AS seq, e.received_at, e.id, e.tags,
               COALESCE(tm.root_event_id, tm.parent_event_id) AS root_id, tm.broadcast
        FROM events e
        LEFT JOIN thread_metadata tm ON tm.community_id = e.community_id
            AND tm.event_created_at = e.created_at AND tm.event_id = e.id
            AND tm.channel_id = $2 AND tm.parent_event_id IS NOT NULL
        WHERE e.community_id = $1 AND e.channel_id = $2 AND e.kind = ANY($3)
          AND e.deleted_at IS NULL AND e.channel_sequence > 0
    ),
    latest AS (
        SELECT r.seq, r.received_at, r.id FROM r
        WHERE r.root_id IS NULL
           OR (r.broadcast AND NOT ",
    has_marker!("$4"),
    " AND NOT ",
    has_marker!("$5"),
    ")
        ORDER BY r.seq DESC LIMIT 1
    )
    UPDATE channels c
    SET main_last_seq = l.seq, main_last_at = l.received_at, main_last_event_id = l.id
    FROM latest l
    WHERE c.community_id = $1 AND c.id = $2
      AND (c.main_last_seq IS NULL OR c.main_last_seq < l.seq)"
);

/// Thread activity and close state from every thread's live replies. An
/// existing row keeps its close state (the write path's or an earlier run's)
/// and only moves `last_*` forward.
const THREAD_STATE_SQL: &str = concat!(
    "WITH r AS (",
    channel_replies!(),
    "),
    marked AS (
        SELECT r.root_id, r.seq, r.received_at, r.pubkey,
               CASE WHEN ",
    has_marker!("$5"),
    " THEN 2 WHEN ",
    has_marker!("$4"),
    " THEN 1 ELSE 0 END AS marker
        FROM r
    ),
    threads AS (
        SELECT x.root_id,
               max(x.seq) FILTER (WHERE x.marker = 0) AS last_seq,
               max(x.received_at) FILTER (WHERE x.marker = 0) AS last_at,
               (array_agg(x.marker ORDER BY x.seq DESC))[1] AS final_marker,
               (array_agg(x.received_at ORDER BY x.seq DESC))[1] AS final_at,
               (array_agg(x.pubkey ORDER BY x.seq DESC))[1] AS final_by
        FROM marked x
        WHERE ",
    live_root!(),
    "
        GROUP BY x.root_id
    )
    INSERT INTO thread_state AS ts
        (community_id, root_event_id, channel_id, last_seq, last_at, closed_at, closed_by)
    SELECT $1, t.root_id, $2, t.last_seq, t.last_at,
           CASE WHEN t.final_marker = 1 THEN t.final_at END,
           CASE WHEN t.final_marker = 1 THEN t.final_by END
    FROM threads t
    ON CONFLICT (community_id, root_event_id) DO UPDATE
    SET last_seq = GREATEST(ts.last_seq, EXCLUDED.last_seq),
        last_at = GREATEST(ts.last_at, EXCLUDED.last_at)
    WHERE (ts.last_seq, ts.last_at)
          IS DISTINCT FROM (GREATEST(ts.last_seq, EXCLUDED.last_seq),
                            GREATEST(ts.last_at, EXCLUDED.last_at))
    RETURNING (xmax = 0) AS inserted, closed_at IS NOT NULL AS closed"
);

/// Room read position for current members without one.
const MEMBER_READS_SQL: &str = "UPDATE channel_members cm
    SET read_seq = c.accepted_sequence, read_origin = 'backfill'
    FROM channels c
    WHERE c.community_id = $1 AND c.id = $2
      AND cm.community_id = $1 AND cm.channel_id = $2
      AND cm.removed_at IS NULL AND cm.read_seq IS NULL
      AND cm.read_origin IS DISTINCT FROM 'user'";

/// Auto follows for current members of threads active since `$6`. An
/// existing row only gets a missing read position, unless it is unfollowed.
const FOLLOWS_SQL: &str = concat!(
    "WITH active AS (
        SELECT ts.root_event_id AS root_id, ts.last_seq
        FROM thread_state ts
        WHERE ts.community_id = $1 AND ts.channel_id = $2
          AND ts.last_seq IS NOT NULL AND ts.last_at >= $6
    ),
    roots AS (
        SELECT x.root_id, root.pubkey, root.tags
        FROM active x
        CROSS JOIN LATERAL (
            SELECT e.pubkey, e.tags FROM events e
            WHERE e.community_id = $1 AND e.id = x.root_id AND e.channel_id = $2
              AND e.deleted_at IS NULL
            ORDER BY e.created_at DESC LIMIT 1
        ) root
    ),
    r AS (",
    channel_replies!(),
    "),
    replies AS (
        SELECT r.root_id, r.pubkey, r.tags FROM r
        JOIN roots x ON x.root_id = r.root_id
        WHERE NOT ",
    has_marker!("$4"),
    " AND NOT ",
    has_marker!("$5"),
    "
    ),
    messages AS (
        SELECT root_id, pubkey, tags FROM roots
        UNION ALL
        SELECT root_id, pubkey, tags FROM replies
    ),
    people AS (
        SELECT m.root_id, m.pubkey FROM messages m
        UNION
        SELECT m.root_id, decode(lower(p->>1), 'hex')
        FROM messages m CROSS JOIN LATERAL jsonb_array_elements(m.tags) p
        WHERE p->>0 = 'p' AND p->>1 ~ '^[0-9a-fA-F]{64}$'
    )
    INSERT INTO thread_follows
        (community_id, root_event_id, pubkey, channel_id, follow_state, follow_origin,
         read_seq, read_origin, unread_mark, updated_at)
    SELECT $1, p.root_id, cm.pubkey, $2, 'auto', 'backfill', a.last_seq, 'backfill', FALSE, $7
    FROM people p
    JOIN active a ON a.root_id = p.root_id
    JOIN channel_members cm ON cm.community_id = $1 AND cm.channel_id = $2
        AND cm.pubkey = p.pubkey AND cm.removed_at IS NULL
    ON CONFLICT (community_id, root_event_id, pubkey) DO UPDATE
    SET read_seq = EXCLUDED.read_seq, read_origin = 'backfill'
    WHERE thread_follows.read_seq IS NULL AND thread_follows.follow_state <> 'unfollowed'
    RETURNING (xmax = 0) AS inserted"
);

async fn backfill_channel_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel_id: Uuid,
    options: &ChatBackfillOptions,
) -> Result<ChatBackfillCounts> {
    // Fail closed before reading anything if the community is being deleted;
    // the per-table fence triggers would reject the writes anyway.
    sqlx::query("SELECT assert_community_write_allowed($1)")
        .bind(community)
        .execute(&mut **tx)
        .await?;
    // The lock message inserts (sequence trigger) and membership changes take
    // for this channel: history and roster stay still until commit.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended(
            'buzz_channel_membership:' || $1::text || ':' || $2::text, 0))",
    )
    .bind(community)
    .bind(channel_id)
    .execute(&mut **tx)
    .await?;

    let mut counts = ChatBackfillCounts {
        channels: 1,
        ..ChatBackfillCounts::default()
    };

    counts.main_last_updated = sqlx::query(MAIN_LAST_SQL)
        .bind(community)
        .bind(channel_id)
        .bind(&MESSAGE_KINDS[..])
        .bind(THREAD_CLOSED_TAG)
        .bind(THREAD_REOPENED_TAG)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    let written = sqlx::query(THREAD_STATE_SQL)
        .bind(community)
        .bind(channel_id)
        .bind(&MESSAGE_KINDS[..])
        .bind(THREAD_CLOSED_TAG)
        .bind(THREAD_REOPENED_TAG)
        .fetch_all(&mut **tx)
        .await?;
    for row in written {
        if row.try_get::<bool, _>("inserted")? {
            counts.thread_state_inserted += 1;
        } else {
            counts.thread_state_updated += 1;
        }
        if row.try_get::<bool, _>("closed")? {
            counts.threads_closed += 1;
        }
    }

    counts.member_reads_filled = sqlx::query(MEMBER_READS_SQL)
        .bind(community)
        .bind(channel_id)
        .execute(&mut **tx)
        .await?
        .rows_affected();

    let written = sqlx::query(FOLLOWS_SQL)
        .bind(community)
        .bind(channel_id)
        .bind(&MESSAGE_KINDS[..])
        .bind(THREAD_CLOSED_TAG)
        .bind(THREAD_REOPENED_TAG)
        .bind(options.now - options.follow_window)
        .bind(options.now)
        .fetch_all(&mut **tx)
        .await?;
    for row in written {
        if row.try_get::<bool, _>("inserted")? {
            counts.follows_inserted += 1;
        } else {
            counts.follow_reads_filled += 1;
        }
    }

    Ok(counts)
}

impl Db {
    /// One page of the community's live channel ids, in id order, after
    /// `after`. Drives the channel-by-channel backfill.
    pub async fn chat_backfill_channels(
        &self,
        community: CommunityId,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<Uuid>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM channels \
             WHERE community_id = $1 AND deleted_at IS NULL \
               AND ($2::uuid IS NULL OR id > $2) \
             ORDER BY id LIMIT $3",
        )
        .bind(community.as_uuid())
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Backfill one channel's chat state in its own transaction. With
    /// `dry_run` the transaction is rolled back after counting.
    pub async fn backfill_chat_state_channel(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        options: &ChatBackfillOptions,
    ) -> Result<ChatBackfillCounts> {
        let connection =
            observability::acquire_writer(&self.pool, observability::WriterOperation::Maintenance)
                .await?;
        let mut tx = sqlx::Transaction::begin(connection, None).await?;
        let counts =
            backfill_channel_tx(&mut tx, *community.as_uuid(), channel_id, options).await?;
        if options.dry_run {
            tx.rollback().await?;
        } else {
            tx.commit().await?;
        }
        Ok(counts)
    }
}
