//! Room history cut (`channels.history_policy = 'since_join'`).
//!
//! In a members-only room with `since_join`, a reader sees only channel
//! events after their current join (`channel_members.join_sequence`). Open
//! rooms and legacy-stamped memberships keep full history. The rule lives in
//! SQL (`channel_history_floor`, `channel_event_before_join`, migration
//! 0076); these lookups serve read paths that cannot fold it into their own
//! query (lookups by id, stored summary counters).

use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::Db;

/// Most event ids one [`Db::history_hidden_event_ids`] call may name.
pub const MAX_HISTORY_EVENT_LOOKUP: usize = 5000;

impl Db {
    /// The channel sequence at or below which `reader` sees nothing in
    /// `channel`; `0` when the whole history is visible.
    #[datastore_span(name = "channel_history_floor", system = "postgresql")]
    pub async fn channel_history_floor(
        &self,
        community: CommunityId,
        channel: Uuid,
        reader: &[u8],
    ) -> Result<i64> {
        let floor: i64 = sqlx::query_scalar("SELECT channel_history_floor($1, $2, $3)")
            .bind(community.as_uuid())
            .bind(channel)
            .bind(reader)
            .fetch_one(&self.pool)
            .await?;
        Ok(floor)
    }

    /// Which of `event_ids` were written before `reader` joined their room
    /// (and so are "not found" to `reader`).
    #[datastore_span(name = "history_hidden_event_ids", system = "postgresql")]
    pub async fn history_hidden_event_ids(
        &self,
        community: CommunityId,
        reader: &[u8],
        event_ids: &[Vec<u8>],
    ) -> Result<Vec<Vec<u8>>> {
        if event_ids.is_empty() {
            return Ok(Vec::new());
        }
        if event_ids.len() > MAX_HISTORY_EVENT_LOOKUP {
            return Err(DbError::InvalidData(format!(
                "at most {MAX_HISTORY_EVENT_LOOKUP} events per history lookup"
            )));
        }
        let rows = sqlx::query(
            "SELECT DISTINCT e.id FROM events e \
              WHERE e.community_id = $1 AND e.id = ANY($3) \
                AND channel_event_before_join(e.community_id, e.channel_id, e.channel_sequence, $2)",
        )
        .bind(community.as_uuid())
        .bind(reader)
        .bind(event_ids)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| row.try_get::<Vec<u8>, _>("id").map_err(Into::into))
            .collect()
    }

    /// Of `readers`, those for whom any of `targets` that lives in `channel`
    /// was written before their join. Live delivery of a reaction,
    /// edit, deletion or summary uses it to skip readers who cannot see what
    /// it is about. Targets outside `channel` are ignored; none in it hides
    /// nothing.
    #[datastore_span(name = "readers_blind_to_targets", system = "postgresql")]
    pub async fn readers_blind_to_targets(
        &self,
        community: CommunityId,
        channel: Uuid,
        targets: &[Vec<u8>],
        readers: &[Vec<u8>],
    ) -> Result<Vec<Vec<u8>>> {
        if targets.is_empty() || readers.is_empty() {
            return Ok(Vec::new());
        }
        if targets.len() > MAX_HISTORY_EVENT_LOOKUP || readers.len() > MAX_HISTORY_EVENT_LOOKUP {
            return Err(DbError::InvalidData(format!(
                "at most {MAX_HISTORY_EVENT_LOOKUP} targets and readers per history lookup"
            )));
        }
        let rows = sqlx::query(
            "WITH t AS (SELECT min(e.channel_sequence) AS seq FROM events e \
                         WHERE e.community_id = $1 AND e.channel_id = $2 AND e.id = ANY($3)) \
             SELECT r.pk FROM unnest($4::bytea[]) AS r(pk), t \
              WHERE t.seq IS NOT NULL AND t.seq <= channel_history_floor($1, $2, r.pk)",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .bind(targets)
        .bind(readers)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| row.try_get::<Vec<u8>, _>("pk").map_err(Into::into))
            .collect()
    }
}
