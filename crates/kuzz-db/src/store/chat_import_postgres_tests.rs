//! Chats S2b first-run upload (`chat_import`), `mark_read` by time, and the
//! write-path origin rules, against state written by the real message insert
//! transaction and the real backfill.
use std::cell::Cell;

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::chat_backfill::ChatBackfillOptions;
use crate::chat_import::{
    ChatImportOutcome, ImportFollow, ImportRead, ImportSkip, MAX_CHAT_IMPORT_ITEMS,
};
use crate::chat_user_state::{FollowState, MarkReadItem, MarkReadTarget, ReadPosition};
use crate::event::ThreadMetadataParams;
use crate::{channel, channel_members, Db};

struct Fixture {
    pool: PgPool,
    db: Db,
    /// Writes messages with the chat-state switch off: history from before
    /// the switch, which only the backfill turns into state.
    history: Db,
    live: Cell<bool>,
    scope: CommunityId,
}

/// Stored follow row: state, follow origin, read position, read origin.
type FollowRow = (String, String, Option<i64>, Option<String>);

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

fn id(event: &Event) -> Vec<u8> {
    event.id.as_bytes().to_vec()
}

fn at(secs: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs as i64, 0).expect("time")
}

fn room_read(room: Uuid, secs: u64) -> ImportRead {
    ImportRead {
        target: MarkReadTarget::Channel(room),
        at: at(secs),
    }
}

fn thread_read(root: &Event, secs: u64) -> ImportRead {
    ImportRead {
        target: MarkReadTarget::Thread(id(root)),
        at: at(secs),
    }
}

fn follow(root: &[u8], state: FollowState) -> ImportFollow {
    ImportFollow {
        root: root.to_vec(),
        state,
    }
}

