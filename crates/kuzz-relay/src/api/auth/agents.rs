//! Kuzz agent registration and key copies (device-bound keys plan §4, PR A3).
//!
//! An owner's app registers each agent key from the device that runs it:
//!
//! 1. `POST /auth/agents/challenge` issues a one-use challenge bound to the
//!    account and the calling device (Redis, [`CHALLENGE_TTL`] seconds).
//! 2. The agent signs [`kuzz_auth::key_backup::agent_register_message`] over
//!    the challenge, the owner's own key and the device key, and
//!    `POST /auth/agents/register` sends that signature, the agent's NIP-OA
//!    tag naming the owner's key, and the agent's secret key. The server
//!    encrypts the key with its backup master key and binds agent, owner and
//!    host device in one transaction.
//! 3. `GET /auth/agents/keys` returns this device's copies after a sign-in.
//!
//! All three are Bearer + device proof calls of a key-backup account. Unlike
//! other `/auth/*` calls the device proof is checked in every auth surface
//! mode, so a device without a recorded device key cannot host an agent, and
//! the key listing needs a sign-in from the last five minutes. `GET /agents/{pubkey}/status` (NIP-98 with a device tag) tells an
//! admitted key which keys are its owner's and whether they can run now.
//! These endpoints are new; no existing route changes. Secret keys are never
//! logged.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use kuzz_auth::device_proof::{verify_event_device_tag, DevicePubkey};
use kuzz_auth::key_backup::EncryptedKeyBackup;
use kuzz_auth::TokenBinding;
use kuzz_db::identity::{AgentKeyBackupRecord, KeyAdmission, KeyRole, RegisterAgentKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{auth_error, authenticate_user_token, bad_request, rate_limit};
use crate::identity::config::DeviceAdmissionMode;
use crate::identity::device::{self, Surface};
use crate::identity::kv;
use crate::nip_fi_http::{admit_nip_fi_http_on_state, Nip98Proof};
use crate::state::AppState;

const CHALLENGE_TTL: u64 = 120;
const CHALLENGE_NAMESPACE: &str = "agent-register:challenge";
/// Version of this HTTP contract.
const AGENT_KEYS_API_VERSION: u16 = 1;
/// Most keys one status call asks about.
pub(crate) const MAX_STATUS_KEYS: usize = 64;

#[derive(Serialize, Deserialize)]
struct Challenge {
    account: String,
    device: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterRequest {
    agent_pubkey: String,
    #[serde(deserialize_with = "deserialize_secret")]
    secret_key: Zeroizing<String>,
    challenge: String,
    signature: String,
    auth_tag: String,
    #[serde(default, rename = "move")]
    move_host: bool,
}

fn deserialize_secret<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(deserializer).map(Zeroizing::new)
}

#[derive(Serialize)]
struct AgentCopy<'a> {
    agent_pubkey: String,
    secret_key: &'a str,
}

/// Serialized straight from the zeroized secrets (no `Value` copies).
#[derive(Serialize)]
struct AgentCopies<'a> {
    keys: Vec<AgentCopy<'a>>,
    version: u16,
}

fn unavailable() -> Response {
    auth_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "backup_unavailable",
        "agent key copies unavailable",
    )
}

fn invalid_proof() -> Response {
    auth_error(
        StatusCode::BAD_REQUEST,
        "invalid_proof",
        "agent key proof is invalid or expired",
    )
}

fn conflict(code: &str, message: &str) -> Response {
    auth_error(StatusCode::CONFLICT, code, message)
}

/// A user session of a key-backup account on a device with a device key,
/// proven by that key in every auth surface mode: the binding, its device id
/// and device key. `fresh`: the session must come from a sign-in in the last
/// five minutes, like key-backup restore.
async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    scope: &str,
    per_minute: u64,
    fresh: bool,
) -> Result<(TokenBinding, Uuid, DevicePubkey), Response> {
    if state.identity.config().key_backup_master.is_none() {
        return Err(super::not_found("agent key copies are not enabled"));
    }
    let binding = authenticate_user_token(state, headers).await?;
    // A stored key that no longer parses counts as keyless (fails closed).
    let key = binding
        .device_pubkey
        .and_then(|key| DevicePubkey::from_sec1(&key).ok());
    let (device, key) = match (binding.device_id, key) {
        (Some(device), Some(key)) => (device, key),
        _ => {
            return Err(conflict(
                "device_key_required",
                "this device has no device key; sign in again with a current app",
            ))
        }
    };
    // Unlike other /auth calls, never relaxed by the auth surface mode: a
    // copied token alone must not reach agent secrets.
    device::check_request_proof_always(state, &key, headers, &binding.token_hash)
        .await
        .map_err(super::rejection_response)?;
    match state.db.account_key_mode(&binding.principal).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(conflict(
                "account_mode_conflict",
                "only key-backup accounts own Kuzz agents",
            ))
        }
        Err(_) => return Err(unavailable()),
    }
    rate_limit(
        state,
        &format!("{scope}:{}", binding.principal),
        60,
        per_minute,
    )
    .await?;
    if fresh {
        let session = binding.session_id.ok_or_else(reauth_required)?;
        match state
            .db
            .key_backup_session_fresh(&binding.principal, session)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(reauth_required()),
            Err(_) => return Err(unavailable()),
        }
    }
    Ok((binding, device, key))
}

