//! Kuzz agent registration (device-bound keys plan §4, PR A3) over the
//! production router: key-backup logins with device keys, header proofs,
//! registration, key copies, WebSocket admission and the status endpoint.

use base64::Engine as _;
use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use sha2::{Digest as _, Sha256};

use super::*;
use crate::identity::admission::test_support::auth_event;
use crate::identity::config::DeviceAdmissionMode;
use crate::identity::device::PROOF_HEADER;
use kuzz_auth::device_proof::{event_proof_message, DevicePubkey, DEVICE_TAG};

pub(super) async fn agents_instance(mode: DeviceAdmissionMode) -> Instance {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    instance_configured(&host, true, None, |config| {
        config.auth_token.device_admission = mode;
        config.auth_token.key_backup_master = Some(
            kuzz_auth::key_backup::BackupMasterKey::from_hex("live-test", &"42".repeat(32))
                .expect("master key"),
        );
    })
    .await
}

pub(super) fn device_key() -> SigningKey {
    loop {
        if let Ok(key) = SigningKey::from_slice(&rand::random::<[u8; 32]>()) {
            return key;
        }
    }
}

pub(super) fn device_hex(key: &SigningKey) -> String {
    hex::encode(key.verifying_key().to_encoded_point(true).as_bytes())
}

fn proof(
    key: &SigningKey,
    purpose: &str,
    binding: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
) -> String {
    let ts = chrono::Utc::now().timestamp();
    let nonce = hex::encode(rand::random::<[u8; 12]>());
    let token_hash = token.map(kuzz_auth::hash_token);
    let body_hash: Option<[u8; 32]> = body.map(|body| Sha256::digest(body).into());
    let scope = kuzz_auth::device_proof::ProofScope {
        purpose,
        binding,
        token_hash: token_hash.as_ref(),
        body_hash: body_hash.as_ref(),
    };
    let message = kuzz_auth::device_proof::proof_message(&scope, ts, &nonce);
    let signature: Signature = key.sign(message.as_bytes());
    format!("{ts}.{nonce}.{}", hex::encode(signature.to_bytes()))
}

/// A key-backup account signed in on one device.
pub(super) struct Owner {
    pub(super) access: String,
    pub(super) device: SigningKey,
    pub(super) device_id: uuid::Uuid,
    pub(super) keys: nostr::Keys,
}

/// Keyed key-backup sign-in of `subject` from `device`.
pub(super) async fn sign_in(
    inst: &Instance,
    subject: &str,
    device: SigningKey,
) -> (String, String, uuid::Uuid, SigningKey) {
    reset_login_limits(inst).await;
    let verifier = verifier();
    let state = verifier[..24].to_owned();
    let challenge = kuzz_auth::token::pkce_s256_challenge(&verifier);
    let key_hex = device_hex(&device);
    let start = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state.as_str()),
                ("code_challenge", challenge.as_str()),
                ("client", "desktop"),
                ("identity_mode", "key_backup"),
                ("redirect_uri", REDIRECT_URI),
                ("device_pubkey", key_hex.as_str()),
            ],
        )
        .send()
        .await
        .expect("start");
    assert_eq!(start.status(), 302);
    let callback = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("code", subject), ("state", state.as_str())],
        )
        .send()
        .await
        .expect("callback");
    let location = callback.headers()["location"].to_str().unwrap().to_owned();
    let login_code = query_param(&location, "code").expect("login code");
    let binding = format!("login:{}", hex::encode(kuzz_auth::hash_token(&login_code)));
    let response = inst
        .post("/auth/oidc/complete")
        .json(&json!({
            "login_code": login_code, "code_verifier": verifier,
            "device_pubkey": key_hex, "device_key_kind": "hardware",
            "device_proof": proof(&device, "bind", &binding, None, None),
        }))
        .send()
        .await
        .expect("complete");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json");
    (
        body["principal_id"].as_str().unwrap().to_owned(),
        body["access"].as_str().unwrap().to_owned(),
        body["device_id"].as_str().unwrap().parse().expect("uuid"),
        device,
    )
}

