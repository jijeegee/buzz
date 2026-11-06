//! The owner tools switch, agent revoke and `/agents/me` (AI tools plan,
//! stage S1), over the production router.

use super::agent_keys::{
    agents_instance, call, call_proven_by, device_key, other_device, owner, pair_agent, register,
    status_call, Owner,
};
use super::key_sockets::{auth_answer, key_socket};
use super::*;
use crate::identity::config::DeviceAdmissionMode;

fn tool_level_path(agent: &nostr::Keys) -> String {
    format!("/auth/agents/{}/tool-level", agent.public_key().to_hex())
}

async fn set_level(
    inst: &Instance,
    who: &Owner,
    agent: &nostr::Keys,
    level: &str,
    class: &str,
    default: bool,
) -> (u16, Value) {
    let body = json!({"level": level, "respond_class": class, "default": default});
    call(inst, who, "POST", &tool_level_path(agent), Some(body)).await
}

fn code((status, body): (u16, Value)) -> (u16, String) {
    (status, body["code"].as_str().unwrap_or_default().to_owned())
}

async fn levels(inst: &Instance, who: &Owner, pubkeys: &[String]) -> Value {
    let (status, body) = call(
        inst,
        who,
        "POST",
        "/auth/agents/tool-levels",
        Some(json!({"pubkeys": pubkeys})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    body["agents"].clone()
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn the_owner_sets_the_switch_from_a_proven_device() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let laptop = other_device(&inst, &pc, &subject).await;
    let agent = nostr::Keys::generate();
    assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);
    let path = tool_level_path(&agent);
    let full = json!({"level": "full", "respond_class": "owner-only"});

    // A copied token alone, or with another device's proof, sets nothing.
    for signer in [None, Some(&device_key())] {
        assert_eq!(
            code(call_proven_by(&inst, &pc, signer, "POST", &path, Some(full.clone())).await),
            (401, "device_proof_required".to_owned())
        );
    }
    let agent_hex = agent.public_key().to_hex();
    assert_eq!(
        levels(&inst, &pc, std::slice::from_ref(&agent_hex)).await[0]["level"],
        Value::Null
    );

    // The host turns it on; a default never overwrites the owner's choice.
    assert_eq!(
        set_level(&inst, &pc, &agent, "full", "owner-only", false).await,
        (200, json!({"result": "set", "level": "full"}))
    );
    assert_eq!(
        set_level(&inst, &pc, &agent, "limited", "owner-only", true).await,
        (200, json!({"result": "already_set", "level": "full"}))
    );
    assert_eq!(
        code(set_level(&inst, &pc, &agent, "full", "anyone", false).await),
        (409, "full_anyone_refused".to_owned())
    );
    // Another device may turn it on only with the class the relay knows.
    assert_eq!(
        code(set_level(&inst, &laptop, &agent, "full", "allowlist", false).await),
        (409, "respond_class_stale".to_owned())
    );
    assert_eq!(
        set_level(&inst, &laptop, &agent, "full", "owner-only", false)
            .await
            .0,
        200
    );
    // Not the caller's agent, or no agent at all.
    let (stranger, _) = owner(&inst).await;
    let strangers_agent = nostr::Keys::generate();
    assert_eq!(
        register(&inst, &stranger, &strangers_agent, false).await.0,
        200
    );
    for key in [&strangers_agent, &nostr::Keys::generate()] {
        assert_eq!(
            code(set_level(&inst, &pc, key, "limited", "owner-only", false).await),
            (404, "agent_not_found".to_owned())
        );
    }

    // The batch read leaves unknown keys out and says which device runs it.
    let unknown = nostr::Keys::generate().public_key().to_hex();
    let asked = [
        agent_hex.clone(),
        unknown,
        strangers_agent.public_key().to_hex(),
    ];
    let entry = |here: bool| json!([{"pubkey": agent_hex, "level": "full", "respond_class": "owner-only", "here": here}]);
    assert_eq!(levels(&inst, &pc, &asked).await, entry(true));
    assert_eq!(levels(&inst, &laptop, &asked).await, entry(false));

    // Each change the owner made is in the action log.
    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM agent_actions \
          WHERE agent_pubkey = $1 AND owner_pubkey = $2 AND action = 'tool_level.set' \
            AND status = 'ok' AND source = 'relay'",
    )
    .bind(agent.public_key().to_bytes().as_slice())
    .bind(pc.keys.public_key().to_bytes().as_slice())
    .fetch_one(inst.state.db.pool())
    .await
    .unwrap();
    assert_eq!(logged, 2);
}

