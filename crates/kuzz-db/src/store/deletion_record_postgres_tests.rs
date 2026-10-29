//! PostgreSQL tests for relay-signed deletion records (`deletion_record`).
use super::deletion_record::{execute_delete_with_record, DeletionRecordCandidate};
use crate::Db;
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    db: Db,
    pool: PgPool,
    community: CommunityId,
    channel: Uuid,
    relay: Keys,
    author: Keys,
    other: Keys,
}

impl Fixture {
    async fn new() -> Self {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("connect test DB");
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        let db = Db::from_pool(pool.clone());
        db.ensure_future_partitions(1, true)
            .await
            .expect("partitions");
        let community = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(community.as_uuid())
            .bind(format!("deletion-record-{}.test", community.as_uuid()))
            .execute(&pool)
            .await
            .expect("community");
        let author = Keys::generate();
        let channel = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO channels(community_id, id, name, visibility, channel_type, created_by, history_policy) \
             VALUES ($1, $2, $3, 'open'::channel_visibility, 'stream', $4, 'all')",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .bind(channel.to_string())
        .bind(author.public_key().to_bytes().as_slice())
        .execute(&pool)
        .await
        .expect("channel");
        Self {
            db,
            pool,
            community,
            channel,
            relay: Keys::generate(),
            author,
            other: Keys::generate(),
        }
    }

    /// Store a signed event of `kind` by `keys` in `channel` (or none).
    async fn store(&self, keys: &Keys, kind: u16, tags: Vec<Tag>, channel: Option<Uuid>) -> Event {
        let event = EventBuilder::new(Kind::Custom(kind), Uuid::new_v4().to_string())
            .tags(tags)
            .sign_with_keys(keys)
            .expect("sign");
        crate::event::insert_event(&self.pool, self.community, &event, channel)
            .await
            .expect("insert");
        event
    }

    async fn message(&self) -> Event {
        let h = Tag::parse(["h", &self.channel.to_string()]).expect("h");
        self.store(&self.author, 9, vec![h], Some(self.channel))
            .await
    }

    async fn soft_delete(&self, id: &[u8]) {
        self.db
            .soft_delete_event_and_update_thread(self.community, id, None, None)
            .await
            .expect("soft delete");
    }

    async fn deleted(&self, id: &[u8]) -> bool {
        sqlx::query_scalar(
            "SELECT deleted_at IS NOT NULL FROM events WHERE community_id=$1 AND id=$2",
        )
        .bind(self.community.as_uuid())
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .expect("row")
    }

