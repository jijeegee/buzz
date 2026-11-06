//! `PUT /auth/agents/{pubkey}/profile`: an owner sets their external agent's
//! name, photo and description from any device, and stays its only writer.

use super::agent_keys::{
    agents_instance, call, call_proven_by, other_device, owner, pair_agent, register, Owner,
};
use super::*;
use crate::handlers::ingest::{ingest_event, IngestAuth};
use crate::identity::config::DeviceAdmissionMode;
use crate::identity::profile::publish_external_agent_profile;
use kuzz_core::principal::PrincipalId;
use kuzz_core::tenant::TenantContext;
use kuzz_core::CommunityId;

/// A paired agent with its owner bound here, as a pairing leaves it.
async fn connected(inst: &Instance, who: &Owner, agent: &nostr::Keys) {
    pair_agent(inst, who, agent).await;
    let bound = crate::api::relay_members::bind_external_owner(
        &inst.state,
        &tenant(inst).await,
        &agent.public_key(),
        &who.keys.public_key().to_bytes(),
    )
    .await;
    assert_eq!(bound, crate::api::relay_members::OwnerBind::Bound);
}

fn path(agent: &nostr::Keys) -> String {
    format!("/auth/agents/{}/profile", agent.public_key().to_hex())
}

async fn tenant(inst: &Instance) -> TenantContext {
    let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM communities WHERE host = $1")
        .bind(&inst.host)
        .fetch_one(inst.state.db.pool())
        .await
        .unwrap();
    TenantContext::resolved(CommunityId::from_uuid(id), inst.host.clone())
}

fn principal(agent: &nostr::Keys) -> PrincipalId {
    PrincipalId::from_slice(&agent.public_key().to_bytes()).unwrap()
}

/// The agent's newest kind:0: (content, sentinel signature, tags).
async fn head(inst: &Instance, agent: &nostr::Keys) -> Option<(Value, bool, Value)> {
    head_in(inst, &tenant(inst).await, agent).await
}

async fn head_in(
    inst: &Instance,
    tenant: &TenantContext,
    agent: &nostr::Keys,
) -> Option<(Value, bool, Value)> {
    let (lock, snapshot) = inst
        .state
        .db
        .lock_principal_profile_publish(tenant.community(), &principal(agent))
        .await
        .unwrap();
    let latest = snapshot.latest.clone();
    lock.release().await.unwrap();
    let latest = latest?;
    let stored = inst
        .state
        .db
        .get_event_by_id_for_event_write(tenant.community(), &latest.id)
        .await
        .unwrap()
        .unwrap();
    Some((
        serde_json::from_str(&latest.content).unwrap(),
        latest.server_stamped,
        serde_json::to_value(&stored.event.tags).unwrap(),
    ))
}

/// The agent's newest kind:0 id, hex.
async fn head_id(inst: &Instance, agent: &nostr::Keys) -> String {
    let tenant = tenant(inst).await;
    let (lock, snapshot) = inst
        .state
        .db
        .lock_principal_profile_publish(tenant.community(), &principal(agent))
        .await
        .unwrap();
    let latest = snapshot.latest.clone().unwrap();
    lock.release().await.unwrap();
    hex::encode(&latest.id)
}

async fn users_row(inst: &Instance, agent: &nostr::Keys) -> (Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT u.display_name, u.about FROM users u JOIN communities c ON c.id = u.community_id \
         WHERE c.host = $1 AND u.pubkey = $2",
    )
    .bind(&inst.host)
    .bind(agent.public_key().to_bytes().as_slice())
    .fetch_one(inst.state.db.pool())
    .await
    .unwrap()
}