/// `GET /agents/me` as `keys` from `device`. Each call gets its own query
/// so two calls in one second are not one NIP-98 event replayed.
async fn me(inst: &Instance, keys: &nostr::Keys, device: &p256::ecdsa::SigningKey) -> Value {
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (status, body) = status_call(inst, keys, device, &format!("/agents/me?n={n}")).await;
    assert_eq!(status, 200, "{body}");
    body
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn agents_me_says_whether_the_key_acts_for_its_owner() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let laptop = other_device(&inst, &pc, &subject).await;
    let agent = nostr::Keys::generate();
    assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);
    let owner_hex = pc.keys.public_key().to_hex();

    let body = me(&inst, &agent, &pc.device).await;
    assert_eq!(
        (&body["role"], &body["owner"], &body["stored_level"]),
        (&json!("agent"), &json!(owner_hex), &Value::Null)
    );
    assert_eq!(
        (&body["effective"], &body["reason"]),
        (&json!("limited"), &json!("not_set"))
    );
    assert_eq!(body["community"], inst.host);

    assert_eq!(
        set_level(&inst, &pc, &agent, "full", "owner-only", false)
            .await
            .0,
        200
    );
    let body = me(&inst, &agent, &pc.device).await;
    assert_eq!(
        (&body["effective"], &body["reason"], &body["stored_level"]),
        (&json!("full"), &Value::Null, &json!("full"))
    );
    assert_eq!(body["respond_class"], "owner-only");
    assert!(body["host_computer"].is_string(), "{body}");

    // From another device the key is refused, and still names its owner.
    let body = me(&inst, &agent, &laptop.device).await;
    assert_eq!(
        (&body["effective"], &body["reason"], &body["owner"]),
        (&json!("limited"), &json!("wrong_host"), &json!(owner_hex))
    );
    // A person's own key, and a key nobody registered.
    let body = me(&inst, &pc.keys, &pc.device).await;
    assert_eq!(
        (&body["role"], &body["reason"]),
        (&json!("person"), &json!("not_an_agent"))
    );
    assert_eq!(body["host_computer"], Value::Null);
    let body = me(&inst, &nostr::Keys::generate(), &pc.device).await;
    assert_eq!(
        (&body["role"], &body["reason"]),
        (&Value::Null, &json!("unregistered"))
    );
    // The owner turns it off: it counts at once.
    assert_eq!(
        set_level(&inst, &pc, &agent, "limited", "owner-only", false)
            .await
            .0,
        200
    );
    let body = me(&inst, &agent, &pc.device).await;
    assert_eq!(
        (&body["effective"], &body["reason"]),
        (&json!("limited"), &json!("limited"))
    );

    // Under `off` devices are not read: never full.
    let off = agents_instance(DeviceAdmissionMode::Off).await;
    let (status, body) = status_call(&off, &agent, &pc.device, "/agents/me").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        (&body["effective"], &body["reason"]),
        (&json!("limited"), &json!("admission_off"))
    );
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn revoke_is_for_the_host_closes_sockets_and_is_for_good() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let laptop = other_device(&inst, &pc, &subject).await;
    let agent = nostr::Keys::generate();
    assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);
    assert_eq!(
        set_level(&inst, &pc, &agent, "full", "owner-only", false)
            .await
            .0,
        200
    );
    let mut socket = key_socket(&inst, &agent, &pc.device).await;
    let path = format!("/auth/agents/{}/revoke", agent.public_key().to_hex());

    assert_eq!(
        code(call_proven_by(&inst, &pc, None, "POST", &path, None).await),
        (401, "device_proof_required".to_owned())
    );
    assert_eq!(
        code(call(&inst, &laptop, "POST", &path, None).await),
        (409, "not_host".to_owned())
    );
    assert_eq!(call(&inst, &pc, "POST", &path, None).await.0, 204);
    wait_closed(&mut socket, Duration::from_secs(5)).await;
    let (accepted, reason) = auth_answer(&inst, &agent, &pc.device).await;
    assert!(!accepted, "{reason}");
    // A repeat answers the same; the key never comes back.
    assert_eq!(call(&inst, &pc, "POST", &path, None).await.0, 204);
    assert_eq!(
        code(register(&inst, &pc, &agent, false).await),
        (409, "agent_revoked".to_owned())
    );
    assert_eq!(
        code(set_level(&inst, &pc, &agent, "limited", "owner-only", false).await),
        (404, "agent_not_found".to_owned())
    );
    assert_eq!(
        levels(&inst, &pc, &[agent.public_key().to_hex()]).await,
        json!([])
    );
    // A paired agent is removed from its own computer only.
    let paired = nostr::Keys::generate();
    pair_agent(&inst, &pc, &paired).await;
    let paired_path = format!("/auth/agents/{}/revoke", paired.public_key().to_hex());
    assert_eq!(
        code(call(&inst, &laptop, "POST", &paired_path, None).await),
        (409, "not_host".to_owned())
    );
    let unknown = format!(
        "/auth/agents/{}/revoke",
        nostr::Keys::generate().public_key().to_hex()
    );
    assert_eq!(
        code(call(&inst, &pc, "POST", &unknown, None).await),
        (404, "agent_not_found".to_owned())
    );
}

