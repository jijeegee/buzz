//! Silent user blocks (`user_blocks`) and their time spans
//! (`user_block_periods`).
//!
//! A `user_blocks` row means `blocker` blocks `blocked` right now: live
//! delivery, push and ingest-time unread counts skip `blocked`'s 1:1 DM
//! events for `blocker`. A `user_block_periods` row keeps one block's time
//! span (open while the block is on): reads of stored 1:1 DM messages hide a
//! message iff its author has a period with the reader covering the
//! message's server receipt time, so messages from before a block stay
//! visible and messages sent during it stay hidden after unblocking. The rows
//! are private to the blocker: nothing here is ever served to `blocked`.

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::Db;

/// Most people one account may block in one community.
pub const MAX_BLOCKS_PER_USER: i64 = 1000;

/// Most channel ids one [`Db::one_to_one_dm_ids`] call may name.
pub const MAX_DM_CHANNEL_LOOKUP: usize = 1000;

/// Most event ids one [`Db::block_hidden_event_ids`] call may name.
pub const MAX_HIDDEN_EVENT_LOOKUP: usize = 5000;

/// SQL condition: the reader blocked the author of event row `$e` at the
/// time the server received it. `$cid` / `$reader` are SQL expressions for
/// the community and the reader's pubkey.
macro_rules! hidden_by_block_period {
    ($cid:literal, $reader:literal, $e:literal) => {
        concat!(
            "EXISTS (SELECT 1 FROM user_block_periods bp WHERE bp.community_id = ",
            $cid,
            " AND bp.blocker = ",
            $reader,
            " AND bp.blocked = ",
            $e,
            ".pubkey AND bp.started_at <= ",
            $e,
            ".received_at AND (bp.ended_at IS NULL OR ",
            $e,
            ".received_at < bp.ended_at))"
        )
    };
}
pub(crate) use hidden_by_block_period;

/// One person the reader blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRecord {
    /// The blocked person's 32-byte pubkey.
    pub blocked: Vec<u8>,
    /// When the block was made.
    pub created_at: DateTime<Utc>,
}

/// Result of [`Db::block_user`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    /// A new block row was stored.
    Added,
    /// The block already existed (idempotent repeat).
    AlreadyBlocked,
    /// The blocker already has [`MAX_BLOCKS_PER_USER`] blocks.
    LimitReached,
}

fn check_key(name: &str, key: &[u8]) -> Result<()> {
    if key.len() != 32 {
        return Err(DbError::InvalidData(format!("{name} must be 32 bytes")));
    }
    Ok(())
}

fn pubkeys(rows: Vec<sqlx::postgres::PgRow>, column: &str) -> Result<Vec<Vec<u8>>> {
    rows.into_iter()
        .map(|row| row.try_get::<Vec<u8>, _>(column).map_err(Into::into))
        .collect()
}

/// Serialize one blocker's block changes (the cap, and one open period per
/// pair).
async fn lock_blocker(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    community: CommunityId,
    blocker: &[u8],
) -> Result<()> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended($1::text || encode($2, 'hex'), 71))",
    )
    .bind(community.as_uuid())
    .bind(blocker)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Open a block period starting now unless one is already open. Uses the
