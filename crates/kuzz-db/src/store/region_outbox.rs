//! Region propagation outbox (migration 0085, global regions stage S4-5).
//!
//! A row asks the worker to deliver one command (`topic` + `payload`) to one
//! other region over the region link. Commands are idempotent, so a row is
//! claimed with a lease, deleted once delivered, and retried with backoff
//! otherwise.

use chrono::{DateTime, Utc};
use kuzz_datastore_tracing::datastore_span;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::error::Result;
use crate::Db;

/// Longest wait between two attempts of one row, in seconds.
const MAX_BACKOFF_SECS: i32 = 60;

/// One claimed outbox row.
#[derive(Debug, Clone)]
pub struct RegionOutboxRow {
    /// Row id.
    pub id: i64,
    /// Region to deliver to.
    pub target_region: String,
    /// Community named in the link call.
    pub community_id: Uuid,
    /// What to apply there.
    pub topic: String,
    /// The command.
    pub payload: Value,
    /// Attempts so far, this one included.
    pub attempt_count: i32,
    /// When the row was written.
    pub created_at: DateTime<Utc>,
    /// Fences this claim: finishing or retrying needs it.
    pub claim_token: Uuid,
}

impl Db {
    /// Queue `topic` + `payload` for every other registered region. Returns
    /// how many rows were written (0 with a single region). A region added
    /// later gets no rows: it had no connections when this happened.
    #[datastore_span(name = "enqueue_region_outbox", system = "postgresql")]
    pub async fn enqueue_region_outbox(
        &self,
        local_region: &str,
        community_id: Uuid,
        topic: &str,
        payload: &Value,
    ) -> Result<u64> {
        let done = sqlx::query(
            "INSERT INTO region_outbox (target_region, community_id, topic, payload) \
             SELECT id, $2, $3, $4 FROM regions WHERE id <> $1",
        )
        .bind(local_region)
        .bind(community_id)
        .bind(topic)
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }

