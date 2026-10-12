//! Inbox summary against real channel, member, event and thread writers.
use buzz_core::CommunityId;
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::inbox_summary::{InboxCursor, InboxReadMarkers, InboxRowKind};
use crate::{channel, channel_members, event, thread, Db};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
    reader: Keys,
    other: Keys,
}

fn at(secs: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs as i64, 0).expect("timestamp")
}

impl Fixture {
    async fn new() -> Self {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("isolated test database");
        if std::env::var("BUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        let scope = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(scope.as_uuid())
            .bind(format!("inbox-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()),
            pool,
            scope,
            reader: Keys::generate(),
            other: Keys::generate(),
        }
    }

    async fn room(&self, name: &str, join: bool) -> Uuid {
        let id = channel::create_channel(
            &self.pool,
            self.scope,
            name,
            ChannelType::Stream,
            ChannelVisibility::Open,
            None,
            &self.other.public_key().to_bytes(),
            None,
        )
        .await
        .expect("channel")
        .id;
        if join {
            channel_members::add_member(
                &self.pool,
                self.scope,
                id,
                &self.reader.public_key().to_bytes(),
                MemberRole::Member,
                None,
            )
            .await
            .expect("member");
        }
        id
    }

    async fn post(&self, room: Uuid, keys: &Keys, content: &str, ts: u64) -> Event {
        let message = EventBuilder::new(Kind::Custom(9), content)
            .tags([Tag::parse(["h", &room.to_string()]).expect("h tag")])
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("signature");
        event::insert_event(&self.pool, self.scope, &message, Some(room))
            .await
            .expect("event writer");
        message
    }

    async fn reply(&self, room: Uuid, root: &Event, keys: &Keys, content: &str, ts: u64) {
        let reply = self.post(room, keys, content, ts).await;
        let root_at = at(root.created_at.as_u64());
        thread::insert_thread_metadata(
            &self.pool,
            self.scope,
            root.id.as_bytes(),
            root_at,
            room,
            None,
            None,
            None,
            None,
            0,
            false,
        )
        .await
        .ok(); // The root row already exists after the first reply.
        thread::insert_thread_metadata(
            &self.pool,
            self.scope,
            reply.id.as_bytes(),
            at(ts),
            room,
            Some(root.id.as_bytes()),
            Some(root_at),
            Some(root.id.as_bytes()),
            Some(root_at),
            1,
            false,
        )
        .await
        .expect("reply metadata");
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn mixes_rooms_and_threads_newest_first_with_unread_counts() {
    let f = Fixture::new().await;
    let busy = f.room("busy", true).await;
    let quiet = f.room("quiet", true).await;
    let outside = f.room("outside", false).await;

    f.post(quiet, &f.other, "quiet hello", 100).await;
    let root = f.post(busy, &f.reader, "thread root", 200).await;
    f.post(busy, &f.other, "main after root", 300).await;
    f.reply(busy, &root, &f.other, "reply one", 400).await;
    f.reply(busy, &root, &f.other, "reply two", 500).await;
    f.post(outside, &f.other, "not mine", 600).await;

    let markers = InboxReadMarkers {
        channels: vec![(busy, at(250))],
        threads: vec![(root.id.as_bytes().to_vec(), at(450))],
    };
    let page =
        f.db.inbox_summary(f.scope, &f.reader.public_key(), 30, None, &markers)
            .await
            .expect("summary");
    assert!(!page.has_more);
    let shape: Vec<_> = page.rows.iter().map(|r| (r.kind, r.channel_id)).collect();
    assert_eq!(
        shape,
        vec![
            (InboxRowKind::Thread, busy),
            (InboxRowKind::Room, busy),
            (InboxRowKind::Room, quiet),
        ],
        "thread replies sort the thread row; replies never become the room preview"
    );
    let thread_row = &page.rows[0];
    assert_eq!(thread_row.reply_count, Some(2));
    assert_eq!(thread_row.latest.as_ref().unwrap()["content"], "reply two");
    assert_eq!(thread_row.root.as_ref().unwrap()["content"], "thread root");
    assert_eq!(
        thread_row.unread,
        Some(1),
        "only replies after the thread marker"
    );
    let busy_row = &page.rows[1];
    assert_eq!(
        busy_row.latest.as_ref().unwrap()["content"],
        "main after root"
    );
    assert_eq!(
        busy_row.unread,
        Some(1),
        "own root and thread replies are not counted"
    );
    assert_eq!(
        page.rows[2].unread, None,
        "no marker means unknown, not zero"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn pages_with_keyset_cursor_without_gaps_or_repeats() {
    let f = Fixture::new().await;
    let mut expected = Vec::new();
    for index in 0..5u64 {
        let room = f.room(&format!("room-{index}"), true).await;
        f.post(room, &f.other, "hi", 1_000 + index).await;
        expected.push(room);
    }
    expected.reverse();
    let reader = f.reader.public_key();
    let markers = InboxReadMarkers::default();
    let first =
        f.db.inbox_summary(f.scope, &reader, 2, None, &markers)
            .await
            .expect("first page");
    assert!(first.has_more);
    let mut seen: Vec<Uuid> = first.rows.iter().map(|r| r.channel_id).collect();
    let mut cursor = first.rows.last().map(|r| InboxCursor {
        at: r.at,
        key: r.key.clone(),
    });
    while let Some(before) = cursor {
        let page =
            f.db.inbox_summary(f.scope, &reader, 2, Some(&before), &markers)
                .await
                .expect("next page");
        seen.extend(page.rows.iter().map(|r| r.channel_id));
        cursor = page.has_more.then(|| {
            let last = page.rows.last().expect("non-empty page");
            InboxCursor {
                at: last.at,
                key: last.key.clone(),
            }
        });
    }
    assert_eq!(seen, expected);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn excludes_left_and_archived_rooms_and_deleted_previews() {
    let f = Fixture::new().await;
    let left = f.room("left", true).await;
    let archived = f.room("archived", true).await;
    let kept = f.room("kept", true).await;
    f.post(left, &f.other, "x", 100).await;
    f.post(archived, &f.other, "x", 100).await;
    f.post(kept, &f.other, "visible", 100).await;
    let gone = f.post(kept, &f.other, "deleted later", 200).await;
    channel_members::remove_member(
        &f.pool,
        f.scope,
        left,
        &f.reader.public_key().to_bytes(),
        &f.reader.public_key().to_bytes(),
    )
    .await
    .expect("leave");
    channel::archive_channel(&f.pool, f.scope, archived)
        .await
        .expect("archive");
    sqlx::query("UPDATE events SET deleted_at = now() WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(gone.id.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .expect("soft delete");

    let page =
        f.db.inbox_summary(
            f.scope,
            &f.reader.public_key(),
            30,
            None,
            &InboxReadMarkers::default(),
        )
        .await
        .expect("summary");
    assert_eq!(page.rows.len(), 1);
    assert_eq!(page.rows[0].channel_id, kept);
    assert_eq!(page.rows[0].latest.as_ref().unwrap()["content"], "visible");
}
