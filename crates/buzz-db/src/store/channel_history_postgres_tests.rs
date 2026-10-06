//! Tests bind the real channel/member/event writers and database ordering guard.
use buzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::{channel, channel_members, event};

struct Fixture {
    pool: PgPool,
    scope: CommunityId,
    channel: Uuid,
    owner: Keys,
    member: Keys,
}

impl Fixture {
    async fn new() -> Self {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("isolated test database");
        if std::env::var("BUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        let scope = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(scope.as_uuid())
            .bind(format!("history-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        let owner = Keys::generate();
        let channel = channel::create_channel(
            &pool,
            scope,
            "history",
            ChannelType::Stream,
            ChannelVisibility::Open,
            None,
            &owner.public_key().to_bytes(),
            None,
        )
        .await
        .expect("real channel creation")
        .id;
        Self {
            pool,
            scope,
            channel,
            owner,
            member: Keys::generate(),
        }
    }

    fn message(&self, content: &str, timestamp: u64) -> Event {
        EventBuilder::new(Kind::Custom(9), content)
            .tags([Tag::parse(["h", &self.channel.to_string()]).expect("h tag")])
            .custom_created_at(Timestamp::from(timestamp))
            .sign_with_keys(&self.owner)
            .expect("fixture signature")
    }

    async fn send(&self, content: &str, timestamp: u64) -> i64 {
        let message = self.message(content, timestamp);
        event::insert_event(&self.pool, self.scope, &message, Some(self.channel))
            .await
            .expect("real event writer");
        sqlx::query_scalar("SELECT channel_sequence FROM events WHERE community_id=$1 AND id=$2")
            .bind(self.scope.as_uuid())
            .bind(message.id.to_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .expect("stored sequence")
    }

    async fn join(&self) {
        channel_members::add_member(
            &self.pool,
            self.scope,
            self.channel,
            &self.member.public_key().to_bytes(),
            MemberRole::Member,
            None,
        )
        .await
        .expect("real member writer");
    }

    async fn cutoff(&self) -> (i64, chrono::DateTime<chrono::Utc>, bool) {
        sqlx::query_as("SELECT join_sequence, joined_at, join_cutoff_is_legacy FROM channel_members WHERE community_id=$1 AND channel_id=$2 AND pubkey=$3")
            .bind(self.scope.as_uuid()).bind(self.channel).bind(self.member.public_key().to_bytes().as_slice())
            .fetch_one(&self.pool).await.expect("current cutoff")
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn acceptance_order_ignores_sender_time_and_rejoin_resets_only_once() {
    let f = Fixture::new().await;
    let before = f
        .send("before join with future timestamp", 2_000_000_000)
        .await;
    f.join().await;
    let initial = f.cutoff().await;
    assert_eq!(initial.0, before);
    assert!(!initial.2);
    let after = f.send("after join with old timestamp", 1).await;
    assert!(after > initial.0);
    f.join().await;
    assert_eq!(f.cutoff().await, initial, "duplicate cannot advance cutoff");
    channel_members::remove_member(
        &f.pool,
        f.scope,
        f.channel,
        &f.member.public_key().to_bytes(),
        &f.owner.public_key().to_bytes(),
    )
    .await
    .expect("real kick writer");
    let absent = f.send("while absent", 2).await;
    f.join().await;
    let rejoined = f.cutoff().await;
    assert_eq!(rejoined.0, absent);
    assert!(rejoined.1 > initial.1);
    assert!(f.send("current period", 3).await > rejoined.0);
    f.join().await;
    assert_eq!(f.cutoff().await, rejoined);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn automatic_membership_uses_the_same_rejoin_boundary() {
    let f = Fixture::new().await;
    f.join().await;
    let original = f.cutoff().await;
    channel_members::remove_member(
        &f.pool,
        f.scope,
        f.channel,
        &f.member.public_key().to_bytes(),
        &f.owner.public_key().to_bytes(),
    )
    .await
    .expect("remove");
    let seq = f.send("before automatic rejoin", 1).await;
    let mut tx = f.pool.begin().await.expect("transaction");
    channel_members::acquire_channel_membership_lock_in_transaction(&mut tx, f.scope, f.channel)
        .await
        .expect("membership lock");
    channel_members::insert_auto_membership_in_transaction(
        &mut tx,
        f.scope,
        f.channel,
        &f.member.public_key().to_bytes(),
        &f.owner.public_key().to_bytes(),
    )
    .await
    .expect("real automatic membership");
    tx.commit().await.expect("commit");
    let current = f.cutoff().await;
    assert_eq!(current.0, seq);
    assert!(current.1 > original.1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn in_flight_event_holds_membership_order_until_commit() {
    let f = Fixture::new().await;
    let mut tx = f.pool.begin().await.expect("transaction");
    let message = f.message("uncommitted message", 1);
    event::insert_event_in_transaction(&mut tx, f.scope, &message, Some(f.channel))
        .await
        .expect("real transactional insert");
    // An independent connection cannot take the membership lock until the event
    // transaction ends. Removing the event trigger makes this assertion fail.
    let acquired: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("buzz_channel_membership:{}:{}", f.scope, f.channel))
            .fetch_one(&f.pool)
            .await
            .expect("independent lock probe");
    assert!(
        !acquired,
        "event and membership must share transaction ordering"
    );
    tx.commit().await.expect("commit accepted event");
    f.join().await;
    assert_eq!(f.cutoff().await.0, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn migration_schema_history_backfill_preserves_old_rooms() {
    let pool = PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("isolated empty migration database");
    crate::migration::run_migrations_through(&pool, 59)
        .await
        .expect("legacy migrations");
    let scope = CommunityId::from_uuid(Uuid::new_v4());
    sqlx::query("INSERT INTO communities(id,host) VALUES($1,$2)")
        .bind(scope.as_uuid())
        .bind(format!("legacy-history-{}.test", scope))
        .execute(&pool)
        .await
        .expect("legacy community");
    let owner = Keys::generate();
    let channel = channel::create_channel(
        &pool,
        scope,
        "legacy",
        ChannelType::Stream,
        ChannelVisibility::Open,
        None,
        &owner.public_key().to_bytes(),
        None,
    )
    .await
    .expect("legacy channel")
    .id;
    let f = Fixture {
        pool,
        scope,
        channel,
        owner,
        member: Keys::generate(),
    };
    // The earlier received event intentionally has the later signed timestamp.
    for (content, timestamp) in [("received first", 2_000_000_000), ("received second", 1)] {
        let message = f.message(content, timestamp);
        event::insert_event(&f.pool, f.scope, &message, Some(f.channel))
            .await
            .expect("legacy event insert");
    }
    f.join().await;
    crate::migration::run_migrations(&f.pool)
        .await
        .expect("additive history migration");
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT content, channel_sequence FROM events WHERE community_id=$1 AND channel_id=$2 ORDER BY channel_sequence")
        .bind(scope.as_uuid()).bind(channel).fetch_all(&f.pool).await.expect("backfill");
    assert_eq!(
        rows,
        vec![("received first".into(), 1), ("received second".into(), 2)]
    );
    let policy: String =
        sqlx::query_scalar("SELECT history_policy FROM channels WHERE community_id=$1 AND id=$2")
            .bind(scope.as_uuid())
            .bind(channel)
            .fetch_one(&f.pool)
            .await
            .expect("old policy");
    assert_eq!(policy, "all");
    let legacy = f.cutoff().await;
    assert_eq!(legacy.0, 2);
    assert!(legacy.2, "do not invent lost current-join evidence");
    f.join().await;
    assert_eq!(
        f.cutoff().await,
        legacy,
        "duplicate preserves legacy warning"
    );
    assert_eq!(f.send("accepted after migration", 2).await, 3);
    let new_channel = channel::create_channel(
        &f.pool,
        scope,
        "new",
        ChannelType::Stream,
        ChannelVisibility::Open,
        None,
        &f.owner.public_key().to_bytes(),
        None,
    )
    .await
    .expect("new channel");
    let new_policy: String =
        sqlx::query_scalar("SELECT history_policy FROM channels WHERE community_id=$1 AND id=$2")
            .bind(scope.as_uuid())
            .bind(new_channel.id)
            .fetch_one(&f.pool)
            .await
            .expect("new policy");
    assert_eq!(new_policy, "since_join");
}