    /// Claim up to `limit` due rows whose lease is unset or expired, oldest
    /// first. Several pods may run the worker; `SKIP LOCKED` keeps them apart.
    #[datastore_span(name = "claim_region_outbox", system = "postgresql")]
    pub async fn claim_region_outbox(
        &self,
        worker_id: &str,
        lease_until: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<RegionOutboxRow>> {
        let rows = sqlx::query(
            "UPDATE region_outbox \
                SET held_by = $1, lease_expires_at = $2, claim_token = $4, \
                    attempt_count = attempt_count + 1 \
              WHERE id IN ( \
                    SELECT id FROM region_outbox \
                     WHERE (retry_after IS NULL OR retry_after <= now()) \
                       AND (lease_expires_at IS NULL OR lease_expires_at < now()) \
                     ORDER BY retry_after NULLS FIRST, id \
                     LIMIT $3 \
                       FOR UPDATE SKIP LOCKED) \
             RETURNING id, target_region, community_id, topic, payload, attempt_count, \
                       created_at, claim_token",
        )
        .bind(worker_id)
        .bind(lease_until)
        .bind(limit)
        .bind(Uuid::new_v4())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(RegionOutboxRow {
                    id: row.try_get("id")?,
                    target_region: row.try_get("target_region")?,
                    community_id: row.try_get("community_id")?,
                    topic: row.try_get("topic")?,
                    payload: row.try_get("payload")?,
                    attempt_count: row.try_get("attempt_count")?,
                    created_at: row.try_get("created_at")?,
                    claim_token: row.try_get("claim_token")?,
                })
            })
            .collect()
    }

    /// Delete a delivered (or abandoned) row. False when the claim was lost.
    #[datastore_span(name = "finish_region_outbox", system = "postgresql")]
    pub async fn finish_region_outbox(&self, id: i64, claim_token: Uuid) -> Result<bool> {
        let done = sqlx::query("DELETE FROM region_outbox WHERE id = $1 AND claim_token = $2")
            .bind(id)
            .bind(claim_token)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Release a failed row for another attempt after `2^attempts` seconds
    /// (at most a minute). False when the claim was lost.
    #[datastore_span(name = "retry_region_outbox", system = "postgresql")]
    pub async fn retry_region_outbox(
        &self,
        id: i64,
        claim_token: Uuid,
        error: &str,
    ) -> Result<bool> {
        let done = sqlx::query(
            "UPDATE region_outbox \
                SET held_by = NULL, lease_expires_at = NULL, claim_token = NULL, \
                    last_error = left($3, 500), \
                    retry_after = now() + make_interval( \
                        secs => LEAST(power(2, LEAST(attempt_count, 10))::int, $4)) \
              WHERE id = $1 AND claim_token = $2",
        )
        .bind(id)
        .bind(claim_token)
        .bind(error)
        .bind(MAX_BACKOFF_SECS)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sqlx::PgPool;

    async fn db() -> Db {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("isolated test database");
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        Db::from_pool(pool)
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn rows_go_to_every_other_region_and_retry_until_finished() {
        let db = db().await;
        // Own region ids and community: other tests clear `regions`.
        let tag = &Uuid::new_v4().simple().to_string()[..8];
        let ids = ["a", "b", "c"].map(|r| format!("q{tag}{r}"));
        for id in &ids {
            sqlx::query("INSERT INTO regions (id, connect_url) VALUES ($1, 'wss://x')")
                .bind(id)
                .execute(&db.pool)
                .await
                .expect("region");
        }
        let community = Uuid::new_v4();
        db.enqueue_region_outbox(&ids[0], community, "t", &json!({"a": 1}))
            .await
            .expect("enqueue");
        let mine = |rows: Vec<RegionOutboxRow>| -> Vec<RegionOutboxRow> {
            rows.into_iter()
                .filter(|row| row.community_id == community)
                .collect()
        };

        let lease = Utc::now() + chrono::Duration::seconds(30);
        let claimed = mine(
            db.claim_region_outbox("w1", lease, 1000)
                .await
                .expect("claim"),
        );
        let mut targets: Vec<_> = claimed
            .iter()
            .map(|row| row.target_region.clone())
            .collect();
        targets.retain(|target| ids.contains(target));
        targets.sort();
        assert_eq!(
            targets,
            vec![ids[1].clone(), ids[2].clone()],
            "every other region, not this one"
        );
        assert!(claimed
            .iter()
            .all(|row| row.attempt_count == 1 && row.payload == json!({"a": 1})));
        assert!(
            mine(
                db.claim_region_outbox("w2", lease, 1000)
                    .await
                    .expect("claim")
            )
            .is_empty(),
            "leased rows are not claimed twice"
        );

        let first = &claimed[0];
        assert!(db
            .retry_region_outbox(first.id, first.claim_token, "down")
            .await
            .expect("retry"));
        assert!(
            !db.finish_region_outbox(first.id, first.claim_token)
                .await
                .expect("finish"),
            "a released claim cannot finish"
        );
        assert!(
            mine(
                db.claim_region_outbox("w3", lease, 1000)
                    .await
                    .expect("claim")
            )
            .is_empty(),
            "a retried row waits for its backoff"
        );
        for row in &claimed[1..] {
            assert!(db
                .finish_region_outbox(row.id, row.claim_token)
                .await
                .expect("finish"));
        }
        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM region_outbox WHERE community_id = $1")
                .bind(community)
                .fetch_one(&db.pool)
                .await
                .expect("count");
        assert_eq!(left, 1);
        sqlx::query("DELETE FROM region_outbox WHERE community_id = $1")
            .bind(community)
            .execute(&db.pool)
            .await
            .expect("clean");
        sqlx::query("DELETE FROM regions WHERE id = ANY($1)")
            .bind(&ids[..])
            .execute(&db.pool)
            .await
            .expect("clean");
    }
}
