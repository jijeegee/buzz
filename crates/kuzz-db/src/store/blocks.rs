//! Silent user blocks (`user_blocks`).
//!
//! A row means `blocker` no longer receives `blocked`'s DM events. The rows
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

/// Most channel ids one [`Db::dm_channel_ids`] call may name.
pub const MAX_DM_CHANNEL_LOOKUP: usize = 1000;

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
        // Serialize one blocker's inserts so the cap holds under concurrency.
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended($1::text || encode($2, 'hex'), 71))",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .execute(&mut *tx)
        .await?;
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

    /// Remove a block. Returns whether a row existed.
    #[datastore_span(name = "unblock_user", system = "postgresql")]
    pub async fn unblock_user(
        &self,
        community: CommunityId,
        blocker: &[u8],
        blocked: &[u8],
    ) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM user_blocks WHERE community_id = $1 AND blocker = $2 AND blocked = $3",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .bind(blocked)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
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

    /// Everyone who blocked `author`: the recipients its DM events skip.
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

    /// Everyone `reader` blocked: the authors whose DM events `reader` skips.
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

    /// Which of `channels` are DMs. Channel type never changes after creation.
    #[datastore_span(name = "dm_channel_ids", system = "postgresql")]
    pub async fn dm_channel_ids(
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
              WHERE community_id = $1 AND id = ANY($2) AND channel_type = 'dm'",
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
