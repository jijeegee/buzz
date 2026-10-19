//! Tests bind the real channel/member/event writers and database ordering guard.
use kuzz_core::CommunityId;
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
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
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
            .bind(format!("kuzz_channel_membership:{}:{}", f.scope, f.channel))
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

#[tokio::test]
#[ignore = "requires Postgres"]
async fn insert_returns_sequence_and_duplicates_do_not_burn_numbers() {
    let f = Fixture::new().await;
    let accepted = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT accepted_sequence FROM channels WHERE community_id=$1 AND id=$2",
        )
        .bind(f.scope.as_uuid())
        .bind(f.channel)
        .fetch_one(&f.pool)
        .await
        .expect("accepted sequence")
    };

    let first = f.message("first", 1_700_000_000);
    let (stored, inserted) = event::insert_event(&f.pool, f.scope, &first, Some(f.channel))
        .await
        .expect("insert");
    assert!(inserted);
    let first_sequence = stored
        .channel_sequence
        .expect("new row returns its sequence");
    assert_eq!(accepted().await, first_sequence);

    // Retry through both writers: same number back, counter unchanged.
    let (retry, inserted) = event::insert_event(&f.pool, f.scope, &first, Some(f.channel))
        .await
        .expect("retry");
    assert!(!inserted);
    assert_eq!(retry.channel_sequence, Some(first_sequence));
    let (retry, inserted) =
        event::insert_event_with_chat_state(&f.pool, f.scope, &first, Some(f.channel), None)
            .await
            .map(|(stored, inserted, _)| (stored, inserted))
            .expect("retry with chat state");
    assert!(!inserted);
    assert_eq!(retry.channel_sequence, Some(first_sequence));
    assert_eq!(accepted().await, first_sequence, "duplicates leave no hole");

    let second = f.message("second", 1_700_000_001);
    let (stored, inserted) =
        event::insert_event_with_chat_state(&f.pool, f.scope, &second, Some(f.channel), None)
            .await
            .map(|(stored, inserted, _)| (stored, inserted))
            .expect("second insert");
    assert!(inserted);
    assert_eq!(stored.channel_sequence, Some(first_sequence + 1));
    assert_eq!(accepted().await, first_sequence + 1);

    // Events without a channel carry no order number.
    let global = EventBuilder::new(Kind::TextNote, "global")
        .sign_with_keys(&f.owner)
        .expect("fixture signature");
    let (stored, _) = event::insert_event(&f.pool, f.scope, &global, None)
        .await
        .expect("global insert");
    assert_eq!(stored.channel_sequence, None);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn query_events_reads_sequence_and_limit_zero_returns_nothing() {
    let f = Fixture::new().await;
    let first = f.send("first", 1_700_000_000).await;
    let second = f.send("second", 1_700_000_001).await;
    let global = EventBuilder::new(Kind::TextNote, "global")
        .sign_with_keys(&f.owner)
        .expect("fixture signature");
    event::insert_event(&f.pool, f.scope, &global, None)
        .await
        .expect("global insert");

    let mut q = event::EventQuery::for_community(f.scope);
    q.channel_id = Some(f.channel);
    let rows = event::query_events(&f.pool, &q)
        .await
        .expect("channel query");
    let sequences: Vec<_> = rows.iter().map(|row| row.channel_sequence).collect();
    assert_eq!(sequences, vec![Some(second), Some(first)]);

    // The #p join path selects the column too.
    let mentioned = Keys::generate();
    let mention = EventBuilder::new(Kind::Custom(9), "hi")
        .tags([
            Tag::parse(["h", &f.channel.to_string()]).expect("h tag"),
            Tag::parse(["p", &mentioned.public_key().to_hex()]).expect("p tag"),
        ])
        .custom_created_at(Timestamp::from(1_700_000_002))
        .sign_with_keys(&f.owner)
        .expect("fixture signature");
    let (stored, _) = event::insert_event(&f.pool, f.scope, &mention, Some(f.channel))
        .await
        .expect("mention insert");
    sqlx::query(
        "INSERT INTO event_mentions \
         (community_id, pubkey_hex, event_id, event_created_at, channel_id, event_kind) \
         VALUES ($1, $2, $3, to_timestamp(1700000002), $4, 9)",
    )
    .bind(f.scope.as_uuid())
    .bind(mentioned.public_key().to_hex())
    .bind(mention.id.as_bytes().as_slice())
    .bind(f.channel)
    .execute(&f.pool)
    .await
    .expect("mention row");
    let mut q = event::EventQuery::for_community(f.scope);
    q.p_tag_hex = Some(mentioned.public_key().to_hex());
    let rows = event::query_events(&f.pool, &q).await.expect("#p query");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].channel_sequence, stored.channel_sequence);
    assert!(rows[0].channel_sequence.is_some());

    let mut q = event::EventQuery::for_community(f.scope);
    q.global_only = true;
    let rows = event::query_events(&f.pool, &q)
        .await
        .expect("global query");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].channel_sequence, None,
        "no channel, no order number"
    );

    // NIP-01 `limit: 0`: no stored rows, so a REQ can be live-only.
    let mut q = event::EventQuery::for_community(f.scope);
    q.limit = Some(0);
    assert!(event::query_events(&f.pool, &q)
        .await
        .expect("limit 0 query")
        .is_empty());
}
