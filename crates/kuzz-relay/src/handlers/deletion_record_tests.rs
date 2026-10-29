//! Relay seams for relay-signed deletion records: undeletable records, the
//! non-author kind:5 path, the staff delete path, and post-commit delivery.
use std::sync::Arc;
use std::time::Duration;

use kuzz_core::tenant::{CommunityId, TenantContext};
use kuzz_pubsub::{ChannelEvent, EventTopic};
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

use crate::state::AppState;

struct Fixture {
    state: Arc<AppState>,
    pool: sqlx::PgPool,
    tenant: TenantContext,
    channel: Uuid,
    owner: Keys,
    author: Keys,
    live: Receiver<ChannelEvent>,
    subscriber: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.subscriber.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let state = crate::state::tests::test_state_with_config(&database_url, |config| {
            config.redis_url = redis_url;
        })
        .await;
        let pool = sqlx::PgPool::connect(&database_url).await.expect("pool");
        let community = Uuid::new_v4();
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(community)
            .bind(format!("deletion-record-{community}.test"))
            .execute(&pool)
            .await
            .expect("community");
        let (owner, author) = (Keys::generate(), Keys::generate());
        let channel = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO channels(community_id, id, name, visibility, channel_type, created_by, history_policy) \
             VALUES ($1, $2, $3, 'open'::channel_visibility, 'stream', $4, 'all')",
        )
        .bind(community)
        .bind(channel)
        .bind(channel.to_string())
        .bind(owner.public_key().to_bytes().as_slice())
        .execute(&pool)
        .await
        .expect("channel");
        for (keys, role) in [(&owner, "owner"), (&author, "member")] {
            sqlx::query(
                "INSERT INTO channel_members(community_id, channel_id, pubkey, role) \
                 VALUES ($1, $2, $3, $4::member_role)",
            )
            .bind(community)
            .bind(channel)
            .bind(keys.public_key().to_bytes().as_slice())
            .bind(role)
            .execute(&pool)
            .await
            .expect("member");
        }
        let tenant = TenantContext::resolved(
            CommunityId::from_uuid(community),
            format!("deletion-record-{community}.test"),
        );
        let subscriber = tokio::spawn(state.pubsub.clone().run_subscriber());
        let live = state.pubsub.subscribe_local();
        state
            .pubsub
            .retain_topic(&tenant, EventTopic::Channel(channel))
            .await;
        // Give SUBSCRIBE a bounded moment to attach (as the pubsub tests do).
        tokio::time::sleep(Duration::from_millis(200)).await;
        Self {
            state,
            pool,
            tenant,
            channel,
            owner,
            author,
            live,
            subscriber,
        }
    }

    fn sign(&self, keys: &Keys, kind: u16, tags: Vec<Vec<String>>) -> Event {
        let mut all = vec![vec!["h".to_string(), self.channel.to_string()]];
        all.extend(tags);
        EventBuilder::new(Kind::Custom(kind), Uuid::new_v4().to_string())
            .tags(all.into_iter().map(|t| Tag::parse(t).expect("tag")))
            .sign_with_keys(keys)
            .expect("sign")
    }

    async fn message(&self) -> Event {
        let event = self.sign(&self.author, 9, vec![]);
        self.state
            .db
            .insert_event(self.tenant.community(), &event, Some(self.channel))
            .await
            .expect("insert");
        event
    }

    fn deletion(&self, keys: &Keys, kind: u16, target: &Event) -> Event {
        self.sign(keys, kind, vec![vec!["e".into(), target.id.to_hex()]])
    }

    async fn records(&self, target: &Event) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE community_id=$1 AND kind=9005 AND pubkey=$2 \
             AND tags @> jsonb_build_array(jsonb_build_array('e', $3::text))",
        )
        .bind(self.tenant.community().as_uuid())
        .bind(self.state.relay_keypair.public_key().to_bytes().as_slice())
        .bind(target.id.to_hex())
        .fetch_one(&self.pool)
        .await
        .expect("count")
    }

    /// The next live event on the channel topic, if one arrives soon.
    async fn next_live(&mut self) -> Option<Event> {
        loop {
            match tokio::time::timeout(Duration::from_millis(1500), self.live.recv()).await {
                Ok(Ok(e)) if e.topic == EventTopic::Channel(self.channel) => return Some(e.event),
                Ok(Ok(_)) => continue,
                _ => return None,
            }
        }
    }

    fn assert_record_for(&self, record: &Event, target: &Event) {
        assert_eq!(record.kind, Kind::Custom(9005));
        assert_eq!(record.pubkey, self.state.relay_keypair.public_key());
        let e = record
            .tags
            .iter()
            .find(|t| t.kind().to_string() == "e")
            .and_then(|t| t.content());
        assert_eq!(e, Some(target.id.to_hex().as_str()));
    }
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn non_author_kind5_stores_and_publishes_one_record() {
    let mut f = Fixture::new().await;
    let target = f.message().await;
    let by_owner = f.deletion(&f.owner, 5, &target);
    super::side_effects::handle_side_effects(&f.tenant, 5, &by_owner, &f.state)
        .await
        .expect("kind 5 side effects");
    assert_eq!(f.records(&target).await, 1);
    let live = f.next_live().await.expect("record delivered after commit");
    f.assert_record_for(&live, &target);

    // The author's own kind 5 is already honoured by device stores.
    let own = f.message().await;
    let by_author = f.deletion(&f.author, 5, &own);
    super::side_effects::handle_side_effects(&f.tenant, 5, &by_author, &f.state)
        .await
        .expect("kind 5 side effects");
    assert_eq!(f.records(&own).await, 0);
    assert!(f.next_live().await.is_none(), "nothing to deliver");
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn staff_delete_stores_and_publishes_one_record() {
    let mut f = Fixture::new().await;
    let target = f.message().await;
    let staff = Keys::generate().public_key().to_bytes();
    let author = f.author.public_key().to_bytes();
    let input = kuzz_db::relay_admin_actions::DirectActionInput {
        community_id: f.tenant.community(),
        request_id: Uuid::new_v4(),
        actor_pubkey: staff.as_slice(),
        actor_role: "operator",
        actor_authority: "relay_operator",
        action: "delete",
        reason: None,
        timeout_secs: None,
        timeout_until: None,
        target_pubkey: Some(author.as_slice()),
        target_event_id: Some(target.id.as_bytes().as_slice()),
        channel_id: Some(f.channel),
    };
    let kuzz_db::relay_admin_actions::DirectClaim::Claimed(rec) =
        f.state.db.claim_direct_action(&input).await.expect("claim")
    else {
        panic!("expected a new claim");
    };
    super::report_resolution::drive_direct_action(&f.state, &f.tenant, &rec, None)
        .await
        .expect("drive delete");
    assert_eq!(f.records(&target).await, 1);
    let live = loop {
        let event = f.next_live().await.expect("record delivered after commit");
        if event.kind == Kind::Custom(9005) {
            break event;
        }
    };
    f.assert_record_for(&live, &target);
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn relay_record_cannot_be_deleted_by_channel_owner() {
    let f = Fixture::new().await;
    let target = f.message().await;
    f.state
        .db
        .soft_delete_event_and_update_thread(f.tenant.community(), target.id.as_bytes(), None, None)
        .await
        .expect("soft delete");
    let record = f
        .state
        .db
        .write_deletion_record(
            f.tenant.community(),
            target.id.as_bytes(),
            &f.state.relay_keypair,
        )
        .await
        .expect("write")
        .expect("record");

    // Control: the owner may remove an ordinary message.
    let other = f.message().await;
    let allowed = f.deletion(&f.owner, 9005, &other);
    super::side_effects::validate_admin_event(&f.tenant, 9005, &allowed, &f.state)
        .await
        .expect("owner may delete a message");

    let by_admin = f.deletion(&f.owner, 9005, &record);
    let error = super::side_effects::validate_admin_event(&f.tenant, 9005, &by_admin, &f.state)
        .await
        .expect_err("9005 must not remove a relay record");
    assert!(
        error.to_string().contains("relay deletion records"),
        "{error}"
    );

    let by_kind5 = f.deletion(&f.owner, 5, &record);
    let error =
        super::side_effects::validate_standard_deletion_event(&f.tenant, &by_kind5, &f.state)
            .await
            .expect_err("kind 5 must not remove a relay record");
    assert!(
        error.to_string().contains("relay deletion records"),
        "{error}"
    );
}
