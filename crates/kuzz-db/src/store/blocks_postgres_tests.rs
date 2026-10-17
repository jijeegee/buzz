//! Silent blocks and people lookup against a real database: the block store,
//! the read paths that hide a blocked author's DM events, and the people
//! lookup/search queries.
use chrono::{Duration, Utc};
use kuzz_core::principal::PrincipalId;
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::blocks::{BlockOutcome, MAX_BLOCKS_PER_USER};
use crate::channel::{ChannelType, ChannelVisibility, MemberRole};
use crate::chat_list::{ChatListQuery, ChatRowKind};
use crate::inbox_summary::InboxReadMarkers;
use crate::people::PersonLookup;
use crate::{channel, channel_members, dm, Db, EventQuery};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

fn chats() -> ChatListQuery {
    ChatListQuery {
        limit: 50,
        before: None,
        pinned_threads: Vec::new(),
        quiet_before: Utc::now() - Duration::days(7),
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
            .bind(format!("blocks-{}.test", scope.as_uuid()))
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

    async fn post(&self, room: Uuid, keys: &Keys, ts: u64) -> Event {
        let event = EventBuilder::new(Kind::Custom(9), format!("message at {ts}"))
            .tags([Tag::parse(["h", &room.to_string()]).expect("h tag")])
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("signature");
        self.db
            .insert_event_with_thread_metadata_and_chat_state(self.scope, &event, Some(room), None)
            .await
            .expect("message write");
        event
    }

    async fn block(&self, blocker: &Keys, blocked: &Keys) {
        let outcome = self
            .db
            .block_user(self.scope, &pk(blocker), &pk(blocked))
            .await
            .expect("block");
        assert_eq!(outcome, BlockOutcome::Added);
    }

    async fn history(&self, reader: Option<&Keys>, rooms: &[Uuid]) -> Vec<Vec<u8>> {
        let mut q = EventQuery::for_community(self.scope);
        q.kinds = Some(vec![9]);
        q.channel_ids = Some(rooms.to_vec());
        q.channel_ids_include_global = false;
        q.blocked_dm_reader = reader.map(pk);
        self.db
            .query_events(&q)
            .await
            .expect("history")
            .into_iter()
            .map(|stored| stored.event.pubkey.to_bytes().to_vec())
            .collect()
    }

    async fn count(&self, reader: &Keys, room: Uuid) -> i64 {
        let mut q = EventQuery::for_community(self.scope);
        q.kinds = Some(vec![9]);
        q.channel_id = Some(room);
        q.blocked_dm_reader = Some(pk(reader));
        self.db.count_events(&q).await.expect("count")
    }

    async fn principal(&self, kind: &str, name: &str, username: Option<&str>) -> PrincipalId {
        let id = PrincipalId::generate();
        sqlx::query(
            "INSERT INTO principals (id, kind, display_name, username) VALUES ($1, $2, $3, $4)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(kind)
        .bind(name)
        .bind(username)
        .execute(&self.pool)
        .await
        .expect("principal");
        id
    }

    async fn email(&self, id: &PrincipalId, email: &str) {
        sqlx::query(
            "INSERT INTO identities (provider, subject, principal_id, email) \
             VALUES ('google', $1, $2, $3)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(id.as_bytes().as_slice())
        .bind(email)
        .execute(&self.pool)
        .await
        .expect("identity");
    }

    async fn user_row(&self, keys: &Keys, name: &str, owner: Option<&Keys>) {
        sqlx::query(
            "INSERT INTO users (community_id, pubkey, display_name, agent_owner_pubkey) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(self.scope.as_uuid())
        .bind(pk(keys))
        .bind(name)
        .bind(owner.map(pk))
        .execute(&self.pool)
        .await
        .expect("users row");
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn block_store_is_idempotent_private_and_capped() {
    let f = Fixture::new().await;
    let (a, b, c) = (Keys::generate(), Keys::generate(), Keys::generate());

    f.block(&a, &b).await;
    assert_eq!(
        f.db.block_user(f.scope, &pk(&a), &pk(&b)).await.unwrap(),
        BlockOutcome::AlreadyBlocked
    );
    assert!(f.db.block_user(f.scope, &pk(&a), &pk(&a)).await.is_err());
    f.block(&c, &b).await;

    let listed = f.db.list_blocks(f.scope, &pk(&a)).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].blocked, pk(&b));
    assert!(f.db.list_blocks(f.scope, &pk(&b)).await.unwrap().is_empty());

    let mut blockers = f.db.blockers_of(f.scope, &pk(&b)).await.unwrap();
    blockers.sort();
    let mut expected = vec![pk(&a), pk(&c)];
    expected.sort();
    assert_eq!(blockers, expected);
    assert_eq!(
        f.db.blocked_by(f.scope, &pk(&a)).await.unwrap(),
        vec![pk(&b)]
    );
    assert_eq!(
        f.db.block_pairs(f.scope, &[pk(&a), pk(&b)], &[pk(&b)])
            .await
            .unwrap(),
        vec![(pk(&a), pk(&b))]
    );

    // Another community never sees these rows.
    let other = Fixture::new().await;
    assert!(other
        .db
        .blockers_of(other.scope, &pk(&b))
        .await
        .unwrap()
        .is_empty());

    assert!(f.db.unblock_user(f.scope, &pk(&a), &pk(&b)).await.unwrap());
    assert!(!f.db.unblock_user(f.scope, &pk(&a), &pk(&b)).await.unwrap());
    assert!(f.db.blocked_by(f.scope, &pk(&a)).await.unwrap().is_empty());

    // The cap holds: fill A's list, then one more is refused.
    sqlx::query(
        "INSERT INTO user_blocks (community_id, blocker, blocked) \
         SELECT $1, $2, decode(lpad(to_hex(g), 64, '0'), 'hex') \
           FROM generate_series(1, $3::bigint) g",
    )
    .bind(f.scope.as_uuid())
    .bind(pk(&a))
    .bind(MAX_BLOCKS_PER_USER)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        f.db.block_user(f.scope, &pk(&a), &pk(&b)).await.unwrap(),
        BlockOutcome::LimitReached
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn blocked_author_dm_events_are_hidden_from_blocker_reads() {
    let f = Fixture::new().await;
    let (a, b, c) = (Keys::generate(), Keys::generate(), Keys::generate());
    let direct = dm::create_dm(&f.pool, f.scope, &[&pk(&a), &pk(&b)], &pk(&a))
        .await
        .unwrap()
        .id;
    let room = f.room(&c, &[&a, &b]).await;
    assert_eq!(
        f.db.dm_channel_ids(f.scope, &[direct, room]).await.unwrap(),
        vec![direct]
    );

    f.block(&a, &b).await;
    f.post(direct, &b, 100).await;
    f.post(direct, &a, 101).await;
    f.post(room, &b, 102).await;

    // History: A loses B's DM message only; B and unfiltered reads keep all.
    let seen = f.history(Some(&a), &[direct, room]).await;
    assert_eq!(seen.len(), 2, "A sees own DM message and B's room message");
    assert!(seen.contains(&pk(&a)) && seen.contains(&pk(&b)));
    assert_eq!(f.history(Some(&b), &[direct, room]).await.len(), 3);
    assert_eq!(f.history(None, &[direct, room]).await.len(), 3);
    assert_eq!(f.count(&a, direct).await, 1);
    assert_eq!(f.count(&b, direct).await, 2);
    assert_eq!(f.count(&a, room).await, 1);

    // Chats list: no unread and no preview from B in the DM; the room counts.
    let rows =
        f.db.chat_list(f.scope, &a.public_key(), &chats())
            .await
            .unwrap()
            .rows;
    let dm_row = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Room && r.channel_id == direct)
        .expect("dm row");
    assert_eq!(dm_row.unread, 0);
    let latest = dm_row.latest.as_ref().expect("own message is latest");
    assert_eq!(latest["pubkey"], a.public_key().to_hex());
    let room_row = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Room && r.channel_id == room)
        .expect("room row");
    assert_eq!(room_row.unread, 1);

    // Inbox summary: same rules.
    let markers = InboxReadMarkers {
        channels: vec![
            (direct, Utc::now() - Duration::days(365 * 60)),
            (room, Utc::now() - Duration::days(365 * 60)),
        ],
        threads: Vec::new(),
    };
    let page =
        f.db.inbox_summary(f.scope, &a.public_key(), 50, None, &markers)
            .await
            .unwrap();
    let dm_summary = page.rows.iter().find(|r| r.channel_id == direct).unwrap();
    assert_eq!(dm_summary.unread, Some(0));
    assert_eq!(
        dm_summary.latest.as_ref().unwrap()["pubkey"],
        a.public_key().to_hex()
    );
    let room_summary = page.rows.iter().find(|r| r.channel_id == room).unwrap();
    assert_eq!(room_summary.unread, Some(1));

    // A hid the DM: B's next message does not bring it back.
    dm::hide_dm(&f.pool, f.scope, direct, &pk(&a))
        .await
        .unwrap();
    f.post(direct, &b, 103).await;
    let rows =
        f.db.chat_list(f.scope, &a.public_key(), &chats())
            .await
            .unwrap()
            .rows;
    assert!(!rows.iter().any(|r| r.channel_id == direct), "stays hidden");
    // Without the block a message would unhide it.
    f.db.unblock_user(f.scope, &pk(&a), &pk(&b)).await.unwrap();
    f.post(direct, &b, 104).await;
    let rows =
        f.db.chat_list(f.scope, &a.public_key(), &chats())
            .await
            .unwrap()
            .rows;
    assert!(
        rows.iter().any(|r| r.channel_id == direct),
        "unblocked unhides"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn new_dm_from_blocked_person_is_hidden_for_blocker() {
    let f = Fixture::new().await;
    let (a, b) = (Keys::generate(), Keys::generate());
    f.block(&a, &b).await;

    let (channel, created) = dm::open_dm(&f.pool, f.scope, &[&pk(&a)], &pk(&b))
        .await
        .unwrap();
    assert!(created);
    let a_rows =
        f.db.chat_list(f.scope, &a.public_key(), &chats())
            .await
            .unwrap()
            .rows;
    assert!(!a_rows.iter().any(|r| r.channel_id == channel.id));
    let b_rows =
        f.db.chat_list(f.scope, &b.public_key(), &chats())
            .await
            .unwrap()
            .rows;
    assert!(
        b_rows.iter().any(|r| r.channel_id == channel.id),
        "opener sees it"
    );

    // Reopening as B keeps A hidden; A opening it brings it back for A.
    dm::open_dm(&f.pool, f.scope, &[&pk(&a)], &pk(&b))
        .await
        .unwrap();
    let a_rows =
        f.db.chat_list(f.scope, &a.public_key(), &chats())
            .await
            .unwrap()
            .rows;
    assert!(!a_rows.iter().any(|r| r.channel_id == channel.id));
    dm::open_dm(&f.pool, f.scope, &[&pk(&b)], &pk(&a))
        .await
        .unwrap();
    let a_rows =
        f.db.chat_list(f.scope, &a.public_key(), &chats())
            .await
            .unwrap()
            .rows;
    assert!(a_rows.iter().any(|r| r.channel_id == channel.id));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn people_lookup_is_exact_discoverable_and_never_returns_email() {
    let f = Fixture::new().await;
    let tag = &Uuid::new_v4().simple().to_string()[..10];
    let ada_name = format!("ada_{tag}");
    let ada = f.principal("user", "Ada", Some(&ada_name)).await;
    let email = format!("Ada.{tag}@Example.com");
    f.email(&ada, &email).await;

    let found =
        f.db.lookup_person(f.scope, PersonLookup::Username(&ada_name.to_uppercase()))
            .await
            .unwrap()
            .expect("username match");
    assert_eq!(found.pubkey, ada.as_bytes().to_vec());
    assert_eq!(found.username.as_deref(), Some(ada_name.as_str()));
    let by_email =
        f.db.lookup_person(f.scope, PersonLookup::Email(&email.to_lowercase()))
            .await
            .unwrap()
            .expect("email match");
    assert_eq!(by_email.pubkey, ada.as_bytes().to_vec());
    // Prefixes never match.
    assert!(f
        .db
        .lookup_person(f.scope, PersonLookup::Username(&ada_name[..6]))
        .await
        .unwrap()
        .is_none());

    // Discovery off hides both routes.
    sqlx::query("UPDATE principals SET discoverable = false WHERE id = $1")
        .bind(ada.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    assert!(f
        .db
        .lookup_person(f.scope, PersonLookup::Username(&ada_name))
        .await
        .unwrap()
        .is_none());
    assert!(f
        .db
        .lookup_person(f.scope, PersonLookup::Email(&email))
        .await
        .unwrap()
        .is_none());

    // Bots never match.
    let bot_name = format!("bot_{tag}");
    f.principal("bot", "Helper", Some(&bot_name)).await;
    assert!(f
        .db
        .lookup_person(f.scope, PersonLookup::Username(&bot_name))
        .await
        .unwrap()
        .is_none());

    // Key-backup accounts resolve to their messaging key.
    let kim_name = format!("kim_{tag}");
    let kim = f.principal("user", "Kim", Some(&kim_name)).await;
    let kim_key = Keys::generate();
    sqlx::query("UPDATE principals SET identity_mode = 'key_backup' WHERE id = $1")
        .bind(kim.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO account_key_backups (account_id, pubkey, version, key_id, nonce, ciphertext) \
         VALUES ($1, $2, 2, 'k1', $3, $4)",
    )
    .bind(kim.as_bytes().as_slice())
    .bind(pk(&kim_key))
    .bind(vec![0u8; 12])
    .bind(vec![0u8; 48])
    .execute(&f.pool)
    .await
    .unwrap();
    f.user_row(&kim_key, "Kim Here", None).await;
    let kim_found =
        f.db.lookup_person(f.scope, PersonLookup::Username(&kim_name))
            .await
            .unwrap()
            .expect("key account");
    assert_eq!(kim_found.pubkey, pk(&kim_key));
    assert_eq!(kim_found.display_name, "Kim Here");
    assert_eq!(
        f.db.principal_messaging_pubkey(&kim).await.unwrap(),
        Some(pk(&kim_key))
    );
    assert_eq!(
        f.db.principal_messaging_pubkey(&ada).await.unwrap(),
        Some(ada.as_bytes().to_vec())
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn shared_channel_people_match_prefix_and_skip_self_agents_bots_blocked() {
    let f = Fixture::new().await;
    let reader = Keys::generate();
    let (alice, alfred, alvin, bob, agent, gone) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    let stranger = Keys::generate();
    f.user_row(&reader, "Alma Reader", None).await;
    f.user_row(&alice, "Alice", None).await;
    f.user_row(&alfred, "Alfred", None).await;
    f.user_row(&alvin, "Alvin", None).await;
    f.user_row(&bob, "Bob", None).await;
    f.user_row(&agent, "Alpha Agent", Some(&reader)).await;
    f.user_row(&gone, "Albert", None).await;
    f.user_row(&stranger, "Alex", None).await;
    let room = f
        .room(&reader, &[&alice, &alfred, &alvin, &bob, &agent, &gone])
        .await;
    f.room(&stranger, &[]).await;
    channel_members::remove_member(&f.pool, f.scope, room, &pk(&gone), &pk(&reader))
        .await
        .unwrap();
    f.block(&reader, &alvin).await;

    let names = |people: Vec<crate::people::PersonRecord>| {
        people
            .into_iter()
            .map(|p| p.display_name)
            .collect::<Vec<_>>()
    };
    let found =
        f.db.shared_channel_people(f.scope, &pk(&reader), "AL", 50)
            .await
            .unwrap();
    assert_eq!(names(found), vec!["Alfred", "Alice"]);
    let one =
        f.db.shared_channel_people(f.scope, &pk(&reader), "al", 1)
            .await
            .unwrap();
    assert_eq!(names(one), vec!["Alfred"]);
    // LIKE wildcards are literal.
    assert!(f
        .db
        .shared_channel_people(f.scope, &pk(&reader), "%", 50)
        .await
        .unwrap()
        .is_empty());
}
