//! Group chats (ownerless private streams) against a real database:
//! creation in one transaction, member roles and cap, creator bootstrap,
//! invitations that outlive their inviter, chats-list and invitation
//! previews, search exclusion, and 1:1-only DM opens.
use chrono::{Duration, Utc};
use kuzz_core::CommunityId;
use nostr::Keys;
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{
    self, MemberRole, GROUP_CHAT_FULL, GROUP_CHAT_MAX_INITIAL_PEOPLE, GROUP_CHAT_MAX_MEMBERS,
};
use crate::chat_list::{ChatListQuery, ChatRowKind};
use crate::error::DbError;
use crate::{channel_members, dm, Db};

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
}

fn pk(keys: &Keys) -> Vec<u8> {
    keys.public_key().to_bytes().to_vec()
}

fn denied(result: crate::error::Result<impl std::fmt::Debug>) -> String {
    match result {
        Err(DbError::AccessDenied(message)) => message,
        other => panic!("expected AccessDenied, got {other:?}"),
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
            .bind(format!("group-chat-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        Self {
            db: Db::from_pool(pool.clone()).with_chat_state_writes(true),
            pool,
            scope,
        }
    }

    /// A group chat by `creator` with `members` (people) and `invitees`.
    async fn chat(&self, creator: &Keys, members: &[&Keys], invitees: &[&Keys]) -> Uuid {
        let id = Uuid::new_v4();
        let members: Vec<(Vec<u8>, MemberRole)> = members
            .iter()
            .map(|k| (pk(k), MemberRole::Member))
            .collect();
        let invitees: Vec<Vec<u8>> = invitees.iter().map(|k| pk(k)).collect();
        let (record, created) = channel::create_group_chat_with_id(
            &self.pool,
            self.scope,
            id,
            "",
            &pk(creator),
            &members,
            &invitees,
        )
        .await
        .expect("create chat");
        assert!(created);
        assert_eq!(record.id, id);
        id
    }

    async fn roles(&self, chat: Uuid) -> Vec<(Vec<u8>, String)> {
        let mut roles: Vec<(Vec<u8>, String)> =
            channel_members::get_members(&self.pool, self.scope, chat)
                .await
                .expect("members")
                .into_iter()
                .map(|m| (m.pubkey, m.role))
                .collect();
        roles.sort();
        roles
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn creation_writes_room_members_and_invitations_in_one_go() {
    let f = Fixture::new().await;
    let (me, friend, agent, stranger) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    let id = Uuid::new_v4();
    let (record, created) = channel::create_group_chat_with_id(
        &f.pool,
        f.scope,
        id,
        "",
        &pk(&me),
        &[
            (pk(&friend), MemberRole::Member),
            (pk(&agent), MemberRole::Bot),
        ],
        &[pk(&stranger)],
    )
    .await
    .expect("create");
    assert!(created);
    assert!(record.group_chat);
    assert_eq!(record.name, "", "unnamed chats are allowed");
    assert_eq!(record.visibility, "private");
    assert_eq!(record.channel_type, "stream");
    assert_eq!(record.ttl_seconds, None);
    let history: String = sqlx::query_scalar(
        "SELECT history_policy FROM channels WHERE community_id = $1 AND id = $2",
    )
    .bind(f.scope.as_uuid())
    .bind(id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(history, "since_join");

    let mut expected = vec![
        (pk(&me), "member".to_owned()),
        (pk(&friend), "member".to_owned()),
        (pk(&agent), "bot".to_owned()),
    ];
    expected.sort();
    assert_eq!(f.roles(id).await, expected, "creator is a plain member");

    let invitations =
        f.db.pending_group_invitations(f.scope, &pk(&stranger))
            .await
            .unwrap();
    assert_eq!(invitations.len(), 1);
    assert_eq!(invitations[0].inviter, pk(&me));
    assert!(invitations[0].channel.group_chat);
    assert_eq!(invitations[0].channel.member_count, 3);
    assert_eq!(invitations[0].member_pubkeys.len(), 3);
    assert!(!invitations[0].member_pubkeys.contains(&pk(&stranger)));

    // A replayed creation writes nothing.
    let (_, again) =
        channel::create_group_chat_with_id(&f.pool, f.scope, id, "x", &pk(&stranger), &[], &[])
            .await
            .unwrap();
    assert!(!again);
    assert_eq!(f.roles(id).await.len(), 3);

    // Owner/admin roles and oversized starts are refused outright.
    assert!(channel::create_group_chat_with_id(
        &f.pool,
        f.scope,
        Uuid::new_v4(),
        "",
        &pk(&me),
        &[(pk(&friend), MemberRole::Admin)],
        &[],
    )
    .await
    .is_err());
    let too_many: Vec<Vec<u8>> = (0..=GROUP_CHAT_MAX_INITIAL_PEOPLE)
        .map(|_| pk(&Keys::generate()))
        .collect();
    assert!(channel::create_group_chat_with_id(
        &f.pool,
        f.scope,
        Uuid::new_v4(),
        "",
        &pk(&me),
        &[],
        &too_many,
    )
    .await
    .is_err());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn members_add_people_as_members_and_never_grant_roles() {
    let f = Fixture::new().await;
    let (me, friend, other) = (Keys::generate(), Keys::generate(), Keys::generate());
    let chat = f.chat(&me, &[&friend], &[]).await;
    for role in [MemberRole::Owner, MemberRole::Admin, MemberRole::Guest] {
        let message = denied(
            channel_members::add_member(&f.pool, f.scope, chat, &pk(&other), role, Some(&pk(&me)))
                .await,
        );
        assert!(message.contains("no owners"), "{role:?}: {message}");
    }
    channel_members::add_member(
        &f.pool,
        f.scope,
        chat,
        &pk(&other),
        MemberRole::Member,
        Some(&pk(&friend)),
    )
    .await
    .expect("any member may add people");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn cap_counts_active_members_under_the_lock() {
    let f = Fixture::new().await;
    let me = Keys::generate();
    let chat = f.chat(&me, &[], &[]).await;
    // Fill the chat to one below the cap directly.
    sqlx::query(
        "INSERT INTO channel_members (community_id, channel_id, pubkey, role, invited_by) \
         SELECT $1, $2, decode(lpad(to_hex(g), 64, '0'), 'hex'), 'member', $3 \
         FROM generate_series(1, $4) g",
    )
    .bind(f.scope.as_uuid())
    .bind(chat)
    .bind(pk(&me))
    .bind(GROUP_CHAT_MAX_MEMBERS - 2)
    .execute(&f.pool)
    .await
    .expect("seed members");

    let last = Keys::generate();
    channel_members::add_member(
        &f.pool,
        f.scope,
        chat,
        &pk(&last),
        MemberRole::Member,
        Some(&pk(&me)),
    )
    .await
    .expect("the 1,000th member fits");
    assert_eq!(
        channel_members::get_member_count(&f.pool, f.scope, chat)
            .await
            .unwrap(),
        GROUP_CHAT_MAX_MEMBERS
    );
    let message = denied(
        channel_members::add_member(
            &f.pool,
            f.scope,
            chat,
            &pk(&Keys::generate()),
            MemberRole::Member,
            Some(&pk(&me)),
        )
        .await,
    );
    assert_eq!(message, GROUP_CHAT_FULL);
    // Re-adding someone already in is not a new seat.
    channel_members::add_member(
        &f.pool,
        f.scope,
        chat,
        &pk(&last),
        MemberRole::Member,
        Some(&pk(&me)),
    )
    .await
    .expect("idempotent re-add at the cap");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn creator_bootstrap_only_before_anyone_joined() {
    let f = Fixture::new().await;
    let (me, friend) = (Keys::generate(), Keys::generate());
    let chat = f.chat(&me, &[&friend], &[]).await;
    channel_members::remove_member(&f.pool, f.scope, chat, &pk(&me), &pk(&me))
        .await
        .expect("leave");
    let message = denied(
        channel_members::add_member(
            &f.pool,
            f.scope,
            chat,
            &pk(&me),
            MemberRole::Member,
            Some(&pk(&me)),
        )
        .await,
    );
    assert!(message.contains("not an active member"), "{message}");

    // A room that never had a member still lets its creator in.
    let empty = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channels (id, community_id, name, channel_type, visibility, created_by) \
         VALUES ($1, $2, 'empty', 'stream', 'private', $3)",
    )
    .bind(empty)
    .bind(f.scope.as_uuid())
    .bind(pk(&me))
    .execute(&f.pool)
    .await
    .unwrap();
    channel_members::add_member(
        &f.pool,
        f.scope,
        empty,
        &pk(&me),
        MemberRole::Owner,
        Some(&pk(&me)),
    )
    .await
    .expect("first-member bootstrap");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn invitations_outlive_the_inviter_while_anyone_is_left() {
    let f = Fixture::new().await;
    let (me, friend, invitee, late) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    let chat = f.chat(&me, &[&friend], &[&invitee, &late]).await;
    channel_members::remove_member(&f.pool, f.scope, chat, &pk(&me), &pk(&me))
        .await
        .expect("inviter leaves");
    let record = channel_members::add_member_via_group_chat_invitation(
        &f.pool,
        f.scope,
        chat,
        &pk(&invitee),
        &pk(&me),
    )
    .await
    .expect("accepted after the inviter left");
    assert_eq!(record.role, "member");
    assert_eq!(record.invited_by.as_deref(), Some(pk(&me).as_slice()));

    for keys in [&friend, &invitee] {
        channel_members::remove_member(&f.pool, f.scope, chat, &pk(keys), &pk(keys))
            .await
            .expect("leave");
    }
    let message = denied(
        channel_members::add_member_via_group_chat_invitation(
            &f.pool,
            f.scope,
            chat,
            &pk(&late),
            &pk(&me),
        )
        .await,
    );
    assert!(message.contains("no members"), "{message}");

    // Managed groups keep the active-inviter rule.
    let group = channel::create_channel(
        &f.pool,
        f.scope,
        "group",
        channel::ChannelType::Stream,
        channel::ChannelVisibility::Private,
        None,
        &pk(&me),
        None,
    )
    .await
    .unwrap()
    .id;
    denied(
        channel_members::add_member_via_group_chat_invitation(
            &f.pool,
            f.scope,
            group,
            &pk(&late),
            &pk(&me),
        )
        .await,
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn chat_list_rows_carry_members_and_count() {
    let f = Fixture::new().await;
    let (me, a, b) = (Keys::generate(), Keys::generate(), Keys::generate());
    let chat = f.chat(&me, &[&a, &b], &[]).await;
    let group = channel::create_channel(
        &f.pool,
        f.scope,
        "group",
        channel::ChannelType::Stream,
        channel::ChannelVisibility::Private,
        None,
        &pk(&me),
        None,
    )
    .await
    .unwrap()
    .id;
    let rows =
        f.db.chat_list(
            f.scope,
            &me.public_key(),
            &ChatListQuery {
                limit: 50,
                before: None,
                pinned_threads: Vec::new(),
                quiet_before: Utc::now() - Duration::days(7),
            },
        )
        .await
        .unwrap()
        .rows;
    let chat_row = rows
        .iter()
        .find(|r| r.kind == ChatRowKind::Room && r.channel_id == chat)
        .expect("chat row");
    assert!(chat_row.group_chat);
    assert_eq!(chat_row.member_count, Some(3));
    let mut others = chat_row.dm_members.clone();
    others.sort();
    let mut expected = vec![pk(&a), pk(&b)];
    expected.sort();
    assert_eq!(others, expected, "other members, never the reader");

    let group_row = rows
        .iter()
        .find(|r| r.channel_id == group)
        .expect("group row");
    assert!(!group_row.group_chat);
    assert_eq!(group_row.member_count, None);
    assert!(group_row.dm_members.is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn search_and_addresses_never_return_chats() {
    let f = Fixture::new().await;
    let me = Keys::generate();
    let chat = f.chat(&me, &[], &[]).await;
    // Even a chat forced open with an address stays out of discovery.
    sqlx::query(
        "UPDATE channels SET name = 'zebra party', visibility = 'open', handle = $3 \
         WHERE community_id = $1 AND id = $2",
    )
    .bind(f.scope.as_uuid())
    .bind(chat)
    .bind(format!("zp{}", &chat.simple().to_string()[..8]))
    .execute(&f.pool)
    .await
    .unwrap();
    let found =
        f.db.search_open_groups(f.scope, &pk(&me), "zebra", 20)
            .await
            .unwrap();
    assert!(found.is_empty(), "{found:?}");
    let handle = format!("zp{}", &chat.simple().to_string()[..8]);
    assert!(f
        .db
        .open_group_by_handle(f.scope, &pk(&me), &handle)
        .await
        .unwrap()
        .is_none());
    let summary =
        f.db.group_summary(f.scope, &pk(&me), chat)
            .await
            .unwrap()
            .expect("direct lookup still works");
    assert!(summary.group_chat);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn dm_open_is_one_to_one_or_notes_to_self() {
    let f = Fixture::new().await;
    let (me, a, b) = (Keys::generate(), Keys::generate(), Keys::generate());
    let err = dm::open_dm(&f.pool, f.scope, &[&pk(&a), &pk(&b)], &pk(&me))
        .await
        .expect_err("three people are a group chat, not a DM");
    assert!(err.to_string().contains("1:1"), "{err}");

    let (one, created) = dm::open_dm(&f.pool, f.scope, &[&pk(&a)], &pk(&me))
        .await
        .expect("1:1");
    assert!(created);
    assert_eq!(f.roles(one.id).await.len(), 2);
    assert_eq!(one.name, "DM");

    let (notes, created) = dm::open_dm(&f.pool, f.scope, &[&pk(&me)], &pk(&me))
        .await
        .expect("notes to self");
    assert!(created);
    assert_eq!(notes.name, "DM", "notes to self is named like any DM");
    assert_eq!(
        f.roles(notes.id).await,
        vec![(pk(&me), "member".to_owned())]
    );
    let (again, created) = dm::open_dm(&f.pool, f.scope, &[], &pk(&me))
        .await
        .expect("same notes room");
    assert!(!created);
    assert_eq!(again.id, notes.id);
}
