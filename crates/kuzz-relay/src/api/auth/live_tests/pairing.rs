//! External agent pairing (device-bound keys plan §14.7, PR A4) over the
//! production router: start, a claim from another computer that connects at
//! once, the owner's list, disconnecting, and admission of the paired key.

use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};

use super::agent_keys::{
    agents_instance, call, call_proven_by, device_hex, device_key, owner, sign_in, ws_auth, Owner,
};
use super::*;
use crate::identity::config::DeviceAdmissionMode;

/// An agent on another computer.
struct External {
    keys: nostr::Keys,
    device: SigningKey,
}

fn external() -> External {
    External {
        keys: nostr::Keys::generate(),
        device: device_key(),
    }
}

/// `kuzz pair` signatures of `agent` and `device` over `code`.
fn claim_body(code: &str, agent: &nostr::Keys, device: &SigningKey) -> Value {
    use nostr::hashes::{sha256::Hash as Sha256Hash, Hash as _};
    let message = kuzz_auth::pairing::pair_claim_message(
        &kuzz_auth::pairing::pair_code_hash(code),
        &agent.public_key().to_hex(),
        &device_hex(device),
    );
    let digest = Sha256Hash::hash(message.as_bytes());
    let agent_signature = agent
        .sign_schnorr(&nostr::secp256k1::Message::from_digest(
            digest.to_byte_array(),
        ))
        .to_string();
    let device_signature: Signature = device.sign(message.as_bytes());
    json!({
        "code": code,
        "agent_pubkey": agent.public_key().to_hex(),
        "agent_signature": agent_signature,
        "device_pubkey": device_hex(device),
        "device_signature": hex::encode(device_signature.to_bytes()),
        "device_name": "studio mac",
    })
}

async fn reset_claim_limits(inst: &Instance) {
    let mut redis = inst.state.redis_pool.get().await.expect("redis");
    let _: () = redis::cmd("DEL")
        .arg("kuzz:ratelimit:pair:claim:127.0.0.1")
        .query_async(&mut redis)
        .await
        .expect("reset claim limits");
}

