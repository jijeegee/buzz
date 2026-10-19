//! `since_join` history cut through the shared `EventQuery` builders
//! (`history_reader` → `channel_event_before_join`).
use kuzz_core::CommunityId;
use nostr::{EventBuilder, Keys, Kind, Tag};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::{channel, channel_members, event, EventQuery};

struct Fixture {
    pool: PgPool,
    scope: CommunityId,
    channel: Uuid,
    owner: Keys,
    member: Keys,
}

impl Fixture {
    async fn new(visibility: ChannelVisibility) -> Self {
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
            .bind(format!("since-join-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        let owner = Keys::generate();
        let channel = channel::create_channel(
            &pool,
            scope,
            "since-join",
            ChannelType::Stream,
            visibility,
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

    async fn send(&self, content: &str) -> String {
        let message = EventBuilder::new(Kind::Custom(9), content)
            .tags([Tag::parse(["h", &self.channel.to_string()]).expect("h tag")])
            .sign_with_keys(&self.owner)
            .expect("fixture signature");
        event::insert_event(&self.pool, self.scope, &message, Some(self.channel))
            .await
            .expect("real event writer");
        message.id.to_hex()
    }

    async fn join(&self) {
        channel_members::add_member(
            &self.pool,
            self.scope,
            self.channel,
            &self.member.public_key().to_bytes(),
            MemberRole::Member,
            Some(&self.owner.public_key().to_bytes()),
        )
        .await
        .expect("real member writer");
    }

    async fn leave(&self) {
        channel_members::remove_member(
            &self.pool,
            self.scope,
            self.channel,
            &self.member.public_key().to_bytes(),
            &self.owner.public_key().to_bytes(),
        )
        .await
        .expect("real kick writer");
    }

    async fn set(&self, sql: &'static str) {
        sqlx::query(sql)
            .bind(self.scope.as_uuid())
            .bind(self.channel)
            .execute(&self.pool)
            .await
            .expect("fixture update");
    }

    fn query(&self, reader: Option<&Keys>) -> EventQuery {
        let mut q = EventQuery::for_community(self.scope);
        q.channel_id = Some(self.channel);
        q.kinds = Some(vec![9]);
        q.order_by_sequence = true;
        q.history_reader = reader.map(|keys| keys.public_key().to_bytes().to_vec());
        q
    }

    /// Visible ids, oldest first, and the matching COUNT.
    async fn read(&self, reader: Option<&Keys>) -> (Vec<String>, i64) {
        let q = self.query(reader);
        let rows = event::query_events(&self.pool, &q).await.expect("query");
        let count = event::count_events(&self.pool, &q).await.expect("count");
        (rows.into_iter().map(|s| s.event.id.to_hex()).collect(), count)
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn members_only_room_hides_rows_from_before_the_current_join() {
    let f = Fixture::new(ChannelVisibility::Private).await;
    let before = f.send("before join").await;
    f.join().await;
    let after = f.send("after join").await;

    assert_eq!(f.read(Some(&f.member)).await, (vec![after.clone()], 1));
    // The creator joined first and the unscoped query is unchanged.
    let all = (vec![before.clone(), after.clone()], 2);
    assert_eq!(f.read(Some(&f.owner)).await, all);
    assert_eq!(f.read(None).await, all);

    // Rejoin moves the cut to the rejoin point: the absent period stays hidden.
    f.leave().await;
    let absent = f.send("while absent").await;
    f.join().await;
    let current = f.send("current period").await;
    assert_eq!(f.read(Some(&f.member)).await, (vec![current.clone()], 1));

    // Policy 'all' and legacy-stamped memberships keep everything.
    f.set("UPDATE channels SET history_policy='all' WHERE community_id=$1 AND id=$2")
        .await;
    let everything = (vec![before, after, absent, current], 4);
    assert_eq!(f.read(Some(&f.member)).await, everything);
    f.set("UPDATE channels SET history_policy='since_join' WHERE community_id=$1 AND id=$2")
        .await;
    // The membership trigger pins this flag on live rows; stamp it the way
    // migration 0060 did, below the trigger.
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE channel_members SET join_cutoff_is_legacy=true WHERE community_id=$1 AND channel_id=$2")
        .bind(f.scope.as_uuid())
        .bind(f.channel)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(f.read(Some(&f.member)).await, everything);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn open_rooms_show_all_history_to_members_and_visitors() {
    let f = Fixture::new(ChannelVisibility::Open).await;
    let policy: String = sqlx::query_scalar(
        "SELECT history_policy FROM channels WHERE community_id=$1 AND id=$2",
    )
    .bind(f.scope.as_uuid())
    .bind(f.channel)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(policy, "since_join", "new rooms default to since_join");
    let before = f.send("before join").await;
    f.join().await;
    let after = f.send("after join").await;
    let all = (vec![before, after], 2);
    assert_eq!(f.read(Some(&f.member)).await, all);
    assert_eq!(f.read(Some(&Keys::generate())).await, all);
}
