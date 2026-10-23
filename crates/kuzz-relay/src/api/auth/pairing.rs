//! External agent pairing (device-bound keys plan §14.7, PR A4).
//!
//! 1. The owner presses "Connect external agent": the app calls
//!    `POST /auth/pairing/start` and shows `kuzz pair <code>`. The code is
//!    128 random bits, works once and expires after ten minutes.
//! 2. On the other computer the agent calls `POST /pair/claim` (no account
//!    session) with the code, its key, that computer's device key and both
//!    keys' signatures over [`kuzz_auth::pairing::pair_claim_message`]. The
//!    first valid claim connects at once (no approval step): the computer
//!    becomes an `external` device of the owner and the agent key is bound
//!    to it. Any later claim of the code is refused (`code_used`).
//! 3. The app lists `GET /auth/pairing` ("<agent> on <device> connected",
//!    with the claim time). The owner disconnects the computer with the
//!    device revoke call (`DELETE /auth/devices/{device_id}`).
//!
//! Key admission then accepts that key from that computer only, with no
//! login there. The `/auth/pairing` calls are Bearer + device proof of a
//! key-backup account in every auth surface mode, like `/auth/agents/*`.
//! Codes are never logged.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{Extensions, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use kuzz_auth::device_proof::DevicePubkey;
use kuzz_auth::pairing::{generate_pair_code, is_pair_code, pair_code_hash, verify_pair_claim};
use kuzz_db::identity::{
    ClaimPairing, PairingClaim, PairingRecord, PairingState, StartPairing, PAIRING_TTL_SECS,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::agents::authorize_device_call;
use super::{auth_error, bad_request, rate_limit, rate_limit_client, unavailable};
use crate::identity::kv;
use crate::state::AppState;

/// Version of this HTTP contract.
const PAIRING_API_VERSION: u16 = 1;
/// Claims per client IP per minute.
const CLAIM_PER_IP: u64 = 30;
/// Claims per minute for all clients without a known IP.
const CLAIM_GLOBAL: u64 = 300;
/// Failed claims (bad signature, unknown, used or expired code) per minute across
/// the deployment before every claim is refused for [`CLAIM_LOCK_SECS`].
const CLAIM_FAILURES_PER_MIN: u64 = 100;
const CLAIM_LOCK_SECS: u64 = 60;
const CLAIM_LOCK_NAMESPACE: &str = "pair-claim:lock";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimRequest {
    code: String,
    agent_pubkey: String,
    agent_signature: String,
    device_pubkey: String,
    device_signature: String,
    #[serde(default)]
    device_key_kind: Option<String>,
    #[serde(default)]
    device_name: Option<String>,
}

fn conflict(code: &str, message: &str) -> Response {
    auth_error(StatusCode::CONFLICT, code, message)
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// A pairing's state as clients see it.
fn view_state(pairing: &PairingRecord) -> &'static str {
    match pairing.state {
        PairingState::Connected => "connected",
        PairingState::Open if pairing.expired() => "expired",
        PairingState::Open => "waiting",
    }
}

fn pairing_json(pairing: &PairingRecord) -> Value {
    json!({
        "id": pairing.id,
        "state": view_state(pairing),
        "agent_pubkey": pairing.agent_pubkey.map(hex::encode),
        "device_pubkey": pairing.device_pubkey.map(hex::encode),
        "device_name": pairing.device_name,
        "device_id": pairing.device_id,
        "created_at": rfc3339(pairing.created_at),
        "claimed_at": pairing.claimed_at.map(rfc3339),
        "expires_at": rfc3339(pairing.expires_at),
    })
}

/// `POST /auth/pairing/start`: a new one-use code for this account.
pub(super) async fn start(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let (binding, device, _) =
        match authorize_device_call(&state, &headers, "auth:pairing:start", 20).await {
            Ok(authorized) => authorized,
            Err(response) => return response,
        };
    let (code, code_hash) = generate_pair_code();
    match state
        .db
        .start_pairing(&binding.principal, device, &code_hash)
        .await
    {
        Ok(StartPairing::Started { id, expires_at }) => axum::Json(json!({
            "pairing_id": id,
            "command": format!("kuzz pair {code}"),
            "code": code,
            "expires_at": rfc3339(expires_at),
            "expires_in": PAIRING_TTL_SECS,
            "version": PAIRING_API_VERSION,
        }))
        .into_response(),
        Ok(StartPairing::TooMany) => auth_error(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_pairings",
            "too many unused pairing codes; use one or wait for them to expire",
        ),
        Err(_) => unavailable(),
    }
}

/// `GET /auth/pairing`: this account's pairings of the last day, newest
/// first.
pub(super) async fn list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let (binding, ..) =
        match authorize_device_call(&state, &headers, "auth:pairing:read", 120).await {
            Ok(authorized) => authorized,
            Err(response) => return response,
        };
    match state.db.recent_pairings(&binding.principal).await {
        Ok(pairings) => axum::Json(json!({
            "pairings": pairings.iter().map(pairing_json).collect::<Vec<_>>(),
            "version": PAIRING_API_VERSION,
        }))
        .into_response(),
        Err(_) => unavailable(),
    }
}

