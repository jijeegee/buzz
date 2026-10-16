//! Chats S3 backfill against real history.
//!
//! History is written with the chat-state switch off (as on the live relay
//! before the switch is turned on), then the backfill fills the state. The
//! main check replays the same signed events into a second community with the
//! switch on and requires the backfill to reach the state the live write path
//! produced.
use chrono::{DateTime, Duration, Utc};
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::chat_backfill::{ChatBackfillCounts, ChatBackfillOptions};
use crate::chat_state::{THREAD_CLOSED_TAG, THREAD_REOPENED_TAG};
use crate::event::ThreadMetadataParams;
use crate::{channel, channel_members, Db};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

fn at(secs: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs as i64, 0).expect("timestamp")
}

fn p_tag(keys: &Keys) -> Tag {
    Tag::parse(["p", &keys.public_key().to_hex()]).expect("p tag")
}

fn marker_tag(tag: &str, root: &Event) -> Tag {
    Tag::parse([tag, &root.id.to_hex()]).expect("marker tag")
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

fn sign(keys: &Keys, ts: u64, tags: Vec<Tag>) -> Event {
    EventBuilder::new(Kind::Custom(9), format!("message at {ts}"))
        .tags(tags)
        .custom_created_at(Timestamp::from(ts))
        .sign_with_keys(keys)
        .expect("signature")
}

/// One message of a scripted history: top-level, or a reply under `root`
/// whose direct parent is `parent`.
struct Step<'a> {
    event: &'a Event,
    parent: Option<&'a Event>,
    root: Option<&'a Event>,
    broadcast: bool,
}

fn top(event: &Event) -> Step<'_> {
    Step {
        event,
        parent: None,
        root: None,
        broadcast: false,
    }
}