fn reauth_required() -> Response {
    auth_error(
        StatusCode::FORBIDDEN,
        "reauth_required",
        "sign in with Google again before restoring agent keys",
    )
}

/// Issue a one-use registration challenge for this account and device.
pub(super) async fn challenge(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let (binding, device, _) =
        match authorize(&state, &headers, "auth:agents:challenge", 120, false).await {
            Ok(authorized) => authorized,
            Err(response) => return response,
        };
    let record = Challenge {
        account: binding.principal.to_hex(),
        device,
    };
    let challenge = hex::encode(rand::random::<[u8; 32]>());
    let Ok(json) = serde_json::to_string(&record) else {
        return unavailable();
    };
    match kv::put_new(
        &state.redis_pool,
        CHALLENGE_NAMESPACE,
        &challenge,
        &json,
        CHALLENGE_TTL,
    )
    .await
    {
        Ok(true) => {
            axum::Json(json!({"challenge": challenge, "expires_in": CHALLENGE_TTL})).into_response()
        }
        Ok(false) | Err(_) => unavailable(),
    }
}

/// Register an agent key on the calling device and store its copy.
pub(super) async fn register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (binding, device, device_key) =
        match authorize(&state, &headers, "auth:agents:register", 120, false).await {
            Ok(authorized) => authorized,
            Err(response) => return response,
        };
    // Wipe owned parsing/secret buffers on every return path. Never log
    // parse or crypto errors: they may quote the secret.
    let body = Zeroizing::new(body.to_vec());
    let Ok(request) = serde_json::from_slice::<RegisterRequest>(&body) else {
        return bad_request(
            "agent_pubkey, secret_key, challenge, signature and auth_tag are required",
        );
    };
    if request.challenge.len() != 64 || !request.challenge.bytes().all(|b| b.is_ascii_hexdigit()) {
        return invalid_proof();
    }
    // GETDEL consumes the challenge before every other check, so a failed
    // or concurrent attempt never gets another try with it.
    let record: Challenge =
        match kv::take(&state.redis_pool, CHALLENGE_NAMESPACE, &request.challenge).await {
            Ok(Some(json)) => match serde_json::from_str(&json) {
                Ok(record) => record,
                Err(_) => return invalid_proof(),
            },
            Ok(None) => return invalid_proof(),
            Err(_) => return unavailable(),
        };
    if record.account != binding.principal.to_hex() || record.device != device {
        return invalid_proof();
    }
    let agent = match nostr::PublicKey::from_hex(&request.agent_pubkey) {
        Ok(agent) if agent.to_hex() == request.agent_pubkey => agent,
        _ => return bad_request("agent_pubkey must be a canonical lowercase public key"),
    };
    let owner_key = match state.db.account_pubkey(&binding.principal).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            return conflict(
                "owner_key_missing",
                "the account has no key yet; finish key setup first",
            )
        }
        Err(_) => return unavailable(),
    };
    // The agent's NIP-OA tag must name this account's own key.
    match kuzz_sdk::nip_oa::verify_auth_tag(&request.auth_tag, &agent) {
        Ok(owner) if owner.to_bytes() == owner_key => {}
        _ => {
            return auth_error(
                StatusCode::FORBIDDEN,
                "auth_tag_invalid",
                "the agent's owner attestation must name this account's key",
            )
        }
    }
    if kuzz_auth::key_backup::verify_agent_register(
        &agent,
        &request.signature,
        &request.challenge,
        &hex::encode(owner_key),
        &device_key.to_hex(),
    )
    .is_err()
    {
        return invalid_proof();
    }
    let Some(master) = &state.identity.config().key_backup_master else {
        return unavailable();
    };
    let encrypted = match master.encrypt_agent(binding.principal.as_bytes(), &request.secret_key) {
        Ok(encrypted) if encrypted.pubkey == agent.to_bytes() => encrypted,
        _ => return invalid_proof(),
    };
    let copy = AgentKeyBackupRecord {
        agent_pubkey: encrypted.pubkey,
        version: encrypted.version,
        key_id: encrypted.key_id,
        nonce: encrypted.nonce,
        ciphertext: encrypted.ciphertext,
    };
    let outcome = match state
        .db
        .register_agent_key(&binding.principal, device, &copy, request.move_host)
        .await
    {
        Ok(outcome) => outcome,
        Err(_) => return unavailable(),
    };
    let result = match outcome {
        RegisterAgentKey::Registered => "registered",
        RegisterAgentKey::Unchanged => "unchanged",
        RegisterAgentKey::Moved => "moved",
        RegisterAgentKey::OtherHost => {
            return conflict(
                "agent_on_other_host",
                "this agent runs on another of your devices; send move: true to move it here",
            )
        }
        RegisterAgentKey::OtherOwner => {
            return conflict(
                "agent_owned_elsewhere",
                "this agent key belongs to another account",
            )
        }
        RegisterAgentKey::OtherRole => {
            return conflict("key_in_use", "this key is not a Kuzz agent key")
        }
        RegisterAgentKey::Revoked => return conflict("agent_revoked", "this agent was removed"),
        RegisterAgentKey::DeviceFull => {
            return conflict(
                "device_agent_limit",
                "this device already hosts the most agents one device can",
            )
        }
        RegisterAgentKey::DeviceGone => {
            return super::rejection_response(crate::identity::TokenRejection::TokenRevoked)
        }
    };
    // The key's host changed (or first got one): drop cached answers here.
    state.identity.admissions.evict_key(&agent.to_bytes());
    if result == "moved" {
        // Every replica rechecks the agent's open sockets: the old host's
        // close as no longer this key's device (A4b).
        crate::identity::publish_key_revocation(&state, &[], &[agent.to_bytes()], "agent_moved")
            .await;
    }
    axum::Json(json!({
        "agent_pubkey": agent.to_hex(),
        "owner": hex::encode(owner_key),
        "host_device_id": device,
        "result": result,
        "version": AGENT_KEYS_API_VERSION,
    }))
    .into_response()
}

