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
        (
            rows.into_iter().map(|s| s.event.id.to_hex()).collect(),
            count,
        )
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
    let policy: String =
        sqlx::query_scalar("SELECT history_policy FROM channels WHERE community_id=$1 AND id=$2")
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

#[tokio::test]
#[ignore = "requires Postgres"]
async fn channel_windows_hide_rows_from_before_the_join() {
    use crate::thread::{
        get_channel_window_by_sequence_for_reader, get_channel_window_for_reader, SequenceCursor,
    };
    use kuzz_core::thread_window::Direction;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let before = f.send("before join").await;
    f.join().await;
    let after = f.send("after join").await;
    let ids = |w: crate::thread::ChannelWindow| -> Vec<String> {
        w.rows
            .into_iter()
            .map(|r| r.stored_event.event.id.to_hex())
            .collect()
    };
    let member = f.member.public_key().to_bytes();
    let reader = Some(member.as_slice());

    let by_time = get_channel_window_for_reader(
        &f.pool,
        f.scope,
        f.channel,
        10,
        None,
        None,
        Direction::Older,
        reader,
    )
    .await
    .unwrap();
    assert!(!by_time.has_more);
    assert_eq!(ids(by_time), [after.clone()]);
    let by_seq = get_channel_window_by_sequence_for_reader(
        &f.pool,
        f.scope,
        f.channel,
        10,
        SequenceCursor::After(0),
        None,
        reader,
    )
    .await
    .unwrap();
    assert_eq!(ids(by_seq), [after.clone()]);
    // No reader: unchanged.
    let all = get_channel_window_by_sequence_for_reader(
        &f.pool,
        f.scope,
        f.channel,
        10,
        SequenceCursor::After(0),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(ids(all), [before, after]);
}

impl Fixture {
    /// A kind:9 reply to `root` (or a kind:7 reaction on it) through the real
    /// event and thread index writers.
    async fn reply(&self, root: &str, kind: u16, content: &str) -> String {
        let root_id = nostr::EventId::from_hex(root).expect("root id");
        let event = EventBuilder::new(Kind::Custom(kind), content)
            .tags([
                Tag::parse(["h", &self.channel.to_string()]).expect("h tag"),
                Tag::parse(["e", root, "", "reply"]).expect("e tag"),
            ])
            .sign_with_keys(&self.owner)
            .expect("fixture signature");
        event::insert_event(&self.pool, self.scope, &event, Some(self.channel))
            .await
            .expect("real event writer");
        if kind == 9 {
            let at = |ts: nostr::Timestamp| {
                chrono::DateTime::from_timestamp(ts.as_secs() as i64, 0).expect("timestamp")
            };
            let root_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
                "SELECT created_at FROM events WHERE community_id = $1 AND id = $2",
            )
            .bind(self.scope.as_uuid())
            .bind(root_id.to_bytes().to_vec())
            .fetch_one(&self.pool)
            .await
            .expect("root row");
            // Root row first (exists after the first reply), then the reply.
            crate::thread::insert_thread_metadata(
                &self.pool, self.scope, root_id.as_bytes(), root_at, self.channel,
                None, None, None, None, 0, false,
            )
            .await
            .ok();
            crate::thread::insert_thread_metadata(
                &self.pool, self.scope, event.id.as_bytes(), at(event.created_at), self.channel,
                Some(root_id.as_bytes()), Some(root_at), Some(root_id.as_bytes()), Some(root_at),
                1, false,
            )
            .await
            .expect("thread index");
        }
        event.id.to_hex()
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn thread_reads_hide_replies_and_aux_from_before_the_join() {
    use crate::thread_window::{AuxQuery, ScanBudget};
    use crate::Db;
    use kuzz_core::thread_window::Request;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    let root = f.send("root").await;
    let early = f.reply(&root, 9, "early reply").await;
    let early_reaction = f.reply(&root, 7, "early +").await;
    f.join().await;
    let late = f.reply(&root, 9, "late reply").await;
    let late_reaction = f.reply(&root, 7, "late +").await;
    let member = f.member.public_key().to_bytes();
    let reader = Some(member.as_slice());
    let root_bytes = nostr::EventId::from_hex(&root).unwrap().to_bytes();

    // Thread window rows.
    let req = Request::parse(&serde_json::json!({"thread_window": true,
        "#h": [f.channel], "#e": [root], "kinds": [9], "limit": 10}))
    .unwrap();
    let rows = |w: crate::thread_window::ThreadWindow| -> Vec<String> {
        w.rows.into_iter().map(|r| r.event.id.to_hex()).collect()
    };
    let (window, mut session) = db
        .get_thread_window_for_reader_with_session(f.scope, &req, reader, &mut ScanBudget::default())
        .await
        .unwrap();
    assert!(window.root_in_channel);
    assert_eq!(rows(window), [late.clone()]);
    let (window, _) = db
        .get_thread_window_with_session(f.scope, &req, &mut ScanBudget::default())
        .await
        .unwrap();
    let mut all = rows(window);
    all.sort();
    let mut expected = vec![early.clone(), late.clone()];
    expected.sort();
    assert_eq!(all, expected);

    // Thread replies (bridge depth path).
    let replies = db
        .get_thread_replies_for_reader(f.scope, &root_bytes, Some(1), 10, None, reader)
        .await
        .unwrap();
    let ids: Vec<String> = replies.iter().map(|r| hex::encode(&r.event_id)).collect();
    assert_eq!(ids, [late.clone()]);
    let replies = db
        .get_thread_replies(f.scope, &root_bytes, Some(1), 10, None)
        .await
        .unwrap();
    assert_eq!(replies.len(), 2);

    // Aux closure: reactions written before the join stay hidden.
    let targets = [root.clone()];
    let accessible = [f.channel];
    let aux = |history_reader| AuxQuery {
        community: f.scope,
        targets: &targets,
        kinds: &[7],
        accessible: &accessible,
        cursor: None,
        history_reader,
    };
    let page = session
        .thread_window_aux(&aux(reader), &mut ScanBudget::default())
        .await
        .unwrap();
    let ids: Vec<String> = page.events.iter().map(|e| e.event.id.to_hex()).collect();
    assert_eq!(ids, [late_reaction.clone()]);
    let page = session
        .thread_window_aux(&aux(None), &mut ScanBudget::default())
        .await
        .unwrap();
    assert_eq!(page.events.len(), 2);
    let _ = early_reaction;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn summaries_and_id_lookups_treat_pre_join_events_as_absent() {
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    let root = f.send("root").await;
    let early = f.reply(&root, 9, "early reply").await;
    f.join().await;
    let late = f.reply(&root, 9, "late reply").await;
    let member = f.member.public_key().to_bytes();
    let owner = f.owner.public_key().to_bytes();
    let bytes = |hex_id: &str| nostr::EventId::from_hex(hex_id).unwrap().to_bytes().to_vec();

    assert!(db.channel_history_floor(f.scope, f.channel, &member).await.unwrap() > 0);
    assert_eq!(db.channel_history_floor(f.scope, f.channel, &owner).await.unwrap(), 0);

    let ids = [bytes(&root), bytes(&early), bytes(&late)];
    let mut hidden = db.history_hidden_event_ids(f.scope, &member, &ids).await.unwrap();
    hidden.sort();
    let mut expected = vec![bytes(&root), bytes(&early)];
    expected.sort();
    assert_eq!(hidden, expected);
    assert!(db.history_hidden_event_ids(f.scope, &owner, &ids).await.unwrap().is_empty());

    // Stored counters (2 replies) minus the pre-join one; the author still
    // has a visible reply, so stays a participant.
    let cut = db
        .blocked_thread_replies(f.scope, &member, &[bytes(&root)], false)
        .await
        .unwrap();
    assert_eq!(cut.len(), 1);
    assert_eq!((cut[0].direct, cut[0].descendants), (1, 1));
    assert!(cut[0].hidden_authors.is_empty());
    assert!(cut[0].last_other_reply_at.is_some());
    assert!(db
        .blocked_thread_replies(f.scope, &owner, &[bytes(&root)], false)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn inbox_skips_rooms_and_threads_from_before_the_join() {
    use crate::inbox_summary::{InboxReadMarkers, InboxRowKind};
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    let old_root = f.send("old root").await;
    f.reply(&old_root, 9, "old reply").await;
    f.join().await;
    let member = f.member.public_key();
    let markers = InboxReadMarkers {
        channels: vec![],
        threads: vec![],
    };

    // Joined, nothing new yet: the room shows with no preview, no thread row.
    let page = db.inbox_summary(f.scope, &member, 30, None, &markers).await.unwrap();
    let shape: Vec<_> = page.rows.iter().map(|r| (r.kind, r.latest.is_some())).collect();
    assert_eq!(shape, [(InboxRowKind::Room, false)]);

    // A late reply to the old root still does not surface the old thread.
    f.reply(&old_root, 9, "late reply to old root").await;
    let new_root = f.send("new root").await;
    f.reply(&new_root, 9, "new reply").await;
    let page = db.inbox_summary(f.scope, &member, 30, None, &markers).await.unwrap();
    let threads: Vec<_> = page
        .rows
        .iter()
        .filter(|r| r.kind == InboxRowKind::Thread)
        .map(|r| r.root.as_ref().unwrap()["content"].clone())
        .collect();
    assert_eq!(threads, ["new root"]);
    let room = page.rows.iter().find(|r| r.kind == InboxRowKind::Room).unwrap();
    assert_eq!(room.latest.as_ref().unwrap()["content"], "new root");

    // The owner (no cut) still sees the old thread.
    let page = db
        .inbox_summary(f.scope, &f.owner.public_key(), 30, None, &markers)
        .await
        .unwrap();
    assert_eq!(page.rows.iter().filter(|r| r.kind == InboxRowKind::Thread).count(), 2);
}