fn reply<'a>(root: &'a Event, event: &'a Event) -> Step<'a> {
    Step {
        event,
        parent: Some(root),
        root: Some(root),
        broadcast: false,
    }
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
            .bind(format!("chat-backfill-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()).with_chat_state_writes(chat_state),
            pool,
            scope,
        }
    }

    async fn room(&self, owner: &Keys, members: &[&Keys]) -> Uuid {
        let id = channel::create_channel(
            &self.pool,
            self.scope,
            &format!("room-{}", Uuid::new_v4()),
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

    async fn write(&self, room: Uuid, step: &Step<'_>) {
        let meta = step.parent.map(|parent| {
            let root = step.root.unwrap_or(parent);
            ThreadMetadataParams {
                event_id: step.event.id.as_bytes(),
                event_created_at: at(step.event.created_at.as_secs()),
                channel_id: room,
                parent_event_id: Some(parent.id.as_bytes()),
                parent_event_created_at: Some(at(parent.created_at.as_secs())),
                root_event_id: Some(root.id.as_bytes()),
                root_event_created_at: Some(at(root.created_at.as_secs())),
                depth: if parent.id == root.id { 1 } else { 2 },
                broadcast: step.broadcast,
            }
        });
        let (_, inserted, _) = self
            .db
            .insert_event_with_thread_metadata_and_chat_state(
                self.scope,
                step.event,
                Some(room),
                meta,
            )
            .await
            .expect("message write");
        assert!(inserted);
    }

    async fn play(&self, room: Uuid, steps: &[Step<'_>]) {
        for step in steps {
            self.write(room, step).await;
        }
    }

    /// Backfill every channel, two channels per page to exercise paging.
    async fn backfill(&self, options: ChatBackfillOptions) -> ChatBackfillCounts {
        backfill_community(&self.db, self.scope, options)
            .await
            .expect("backfill")
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

    async fn rows(&self, sql: &'static str) -> Vec<String> {
        sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql))
            .bind(self.scope.as_uuid())
            .fetch_all(&self.pool)
            .await
            .expect("snapshot rows")
    }

    /// Main timeline state, with seq and time mapped back to event ids so two
    /// communities holding the same events compare equal.
    async fn main_snapshot(&self) -> Vec<String> {
        self.rows(
            "SELECT concat_ws('|', encode(c.main_last_event_id, 'hex'),
                    (SELECT encode(e.id, 'hex') FROM events e WHERE e.community_id = c.community_id
                        AND e.channel_id = c.id AND e.channel_sequence = c.main_last_seq),
                    (SELECT encode(e.id, 'hex') FROM events e WHERE e.community_id = c.community_id
                        AND e.channel_id = c.id AND e.received_at = c.main_last_at LIMIT 1))
             FROM channels c WHERE c.community_id = $1 ORDER BY 1",
        )
        .await
    }

    async fn thread_snapshot(&self) -> Vec<String> {
        self.rows(
            "SELECT concat_ws('|', encode(ts.root_event_id, 'hex'),
                    'last=' || COALESCE((SELECT encode(e.id, 'hex') FROM events e
                        WHERE e.community_id = ts.community_id AND e.channel_id = ts.channel_id
                          AND e.channel_sequence = ts.last_seq), '-'),
                    'last_at=' || COALESCE((SELECT encode(e.id, 'hex') FROM events e
                        WHERE e.community_id = ts.community_id AND e.channel_id = ts.channel_id
                          AND e.received_at = ts.last_at LIMIT 1), '-'),
                    'closed=' || COALESCE((SELECT encode(e.id, 'hex') FROM events e
                        WHERE e.community_id = ts.community_id AND e.channel_id = ts.channel_id
                          AND e.received_at = ts.closed_at LIMIT 1), '-'),
                    'by=' || COALESCE(encode(ts.closed_by, 'hex'), '-'))
             FROM thread_state ts WHERE ts.community_id = $1 ORDER BY 1",
        )
        .await
    }

    async fn follow_snapshot(&self) -> Vec<String> {
        self.rows(
            "SELECT concat_ws('|', encode(root_event_id, 'hex'), encode(pubkey, 'hex'),
                    follow_state)
             FROM thread_follows WHERE community_id = $1 ORDER BY 1",
        )
        .await
    }

    /// Every chat-state column of the community, for no-change checks.
    async fn full_snapshot(&self) -> Vec<String> {
        let mut out = self
            .rows(
                "SELECT concat_ws('|', 'channel', id::text, main_last_seq::text,
                        main_last_at::text, encode(main_last_event_id, 'hex'))
                 FROM channels WHERE community_id = $1 ORDER BY 1",
            )
            .await;
        out.extend(
            self.rows(
                "SELECT concat_ws('|', 'member', channel_id::text, encode(pubkey, 'hex'),
                        read_seq::text, read_origin, unread_mark::text)
                 FROM channel_members WHERE community_id = $1 ORDER BY 1",
            )
            .await,
        );
        out.extend(
            self.rows(
                "SELECT concat_ws('|', 'thread', encode(root_event_id, 'hex'), last_seq::text,
                        last_at::text, closed_at::text, encode(closed_by, 'hex'))
                 FROM thread_state WHERE community_id = $1 ORDER BY 1",
            )
            .await,
        );
        out.extend(
            self.rows(
                "SELECT concat_ws('|', 'follow', encode(root_event_id, 'hex'),
                        encode(pubkey, 'hex'), follow_state, follow_origin, read_seq::text,
                        unread_mark::text, updated_at::text)
                 FROM thread_follows WHERE community_id = $1 ORDER BY 1",
            )
            .await,
        );
        out
    }

    async fn member(&self, room: Uuid, keys: &Keys) -> (Option<i64>, Option<String>) {
        let row = sqlx::query(
            "SELECT read_seq, read_origin FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
        )
        .bind(self.scope.as_uuid())
        .bind(room)
        .bind(pk(keys))
        .fetch_one(&self.pool)
        .await
        .expect("member row");
        (row.get("read_seq"), row.get("read_origin"))
    }

    async fn follow(&self, root: &Event, keys: &Keys) -> Option<(String, String, Option<i64>)> {
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
        .map(|row| {
            (
                row.get("follow_state"),
                row.get("follow_origin"),
                row.get("read_seq"),
            )
        })
    }

    async fn thread(&self, root: &Event) -> Option<(Option<i64>, bool, Option<Vec<u8>>)> {
        sqlx::query(
            "SELECT last_seq, closed_at IS NOT NULL AS closed, closed_by FROM thread_state \
             WHERE community_id = $1 AND root_event_id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(root.id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .expect("thread state")
        .map(|row| (row.get("last_seq"), row.get("closed"), row.get("closed_by")))
    }

    async fn main_last_seq(&self, room: Uuid) -> Option<i64> {
        sqlx::query_scalar("SELECT main_last_seq FROM channels WHERE community_id = $1 AND id = $2")
            .bind(self.scope.as_uuid())
            .bind(room)
            .fetch_one(&self.pool)
            .await
            .expect("channel row")
    }

    async fn delete(&self, event: &Event) {
        sqlx::query("UPDATE events SET deleted_at = now() WHERE community_id = $1 AND id = $2")
            .bind(self.scope.as_uuid())
            .bind(event.id.as_bytes().as_slice())
            .execute(&self.pool)
            .await
            .expect("soft delete");
    }
}

