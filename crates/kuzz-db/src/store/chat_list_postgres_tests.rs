//! Chats S2 requests (`chat_list`, `mark_read`, `thread_follow`) against
//! state written by the real S1 message insert transaction.
use chrono::{Duration, Utc};
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::chat_list::{ChatListCursor, ChatListQuery, ChatListRow, ChatRowKind, CHAT_UNREAD_CAP};
use crate::chat_state::{THREAD_CLOSED_TAG, THREAD_REOPENED_TAG};
use crate::chat_user_state::{
    ChatStateDenied, FollowState, MarkReadItem, MarkReadTarget, ReadPosition,
};
use crate::event::ThreadMetadataParams;
use crate::{channel, channel_members, dm, Db};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

fn p_tag(keys: &Keys) -> Tag {
    Tag::parse(["p", &keys.public_key().to_hex()]).expect("p tag")
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

fn root_id(event: &Event) -> Vec<u8> {
    event.id.as_bytes().to_vec()
}

fn query(limit: u32, pinned: &[&Event]) -> ChatListQuery {
    ChatListQuery {
        limit,
        before: None,
        pinned_threads: pinned.iter().map(|e| root_id(e)).collect(),
        quiet_before: Utc::now() - Duration::days(7),
    }
}

fn channel_item(room: Uuid, position: Option<ReadPosition>) -> MarkReadItem {
    MarkReadItem {
        target: MarkReadTarget::Channel(room),
        position,
    }
}

fn thread_item(root: &Event, position: Option<ReadPosition>) -> MarkReadItem {
    MarkReadItem {
        target: MarkReadTarget::Thread(root_id(root)),
        position,
    }
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
            .bind(format!("chat-list-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()).with_chat_state_writes(true),
            pool,
            scope,
        }
    }

    async fn room_named(&self, name: &str, owner: &Keys, members: &[&Keys]) -> Uuid {
        let id = channel::create_channel(
            &self.pool,
            self.scope,
            name,
            ChannelType::Stream,
            ChannelVisibility::Open,
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
                None,
            )
            .await
            .expect("member");
        }
        id
    }

    async fn room(&self, owner: &Keys, members: &[&Keys]) -> Uuid {
        self.room_named("room", owner, members).await
    }

    fn sign(&self, room: Uuid, keys: &Keys, ts: u64, tags: Vec<Tag>) -> Event {
        let mut all = vec![Tag::parse(["h", &room.to_string()]).expect("h tag")];
        all.extend(tags);
        EventBuilder::new(Kind::Custom(9), format!("message at {ts}"))
            .tags(all)
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("signature")
    }

    async fn post(&self, room: Uuid, keys: &Keys, ts: u64, tags: Vec<Tag>) -> Event {
        let event = self.sign(room, keys, ts, tags);
        self.db
            .insert_event_with_thread_metadata_and_chat_state(self.scope, &event, Some(room), None)
            .await
            .expect("message write");
        event
    }

    async fn reply_as(
        &self,
        room: Uuid,
        root: &Event,
        keys: &Keys,
        ts: u64,
        mut tags: Vec<Tag>,
        broadcast: bool,
    ) -> Event {
        tags.push(Tag::parse(["e", &root.id.to_hex(), "", "root"]).expect("e tag"));
        let event = self.sign(room, keys, ts, tags);
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
            broadcast,
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
        event
    }

    async fn reply(&self, room: Uuid, root: &Event, keys: &Keys, ts: u64, tags: Vec<Tag>) -> Event {
        self.reply_as(room, root, keys, ts, tags, false).await
    }

    async fn marker(&self, room: Uuid, root: &Event, keys: &Keys, ts: u64, tag: &str) -> Event {
        let marker = Tag::parse([tag, &root.id.to_hex()]).expect("marker tag");
        self.reply(room, root, keys, ts, vec![marker]).await
    }

    async fn seq(&self, event: &Event) -> i64 {
        sqlx::query_scalar(
            "SELECT channel_sequence FROM events WHERE community_id = $1 AND id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(event.id.as_bytes().as_slice())
        .fetch_one(&self.pool)
        .await
        .expect("event sequence")
    }

    async fn list(&self, reader: &Keys, query: &ChatListQuery) -> Vec<ChatListRow> {
        self.db
            .chat_list(self.scope, &reader.public_key(), query)
            .await
            .expect("chat list")
            .rows
    }

    async fn make_quiet(&self, root: &Event) {
        sqlx::query(
            "UPDATE thread_state SET last_at = NOW() - INTERVAL '8 days' \
             WHERE community_id = $1 AND root_event_id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(root.id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .expect("age thread");
    }

    async fn mark(&self, reader: &Keys, items: &[MarkReadItem], unread_mark: bool) {
        self.db
            .chat_mark_read(self.scope, &reader.public_key(), items, unread_mark)
            .await
            .expect("mark read")
            .expect("mark read allowed");
    }
}

fn room_row(rows: &[ChatListRow], room: Uuid) -> Option<&ChatListRow> {
    rows.iter()
        .find(|r| r.kind == ChatRowKind::Room && r.channel_id == room)
}

fn thread_row<'a>(rows: &'a [ChatListRow], root: &Event) -> Option<&'a ChatListRow> {
    let id = root_id(root);
    rows.iter()
        .find(|r| r.kind == ChatRowKind::Thread && r.root_id.as_deref() == Some(id.as_slice()))
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn counts_unread_and_mentions_from_the_sequence() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;

    let root = f.post(room, &reader, 100, Vec::new()).await;
    f.reply(room, &root, &other, 101, vec![p_tag(&reader)])
        .await;
    f.reply(room, &root, &other, 102, Vec::new()).await;
    f.reply(room, &root, &reader, 103, Vec::new()).await;
    f.reply(room, &root, &other, 104, Vec::new()).await;
    // Close and reopen markers are not messages.
    f.marker(room, &root, &other, 105, THREAD_CLOSED_TAG).await;
    f.marker(room, &root, &other, 106, THREAD_REOPENED_TAG)
        .await;
    // A broadcast reply counts on the main timeline (and in its thread).
    let broadcast = f
        .reply_as(room, &root, &other, 107, vec![p_tag(&reader)], true)
        .await;
    f.post(room, &other, 108, vec![p_tag(&reader)]).await;

    let rows = f.list(&reader, &query(50, &[])).await;
    let main = room_row(&rows, room).expect("room row");
    assert_eq!(main.channel_name, "room");
    assert_eq!(main.read_seq, Some(f.seq(&root).await), "own post read");
    assert_eq!(
        main.unread, 2,
        "broadcast + top-level; plain replies excluded"
    );
    assert_eq!(main.unread_mentions, 2);
    assert_eq!(
        main.latest.as_ref().expect("latest")["content"],
        "message at 108"
    );

    let thread = thread_row(&rows, &root).expect("thread row");
    assert!(thread.read_seq.is_some(), "reader replied");
    assert_eq!(
        thread.unread, 2,
        "replies after the reader's own reply, markers excluded: 104 and broadcast"
    );
    assert_eq!(
        thread.unread_mentions, 1,
        "the broadcast mentions the reader"
    );
    assert_eq!(thread.last_seq, Some(f.seq(&broadcast).await));
    assert_eq!(
        thread.latest.as_ref().expect("latest")["id"],
        broadcast.id.to_hex(),
        "latest reply skips markers"
    );
    assert!(!thread.closed);
    assert_eq!(thread.follow_state.as_deref(), Some("auto"));

    // Counts stop at the cap + 1 so callers can show "99+".
    let busy = f.room(&other, &[&reader]).await;
    for ts in 0..(CHAT_UNREAD_CAP as u64 + 5) {
        f.post(busy, &other, 200 + ts, vec![p_tag(&reader)]).await;
    }
    let rows = f.list(&reader, &query(50, &[])).await;
    let busy_row = room_row(&rows, busy).expect("busy row");
    assert_eq!(busy_row.unread, CHAT_UNREAD_CAP + 1);
    assert_eq!(busy_row.unread_mentions, CHAT_UNREAD_CAP + 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn threads_follow_close_quiet_and_pinned_rules() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;

    let live = f.post(room, &reader, 100, Vec::new()).await;
    f.reply(room, &live, &other, 101, Vec::new()).await;
    let name = EventBuilder::new(Kind::Custom(40009), "Launch plan")
        .tags([
            Tag::parse(["h", &room.to_string()]).expect("h"),
            Tag::parse(["e", &live.id.to_hex()]).expect("e"),
        ])
        .custom_created_at(Timestamp::from(102))
        .sign_with_keys(&other)
        .expect("name");
    f.db.insert_event(f.scope, &name, Some(room))
        .await
        .expect("thread name");

    let unfollowed = f.post(room, &reader, 110, Vec::new()).await;
    f.reply(room, &unfollowed, &other, 111, Vec::new()).await;
    f.db.chat_thread_follow(
        f.scope,
        &reader.public_key(),
        &root_id(&unfollowed),
        FollowState::Unfollowed,
    )
    .await
    .expect("unfollow")
    .expect("allowed");

    let closed = f.post(room, &reader, 120, Vec::new()).await;
    f.reply(room, &closed, &other, 121, Vec::new()).await;
    f.marker(room, &closed, &other, 122, THREAD_CLOSED_TAG)
        .await;

    // Quiet and read: hidden. Quiet but unread: shown.
    let quiet_read = f.post(room, &reader, 130, Vec::new()).await;
    f.reply(room, &quiet_read, &other, 131, Vec::new()).await;
    f.mark(
        &reader,
        &[thread_item(&quiet_read, Some(ReadPosition::Seq(i64::MAX)))],
        false,
    )
    .await;
    f.make_quiet(&quiet_read).await;
    let quiet_unread = f.post(room, &reader, 140, Vec::new()).await;
    f.reply(room, &quiet_unread, &other, 141, Vec::new()).await;
    f.make_quiet(&quiet_unread).await;

    // Someone else's thread the reader never joined.
    let foreign = f.post(room, &other, 150, Vec::new()).await;
    f.reply(room, &foreign, &other, 151, Vec::new()).await;

    let rows = f.list(&reader, &query(50, &[])).await;
    let shown = |root: &Event| thread_row(&rows, root).is_some();
    assert!(shown(&live));
    assert_eq!(
        thread_row(&rows, &live)
            .expect("live")
            .thread_name
            .as_deref(),
        Some("Launch plan")
    );
    assert!(!shown(&unfollowed), "unfollowed threads are left out");
    assert!(!shown(&closed), "closed threads are left out");
    assert!(!shown(&quiet_read), "quiet read threads are left out");
    assert!(shown(&quiet_unread), "quiet threads with unread stay");
    assert!(!shown(&foreign), "threads the reader does not follow");

    // Pinned threads come back whatever their state; closed ones say so.
    let rows = f
        .list(
            &reader,
            &query(50, &[&unfollowed, &closed, &quiet_read, &foreign]),
        )
        .await;
    for root in [&unfollowed, &closed, &quiet_read, &foreign] {
        assert!(thread_row(&rows, root).is_some(), "pinned thread listed");
    }
    assert!(thread_row(&rows, &closed).expect("closed").closed);
    assert_eq!(
        thread_row(&rows, &unfollowed)
            .expect("pinned")
            .follow_state
            .as_deref(),
        Some("unfollowed")
    );
    assert_eq!(
        thread_row(&rows, &foreign).expect("foreign").follow_state,
        None
    );
    assert_eq!(thread_row(&rows, &foreign).expect("foreign").unread, 1);

    // A new reply reopens the closed thread for every follower.
    f.reply(room, &closed, &other, 123, Vec::new()).await;
    let rows = f.list(&reader, &query(50, &[])).await;
    assert!(thread_row(&rows, &closed).is_some(), "reply reopens");

    // Leaving the room drops the room and its threads, pinned or not.
    channel_members::remove_member(&f.pool, f.scope, room, &pk(&reader), &pk(&other))
        .await
        .expect("remove");
    let rows = f.list(&reader, &query(50, &[&live])).await;
    assert!(rows.is_empty(), "removed room leaves nothing: {rows:?}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn rooms_skip_hidden_dms_and_keep_archived_until_read() {
    let f = Fixture::new().await;
    let (reader, friend, other) = (Keys::generate(), Keys::generate(), Keys::generate());

    let direct = dm::create_dm(
        &f.pool,
        f.scope,
        &[&pk(&reader), &pk(&friend)],
        &pk(&reader),
    )
    .await
    .expect("dm")
    .id;
    let rows = f.list(&reader, &query(50, &[])).await;
    let dm_row = room_row(&rows, direct).expect("dm row");
    assert_eq!(dm_row.channel_type, "dm");
    assert_eq!(dm_row.dm_members, vec![pk(&friend)]);

    dm::hide_dm(&f.pool, f.scope, direct, &pk(&reader))
        .await
        .expect("hide");
    assert!(room_row(&f.list(&reader, &query(50, &[])).await, direct).is_none());
    // An incoming message brings it back.
    f.post(direct, &friend, 100, Vec::new()).await;
    assert!(room_row(&f.list(&reader, &query(50, &[])).await, direct).is_some());

    let archived = f.room_named("old", &other, &[&reader]).await;
    let last = f.post(archived, &other, 110, Vec::new()).await;
    f.db.archive_channel(f.scope, archived)
        .await
        .expect("archive");
    let rows = f.list(&reader, &query(50, &[])).await;
    let row = room_row(&rows, archived).expect("archived room with unread stays");
    assert!(row.archived);
    assert_eq!(row.unread, 1);

    f.mark(
        &reader,
        &[channel_item(
            archived,
            Some(ReadPosition::Event(root_id(&last))),
        )],
        false,
    )
    .await;
    assert!(
        room_row(&f.list(&reader, &query(50, &[])).await, archived).is_none(),
        "read archived room leaves"
    );
    // Marking it unread brings it back.
    f.mark(&reader, &[channel_item(archived, None)], true).await;
    let rows = f.list(&reader, &query(50, &[])).await;
    assert!(room_row(&rows, archived).expect("marked").unread_mark);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn pages_newest_first_without_gaps_or_repeats() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let mut expected = Vec::new();
    for i in 0..4 {
        let room = f.room(&other, &[&reader]).await;
        f.post(room, &other, 100 + i, Vec::new()).await;
        expected.push(room.to_string());
        let root = f.post(room, &reader, 200 + i, Vec::new()).await;
        f.reply(room, &root, &other, 300 + i, Vec::new()).await;
        expected.push(root.id.to_hex());
    }

    let mut seen = Vec::new();
    let mut q = query(3, &[]);
    loop {
        let page =
            f.db.chat_list(f.scope, &reader.public_key(), &q)
                .await
                .expect("page");
        assert!(page.rows.len() <= 3);
        let mut previous: Option<(chrono::DateTime<Utc>, String)> =
            seen.last().map(|r: &ChatListRow| (r.at, r.key.clone()));
        for row in &page.rows {
            if let Some(prev) = previous {
                assert!((row.at, row.key.clone()) < prev, "strictly newest first");
            }
            previous = Some((row.at, row.key.clone()));
        }
        seen.extend(page.rows.iter().cloned());
        if !page.has_more {
            break;
        }
        let last = page.rows.last().expect("non-empty page");
        q.before = Some(ChatListCursor {
            at: last.at,
            key: last.key.clone(),
        });
    }
    let mut keys: Vec<String> = seen.iter().map(|r| r.key.clone()).collect();
    keys.sort();
    expected.sort();
    assert_eq!(keys, expected, "every room and thread exactly once");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn mark_read_merges_forward_and_is_atomic() {
    let f = Fixture::new().await;
    let (reader, other, outsider) = (Keys::generate(), Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let foreign_room = f.room(&other, &[]).await;
    let first = f.post(room, &other, 100, Vec::new()).await;
    let second = f.post(room, &other, 101, Vec::new()).await;
    let root = f.post(room, &reader, 102, Vec::new()).await;
    let reply = f.reply(room, &root, &other, 103, Vec::new()).await;
    let elsewhere = f.post(foreign_room, &other, 104, Vec::new()).await;
    let (first_seq, second_seq) = (f.seq(&first).await, f.seq(&second).await);
    let accepted: i64 = sqlx::query_scalar(
        "SELECT accepted_sequence FROM channels WHERE community_id = $1 AND id = $2",
    )
    .bind(f.scope.as_uuid())
    .bind(room)
    .fetch_one(&f.pool)
    .await
    .expect("accepted sequence");

    // Own top-level post already moved the room read position.
    let reader_pk = reader.public_key();
    let result =
        f.db.chat_mark_read(
            f.scope,
            &reader_pk,
            &[channel_item(room, Some(ReadPosition::Seq(first_seq)))],
            false,
        )
        .await
        .expect("mark")
        .expect("allowed");
    assert_eq!(
        result[0].read_seq,
        Some(f.seq(&root).await),
        "never moves back"
    );
    let origin: Option<String> = sqlx::query_scalar(
        "SELECT read_origin FROM channel_members \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
    )
    .bind(f.scope.as_uuid())
    .bind(room)
    .bind(pk(&reader))
    .fetch_one(&f.pool)
    .await
    .expect("origin");
    assert_eq!(origin.as_deref(), Some("user"));
    assert!(second_seq < f.seq(&root).await);

    // Positions past the channel's sequence are clamped.
    let result =
        f.db.chat_mark_read(
            f.scope,
            &reader_pk,
            &[
                channel_item(room, Some(ReadPosition::Seq(i64::MAX))),
                thread_item(&root, Some(ReadPosition::Event(root_id(&reply)))),
            ],
            true,
        )
        .await
        .expect("mark")
        .expect("allowed");
    assert_eq!(result[0].read_seq, Some(accepted));
    assert!(result[0].unread_mark && result[1].unread_mark);
    assert_eq!(result[1].read_seq, Some(f.seq(&reply).await));
    assert!(result[1].stored);

    // A refused item rolls back the whole request.
    let before = f.list(&reader, &query(50, &[])).await;
    for (items, denied) in [
        (
            vec![
                channel_item(room, None),
                channel_item(foreign_room, Some(ReadPosition::Seq(1))),
            ],
            ChatStateDenied::NotMember { index: 1 },
        ),
        (
            vec![
                channel_item(room, None),
                channel_item(room, Some(ReadPosition::Event(root_id(&elsewhere)))),
            ],
            ChatStateDenied::UnknownEvent { index: 1 },
        ),
        (
            vec![thread_item(
                &root,
                Some(ReadPosition::Event(root_id(&second))),
            )],
            ChatStateDenied::UnknownEvent { index: 0 },
        ),
    ] {
        let refused =
            f.db.chat_mark_read(f.scope, &reader_pk, &items, false)
                .await
                .expect("mark");
        assert_eq!(refused, Err(denied));
    }
    let after = f.list(&reader, &query(50, &[])).await;
    assert!(
        room_row(&after, room).expect("room").unread_mark,
        "the refused requests changed nothing"
    );
    assert_eq!(before.len(), after.len());

    // Outsiders cannot touch the room or its threads.
    let outsider_pk = outsider.public_key();
    for item in [channel_item(room, None), thread_item(&root, None)] {
        assert_eq!(
            f.db.chat_mark_read(f.scope, &outsider_pk, &[item], false)
                .await
                .expect("mark"),
            Err(ChatStateDenied::NotMember { index: 0 })
        );
    }

    // A thread with no follow row has nowhere to store a read position.
    let unjoined = f.post(room, &other, 105, Vec::new()).await;
    let result =
        f.db.chat_mark_read(
            f.scope,
            &reader_pk,
            &[thread_item(&unjoined, Some(ReadPosition::Seq(1)))],
            false,
        )
        .await
        .expect("mark")
        .expect("allowed");
    assert!(!result[0].stored);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn thread_follow_sets_user_state_for_members_only() {
    let f = Fixture::new().await;
    let (reader, other, outsider) = (Keys::generate(), Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let root = f.post(room, &other, 100, Vec::new()).await;
    let reply = f.reply(room, &root, &other, 101, Vec::new()).await;
    let state = |keys: Keys| {
        let (pool, scope, root) = (f.pool.clone(), f.scope, root_id(&root));
        async move {
            sqlx::query_as::<_, (String, String)>(
                "SELECT follow_state, follow_origin FROM thread_follows \
                 WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3",
            )
            .bind(scope.as_uuid())
            .bind(root)
            .bind(pk(&keys))
            .fetch_optional(&pool)
            .await
            .expect("follow row")
        }
    };

    let follow = |who: &Keys, id: Vec<u8>, s: FollowState| {
        let (db, scope, key) = (f.db.clone(), f.scope, who.public_key());
        async move {
            db.chat_thread_follow(scope, &key, &id, s)
                .await
                .expect("follow")
        }
    };
    assert_eq!(
        follow(&reader, root_id(&root), FollowState::Followed).await,
        Ok(room)
    );
    assert_eq!(
        state(reader.clone()).await,
        Some(("followed".into(), "user".into()))
    );
    assert!(thread_row(&f.list(&reader, &query(50, &[])).await, &root).is_some());
    let read_seq: Option<i64> = sqlx::query_scalar(
        "SELECT read_seq FROM thread_follows \
         WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3",
    )
    .bind(f.scope.as_uuid())
    .bind(root_id(&root))
    .bind(pk(&reader))
    .fetch_one(&f.pool)
    .await
    .expect("read position");
    assert_eq!(
        read_seq,
        Some(f.seq(&reply).await),
        "a new explicit follow starts caught up"
    );

    assert_eq!(
        follow(&reader, root_id(&root), FollowState::Unfollowed).await,
        Ok(room)
    );
    assert_eq!(
        state(reader.clone()).await,
        Some(("unfollowed".into(), "user".into()))
    );
    // A mention does not bring an unfollowed thread back.
    f.reply(room, &root, &other, 102, vec![p_tag(&reader)])
        .await;
    assert!(thread_row(&f.list(&reader, &query(50, &[])).await, &root).is_none());
    // Replying follows again.
    f.reply(room, &root, &reader, 103, Vec::new()).await;
    assert_eq!(state(reader.clone()).await.expect("row").0, "followed");

    assert_eq!(
        follow(&outsider, root_id(&root), FollowState::Followed).await,
        Err(ChatStateDenied::NotMember { index: 0 })
    );
    assert_eq!(state(outsider.clone()).await, None);
    assert_eq!(
        follow(&reader, root_id(&reply), FollowState::Followed).await,
        Err(ChatStateDenied::NotThreadRoot)
    );
    assert_eq!(
        follow(&reader, vec![7; 32], FollowState::Followed).await,
        Err(ChatStateDenied::NotMember { index: 0 })
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn requests_are_tenant_scoped() {
    let f = Fixture::new().await;
    let other_tenant = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let root = f.post(room, &reader, 100, Vec::new()).await;
    f.reply(room, &root, &other, 101, Vec::new()).await;
    let theirs = other_tenant.room(&other, &[&reader]).await;

    let rows = other_tenant.list(&reader, &query(50, &[&root])).await;
    assert_eq!(rows.len(), 1, "only the other tenant's room: {rows:?}");
    assert_eq!(rows[0].channel_id, theirs);

    let reader_pk = reader.public_key();
    assert_eq!(
        other_tenant
            .db
            .chat_mark_read(
                other_tenant.scope,
                &reader_pk,
                &[channel_item(room, None)],
                true
            )
            .await
            .expect("mark"),
        Err(ChatStateDenied::NotMember { index: 0 })
    );
    assert_eq!(
        other_tenant
            .db
            .chat_thread_follow(
                other_tenant.scope,
                &reader_pk,
                &root_id(&root),
                FollowState::Unfollowed
            )
            .await
            .expect("follow"),
        Err(ChatStateDenied::NotMember { index: 0 })
    );
    let rows = f.list(&reader, &query(50, &[])).await;
    assert!(!room_row(&rows, room).expect("room").unread_mark);
    assert_eq!(
        thread_row(&rows, &root)
            .expect("thread")
            .follow_state
            .as_deref(),
        Some("auto")
    );
}
