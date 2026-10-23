//! Thread search by name for one reader.
//!
//! The Chats list leaves out closed threads and threads that have been quiet
//! for a while, so a client that matches names only over its Chats rows
//! cannot find them. This searches the stored newest thread name
//! (`thread_state.name`, migration 0072) instead, whatever the thread's
//! close state, quietness, or the reader's follow state.
//!
//! Privacy: a thread is returned only from a room the reader is a current
//! member of (`channel_members.removed_at IS NULL`), that is not deleted, and
//! that the reader has not hidden. Open channels the reader never joined are
//! not searched, and neither are DMs or private rooms of others. In a 1:1 DM
//! with someone the reader blocked, a thread is left out when its root or
//! its newest name was sent during a block period (the reader never received
//! it).
//!
//! The match is a case-insensitive substring of the name, so a word in the
//! middle matches. Results are ordered exact, prefix, word start, middle,
//! then most recent activity. The statement runs under the same budget as
//! message search ([`THREAD_SEARCH_STATEMENT_TIMEOUT_MS`]).
//!
//! Scale note: rooms are joined to `thread_state` through its primary key
//! prefix (`community_id`), and each named row is checked with `strpos`.
//! That is fine while a community has a few thousand named threads; a
//! `pg_trgm` GIN index on `lower(name)` would be the next step at scale.

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::{Acquire, Row};
use uuid::Uuid;

use crate::chat_list::MAX_DM_MEMBERS;
use crate::{observability, Db, DbError, Result};

/// Largest result count a caller may request.
pub const MAX_THREAD_SEARCH_LIMIT: u32 = 50;
/// Longest query, in characters.
pub const MAX_THREAD_SEARCH_QUERY_CHARS: usize = 100;
/// Statement budget, the same as message search (`kuzz-search`).
pub const THREAD_SEARCH_STATEMENT_TIMEOUT_MS: u64 = 2_000;

/// One thread whose name matched.
#[derive(Debug, Clone)]
pub struct ThreadSearchRow {
    /// Thread root event id (32 bytes).
    pub root_id: Vec<u8>,
    /// Room the thread is in.
    pub channel_id: Uuid,
    /// `stream`, `forum`, `dm`, ... as stored on the channel.
    pub channel_type: String,
    /// Channel name (DM names are built by the client from `dm_members`).
    pub channel_name: String,
    /// Whether the room is archived.
    pub archived: bool,
    /// Ownerless group chat.
    pub group_chat: bool,
    /// Group chats only: active members, the reader included.
    pub member_count: Option<i64>,
    /// For DM rooms and group chats: the other current members (raw 32-byte
    /// pubkeys, oldest first, at most [`MAX_DM_MEMBERS`]). Empty otherwise.
    pub dm_members: Vec<Vec<u8>>,
    /// Newest non-empty thread name (kind 40009).
    pub name: String,
    /// Latest reply time, or the root's time when there is no reply.
    pub at: DateTime<Utc>,
    /// The thread is closed (completed).
    pub closed: bool,
    /// 0 exact, 1 prefix, 2 word start, 3 middle.
    pub match_rank: i32,
}

/// Characters a word starts after (the mobile client's set); each becomes a
/// space before the word-start test.
const WORD_BREAKS: &str = "-_#./([·›";
const WORD_BREAK_SPACES: &str = "         ";

