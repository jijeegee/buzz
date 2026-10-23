//! Stored chat state (migration 0072) against the values the chats list used
//! to compute from messages on every read. After every kind of change the
//! stored latest ids, thread names, and unread counts must equal a fresh
//! count written independently of the migration's functions.
use chrono::Utc;
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::chat_import::ImportFollow;
use crate::chat_list::{ChatListQuery, ChatRowKind, CHAT_UNREAD_CAP};
use crate::chat_state::{THREAD_CLOSED_TAG, THREAD_REOPENED_TAG};
use crate::chat_user_state::{FollowState, MarkReadItem, MarkReadTarget};
use crate::event::ThreadMetadataParams;
use crate::{channel, channel_members, dm, Db};

/// Old-way counts, written out without the migration's helper functions.
/// `$1` community. Markers never count (the one intended change).
macro_rules! not_marker_of {
    ($root:literal) => {
        concat!(
            "NOT EXISTS (SELECT 1 FROM jsonb_array_elements(e.tags) mt \
              WHERE mt->>0 IN ('kuzz:thread-closed', 'kuzz:thread-reopened') \
                AND lower(mt->>1) = encode(",
            $root,
            ", 'hex'))"
        )
    };
}

macro_rules! main_only {
    () => {
        concat!(
            "NOT EXISTS (SELECT 1 FROM thread_metadata r \
              WHERE r.community_id = e.community_id AND r.event_id = e.id AND r.depth > 0 \
                AND (NOT r.broadcast OR NOT ",
            not_marker_of!("r.root_event_id"),
            "))"
        )
    };
}

/// Not received from someone during one of the reader's block periods of
/// them, in a DM of exactly two members.
macro_rules! unblocked {
    ($reader:literal, $channel:literal) => {
        concat!(
            "NOT (EXISTS (SELECT 1 FROM channels dc WHERE dc.community_id = e.community_id \
                    AND dc.id = ",
            $channel,
            " AND dc.channel_type = 'dm') \
              AND (SELECT count(*) FROM channel_members dm WHERE dm.community_id = e.community_id \
                    AND dm.channel_id = ",
            $channel,
            ") = 2 \
              AND EXISTS (SELECT 1 FROM user_block_periods bp \
                    WHERE bp.community_id = e.community_id AND bp.blocker = ",
            $reader,
            " AND bp.blocked = e.pubkey AND bp.started_at <= e.received_at \
                    AND (bp.ended_at IS NULL OR e.received_at < bp.ended_at)))"
        )
    };
}

macro_rules! mentions {
    ($reader:literal) => {
        concat!(
            "EXISTS (SELECT 1 FROM event_mentions em WHERE em.community_id = e.community_id \
              AND em.event_id = e.id AND em.pubkey_hex = encode(",
            $reader,
            ", 'hex'))"
        )
    };
}

macro_rules! room_from {
    () => {
        concat!(
            " FROM events e WHERE e.community_id = cm.community_id \
               AND e.channel_id = cm.channel_id \
               AND e.channel_sequence > COALESCE(cm.read_seq, 0) \
               AND e.channel_sequence > channel_history_floor(cm.community_id, cm.channel_id, cm.pubkey) \
               AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL \
               AND e.pubkey <> cm.pubkey AND ",
            main_only!(),
            " AND ",
            unblocked!("cm.pubkey", "cm.channel_id")
        )
    };
}

macro_rules! thread_from {
    () => {
        concat!(
            " FROM thread_metadata r2 JOIN events e ON e.community_id = r2.community_id \
                AND e.created_at = r2.event_created_at AND e.id = r2.event_id \
              WHERE r2.community_id = tf.community_id AND r2.root_event_id = tf.root_event_id \
                AND r2.depth > 0 \
                AND e.channel_sequence > COALESCE(tf.read_seq, 0) \
                AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL \
                AND e.pubkey <> tf.pubkey AND ",
            not_marker_of!("tf.root_event_id"),
            " AND ",
            unblocked!("tf.pubkey", "tf.channel_id")
        )
    };
}