async fn backfill_community(
    db: &Db,
    scope: CommunityId,
    options: ChatBackfillOptions,
) -> crate::Result<ChatBackfillCounts> {
    let mut total = ChatBackfillCounts::default();
    let mut after = None;
    loop {
        let page = db.chat_backfill_channels(scope, after, 2).await?;
        let Some(last) = page.last().copied() else {
            return Ok(total);
        };
        for channel in page {
            total += db
                .backfill_chat_state_channel(scope, channel, &options)
                .await?;
        }
        after = Some(last);
    }
}

fn now_options() -> ChatBackfillOptions {
    ChatBackfillOptions::new(Utc::now(), false)
}

/// Cast of a scripted room: `dave` is mentioned but never a member.
struct Cast {
    owner: Keys,
    alice: Keys,
    bob: Keys,
    carol: Keys,
    dave: Keys,
}

impl Cast {
    fn new() -> Self {
        Self {
            owner: Keys::generate(),
            alice: Keys::generate(),
            bob: Keys::generate(),
            carol: Keys::generate(),
            dave: Keys::generate(),
        }
    }
}

/// Signed events of the scripted history, shared by every community.
struct History {
    a: Event,
    a1: Event,
    a2: Event,
    b: Event,
    b1: Event,
    c: Event,
    c1: Event,
    c_close: Event,
    d: Event,
    d1: Event,
    d_close: Event,
    d2: Event,
    e: Event,
    e1: Event,
    e_close: Event,
    e_reopen: Event,
    f: Event,
    f_close: Event,
    g: Event,
    g_close: Event,
}

