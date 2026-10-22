//! Postgres-backed tests for ownerless group chats through the real ingest
//! path (kind:9007 creation, 9000/9001/9002/9008 rules, DM commands and
//! workflows). Run with `KUZZ_TEST_DATABASE_URL` pointing at a migrated,
//! throwaway database and `--include-ignored`.

use std::sync::Arc;

use kuzz_core::kind::{KIND_CONTACT_LIST, KIND_DM_ADD_MEMBER, KIND_DM_OPEN, KIND_WORKFLOW_DEF};
use kuzz_core::tenant::TenantContext;
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use uuid::Uuid;

use crate::handlers::ingest::{ingest_event, IngestAuth, IngestError, IngestResult};
use crate::state::AppState;

async fn state() -> Arc<AppState> {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("connect test Postgres");
    crate::state::tests::test_state_with_database_pool(pool).await
}

async fn tenant(state: &AppState) -> TenantContext {
    let host = format!("group-chat-{}.test", Uuid::new_v4().simple());
    let community = state
        .db
        .ensure_configured_community(&host)
        .await
        .expect("ensure community")
        .id;
    TenantContext::resolved(community, &host)
}

fn auth(keys: &Keys) -> IngestAuth {
    IngestAuth::Nip42 {
        pubkey: keys.public_key(),
        scopes: kuzz_auth::Scope::all_known(),
        channel_ids: None,
        conn_id: Uuid::new_v4(),
    }
}

fn sign(keys: &Keys, kind: u32, content: &str, tags: Vec<Vec<String>>) -> Event {
    let tags: Vec<Tag> = tags.into_iter().map(|t| Tag::parse(t).unwrap()).collect();
    EventBuilder::new(Kind::Custom(kind as u16), content)
        .tags(tags)
        .allow_self_tagging()
        .sign_with_keys(keys)
        .expect("sign")
}

