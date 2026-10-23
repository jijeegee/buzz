//! Thread search by name against state written by the real message insert
//! transaction and the thread-name trigger.
use chrono::{Duration, Utc};
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::chat_state::THREAD_CLOSED_TAG;
use crate::chat_user_state::FollowState;
use crate::event::ThreadMetadataParams;
use crate::thread_search::{ThreadSearchRow, MAX_THREAD_SEARCH_LIMIT};
use crate::{channel, channel_members, dm, Db, DbError};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

fn h_tag(room: Uuid) -> Tag {
    Tag::parse(["h", &room.to_string()]).expect("h tag")
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
            .bind(format!("thread-search-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()).with_chat_state_writes(true),
            pool,
            scope,
        }
    }

    async fn room(&self, owner: &Keys, members: &[&Keys], visibility: ChannelVisibility) -> Uuid {
        let id = channel::create_channel(
            &self.pool,
            self.scope,
            "room",
            ChannelType::Stream,
            visibility,
            None,
            &pk(owner),
            None,
        )
        .await
        .expect("channel")
        .id;
        for member in members {
            channel_members::add_member(
                &self.pool,
                self.scope,
                id,
                &pk(member),
                MemberRole::Member,
                Some(&pk(owner)),
            )
            .await
            .expect("member");
        }
        id
    }

    async fn post(&self, room: Uuid, keys: &Keys, ts: u64) -> Event {
        let event = EventBuilder::new(Kind::Custom(9), format!("message at {ts}"))
            .tags([h_tag(room)])
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("signature");
        self.db
            .insert_event_with_thread_metadata_and_chat_state(self.scope, &event, Some(room), None)
            .await
            .expect("message write");
        event
    }

    async fn reply(&self, room: Uuid, root: &Event, keys: &Keys, ts: u64, tags: Vec<Tag>) {
        let mut all = vec![
            h_tag(room),
            Tag::parse(["e", &root.id.to_hex(), "", "root"]).expect("e tag"),
        ];
        all.extend(tags);
        let event = EventBuilder::new(Kind::Custom(9), format!("reply at {ts}"))
            .tags(all)
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("signature");
        let root_at = chrono::DateTime::from_timestamp(root.created_at.as_secs() as i64, 0)
            .expect("root time");
        let meta = ThreadMetadataParams {
            event_id: event.id.as_bytes(),
            event_created_at: chrono::DateTime::from_timestamp(ts as i64, 0).expect("time"),
            channel_id: room,
            parent_event_id: Some(root.id.as_bytes()),
            parent_event_created_at: Some(root_at),
            root_event_id: Some(root.id.as_bytes()),
            root_event_created_at: Some(root_at),
            depth: 1,
            broadcast: false,
        };
        self.db
            .insert_event_with_thread_metadata_and_chat_state(
                self.scope,
                &event,
                Some(room),
                Some(meta),
            )
            .await
            .expect("reply write");
    }

    async fn name(&self, room: Uuid, root: &Event, keys: &Keys, ts: u64, name: &str) {
        let event = EventBuilder::new(Kind::Custom(40009), name)
            .tags([
                h_tag(room),
                Tag::parse(["e", &root.id.to_hex()]).expect("e"),
            ])
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("name");
        self.db
            .insert_event(self.scope, &event, Some(room))
            .await
            .expect("thread name");
    }

    /// A named thread with one reply, rooted by `keys`.
    async fn thread(&self, room: Uuid, keys: &Keys, ts: u64, name: &str) -> Event {
        let root = self.post(room, keys, ts).await;
        self.reply(room, &root, keys, ts + 1, Vec::new()).await;
        self.name(room, &root, keys, ts + 2, name).await;
        root
    }

    async fn search(&self, reader: &Keys, query: &str) -> Vec<ThreadSearchRow> {
        self.db
            .thread_search(self.scope, &reader.public_key(), query, 20)
            .await
            .expect("thread search")
    }
}