/// relay's clock, like events.received_at (set by the relay on write).
async fn open_period(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    community: CommunityId,
    blocker: &[u8],
    blocked: &[u8],
) -> Result<()> {
    sqlx::query(
        "INSERT INTO user_block_periods (community_id, blocker, blocked, started_at) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (community_id, blocker, blocked) WHERE ended_at IS NULL DO NOTHING",
    )
    .bind(community.as_uuid())
    .bind(blocker)
    .bind(blocked)
    .bind(Utc::now())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

impl Db {
    /// Block `blocked` for `blocker`. Idempotent; refuses self-blocks.
    #[datastore_span(name = "block_user", system = "postgresql")]
    pub async fn block_user(
        &self,
        community: CommunityId,
        blocker: &[u8],
        blocked: &[u8],
    ) -> Result<BlockOutcome> {
        check_key("blocker", blocker)?;
        check_key("blocked", blocked)?;
        if blocker == blocked {
            return Err(DbError::InvalidData("cannot block yourself".into()));
        }
        let mut tx = self.pool.begin().await?;
        lock_blocker(&mut tx, community, blocker).await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_blocks \
              WHERE community_id = $1 AND blocker = $2 AND blocked = $3)",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .bind(blocked)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            // Heal a block row without an open period (rows written before
            // migration 0073 are backfilled; this covers anything else).
            open_period(&mut tx, community, blocker, blocked).await?;
            tx.commit().await?;
            return Ok(BlockOutcome::AlreadyBlocked);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM user_blocks WHERE community_id = $1 AND blocker = $2",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .fetch_one(&mut *tx)
        .await?;
        if count >= MAX_BLOCKS_PER_USER {
            tx.commit().await?;
            return Ok(BlockOutcome::LimitReached);
        }
        // The period first: the user_blocks trigger recounts the blocker's
        // DM rows and should see it.
        open_period(&mut tx, community, blocker, blocked).await?;
        sqlx::query(
            "INSERT INTO user_blocks (community_id, blocker, blocked) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .bind(blocked)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(BlockOutcome::Added)
    }

    /// Remove a block and close its period. Returns whether a block row
    /// existed. Messages received while it was on stay hidden.
    #[datastore_span(name = "unblock_user", system = "postgresql")]
    pub async fn unblock_user(
        &self,
        community: CommunityId,
        blocker: &[u8],
        blocked: &[u8],
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        lock_blocker(&mut tx, community, blocker).await?;
        // Closed before the block row goes, so the user_blocks trigger's
        // recount sees the final period.
        sqlx::query(
            "UPDATE user_block_periods SET ended_at = GREATEST($4, started_at) \
              WHERE community_id = $1 AND blocker = $2 AND blocked = $3 AND ended_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .bind(blocked)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query(
            "DELETE FROM user_blocks WHERE community_id = $1 AND blocker = $2 AND blocked = $3",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .bind(blocked)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    /// Everyone `reader` blocked at any time (open or closed periods): the
    /// authors whose stored 1:1 DM messages may be hidden from `reader`.
    #[datastore_span(name = "period_blocked_by", system = "postgresql")]
    pub async fn period_blocked_by(
        &self,
        community: CommunityId,
        reader: &[u8],
    ) -> Result<Vec<Vec<u8>>> {
        let rows = sqlx::query(
            "SELECT DISTINCT blocked FROM user_block_periods \
              WHERE community_id = $1 AND blocker = $2",
        )
        .bind(community.as_uuid())
        .bind(reader)
        .fetch_all(&self.pool)
        .await?;
        pubkeys(rows, "blocked")
    }

    /// Which of `event_ids` the server received from someone while `reader`
    /// blocked them. Callers pass only events of 1:1 DMs.
    #[datastore_span(name = "block_hidden_event_ids", system = "postgresql")]
    pub async fn block_hidden_event_ids(
        &self,
        community: CommunityId,
        reader: &[u8],
        event_ids: &[Vec<u8>],
    ) -> Result<Vec<Vec<u8>>> {
        if event_ids.is_empty() {
            return Ok(Vec::new());
        }
        if event_ids.len() > MAX_HIDDEN_EVENT_LOOKUP {
            return Err(DbError::InvalidData(format!(
                "at most {MAX_HIDDEN_EVENT_LOOKUP} events per block lookup"
            )));
        }
        let rows = sqlx::query(concat!(
            "SELECT DISTINCT e.id FROM events e \
              WHERE e.community_id = $1 AND e.id = ANY($3) AND ",
            hidden_by_block_period!("$1", "$2", "e")
        ))
        .bind(community.as_uuid())
        .bind(reader)
        .bind(event_ids)
        .fetch_all(&self.pool)
        .await?;
        pubkeys(rows, "id")
    }

    /// The blocker's own block list, newest first.
    #[datastore_span(name = "list_blocks", system = "postgresql")]
    pub async fn list_blocks(
        &self,
        community: CommunityId,
        blocker: &[u8],
    ) -> Result<Vec<BlockRecord>> {
        let rows = sqlx::query(
            "SELECT blocked, created_at FROM user_blocks \
              WHERE community_id = $1 AND blocker = $2 \
              ORDER BY created_at DESC, blocked LIMIT $3",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .bind(MAX_BLOCKS_PER_USER)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(BlockRecord {
                    blocked: row.try_get("blocked")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    /// Everyone who blocks `author` now: the recipients its live 1:1 DM
    /// events and pushes skip.
    #[datastore_span(name = "blockers_of", system = "postgresql")]
    pub async fn blockers_of(&self, community: CommunityId, author: &[u8]) -> Result<Vec<Vec<u8>>> {
        let rows =
            sqlx::query("SELECT blocker FROM user_blocks WHERE community_id = $1 AND blocked = $2")
                .bind(community.as_uuid())
                .bind(author)
                .fetch_all(&self.pool)
                .await?;
        pubkeys(rows, "blocker")
    }

    /// Everyone `reader` blocks now.
    #[datastore_span(name = "blocked_by", system = "postgresql")]
    pub async fn blocked_by(&self, community: CommunityId, reader: &[u8]) -> Result<Vec<Vec<u8>>> {
        let rows = sqlx::query(
            "SELECT blocked FROM user_blocks WHERE community_id = $1 AND blocker = $2 LIMIT $3",
        )
        .bind(community.as_uuid())
        .bind(reader)
        .bind(MAX_BLOCKS_PER_USER)
        .fetch_all(&self.pool)
        .await?;
        pubkeys(rows, "blocked")
    }

    /// `(blocker, blocked)` pairs among the given blockers and blocked authors.
    #[datastore_span(name = "block_pairs", system = "postgresql")]
    pub async fn block_pairs(
        &self,
        community: CommunityId,
        blockers: &[Vec<u8>],
        blocked: &[Vec<u8>],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if blockers.is_empty() || blocked.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT blocker, blocked FROM user_blocks \
              WHERE community_id = $1 AND blocked = ANY($2) AND blocker = ANY($3)",
        )
        .bind(community.as_uuid())
        .bind(blocked)
        .bind(blockers)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| Ok((row.try_get("blocker")?, row.try_get("blocked")?)))
            .collect()
    }

    /// Which of `channels` are DMs between exactly two people: the only
    /// channels the server filters by block. Neither the channel type nor a
    /// DM's participant set ever changes.
    #[datastore_span(name = "one_to_one_dm_ids", system = "postgresql")]
    pub async fn one_to_one_dm_ids(
        &self,
        community: CommunityId,
        channels: &[Uuid],
    ) -> Result<Vec<Uuid>> {
        if channels.is_empty() {
            return Ok(Vec::new());
        }
        if channels.len() > MAX_DM_CHANNEL_LOOKUP {
            return Err(DbError::InvalidData(format!(
                "at most {MAX_DM_CHANNEL_LOOKUP} channels per DM lookup"
            )));
        }
        let rows = sqlx::query(
            "SELECT id FROM channels \
              WHERE community_id = $1 AND id = ANY($2) AND dm_is_one_to_one(community_id, id)",
        )
        .bind(community.as_uuid())
        .bind(channels)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| row.try_get::<Uuid, _>("id").map_err(Into::into))
            .collect()
    }
}