const MISMATCHES_SQL: &str = concat!(
    "SELECT 'room ' || encode(cm.pubkey, 'hex') || ' ' || cm.channel_id::text \
            || ' stored ' || cm.unread_count || '/' || cm.unread_mentions \
            || ' expected ' || x.unread || '/' || x.mentions AS problem \
       FROM channel_members cm \
      CROSS JOIN LATERAL (SELECT \
            (SELECT count(*) FROM (SELECT 1",
    room_from!(),
    " LIMIT 100) n) AS unread, \
            (SELECT count(*) FROM (SELECT 1",
    room_from!(),
    " AND ",
    mentions!("cm.pubkey"),
    " LIMIT 100) n) AS mentions) x \
      WHERE cm.community_id = $1 AND cm.removed_at IS NULL \
        AND (cm.unread_count <> x.unread OR cm.unread_mentions <> x.mentions) \
     UNION ALL \
     SELECT 'thread ' || encode(tf.pubkey, 'hex') || ' ' || encode(tf.root_event_id, 'hex') \
            || ' stored ' || tf.unread_count || '/' || tf.unread_mentions \
            || ' expected ' || x.unread || '/' || x.mentions \
       FROM thread_follows tf \
      CROSS JOIN LATERAL (SELECT \
            (SELECT count(*) FROM (SELECT 1",
    thread_from!(),
    " LIMIT 100) n) AS unread, \
            (SELECT count(*) FROM (SELECT 1",
    thread_from!(),
    " AND ",
    mentions!("tf.pubkey"),
    " LIMIT 100) n) AS mentions) x \
      WHERE tf.community_id = $1 \
        AND (tf.unread_count <> x.unread OR tf.unread_mentions <> x.mentions) \
     UNION ALL \
     SELECT 'room latest ' || c.id::text || ' stored ' \
            || COALESCE(encode(c.main_last_event_id, 'hex'), 'none') \
            || ' expected ' || COALESCE(encode(x.id, 'hex'), 'none') \
       FROM channels c \
      CROSS JOIN LATERAL (SELECT (SELECT e.id FROM events e \
            WHERE e.community_id = c.community_id AND e.channel_id = c.id \
              AND e.kind IN (9, 40002, 45001, 45003) AND e.deleted_at IS NULL AND ",
    main_only!(),
    " ORDER BY e.channel_sequence DESC LIMIT 1) AS id) x \
      WHERE c.community_id = $1 AND c.main_last_event_id IS DISTINCT FROM x.id \
     UNION ALL \
     SELECT 'thread latest ' || encode(ts.root_event_id, 'hex') || ' stored ' \
            || COALESCE(encode(ts.last_event_id, 'hex'), 'none') \
            || ' expected ' || COALESCE(encode(x.id, 'hex'), 'none') \
            || ' name ' || COALESCE(ts.name, 'none') || ' expected ' || COALESCE(x.name, 'none') \
       FROM thread_state ts \
      CROSS JOIN LATERAL (SELECT (SELECT e.id FROM thread_metadata r2 \
            JOIN events e ON e.community_id = r2.community_id \
             AND e.created_at = r2.event_created_at AND e.id = r2.event_id \
            WHERE r2.community_id = ts.community_id AND r2.root_event_id = ts.root_event_id \
              AND r2.depth > 0 AND e.kind IN (9, 40002, 45001, 45003) \
              AND e.deleted_at IS NULL AND ",
    not_marker_of!("ts.root_event_id"),
    " ORDER BY e.channel_sequence DESC LIMIT 1) AS id, \
            (SELECT NULLIF(e.content, '') FROM events e \
            WHERE e.community_id = ts.community_id AND e.channel_id = ts.channel_id \
              AND e.kind = 40009 AND e.deleted_at IS NULL \
              AND e.tags @> jsonb_build_array(jsonb_build_array('e', \
                    encode(ts.root_event_id, 'hex'))) \
            ORDER BY e.created_at DESC, e.id DESC LIMIT 1) AS name) x \
      WHERE ts.community_id = $1 \
        AND (ts.last_event_id IS DISTINCT FROM x.id OR ts.name IS DISTINCT FROM x.name)"
);

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

fn p_tag(keys: &Keys) -> Tag {
    Tag::parse(["p", &keys.public_key().to_hex()]).expect("p tag")
}

fn id(event: &Event) -> Vec<u8> {
    event.id.as_bytes().to_vec()
}