fn tag(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

async fn send(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    keys: &Keys,
    event: Event,
) -> Result<IngestResult, IngestError> {
    ingest_event(state, tenant, event, auth(keys)).await
}

fn rejection(result: Result<IngestResult, IngestError>) -> String {
    match result {
        Err(IngestError::Rejected(m)) | Err(IngestError::AuthFailed(m)) => m,
        Err(other) => panic!("expected a rejection, got {other:?}"),
        Ok(r) => r.message,
    }
}

async fn ensure_users(state: &AppState, tenant: &TenantContext, keys: &[&Keys]) {
    for k in keys {
        state
            .db
            .ensure_user(tenant.community(), k.public_key().as_bytes())
            .await
            .expect("ensure user");
    }
}

/// `friend` lists `creator` as a contact, so the creator may add them.
async fn befriend(state: &Arc<AppState>, tenant: &TenantContext, friend: &Keys, creator: &Keys) {
    let contacts = sign(
        friend,
        KIND_CONTACT_LIST,
        "",
        vec![tag(&["p", &creator.public_key().to_hex()])],
    );
    let r = send(state, tenant, friend, contacts)
        .await
        .expect("store contact list");
    assert!(r.accepted, "{}", r.message);
}

async fn create_chat(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    creator: &Keys,
    people: &[&Keys],
) -> (Uuid, Result<IngestResult, IngestError>) {
    let id = Uuid::new_v4();
    let mut tags = vec![tag(&["h", &id.to_string()]), tag(&["group_chat"])];
    for p in people {
        tags.push(tag(&["p", &p.public_key().to_hex()]));
    }
    let event = sign(creator, 9007, "", tags);
    (id, send(state, tenant, creator, event).await)
}

async fn active_members(
    state: &AppState,
    tenant: &TenantContext,
    channel: Uuid,
) -> Vec<(Vec<u8>, String)> {
    state
        .db
        .get_members(tenant.community(), channel)
        .await
        .expect("members")
        .into_iter()
        .filter(|m| m.removed_at.is_none())
        .map(|m| (m.pubkey, m.role))
        .collect()
}

fn role_of(members: &[(Vec<u8>, String)], keys: &Keys) -> Option<String> {
    let pk = keys.public_key().to_bytes().to_vec();
    members
        .iter()
        .find(|(p, _)| *p == pk)
        .map(|(_, r)| r.clone())
}

/// Creation adds friends as members and the creator's agent as a bot in one
/// step, invites strangers, silently skips someone who blocked the creator,
/// and announces the marker on kind:39000.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn create_adds_friends_bots_invites_strangers_and_skips_blockers() {
    let state = state().await;
    let tenant = tenant(&state).await;
    let (creator, friend, agent, stranger, blocker) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    ensure_users(
        &state,
        &tenant,
        &[&creator, &friend, &agent, &stranger, &blocker],
    )
    .await;
    befriend(&state, &tenant, &friend, &creator).await;
    befriend(&state, &tenant, &blocker, &creator).await;
    assert!(state
        .db
        .set_agent_owner(
            tenant.community(),
            agent.public_key().as_bytes(),
            creator.public_key().as_bytes(),
        )
        .await
        .expect("set agent owner"));
    state
        .db
        .block_user(
            tenant.community(),
            blocker.public_key().as_bytes(),
            creator.public_key().as_bytes(),
        )
        .await
        .expect("block");

    let (id, result) = create_chat(
        &state,
        &tenant,
        &creator,
        &[&friend, &agent, &stranger, &blocker],
    )
    .await;
    let result = result.expect("create chat");
    assert!(result.accepted);
    assert_eq!(result.message, "invite-sent:1");

    let channel = state
        .db
        .get_channel(tenant.community(), id)
        .await
        .expect("channel");
    assert!(channel.group_chat);
    assert_eq!(channel.visibility, "private");

    let members = active_members(&state, &tenant, id).await;
    assert_eq!(role_of(&members, &creator).as_deref(), Some("member"));
    assert_eq!(role_of(&members, &friend).as_deref(), Some("member"));
    assert_eq!(role_of(&members, &agent).as_deref(), Some("bot"));
    assert_eq!(role_of(&members, &stranger), None);
    assert_eq!(role_of(&members, &blocker), None);
    assert_eq!(members.len(), 3);

    let invited = state
        .db
        .pending_group_invitations(tenant.community(), stranger.public_key().as_bytes())
        .await
        .expect("invitations");
    assert_eq!(invited.len(), 1);
    assert!(invited[0].channel.group_chat);
    assert!(!invited[0].member_pubkeys.is_empty());
    assert!(state
        .db
        .pending_group_invitations(tenant.community(), blocker.public_key().as_bytes())
        .await
        .expect("invitations")
        .is_empty());

    let marker: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE community_id = $1 AND kind = 39000 \
         AND tags @> $2::jsonb AND tags @> '[[\"group_chat\"]]'::jsonb)",
    )
    .bind(tenant.community().as_uuid())
    .bind(serde_json::json!([["d", id.to_string()]]))
    .fetch_one(state.db.pool())
    .await
    .expect("39000 lookup");
    assert!(marker, "kind:39000 must carry the group_chat marker");
}

/// Malformed creations are refused before anything is written.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn create_rejects_open_chats_and_managed_group_tags() {
    let state = state().await;
    let tenant = tenant(&state).await;
    let creator = Keys::generate();
    ensure_users(&state, &tenant, &[&creator]).await;
    for extra in [
        tag(&["channel_type", "forum"]),
        tag(&["visibility", "open"]),
        tag(&["about", "x"]),
        tag(&["handle", "crew"]),
    ] {
        let id = Uuid::new_v4();
        let event = sign(
            &creator,
            9007,
            "",
            vec![
                tag(&["h", &id.to_string()]),
                tag(&["group_chat"]),
                extra.clone(),
            ],
        );
        let msg = rejection(send(&state, &tenant, &creator, event).await);
        assert!(msg.starts_with("invalid"), "{extra:?}: {msg}");
        assert!(
            state.db.get_channel(tenant.community(), id).await.is_err(),
            "{extra:?} must not create a channel"
        );
    }
}