/// A new account with its own key, signed in on a new device.
pub(super) async fn owner(inst: &Instance) -> (Owner, String) {
    let subject = uuid::Uuid::new_v4().to_string();
    let (principal, access, device_id, device) = sign_in(inst, &subject, device_key()).await;
    let keys = nostr::Keys::generate();
    sqlx::query(
        "INSERT INTO account_key_backups (account_id, pubkey, version, key_id, nonce, ciphertext) \
         VALUES ($1, $2, 2, 'k1', $3, $4)",
    )
    .bind(hex::decode(&principal).unwrap())
    .bind(keys.public_key().to_bytes().as_slice())
    .bind([0u8; 12].as_slice())
    .bind([0u8; 48].as_slice())
    .execute(inst.state.db.pool())
    .await
    .expect("account key");
    (
        Owner {
            access,
            device,
            device_id,
            keys,
        },
        subject,
    )
}

/// The same account signed in on another device.
pub(super) async fn other_device(inst: &Instance, owner: &Owner, subject: &str) -> Owner {
    let (_, access, device_id, device) = sign_in(inst, subject, device_key()).await;
    Owner {
        access,
        device,
        device_id,
        keys: owner.keys.clone(),
    }
}

/// An `/auth/*` call with Bearer and header proof.
pub(super) async fn call(
    inst: &Instance,
    who: &Owner,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_proven_by(inst, who, Some(&who.device), method, path, body).await
}

