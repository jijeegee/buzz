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
        q.history_reader = reader.map(|keys| keys.public_key().to_bytes().to_vec());
        q
    }

    /// Visible ids, oldest first, and the matching COUNT.
    async fn read(&self, reader: Option<&Keys>) -> (Vec<String>, i64) {
        let q = self.query(reader);
        let rows = event::query_events(&self.pool, &q).await.expect("query");
        let count = event::count_events(&self.pool, &q).await.expect("count");
        // Oldest first by the channel order number (fixture rows can share a
        // second, so created_at alone does not order them).
        let order: Vec<(Vec<u8>, i64)> = sqlx::query_as(
            "SELECT id, channel_sequence FROM events WHERE community_id = $1 AND channel_id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(self.channel)
        .fetch_all(&self.pool)
        .await
        .expect("order numbers");
        let seq_of = |id: &[u8]| {
            order
                .iter()
                .find(|(row_id, _)| row_id.as_slice() == id)
                .map(|(_, seq)| *seq)
        };
        let mut rows = rows;
        rows.sort_by_key(|s| seq_of(s.event.id.as_bytes()));
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
async fn open_rooms_follow_the_history_policy() {
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
    // since_join: members from their join, visitors nothing.
    assert_eq!(f.read(Some(&f.member)).await, (vec![after.clone()], 1));
    assert_eq!(f.read(Some(&Keys::generate())).await, (vec![], 0));
    // all: everyone reads everything.
    f.set("UPDATE channels SET history_policy='all' WHERE community_id=$1 AND id=$2")
        .await;
    let all = (vec![before, after], 2);
    assert_eq!(f.read(Some(&f.member)).await, all);
    assert_eq!(f.read(Some(&Keys::generate())).await, all);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn channel_windows_hide_rows_from_before_the_join() {
    use crate::thread::get_channel_window_for_reader;
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
    // No reader: unchanged (newest first).
    let all = get_channel_window_for_reader(
        &f.pool,
        f.scope,
        f.channel,
        10,
        None,
        None,
        Direction::Older,
        None,
    )
    .await
    .unwrap();
    let mut all = ids(all);
    all.sort();
    let mut expected = vec![before, after];
    expected.sort();
    assert_eq!(all, expected);
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
                &self.pool,
                self.scope,
                root_id.as_bytes(),
                root_at,
                self.channel,
                None,
                None,
                None,
                None,
                0,
                false,
            )
            .await
            .ok();
            crate::thread::insert_thread_metadata(
                &self.pool,
                self.scope,
                event.id.as_bytes(),
                at(event.created_at),
                self.channel,
                Some(root_id.as_bytes()),
                Some(root_at),
                Some(root_id.as_bytes()),
                Some(root_at),
                1,
                false,
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
        .get_thread_window_for_reader_with_session(
            f.scope,
            &req,
            reader,
            &mut ScanBudget::default(),
        )
        .await
        .unwrap();
    // The whole thread of a pre-join root is cut, later replies included.
    assert!(!window.root_in_channel);
    assert!(rows(window).is_empty());
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
    assert!(ids.is_empty(), "{ids:?}");
    let replies = db
        .get_thread_replies(f.scope, &root_bytes, Some(1), 10, None)
        .await
        .unwrap();
    assert_eq!(replies.len(), 2);

    // Aux closure: reactions on the pre-join root stay hidden, whenever they
    // were written.
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
    assert!(ids.is_empty(), "{ids:?}");
    let page = session
        .thread_window_aux(&aux(None), &mut ScanBudget::default())
        .await
        .unwrap();
    assert_eq!(page.events.len(), 2);
    let _ = (early_reaction, late_reaction);
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
    let bytes = |hex_id: &str| {
        nostr::EventId::from_hex(hex_id)
            .unwrap()
            .to_bytes()
            .to_vec()
    };

    assert!(
        db.channel_history_floor(f.scope, f.channel, &member)
            .await
            .unwrap()
            > 0
    );
    assert_eq!(
        db.channel_history_floor(f.scope, f.channel, &owner)
            .await
            .unwrap(),
        0
    );

    let ids = [bytes(&root), bytes(&early), bytes(&late)];
    let mut hidden = db
        .history_hidden_event_ids(f.scope, &member, &ids)
        .await
        .unwrap();
    hidden.sort();
    let mut expected = vec![bytes(&root), bytes(&early), bytes(&late)];
    expected.sort();
    assert_eq!(hidden, expected);
    assert!(db
        .history_hidden_event_ids(f.scope, &owner, &ids)
        .await
        .unwrap()
        .is_empty());

    // Stored counters (2 replies): both are cut with their pre-join root,
    // the later one included, so the author has no visible reply left.
    let cut = db
        .blocked_thread_replies(f.scope, &member, &[bytes(&root)], false)
        .await
        .unwrap();
    assert_eq!(cut.len(), 1);
    assert_eq!((cut[0].direct, cut[0].descendants), (2, 2));
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
    let page = db
        .inbox_summary(f.scope, &member, 30, None, &markers)
        .await
        .unwrap();
    let shape: Vec<_> = page
        .rows
        .iter()
        .map(|r| (r.kind, r.latest.is_some()))
        .collect();
    assert_eq!(shape, [(InboxRowKind::Room, false)]);

    // A late reply to the old root still does not surface the old thread.
    f.reply(&old_root, 9, "late reply to old root").await;
    let new_root = f.send("new root").await;
    f.reply(&new_root, 9, "new reply").await;
    let page = db
        .inbox_summary(f.scope, &member, 30, None, &markers)
        .await
        .unwrap();
    let threads: Vec<_> = page
        .rows
        .iter()
        .filter(|r| r.kind == InboxRowKind::Thread)
        .map(|r| r.root.as_ref().unwrap()["content"].clone())
        .collect();
    assert_eq!(threads, ["new root"]);
    let room = page
        .rows
        .iter()
        .find(|r| r.kind == InboxRowKind::Room)
        .unwrap();
    assert_eq!(room.latest.as_ref().unwrap()["content"], "new root");

    // The owner (no cut) still sees the old thread.
    let page = db
        .inbox_summary(f.scope, &f.owner.public_key(), 30, None, &markers)
        .await
        .unwrap();
    assert_eq!(
        page.rows
            .iter()
            .filter(|r| r.kind == InboxRowKind::Thread)
            .count(),
        2
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn chat_list_previews_only_messages_after_the_join() {
    use crate::chat_list::{ChatListQuery, ChatRowKind};
    use crate::event::ThreadMetadataParams;
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone()).with_chat_state_writes(true);
    let post = |content: &'static str, root: Option<nostr::Event>| {
        let db = db.clone();
        let (scope, channel, owner) = (f.scope, f.channel, f.owner.clone());
        async move {
            let mut tags = vec![Tag::parse(["h", &channel.to_string()]).unwrap()];
            if let Some(root) = &root {
                tags.push(Tag::parse(["e", &root.id.to_hex(), "", "root"]).unwrap());
            }
            let event = EventBuilder::new(Kind::Custom(9), content)
                .tags(tags)
                .sign_with_keys(&owner)
                .unwrap();
            let at = |e: &nostr::Event| {
                chrono::DateTime::from_timestamp(e.created_at.as_secs() as i64, 0).unwrap()
            };
            let meta = root.as_ref().map(|root| ThreadMetadataParams {
                event_id: event.id.as_bytes(),
                event_created_at: at(&event),
                channel_id: channel,
                parent_event_id: Some(root.id.as_bytes()),
                parent_event_created_at: Some(at(root)),
                root_event_id: Some(root.id.as_bytes()),
                root_event_created_at: Some(at(root)),
                depth: 1,
                broadcast: false,
            });
            db.insert_event_with_thread_metadata_and_chat_state(scope, &event, Some(channel), meta)
                .await
                .unwrap();
            event
        }
    };
    let old_root = post("old root", None).await;
    post("old reply", Some(old_root.clone())).await;
    f.join().await;
    let query = ChatListQuery {
        limit: 30,
        before: None,
        pinned_threads: vec![old_root.id.as_bytes().to_vec()],
        quiet_before: chrono::Utc::now() - chrono::Duration::days(7),
    };
    let rows = db
        .chat_list(f.scope, &f.member.public_key(), &query)
        .await
        .unwrap()
        .rows;
    let shape: Vec<_> = rows.iter().map(|r| (r.kind, r.latest.is_some())).collect();
    assert_eq!(
        shape,
        [(ChatRowKind::Room, false)],
        "no old preview, no old thread"
    );

    post("late reply to old root", Some(old_root.clone())).await;
    post("new message", None).await;
    let rows = db
        .chat_list(f.scope, &f.member.public_key(), &query)
        .await
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].latest.as_ref().unwrap()["content"], "new message");

    // The owner (no cut) still gets the pinned old thread.
    let rows = db
        .chat_list(f.scope, &f.owner.public_key(), &query)
        .await
        .unwrap()
        .rows;
    assert!(rows.iter().any(|r| r.kind == ChatRowKind::Thread));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn mark_read_and_follow_treat_pre_join_ids_as_unknown() {
    use crate::chat_user_state::{
        ChatStateDenied, FollowState, MarkReadItem, MarkReadTarget, ReadPosition,
    };
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone()).with_chat_state_writes(true);
    let old = f.send("old root").await;
    f.join().await;
    let new = f.send("new root").await;
    let member = f.member.public_key();
    let id = |hex_id: &str| {
        nostr::EventId::from_hex(hex_id)
            .unwrap()
            .to_bytes()
            .to_vec()
    };

    let follow = |root: Vec<u8>| {
        let db = db.clone();
        async move {
            db.chat_thread_follow(f.scope, &member, &root, FollowState::Followed)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        follow(id(&old)).await,
        Err(ChatStateDenied::NotMember { index: 0 })
    );
    assert_eq!(follow(id(&new)).await, Ok(f.channel));

    let read = |target: MarkReadTarget, event: Vec<u8>| {
        let db = db.clone();
        async move {
            let item = MarkReadItem {
                target,
                position: Some(ReadPosition::Event(event)),
            };
            db.chat_mark_read(f.scope, &member, &[item], false)
                .await
                .unwrap()
                .map(|_| ())
        }
    };
    let room = MarkReadTarget::Channel(f.channel);
    assert_eq!(
        read(room.clone(), id(&old)).await,
        Err(ChatStateDenied::UnknownEvent { index: 0 })
    );
    assert_eq!(read(room, id(&new)).await, Ok(()));
    assert_eq!(
        read(MarkReadTarget::Thread(id(&old)), id(&old)).await,
        Err(ChatStateDenied::NotMember { index: 0 })
    );
    // The owner (no cut) may still follow the old root.
    assert!(db
        .chat_thread_follow(
            f.scope,
            &f.owner.public_key(),
            &id(&old),
            FollowState::Followed
        )
        .await
        .unwrap()
        .is_ok());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn files_from_before_the_join_are_not_readable() {
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    let (scope, channel) = (f.scope, f.channel);
    let attach = |hex_id: String, sha: String| {
        let db = db.clone();
        let pool = f.pool.clone();
        async move {
            let id = nostr::EventId::from_hex(&hex_id)
                .unwrap()
                .to_bytes()
                .to_vec();
            let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
                "SELECT created_at FROM events WHERE community_id = $1 AND id = $2",
            )
            .bind(scope.as_uuid())
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
            db.record_media_references(scope, &id, at, Some(channel), &[sha])
                .await
                .unwrap();
        }
    };
    let old_sha = "a".repeat(64);
    let new_sha = "b".repeat(64);
    attach(f.send("old file").await, old_sha.clone()).await;
    f.join().await;
    attach(f.send("new file").await, new_sha.clone()).await;
    let both = [old_sha.clone(), new_sha.clone()];
    let member = f.member.public_key().to_bytes();
    let owner = f.owner.public_key().to_bytes();
    assert_eq!(
        db.media_hashes_readable_by(f.scope, &member, &both)
            .await
            .unwrap(),
        [new_sha.clone()]
    );
    let mut all = db
        .media_hashes_readable_by(f.scope, &owner, &both)
        .await
        .unwrap();
    all.sort();
    assert_eq!(all, both);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn artifact_revisions_from_before_the_join_are_hidden() {
    use crate::Db;
    use kuzz_core::artifact::{ArtifactQuery, ArtifactView};

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    let revision = |content: &'static str| {
        let event = EventBuilder::new(Kind::Custom(45010), content)
            .tags([
                Tag::parse(["h", &f.channel.to_string()]).unwrap(),
                Tag::parse(["d", "doc"]).unwrap(),
            ])
            .sign_with_keys(&f.owner)
            .unwrap();
        let (pool, scope, channel) = (f.pool.clone(), f.scope, f.channel);
        async move {
            event::insert_event(&pool, scope, &event, Some(channel))
                .await
                .unwrap();
            event.id.to_hex()
        }
    };
    revision("v1").await;
    f.join().await;
    let v2 = revision("v2").await;
    let query = ArtifactQuery {
        view: ArtifactView::History,
        tags: vec![("d".into(), vec!["doc".into()])],
        limit: 10,
        offset: 0,
    };
    let read = |reader: Vec<u8>, count: bool| {
        let db = db.clone();
        let query = &query;
        async move {
            db.query_artifacts(f.scope, &reader, query, count)
                .await
                .unwrap()
        }
    };
    let member = f.member.public_key().to_bytes().to_vec();
    let (rows, _) = read(member.clone(), false).await;
    assert_eq!(
        rows.iter().map(|r| r.event.id.to_hex()).collect::<Vec<_>>(),
        [v2]
    );
    assert_eq!(read(member, true).await.1, 1);
    assert_eq!(
        read(f.owner.public_key().to_bytes().to_vec(), true).await.1,
        2
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn later_reactions_and_edits_on_pre_join_messages_stay_hidden() {
    let f = Fixture::new(ChannelVisibility::Private).await;
    let old = f.send("old message").await;
    f.join().await;
    let new = f.send("new message").await;
    let reaction_on_old = f.reply(&old, 7, "on old").await;
    let edit_on_old = f.reply(&old, 40003, "edited old").await;
    let reaction_on_new = f.reply(&new, 7, "on new").await;

    for one_channel in [true, false] {
        let mut q = EventQuery::for_community(f.scope);
        q.channel_id = one_channel.then_some(f.channel);
        q.kinds = Some(vec![7, 40003]);
        q.history_reader = Some(f.member.public_key().to_bytes().to_vec());
        let rows = event::query_events(&f.pool, &q).await.unwrap();
        let ids: Vec<String> = rows.iter().map(|r| r.event.id.to_hex()).collect();
        assert_eq!(ids, [reaction_on_new.clone()], "one_channel={one_channel}");
        assert_eq!(event::count_events(&f.pool, &q).await.unwrap(), 1);
        q.history_reader = Some(f.owner.public_key().to_bytes().to_vec());
        assert_eq!(event::count_events(&f.pool, &q).await.unwrap(), 3);
    }
    let _ = (reaction_on_old, edit_on_old);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn chat_list_counts_and_latest_ignore_pre_join_messages() {
    use crate::chat_list::{ChatListQuery, ChatRowKind};
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone()).with_chat_state_writes(true);
    let post = |content: &'static str| {
        let event = EventBuilder::new(Kind::Custom(9), content)
            .tags([Tag::parse(["h", &f.channel.to_string()]).unwrap()])
            .sign_with_keys(&f.owner)
            .unwrap();
        let (db, scope, channel) = (db.clone(), f.scope, f.channel);
        async move {
            db.insert_event_with_thread_metadata_and_chat_state(scope, &event, Some(channel), None)
                .await
                .unwrap();
            event
        }
    };
    post("old one").await;
    post("old two").await;
    f.join().await;
    let query = ChatListQuery {
        limit: 30,
        before: None,
        pinned_threads: vec![],
        quiet_before: chrono::Utc::now() - chrono::Duration::days(7),
    };
    let rows = db
        .chat_list(f.scope, &f.member.public_key(), &query)
        .await
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].kind, rows[0].unread), (ChatRowKind::Room, 0));

    let new = post("new one").await;
    let rows = db
        .chat_list(f.scope, &f.member.public_key(), &query)
        .await
        .unwrap()
        .rows;
    assert_eq!(rows[0].unread, 1);
    assert_eq!(rows[0].latest.as_ref().unwrap()["content"], "new one");

    // Deleting the only post-join message must not bring back an old preview.
    sqlx::query("UPDATE events SET deleted_at = now() WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(new.id.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE channels SET main_last_event_id = (SELECT id FROM events WHERE community_id = $1 \
         AND channel_id = $2 AND deleted_at IS NULL ORDER BY channel_sequence DESC LIMIT 1) \
         WHERE community_id = $1 AND id = $2",
    )
    .bind(f.scope.as_uuid())
    .bind(f.channel)
    .execute(&f.pool)
    .await
    .unwrap();
    let rows = db
        .chat_list(f.scope, &f.member.public_key(), &query)
        .await
        .unwrap()
        .rows;
    assert!(rows[0].latest.is_none(), "{:?}", rows[0].latest);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn feed_mentions_and_activity_skip_messages_from_before_the_join() {
    let f = Fixture::new(ChannelVisibility::Private).await;
    let member_hex = f.member.public_key().to_hex();
    let mention = |content: &'static str| {
        let f = &f;
        let member_hex = member_hex.clone();
        async move {
            let event = EventBuilder::new(Kind::Custom(9), content)
                .tags([
                    Tag::parse(["h", &f.channel.to_string()]).expect("h tag"),
                    Tag::parse(["p", &member_hex]).expect("p tag"),
                ])
                .sign_with_keys(&f.owner)
                .expect("fixture signature");
            event::insert_event(&f.pool, f.scope, &event, Some(f.channel))
                .await
                .expect("real event writer");
            sqlx::query(
                "INSERT INTO event_mentions \
                 (community_id, pubkey_hex, event_id, event_created_at, channel_id, event_kind) \
                 SELECT community_id, $2, id, created_at, channel_id, kind FROM events \
                  WHERE community_id = $1 AND id = $3",
            )
            .bind(f.scope.as_uuid())
            .bind(&member_hex)
            .bind(event.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .expect("mention row");
            event.id.to_hex()
        }
    };
    let old = mention("old mention").await;
    f.join().await;
    let new = mention("new mention").await;
    let member = f.member.public_key().to_bytes();
    let ids = |rows: Vec<kuzz_core::StoredEvent>| -> Vec<String> {
        rows.into_iter().map(|r| r.event.id.to_hex()).collect()
    };

    let mentions = crate::feed::query_mentions(&f.pool, f.scope, &member, &[f.channel], None, 50)
        .await
        .expect("mentions");
    assert_eq!(ids(mentions), [new.clone()]);
    let activity =
        crate::feed::query_activity(&f.pool, f.scope, &[f.channel], None, 50, Some(&member))
            .await
            .expect("activity");
    assert_eq!(ids(activity), [new.clone()]);
    let owner = f.owner.public_key().to_bytes();
    let activity =
        crate::feed::query_activity(&f.pool, f.scope, &[f.channel], None, 50, Some(&owner))
            .await
            .expect("owner activity");
    assert_eq!(ids(activity).len(), 2, "owner keeps {old}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn room_state_from_before_the_join_stays_visible() {
    let f = Fixture::new(ChannelVisibility::Private).await;
    let state = EventBuilder::new(Kind::Custom(30078), "room settings")
        .tags([
            Tag::parse(["h", &f.channel.to_string()]).expect("h tag"),
            Tag::parse(["d", "settings"]).expect("d tag"),
        ])
        .sign_with_keys(&f.owner)
        .expect("fixture signature");
    event::insert_event(&f.pool, f.scope, &state, Some(f.channel))
        .await
        .expect("real event writer");
    let old = f.send("old message").await;
    f.join().await;
    let new = f.send("new message").await;

    for one_channel in [true, false] {
        let mut q = EventQuery::for_community(f.scope);
        q.channel_id = one_channel.then_some(f.channel);
        q.history_reader = Some(f.member.public_key().to_bytes().to_vec());
        let mut ids: Vec<String> = event::query_events(&f.pool, &q)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.event.id.to_hex())
            .collect();
        ids.sort();
        let mut expected = vec![state.id.to_hex(), new.clone()];
        expected.sort();
        // The room's own creation/system rows may also be state; the old
        // message never is.
        assert!(!ids.contains(&old), "one_channel={one_channel}");
        assert!(
            expected.iter().all(|id| ids.contains(id)),
            "one_channel={one_channel}"
        );
        // A kinds filter of messages only skips the state exemption.
        q.kinds = Some(vec![9]);
        assert_eq!(event::count_events(&f.pool, &q).await.unwrap(), 1);
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_reader_without_current_membership_sees_nothing() {
    // A replica that has not seen the join yet, or a stale removed row.
    let f = Fixture::new(ChannelVisibility::Private).await;
    f.send("before").await;
    assert_eq!(f.read(Some(&f.member)).await, (vec![], 0));
    f.join().await;
    let after = f.send("after").await;
    assert_eq!(f.read(Some(&f.member)).await, (vec![after], 1));
    f.leave().await;
    assert_eq!(f.read(Some(&f.member)).await, (vec![], 0));
    // Policy 'all': no cut at all.
    f.set("UPDATE channels SET history_policy='all' WHERE community_id=$1 AND id=$2")
        .await;
    assert_eq!(f.read(Some(&f.member)).await.1, 2);
}

impl Fixture {
    /// A kind:9 reply to `root` copied to the main timeline ("also send to
    /// channel").
    async fn broadcast_reply(&self, root: &str, content: &str) -> String {
        let root_id = nostr::EventId::from_hex(root).expect("root id");
        let event = EventBuilder::new(Kind::Custom(9), content)
            .tags([
                Tag::parse(["h", &self.channel.to_string()]).expect("h tag"),
                Tag::parse(["e", root, "", "reply"]).expect("e tag"),
            ])
            .sign_with_keys(&self.owner)
            .expect("fixture signature");
        event::insert_event(&self.pool, self.scope, &event, Some(self.channel))
            .await
            .expect("real event writer");
        let created = |id: &[u8]| {
            let pool = self.pool.clone();
            let scope = self.scope;
            let id = id.to_vec();
            async move {
                sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
                    "SELECT created_at FROM events WHERE community_id = $1 AND id = $2",
                )
                .bind(scope.as_uuid())
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("row")
            }
        };
        let root_at = created(root_id.as_bytes()).await;
        let at = created(event.id.as_bytes()).await;
        crate::thread::insert_thread_metadata(
            &self.pool,
            self.scope,
            event.id.as_bytes(),
            at,
            self.channel,
            Some(root_id.as_bytes()),
            Some(root_at),
            Some(root_id.as_bytes()),
            Some(root_at),
            1,
            true,
        )
        .await
        .expect("thread index");
        event.id.to_hex()
    }

    /// A kind:9 message from the owner quoting `quoted` (`q` tag).
    async fn quote(&self, quoted: &str, content: &str) -> String {
        let message = EventBuilder::new(Kind::Custom(9), content)
            .tags([
                Tag::parse(["h", &self.channel.to_string()]).expect("h tag"),
                Tag::parse(["q", quoted, ""]).expect("q tag"),
            ])
            .sign_with_keys(&self.owner)
            .expect("fixture signature");
        event::insert_event(&self.pool, self.scope, &message, Some(self.channel))
            .await
            .expect("real event writer");
        message.id.to_hex()
    }
}

fn id_bytes(hex_id: &str) -> Vec<u8> {
    nostr::EventId::from_hex(hex_id)
        .unwrap()
        .to_bytes()
        .to_vec()
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn replies_in_an_old_thread_are_cut_with_their_root() {
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    let root = f.send("old root").await;
    f.join().await;
    let late = f.reply(&root, 9, "late reply in old thread").await;
    let copy = f.broadcast_reply(&root, "late copy to channel").await;
    let reaction = f.reply(&late, 7, "+").await;
    let fresh = f.send("new message").await;
    let member = f.member.public_key().to_bytes().to_vec();
    let owner = f.owner.public_key().to_bytes().to_vec();

    // Stored on the reply rows (migration 0089 trigger).
    let stamped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE community_id = $1 AND channel_id = $2 \
          AND thread_root_sequence > 0",
    )
    .bind(f.scope.as_uuid())
    .bind(f.channel)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(stamped, 2, "late reply and its channel copy");

    // One channel (index path) and many channels (per-row path).
    assert_eq!(f.read(Some(&f.member)).await, (vec![fresh.clone()], 1));
    let mut q = EventQuery::for_community(f.scope);
    q.kinds = Some(vec![7, 9]);
    q.history_reader = Some(member.clone());
    let ids: Vec<String> = event::query_events(&f.pool, &q)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.event.id.to_hex())
        .collect();
    assert_eq!(ids, [fresh.clone()]);

    // Id lookups, live delivery and push wakes agree.
    let hidden = db
        .history_hidden_event_ids(f.scope, &member, &[id_bytes(&late), id_bytes(&copy)])
        .await
        .unwrap();
    assert_eq!(hidden.len(), 2);
    let blind = db
        .readers_blind_to_targets(
            f.scope,
            f.channel,
            &[id_bytes(&late), id_bytes(&reaction)],
            &[member.clone(), owner.clone()],
            false,
        )
        .await
        .unwrap();
    assert_eq!(blind, [member.clone()]);
    // A deletion of an old and a new message still reaches the member.
    for (targets, expected) in [
        (vec![id_bytes(&late), id_bytes(&fresh)], vec![]),
        (vec![id_bytes(&late)], vec![member.clone()]),
    ] {
        let blind = db
            .readers_blind_to_targets(f.scope, f.channel, &targets, &[member.clone()], true)
            .await
            .unwrap();
        assert_eq!(blind, expected);
    }
    let deletion = |targets: &[&str]| {
        let tags: Vec<Vec<String>> = targets
            .iter()
            .map(|t| vec!["e".to_string(), t.to_string()])
            .collect();
        serde_json::json!(tags)
    };
    for (targets, hidden) in [
        (deletion(&[&late, &fresh]), false),
        (deletion(&[&late]), true),
    ] {
        let before: bool =
            sqlx::query_scalar("SELECT channel_event_targets_before_join($1, $2, 5, $3, $4)")
                .bind(f.scope.as_uuid())
                .bind(f.channel)
                .bind(targets)
                .bind(&member)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(before, hidden);
    }
    let pairs = db
        .history_hidden_pairs(
            f.scope,
            &[id_bytes(&copy), id_bytes(&copy), id_bytes(&fresh)],
            &[member.clone(), owner.clone(), member.clone()],
        )
        .await
        .unwrap();
    assert_eq!(pairs, [(id_bytes(&copy), member.clone())]);

    // The channel copy never counts as unread.
    let (unread, _): (i32, i32) =
        sqlx::query_as("SELECT unread, mentions FROM chat_room_unread($1, $2, $3, 0)")
            .bind(f.scope.as_uuid())
            .bind(f.channel)
            .bind(&member)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(unread, 1, "only the new message");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn switching_to_since_join_keeps_what_current_members_see() {
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    db.set_channel_history_policy(f.scope, f.channel, "all")
        .await
        .unwrap();
    let before = f.send("before join").await;
    f.join().await;
    let after = f.send("after join").await;
    let everything = (vec![before.clone(), after.clone()], 2);
    assert_eq!(f.read(Some(&f.member)).await, everything);

    db.set_channel_history_policy(f.scope, f.channel, "since_join")
        .await
        .unwrap();
    assert_eq!(
        db.channel_history_policy(f.scope, f.channel).await.unwrap(),
        "since_join"
    );
    // The current member keeps everything; a later rejoin is cut.
    assert_eq!(f.read(Some(&f.member)).await, everything);
    f.leave().await;
    f.join().await;
    let later = f.send("after rejoin").await;
    assert_eq!(f.read(Some(&f.member)).await, (vec![later], 1));
    assert!(db
        .set_channel_history_policy(f.scope, f.channel, "sometimes")
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn group_chats_always_cut_history() {
    let f = Fixture::new(ChannelVisibility::Private).await;
    f.set(
        "UPDATE channels SET group_chat = true, history_policy = 'all' \
          WHERE community_id = $1 AND id = $2",
    )
    .await;
    f.send("before join").await;
    f.join().await;
    let after = f.send("after join").await;
    assert_eq!(f.read(Some(&f.member)).await, (vec![after], 1));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn quote_preview_serves_a_pre_join_original_only_through_a_visible_quote() {
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    let original = f.send("said before you joined").await;
    let other = f.send("not quoted").await;
    f.join().await;
    let quoting = f.quote(&original, "look at this").await;
    let plain = f.send("no quote").await;
    let member = f.member.public_key().to_bytes().to_vec();

    // The original itself stays hidden.
    assert_eq!(f.read(Some(&f.member)).await.1, 2);
    let preview = db
        .quote_preview(
            f.scope,
            f.channel,
            &member,
            &id_bytes(&quoting),
            &id_bytes(&original),
        )
        .await
        .unwrap()
        .expect("preview");
    assert_eq!(preview.content, "said before you joined");
    assert_eq!(preview.author, f.owner.public_key().to_bytes().to_vec());
    // Not the quoted id, not a quote, or a reader outside the room: nothing.
    for (by, about, reader) in [
        (&quoting, &other, member.clone()),
        (&plain, &original, member.clone()),
        (
            &quoting,
            &original,
            Keys::generate().public_key().to_bytes().to_vec(),
        ),
    ] {
        assert!(db
            .quote_preview(f.scope, f.channel, &reader, &id_bytes(by), &id_bytes(about))
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn share_a_room_needs_both_current_members() {
    use crate::Db;

    let f = Fixture::new(ChannelVisibility::Private).await;
    let db = Db::from_pool(f.pool.clone());
    let owner = f.owner.public_key().to_bytes().to_vec();
    let member = f.member.public_key().to_bytes().to_vec();
    assert!(!db.share_a_room(f.scope, &owner, &member).await.unwrap());
    f.join().await;
    assert!(db.share_a_room(f.scope, &owner, &member).await.unwrap());
    assert!(db.share_a_room(f.scope, &member, &owner).await.unwrap());
    f.leave().await;
    assert!(!db.share_a_room(f.scope, &owner, &member).await.unwrap());

    // Open rooms don't count.
    let open = Fixture::new(ChannelVisibility::Open).await;
    open.join().await;
    let (owner, member) = (
        open.owner.public_key().to_bytes().to_vec(),
        open.member.public_key().to_bytes().to_vec(),
    );
    assert!(!Db::from_pool(open.pool.clone())
        .share_a_room(open.scope, &owner, &member)
        .await
        .unwrap());
}