    /// Relay-signed 9005 rows naming `id`.
    async fn records(&self, id: &[u8]) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE community_id=$1 AND kind=9005 AND pubkey=$2 \
             AND tags @> jsonb_build_array(jsonb_build_array('e', $3::text))",
        )
        .bind(self.community.as_uuid())
        .bind(self.relay.public_key().to_bytes().as_slice())
        .bind(hex::encode(id))
        .fetch_one(&self.pool)
        .await
        .expect("count")
    }

    /// An enforcing staff delete action holding a lease; `report` makes it a
    /// report action, otherwise it is a direct one.
    async fn action(&self, target: &[u8], report: Option<Uuid>, lease_secs: i64) -> (Uuid, Uuid) {
        let token = Uuid::new_v4();
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO relay_admin_actions (report_id, report_community_id, request_id, \
               actor_pubkey, actor_role, action, state, action_lease_token, \
               action_lease_expires_at, enforcement_target_event_id, enforcement_target_pubkey) \
             VALUES ($1, $2, $3, $4, 'operator', 'delete', 'enforcing', $5, \
               now() + make_interval(secs => $6), $7, $8) RETURNING id",
        )
        .bind(report)
        .bind(self.community.as_uuid())
        .bind(Uuid::new_v4())
        .bind(self.other.public_key().to_bytes().as_slice())
        .bind(token)
        .bind(lease_secs as f64)
        .bind(report.is_none().then_some(target))
        .bind(
            report
                .is_none()
                .then(|| self.author.public_key().to_bytes().to_vec()),
        )
        .fetch_one(&self.pool)
        .await
        .expect("action");
        (id, token)
    }

    async fn report(&self, target: &[u8]) -> Uuid {
        let report_event = Keys::generate().public_key().to_bytes();
        sqlx::query_scalar(
            "INSERT INTO moderation_reports (community_id, report_event_id, reporter_pubkey, \
               target_kind, target_event_id, report_type) \
             VALUES ($1, $2, $3, 'event', $4, 'spam') RETURNING id",
        )
        .bind(self.community.as_uuid())
        .bind(report_event.as_slice())
        .bind(self.other.public_key().to_bytes().as_slice())
        .bind(target)
        .fetch_one(&self.pool)
        .await
        .expect("report")
    }

    async fn delete(
        &self,
        target: &[u8],
        report: Option<Uuid>,
    ) -> super::deletion_record::AdminDeleteOutcome {
        let (action, token) = self.action(target, report, 60).await;
        execute_delete_with_record(
            &self.pool,
            action,
            token,
            self.community,
            target,
            None,
            None,
            Some(&self.relay),
        )
        .await
        .expect("delete")
    }

    async fn succeed(&self, target: &[u8], report: Option<Uuid>) {
        let (action, token) = self.action(target, report, 60).await;
        let committed = crate::relay_admin_actions::execute_delete_with_marker(
            &self.pool,
            action,
            token,
            self.community,
            target,
            None,
            None,
        )
        .await
        .expect("legacy delete");
        assert!(committed);
        sqlx::query("UPDATE relay_admin_actions SET state='succeeded' WHERE id=$1")
            .bind(action)
            .execute(&self.pool)
            .await
            .expect("succeed");
    }

    async fn candidates(&self) -> Vec<DeletionRecordCandidate> {
        self.db
            .deletion_record_backfill_candidates(
                self.community,
                &self.relay.public_key(),
                None,
                100,
            )
            .await
            .expect("candidates")
    }
}

fn tag_value(event: &Event, name: &str) -> Option<String> {
    event
        .tags
        .iter()
        .find(|t| t.kind().to_string() == name)
        .and_then(|t| t.content().map(str::to_owned))
}