/// An `/auth/*` call with `who`'s Bearer and a header proof by `signer`
/// (none: no proof header).
pub(super) async fn call_proven_by(
    inst: &Instance,
    who: &Owner,
    signer: Option<&SigningKey>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let bytes = body.map(|body| serde_json::to_vec(&body).unwrap());
    let binding = format!("{method} {}{path}", inst.state.identity.config().public_url);
    let mut request = match method {
        "GET" => inst.get(path),
        "DELETE" => inst.delete(path),
        _ => inst.post(path),
    }
    .bearer_auth(&who.access);
    if let Some(signer) = signer {
        let proof = proof(
            signer,
            "auth",
            &binding,
            Some(&who.access),
            bytes.as_deref(),
        );
        request = request.header(PROOF_HEADER, proof);
    }
    if let Some(bytes) = bytes {
        request = request
            .header("content-type", "application/json")
            .body(bytes);
    }
    let response = request.send().await.expect("send");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn challenge(inst: &Instance, who: &Owner) -> String {
    let (status, body) = call(inst, who, "POST", "/auth/agents/challenge", None).await;
    assert_eq!(status, 200, "{body}");
    body["challenge"].as_str().unwrap().to_owned()
}

fn agent_signature(
    agent: &nostr::Keys,
    challenge: &str,
    owner: &nostr::Keys,
    device: &SigningKey,
) -> String {
    use nostr::hashes::{sha256::Hash as Sha256Hash, Hash as _};
    let message = kuzz_auth::key_backup::agent_register_message(
        challenge,
        &owner.public_key().to_hex(),
        &device_hex(device),
    );
    let digest = Sha256Hash::hash(message.as_bytes());
    agent
        .sign_schnorr(&nostr::secp256k1::Message::from_digest(
            digest.to_byte_array(),
        ))
        .to_string()
}

/// A well-formed registration body for `agent` from `who`.
fn register_body(agent: &nostr::Keys, who: &Owner, challenge: &str) -> Value {
    json!({
        "agent_pubkey": agent.public_key().to_hex(),
        "secret_key": agent.secret_key().to_secret_hex(),
        "challenge": challenge,
        "signature": agent_signature(agent, challenge, &who.keys, &who.device),
        "auth_tag": kuzz_sdk::nip_oa::compute_auth_tag(&who.keys, &agent.public_key(), "").unwrap(),
    })
}

async fn register(
    inst: &Instance,
    who: &Owner,
    agent: &nostr::Keys,
    move_host: bool,
) -> (u16, Value) {
    let challenge = challenge(inst, who).await;
    let mut body = register_body(agent, who, &challenge);
    if move_host {
        body["move"] = json!(true);
    }
    call(inst, who, "POST", "/auth/agents/register", Some(body)).await
}

async fn listed(inst: &Instance, who: &Owner) -> Vec<(String, String)> {
    let (status, body) = call(inst, who, "GET", "/auth/agents/keys", None).await;
    assert_eq!(status, 200, "{body}");
    body["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| {
            (
                key["agent_pubkey"].as_str().unwrap().to_owned(),
                key["secret_key"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn register_stores_a_copy_restored_only_on_its_host() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let agent = nostr::Keys::generate();

    let (status, body) = register(&inst, &pc, &agent, false).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["result"], "registered");
    assert_eq!(body["owner"], pc.keys.public_key().to_hex());
    assert_eq!(body["host_device_id"], pc.device_id.to_string());
    let copies = listed(&inst, &pc).await;
    assert_eq!(
        copies,
        [(
            agent.public_key().to_hex(),
            agent.secret_key().to_secret_hex()
        )]
    );
    // Stored encrypted only.
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM agent_key_backups WHERE agent_pubkey = $1")
            .bind(agent.public_key().to_bytes().as_slice())
            .fetch_one(inst.state.db.pool())
            .await
            .expect("copy row");
    assert_ne!(stored[..32], agent.secret_key().to_secret_bytes());

    // Same owner and host again: idempotent.
    let (status, body) = register(&inst, &pc, &agent, false).await;
    assert_eq!((status, body["result"].clone()), (200, json!("unchanged")));

    // Another device of the owner: needs `move`, then the copy follows.
    let laptop = other_device(&inst, &pc, &subject).await;
    assert!(
        listed(&inst, &laptop).await.is_empty(),
        "scoped to the host"
    );
    let (status, body) = register(&inst, &laptop, &agent, false).await;
    assert_eq!(
        (status, body["code"].clone()),
        (409, json!("agent_on_other_host"))
    );
    let (status, body) = register(&inst, &laptop, &agent, true).await;
    assert_eq!((status, body["result"].clone()), (200, json!("moved")));
    assert!(listed(&inst, &pc).await.is_empty());
    assert_eq!(listed(&inst, &laptop).await.len(), 1);

    // A challenge is used once and only by the device it was issued to.
    let used = challenge(&inst, &laptop).await;
    let body = register_body(&agent, &laptop, &used);
    let first = call(
        &inst,
        &laptop,
        "POST",
        "/auth/agents/register",
        Some(body.clone()),
    )
    .await;
    assert_eq!(first.0, 200);
    let (status, body) = call(&inst, &laptop, "POST", "/auth/agents/register", Some(body)).await;
    assert_eq!(
        (status, body["code"].clone()),
        (400, json!("invalid_proof"))
    );
    let laptop_challenge = challenge(&inst, &laptop).await;
    let body = register_body(&agent, &pc, &laptop_challenge);
    let (status, body) = call(&inst, &pc, "POST", "/auth/agents/register", Some(body)).await;
    assert_eq!(
        (status, body["code"].clone()),
        (400, json!("invalid_proof"))
    );

    // Without the device proof the call is refused under enforce.
    let response = inst
        .get("/auth/agents/keys")
        .bearer_auth(&pc.access)
        .send()
        .await
        .expect("send");
    assert_eq!(response.status(), 401);
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn register_refuses_bad_proofs_and_other_accounts() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    let expect = |(status, body): (u16, Value), want: (u16, &str)| {
        assert_eq!(
            (status, body["code"].as_str().unwrap_or_default()),
            want,
            "{body}"
        );
    };

    // The agent signature must cover this challenge, owner and device.
    let used = challenge(&inst, &pc).await;
    let mut body = register_body(&agent, &pc, &used);
    body["signature"] = json!(agent_signature(&agent, &used, &pc.keys, &device_key()));
    expect(
        call(&inst, &pc, "POST", "/auth/agents/register", Some(body)).await,
        (400, "invalid_proof"),
    );
    let used = challenge(&inst, &pc).await;
    let mut body = register_body(&agent, &pc, &used);
    body["signature"] = json!(agent_signature(
        &nostr::Keys::generate(),
        &used,
        &pc.keys,
        &pc.device
    ));
    expect(
        call(&inst, &pc, "POST", "/auth/agents/register", Some(body)).await,
        (400, "invalid_proof"),
    );
    // The NIP-OA tag must name this account's key.
    let used = challenge(&inst, &pc).await;
    let mut body = register_body(&agent, &pc, &used);
    body["auth_tag"] = json!(kuzz_sdk::nip_oa::compute_auth_tag(
        &nostr::Keys::generate(),
        &agent.public_key(),
        ""
    )
    .unwrap());
    expect(
        call(&inst, &pc, "POST", "/auth/agents/register", Some(body)).await,
        (403, "auth_tag_invalid"),
    );
    // The secret must be the agent's.
    let used = challenge(&inst, &pc).await;
    let mut body = register_body(&agent, &pc, &used);
    body["secret_key"] = json!(nostr::Keys::generate().secret_key().to_secret_hex());
    expect(
        call(&inst, &pc, "POST", "/auth/agents/register", Some(body)).await,
        (400, "invalid_proof"),
    );
    let none: i64 = sqlx::query_scalar("SELECT count(*) FROM key_registry WHERE pubkey = $1")
        .bind(agent.public_key().to_bytes().as_slice())
        .fetch_one(inst.state.db.pool())
        .await
        .unwrap();
    assert_eq!(none, 0, "nothing was registered");

    // Registered to this owner, another account cannot take it, even with move.
    assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);
    let (stranger, _) = owner(&inst).await;
    expect(
        register(&inst, &stranger, &agent, true).await,
        (409, "agent_owned_elsewhere"),
    );
    // An account key is never an agent.
    let (other, _) = owner(&inst).await;
    let used = challenge(&inst, &pc).await;
    let mut body = register_body(&other.keys, &pc, &used);
    body["auth_tag"] =
        json!(kuzz_sdk::nip_oa::compute_auth_tag(&pc.keys, &other.keys.public_key(), "").unwrap());
    expect(
        call(&inst, &pc, "POST", "/auth/agents/register", Some(body)).await,
        (409, "key_in_use"),
    );
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn agent_endpoints_need_the_device_proof_in_every_auth_mode() {
    use crate::identity::device::{override_mode, Surface};
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let agent = nostr::Keys::generate();
    assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);
    let thief = device_key();
    let code = |(status, body): (u16, Value)| {
        (status, body["code"].as_str().unwrap_or_default().to_owned())
    };
    let refused = (401, "device_proof_required".to_owned());

    for mode in [DeviceAdmissionMode::Off, DeviceAdmissionMode::Log] {
        override_mode(&inst.state, Surface::Auth, mode);
        // A copied token alone, or with another device's proof, reaches nothing.
        for signer in [None, Some(&thief)] {
            for (method, path) in [
                ("GET", "/auth/agents/keys"),
                ("POST", "/auth/agents/challenge"),
            ] {
                assert_eq!(
                    code(call_proven_by(&inst, &pc, signer, method, path, None).await),
                    refused,
                    "{mode:?} {path}"
                );
            }
            let used = challenge(&inst, &pc).await;
            let body = register_body(&agent, &pc, &used);
            assert_eq!(
                code(
                    call_proven_by(
                        &inst,
                        &pc,
                        signer,
                        "POST",
                        "/auth/agents/register",
                        Some(body)
                    )
                    .await
                ),
                refused,
                "{mode:?} register"
            );
        }
        // The device's own proof still works in this mode.
        assert_eq!(listed(&inst, &pc).await.len(), 1, "{mode:?}");
        assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);
    }

    // A device signed in while the auth surface was off has no device key.
    override_mode(&inst.state, Surface::Auth, DeviceAdmissionMode::Off);
    let keyless = other_device(&inst, &pc, &subject).await;
    assert_eq!(
        code(call(&inst, &keyless, "GET", "/auth/agents/keys", None).await),
        (409, "device_key_required".to_owned())
    );
    override_mode(&inst.state, Surface::Auth, DeviceAdmissionMode::Enforce);

    // The listing needs a sign-in from the last five minutes.
    sqlx::query(
        "UPDATE sessions SET created_at = now() - interval '10 minutes' \
          WHERE device_id = $1 AND revoked_at IS NULL",
    )
    .bind(pc.device_id)
    .execute(inst.state.db.pool())
    .await
    .unwrap();
    assert_eq!(
        code(call(&inst, &pc, "GET", "/auth/agents/keys", None).await),
        (403, "reauth_required".to_owned())
    );
    // Registration does not.
    let (status, body) = register(&inst, &pc, &agent, false).await;
    assert_eq!((status, body["result"].clone()), (200, json!("unchanged")));

    // A full device refuses new agents instead of cutting the listing short.
    sqlx::query(
        "INSERT INTO key_registry (pubkey, principal_id, role, host_device_id) \
         SELECT decode(md5(random()::text || g::text) || md5(g::text || random()::text), 'hex'), \
                d.principal_id, 'agent', d.id \
           FROM devices d, generate_series(1, 499) AS g WHERE d.id = $1",
    )
    .bind(pc.device_id)
    .execute(inst.state.db.pool())
    .await
    .unwrap();
    assert_eq!(
        code(register(&inst, &pc, &nostr::Keys::generate(), false).await),
        (409, "device_agent_limit".to_owned())
    );
}

/// NIP-98 GET of `path` by `keys` with a device tag from `device`.
async fn status_call(
    inst: &Instance,
    keys: &nostr::Keys,
    device: &SigningKey,
    path: &str,
) -> (u16, Value) {
    let scheme = if inst.state.config.relay_url.starts_with("wss://") {
        "https"
    } else {
        "http"
    };
    let created_at = nostr::Timestamp::now();
    let public = DevicePubkey::from_sec1(device.verifying_key().to_encoded_point(true).as_bytes())
        .expect("device pubkey");
    let mut tags = vec![
        vec!["u".to_owned(), format!("{scheme}://{}{path}", inst.host)],
        vec!["method".to_owned(), "GET".to_owned()],
        vec![DEVICE_TAG.to_owned(), public.to_hex(), String::new()],
    ];
    let message = event_proof_message(
        &keys.public_key().to_hex(),
        created_at.as_secs(),
        27235,
        &tags,
        "",
    );
    let signature: Signature = device.sign(message.as_bytes());
    tags[2][2] = hex::encode(signature.to_bytes());
    let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
        .tags(
            tags.into_iter()
                .map(|tag| nostr::Tag::parse(tag).expect("tag")),
        )
        .custom_created_at(created_at)
        .sign_with_keys(keys)
        .expect("sign");
    let header = format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap())
    );
    let response = inst
        .get(path)
        .header("authorization", header)
        .send()
        .await
        .expect("send");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// NIP-42 AUTH of `keys` from `device`: (accepted, reason).