impl History {
    fn new(cast: &Cast) -> Self {
        let a = sign(&cast.owner, 100, vec![p_tag(&cast.alice)]);
        let a1 = sign(&cast.alice, 101, vec![p_tag(&cast.bob)]);
        let a2 = sign(&cast.carol, 102, Vec::new());
        let b = sign(&cast.bob, 110, Vec::new());
        let b1 = sign(&cast.carol, 111, Vec::new());
        let c = sign(&cast.owner, 120, Vec::new());
        let c1 = sign(&cast.alice, 121, Vec::new());
        let c_close = sign(&cast.owner, 122, vec![marker_tag(THREAD_CLOSED_TAG, &c)]);
        let d = sign(&cast.bob, 130, Vec::new());
        let d1 = sign(&cast.carol, 131, Vec::new());
        let d_close = sign(&cast.owner, 132, vec![marker_tag(THREAD_CLOSED_TAG, &d)]);
        let d2 = sign(&cast.alice, 133, Vec::new());
        let e = sign(&cast.carol, 140, vec![p_tag(&cast.dave)]);
        let e1 = sign(&cast.owner, 141, Vec::new());
        let e_close = sign(&cast.owner, 142, vec![marker_tag(THREAD_CLOSED_TAG, &e)]);
        let e_reopen = sign(&cast.bob, 143, vec![marker_tag(THREAD_REOPENED_TAG, &e)]);
        let f = sign(&cast.owner, 150, Vec::new());
        let f_close = sign(&cast.owner, 151, vec![marker_tag(THREAD_CLOSED_TAG, &f)]);
        let g = sign(&cast.owner, 160, Vec::new());
        let g_close = sign(&cast.alice, 161, vec![marker_tag(THREAD_CLOSED_TAG, &g)]);
        Self {
            a,
            a1,
            a2,
            b,
            b1,
            c,
            c1,
            c_close,
            d,
            d1,
            d_close,
            d2,
            e,
            e1,
            e_close,
            e_reopen,
            f,
            f_close,
            g,
            g_close,
        }
    }

    fn steps(&self) -> Vec<Step<'_>> {
        let h = self;
        vec![
            top(&h.a),
            reply(&h.a, &h.a1),
            // Nested reply: parent a1, root a.
            Step {
                event: &h.a2,
                parent: Some(&h.a1),
                root: Some(&h.a),
                broadcast: false,
            },
            top(&h.b),
            // Broadcast reply: shows on the main timeline.
            Step {
                broadcast: true,
                ..reply(&h.b, &h.b1)
            },
            top(&h.c),
            reply(&h.c, &h.c1),
            reply(&h.c, &h.c_close),
            top(&h.d),
            reply(&h.d, &h.d1),
            reply(&h.d, &h.d_close),
            reply(&h.d, &h.d2),
            top(&h.e),
            reply(&h.e, &h.e1),
            reply(&h.e, &h.e_close),
            reply(&h.e, &h.e_reopen),
            top(&h.f),
            reply(&h.f, &h.f_close),
            top(&h.g),
            // A broadcast close marker is a marker, not a main message.
            Step {
                broadcast: true,
                ..reply(&h.g, &h.g_close)
            },
        ]
    }
}

