//! Chats S1 write path against the real message insert transaction.
//!
//! Every test drives `Db::insert_event_with_thread_metadata_and_chat_state`,
//! the seam the relay ingest path calls, and reads the resulting rows back.
use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::chat_state::{ChatStateWriteOutcome, THREAD_CLOSED_TAG, THREAD_REOPENED_TAG};
use crate::event::ThreadMetadataParams;
use crate::{channel, channel_members, dm, Db};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

#[derive(Debug, PartialEq, Eq)]
struct Follow {
    state: String,
    origin: String,
    read_seq: Option<i64>,
}

#[derive(Debug)]
struct ThreadState {
    last_seq: Option<i64>,
    closed_at: Option<DateTime<Utc>>,
    closed_by: Option<Vec<u8>>,
}

fn at(secs: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs as i64, 0).expect("timestamp")
}

fn p_tag(keys: &Keys) -> Tag {
    Tag::parse(["p", &keys.public_key().to_hex()]).expect("p tag")
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

impl Fixture {
    async fn new(chat_state: bool) -> Self {
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
            .bind(format!("chat-state-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()).with_chat_state_writes(chat_state),
            pool,
            scope,
        }
    }

    /// A stream room owned by `owner` with the given extra members.
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

    async fn write(
        &self,
        room: Uuid,
        event: &Event,
        meta: Option<ThreadMetadataParams<'_>>,
    ) -> (bool, ChatStateWriteOutcome) {
        let (_, inserted, outcome) = self
            .db
            .insert_event_with_thread_metadata_and_chat_state(self.scope, event, Some(room), meta)
            .await
            .expect("message write");
        (inserted, outcome)
    }

    async fn post(&self, room: Uuid, keys: &Keys, ts: u64, tags: Vec<Tag>) -> Event {
        let event = self.sign(room, keys, ts, tags);
        self.write(room, &event, None).await;
        event
    }

    async fn reply_event(
        &self,
        room: Uuid,
        root: &Event,
        event: &Event,
        broadcast: bool,
    ) -> (bool, ChatStateWriteOutcome) {
        let root_at = at(root.created_at.as_secs());
        let meta = ThreadMetadataParams {
            event_id: event.id.as_bytes(),
            event_created_at: at(event.created_at.as_secs()),
            channel_id: room,
            parent_event_id: Some(root.id.as_bytes()),
            parent_event_created_at: Some(root_at),
            root_event_id: Some(root.id.as_bytes()),
            root_event_created_at: Some(root_at),
            depth: 1,
            broadcast,
        };
        self.write(room, event, Some(meta)).await
    }

    async fn reply(
        &self,
        room: Uuid,
        root: &Event,
        keys: &Keys,
        ts: u64,
        mut tags: Vec<Tag>,
    ) -> Event {
        tags.push(Tag::parse(["e", &root.id.to_hex(), "", "root"]).expect("e tag"));
        let event = self.sign(room, keys, ts, tags);
        self.reply_event(room, root, &event, false).await;
        event
    }

    async fn marker(&self, room: Uuid, root: &Event, keys: &Keys, ts: u64, tag: &str) -> Event {
        self.reply(
            room,
            root,
            keys,
            ts,
            vec![Tag::parse([tag, &root.id.to_hex()]).expect("marker tag")],
        )
        .await
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

    async fn follow(&self, root: &Event, keys: &Keys) -> Option<Follow> {
        sqlx::query(
            "SELECT follow_state, follow_origin, read_seq FROM thread_follows \
             WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3",
        )
        .bind(self.scope.as_uuid())
        .bind(root.id.as_bytes().as_slice())
        .bind(pk(keys))
        .fetch_optional(&self.pool)
        .await
        .expect("follow row")
        .map(|row| Follow {
            state: row.get("follow_state"),
            origin: row.get("follow_origin"),
            read_seq: row.get("read_seq"),
        })
    }

    async fn set_follow_state(&self, root: &Event, keys: &Keys, state: &str) {
        sqlx::query(
            "UPDATE thread_follows SET follow_state = $4, follow_origin = 'user' \
             WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3",
        )
        .bind(self.scope.as_uuid())
        .bind(root.id.as_bytes().as_slice())
        .bind(pk(keys))
        .bind(state)
        .execute(&self.pool)
        .await
        .expect("set follow state");
    }

    async fn thread_state(&self, root: &Event) -> Option<ThreadState> {
        sqlx::query(
            "SELECT last_seq, closed_at, closed_by FROM thread_state \
             WHERE community_id = $1 AND root_event_id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(root.id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .expect("thread state")
        .map(|row| ThreadState {
            last_seq: row.get("last_seq"),
            closed_at: row.get("closed_at"),
            closed_by: row.get("closed_by"),
        })
    }

    async fn main_last(&self, room: Uuid) -> (Option<i64>, Option<Vec<u8>>) {
        let row = sqlx::query(
            "SELECT main_last_seq, main_last_event_id FROM channels \
             WHERE community_id = $1 AND id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(room)
        .fetch_one(&self.pool)
        .await
        .expect("channel row");
        (row.get("main_last_seq"), row.get("main_last_event_id"))
    }

    async fn member_read_seq(&self, room: Uuid, keys: &Keys) -> Option<i64> {
        sqlx::query_scalar(
            "SELECT read_seq FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
        )
        .bind(self.scope.as_uuid())
        .bind(room)
        .bind(pk(keys))
        .fetch_one(&self.pool)
        .await
        .expect("member row")
    }

    async fn mention_count(&self, event: &Event) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM event_mentions WHERE community_id = $1 AND event_id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(event.id.as_bytes().as_slice())
        .fetch_one(&self.pool)
        .await
        .expect("mention count")
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn first_reply_follows_author_root_author_and_mentions_of_members_only() {
    let f = Fixture::new(true).await;
    let (owner, replier, reply_mention, root_mention, outsider) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    let room = f
        .room(&owner, &[&replier, &reply_mention, &root_mention])
        .await;

    let root = f
        .post(
            room,
            &owner,
            100,
            vec![p_tag(&root_mention), p_tag(&outsider)],
        )
        .await;
    let first = f
        .reply(
            room,
            &root,
            &replier,
            110,
            vec![p_tag(&reply_mention), p_tag(&outsider)],
        )
        .await;
    let first_seq = f.seq(&first).await;

    assert_eq!(
        f.follow(&root, &replier).await,
        Some(Follow {
            state: "auto".into(),
            origin: "live".into(),
            read_seq: Some(first_seq),
        }),
        "reply author follows and has read up to their reply"
    );
    for (who, keys) in [
        ("root author", &owner),
        ("root mention", &root_mention),
        ("reply mention", &reply_mention),
    ] {
        let row = f
            .follow(&root, keys)
            .await
            .unwrap_or_else(|| panic!("{who} row"));
        assert_eq!(row.state, "auto", "{who}");
        assert_eq!(row.read_seq, None, "{who} has not read the reply");
    }
    assert_eq!(
        f.follow(&root, &outsider).await,
        None,
        "a mentioned non-member never gets a follow row"
    );

    let state = f.thread_state(&root).await.expect("thread state row");
    assert_eq!(state.last_seq, Some(first_seq));
    assert!(state.closed_at.is_none());

    // A missing root-author row is seeded again by a later reply (the first
    // reply may have been a close marker, which seeds nothing).
    sqlx::query("DELETE FROM thread_follows WHERE community_id = $1 AND pubkey = $2")
        .bind(f.scope.as_uuid())
        .bind(pk(&owner))
        .execute(&f.pool)
        .await
        .expect("drop root author row");
    f.reply(room, &root, &replier, 120, Vec::new()).await;
    assert_eq!(
        f.follow(&root, &owner).await.map(|row| row.state),
        Some("auto".into())
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn unfollowed_stays_on_mention_and_replying_follows_again() {
    let f = Fixture::new(true).await;
    let (owner, member) = (Keys::generate(), Keys::generate());
    let room = f.room(&owner, &[&member]).await;
    let root = f.post(room, &owner, 100, Vec::new()).await;
    f.reply(room, &root, &owner, 110, vec![p_tag(&member)])
        .await;
    f.set_follow_state(&root, &member, "unfollowed").await;

    let mention = f
        .reply(room, &root, &owner, 120, vec![p_tag(&member)])
        .await;
    let row = f.follow(&root, &member).await.expect("row");
    assert_eq!(
        row.state, "unfollowed",
        "a later mention does not bring it back"
    );
    assert_eq!(row.origin, "user");

    // Replaying an already-stored reply changes nothing.
    let (inserted, _) = f.reply_event(room, &root, &mention, false).await;
    assert!(!inserted);
    assert_eq!(
        f.follow(&root, &member).await.expect("row").state,
        "unfollowed"
    );

    let own = f.reply(room, &root, &member, 130, Vec::new()).await;
    assert_eq!(
        f.follow(&root, &member).await,
        Some(Follow {
            state: "followed".into(),
            origin: "live".into(),
            read_seq: Some(f.seq(&own).await),
        }),
        "replying in an unfollowed thread follows it again"
    );

    // An explicit follow is not downgraded by a later reply.
    f.reply(room, &root, &member, 140, Vec::new()).await;
    assert_eq!(
        f.follow(&root, &member).await.expect("row").state,
        "followed"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn removed_member_gets_no_follow_rows() {
    let f = Fixture::new(true).await;
    let (owner, gone) = (Keys::generate(), Keys::generate());
    let room = f.room(&owner, &[&gone]).await;
    let root = f.post(room, &gone, 100, Vec::new()).await;
    channel_members::remove_member(&f.pool, f.scope, room, &pk(&gone), &pk(&owner))
        .await
        .expect("remove member");

    // Removed root author and a removed mention are both skipped.
    f.reply(room, &root, &owner, 110, vec![p_tag(&gone)]).await;
    assert_eq!(f.follow(&root, &gone).await, None);
    assert!(f.follow(&root, &owner).await.is_some());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn close_and_reopen_markers_and_auto_reopen() {
    let f = Fixture::new(true).await;
    let (owner, closer) = (Keys::generate(), Keys::generate());
    let room = f.room(&owner, &[&closer]).await;
    let root = f.post(room, &owner, 100, Vec::new()).await;
    let reply = f.reply(room, &root, &owner, 110, Vec::new()).await;
    let reply_seq = f.seq(&reply).await;

    // The task result on the main timeline carries the close tag too; it is
    // a top-level message and must not close the thread.
    let result = f
        .post(
            room,
            &closer,
            115,
            vec![Tag::parse([THREAD_CLOSED_TAG, &root.id.to_hex()]).expect("tag")],
        )
        .await;
    assert!(f
        .thread_state(&root)
        .await
        .expect("state")
        .closed_at
        .is_none());
    assert_eq!(f.main_last(room).await.0, Some(f.seq(&result).await));

    let close = f.marker(room, &root, &closer, 120, THREAD_CLOSED_TAG).await;
    let state = f.thread_state(&root).await.expect("state");
    assert!(state.closed_at.is_some());
    assert_eq!(state.closed_by, Some(pk(&closer)));
    assert_eq!(
        state.last_seq,
        Some(reply_seq),
        "markers do not move ordering"
    );
    assert_eq!(
        f.follow(&root, &closer).await,
        None,
        "closing never follows"
    );
    assert_eq!(
        f.main_last(room).await.0,
        Some(f.seq(&result).await),
        "markers are not main-timeline activity"
    );
    // Mentions are still indexed for markers with p tags (none here).
    assert_eq!(f.mention_count(&close).await, 0);

    f.marker(room, &root, &owner, 130, THREAD_REOPENED_TAG)
        .await;
    let state = f.thread_state(&root).await.expect("state");
    assert!(state.closed_at.is_none() && state.closed_by.is_none());
    assert_eq!(state.last_seq, Some(reply_seq));

    f.marker(room, &root, &closer, 140, THREAD_CLOSED_TAG).await;
    assert!(f
        .thread_state(&root)
        .await
        .expect("state")
        .closed_at
        .is_some());
    let next = f.reply(room, &root, &closer, 150, Vec::new()).await;
    let state = f.thread_state(&root).await.expect("state");
    assert!(state.closed_at.is_none(), "any other reply reopens");
    assert_eq!(state.last_seq, Some(f.seq(&next).await));

    // A close tag naming a different root is an ordinary reply.
    f.marker(room, &root, &closer, 160, THREAD_CLOSED_TAG).await;
    let stray = f
        .reply(
            room,
            &root,
            &closer,
            170,
            vec![Tag::parse([THREAD_CLOSED_TAG, &"0".repeat(64)]).expect("tag")],
        )
        .await;
    let state = f.thread_state(&root).await.expect("state");
    assert!(state.closed_at.is_none());
    assert_eq!(state.last_seq, Some(f.seq(&stray).await));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn close_marker_on_a_thread_without_replies_creates_state() {
    let f = Fixture::new(true).await;
    let owner = Keys::generate();
    let room = f.room(&owner, &[]).await;
    let root = f.post(room, &owner, 100, Vec::new()).await;
    f.marker(room, &root, &owner, 110, THREAD_CLOSED_TAG).await;
    let state = f.thread_state(&root).await.expect("state row upserted");
    assert!(state.closed_at.is_some());
    assert_eq!(state.last_seq, None);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn only_applied_markers_report_a_close_change() {
    let f = Fixture::new(true).await;
    let owner = Keys::generate();
    let room = f.room(&owner, &[]).await;
    let root = f.post(room, &owner, 100, Vec::new()).await;
    let unstored_root = f.sign(room, &owner, 101, Vec::new());
    let reply_to = |root: &Event, ts: u64, tag: Option<&str>| {
        let mut tags = vec![Tag::parse(["e", &root.id.to_hex(), "", "root"]).expect("e tag")];
        if let Some(tag) = tag {
            tags.push(Tag::parse([tag, &root.id.to_hex()]).expect("marker tag"));
        }
        f.sign(room, &owner, ts, tags)
    };

    for (event, target, expected) in [
        (reply_to(&root, 110, Some(THREAD_CLOSED_TAG)), &root, true),
        (reply_to(&root, 120, None), &root, false),
        (reply_to(&root, 130, Some(THREAD_CLOSED_TAG)), &root, true),
        (reply_to(&root, 140, Some(THREAD_REOPENED_TAG)), &root, true),
        (
            reply_to(&unstored_root, 150, Some(THREAD_CLOSED_TAG)),
            &unstored_root,
            false,
        ),
    ] {
        let (_, outcome) = f.reply_event(room, target, &event, false).await;
        assert_eq!(
            outcome.thread_close_changed, expected,
            "{}",
            event.created_at
        );
    }
    // The close tag on a top-level message changes no thread.
    let top = f.sign(
        room,
        &owner,
        160,
        vec![Tag::parse([THREAD_CLOSED_TAG, &root.id.to_hex()]).expect("tag")],
    );
    assert!(!f.write(room, &top, None).await.1.thread_close_changed);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn top_level_and_broadcast_replies_are_main_timeline() {
    let f = Fixture::new(true).await;
    let (owner, member) = (Keys::generate(), Keys::generate());
    let room = f.room(&owner, &[&member]).await;

    let root = f.post(room, &member, 100, Vec::new()).await;
    let root_seq = f.seq(&root).await;
    assert_eq!(
        f.main_last(room).await,
        (Some(root_seq), Some(root.id.as_bytes().to_vec()))
    );
    assert_eq!(f.member_read_seq(room, &member).await, Some(root_seq));
    assert_eq!(f.member_read_seq(room, &owner).await, None);

    f.reply(room, &root, &owner, 110, Vec::new()).await;
    assert_eq!(
        f.main_last(room).await.0,
        Some(root_seq),
        "thread-only reply"
    );
    assert_eq!(
        f.member_read_seq(room, &owner).await,
        None,
        "a thread-only reply does not mark the room read"
    );

    let mut tags = vec![
        Tag::parse(["e", &root.id.to_hex(), "", "root"]).expect("e tag"),
        Tag::parse(["broadcast", "1"]).expect("broadcast tag"),
    ];
    tags.push(p_tag(&member));
    let broadcast = f.sign(room, &owner, 120, tags);
    f.reply_event(room, &root, &broadcast, true).await;
    let broadcast_seq = f.seq(&broadcast).await;
    assert_eq!(
        f.main_last(room).await,
        (Some(broadcast_seq), Some(broadcast.id.as_bytes().to_vec()))
    );
    assert_eq!(f.member_read_seq(room, &owner).await, Some(broadcast_seq));
    assert_eq!(
        f.thread_state(&root).await.expect("state").last_seq,
        Some(broadcast_seq),
        "a broadcast reply is also thread activity"
    );
    assert_eq!(
        f.mention_count(&broadcast).await,
        1,
        "mentions commit in the message transaction"
    );

    // Unread for the member, counted from the sequence index: the two replies.
    let read = f.member_read_seq(room, &member).await.expect("read seq");
    let unread: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT 1 FROM events \
          WHERE community_id = $1 AND channel_id = $2 AND channel_sequence > $3 \
          LIMIT 99) capped",
    )
    .bind(f.scope.as_uuid())
    .bind(room)
    .bind(read)
    .fetch_one(&f.pool)
    .await
    .expect("count");
    assert_eq!(unread, 2);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn incoming_dm_unhides_for_recipients_only() {
    let f = Fixture::new(true).await;
    let (sender, recipient) = (Keys::generate(), Keys::generate());
    let room = dm::create_dm(
        &f.pool,
        f.scope,
        &[&pk(&sender), &pk(&recipient)],
        &pk(&sender),
    )
    .await
    .expect("dm")
    .id;
    for keys in [&sender, &recipient] {
        dm::hide_dm(&f.pool, f.scope, room, &pk(keys))
            .await
            .expect("hide");
    }

    let event = f.sign(room, &sender, 100, Vec::new());
    let (_, outcome) = f.write(room, &event, None).await;
    assert_eq!(outcome.unhidden_dm_members, vec![pk(&recipient)]);

    let hidden: Vec<(Vec<u8>, bool)> = sqlx::query(
        "SELECT pubkey, hidden_at IS NOT NULL AS hidden FROM channel_members \
         WHERE community_id = $1 AND channel_id = $2",
    )
    .bind(f.scope.as_uuid())
    .bind(room)
    .fetch_all(&f.pool)
    .await
    .expect("members")
    .into_iter()
    .map(|row| (row.get("pubkey"), row.get("hidden")))
    .collect();
    for (pubkey, is_hidden) in hidden {
        assert_eq!(
            is_hidden,
            pubkey == pk(&sender),
            "only the sender stays hidden"
        );
    }

    // A stream room never touches hidden_at.
    let owner = Keys::generate();
    let stream = f.room(&owner, &[&recipient]).await;
    sqlx::query(
        "UPDATE channel_members SET hidden_at = NOW() \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
    )
    .bind(f.scope.as_uuid())
    .bind(stream)
    .bind(pk(&recipient))
    .execute(&f.pool)
    .await
    .expect("hide stream member");
    let event = f.sign(stream, &owner, 110, Vec::new());
    let (_, outcome) = f.write(stream, &event, None).await;
    assert!(outcome.unhidden_dm_members.is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn switch_off_writes_no_chat_state() {
    let f = Fixture::new(false).await;
    let (owner, member) = (Keys::generate(), Keys::generate());
    let room = f.room(&owner, &[&member]).await;
    let dm_room = dm::create_dm(&f.pool, f.scope, &[&pk(&owner), &pk(&member)], &pk(&owner))
        .await
        .expect("dm")
        .id;
    dm::hide_dm(&f.pool, f.scope, dm_room, &pk(&member))
        .await
        .expect("hide");

    let root = f.post(room, &owner, 100, Vec::new()).await;
    let reply = f
        .reply(room, &root, &owner, 110, vec![p_tag(&member)])
        .await;
    f.marker(room, &root, &owner, 120, THREAD_CLOSED_TAG).await;
    let dm_message = f.sign(dm_room, &owner, 130, Vec::new());
    let (_, outcome) = f.write(dm_room, &dm_message, None).await;

    assert_eq!(outcome, ChatStateWriteOutcome::default());
    assert!(f.thread_state(&root).await.is_none());
    assert_eq!(f.follow(&root, &owner).await, None);
    assert_eq!(f.follow(&root, &member).await, None);
    assert_eq!(f.main_last(room).await, (None, None));
    assert_eq!(f.member_read_seq(room, &owner).await, None);
    let still_hidden: bool = sqlx::query_scalar(
        "SELECT hidden_at IS NOT NULL FROM channel_members \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
    )
    .bind(f.scope.as_uuid())
    .bind(dm_room)
    .bind(pk(&member))
    .fetch_one(&f.pool)
    .await
    .expect("dm member");
    assert!(still_hidden);
    assert_eq!(
        f.mention_count(&reply).await,
        1,
        "mentions are still indexed after commit"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn chat_state_rows_are_tenant_scoped_and_fenced() {
    let f = Fixture::new(true).await;
    let other = Fixture::new(true).await;
    let owner = Keys::generate();
    let room = f.room(&owner, &[]).await;
    let root = f.post(room, &owner, 100, Vec::new()).await;
    f.reply(room, &root, &owner, 110, Vec::new()).await;

    let count_in = |scope: CommunityId, table: &'static str| {
        let pool = f.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table} WHERE community_id = $1"
            )))
            .bind(scope.as_uuid())
            .fetch_one(&pool)
            .await
            .expect("count")
        }
    };
    for table in ["thread_follows", "thread_state"] {
        assert_eq!(count_in(f.scope, table).await, 1, "{table}");
        assert_eq!(count_in(other.scope, table).await, 0, "{table}");
        assert!(crate::deletion::EXPECTED_SCOPED_TABLES.contains(&table));
        assert!(crate::deletion::PURGE_SCOPED_TABLES.contains(&table));
        let fenced: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_trigger \
              WHERE tgrelid = $1::regclass AND tgname = 'community_write_fence_' || $1)",
        )
        .bind(table)
        .fetch_one(&f.pool)
        .await
        .expect("fence trigger lookup");
        assert!(fenced, "{table} must carry the community write fence");
    }

    // A fenced community rejects new chat-state rows.
    let other_room = other.room(&owner, &[]).await;
    let mut tx = f.pool.begin().await.expect("tx");
    sqlx::query(
        "SELECT set_config('buzz.deletion_executor_community', $1, true),                 set_config('buzz.deletion_fence_generation', '1', true)",
    )
    .bind(other.scope.as_uuid().to_string())
    .execute(&mut *tx)
    .await
    .expect("executor identity");
    sqlx::query(
        "UPDATE communities SET deletion_state = 'fenced', deletion_fence_generation = 1          WHERE id = $1",
    )
    .bind(other.scope.as_uuid())
    .execute(&mut *tx)
    .await
    .expect("fence community");
    tx.commit().await.expect("commit fence");
    for statement in [
        "INSERT INTO thread_state (community_id, root_event_id, channel_id) VALUES ($1, $2, $3)",
        "INSERT INTO thread_follows (community_id, root_event_id, pubkey, channel_id,          follow_state, follow_origin) VALUES ($1, $2, $2, $3, 'auto', 'live')",
    ] {
        let rejected = sqlx::query(statement)
            .bind(other.scope.as_uuid())
            .bind(root.id.as_bytes().as_slice())
            .bind(other_room)
            .execute(&f.pool)
            .await
            .expect_err("fenced community must reject writes");
        assert!(
            rejected.to_string().contains("community write fenced"),
            "unexpected rejection: {rejected}"
        );
    }
}
