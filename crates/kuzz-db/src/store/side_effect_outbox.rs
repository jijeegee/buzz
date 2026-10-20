//! Side-effect outbox (migration 0077).
//!
//! The `enqueue_event_side_effect` trigger writes a pending row in the same
//! transaction as a stored event whose kind has side effects. The relay runs
//! the side effects after commit and calls [`Db::complete_side_effect`]; a row
//! left behind is retried once its lease expires.

use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;

use crate::error::Result;
use crate::Db;

/// How long a claim owns a row before another relay may retry it.
pub const SIDE_EFFECT_LEASE_SECS: i64 = 60;

/// Attempts after which a row stays for an operator instead of retrying.
pub const SIDE_EFFECT_MAX_ATTEMPTS: i32 = 10;

/// A pending side effect claimed by the retry worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedSideEffect {
    /// Community the event was stored in.
    pub community_id: CommunityId,
    /// Raw event id.
    pub event_id: Vec<u8>,
    /// Event kind.
    pub kind: i32,
    /// Attempts including this claim.
    pub attempts: i32,
}

impl Db {
    /// The side effects of this event ran: drop its outbox row.
    #[datastore_span(name = "complete_side_effect", system = "postgresql")]
    pub async fn complete_side_effect(
        &self,
        community_id: CommunityId,
        event_id: &[u8],
    ) -> Result<()> {
        sqlx::query("DELETE FROM event_side_effects WHERE community_id = $1 AND event_id = $2")
            .bind(community_id.as_uuid())
            .bind(event_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The side effects failed: keep the row and retry after a backoff of
    /// 30 s doubling per attempt, at most an hour.
    #[datastore_span(name = "record_side_effect_failure", system = "postgresql")]
    pub async fn record_side_effect_failure(
        &self,
        community_id: CommunityId,
        event_id: &[u8],
        error: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE event_side_effects
            SET last_error = left($3, 2000),
                lease_until = now() + LEAST(
                    interval '1 hour',
                    interval '30 seconds' * power(2, LEAST(GREATEST(attempts - 1, 0), 7)))
            WHERE community_id = $1 AND event_id = $2
            "#,
        )
        .bind(community_id.as_uuid())
        .bind(event_id)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Claim one event's pending side effects for a duplicate re-send.
    ///
    /// Wins only when no run currently owns the row: its lease expired or its
    /// last run already failed. Returns whether this caller should run them.
    #[datastore_span(name = "claim_side_effect", system = "postgresql")]
    pub async fn claim_side_effect(
        &self,
        community_id: CommunityId,
        event_id: &[u8],
    ) -> Result<bool> {
        let claimed = sqlx::query(
            r#"
            UPDATE event_side_effects
            SET lease_until = now() + make_interval(secs => $3),
                attempts = attempts + 1
            WHERE community_id = $1 AND event_id = $2
              AND attempts < $4
              AND (lease_until <= now() OR last_error IS NOT NULL)
            "#,
        )
        .bind(community_id.as_uuid())
        .bind(event_id)
        .bind(SIDE_EFFECT_LEASE_SECS as f64)
        .bind(SIDE_EFFECT_MAX_ATTEMPTS)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(claimed == 1)
    }

    /// Claim up to `limit` rows of `kinds` whose lease expired, oldest first.
    /// Rows held by another relay's claim are skipped, never waited on.
    #[datastore_span(name = "claim_due_side_effects", system = "postgresql")]
    pub async fn claim_due_side_effects(
        &self,
        kinds: &[i32],
        limit: i64,
    ) -> Result<Vec<ClaimedSideEffect>> {
        let rows = sqlx::query(
            r#"
            UPDATE event_side_effects AS o
            SET lease_until = now() + make_interval(secs => $2),
                attempts = o.attempts + 1
            FROM (
                SELECT community_id, event_id
                FROM event_side_effects
                WHERE lease_until <= now() AND attempts < $3 AND kind = ANY($4)
                ORDER BY lease_until
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            ) AS due
            WHERE o.community_id = due.community_id AND o.event_id = due.event_id
            RETURNING o.community_id, o.event_id, o.kind, o.attempts
            "#,
        )
        .bind(limit)
        .bind(SIDE_EFFECT_LEASE_SECS as f64)
        .bind(SIDE_EFFECT_MAX_ATTEMPTS)
        .bind(kinds)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| ClaimedSideEffect {
                community_id: CommunityId::from_uuid(row.get("community_id")),
                event_id: row.get("event_id"),
                kind: row.get("kind"),
                attempts: row.get("attempts"),
            })
            .collect())
    }

    /// Rows still pending, for the backlog metric.
    #[datastore_span(name = "count_pending_side_effects", system = "postgresql")]
    pub async fn count_pending_side_effects(&self) -> Result<i64> {
        let row = sqlx::query("SELECT count(*) AS n FROM event_side_effects")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get("n"))
    }
}