async fn scripted_room(f: &Fixture, cast: &Cast, history: &History) -> Uuid {
    let room = f
        .room(&cast.owner, &[&cast.alice, &cast.bob, &cast.carol])
        .await;
    f.play(room, &history.steps()).await;
    room
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_reaches_the_live_write_path_state() {
    let cast = Cast::new();
    let history = History::new(&cast);
    let live = Fixture::new(true).await;
    let old = Fixture::new(false).await;
    scripted_room(&live, &cast, &history).await;
    let room = scripted_room(&old, &cast, &history).await;

    assert!(
        old.thread_snapshot().await.is_empty(),
        "switch off writes nothing"
    );
    old.backfill(now_options()).await;

    assert_eq!(old.main_snapshot().await, live.main_snapshot().await);
    assert_eq!(old.thread_snapshot().await, live.thread_snapshot().await);
    assert_eq!(old.follow_snapshot().await, live.follow_snapshot().await);

    // Spelled out for the backfilled side.
    let h = &history;
    assert_eq!(old.main_last_seq(room).await, Some(old.seq(&h.g).await));
    assert_eq!(
        old.thread(&h.a).await,
        Some((Some(old.seq(&h.a2).await), false, None)),
        "nested replies count toward the root's thread"
    );
    assert_eq!(
        old.thread(&h.c).await,
        Some((Some(old.seq(&h.c1).await), true, Some(pk(&cast.owner)))),
        "close marker as the latest reply closes; markers are not last_seq"
    );
    assert_eq!(
        old.thread(&h.d).await,
        Some((Some(old.seq(&h.d2).await), false, None)),
        "a reply after the close reopens"
    );
    assert_eq!(
        old.thread(&h.e).await,
        Some((Some(old.seq(&h.e1).await), false, None)),
        "a reopen marker reopens"
    );
    assert_eq!(
        old.thread(&h.f).await,
        Some((None, true, Some(pk(&cast.owner))))
    );
    assert_eq!(
        old.thread(&h.g).await,
        Some((None, true, Some(pk(&cast.alice))))
    );

    // Follows: root author, repliers, mentions in root and replies; members
    // only; marker authors and marker-only threads get nothing.
    let a_last = Some(old.seq(&h.a2).await);
    for who in [&cast.owner, &cast.alice, &cast.bob, &cast.carol] {
        assert_eq!(
            old.follow(&h.a, who).await,
            Some(("auto".into(), "backfill".into(), a_last)),
        );
    }
    assert_eq!(
        old.follow(&h.e, &cast.dave).await,
        None,
        "non-member mention"
    );
    assert_eq!(
        old.follow(&h.e, &cast.bob).await,
        None,
        "reopen marker author"
    );
    assert!(old.follow(&h.e, &cast.carol).await.is_some());
    assert_eq!(
        old.follow(&h.f, &cast.owner).await,
        None,
        "marker-only thread"
    );
    assert_eq!(
        old.follow(&h.g, &cast.alice).await,
        None,
        "marker-only thread"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_is_idempotent_and_dry_run_writes_nothing() {
    let cast = Cast::new();
    let history = History::new(&cast);
    let f = Fixture::new(false).await;
    scripted_room(&f, &cast, &history).await;
    f.room(&cast.owner, &[&cast.alice]).await;
    f.room(&cast.bob, &[]).await;
    let now = Utc::now();
    let before = f.full_snapshot().await;

    let dry = f.backfill(ChatBackfillOptions::new(now, true)).await;
    assert_eq!(f.full_snapshot().await, before, "dry run rolls back");
    assert_eq!(dry.channels, 3);
    assert!(!dry.is_unchanged());

    let first = f.backfill(ChatBackfillOptions::new(now, false)).await;
    assert_eq!(first, dry, "dry run reports what a real run writes");
    assert_eq!(first.main_last_updated, 1);
    assert_eq!(first.thread_state_inserted, 7);
    assert_eq!(first.thread_state_updated, 0);
    assert_eq!(first.threads_closed, 3);
    assert_eq!(first.member_reads_filled, 4 + 2 + 1);
    let after_first = f.full_snapshot().await;
    assert_ne!(after_first, before);

    let second = f.backfill(ChatBackfillOptions::new(now, false)).await;
    assert!(second.is_unchanged(), "re-run writes nothing: {second:?}");
    assert_eq!(second.channels, 3);
    assert_eq!(f.full_snapshot().await, after_first);

    // A later run with a later clock still changes nothing.
    let later = f
        .backfill(ChatBackfillOptions::new(now + Duration::hours(1), false))
        .await;
    assert!(later.is_unchanged(), "{later:?}");
    assert_eq!(f.full_snapshot().await, after_first);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_fills_member_reads_without_overwriting() {
    let f = Fixture::new(false).await;
    let (owner, user, live, unread_only, gone) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    let room = f.room(&owner, &[&user, &live, &unread_only, &gone]).await;
    let root = sign(&owner, 100, Vec::new());
    f.play(room, &[top(&root)]).await;
    let answer = sign(&user, 101, Vec::new());
    f.play(room, &[reply(&root, &answer)]).await;
    channel_members::remove_member(&f.pool, f.scope, room, &pk(&gone), &pk(&owner))
        .await
        .expect("remove member");

    let set = |keys: &Keys, seq: Option<i64>, origin: Option<&'static str>| {
        let pool = f.pool.clone();
        let key = pk(keys);
        let scope = f.scope;
        async move {
            sqlx::query(
                "UPDATE channel_members SET read_seq = $4, read_origin = $5 \
                 WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
            )
            .bind(scope.as_uuid())
            .bind(room)
            .bind(key)
            .bind(seq)
            .bind(origin)
            .execute(&pool)
            .await
            .expect("seed read position");
        }
    };
    set(&user, Some(1), Some("user")).await;
    set(&live, Some(2), None).await;
    set(&unread_only, None, Some("user")).await;

    let counts = f.backfill(now_options()).await;
    assert_eq!(counts.member_reads_filled, 1);

    let current: i64 = sqlx::query_scalar(
        "SELECT accepted_sequence FROM channels WHERE community_id = $1 AND id = $2",
    )
    .bind(f.scope.as_uuid())
    .bind(room)
    .fetch_one(&f.pool)
    .await
    .expect("accepted sequence");
    assert_eq!(current, f.seq(&answer).await);
    assert_eq!(
        f.member(room, &owner).await,
        (Some(current), Some("backfill".into()))
    );
    assert_eq!(f.member(room, &user).await, (Some(1), Some("user".into())));
    assert_eq!(f.member(room, &live).await, (Some(2), None));
    assert_eq!(
        f.member(room, &unread_only).await,
        (None, Some("user".into()))
    );
    assert_eq!(f.member(room, &gone).await, (None, None), "removed member");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_follows_members_in_window_and_keeps_existing_rows() {
    let f = Fixture::new(false).await;
    let (owner, alice, bob, gone, outsider) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    let room = f.room(&owner, &[&alice, &bob, &gone]).await;
    let root = sign(&owner, 100, vec![p_tag(&gone), p_tag(&outsider)]);
    let answer = sign(&alice, 101, vec![p_tag(&bob)]);
    let gone_reply = sign(&gone, 102, Vec::new());
    f.play(
        room,
        &[top(&root), reply(&root, &answer), reply(&root, &gone_reply)],
    )
    .await;
    channel_members::remove_member(&f.pool, f.scope, room, &pk(&gone), &pk(&owner))
        .await
        .expect("remove member");
    // bob unfollowed on a device before the backfill ran.
    sqlx::query(
        "INSERT INTO thread_follows (community_id, root_event_id, pubkey, channel_id, \
             follow_state, follow_origin) VALUES ($1, $2, $3, $4, 'unfollowed', 'user')",
    )
    .bind(f.scope.as_uuid())
    .bind(root.id.as_bytes().as_slice())
    .bind(pk(&bob))
    .bind(room)
    .execute(&f.pool)
    .await
    .expect("existing unfollow");

    // Latest reply older than the 30-day window: thread state, no follows.
    let stale = f
        .backfill(ChatBackfillOptions::new(
            Utc::now() + Duration::days(31),
            false,
        ))
        .await;
    assert_eq!(stale.thread_state_inserted, 1);
    assert_eq!(stale.follows_inserted, 0);
    assert_eq!(f.follow(&root, &owner).await, None);

    // Inside the window: follows for current members only.
    let fresh = f
        .backfill(ChatBackfillOptions::new(
            Utc::now() + Duration::days(29),
            false,
        ))
        .await;
    assert_eq!(fresh.follows_inserted, 2);
    let last = Some(f.seq(&gone_reply).await);
    for who in [&owner, &alice] {
        assert_eq!(
            f.follow(&root, who).await,
            Some(("auto".into(), "backfill".into(), last))
        );
    }
    assert_eq!(
        f.follow(&root, &bob).await,
        Some(("unfollowed".into(), "user".into(), None)),
        "an existing row is never touched"
    );
    assert_eq!(f.follow(&root, &gone).await, None, "removed member");
    assert_eq!(f.follow(&root, &outsider).await, None, "never a member");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_reads_live_messages_and_never_moves_state_back() {
    let f = Fixture::new(false).await;
    let owner = Keys::generate();
    let room = f.room(&owner, &[]).await;
    let kept = sign(&owner, 100, Vec::new());
    let dropped_root = sign(&owner, 101, Vec::new());
    let orphan = sign(&owner, 102, Vec::new());
    let root = sign(&owner, 103, Vec::new());
    let answer = sign(&owner, 104, Vec::new());
    let dropped_answer = sign(&owner, 105, Vec::new());
    let dropped_top = sign(&owner, 106, Vec::new());
    f.play(
        room,
        &[
            top(&kept),
            top(&dropped_root),
            reply(&dropped_root, &orphan),
            top(&root),
            reply(&root, &answer),
            reply(&root, &dropped_answer),
            top(&dropped_top),
        ],
    )
    .await;
    for event in [&dropped_root, &dropped_answer, &dropped_top] {
        f.delete(event).await;
    }

    f.backfill(now_options()).await;
    assert_eq!(f.main_last_seq(room).await, Some(f.seq(&root).await));
    assert_eq!(
        f.thread(&dropped_root).await,
        None,
        "deleted root: no thread"
    );
    assert_eq!(
        f.thread(&root).await,
        Some((Some(f.seq(&answer).await), false, None))
    );

    // State already ahead of history (written live) is never lowered.
    sqlx::query("UPDATE channels SET main_last_seq = 1000 WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(room)
        .execute(&f.pool)
        .await
        .expect("advance main");
    sqlx::query(
        "UPDATE thread_state SET last_seq = 1000 WHERE community_id = $1 AND root_event_id = $2",
    )
    .bind(f.scope.as_uuid())
    .bind(root.id.as_bytes().as_slice())
    .execute(&f.pool)
    .await
    .expect("advance thread");
    let again = f.backfill(now_options()).await;
    assert!(again.is_unchanged(), "{again:?}");
    assert_eq!(f.main_last_seq(room).await, Some(1000));
    assert_eq!(f.thread(&root).await, Some((Some(1000), false, None)));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_is_tenant_scoped_and_respects_the_fence() {
    let cast = Cast::new();
    let history = History::new(&cast);
    let f = Fixture::new(false).await;
    let other = Fixture::new(false).await;
    scripted_room(&f, &cast, &history).await;
    let other_room = scripted_room(&other, &cast, &history).await;
    let other_before = other.full_snapshot().await;

    let listed =
        f.db.chat_backfill_channels(f.scope, None, 100)
            .await
            .expect("list channels");
    assert!(!listed.contains(&other_room));
    f.backfill(now_options()).await;
    assert!(!f.thread_snapshot().await.is_empty());
    assert_eq!(
        other.full_snapshot().await,
        other_before,
        "other tenant untouched"
    );

    // A community being deleted is fenced: the backfill fails before writing.
    let mut tx = f.pool.begin().await.expect("tx");
    sqlx::query(
        "SELECT set_config('buzz.deletion_executor_community', $1, true), \
                set_config('buzz.deletion_fence_generation', '1', true)",
    )
    .bind(other.scope.as_uuid().to_string())
    .execute(&mut *tx)
    .await
    .expect("executor identity");
    sqlx::query(
        "UPDATE communities SET deletion_state = 'fenced', deletion_fence_generation = 1 \
         WHERE id = $1",
    )
    .bind(other.scope.as_uuid())
    .execute(&mut *tx)
    .await
    .expect("fence community");
    tx.commit().await.expect("commit fence");

    for dry_run in [true, false] {
        let rejected = other
            .db
            .backfill_chat_state_channel(
                other.scope,
                other_room,
                &ChatBackfillOptions::new(Utc::now(), dry_run),
            )
            .await
            .expect_err("fenced community must reject the backfill");
        assert!(
            rejected.to_string().contains("community write fenced"),
            "unexpected rejection: {rejected}"
        );
    }
    assert_eq!(other.full_snapshot().await, other_before);
}