const THREAD_SEARCH_SQL: &str = concat!(
    r#"
WITH blk AS MATERIALIZED (
    SELECT DISTINCT bp.blocked FROM user_block_periods bp
    WHERE bp.community_id = $1 AND bp.blocker = $2
),
my AS MATERIALIZED (
    SELECT cm.channel_id, c.channel_type::text AS channel_type, c.name AS channel_name,
           c.group_chat, c.archived_at,
           (c.channel_type = 'dm' AND EXISTS (
               SELECT 1 FROM blk JOIN channel_members bm ON bm.community_id = $1
                   AND bm.channel_id = cm.channel_id AND bm.pubkey = blk.blocked
           ) AND dm_is_one_to_one($1, cm.channel_id)) AS dm_blocked
    FROM channel_members cm
    JOIN channels c ON c.community_id = cm.community_id AND c.id = cm.channel_id
    WHERE cm.community_id = $1 AND cm.pubkey = $2 AND cm.removed_at IS NULL
      AND cm.hidden_at IS NULL AND c.deleted_at IS NULL
),
q AS (
    SELECT lower($3) AS lq
),
hits AS (
    SELECT ts.root_event_id, ts.channel_id, ts.name, ts.last_at,
           ts.closed_at IS NOT NULL AS closed, lower(ts.name) AS lname,
           m.channel_type, m.channel_name, m.group_chat, m.archived_at, m.dm_blocked
    FROM thread_state ts
    JOIN my m ON m.channel_id = ts.channel_id
    CROSS JOIN q
    WHERE ts.community_id = $1 AND ts.name IS NOT NULL
      AND strpos(lower(ts.name), q.lq) > 0
),
ranked AS (
    SELECT h.*, root.created_at AS root_at,
           CASE WHEN h.lname = q.lq THEN 0
                WHEN starts_with(h.lname, q.lq) THEN 1
                WHEN strpos(' ' || translate(h.lname, $5, $6),
                            ' ' || translate(q.lq, $5, $6)) > 0 THEN 2
                ELSE 3 END AS match_rank
    FROM hits h
    CROSS JOIN q
    LEFT JOIN LATERAL (
        SELECT e.created_at, e.received_at, e.pubkey, e.channel_sequence FROM events e
        WHERE e.community_id = $1 AND e.id = h.root_event_id
        ORDER BY e.created_at DESC LIMIT 1
    ) root ON TRUE
    -- since_join rooms: a thread whose root predates the reader's join is
    -- cut whole (a missing root counts as old).
    WHERE NOT channel_event_before_join($1, h.channel_id, COALESCE(root.channel_sequence, 0), $2)
    -- In a 1:1 DM with someone the reader blocked: never surface a root or a
    -- name the reader did not receive.
      AND NOT (h.dm_blocked AND (
        "#,
    crate::blocks::hidden_by_block_period!("$1", "$2", "root"),
    r#"
        OR EXISTS (
            SELECT 1 FROM (
                SELECT e.pubkey, e.received_at FROM events e
                WHERE e.community_id = $1 AND e.channel_id = h.channel_id
                  AND e.kind = 40009 AND e.deleted_at IS NULL
                  AND e.tags @> jsonb_build_array(
                      jsonb_build_array('e', encode(h.root_event_id, 'hex')))
                ORDER BY e.created_at DESC, e.id DESC LIMIT 1
            ) n WHERE "#,
    crate::blocks::hidden_by_block_period!("$1", "$2", "n"),
    r#"
        )))
),
page AS (
    SELECT * FROM ranked
    ORDER BY match_rank, COALESCE(last_at, root_at) DESC NULLS LAST, root_event_id
    LIMIT $4
)
SELECT p.root_event_id, p.channel_id, p.channel_type, p.channel_name,
    p.archived_at IS NOT NULL AS archived, p.group_chat, p.name, p.closed, p.match_rank,
    COALESCE(p.last_at, p.root_at, to_timestamp(0)) AS at,
    CASE WHEN p.group_chat THEN (
        SELECT count(*) FROM channel_members o
        WHERE o.community_id = $1 AND o.channel_id = p.channel_id AND o.removed_at IS NULL
    ) END AS member_count,
    CASE WHEN p.channel_type = 'dm' OR p.group_chat THEN ARRAY(
        SELECT o.pubkey FROM channel_members o
        WHERE o.community_id = $1 AND o.channel_id = p.channel_id
          AND o.removed_at IS NULL AND o.pubkey <> $2
        ORDER BY o.joined_at, o.pubkey LIMIT $7
    ) ELSE ARRAY[]::bytea[] END AS dm_members
FROM page p
ORDER BY p.match_rank, at DESC, p.root_event_id
"#,
);

impl Db {
    /// Threads in `reader`'s rooms whose stored name contains `query`
    /// (case-insensitive), closed and quiet ones included, best match first.
    ///
    /// `query` must be non-blank, at most [`MAX_THREAD_SEARCH_QUERY_CHARS`]
    /// characters, and free of NUL; `limit` is `1..=MAX_THREAD_SEARCH_LIMIT`.
    #[datastore_span(name = "thread_search", system = "postgresql")]
    pub async fn thread_search(
        &self,
        community: CommunityId,
        reader: &nostr::PublicKey,
        query: &str,
        limit: u32,
    ) -> Result<Vec<ThreadSearchRow>> {
        let query = query.trim();
        if query.is_empty()
            || query.chars().count() > MAX_THREAD_SEARCH_QUERY_CHARS
            || query.contains('\0')
        {
            return Err(DbError::InvalidData(format!(
                "thread search query must be 1..={MAX_THREAD_SEARCH_QUERY_CHARS} characters"
            )));
        }
        if limit == 0 || limit > MAX_THREAD_SEARCH_LIMIT {
            return Err(DbError::InvalidData(format!(
                "thread search limit must be 1..={MAX_THREAD_SEARCH_LIMIT}"
            )));
        }
        let mut conn = observability::acquire_writer(
            &self.pool,
            observability::WriterOperation::SubscriptionHistory,
        )
        .await?;
        // Transaction-local budget (like `SET LOCAL`): the pooled connection
        // goes back without it.
        let mut tx = conn.begin().await?;
        sqlx::query("SELECT set_config('statement_timeout', $1, true)")
            .bind(THREAD_SEARCH_STATEMENT_TIMEOUT_MS.to_string())
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(THREAD_SEARCH_SQL)
            .bind(community.as_uuid())
            .bind(reader.to_bytes().to_vec())
            .bind(query)
            .bind(i64::from(limit))
            .bind(WORD_BREAKS)
            .bind(WORD_BREAK_SPACES)
            .bind(MAX_DM_MEMBERS)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        rows.into_iter()
            .map(|row| {
                Ok(ThreadSearchRow {
                    root_id: row.try_get("root_event_id")?,
                    channel_id: row.try_get("channel_id")?,
                    channel_type: row.try_get("channel_type")?,
                    channel_name: row.try_get("channel_name")?,
                    archived: row.try_get("archived")?,
                    group_chat: row.try_get("group_chat")?,
                    member_count: row.try_get("member_count")?,
                    dm_members: row.try_get("dm_members")?,
                    name: row.try_get("name")?,
                    at: row.try_get("at")?,
                    closed: row.try_get("closed")?,
                    match_rank: row.try_get("match_rank")?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_breaks_map_one_to_one_onto_spaces() {
        assert_eq!(
            WORD_BREAKS.chars().count(),
            WORD_BREAK_SPACES.chars().count()
        );
        assert!(WORD_BREAK_SPACES.chars().all(|c| c == ' '));
    }
}