/// Count a failed claim; past [`CLAIM_FAILURES_PER_MIN`] lock every claim
/// for [`CLAIM_LOCK_SECS`]. Best effort: the per-IP limit still applies.
async fn claim_failed(state: &AppState) {
    if let Err(response) =
        rate_limit(state, "pair:claim:failures", 60, CLAIM_FAILURES_PER_MIN).await
    {
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            tracing::warn!("pair claim failures over the global limit; claims locked");
            let _ = kv::put(
                &state.redis_pool,
                CLAIM_LOCK_NAMESPACE,
                "global",
                "1",
                CLAIM_LOCK_SECS,
            )
            .await;
        }
    }
}

/// `POST /pair/claim` (no account session): an external agent uses a
/// pairing code to connect its key on its computer.
pub(super) async fn claim(
    State(state): State<Arc<AppState>>,
    extensions: Extensions,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = rate_limit_client(
        &state,
        &extensions,
        &headers,
        "pair:claim",
        CLAIM_PER_IP,
        CLAIM_GLOBAL,
    )
    .await
    {
        return response;
    }
    match kv::get(&state.redis_pool, CLAIM_LOCK_NAMESPACE, "global").await {
        Ok(None) => {}
        Ok(Some(_)) => {
            return auth_error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                &format!("too many failed pairing claims; retry in {CLAIM_LOCK_SECS}s"),
            )
        }
        Err(_) => return unavailable(),
    }
    let Ok(request) = serde_json::from_slice::<ClaimRequest>(&body) else {
        return bad_request(
            "code, agent_pubkey, agent_signature, device_pubkey and device_signature are required",
        );
    };
    let agent = match nostr::PublicKey::from_hex(&request.agent_pubkey) {
        Ok(agent) if agent.to_hex() == request.agent_pubkey => agent,
        _ => return bad_request("agent_pubkey must be a canonical lowercase public key"),
    };
    let Ok(device) = DevicePubkey::from_hex(&request.device_pubkey) else {
        return bad_request("device_pubkey must be a P-256 SEC1 public key");
    };
    let key_kind = request.device_key_kind.as_deref().unwrap_or("software");
    if !matches!(key_kind, "hardware" | "software") {
        return bad_request("device_key_kind must be hardware or software");
    }
    if let Some(name) = &request.device_name {
        if let Err(response) = super::validate_device_name(name) {
            return response;
        }
    }
    if !is_pair_code(&request.code) {
        claim_failed(&state).await;
        return invalid_code();
    }
    if verify_pair_claim(
        &request.code,
        &agent,
        &request.agent_signature,
        &device,
        &request.device_signature,
    )
    .is_err()
    {
        claim_failed(&state).await;
        return auth_error(
            StatusCode::BAD_REQUEST,
            "invalid_signature",
            "the agent and device signatures must cover this code, agent and device",
        );
    }
    let code_hash = pair_code_hash(&request.code);
    let agent_bytes = agent.to_bytes();
    let outcome = match state
        .db
        .claim_pairing(&PairingClaim {
            code_hash: &code_hash,
            agent_pubkey: &agent_bytes,
            device_pubkey: device.as_bytes(),
            device_key_kind: key_kind,
            device_name: request.device_name.as_deref(),
        })
        .await
    {
        Ok(outcome) => outcome,
        Err(_) => return unavailable(),
    };
    match outcome {
        ClaimPairing::Connected {
            pairing_id,
            device_id,
        } => {
            // A refusal cached before the pairing must not outlive it.
            state.identity.admissions.evict_key(&agent_bytes);
            tracing::info!(pairing = %pairing_id, device = %device_id, "auth.pairing_connected");
            axum::Json(json!({
                "pairing_id": pairing_id,
                "state": "connected",
                "device_id": device_id,
                "version": PAIRING_API_VERSION,
            }))
            .into_response()
        }
        ClaimPairing::Used => {
            claim_failed(&state).await;
            conflict(
                "code_used",
                "this pairing code was used already; ask the owner for a new one",
            )
        }
        ClaimPairing::KeyInUse => conflict(
            "key_in_use",
            "this agent key is an account key or already registered",
        ),
        ClaimPairing::DeviceLimit => conflict(
            "device_limit",
            "this account already has the most paired computers it can",
        ),
        ClaimPairing::Invalid => {
            claim_failed(&state).await;
            invalid_code()
        }
    }
}

fn invalid_code() -> Response {
    auth_error(
        StatusCode::NOT_FOUND,
        "invalid_code",
        "this pairing code is unknown or expired",
    )
}