/// Membership edits in a group chat: no owner/admin grants, a person never
/// joins as a bot, members remove only themselves or their own agent, only
/// goals may change, and the chat cannot be deleted.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn ownerless_rules_for_put_remove_edit_and_delete() {
    let state = state().await;
    let tenant = tenant(&state).await;
    let (creator, friend, other, agent) = (
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
        Keys::generate(),
    );
    ensure_users(&state, &tenant, &[&creator, &friend, &other, &agent]).await;
    befriend(&state, &tenant, &friend, &creator).await;
    befriend(&state, &tenant, &other, &creator).await;
    assert!(state
        .db
        .set_agent_owner(
            tenant.community(),
            agent.public_key().as_bytes(),
            friend.public_key().as_bytes(),
        )
        .await
        .expect("set agent owner"));
    let (id, result) = create_chat(&state, &tenant, &creator, &[&friend]).await;
    assert!(result.expect("create").accepted);
    let h = id.to_string();

    // 9000: owner/admin/guest grants refused; a person cannot be a bot.
    for role in ["owner", "admin", "guest"] {
        let put = sign(
            &creator,
            9000,
            "",
            vec![
                tag(&["h", &h]),
                tag(&["p", &other.public_key().to_hex()]),
                tag(&["role", role]),
            ],
        );
        let msg = rejection(send(&state, &tenant, &creator, put).await);
        assert!(!msg.is_empty(), "role {role} must be refused");
    }
    let as_bot = sign(
        &creator,
        9000,
        "",
        vec![
            tag(&["h", &h]),
            tag(&["p", &other.public_key().to_hex()]),
            tag(&["role", "bot"]),
        ],
    );
    assert!(send(&state, &tenant, &creator, as_bot).await.is_err());
    assert_eq!(
        role_of(&active_members(&state, &tenant, id).await, &other),
        None
    );

    // A plain add of a friend works and lands as member; the friend adds
    // their own agent, which lands as bot.
    let put = sign(
        &creator,
        9000,
        "",
        vec![tag(&["h", &h]), tag(&["p", &other.public_key().to_hex()])],
    );
    assert!(
        send(&state, &tenant, &creator, put)
            .await
            .expect("add")
            .accepted
    );
    let put_agent = sign(
        &friend,
        9000,
        "",
        vec![tag(&["h", &h]), tag(&["p", &agent.public_key().to_hex()])],
    );
    assert!(
        send(&state, &tenant, &friend, put_agent)
            .await
            .expect("add agent")
            .accepted
    );
    let members = active_members(&state, &tenant, id).await;
    assert_eq!(role_of(&members, &other).as_deref(), Some("member"));
    assert_eq!(role_of(&members, &agent).as_deref(), Some("bot"));

    // 9001: nobody removes another person; the creator cannot remove the
    // friend's agent, the friend can.
    let kick = sign(
        &creator,
        9001,
        "",
        vec![tag(&["h", &h]), tag(&["p", &other.public_key().to_hex()])],
    );
    assert!(send(&state, &tenant, &creator, kick).await.is_err());
    let kick_agent = sign(
        &creator,
        9001,
        "",
        vec![tag(&["h", &h]), tag(&["p", &agent.public_key().to_hex()])],
    );
    assert!(send(&state, &tenant, &creator, kick_agent).await.is_err());
    let own_agent = sign(
        &friend,
        9001,
        "",
        vec![tag(&["h", &h]), tag(&["p", &agent.public_key().to_hex()])],
    );
    assert!(
        send(&state, &tenant, &friend, own_agent)
            .await
            .expect("remove own agent")
            .accepted
    );

    // 9002: only goals.
    let rename = sign(
        &creator,
        9002,
        "",
        vec![tag(&["h", &h]), tag(&["name", "x"])],
    );
    assert!(send(&state, &tenant, &creator, rename).await.is_err());
    let goals = sign(
        &creator,
        9002,
        "",
        vec![tag(&["h", &h]), tag(&["goals", "off"])],
    );
    assert!(
        send(&state, &tenant, &creator, goals)
            .await
            .expect("goals")
            .accepted
    );

    // 9008: never.
    let delete = sign(&creator, 9008, "", vec![tag(&["h", &h])]);
    let msg = rejection(send(&state, &tenant, &creator, delete).await);
    assert!(msg.contains("cannot be deleted"), "{msg}");

    // Self-leave works.
    let leave = sign(
        &other,
        9001,
        "",
        vec![tag(&["h", &h]), tag(&["p", &other.public_key().to_hex()])],
    );
    assert!(
        send(&state, &tenant, &other, leave)
            .await
            .expect("leave")
            .accepted
    );
    assert_eq!(
        role_of(&active_members(&state, &tenant, id).await, &other),
        None
    );
}

