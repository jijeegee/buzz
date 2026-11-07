//! Window checks (NIP-CW `check`) and the window block filter against a real
//! database: a check answers exactly the page the same request would serve,
//! and 1:1 DM rows hidden by block periods are dropped before the page limit.
use chrono::{DateTime, Utc};
use kuzz_core::thread_window::Request;
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::event::ThreadMetadataParams;
use crate::thread::ChannelWindow;
use crate::thread_window::ScanBudget;
use crate::{channel, channel_members, dm, Db};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

fn ts(event: &Event) -> DateTime<Utc> {
    DateTime::from_timestamp(event.created_at.as_secs() as i64, 0).expect("timestamp")
}

fn ids(window: &ChannelWindow) -> Vec<String> {
    window
        .rows
        .iter()
        .map(|row| row.stored_event.event.id.to_hex())
        .collect()
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
            .bind(format!("window-check-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()),
            pool,
            scope,
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

    /// A kind-9 message at `secs`; with `root`, a depth-1 reply in its thread.
    async fn post(&self, room: Uuid, keys: &Keys, secs: u64, root: Option<&Event>) -> Event {
        let mut tags = vec![Tag::parse(["h", &room.to_string()]).expect("h tag")];
        if let Some(root) = root {
            tags.push(Tag::parse(["e", &root.id.to_hex(), "", "root"]).expect("e tag"));
        }
        let event = EventBuilder::new(Kind::Custom(9), format!("message at {secs}"))
            .tags(tags)
            .custom_created_at(Timestamp::from(secs))
            .sign_with_keys(keys)
            .expect("signature");
        let meta = root.map(|root| ThreadMetadataParams {
            event_id: event.id.as_bytes(),
            event_created_at: ts(&event),
            channel_id: room,
            parent_event_id: Some(root.id.as_bytes()),
            parent_event_created_at: Some(ts(root)),
            root_event_id: Some(root.id.as_bytes()),
            root_event_created_at: Some(ts(root)),
            depth: 1,
            broadcast: false,
        });
        self.db
            .insert_event_with_thread_metadata_and_chat_state(self.scope, &event, Some(room), meta)
            .await
            .expect("message write");
        event
    }

    async fn page(
        &self,
        room: Uuid,
        reader: &Keys,
        blocked: bool,
        limit: u32,
        cursor: Option<&Event>,
    ) -> ChannelWindow {
        let reader = pk(reader);
        self.db
            .get_channel_window_with_session(
                self.scope,
                room,
                limit,
                cursor.map(|e| (ts(e), e.id.to_bytes().to_vec())),
                Some(&[9]),
                Some(&reader),
                blocked.then_some(reader.as_slice()),
            )
            .await
            .expect("page")
            .0
    }

    async fn check(
        &self,
        room: Uuid,
        reader: &Keys,
        blocked: bool,
        limit: u32,
        cursor: Option<&Event>,
    ) -> (ChannelWindow, i64) {
        let reader = pk(reader);
        self.db
            .check_channel_window(
                self.scope,
                room,
                limit,
                cursor.map(|e| (ts(e), e.id.to_bytes().to_vec())),
                Some(&[9]),
                Some(&reader),
                blocked.then_some(reader.as_slice()),
            )
            .await
            .expect("check")
    }

    async fn room_seq(&self, room: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT accepted_sequence FROM channels WHERE community_id = $1 AND id = $2",
        )
        .bind(self.scope.as_uuid())
        .bind(room)
        .fetch_one(&self.pool)
        .await
        .expect("room sequence")
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn room_window_check_answers_the_page_it_checks() {
    let f = Fixture::new().await;
    let owner = Keys::generate();
    let room = f.room(&owner, &[]).await;
    let mut posts = Vec::new();
    for secs in 100..105 {
        posts.push(f.post(room, &owner, secs, None).await);
    }
    let hex = |i: usize| posts[i].id.to_hex();

    // Full window: the check's oldest row is the page's last row.
    let page = f.page(room, &owner, false, 3, None).await;
    let (check, room_seq) = f.check(room, &owner, false, 3, None).await;
    assert_eq!(ids(&page), vec![hex(4), hex(3), hex(2)]);
    assert_eq!(ids(&check), ids(&page));
    assert!(check.has_more);
    assert_eq!(check.next_cursor, page.next_cursor);
    assert_eq!(room_seq, f.room_seq(room).await);
    assert!(room_seq > 0);

    // Short window below a cursor: fewer rows than the limit, no more.
    let (short, _) = f.check(room, &owner, false, 3, Some(&posts[2])).await;
    assert_eq!(ids(&short), vec![hex(1), hex(0)]);
    assert!(!short.has_more);

    // A deleted row leaves the window, which reaches one row further.
    assert!(f
        .db
        .soft_delete_event_and_update_thread(f.scope, posts[3].id.as_bytes(), None, None)
        .await
        .expect("delete"));
    let (after_delete, _) = f.check(room, &owner, false, 3, None).await;
    assert_eq!(ids(&after_delete), vec![hex(4), hex(2), hex(1)]);
    assert_eq!(
        ids(&after_delete),
        ids(&f.page(room, &owner, false, 3, None).await)
    );

    // Nothing below the oldest row: an empty window.
    let (empty, _) = f.check(room, &owner, false, 3, Some(&posts[0])).await;
    assert!(empty.rows.is_empty());
    assert!(!empty.has_more);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn blocked_dm_rows_are_dropped_before_the_window_limit() {
    let f = Fixture::new().await;
    let (a, b, c) = (Keys::generate(), Keys::generate(), Keys::generate());
    let direct = dm::create_dm(&f.pool, f.scope, &[&pk(&a), &pk(&b)], &pk(&a))
        .await
        .expect("dm")
        .id;
    let group = dm::create_dm(&f.pool, f.scope, &[&pk(&a), &pk(&b), &pk(&c)], &pk(&a))
        .await
        .expect("group dm")
        .id;

    let before = f.post(direct, &b, 100, None).await;
    f.db.block_user(f.scope, &pk(&a), &pk(&b))
        .await
        .expect("block");
    for secs in 101..104 {
        f.post(direct, &b, secs, None).await;
    }
    let mine = f.post(direct, &a, 104, None).await;

    // A's two newest rows skip the three hidden ones: the page is full.
    let page = f.page(direct, &a, true, 2, None).await;
    assert_eq!(ids(&page), vec![mine.id.to_hex(), before.id.to_hex()]);
    assert!(!page.has_more);
    let (check, _) = f.check(direct, &a, true, 2, None).await;
    assert_eq!(ids(&check), ids(&page));
    assert!(!check.has_more);
    // Without the filter the hidden rows would take the limit.
    let unfiltered = f.page(direct, &a, false, 2, None).await;
    assert_eq!(unfiltered.rows.len(), 2);
    assert!(unfiltered.has_more);
    // B never blocked anyone: B sees everything.
    assert_eq!(f.page(direct, &b, true, 10, None).await.rows.len(), 5);

    // Group DMs are not filtered.
    f.post(group, &b, 105, None).await;
    assert_eq!(f.page(group, &a, true, 10, None).await.rows.len(), 1);

    // Thread windows drop the same rows before their limit.
    let root = f.post(direct, &a, 110, None).await;
    f.post(direct, &b, 111, Some(&root)).await;
    let reply = f.post(direct, &a, 112, Some(&root)).await;
    f.post(direct, &b, 113, Some(&root)).await;
    let request = Request::parse(&serde_json::json!({"thread_window": true,
        "#h": [direct], "#e": [root.id.to_hex()], "kinds": [9], "limit": 1}))
    .expect("request");
    let reader = pk(&a);
    let (window, _) =
        f.db.get_thread_window_for_reader_with_session(
            f.scope,
            &request,
            Some(&reader),
            Some(&reader),
            &mut ScanBudget::default(),
        )
        .await
        .expect("thread page");
    let reply_ids = |rows: &[kuzz_core::StoredEvent]| -> Vec<String> {
        rows.iter().map(|row| row.event.id.to_hex()).collect()
    };
    assert_eq!(reply_ids(&window.rows), vec![reply.id.to_hex()]);
    assert!(!window.has_more);
    let (checked, room_seq) =
        f.db.check_thread_window(
            f.scope,
            &request,
            Some(&reader),
            Some(&reader),
            &mut ScanBudget::default(),
        )
        .await
        .expect("thread check");
    assert!(checked.root_in_channel);
    assert_eq!(reply_ids(&checked.rows), reply_ids(&window.rows));
    assert!(!checked.has_more);
    assert_eq!(room_seq, f.room_seq(direct).await);
    // Unfiltered, B's newest reply would fill the one-row page.
    let (unfiltered, _) =
        f.db.check_thread_window(
            f.scope,
            &request,
            Some(&reader),
            None,
            &mut ScanBudget::default(),
        )
        .await
        .expect("unfiltered thread check");
    assert_ne!(reply_ids(&unfiltered.rows), reply_ids(&window.rows));
    assert!(unfiltered.has_more);
}