/// The agent's (level, respond_class) as the owner reads it.
async fn stored(inst: &Instance, who: &Owner, agent: &nostr::Keys) -> (Value, Value) {
    let found = levels(inst, who, &[agent.public_key().to_hex()]).await;
    (found[0]["level"].clone(), found[0]["respond_class"].clone())
}

/// A kind:30177 managed-agent record for `agent` signed by `keys`.
fn managed_agent(keys: &nostr::Keys, agent: &nostr::Keys, respond_to: &str, at: u64) -> Value {
    let event = nostr::EventBuilder::new(
        nostr::Kind::Custom(kuzz_core::kind::KIND_MANAGED_AGENT as u16),
        json!({"respond_to": respond_to}).to_string(),
    )
    .tags([nostr::Tag::parse(["d", &agent.public_key().to_hex()]).unwrap()])
    .custom_created_at(nostr::Timestamp::from(at))
    .sign_with_keys(keys)
    .unwrap();
    serde_json::to_value(&event).unwrap()
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn the_owners_record_keeps_the_class_and_anyone_turns_full_off() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);
    assert_eq!(
        set_level(&inst, &pc, &agent, "full", "owner-only", false)
            .await
            .0,
        200
    );
    let now = nostr::Timestamp::now().as_secs();
    let mut ws = key_socket(&inst, &pc.keys, &pc.device).await;
    let publish = |event: Value| json!(["EVENT", event]);

    // Someone else's record for the agent changes nothing.
    let (stranger, _) = owner(&inst).await;
    let mut other = key_socket(&inst, &stranger.keys, &stranger.device).await;
    send(
        &mut other,
        publish(managed_agent(&stranger.keys, &agent, "anyone", now)),
    )
    .await;
    next_ok(&mut other).await;
    assert_eq!(
        stored(&inst, &pc, &agent).await,
        (json!("full"), json!("owner-only"))
    );

    // The owner's switch to anyone turns the tools off.
    send(
        &mut ws,
        publish(managed_agent(&pc.keys, &agent, "anyone", now)),
    )
    .await;
    let ok = next_ok(&mut ws).await;
    assert_eq!(ok[2], true, "{ok}");
    assert_eq!(
        stored(&inst, &pc, &agent).await,
        (json!("limited"), json!("anyone"))
    );
    // An older record replayed changes nothing; a newer one moves the class.
    send(
        &mut ws,
        publish(managed_agent(&pc.keys, &agent, "allowlist", now - 10)),
    )
    .await;
    next_ok(&mut ws).await;
    assert_eq!(
        stored(&inst, &pc, &agent).await,
        (json!("limited"), json!("anyone"))
    );
    send(
        &mut ws,
        publish(managed_agent(&pc.keys, &agent, "allowlist", now + 1)),
    )
    .await;
    let ok = next_ok(&mut ws).await;
    assert_eq!(ok[2], true, "{ok}");
    assert_eq!(
        stored(&inst, &pc, &agent).await,
        (json!("limited"), json!("allowlist"))
    );
}

/// A paired agent's class comes only from the owner's record: after the
/// desktop records `anyone`, a stale client asking for `full` with
/// `owner-only` is refused; limited still works.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_paired_agent_is_never_turned_on_with_a_stale_class() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let laptop = other_device(&inst, &pc, &subject).await;
    let paired = nostr::Keys::generate();
    pair_agent(&inst, &pc, &paired).await;
    let now = nostr::Timestamp::now().as_secs();

    // No record yet: the class is unknown, so full is refused everywhere.
    for who in [&pc, &laptop] {
        assert_eq!(
            code(set_level(&inst, who, &paired, "full", "owner-only", false).await),
            (409, "respond_class_stale".to_owned())
        );
    }
    let mut ws = key_socket(&inst, &pc.keys, &pc.device).await;
    send(
        &mut ws,
        json!([
            "EVENT",
            managed_agent(&pc.keys, &paired, "owner-only", now - 10)
        ]),
    )
    .await;
    let ok = next_ok(&mut ws).await;
    assert_eq!(ok[2], true, "{ok}");
    assert_eq!(
        set_level(&inst, &laptop, &paired, "full", "owner-only", false)
            .await
            .0,
        200
    );

    // The desktop switches it to anyone: the tools go off, and a stale
    // client cannot turn them back on.
    send(
        &mut ws,
        json!(["EVENT", managed_agent(&pc.keys, &paired, "anyone", now)]),
    )
    .await;
    let ok = next_ok(&mut ws).await;
    assert_eq!(ok[2], true, "{ok}");
    assert_eq!(
        stored(&inst, &pc, &paired).await,
        (json!("limited"), json!("anyone"))
    );
    assert_eq!(
        code(set_level(&inst, &pc, &paired, "full", "owner-only", false).await),
        (409, "respond_class_stale".to_owned())
    );
    assert_eq!(
        set_level(&inst, &pc, &paired, "limited", "owner-only", false).await,
        (200, json!({"result": "set", "level": "limited"}))
    );
    assert_eq!(
        stored(&inst, &pc, &paired).await,
        (json!("limited"), json!("anyone"))
    );
}