/// A pending invitation still lets its invitee in after the inviter left.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn invitation_is_accepted_after_the_inviter_left() {
    let state = state().await;
    let tenant = tenant(&state).await;
    let (creator, friend, stranger) = (Keys::generate(), Keys::generate(), Keys::generate());
    ensure_users(&state, &tenant, &[&creator, &friend, &stranger]).await;
    befriend(&state, &tenant, &friend, &creator).await;
    let (id, result) = create_chat(&state, &tenant, &creator, &[&friend, &stranger]).await;
    assert_eq!(result.expect("create").message, "invite-sent:1");
    let leave = sign(
        &creator,
        9001,
        "",
        vec![
            tag(&["h", &id.to_string()]),
            tag(&["p", &creator.public_key().to_hex()]),
        ],
    );
    assert!(
        send(&state, &tenant, &creator, leave)
            .await
            .expect("leave")
            .accepted
    );

    crate::handlers::side_effects::add_group_chat_member_by_invitation(
        &tenant,
        &state,
        id,
        stranger.public_key().as_bytes(),
        creator.public_key().as_bytes(),
    )
    .await
    .expect("accept after inviter left");
    assert_eq!(
        role_of(&active_members(&state, &tenant, id).await, &stranger).as_deref(),
        Some("member")
    );
}

/// DMs are 1:1: no 9000 add, 41010 takes one other person (or only self),
/// 41011 is gone.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn dms_stay_one_to_one() {
    let state = state().await;
    let tenant = tenant(&state).await;
    let (a, b, c) = (Keys::generate(), Keys::generate(), Keys::generate());
    ensure_users(&state, &tenant, &[&a, &b, &c]).await;

    let three = sign(
        &a,
        KIND_DM_OPEN,
        "",
        vec![
            tag(&["p", &b.public_key().to_hex()]),
            tag(&["p", &c.public_key().to_hex()]),
        ],
    );
    let msg = rejection(send(&state, &tenant, &a, three).await);
    assert!(msg.contains("1:1"), "{msg}");

    let to_self = sign(
        &a,
        KIND_DM_OPEN,
        "",
        vec![tag(&["p", &a.public_key().to_hex()])],
    );
    assert!(
        send(&state, &tenant, &a, to_self)
            .await
            .expect("notes to self")
            .accepted
    );

    let pair = sign(
        &a,
        KIND_DM_OPEN,
        "",
        vec![tag(&["p", &b.public_key().to_hex()])],
    );
    let opened = send(&state, &tenant, &a, pair).await.expect("open dm");
    assert!(opened.accepted, "{}", opened.message);
    let dm = state
        .db
        .open_dm(
            tenant.community(),
            &[a.public_key().as_bytes(), b.public_key().as_bytes()],
            a.public_key().as_bytes(),
        )
        .await
        .expect("dm lookup")
        .0
        .id;

    let add = sign(
        &a,
        9000,
        "",
        vec![
            tag(&["h", &dm.to_string()]),
            tag(&["p", &c.public_key().to_hex()]),
        ],
    );
    let msg = rejection(send(&state, &tenant, &a, add).await);
    assert!(msg.contains("1:1"), "{msg}");

    let grow = sign(
        &a,
        KIND_DM_ADD_MEMBER,
        "",
        vec![
            tag(&["h", &dm.to_string()]),
            tag(&["p", &c.public_key().to_hex()]),
        ],
    );
    assert!(send(&state, &tenant, &a, grow).await.is_err());
}

/// Workflows cannot be created in a group chat.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn workflows_are_rejected_in_group_chats() {
    let state = state().await;
    let tenant = tenant(&state).await;
    let creator = Keys::generate();
    ensure_users(&state, &tenant, &[&creator]).await;
    let (id, result) = create_chat(&state, &tenant, &creator, &[]).await;
    assert!(result.expect("create").accepted);
    let workflow = sign(
        &creator,
        KIND_WORKFLOW_DEF,
        "name: x\ntrigger:\n  on: message_posted\nsteps: []\n",
        vec![
            tag(&["h", &id.to_string()]),
            tag(&["d", &Uuid::new_v4().to_string()]),
        ],
    );
    let msg = rejection(send(&state, &tenant, &creator, workflow).await);
    assert!(msg.contains("chats cannot have workflows"), "{msg}");
}