/// This device's agent key copies, for restore after a sign-in.
pub(super) async fn keys(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let (binding, device, _) = match authorize(&state, &headers, "auth:agent-keys", 10, true).await
    {
        Ok(authorized) => authorized,
        Err(response) => return response,
    };
    let copies = match state.db.device_agent_keys(&binding.principal, device).await {
        Ok(copies) => copies,
        Err(_) => return unavailable(),
    };
    let Some(master) = &state.identity.config().key_backup_master else {
        return unavailable();
    };
    let mut secrets = Vec::with_capacity(copies.len());
    for copy in &copies {
        let envelope = EncryptedKeyBackup {
            pubkey: copy.agent_pubkey,
            version: copy.version,
            key_id: copy.key_id.clone(),
            nonce: copy.nonce.clone(),
            ciphertext: copy.ciphertext.clone(),
        };
        // One unreadable copy fails the whole answer: a partial list would
        // read as "these are all your agents".
        match master.decrypt_agent(binding.principal.as_bytes(), &envelope) {
            Ok(secret) => secrets.push(secret),
            Err(_) => return unavailable(),
        }
    }
    let keys: Vec<AgentCopy<'_>> = copies
        .iter()
        .zip(&secrets)
        .map(|(copy, secret)| AgentCopy {
            agent_pubkey: hex::encode(copy.agent_pubkey),
            secret_key: secret.as_str(),
        })
        .collect();
    axum::Json(AgentCopies {
        keys,
        version: AGENT_KEYS_API_VERSION,
    })
    .into_response()
}

/// `GET /agents/{pubkey}/status`.
pub(crate) async fn status_one(
    State(state): State<Arc<AppState>>,
    Path(pubkey): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    status(&state, &headers, &uri, std::slice::from_ref(&pubkey)).await
}

/// `GET /agents/status?pubkeys=<hex>,<hex>`.
pub(crate) async fn status_many(
    State(state): State<Arc<AppState>>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let requested = uri.query().and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(name, _)| name == "pubkeys")
            .map(|(_, value)| value.split(',').map(str::to_owned).collect::<Vec<_>>())
    });
    match requested {
        Some(pubkeys) => status(&state, &headers, &uri, &pubkeys).await,
        None => bad_request("pubkeys is required"),
    }
}