/// `POST /pair/claim` with no account session: (status, code or state).
async fn post_claim(inst: &Instance, body: &Value) -> (u16, Value) {
    reset_claim_limits(inst).await;
    let response = inst
        .post("/pair/claim")
        .json(body)
        .send()
        .await
        .expect("send");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn claim(inst: &Instance, code: &str, agent: &External) -> (u16, Value) {
    post_claim(inst, &claim_body(code, &agent.keys, &agent.device)).await
}

/// Start a pairing: (pairing id, code).
async fn start(inst: &Instance, who: &Owner) -> (String, String) {
    let (status, body) = call(inst, who, "POST", "/auth/pairing/start", None).await;
    assert_eq!(status, 200, "{body}");
    let code = body["code"].as_str().unwrap().to_owned();
    // The configured public relay, never the request's Host.
    let relay = &inst.state.config.relay_url;
    assert!(!relay.contains(&inst.host), "{relay}");
    assert_eq!(body["relay_url"], json!(relay));
    assert_eq!(body["command"], format!("kuzz pair --relay {relay} {code}"));
    assert_eq!(body["expires_in"], 600);
    (body["pairing_id"].as_str().unwrap().to_owned(), code)
}

async fn list(inst: &Instance, who: &Owner) -> Vec<Value> {
    let (status, body) = call(inst, who, "GET", "/auth/pairing", None).await;
    assert_eq!(status, 200, "{body}");
    body["pairings"].as_array().unwrap().clone()
}

fn code_of((status, body): (u16, Value)) -> (u16, String) {
    (status, body["code"].as_str().unwrap_or_default().to_owned())
}

/// Key admission of `keys` from `device`, read from the database. Refusals
/// are checked here rather than over WebSocket, whose denial closes the
/// socket and can reset the connection before the client reads the `OK` on
/// Windows.
async fn admitted(inst: &Instance, keys: &nostr::Keys, device: &SigningKey) -> bool {
    let device: [u8; 33] = hex::decode(device_hex(device)).unwrap().try_into().unwrap();
    matches!(
        inst.state
            .db
            .key_admission(&keys.public_key().to_bytes(), Some(&device))
            .await
            .unwrap(),
        kuzz_db::identity::KeyAdmission::Admitted { .. }
    )
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_claimed_code_connects_the_agent_on_its_computer_only() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = external();

    let (id, code) = start(&inst, &pc).await;
    assert_eq!(code.len(), 22);
    let listed = list(&inst, &pc).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0]["id"].as_str(), &listed[0]["state"]),
        (Some(id.as_str()), &json!("waiting"))
    );

    // The first valid claim connects at once.
    let (status, body) = claim(&inst, &code, &agent).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        (body["pairing_id"].as_str(), &body["state"]),
        (Some(id.as_str()), &json!("connected"))
    );
    // The agent learns whose account key it now works for.
    assert_eq!(body["owner_pubkey"], json!(pc.keys.public_key().to_hex()));
    let external_device: uuid::Uuid = body["device_id"].as_str().unwrap().parse().unwrap();
    // The owner's list: "<agent> on <device> connected", with the time.
    let listed = list(&inst, &pc).await;
    assert_eq!(listed[0]["state"], "connected");
    assert_eq!(listed[0]["agent_pubkey"], agent.keys.public_key().to_hex());
    assert_eq!(listed[0]["device_pubkey"], device_hex(&agent.device));
    assert_eq!(listed[0]["device_name"], "studio mac");
    assert_eq!(listed[0]["device_id"], external_device.to_string());
    assert!(listed[0]["claimed_at"].is_string());
    let (stranger, _) = owner(&inst).await;
    assert!(list(&inst, &stranger).await.is_empty());

    // Admitted from the paired computer, with no login there; not from the
    // owner's own computer.
    assert_eq!(
        ws_auth(&inst, &agent.keys, &agent.device).await,
        (true, String::new())
    );
    assert!(!admitted(&inst, &agent.keys, &pc.device).await);

    // Single use: any later claim is refused, the same keys included.
    for later in [&agent, &external()] {
        assert_eq!(
            code_of(claim(&inst, &code, later).await),
            (409, "code_used".to_owned())
        );
    }

    // The computer is listed with the owner's devices; revoking it
    // disconnects the agent.
    let (status, body) = call(&inst, &pc, "GET", "/auth/devices", None).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.to_string().contains(&external_device.to_string()));
    let path = format!("/auth/devices/{external_device}");
    assert_eq!(call(&inst, &pc, "DELETE", &path, None).await.0, 204);
    assert!(!admitted(&inst, &agent.keys, &agent.device).await);
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn claims_expire_and_refuse_bad_or_taken_keys() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    let agent = external();

    // Expired and unknown codes connect nothing; the owner sees `expired`.
    let (expired, code) = start(&inst, &pc).await;
    sqlx::query("UPDATE pairing_requests SET expires_at = now() WHERE id = $1::uuid")
        .bind(&expired)
        .execute(inst.state.db.pool())
        .await
        .unwrap();
    assert_eq!(
        code_of(claim(&inst, &code, &agent).await),
        (404, "invalid_code".to_owned())
    );
    assert_eq!(list(&inst, &pc).await[0]["state"], "expired");
    let (unknown, _) = kuzz_auth::pairing::generate_pair_code();
    assert_eq!(
        code_of(claim(&inst, &unknown, &agent).await),
        (404, "invalid_code".to_owned())
    );
    assert_eq!(
        code_of(post_claim(&inst, &json!({"code": "short"})).await),
        (400, "invalid_request".to_owned())
    );

    // Both signatures must cover this code, agent and device.
    let (_, code) = start(&inst, &pc).await;
    let mut forged = claim_body(&code, &agent.keys, &agent.device);
    forged["agent_signature"] =
        claim_body(&code, &nostr::Keys::generate(), &agent.device)["agent_signature"].clone();
    assert_eq!(
        code_of(post_claim(&inst, &forged).await),
        (400, "invalid_signature".to_owned())
    );
    let mut forged = claim_body(&code, &agent.keys, &agent.device);
    forged["device_pubkey"] = json!(device_hex(&device_key()));
    assert_eq!(
        code_of(post_claim(&inst, &forged).await),
        (400, "invalid_signature".to_owned())
    );
    let (other_code, _) = kuzz_auth::pairing::generate_pair_code();
    let mut forged = claim_body(&other_code, &agent.keys, &agent.device);
    forged["code"] = json!(code);
    assert_eq!(
        code_of(post_claim(&inst, &forged).await),
        (400, "invalid_signature".to_owned())
    );

    // S6: an account key is never paired, nor an already registered key.
    let account_agent = External {
        keys: pc.keys.clone(),
        device: device_key(),
    };
    assert_eq!(
        code_of(claim(&inst, &code, &account_agent).await),
        (409, "key_in_use".to_owned())
    );
    // The refusals left the code usable.
    assert_eq!(claim(&inst, &code, &agent).await.0, 200);
    let (_, code) = start(&inst, &pc).await;
    let same_key = External {
        keys: agent.keys.clone(),
        device: device_key(),
    };
    assert_eq!(
        code_of(claim(&inst, &code, &same_key).await),
        (409, "key_in_use".to_owned())
    );

    // Failed claims lock nobody out; only the client's own limit applies.
    for _ in 0..5 {
        let (unknown, _) = kuzz_auth::pairing::generate_pair_code();
        assert_eq!(claim(&inst, &unknown, &agent).await.0, 404);
    }
    let mut redis = inst.state.redis_pool.get().await.expect("redis");
    let _: () = redis::cmd("SET")
        .arg("kuzz:ratelimit:pair:claim:127.0.0.1")
        .arg(1000)
        .arg("EX")
        .arg(60)
        .query_async(&mut redis)
        .await
        .unwrap();
    let response = inst
        .post("/pair/claim")
        .json(&claim_body(&code, &external().keys, &agent.device))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    reset_claim_limits(&inst).await;
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn signing_in_on_the_paired_computer_never_revives_it() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let agent = external();
    let (_, code) = start(&inst, &pc).await;
    let (status, body) = claim(&inst, &code, &agent).await;
    assert_eq!(status, 200, "{body}");
    let external_device: uuid::Uuid = body["device_id"].as_str().unwrap().parse().unwrap();

    // The owner disconnects the computer, then signs in there with the same
    // device key: a separate login device; the external one stays revoked.
    let path = format!("/auth/devices/{external_device}");
    assert_eq!(call(&inst, &pc, "DELETE", &path, None).await.0, 204);
    let (_, _, signed_in, _) = sign_in(&inst, &subject, agent.device.clone()).await;
    assert_ne!(signed_in, external_device);
    let (kind, revoked): (String, bool) =
        sqlx::query_as("SELECT kind, revoked_at IS NOT NULL FROM devices WHERE id = $1")
            .bind(external_device)
            .fetch_one(inst.state.db.pool())
            .await
            .unwrap();
    assert_eq!((kind.as_str(), revoked), ("external", true));
    assert!(!admitted(&inst, &agent.keys, &agent.device).await);
    // The owner's own key works from the new login there.
    assert_eq!(
        ws_auth(&inst, &pc.keys, &agent.device).await,
        (true, String::new())
    );
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn pairing_calls_need_the_device_proof_in_every_auth_mode() {
    use crate::identity::device::{override_mode, Surface};
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, _) = owner(&inst).await;
    start(&inst, &pc).await;
    let thief = device_key();
    let refused = (401, "device_proof_required".to_owned());

    for mode in [DeviceAdmissionMode::Off, DeviceAdmissionMode::Log] {
        override_mode(&inst.state, Surface::Auth, mode);
        for signer in [None, Some(&thief)] {
            for (method, path) in [("POST", "/auth/pairing/start"), ("GET", "/auth/pairing")] {
                assert_eq!(
                    code_of(call_proven_by(&inst, &pc, signer, method, path, None).await),
                    refused,
                    "{mode:?} {path}"
                );
            }
        }
        // The device's own proof still works in this mode.
        assert_eq!(list(&inst, &pc).await.len(), 1, "{mode:?}");
    }
    override_mode(&inst.state, Surface::Auth, DeviceAdmissionMode::Enforce);
    // The refused calls started nothing.
    assert_eq!(list(&inst, &pc).await.len(), 1);
}