fn found(rows: &[ThreadSearchRow], root: &Event) -> Option<ThreadSearchRow> {
    rows.iter()
        .find(|row| row.root_id == root.id.as_bytes().as_slice())
        .cloned()
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn finds_closed_quiet_and_unfollowed_threads_by_any_part_of_the_name() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader], ChannelVisibility::Open).await;

    let closed = f.thread(room, &other, 100, "Launch plan Q3").await;
    let marker = Tag::parse([THREAD_CLOSED_TAG, &closed.id.to_hex()]).expect("marker");
    f.reply(room, &closed, &other, 110, vec![marker]).await;

    let quiet = f
        .thread(room, &reader, 200, "Release notes for launch")
        .await;
    sqlx::query(
        "UPDATE thread_state SET last_at = NOW() - INTERVAL '90 days' \
         WHERE community_id = $1 AND root_event_id = $2",
    )
    .bind(f.scope.as_uuid())
    .bind(quiet.id.as_bytes().as_slice())
    .execute(&f.pool)
    .await
    .expect("age thread");

    let unfollowed = f.thread(room, &reader, 300, "prelaunch checklist").await;
    f.db.chat_thread_follow(
        f.scope,
        &reader.public_key(),
        unfollowed.id.as_bytes(),
        FollowState::Unfollowed,
    )
    .await
    .expect("unfollow")
    .expect("allowed");

    let exact = f.thread(room, &other, 400, "launch").await;
    let unrelated = f.thread(room, &other, 500, "Budget").await;

    let rows = f.search(&reader, "LAUNCH").await;
    let closed_row = found(&rows, &closed).expect("closed thread found");
    assert!(closed_row.closed);
    assert_eq!(closed_row.name, "Launch plan Q3");
    assert_eq!(closed_row.channel_id, room);
    assert!(!found(&rows, &quiet).expect("quiet thread found").closed);
    assert!(found(&rows, &unfollowed).is_some(), "unfollowed found");
    assert!(found(&rows, &unrelated).is_none());

    // Exact, prefix, word start, middle.
    let order: Vec<_> = rows.iter().map(|row| row.root_id.clone()).collect();
    let ids: Vec<Vec<u8>> = [&exact, &closed, &quiet, &unfollowed]
        .iter()
        .map(|e| e.id.as_bytes().to_vec())
        .collect();
    assert_eq!(order, ids);
    assert_eq!(
        rows.iter().map(|row| row.match_rank).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );

    // A word in the middle, case-insensitive.
    let rows = f.search(&reader, "notes FOR").await;
    assert_eq!(rows.len(), 1);
    assert!(found(&rows, &quiet).is_some());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn never_shows_names_from_rooms_the_reader_is_not_in() {
    let f = Fixture::new().await;
    let (reader, other, third) = (Keys::generate(), Keys::generate(), Keys::generate());

    // Open and private rooms the reader never joined.
    let open = f.room(&other, &[], ChannelVisibility::Open).await;
    let private = f.room(&other, &[&third], ChannelVisibility::Private).await;
    let open_thread = f.thread(open, &other, 100, "secret plan").await;
    let private_thread = f.thread(private, &other, 110, "secret plan").await;

    // A DM between two other people.
    let others_dm = dm::create_dm(&f.pool, f.scope, &[&pk(&other), &pk(&third)], &pk(&other))
        .await
        .expect("dm")
        .id;
    let dm_thread = f.thread(others_dm, &other, 120, "secret plan").await;

    // A room the reader left.
    let left = f.room(&other, &[&reader], ChannelVisibility::Open).await;
    let left_thread = f.thread(left, &other, 130, "secret plan").await;
    channel_members::remove_member(&f.pool, f.scope, left, &pk(&reader), &pk(&other))
        .await
        .expect("remove");

    // The reader's own DM, then hidden.
    let own_dm = dm::create_dm(&f.pool, f.scope, &[&pk(&reader), &pk(&other)], &pk(&reader))
        .await
        .expect("dm")
        .id;
    let own_dm_thread = f.thread(own_dm, &other, 140, "secret plan").await;
    let rows = f.search(&reader, "secret").await;
    assert_eq!(rows.len(), 1, "only the reader's own DM: {rows:?}");
    let row = found(&rows, &own_dm_thread).expect("own DM thread");
    assert_eq!(row.channel_type, "dm");
    assert_eq!(row.dm_members, vec![pk(&other)]);
    dm::hide_dm(&f.pool, f.scope, own_dm, &pk(&reader))
        .await
        .expect("hide");

    let rows = f.search(&reader, "secret").await;
    for root in [
        &open_thread,
        &private_thread,
        &dm_thread,
        &left_thread,
        &own_dm_thread,
    ] {
        assert!(found(&rows, root).is_none(), "leaked: {rows:?}");
    }
    // The members still find theirs.
    assert_eq!(f.search(&third, "secret").await.len(), 2);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn skips_dm_threads_named_or_started_by_someone_the_reader_blocked() {
    let f = Fixture::new().await;
    let (reader, friend) = (Keys::generate(), Keys::generate());
    let direct = dm::create_dm(
        &f.pool,
        f.scope,
        &[&pk(&reader), &pk(&friend)],
        &pk(&reader),
    )
    .await
    .expect("dm")
    .id;
    let before = f.thread(direct, &friend, 100, "trip before").await;
    let own = f.thread(direct, &reader, 110, "trip own").await;
    let renamed = f.thread(direct, &reader, 120, "trip renamed").await;
    f.db.block_user(f.scope, &pk(&reader), &pk(&friend))
        .await
        .expect("block");
    let during = f.thread(direct, &friend, 130, "trip during").await;
    f.name(direct, &renamed, &friend, 140, "trip renamed by them")
        .await;

    let rows = f.search(&reader, "trip").await;
    assert!(found(&rows, &before).is_some(), "sent before the block");
    assert!(found(&rows, &own).is_some());
    assert!(found(&rows, &during).is_none(), "root hidden by the block");
    assert!(found(&rows, &renamed).is_none(), "name hidden by the block");
    // The blocked person still finds every thread.
    assert_eq!(f.search(&friend, "trip").await.len(), 4);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn limits_and_rejects_bad_requests() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader], ChannelVisibility::Open).await;
    for i in 0..4 {
        f.thread(room, &other, 100 + i * 10, &format!("topic {i}"))
            .await;
    }
    let rows =
        f.db.thread_search(f.scope, &reader.public_key(), "topic", 3)
            .await
            .expect("search");
    assert_eq!(rows.len(), 3);
    // Same rank: newest activity first.
    assert!(rows.windows(2).all(|w| w[0].at >= w[1].at));
    assert!(rows[0].at > Utc::now() - Duration::days(1));

    for (query, limit) in [
        ("", 5),
        ("   ", 5),
        ("a\0b", 5),
        (&"x".repeat(101)[..], 5),
        ("topic", 0),
        ("topic", MAX_THREAD_SEARCH_LIMIT + 1),
    ] {
        let result =
            f.db.thread_search(f.scope, &reader.public_key(), query, limit)
                .await;
        assert!(
            matches!(result, Err(DbError::InvalidData(_))),
            "{query:?} {limit}"
        );
    }
    // `%` and `_` are plain characters, not patterns.
    assert!(f.search(&reader, "%").await.is_empty());
    assert!(f.search(&reader, "_").await.is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn threads_from_before_the_join_are_not_found_in_since_join_rooms() {
    let f = Fixture::new().await;
    let (owner, late) = (Keys::generate(), Keys::generate());
    let room = f.room(&owner, &[], ChannelVisibility::Private).await;
    let old = f.thread(room, &owner, 100, "budget before").await;
    channel_members::add_member(
        &f.pool,
        f.scope,
        room,
        &pk(&late),
        MemberRole::Member,
        Some(&pk(&owner)),
    )
    .await
    .expect("late member");
    let new = f.thread(room, &owner, 200, "budget after").await;

    let rows = f.search(&late, "budget").await;
    assert!(found(&rows, &old).is_none(), "pre-join thread is cut");
    assert!(found(&rows, &new).is_some(), "post-join thread is found");

    sqlx::query("UPDATE channels SET history_policy = 'all' WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(room)
        .execute(&f.pool)
        .await
        .expect("full history room");
    let rows = f.search(&late, "budget").await;
    assert!(found(&rows, &old).is_some(), "full-history room shows it");
}