fn at(ts: u64) -> chrono::DateTime<Utc> {
    chrono::DateTime::from_timestamp(ts as i64, 0).expect("time")
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
            .bind(format!("chat-stored-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()).with_chat_state_writes(true),
            pool,
            scope,
        }
    }

    async fn room(&self, owner: &Keys, members: &[&Keys]) -> Uuid {
        let room = channel::create_channel(
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
        // New members here count the messages already there.
        sqlx::query(
            "UPDATE channels SET history_policy = 'all' WHERE community_id = $1 AND id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(room)
        .execute(&self.pool)
        .await
        .expect("full history room");
        for member in members {
            self.join(room, member).await;
        }
        room
    }

    async fn join(&self, room: Uuid, member: &Keys) {
        channel_members::add_member(
            &self.pool,
            self.scope,
            room,
            &pk(member),
            MemberRole::Member,
            None,
        )
        .await
        .expect("member");
    }

    fn sign(
        &self,
        room: Uuid,
        kind: u16,
        keys: &Keys,
        ts: u64,
        content: &str,
        tags: Vec<Tag>,
    ) -> Event {
        let mut all = vec![Tag::parse(["h", &room.to_string()]).expect("h tag")];
        all.extend(tags);
        EventBuilder::new(Kind::Custom(kind), content)
            .tags(all)
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("signature")
    }

    async fn post(&self, room: Uuid, keys: &Keys, ts: u64, tags: Vec<Tag>) -> Event {
        let event = self.sign(room, 9, keys, ts, &format!("message at {ts}"), tags);
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
        let event = self.sign(room, 9, keys, ts, &format!("reply at {ts}"), tags);
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

    async fn marker(
        &self,
        room: Uuid,
        root: &Event,
        keys: &Keys,
        ts: u64,
        tag: &str,
        broadcast: bool,
    ) -> Event {
        let marker = Tag::parse([tag, &root.id.to_hex()]).expect("marker tag");
        self.reply_as(room, root, keys, ts, vec![marker], broadcast)
            .await
    }

    async fn name(&self, room: Uuid, root: &Event, keys: &Keys, ts: u64, name: &str) -> Event {
        let event = self.sign(
            room,
            40009,
            keys,
            ts,
            name,
            vec![Tag::parse(["e", &root.id.to_hex()]).expect("e tag")],
        );
        // The plain insert path: the stored name must not depend on which
        // write path stored the event.
        self.db
            .insert_event(self.scope, &event, Some(room))
            .await
            .expect("thread name");
        event
    }

    async fn delete(&self, event: &Event, root: Option<&Event>) {
        let root_id = root.map(id);
        assert!(self
            .db
            .soft_delete_event_and_update_thread(
                self.scope,
                event.id.as_bytes(),
                root_id.as_deref(),
                root_id.as_deref(),
            )
            .await
            .expect("delete"));
    }

    async fn restore(&self, event: &Event) {
        sqlx::query("UPDATE events SET deleted_at = NULL WHERE community_id = $1 AND id = $2")
            .bind(self.scope.as_uuid())
            .bind(event.id.as_bytes().as_slice())
            .execute(&self.pool)
            .await
            .expect("restore");
    }

    async fn read_room(&self, reader: &Keys, room: Uuid) {
        self.mark(reader, MarkReadTarget::Channel(room)).await;
    }

    async fn read_thread(&self, reader: &Keys, root: &Event) {
        self.mark(reader, MarkReadTarget::Thread(id(root))).await;
    }

    async fn mark(&self, reader: &Keys, target: MarkReadTarget) {
        self.db
            .chat_mark_read(
                self.scope,
                &reader.public_key(),
                &[MarkReadItem {
                    target,
                    position: Some(crate::chat_user_state::ReadPosition::Seq(i64::from(
                        i32::MAX,
                    ))),
                }],
                false,
            )
            .await
            .expect("mark read")
            .expect("mark read allowed");
    }

    /// Every stored value equals a fresh count; `step` names the change.
    async fn check(&self, step: &str) {
        let problems: Vec<String> = sqlx::query(MISMATCHES_SQL)
            .bind(self.scope.as_uuid())
            .fetch_all(&self.pool)
            .await
            .expect("compare stored state")
            .into_iter()
            .map(|row| row.get::<String, _>("problem"))
            .collect();
        assert!(
            problems.is_empty(),
            "after {step}:\n{}",
            problems.join("\n")
        );
    }

    async fn room_counts(&self, reader: &Keys, room: Uuid) -> (i32, i32) {
        sqlx::query_as(
            "SELECT unread_count, unread_mentions FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
        )
        .bind(self.scope.as_uuid())
        .bind(room)
        .bind(pk(reader))
        .fetch_one(&self.pool)
        .await
        .expect("room counts")
    }

    async fn thread_counts(&self, reader: &Keys, root: &Event) -> Option<(i32, i32)> {
        sqlx::query_as(
            "SELECT unread_count, unread_mentions FROM thread_follows \
             WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3",
        )
        .bind(self.scope.as_uuid())
        .bind(id(root))
        .bind(pk(reader))
        .fetch_optional(&self.pool)
        .await
        .expect("thread counts")
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn messages_replies_and_markers_keep_stored_counts() {
    let f = Fixture::new().await;
    let (reader, other, third) = (Keys::generate(), Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader, &third]).await;

    let root = f.post(room, &reader, 100, Vec::new()).await;
    f.check("own post").await;
    f.post(room, &other, 101, vec![p_tag(&reader)]).await;
    f.check("message mentioning the reader").await;
    assert_eq!(f.room_counts(&reader, room).await, (1, 1));
    assert_eq!(f.room_counts(&third, room).await, (2, 0));

    f.reply(room, &root, &other, 102, Vec::new()).await;
    f.check("reply: root author auto-follows").await;
    assert_eq!(f.thread_counts(&reader, &root).await, Some((1, 0)));
    f.reply(room, &root, &other, 103, vec![p_tag(&third)]).await;
    f.check("reply mentioning a new follower").await;
    assert_eq!(f.thread_counts(&third, &root).await, Some((1, 1)));
    f.reply_as(room, &root, &third, 104, vec![p_tag(&reader)], true)
        .await;
    f.check("broadcast reply").await;
    assert_eq!(f.room_counts(&reader, room).await, (2, 2));
    assert_eq!(f.thread_counts(&reader, &root).await, Some((3, 1)));

    // Close and reopen markers count nowhere and never become the latest,
    // broadcast or not.
    f.marker(room, &root, &other, 105, THREAD_CLOSED_TAG, false)
        .await;
    f.check("close marker").await;
    f.marker(room, &root, &other, 106, THREAD_REOPENED_TAG, true)
        .await;
    f.check("broadcast reopen marker").await;
    assert_eq!(f.room_counts(&reader, room).await, (2, 2));
    assert_eq!(f.thread_counts(&reader, &root).await, Some((3, 1)));

    f.reply(room, &root, &reader, 107, Vec::new()).await;
    f.check("reader replies: thread read").await;
    assert_eq!(f.thread_counts(&reader, &root).await, Some((0, 0)));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn reads_joins_and_follows_recount() {
    let f = Fixture::new().await;
    let (reader, other, late) = (Keys::generate(), Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;

    let root = f.post(room, &other, 100, vec![p_tag(&reader)]).await;
    f.post(room, &other, 101, Vec::new()).await;
    f.reply(room, &root, &reader, 102, Vec::new()).await;
    f.reply(room, &root, &other, 103, vec![p_tag(&reader)])
        .await;
    f.check("setup").await;

    f.read_room(&reader, room).await;
    f.check("room read").await;
    assert_eq!(f.room_counts(&reader, room).await, (0, 0));
    f.read_thread(&reader, &root).await;
    f.check("thread read").await;
    assert_eq!(f.thread_counts(&reader, &root).await, Some((0, 0)));

    // A new member is counted from the messages already there.
    f.join(room, &late).await;
    f.check("join").await;
    f.post(room, &other, 104, Vec::new()).await;
    f.check("message after join").await;

    // Follow rows created by an explicit follow and by a first-run upload.
    f.db.chat_thread_follow(
        f.scope,
        &late.public_key(),
        &id(&root),
        FollowState::Followed,
    )
    .await
    .expect("follow")
    .expect("allowed");
    f.check("explicit follow").await;
    let second = f.post(room, &other, 105, Vec::new()).await;
    f.reply(room, &second, &other, 106, Vec::new()).await;
    let imported =
        f.db.chat_import(
            f.scope,
            &late.public_key(),
            &[],
            &[ImportFollow {
                root: id(&second),
                state: FollowState::Followed,
            }],
        )
        .await
        .expect("import");
    assert!(imported.follows[0].is_applied(), "{imported:?}");
    f.check("imported follow").await;
    f.reply(room, &second, &other, 107, Vec::new()).await;
    f.check("reply after imported follow").await;
    assert!(f.thread_counts(&late, &second).await.is_some());

    // Leave and rejoin: the rejoined row and its follow rows are recounted.
    channel_members::remove_member(&f.pool, f.scope, room, &pk(&late), &pk(&late))
        .await
        .expect("leave");
    f.post(room, &other, 108, Vec::new()).await;
    f.reply(room, &second, &other, 109, Vec::new()).await;
    f.join(room, &late).await;
    f.check("rejoin").await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn deletes_restores_and_names_recount() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;

    let root = f.post(room, &reader, 100, Vec::new()).await;
    let first = f.post(room, &other, 101, vec![p_tag(&reader)]).await;
    let last = f.post(room, &other, 102, Vec::new()).await;
    let reply = f
        .reply(room, &root, &other, 103, vec![p_tag(&reader)])
        .await;
    let last_reply = f.reply(room, &root, &other, 104, Vec::new()).await;
    f.check("setup").await;

    f.delete(&first, None).await;
    f.check("delete an unread mention").await;
    assert_eq!(f.room_counts(&reader, room).await, (1, 0));
    f.delete(&last, None).await;
    f.check("delete the room's latest").await;
    f.delete(&last_reply, Some(&root)).await;
    f.check("delete the thread's latest").await;
    f.delete(&reply, Some(&root)).await;
    f.check("delete the thread's only reply").await;
    f.restore(&last).await;
    f.check("restore the room's latest").await;
    f.restore(&reply).await;
    f.check("restore a reply").await;

    // Thread names, from any write path; an empty name clears.
    let named = f.name(room, &root, &other, 105, "Launch plan").await;
    f.check("name").await;
    let renamed = f.name(room, &root, &other, 106, "Launch v2").await;
    f.check("rename").await;
    f.delete(&renamed, None).await;
    f.check("delete the newest name").await;
    f.name(room, &root, &other, 107, "").await;
    f.check("empty name").await;
    // A thread with no reply can still have a name.
    let quiet = f.post(room, &other, 108, Vec::new()).await;
    f.name(room, &quiet, &reader, 109, "Quiet thread").await;
    f.check("name without replies").await;

    let rows =
        f.db.chat_list(
            f.scope,
            &reader.public_key(),
            &ChatListQuery {
                limit: 50,
                before: None,
                pinned_threads: vec![id(&quiet)],
                quiet_before: Utc::now() - chrono::Duration::days(7),
            },
        )
        .await
        .expect("chat list")
        .rows;
    let thread = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Thread && r.root_id.as_deref() == Some(&id(&quiet)[..]))
        .expect("pinned thread");
    assert_eq!(thread.thread_name.as_deref(), Some("Quiet thread"));
    let _ = named;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn dm_blocks_recount_and_skip_new_messages() {
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
    let root = f.post(direct, &reader, 100, Vec::new()).await;
    let earlier = f.post(direct, &friend, 101, vec![p_tag(&reader)]).await;
    f.reply(direct, &root, &friend, 102, Vec::new()).await;
    f.check("setup").await;
    assert_eq!(f.room_counts(&reader, direct).await, (1, 1));
    assert_eq!(f.thread_counts(&reader, &root).await, Some((1, 0)));

    // What was sent before the block keeps counting.
    f.db.block_user(f.scope, &pk(&reader), &pk(&friend))
        .await
        .expect("block");
    f.check("block").await;
    assert_eq!(f.room_counts(&reader, direct).await, (1, 1));
    assert_eq!(f.thread_counts(&reader, &root).await, Some((1, 0)));

    f.post(direct, &friend, 103, vec![p_tag(&reader)]).await;
    f.reply(direct, &root, &friend, 104, Vec::new()).await;
    f.check("messages while blocked").await;
    assert_eq!(f.room_counts(&reader, direct).await, (1, 1));
    assert_eq!(f.thread_counts(&reader, &root).await, Some((1, 0)));

    // The chats list skips the message sent while blocked as the latest.
    let rows =
        f.db.chat_list(
            f.scope,
            &reader.public_key(),
            &ChatListQuery {
                limit: 50,
                before: None,
                pinned_threads: Vec::new(),
                quiet_before: Utc::now() - chrono::Duration::days(7),
            },
        )
        .await
        .expect("chat list")
        .rows;
    let dm_row = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Room && r.channel_id == direct)
        .expect("dm row");
    assert_eq!(
        dm_row.latest.as_ref().expect("latest")["id"],
        earlier.id.to_hex()
    );

    // Unblocking does not bring them back; new messages count again.
    f.db.unblock_user(f.scope, &pk(&reader), &pk(&friend))
        .await
        .expect("unblock");
    f.check("unblock").await;
    assert_eq!(f.room_counts(&reader, direct).await, (1, 1));
    assert_eq!(f.thread_counts(&reader, &root).await, Some((1, 0)));
    f.post(direct, &friend, 105, Vec::new()).await;
    f.reply(direct, &root, &friend, 106, Vec::new()).await;
    f.check("messages after unblock").await;
    assert_eq!(f.room_counts(&reader, direct).await, (2, 1));
    assert_eq!(f.thread_counts(&reader, &root).await, Some((2, 0)));

    // A read position move recounts with the same rule.
    f.read_room(&reader, direct).await;
    f.check("read").await;
    assert_eq!(f.room_counts(&reader, direct).await, (0, 0));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn group_dm_blocks_do_not_filter_counts() {
    let f = Fixture::new().await;
    let (reader, friend, third) = (Keys::generate(), Keys::generate(), Keys::generate());
    let group = dm::create_dm(
        &f.pool,
        f.scope,
        &[&pk(&reader), &pk(&friend), &pk(&third)],
        &pk(&reader),
    )
    .await
    .expect("group dm")
    .id;
    f.db.block_user(f.scope, &pk(&reader), &pk(&friend))
        .await
        .expect("block");
    f.post(group, &friend, 100, vec![p_tag(&reader)]).await;
    f.check("group message while blocked").await;
    assert_eq!(f.room_counts(&reader, group).await, (1, 1));
    let rows = chats(&f, &reader).await;
    let row = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Room && r.channel_id == group)
        .expect("group row");
    assert_eq!(row.unread, 1);
}

async fn chats(f: &Fixture, reader: &Keys) -> Vec<crate::chat_list::ChatListRow> {
    f.db.chat_list(
        f.scope,
        &reader.public_key(),
        &ChatListQuery {
            limit: 50,
            before: None,
            pinned_threads: Vec::new(),
            quiet_before: Utc::now() - chrono::Duration::days(7),
        },
    )
    .await
    .expect("chat list")
    .rows
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn dm_blocked_messages_neither_reorder_nor_unarchive() {
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
    let room = f.room(&other, &[&reader]).await;
    let root = f.post(direct, &reader, 100, Vec::new()).await;
    let mine = f.reply(direct, &root, &reader, 101, Vec::new()).await;
    f.db.chat_thread_follow(
        f.scope,
        &reader.public_key(),
        &id(&root),
        FollowState::Followed,
    )
    .await
    .expect("follow")
    .expect("allowed");
    f.post(room, &other, 102, Vec::new()).await;
    f.read_room(&reader, room).await;

    f.db.block_user(f.scope, &pk(&reader), &pk(&friend))
        .await
        .expect("block");
    f.post(direct, &friend, 103, Vec::new()).await;
    f.reply(direct, &root, &friend, 104, Vec::new()).await;
    f.check("blocked traffic").await;

    // The blocked sender's newer messages do not lift the DM or its thread
    // above the room that really has the newest message.
    let rows = chats(&f, &reader).await;
    assert_eq!(
        (rows[0].kind, rows[0].channel_id),
        (ChatRowKind::Room, room),
        "the room with the newest visible message comes first"
    );
    let dm_row = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Room && r.channel_id == direct)
        .expect("dm row");
    assert_eq!(
        dm_row.latest.as_ref().expect("latest")["id"],
        root.id.to_hex()
    );
    assert!(
        dm_row.last_seq <= dm_row.read_seq,
        "nothing unread to the reader"
    );
    let thread = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Thread && r.root_id.as_deref() == Some(&id(&root)[..]))
        .expect("thread row");
    assert_eq!(
        thread.latest.as_ref().expect("latest")["id"],
        mine.id.to_hex()
    );
    assert_eq!(thread.unread, 0);

    // An archived DM stays archived while only the blocked sender writes.
    sqlx::query("UPDATE channels SET archived_at = now() WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(direct)
        .execute(&f.pool)
        .await
        .expect("archive");
    f.post(direct, &friend, 105, Vec::new()).await;
    f.reply(direct, &root, &friend, 106, Vec::new()).await;
    let rows = chats(&f, &reader).await;
    assert!(
        rows.iter().all(|r| r.channel_id != direct),
        "archived DM and its thread stay out of the list"
    );

    // Unblocking does not lift it either: those messages stay hidden.
    f.db.unblock_user(f.scope, &pk(&reader), &pk(&friend))
        .await
        .expect("unblock");
    f.check("unblock").await;
    let rows = chats(&f, &reader).await;
    assert!(
        rows.iter().all(|r| r.channel_id != direct),
        "messages sent while blocked do not unarchive after unblocking"
    );

    // A new message after unblocking brings it back.
    let fresh = f.post(direct, &friend, 107, Vec::new()).await;
    let rows = chats(&f, &reader).await;
    let dm_row = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Room && r.channel_id == direct)
        .expect("dm back");
    assert_eq!(
        dm_row.latest.as_ref().expect("latest")["id"],
        fresh.id.to_hex()
    );
    assert_eq!(dm_row.unread, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn dm_thread_summary_without_blocked_replies() {
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
    let root = f.post(direct, &reader, 100, Vec::new()).await;
    f.reply(direct, &root, &reader, 101, Vec::new()).await;
    let before = f.reply(direct, &root, &friend, 102, Vec::new()).await;
    f.db.block_user(f.scope, &pk(&reader), &pk(&friend))
        .await
        .expect("block");
    f.reply(direct, &root, &friend, 103, Vec::new()).await;
    f.marker(direct, &root, &friend, 104, THREAD_CLOSED_TAG, false)
        .await;
    let gone = f.reply(direct, &root, &friend, 105, Vec::new()).await;
    f.delete(&gone, Some(&root)).await;
    // A thread the friend replied to only while blocked.
    let quiet = f.post(direct, &reader, 106, Vec::new()).await;
    f.reply(direct, &quiet, &friend, 107, Vec::new()).await;
    f.db.unblock_user(f.scope, &pk(&reader), &pk(&friend))
        .await
        .expect("unblock");

    let mut summary =
        f.db.get_thread_summary(f.scope, &id(&root))
            .await
            .expect("summary")
            .expect("thread");
    assert_eq!((summary.reply_count, summary.descendant_count), (3, 3));
    let mut quiet_summary =
        f.db.get_thread_summary(f.scope, &id(&quiet))
            .await
            .expect("summary")
            .expect("thread");
    assert_eq!(quiet_summary.reply_count, 1);
    assert!(quiet_summary.participants.contains(&pk(&friend)));

    let hidden =
        f.db.blocked_thread_replies(f.scope, &pk(&reader), &[id(&root), id(&quiet)], true)
            .await
            .expect("blocked replies");
    assert_eq!(hidden.len(), 2);
    let main = hidden.iter().find(|h| h.target == id(&root)).unwrap();
    // Only the reply sent while blocked comes out; the earlier reply stays.
    assert_eq!((main.direct, main.descendants), (1, 1));
    assert!(
        main.hidden_authors.is_empty(),
        "the friend's earlier reply shows"
    );
    let before_at: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT received_at FROM events WHERE community_id = $1 AND id = $2")
            .bind(f.scope.as_uuid())
            .bind(id(&before))
            .fetch_one(&f.pool)
            .await
            .expect("received_at");
    assert_eq!(main.last_other_reply_at, Some(before_at));
    summary.without_blocked(main);
    assert_eq!((summary.reply_count, summary.descendant_count), (2, 2));
    assert_eq!(summary.participants.len(), 2);

    let side = hidden.iter().find(|h| h.target == id(&quiet)).unwrap();
    assert_eq!((side.direct, side.descendants), (1, 1));
    assert_eq!(side.hidden_authors, vec![pk(&friend)]);
    assert_eq!(side.last_other_reply_at, None);
    quiet_summary.without_blocked(side);
    assert_eq!(quiet_summary.reply_count, 0);
    assert!(!quiet_summary.participants.contains(&pk(&friend)));

    // Someone who never blocked anyone: nothing to take out.
    assert!(f
        .db
        .blocked_thread_replies(f.scope, &pk(&friend), &[id(&root), id(&quiet)], true)
        .await
        .expect("blocked replies")
        .is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn counts_stop_at_the_cap_and_refill_matches() {
    let f = Fixture::new().await;
    let (reader, other) = (Keys::generate(), Keys::generate());
    let room = f.room(&other, &[&reader]).await;
    let mut posts = Vec::new();
    for ts in 0..(CHAT_UNREAD_CAP as u64 + 6) {
        let tags = if ts % 2 == 0 {
            vec![p_tag(&reader)]
        } else {
            Vec::new()
        };
        posts.push(f.post(room, &other, 1000 + ts, tags).await);
    }
    f.check("over the cap").await;
    assert_eq!(f.room_counts(&reader, room).await, (100, 53));

    // Read halfway: recounted from messages, no longer capped.
    f.db.chat_mark_read(
        f.scope,
        &reader.public_key(),
        &[MarkReadItem {
            target: MarkReadTarget::Channel(room),
            position: Some(crate::chat_user_state::ReadPosition::Event(id(&posts[60]))),
        }],
        false,
    )
    .await
    .expect("mark read")
    .expect("allowed");
    f.check("read halfway").await;

    // The refill (migration, backfill, or by hand) writes the same values.
    sqlx::query(
        "UPDATE channel_members SET unread_count = 0, unread_mentions = 0 \
          WHERE community_id = $1",
    )
    .bind(f.scope.as_uuid())
    .execute(&f.pool)
    .await
    .expect("zero counts");
    sqlx::query("UPDATE channels SET main_last_event_id = NULL WHERE community_id = $1")
        .bind(f.scope.as_uuid())
        .execute(&f.pool)
        .await
        .expect("clear latest");
    sqlx::query("SELECT refill_chat_stored_state_channel($1, $2)")
        .bind(f.scope.as_uuid())
        .bind(room)
        .execute(&f.pool)
        .await
        .expect("refill");
    f.check("refill").await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn concurrent_writes_reads_follows_and_deletes_stay_consistent() {
    let f = std::sync::Arc::new(Fixture::new().await);
    let people: Vec<Keys> = (0..4).map(|_| Keys::generate()).collect();
    let refs: Vec<&Keys> = people.iter().skip(1).collect();
    let room = f.room(&people[0], &refs).await;
    let root = f.post(room, &people[0], 100, Vec::new()).await;
    f.reply(room, &root, &people[1], 101, Vec::new()).await;

    let mut tasks = Vec::new();
    for (n, keys) in people.iter().enumerate() {
        let (f, keys, root) = (f.clone(), keys.clone(), root.clone());
        let mention = people[(n + 1) % people.len()].clone();
        tasks.push(tokio::spawn(async move {
            for i in 0..15u64 {
                let ts = 200 + i * 10 + n as u64;
                match i % 5 {
                    0 => {
                        f.post(room, &keys, ts, vec![p_tag(&mention)]).await;
                    }
                    1 => {
                        f.reply(room, &root, &keys, ts, vec![p_tag(&mention)]).await;
                    }
                    2 => f.read_room(&keys, room).await,
                    3 => {
                        let state = if i % 2 == 0 {
                            FollowState::Followed
                        } else {
                            FollowState::Unfollowed
                        };
                        f.db.chat_thread_follow(f.scope, &keys.public_key(), &id(&root), state)
                            .await
                            .expect("follow")
                            .expect("allowed");
                    }
                    _ => {
                        let gone = f.post(room, &keys, ts, Vec::new()).await;
                        f.delete(&gone, None).await;
                    }
                }
            }
        }));
    }
    for task in tasks {
        task.await.expect("task");
    }
    f.check("concurrent traffic").await;
}
