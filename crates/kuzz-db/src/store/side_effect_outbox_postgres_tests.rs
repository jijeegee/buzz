//! Side-effect outbox (migration 0077) against a real database.
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{event, Db};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
    keys: Keys,
}

impl Fixture {
    async fn new() -> Self {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("isolated test database");
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        let scope = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(scope.as_uuid())
            .bind(format!("outbox-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()),
            pool,
            scope,
            keys: Keys::generate(),
        }
    }

    async fn store(&self, kind: u16, content: &str) -> (Event, bool) {
        let event = EventBuilder::new(Kind::Custom(kind), content)
            .tags([Tag::parse(["e", &"ab".repeat(32)]).expect("e tag")])
            .sign_with_keys(&self.keys)
            .expect("fixture signature");
        let (_, inserted) = event::insert_event(&self.pool, self.scope, &event, None)
            .await
            .expect("event insert");
        (event, inserted)
    }

    async fn row(&self, event: &Event) -> Option<(i32, i32, Option<String>)> {
        sqlx::query(
            "SELECT kind, attempts, last_error FROM event_side_effects \
             WHERE community_id = $1 AND event_id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(event.id.to_bytes().to_vec())
        .fetch_optional(&self.pool)
        .await
        .expect("outbox row")
        .map(|r| (r.get("kind"), r.get("attempts"), r.get("last_error")))
    }

    async fn expire_lease(&self, event: &Event) {
        sqlx::query(
            "UPDATE event_side_effects SET lease_until = now() - interval '1 second' \
             WHERE community_id = $1 AND event_id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(event.id.to_bytes().to_vec())
        .execute(&self.pool)
        .await
        .expect("expire lease");
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn storing_a_side_effect_kind_writes_one_pending_row() {
    let fx = Fixture::new().await;
    let (deletion, inserted) = fx.store(5, "").await;
    assert!(inserted);
    assert_eq!(fx.row(&deletion).await, Some((5, 0, None)));

    // A duplicate insert neither adds a row nor resets the existing one.
    fx.db
        .record_side_effect_failure(fx.scope, &deletion.id.to_bytes(), "boom")
        .await
        .expect("failure");
    let (_, inserted) = event::insert_event(&fx.pool, fx.scope, &deletion, None)
        .await
        .expect("duplicate insert");
    assert!(!inserted);
    assert_eq!(fx.row(&deletion).await, Some((5, 0, Some("boom".into()))));

    // Ordinary messages have no side effects and no row.
    let (message, _) = fx.store(9, "hello").await;
    assert_eq!(fx.row(&message).await, None);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_resend_claims_only_a_failed_or_expired_row() {
    let fx = Fixture::new().await;
    let (deletion, _) = fx.store(5, "").await;
    let id = deletion.id.to_bytes();

    // The first run still owns the fresh row.
    assert!(!fx.db.claim_side_effect(fx.scope, &id).await.expect("claim"));

    fx.db
        .record_side_effect_failure(fx.scope, &id, "transient")
        .await
        .expect("failure");
    assert!(fx.db.claim_side_effect(fx.scope, &id).await.expect("claim"));
    assert_eq!(fx.row(&deletion).await.map(|r| r.1), Some(1));

    fx.db
        .complete_side_effect(fx.scope, &id)
        .await
        .expect("complete");
    assert_eq!(fx.row(&deletion).await, None);
    assert!(!fx.db.claim_side_effect(fx.scope, &id).await.expect("claim"));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn the_worker_claims_expired_rows_of_listed_kinds_once() {
    let fx = Fixture::new().await;
    let (deletion, _) = fx.store(5, "").await;
    let (put_user, _) = fx.store(9000, "").await;
    fx.expire_lease(&deletion).await;
    fx.expire_lease(&put_user).await;

    let mine = |claimed: Vec<crate::side_effect_outbox::ClaimedSideEffect>| {
        claimed
            .into_iter()
            .filter(|c| c.community_id == fx.scope)
            .map(|c| (c.event_id, c.kind, c.attempts))
            .collect::<Vec<_>>()
    };
    let claimed = mine(
        fx.db
            .claim_due_side_effects(&[5], 1000)
            .await
            .expect("claim"),
    );
    assert_eq!(claimed, vec![(deletion.id.to_bytes().to_vec(), 5, 1)]);

    // Claimed rows are leased; kind 9000 was never listed.
    let again = mine(
        fx.db
            .claim_due_side_effects(&[5], 1000)
            .await
            .expect("claim"),
    );
    assert!(again.is_empty());
    assert_eq!(fx.row(&put_user).await.map(|r| r.1), Some(0));
}