fn assert_record_shape(f: &Fixture, record: &Event, target: &Event) {
    record.verify().expect("valid signature");
    assert_eq!(record.kind, Kind::Custom(9005));
    assert_eq!(record.pubkey, f.relay.public_key());
    assert!(record.content.is_empty());
    assert_eq!(tag_value(record, "h"), Some(f.channel.to_string()));
    assert_eq!(tag_value(record, "e"), Some(target.id.to_hex()));
    assert_eq!(
        tag_value(record, "k"),
        Some(target.kind.as_u16().to_string())
    );
    let age = nostr::Timestamp::now().as_secs() as i64 - record.created_at.as_secs() as i64;
    assert!((0..60).contains(&age), "signed at commit time, age {age}s");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn direct_delete_stores_one_record_across_retries() {
    let f = Fixture::new().await;
    let target = f.message().await;
    let id = target.id.as_bytes();

    let first = f.delete(id, None).await;
    assert!(first.committed);
    assert_record_shape(&f, first.record.as_ref().expect("record"), &target);
    assert!(f.deleted(id).await);
    assert_eq!(f.records(id).await, 1);

    // A second action against the deleted row commits but adds no record.
    let second = f.delete(id, None).await;
    assert!(second.committed);
    assert!(second.record.is_none(), "nothing new to publish");
    assert_eq!(f.records(id).await, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn report_delete_stores_one_record_from_the_target_row() {
    let f = Fixture::new().await;
    let target = f.message().await;
    let id = target.id.as_bytes();
    let report = f.report(id).await;

    let outcome = f.delete(id, Some(report)).await;
    assert!(outcome.committed);
    assert_record_shape(&f, outcome.record.as_ref().expect("record"), &target);
    assert!(f.deleted(id).await);
    assert_eq!(f.records(id).await, 1);

    assert!(f
        .delete(id, Some(f.report(id).await))
        .await
        .record
        .is_none());
    assert_eq!(f.records(id).await, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn lost_lease_rolls_back_delete_and_record() {
    let f = Fixture::new().await;
    let target = f.message().await;
    let id = target.id.as_bytes();
    let (action, token) = f.action(id, None, -1).await;

    let outcome = execute_delete_with_record(
        &f.pool,
        action,
        token,
        f.community,
        id,
        None,
        None,
        Some(&f.relay),
    )
    .await
    .expect("delete");
    assert!(!outcome.committed);
    assert!(
        outcome.record.is_none(),
        "nothing committed, nothing to publish"
    );
    assert!(!f.deleted(id).await);
    assert_eq!(f.records(id).await, 0);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn channel_less_and_non_content_targets_get_no_record() {
    let f = Fixture::new().await;
    let loose = f.store(&f.author, 9, vec![], None).await;
    let outcome = f.delete(loose.id.as_bytes(), None).await;
    assert!(outcome.committed);
    assert!(outcome.record.is_none());
    assert!(f.deleted(loose.id.as_bytes()).await);
    assert_eq!(f.records(loose.id.as_bytes()).await, 0);

    let h = Tag::parse(["h", &f.channel.to_string()]).expect("h");
    let reaction = f.store(&f.author, 7, vec![h], Some(f.channel)).await;
    f.soft_delete(reaction.id.as_bytes()).await;
    let written =
        f.db.write_deletion_record(f.community, reaction.id.as_bytes(), &f.relay)
            .await
            .expect("write");
    assert!(written.is_none());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn non_author_deletion_record_is_written_once() {
    let f = Fixture::new().await;
    let target = f.message().await;
    let id = target.id.as_bytes();

    // Not deleted yet: the record never precedes the removal.
    let early =
        f.db.write_deletion_record(f.community, id, &f.relay)
            .await
            .expect("write");
    assert!(early.is_none());

    f.soft_delete(id).await;
    let record =
        f.db.write_deletion_record(f.community, id, &f.relay)
            .await
            .expect("write")
            .expect("record");
    assert_record_shape(&f, &record, &target);
    let again =
        f.db.write_deletion_record(f.community, id, &f.relay)
            .await
            .expect("write");
    assert!(again.is_none());
    assert_eq!(f.records(id).await, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn concurrent_record_writers_store_one() {
    let f = Fixture::new().await;
    let target = f.message().await;
    let id = target.id.as_bytes().to_vec();
    f.soft_delete(&id).await;

    // Writer A holds an uncommitted record. Writer B signs in a later second,
    // so its record has a different id and only the lock + check stop it.
    let mut tx = f.db.begin_event_write_transaction().await.expect("begin");
    let held = super::deletion_record::record_deletion_in_tx(&mut tx, f.community, &id, &f.relay)
        .await
        .expect("record in tx");
    assert!(held.is_some());
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let db = f.db.clone();
    let (community, relay, target_id) = (f.community, f.relay.clone(), id.clone());
    let second = tokio::spawn(async move {
        db.write_deletion_record(community, &target_id, &relay)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !second.is_finished(),
        "second writer waits for the target lock"
    );
    tx.commit().await.expect("commit");
    let second = second.await.expect("join").expect("write");
    assert!(second.is_none());
    assert_eq!(f.records(&id).await, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_candidates_cover_staff_and_non_author_deletes_once() {
    let f = Fixture::new().await;
    let h = || Tag::parse(["h", &f.channel.to_string()]).expect("h");
    let direct = f.message().await;
    f.succeed(direct.id.as_bytes(), None).await;
    let reported = f.message().await;
    f.succeed(
        reported.id.as_bytes(),
        Some(f.report(reported.id.as_bytes()).await),
    )
    .await;

    // Kind 5 by another signer (e.g. the agent owner) and by the author.
    let by_owner = f.message().await;
    let e = Tag::parse(["e", &by_owner.id.to_hex()]).expect("e");
    f.store(&f.other, 5, vec![e, h()], Some(f.channel)).await;
    f.soft_delete(by_owner.id.as_bytes()).await;
    let by_author = f.message().await;
    let e = Tag::parse(["e", &by_author.id.to_hex()]).expect("e");
    f.store(&f.author, 5, vec![e, h()], Some(f.channel)).await;
    f.soft_delete(by_author.id.as_bytes()).await;

    // Staff delete of a channel-less row, and of a row still visible.
    let loose = f.store(&f.author, 9, vec![], None).await;
    f.succeed(loose.id.as_bytes(), None).await;
    let restored = f.message().await;
    let report = f.report(restored.id.as_bytes()).await;
    sqlx::query(
        "INSERT INTO relay_admin_actions (report_id, report_community_id, request_id, \
           actor_pubkey, actor_role, action, state) \
         VALUES ($1, $2, $3, $4, 'operator', 'delete', 'succeeded')",
    )
    .bind(report)
    .bind(f.community.as_uuid())
    .bind(Uuid::new_v4())
    .bind(f.other.public_key().to_bytes().as_slice())
    .execute(&f.pool)
    .await
    .expect("visible-target action");

    let mut expected: Vec<Vec<u8>> = [&direct, &reported, &by_owner]
        .iter()
        .map(|e| e.id.as_bytes().to_vec())
        .collect();
    expected.sort();
    let found: Vec<Vec<u8>> = f
        .candidates()
        .await
        .into_iter()
        .map(|c| c.target_id)
        .collect();
    assert_eq!(found, expected);

    // Keyset paging: one row per page walks the same set.
    let mut paged = Vec::new();
    let mut after: Option<Vec<u8>> = None;
    loop {
        let page =
            f.db.deletion_record_backfill_candidates(
                f.community,
                &f.relay.public_key(),
                after.as_deref(),
                1,
            )
            .await
            .expect("page");
        let Some(last) = page.last() else { break };
        after = Some(last.target_id.clone());
        paged.extend(page.into_iter().map(|c| c.target_id));
    }
    assert_eq!(paged, expected);

    for id in &expected {
        let record =
            f.db.write_deletion_record(f.community, id, &f.relay)
                .await
                .expect("write");
        assert!(record.is_some());
    }
    assert!(f.candidates().await.is_empty(), "recorded targets drop out");
    for id in &expected {
        assert_eq!(f.records(id).await, 1);
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn staff_delete_takes_the_channel_lock_before_thread_counters() {
    use crate::event::{insert_event_with_thread_metadata, ThreadMetadataParams};
    let f = Fixture::new().await;
    let at =
        |e: &Event| chrono::DateTime::from_timestamp(e.created_at.as_secs() as i64, 0).expect("ts");
    let h = Tag::parse(["h", &f.channel.to_string()]).expect("h");
    // Kind 40001: unlike kind 9, its soft delete fires no chat-state trigger
    // that would take the channel lock before the counter updates.
    let mut thread = Vec::new();
    for depth in 0..2 {
        let event = EventBuilder::new(Kind::Custom(40001), Uuid::new_v4().to_string())
            .tags([h.clone()])
            .sign_with_keys(&f.author)
            .expect("sign");
        let parent: Option<&Event> = thread.first();
        insert_event_with_thread_metadata(
            &f.pool,
            f.community,
            &event,
            Some(f.channel),
            Some(ThreadMetadataParams {
                event_id: event.id.as_bytes(),
                event_created_at: at(&event),
                channel_id: f.channel,
                parent_event_id: parent.map(|p| p.id.as_bytes().as_slice()),
                parent_event_created_at: parent.map(at),
                root_event_id: parent.map(|p| p.id.as_bytes().as_slice()),
                root_event_created_at: parent.map(at),
                depth,
                broadcast: false,
            }),
        )
        .await
        .expect("thread event");
        thread.push(event);
    }
    let (root, reply) = (
        thread[0].id.as_bytes().to_vec(),
        thread[1].id.as_bytes().to_vec(),
    );

    // A channel writer: channel lock (as the insert trigger takes it), then
    // the parent's thread counters (as a reply insert updates them).
    let mut writer = f.pool.begin().await.expect("begin writer");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "kuzz_channel_membership:{}:{}",
            f.community.as_uuid(),
            f.channel
        ))
        .execute(&mut *writer)
        .await
        .expect("channel lock");

    let (action, token) = f.action(&reply, None, 60).await;
    let (pool, community, relay) = (f.pool.clone(), f.community, f.relay.clone());
    let (target, parent) = (reply.clone(), root.clone());
    let staff = tokio::spawn(async move {
        execute_delete_with_record(
            &pool,
            action,
            token,
            community,
            &target,
            Some(&parent),
            Some(&parent),
            Some(&relay),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !staff.is_finished(),
        "staff delete waits for the channel lock"
    );

    // Deadlocks if the staff delete already holds the parent's counter row.
    sqlx::query(
        "UPDATE thread_metadata SET reply_count = reply_count + 1 \
         WHERE community_id = $1 AND event_id = $2",
    )
    .bind(f.community.as_uuid())
    .bind(&root)
    .execute(&mut *writer)
    .await
    .expect("writer updates thread counters without a deadlock");
    writer.commit().await.expect("writer commits");

    let outcome = staff.await.expect("join").expect("staff delete");
    assert!(outcome.committed && !outcome.record_failed);
    assert!(outcome.record.is_some());
    assert_eq!(f.records(&reply).await, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn record_commit_failure_keeps_the_delete_and_marker() {
    let f = Fixture::new().await;
    let target = f.message().await;
    let id = target.id.as_bytes();
    // The replica fence (migration 0021) then rejects every new channel row
    // at commit, as it would a record signed too early.
    let fenced = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SELECT set_config('kuzz.created_at_floor', '0.000001', false)")
                    .execute(conn)
                    .await
                    .map(|_| ())
            })
        })
        .connect(&crate::test_support::database_url())
        .await
        .expect("fenced pool");

    let (action, token) = f.action(id, None, 60).await;
    let outcome = execute_delete_with_record(
        &fenced,
        action,
        token,
        f.community,
        id,
        None,
        None,
        Some(&f.relay),
    )
    .await
    .expect("delete commits without the record");
    assert!(outcome.committed && outcome.record_failed);
    assert!(outcome.record.is_none());
    assert!(f.deleted(id).await);
    assert_eq!(f.records(id).await, 0);
    let marker: Option<String> =
        sqlx::query_scalar("SELECT step_marker FROM relay_admin_actions WHERE id = $1")
            .bind(action)
            .fetch_one(&f.pool)
            .await
            .expect("marker");
    assert_eq!(marker.as_deref(), Some("mutation_committed"));
    // The backfill then finds it.
    sqlx::query("UPDATE relay_admin_actions SET state = 'failed' WHERE id = $1")
        .bind(action)
        .execute(&f.pool)
        .await
        .expect("fail action");
    assert_eq!(f.candidates().await.len(), 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn future_dated_target_gets_one_record() {
    let f = Fixture::new().await;
    let h = Tag::parse(["h", &f.channel.to_string()]).expect("h");
    let ahead = nostr::Timestamp::from(nostr::Timestamp::now().as_secs() + 2 * 86_400);
    let target = EventBuilder::new(Kind::Custom(9), Uuid::new_v4().to_string())
        .tags([h])
        .custom_created_at(ahead)
        .sign_with_keys(&f.author)
        .expect("sign");
    crate::event::insert_event(&f.pool, f.community, &target, Some(f.channel))
        .await
        .expect("insert");
    let id = target.id.as_bytes();

    assert!(f.delete(id, None).await.record.is_some());
    assert!(
        f.delete(id, None).await.record.is_none(),
        "existing record found"
    );
    assert_eq!(f.records(id).await, 1);
    assert!(f.candidates().await.is_empty());
}