pub(super) async fn ws_auth(
    inst: &Instance,
    keys: &nostr::Keys,
    device: &SigningKey,
) -> (bool, String) {
    let mut request = format!("ws://{}/", inst.addr)
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("host", inst.host.parse().expect("host header"));
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.expect("ws");
    let challenge = next_json(&mut ws).await;
    let event = auth_event(
        keys,
        challenge[1].as_str().expect("challenge"),
        &format!("ws://{}", inst.host),
        Some(device),
    );
    send(&mut ws, json!(["AUTH", event])).await;
    let ok = next_ok(&mut ws).await;
    (
        ok[2].as_bool().expect("accepted"),
        ok[3].as_str().unwrap_or_default().to_owned(),
    )
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_registered_agent_runs_on_its_host_and_status_reports_it() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let laptop = other_device(&inst, &pc, &subject).await;
    let agent = nostr::Keys::generate();

    // Bound to the owner by the backfill but not registered yet: parked.
    sqlx::query(
        "INSERT INTO key_registry (pubkey, principal_id, role) \
         SELECT $1, principal_id, 'agent' FROM devices WHERE id = $2",
    )
    .bind(agent.public_key().to_bytes().as_slice())
    .bind(pc.device_id)
    .execute(inst.state.db.pool())
    .await
    .unwrap();
    assert_eq!(
        ws_auth(&inst, &agent, &pc.device).await,
        (false, "restricted: unbound".to_owned())
    );
    assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);

    // Key admission: the host with the owner signed in.
    assert_eq!(
        ws_auth(&inst, &agent, &pc.device).await,
        (true, String::new())
    );
    let laptop_key = DevicePubkey::from_sec1(
        laptop
            .device
            .verifying_key()
            .to_encoded_point(true)
            .as_bytes(),
    )
    .unwrap();
    assert!(matches!(
        inst.state
            .db
            .key_admission(&agent.public_key().to_bytes(), Some(laptop_key.as_bytes()))
            .await
            .unwrap(),
        kuzz_db::identity::KeyAdmission::Refused { .. }
    ));

    // The agent asks about itself, its owner and a stranger.
    let stranger = nostr::Keys::generate().public_key().to_hex();
    let path = format!(
        "/agents/status?pubkeys={},{},{stranger}",
        agent.public_key().to_hex(),
        pc.keys.public_key().to_hex()
    );
    let (status, body) = status_call(&inst, &agent, &pc.device, &path).await;
    assert_eq!(status, 200, "{body}");
    let owner_hex = pc.keys.public_key().to_hex();
    assert_eq!(
        body,
        json!({"admission": "enforce", "keys": [
            {"pubkey": agent.public_key().to_hex(), "role": "agent", "owner": owner_hex, "live": true},
            {"pubkey": owner_hex, "role": "person", "owner": owner_hex, "live": true},
            {"pubkey": stranger, "role": null, "owner": null, "live": false},
        ]})
    );
    // Not from another device, and not by a stranger's key.
    let single = format!("/agents/{}/status", agent.public_key().to_hex());
    let (status, body) = status_call(&inst, &agent, &laptop.device, &single).await;
    assert_eq!((status, body["code"].clone()), (403, json!("not_admitted")));
    assert!(
        body["error"].as_str().unwrap().ends_with("(wrong_host)"),
        "{body}"
    );
    assert_eq!(body["admission"], "enforce");
    let (status, _) = status_call(&inst, &nostr::Keys::generate(), &pc.device, &single).await;
    assert_eq!(status, 403);

    // The owner signs out on the host: the agent is no longer live there.
    sqlx::query(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = 'logout' WHERE device_id = $1",
    )
    .bind(pc.device_id)
    .execute(inst.state.db.pool())
    .await
    .unwrap();
    let (status, body) = status_call(&inst, &pc.keys, &laptop.device, &single).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["keys"][0]["role"], "agent");
    assert_eq!(body["keys"][0]["live"], false);

    // Under `off` nothing is evaluated.
    let off = agents_instance(DeviceAdmissionMode::Off).await;
    let (status, body) = status_call(&off, &agent, &pc.device, &single).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({"admission": "off", "keys": []}));
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn status_names_log_mode_on_a_wrong_host_refusal() {
    // Under `log` the relay still admits the agent elsewhere: the refusal
    // says so, and kuzz-acp keeps running (it stops only under `enforce`).
    let inst = agents_instance(DeviceAdmissionMode::Log).await;
    let (pc, subject) = owner(&inst).await;
    let laptop = other_device(&inst, &pc, &subject).await;
    let agent = nostr::Keys::generate();
    assert_eq!(register(&inst, &pc, &agent, false).await.0, 200);

    let single = format!("/agents/{}/status", agent.public_key().to_hex());
    let (status, body) = status_call(&inst, &agent, &laptop.device, &single).await;
    assert_eq!((status, body["code"].clone()), (403, json!("not_admitted")));
    assert!(
        body["error"].as_str().unwrap().ends_with("(wrong_host)"),
        "{body}"
    );
    assert_eq!(body["admission"], "log");

    let (status, body) = status_call(&inst, &agent, &pc.device, &single).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["admission"], "log");
}