fn skipped(outcome: &[crate::chat_import::ImportItemResult]) -> Vec<Option<ImportSkip>> {
    outcome.iter().map(|r| r.skipped).collect()
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
            .bind(format!("chat-import-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()).with_chat_state_writes(true),
            history: Db::from_pool(pool.clone()).with_chat_state_writes(false),
            live: Cell::new(true),
            pool,
            scope,
        }
    }

    /// Messages from now on are history (switch off) until [`Self::go_live`].
    fn start_history(&self) {
        self.live.set(false);
    }

    fn go_live(&self) {
        self.live.set(true);
    }

    fn writer(&self) -> &Db {
        if self.live.get() {
            &self.db
        } else {
            &self.history
        }
    }

    async fn room(&self, owner: &Keys, members: &[&Keys]) -> Uuid {
        let id = channel::create_channel(
            &self.pool,
            self.scope,
            "room",
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

    fn sign(&self, room: Uuid, keys: &Keys, ts: u64, tags: Vec<Tag>) -> Event {
        let mut all = vec![Tag::parse(["h", &room.to_string()]).expect("h tag")];
        all.extend(tags);
        EventBuilder::new(Kind::Custom(9), format!("message at {ts}"))
            .tags(all)
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("signature")
    }

    async fn post(&self, room: Uuid, keys: &Keys, ts: u64) -> Event {
        let event = self.sign(room, keys, ts, Vec::new());
        self.writer()
            .insert_event_with_thread_metadata_and_chat_state(self.scope, &event, Some(room), None)
            .await
            .expect("message write");
        event
    }

    async fn reply(&self, room: Uuid, root: &Event, keys: &Keys, ts: u64, tags: Vec<Tag>) -> Event {
        let mut tags = tags;
        tags.push(Tag::parse(["e", &root.id.to_hex(), "", "root"]).expect("e tag"));
        let event = self.sign(room, keys, ts, tags);
        let root_at = at(root.created_at.as_secs());
        let meta = ThreadMetadataParams {
            event_id: event.id.as_bytes(),
            event_created_at: at(ts),
            channel_id: room,
            parent_event_id: Some(root.id.as_bytes()),
            parent_event_created_at: Some(root_at),
            root_event_id: Some(root.id.as_bytes()),
            root_event_created_at: Some(root_at),
            depth: 1,
            broadcast: false,
        };
        self.writer()
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

    /// Backfill `room`; later messages are live.
    async fn backfill(&self, room: Uuid) {
        self.db
            .backfill_chat_state_channel(self.scope, room, &ChatBackfillOptions::from_now(false))
            .await
            .expect("backfill");
        self.go_live();
    }

    async fn member_read(&self, reader: &Keys, room: Uuid) -> (Option<i64>, Option<String>) {
        sqlx::query_as(
            "SELECT read_seq, read_origin FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
        )
        .bind(self.scope.as_uuid())
        .bind(room)
        .bind(pk(reader))
        .fetch_one(&self.pool)
        .await
        .expect("member row")
    }

    async fn follow_row(&self, reader: &Keys, root: &Event) -> Option<FollowRow> {
        sqlx::query_as(
            "SELECT follow_state, follow_origin, read_seq, read_origin FROM thread_follows \
             WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3",
        )
        .bind(self.scope.as_uuid())
        .bind(id(root))
        .bind(pk(reader))
        .fetch_optional(&self.pool)
        .await
        .expect("follow row")
    }

    async fn import(
        &self,
        reader: &Keys,
        reads: &[ImportRead],
        follows: &[ImportFollow],
    ) -> ChatImportOutcome {
        self.db
            .chat_import(self.scope, &reader.public_key(), reads, follows)
            .await
            .expect("chat import")
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn room_read_lowers_or_raises_a_backfilled_position_once() {
    let f = Fixture::new().await;
    f.start_history();
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let mut posts = Vec::new();
    for ts in 100..104 {
        posts.push(f.post(room, &other, ts).await);
    }
    f.backfill(room).await;
    let latest = f.seq(&posts[3]).await;
    assert_eq!(
        f.member_read(&reader, room).await,
        (Some(latest), Some("backfill".into()))
    );

    // Lower: read only up to the second message (a time between messages
    // resolves to the last message before it).
    let outcome = f.import(&reader, &[room_read(room, 101)], &[]).await;
    let second = f.seq(&posts[1]).await;
    assert_eq!(outcome.reads[0].skipped, None);
    assert_eq!(outcome.reads[0].read_seq, Some(second));
    assert_eq!(outcome.reads[0].channel_id, Some(room));
    assert!(outcome.any_applied());
    assert_eq!(
        f.member_read(&reader, room).await,
        (Some(second), Some("user".into()))
    );

    // Once only: a second upload (even forward) changes nothing.
    let outcome = f.import(&reader, &[room_read(room, 103)], &[]).await;
    assert_eq!(skipped(&outcome.reads), vec![Some(ImportSkip::AlreadySet)]);
    assert!(!outcome.any_applied());
    assert_eq!(f.member_read(&reader, room).await.0, Some(second));

    // Raise: a backfilled position behind newer messages moves forward.
    let (late, other_room) = (Keys::generate(), f.room(&other, &[]).await);
    channel_members::add_member(
        &f.pool,
        f.scope,
        other_room,
        &pk(&late),
        MemberRole::Member,
        None,
    )
    .await
    .expect("member");
    f.post(other_room, &other, 200).await;
    f.backfill(other_room).await;
    let newer = f.post(other_room, &other, 201).await;
    let outcome = f.import(&late, &[room_read(other_room, 999)], &[]).await;
    assert_eq!(outcome.reads[0].read_seq, Some(f.seq(&newer).await));

    // A time before any message means nothing read.
    let fresh = Keys::generate();
    channel_members::add_member(
        &f.pool,
        f.scope,
        room,
        &pk(&fresh),
        MemberRole::Member,
        None,
    )
    .await
    .expect("member");
    let outcome = f.import(&fresh, &[room_read(room, 50)], &[]).await;
    assert_eq!(
        outcome.reads[0].read_seq,
        Some(0),
        "no position yet: applies"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn the_readers_own_reads_and_posts_win_over_an_upload() {
    let f = Fixture::new().await;
    f.start_history();
    let (reader, poster, other) = (Keys::generate(), Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader, &poster]).await;
    let first = f.post(room, &other, 100).await;
    f.post(room, &other, 101).await;
    f.backfill(room).await;

    // An explicit read makes the position the reader's own.
    f.db.chat_mark_read(
        f.scope,
        &reader.public_key(),
        &[MarkReadItem {
            target: MarkReadTarget::Channel(room),
            position: Some(ReadPosition::Seq(1)),
        }],
        false,
    )
    .await
    .expect("mark")
    .expect("allowed");
    assert_eq!(
        f.member_read(&reader, room).await.1.as_deref(),
        Some("user")
    );
    let outcome = f.import(&reader, &[room_read(room, 100)], &[]).await;
    assert_eq!(skipped(&outcome.reads), vec![Some(ImportSkip::AlreadySet)]);

    // Posting a top-level message is the poster's own read too.
    let own = f.post(room, &poster, 102).await;
    assert_eq!(
        f.member_read(&poster, room).await,
        (Some(f.seq(&own).await), Some("user".into()))
    );
    let outcome = f.import(&poster, &[room_read(room, 100)], &[]).await;
    assert_eq!(skipped(&outcome.reads), vec![Some(ImportSkip::AlreadySet)]);
    assert_eq!(
        f.member_read(&poster, room).await.0,
        Some(f.seq(&own).await)
    );
    assert!(f.seq(&first).await < f.seq(&own).await);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn mark_read_at_resolves_a_time_and_only_moves_forward() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let root = f.post(room, &other, 100).await;
    let a = f.post(room, &other, 101).await;
    let b = f.post(room, &other, 102).await;
    f.reply(room, &root, &reader, 103, Vec::new()).await;
    let r2 = f.reply(room, &root, &other, 104, Vec::new()).await;
    let r3 = f.reply(room, &root, &other, 105, Vec::new()).await;
    let mark = |target: MarkReadTarget, secs: u64| {
        let (db, scope, key) = (f.db.clone(), f.scope, reader.public_key());
        async move {
            db.chat_mark_read(
                scope,
                &key,
                &[MarkReadItem {
                    target,
                    position: Some(ReadPosition::At(at(secs))),
                }],
                false,
            )
            .await
            .expect("mark")
            .expect("allowed")[0]
                .read_seq
        }
    };
    assert_eq!(
        mark(MarkReadTarget::Channel(room), 101).await,
        Some(f.seq(&a).await)
    );
    assert_eq!(
        mark(MarkReadTarget::Channel(room), 100).await,
        Some(f.seq(&a).await),
        "never moves back"
    );
    assert_eq!(
        mark(MarkReadTarget::Channel(room), 102).await,
        Some(f.seq(&b).await)
    );
    // A thread time counts only the thread's root and replies.
    assert_eq!(
        mark(MarkReadTarget::Thread(id(&root)), 104).await,
        Some(f.seq(&r2).await)
    );
    assert_eq!(
        mark(MarkReadTarget::Thread(id(&root)), 999).await,
        Some(f.seq(&r3).await)
    );
    assert_eq!(
        f.follow_row(&reader, &root)
            .await
            .expect("row")
            .3
            .as_deref(),
        Some("user")
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn thread_read_uses_the_later_of_thread_and_room_times() {
    let f = Fixture::new().await;
    f.start_history();
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let mention = Tag::parse(["p", &reader.public_key().to_hex()]).expect("p tag");
    let root = f.post(room, &other, 100).await;
    let quiet = f.post(room, &other, 101).await;
    let r1 = f
        .reply(room, &root, &other, 102, vec![mention.clone()])
        .await;
    let r2 = f.reply(room, &root, &other, 103, Vec::new()).await;
    f.reply(room, &root, &other, 104, Vec::new()).await;
    // Backfilled rows from before `read_origin` existed count as backfill.
    f.backfill(room).await;
    sqlx::query("UPDATE thread_follows SET read_origin = NULL WHERE community_id = $1")
        .bind(f.scope.as_uuid())
        .execute(&f.pool)
        .await
        .expect("legacy row");
    let row = f
        .follow_row(&reader, &root)
        .await
        .expect("backfilled follow");
    assert_eq!((row.0.as_str(), row.1.as_str()), ("auto", "backfill"));

    // The thread was read at 102 on this device, the room at 103: the room
    // time wins for the thread too.
    let outcome = f
        .import(
            &reader,
            &[thread_read(&root, 102), room_read(room, 103)],
            &[],
        )
        .await;
    assert_eq!(outcome.reads[0].read_seq, Some(f.seq(&r2).await));
    assert_eq!(outcome.reads[1].read_seq, Some(f.seq(&r2).await));
    assert_eq!(
        f.follow_row(&reader, &root)
            .await
            .expect("row")
            .3
            .as_deref(),
        Some("user")
    );
    assert!(f.seq(&r1).await < f.seq(&r2).await && f.seq(&quiet).await < f.seq(&r1).await);
    let outcome = f.import(&reader, &[thread_read(&root, 102)], &[]).await;
    assert_eq!(skipped(&outcome.reads), vec![Some(ImportSkip::AlreadySet)]);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn follows_overwrite_only_backfilled_rows_and_fill_missing_ones() {
    let f = Fixture::new().await;
    f.start_history();
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let mention = Tag::parse(["p", &reader.public_key().to_hex()]).expect("p tag");
    let backfilled = f.post(room, &other, 100).await;
    f.reply(room, &backfilled, &other, 101, vec![mention.clone()])
        .await;
    let missing = f.post(room, &other, 102).await;
    let missing_last = f.reply(room, &missing, &other, 103, Vec::new()).await;
    let read_too = f.post(room, &other, 104).await;
    let read_too_first = f.reply(room, &read_too, &other, 105, Vec::new()).await;
    f.reply(room, &read_too, &other, 106, Vec::new()).await;
    f.backfill(room).await;
    // A live mention after the backfill: the server's own row.
    let live = f.post(room, &other, 107).await;
    f.reply(room, &live, &other, 108, vec![mention]).await;
    assert_eq!(
        f.follow_row(&reader, &live).await.expect("live row").1,
        "live"
    );

    let outcome = f
        .import(
            &reader,
            &[thread_read(&read_too, 105)],
            &[
                follow(&id(&backfilled), FollowState::Unfollowed),
                follow(&id(&missing), FollowState::Followed),
                follow(&id(&live), FollowState::Unfollowed),
                follow(&id(&read_too), FollowState::Followed),
            ],
        )
        .await;
    assert_eq!(
        skipped(&outcome.follows),
        vec![None, None, Some(ImportSkip::AlreadySet), None]
    );
    assert_eq!(outcome.reads[0].skipped, None, "{outcome:?}");
    let row = f.follow_row(&reader, &backfilled).await.expect("row");
    assert_eq!((row.0.as_str(), row.1.as_str()), ("unfollowed", "user"));
    assert_eq!(
        row.3.as_deref(),
        Some("backfill"),
        "its read position stays correctable"
    );
    assert_eq!(
        f.follow_row(&reader, &missing).await,
        Some((
            "followed".into(),
            "user".into(),
            Some(f.seq(&missing_last).await),
            Some("user".into())
        )),
        "a new row starts caught up"
    );
    assert_eq!(
        f.follow_row(&reader, &read_too).await.expect("row").2,
        Some(f.seq(&read_too_first).await),
        "a new row takes the same upload's read position"
    );
    assert_eq!(
        f.follow_row(&reader, &live).await.expect("row").0,
        "auto",
        "a live row is never overwritten"
    );

    // Once only.
    let outcome = f
        .import(
            &reader,
            &[],
            &[follow(&id(&backfilled), FollowState::Followed)],
        )
        .await;
    assert_eq!(
        skipped(&outcome.follows),
        vec![Some(ImportSkip::AlreadySet)]
    );
    assert_eq!(
        f.follow_row(&reader, &backfilled).await.expect("row").0,
        "unfollowed"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn items_that_cannot_apply_are_skipped_one_by_one() {
    let f = Fixture::new().await;
    let other_tenant = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let closed_room = f.room(&other, &[]).await;
    let theirs = other_tenant.room(&other, &[&reader]).await;
    let root = f.post(room, &other, 100).await;
    let reply = f.reply(room, &root, &other, 101, Vec::new()).await;
    let hidden = f.post(closed_room, &other, 102).await;
    let foreign_root = other_tenant.post(theirs, &other, 103).await;
    // A deleted message and a non-message kind are not thread roots.
    let deleted = f.post(room, &other, 104).await;
    sqlx::query("UPDATE events SET deleted_at = NOW() WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(id(&deleted))
        .execute(&f.pool)
        .await
        .expect("delete root");
    let reaction = EventBuilder::new(Kind::Custom(7), "+")
        .tags([Tag::parse(["h", &room.to_string()]).expect("h tag")])
        .custom_created_at(Timestamp::from(105))
        .sign_with_keys(&other)
        .expect("signature");
    f.db.insert_event_with_thread_metadata_and_chat_state(f.scope, &reaction, Some(room), None)
        .await
        .expect("reaction write");

    let outcome = f
        .import(
            &reader,
            &[
                room_read(closed_room, 200),
                room_read(theirs, 200),
                thread_read(&foreign_root, 200),
                thread_read(&root, 200),
                thread_read(&reply, 200),
                thread_read(&deleted, 200),
                room_read(room, 200),
            ],
            &[
                follow(&id(&hidden), FollowState::Followed),
                follow(&id(&foreign_root), FollowState::Followed),
                follow(&id(&reply), FollowState::Followed),
                follow(&[9; 32], FollowState::Followed),
                follow(&id(&deleted), FollowState::Followed),
                follow(&id(&reaction), FollowState::Followed),
            ],
        )
        .await;
    assert_eq!(
        skipped(&outcome.reads),
        vec![
            Some(ImportSkip::NotMember),
            Some(ImportSkip::NotMember),
            Some(ImportSkip::UnknownThread),
            Some(ImportSkip::NotFollowed),
            Some(ImportSkip::NotThreadRoot),
            Some(ImportSkip::UnknownThread),
            None,
        ]
    );
    assert_eq!(
        skipped(&outcome.follows),
        vec![
            Some(ImportSkip::NotMember),
            Some(ImportSkip::UnknownThread),
            Some(ImportSkip::NotThreadRoot),
            Some(ImportSkip::UnknownThread),
            Some(ImportSkip::UnknownThread),
            Some(ImportSkip::UnknownThread),
        ]
    );
    // The explicit follow request uses the same root rule.
    for gone in [&deleted, &reaction] {
        assert_eq!(
            f.db.chat_thread_follow(
                f.scope,
                &reader.public_key(),
                &id(gone),
                FollowState::Followed
            )
            .await
            .expect("follow"),
            Err(crate::chat_user_state::ChatStateDenied::NotMember { index: 0 })
        );
    }
    assert_eq!(outcome.follows[0].channel_id, Some(closed_room));
    assert!(f.follow_row(&reader, &hidden).await.is_none());
    assert_eq!(
        f.member_read(&reader, room).await.1.as_deref(),
        Some("user")
    );
    let theirs_read: Option<String> = sqlx::query_scalar(
        "SELECT read_origin FROM channel_members \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
    )
    .bind(other_tenant.scope.as_uuid())
    .bind(theirs)
    .bind(pk(&reader))
    .fetch_one(&f.pool)
    .await
    .expect("other tenant row");
    assert_eq!(theirs_read, None, "another community is never touched");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_reply_ends_backfill_for_its_thread() {
    let f = Fixture::new().await;
    f.start_history();
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let mention = Tag::parse(["p", &reader.public_key().to_hex()]).expect("p tag");
    let root = f.post(room, &other, 100).await;
    f.reply(room, &root, &other, 101, vec![mention]).await;
    f.backfill(room).await;
    assert_eq!(
        f.follow_row(&reader, &root)
            .await
            .expect("row")
            .3
            .as_deref(),
        Some("backfill")
    );

    let own = f.reply(room, &root, &reader, 102, Vec::new()).await;
    assert_eq!(
        f.follow_row(&reader, &root).await,
        Some((
            "auto".into(),
            "live".into(),
            Some(f.seq(&own).await),
            Some("user".into())
        ))
    );
    let outcome = f
        .import(
            &reader,
            &[thread_read(&root, 100)],
            &[follow(&id(&root), FollowState::Unfollowed)],
        )
        .await;
    assert_eq!(skipped(&outcome.reads), vec![Some(ImportSkip::AlreadySet)]);
    assert_eq!(
        skipped(&outcome.follows),
        vec![Some(ImportSkip::AlreadySet)]
    );
    assert_eq!(f.follow_row(&reader, &root).await.expect("row").0, "auto");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_room_time_matches_signer_time_including_late_arrivals() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    for ts in 100..104 {
        f.post(room, &other, ts).await;
    }
    // Signed at 101 but received last: the highest sequence at or before 101.
    let late = f.post(room, &other, 101).await;
    let outcome = f.import(&reader, &[room_read(room, 101)], &[]).await;
    assert_eq!(outcome.reads[0].read_seq, Some(f.seq(&late).await));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_full_upload_of_the_item_cap_applies_in_one_request() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let mut roots = Vec::new();
    for ts in 0..(MAX_CHAT_IMPORT_ITEMS as u64 - 1) {
        roots.push(f.post(room, &other, 1_000 + ts).await);
    }
    let follows: Vec<ImportFollow> = roots
        .iter()
        .map(|root| follow(&id(root), FollowState::Followed))
        .collect();
    let outcome = f.import(&reader, &[room_read(room, 1_000)], &follows).await;
    assert_eq!(
        outcome.reads.len() + outcome.follows.len(),
        MAX_CHAT_IMPORT_ITEMS
    );
    assert!(outcome
        .reads
        .iter()
        .chain(&outcome.follows)
        .all(|r| r.skipped.is_none()));
    assert_eq!(outcome.reads[0].read_seq, Some(f.seq(&roots[0]).await));
}