/// The agent signs and submits its own kind:0, as `kuzz pair --name` does.
async fn agent_signs(
    inst: &Instance,
    agent: &nostr::Keys,
    name: &str,
    tags: Vec<nostr::Tag>,
) -> Result<(), String> {
    let content = json!({"display_name": name, "nip05": "x@example.com"}).to_string();
    let event = nostr::EventBuilder::new(nostr::Kind::Metadata, content)
        .tags(tags)
        .sign_with_keys(agent)
        .unwrap();
    let auth = IngestAuth::Nip42 {
        pubkey: agent.public_key(),
        scopes: vec![kuzz_auth::Scope::UsersWrite],
        channel_ids: None,
        conn_id: uuid::Uuid::new_v4(),
    };
    ingest_event(&inst.state, &tenant(inst).await, event, auth)
        .await
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

async fn save(inst: &Instance, who: &Owner, agent: &nostr::Keys, body: Value) -> (u16, Value) {
    call(inst, who, "PUT", &path(agent), Some(body)).await
}

fn named(name: &str) -> Value {
    json!({"display_name": name, "avatar_url": null, "about": null})
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn any_device_of_the_owner_sets_the_profile_and_the_agent_cannot_overwrite_it() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let laptop = other_device(&inst, &pc, &subject).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;

    // Never edited: the agent writes its own profile, as today.
    let proof = kuzz_sdk::nip_oa::compute_auth_tag(&pc.keys, &agent.public_key(), "").unwrap();
    let proof = serde_json::from_str::<Vec<String>>(&proof).unwrap();
    let auth_tag = nostr::Tag::parse(proof).unwrap();
    agent_signs(&inst, &agent, "Self named", vec![auth_tag])
        .await
        .unwrap();
    let (_, stamped, _) = head(&inst, &agent).await.unwrap();
    assert!(!stamped);

    // From another device, with the agent's computer off.
    let body = json!({
        "display_name": " Sales ",
        "avatar_url": "data:image/svg+xml;base64,AAAA",
        "about": "Sells socks",
    });
    let (status, answer) = save(&inst, &laptop, &agent, body).await;
    assert_eq!(status, 204, "{answer}");
    let (content, stamped, tags) = head(&inst, &agent).await.unwrap();
    assert!(stamped);
    assert_eq!(content["name"], "Sales");
    assert_eq!(content["display_name"], "Sales");
    assert_eq!(content["picture"], "data:image/svg+xml;base64,AAAA");
    assert_eq!(content["about"], "Sells socks");
    assert_eq!(content["nip05"], "x@example.com", "other fields stay");
    assert_eq!(tags[0][0], "auth", "tags stay");
    assert_eq!(
        users_row(&inst, &agent).await,
        (Some("Sales".to_owned()), Some("Sells socks".to_owned()))
    );

    // The agent is refused, and a reconnect keeps the edit.
    let refused = agent_signs(&inst, &agent, "Self named again", vec![])
        .await
        .unwrap_err();
    assert!(refused.contains("set by its owner"), "{refused}");
    assert_eq!(head(&inst, &agent).await.unwrap().0["name"], "Sales");

    // Blank clears the photo and the description.
    let (status, _) = save(&inst, &pc, &agent, named("Sales")).await;
    assert_eq!(status, 204);
    let (content, _, _) = head(&inst, &agent).await.unwrap();
    assert!(content.get("picture").is_none(), "{content}");
    assert!(content.get("about").is_none(), "{content}");
    assert_eq!(
        users_row(&inst, &agent).await.1.unwrap_or_default(),
        String::new()
    );

    // Saved unchanged: no new event.
    let tenant = tenant(&inst).await;
    assert!(
        !publish_external_agent_profile(&inst.state, &tenant, &principal(&agent))
            .await
            .unwrap()
    );
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn only_the_owner_of_a_live_external_key_may_save() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (mine, _) = owner(&inst).await;
    let (theirs, _) = owner(&inst).await;
    let paired = nostr::Keys::generate();
    connected(&inst, &mine, &paired).await;

    let not_found = |(status, _): (u16, Value)| status == 404;
    assert!(not_found(
        save(&inst, &theirs, &paired, named("Mine")).await
    ));
    let unknown = nostr::Keys::generate();
    assert!(not_found(save(&inst, &mine, &unknown, named("X")).await));
    let internal = nostr::Keys::generate();
    assert_eq!(register(&inst, &mine, &internal, false).await.0, 200);
    assert!(not_found(save(&inst, &mine, &internal, named("X")).await));

    for (body, field) in [
        (json!({"avatar_url": null}), "name"),
        (named(&"n".repeat(65)), "long name"),
        (named("   "), "blank name"),
        (
            json!({"display_name": "X", "about": "a".repeat(281)}),
            "long about",
        ),
        (
            json!({"display_name": "X", "avatar_url": "ftp://x/y"}),
            "url",
        ),
    ] {
        assert_eq!(save(&inst, &mine, &paired, body).await.0, 400, "{field}");
    }
    assert!(head(&inst, &paired).await.is_none(), "nothing was stamped");
    // A refused save leaves the agent its own writer.
    agent_signs(&inst, &paired, "Mine", vec![]).await.unwrap();
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn the_edit_survives_disconnect_and_same_key_reconnect() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;
    assert_eq!(save(&inst, &pc, &agent, named("Kept")).await.0, 204);

    let unbind = "UPDATE key_registry SET host_device_id = NULL WHERE pubkey = $1";
    sqlx::query(unbind)
        .bind(agent.public_key().to_bytes().as_slice())
        .execute(inst.state.db.pool())
        .await
        .unwrap();
    // Still the owner's to edit while disconnected, and still refused.
    assert_eq!(save(&inst, &pc, &agent, named("Kept 2")).await.0, 204);
    assert!(agent_signs(&inst, &agent, "Self", vec![]).await.is_err());
    assert_eq!(head(&inst, &agent).await.unwrap().0["name"], "Kept 2");

    // The same key connected to a computer again.
    sqlx::query("UPDATE key_registry SET host_device_id = $2 WHERE pubkey = $1")
        .bind(agent.public_key().to_bytes().as_slice())
        .bind(pc.device_id)
        .execute(inst.state.db.pool())
        .await
        .unwrap();
    assert!(agent_signs(&inst, &agent, "Self", vec![]).await.is_err());
    assert_eq!(head(&inst, &agent).await.unwrap().0["name"], "Kept 2");
}

/// The agent's event won the race past the ingest check: the owner's profile
/// goes back on top, without the publish waiting on its own lock.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_signed_head_with_equal_text_is_replaced_and_a_slipped_event_is_stamped_over() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;
    assert_eq!(save(&inst, &pc, &agent, named("Owner")).await.0, 204);

    // What the ingest check would have let through a moment earlier.
    let content = json!({"name": "Owner", "display_name": "Owner"}).to_string();
    let slipped = nostr::EventBuilder::new(nostr::Kind::Metadata, content)
        .custom_created_at(nostr::Timestamp::from(
            nostr::Timestamp::now().as_secs() + 5,
        ))
        .sign_with_keys(&agent)
        .unwrap();
    let tenant = tenant(&inst).await;
    inst.state
        .db
        .replace_addressable_event(tenant.community(), &slipped, None)
        .await
        .unwrap();
    assert!(!head(&inst, &agent).await.unwrap().1);

    let id = principal(&agent);
    let publish = publish_external_agent_profile(&inst.state, &tenant, &id);
    let stored = tokio::time::timeout(Duration::from_secs(10), publish)
        .await
        .expect("no self-deadlock")
        .unwrap();
    assert!(stored, "an equal-text signed head is replaced");
    assert!(head(&inst, &agent).await.unwrap().1);
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_banned_agent_is_left_alone() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;
    let tenant = tenant(&inst).await;
    sqlx::query(
        "INSERT INTO community_bans (community_id, pubkey, banned, actor_pubkey)          VALUES ($1, $2, true, $3)",
    )
    .bind(tenant.community().as_uuid())
    .bind(agent.public_key().to_bytes().as_slice())
    .bind(pc.keys.public_key().to_bytes().as_slice())
    .execute(inst.state.db.pool())
    .await
    .unwrap();
    assert_eq!(save(&inst, &pc, &agent, named("Banned")).await.0, 204);
    assert!(head(&inst, &agent).await.is_none());
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_device_proof_is_required() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;
    let body = Some(named("No proof"));
    let (status, _) = call_proven_by(&inst, &pc, None, "PUT", &path(&agent), body).await;
    assert_eq!(status, 401);
    assert!(head(&inst, &agent).await.is_none());
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_community_new_to_the_agent_gets_the_owner_set_profile() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;
    assert_eq!(save(&inst, &pc, &agent, named("Everywhere")).await.0, 204);

    let id = uuid::Uuid::new_v4();
    let host = format!("second-{}.test", id.simple());
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(id)
        .bind(&host)
        .execute(inst.state.db.pool())
        .await
        .unwrap();
    let second = TenantContext::resolved(CommunityId::from_uuid(id), host);
    // The owner naming a community the agent is not in brings it nowhere.
    let key = principal(&agent);
    assert!(!publish_external_agent_profile(&inst.state, &second, &key)
        .await
        .unwrap());
    assert!(head_in(&inst, &second, &agent).await.is_none());
    let bound = crate::api::relay_members::bind_external_owner(
        &inst.state,
        &second,
        &agent.public_key(),
        &pc.keys.public_key().to_bytes(),
    )
    .await;
    assert_eq!(bound, crate::api::relay_members::OwnerBind::Bound);

    for _ in 0..50 {
        if let Some((content, stamped, _)) = head_in(&inst, &second, &agent).await {
            assert!(stamped);
            assert_eq!(content["name"], "Everywhere");
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the owner-set profile never reached the new community");
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_photo_the_owner_uploaded_is_readable_by_other_members() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;
    let tenant = tenant(&inst).await;
    let hash = "ab".repeat(32);
    inst.state
        .db
        .record_media_upload(tenant.community(), &hash, &pc.keys.public_key().to_bytes())
        .await
        .unwrap();

    let url = format!("https://{}/media/{hash}.png", inst.host);
    let body = json!({"display_name": "Pictured", "avatar_url": url, "about": null});
    let (status, answer) = save(&inst, &pc, &agent, body).await;
    assert_eq!(status, 204, "{answer}");

    let member = nostr::Keys::generate().public_key().to_bytes();
    let readable = inst
        .state
        .db
        .media_hashes_readable_by(tenant.community(), &member, &[hash.clone()])
        .await
        .unwrap();
    assert_eq!(readable, vec![hash]);
}

/// The race itself, through ingest: the agent's kind:0 is past the check when
/// the owner saves, so it lands on top until the repair stamps over it.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn an_agent_event_that_raced_the_save_is_stamped_over() {
    use crate::handlers::ingest::owner_profile_test_hooks as hooks;
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;

    let (arrived, release) = hooks::arm(agent.public_key().to_bytes());
    let raced = agent_signs(&inst, &agent, "Raced", vec![]);
    let saved = async {
        arrived.await.unwrap();
        let status = save(&inst, &pc, &agent, named("Owner")).await.0;
        release.notify_one();
        status
    };
    let (raced, status) = tokio::join!(raced, saved);
    raced.unwrap();
    assert_eq!(status, 204);

    for _ in 0..50 {
        let (content, stamped, _) = head(&inst, &agent).await.unwrap();
        if stamped && content["name"] == "Owner" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the owner's profile did not come back on top");
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn the_agent_cannot_delete_the_owner_set_profile() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;
    agent_signs(&inst, &agent, "Self named", vec![])
        .await
        .unwrap();
    assert_eq!(save(&inst, &pc, &agent, named("Owner")).await.0, 204);
    let stamped_id = head_id(&inst, &agent).await;

    let by_id = nostr::Tag::parse(["e", stamped_id.as_str()]).unwrap();
    let address = format!("0:{}:", agent.public_key().to_hex());
    let by_address = nostr::Tag::parse(["a", address.as_str()]).unwrap();
    for tag in [by_id, by_address] {
        let event = nostr::EventBuilder::new(nostr::Kind::EventDeletion, "")
            .tags([tag])
            .sign_with_keys(&agent)
            .unwrap();
        let auth = IngestAuth::Nip42 {
            pubkey: agent.public_key(),
            scopes: vec![
                kuzz_auth::Scope::UsersWrite,
                kuzz_auth::Scope::MessagesWrite,
            ],
            channel_ids: None,
            conn_id: uuid::Uuid::new_v4(),
        };
        let refused = ingest_event(&inst.state, &tenant(&inst).await, event, auth)
            .await
            .map(|_| ())
            .unwrap_err();
        let refused = format!("{refused:?}");
        assert!(refused.contains("set by its owner"), "{refused}");
    }
    assert_eq!(head(&inst, &agent).await.unwrap().0["name"], "Owner");
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn an_agent_of_a_banned_owner_is_left_alone() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    connected(&inst, &pc, &agent).await;
    let tenant = tenant(&inst).await;
    let owner_key = pc.keys.public_key().to_bytes();
    sqlx::query(
        "INSERT INTO community_bans (community_id, pubkey, banned, actor_pubkey) \
         VALUES ($1, $2, true, $2)",
    )
    .bind(tenant.community().as_uuid())
    .bind(owner_key.as_slice())
    .execute(inst.state.db.pool())
    .await
    .unwrap();
    assert_eq!(save(&inst, &pc, &agent, named("Banned")).await.0, 204);
    assert!(head(&inst, &agent).await.is_none());
}