/// The NIP-98 event of a request whose `Authorization` already verified.
fn nip98_event(headers: &HeaderMap) -> Option<nostr::Event> {
    use base64::Engine as _;
    let encoded = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Nostr ")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// What each of `requested` is to the caller's owner: `role` and `owner`
/// (the owner's own key) for the owner's keys, `null` for any other key, and
/// `live` when the key may be used right now (an agent or external key on
/// its host device, the owner's key on the caller's device). The caller must
/// be an admitted key (person, agent or external) of that owner, presented
/// from its device. Under admission `off` for WebSocket keys nothing is
/// evaluated: the answer is `{"admission": "off", "keys": []}`, and callers
/// keep their previous rules. Every answer, `403 not_admitted` included,
/// names the effective `admission` mode (`off`, `log` or `enforce`).
async fn status(
    state: &AppState,
    headers: &HeaderMap,
    uri: &Uri,
    requested: &[String],
) -> Response {
    let mut pubkeys: Vec<[u8; 32]> = Vec::with_capacity(requested.len());
    for raw in requested {
        match nostr::PublicKey::from_hex(raw) {
            Ok(key) if key.to_hex() == *raw => {
                if !pubkeys.contains(&key.to_bytes()) {
                    pubkeys.push(key.to_bytes());
                }
            }
            _ => return bad_request("pubkeys must be canonical lowercase public keys"),
        }
    }
    if pubkeys.is_empty() || pubkeys.len() > MAX_STATUS_KEYS {
        return bad_request(&format!("ask about 1-{MAX_STATUS_KEYS} keys"));
    }

    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return super::not_found("no community is configured for this host");
    };
    let path = uri
        .path_and_query()
        .map_or_else(|| uri.path(), |pq| pq.as_str());
    let url = crate::api::bridge::nip98_expected_url(&state.config.relay_url, &tenant, path);
    // A real NIP-98 event only: the dev X-Pubkey fallback carries no device.
    let admission = match admit_nip_fi_http_on_state(state, headers, || {
        crate::api::bridge::verify_bridge_auth(headers, "GET", &url, None, true)
            .map(|auth| Nip98Proof::new(auth.pubkey, auth.event_id_bytes))
            .map_err(IntoResponse::into_response)
    }) {
        Ok(admission) => admission,
        Err(response) => return response,
    };
    let caller = *admission.proven_pubkey();
    if let Err(error) =
        crate::api::bridge::check_nip98_replay(state, &tenant, admission.into_extra()).await
    {
        return error.into_response();
    }
    if let Err(response) = rate_limit(
        state,
        &format!("agents:status:{}", caller.to_hex()),
        60,
        120,
    )
    .await
    {
        return response;
    }
    // A NIP-98 HTTP call: the `http` surface.
    let mode = device::mode(state, Surface::Http);
    if mode == DeviceAdmissionMode::Off {
        return axum::Json(json!({"admission": mode.as_str(), "keys": []})).into_response();
    }
    // `admission` carries the effective mode: under `log` the relay still
    // admits the caller elsewhere, so clients must not act on the refusal.
    let not_admitted = |reason: &str| {
        (
            StatusCode::FORBIDDEN,
            axum::Json(json!({
                "error": format!("the signing key is not admitted from this device ({reason})"),
                "code": "not_admitted",
                "admission": mode.as_str(),
            })),
        )
            .into_response()
    };
    let caller_device = match nip98_event(headers).map(|event| verify_event_device_tag(&event)) {
        Some(Ok(Some(device))) => device,
        Some(Ok(None)) => return not_admitted("no_device"),
        _ => return not_admitted("bad_sig"),
    };
    let owner = match state
        .db
        .key_admission(&caller.to_bytes(), Some(caller_device.as_bytes()))
        .await
    {
        Ok(KeyAdmission::Admitted { principal, .. }) => principal,
        Ok(KeyAdmission::Refused { reason, .. }) => return not_admitted(reason.as_str()),
        Ok(KeyAdmission::Unknown) => return not_admitted("unregistered"),
        Err(_) => return super::unavailable(),
    };
    let (owner_key, owned) = match tokio::try_join!(
        state.db.account_pubkey(&owner),
        state.db.owned_keys(&owner, &pubkeys)
    ) {
        Ok(found) => found,
        Err(_) => return super::unavailable(),
    };
    let owner_hex = owner_key.map(hex::encode);
    let mut keys = Vec::with_capacity(pubkeys.len());
    for pubkey in &pubkeys {
        let Some(found) = owned.iter().find(|key| key.pubkey == *pubkey) else {
            keys.push(json!({
                "pubkey": hex::encode(pubkey), "role": null, "owner": null, "live": false,
            }));
            continue;
        };
        let device = match found.role {
            KeyRole::Person => Some(*caller_device.as_bytes()),
            KeyRole::Agent | KeyRole::External => found.host_device_pubkey,
        };
        let live = match device {
            None => false,
            Some(device) => match state.db.key_admission(pubkey, Some(&device)).await {
                Ok(admission) => matches!(admission, KeyAdmission::Admitted { .. }),
                Err(_) => return super::unavailable(),
            },
        };
        keys.push(json!({
            "pubkey": hex::encode(pubkey),
            "role": found.role.as_str(),
            "owner": owner_hex,
            "live": live,
        }));
    }
    axum::Json(json!({"admission": mode.as_str(), "keys": keys})).into_response()
}
