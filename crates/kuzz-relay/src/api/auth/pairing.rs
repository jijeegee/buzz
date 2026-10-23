//! External agent pairing (device-bound keys plan §14.7, PR A4).
//!
//! 1. The owner presses "Connect external agent": the app calls
//!    `POST /auth/pairing/start` and shows
//!    `kuzz pair --relay <relay> <code>`. The relay is this deployment's
//!    configured public URL (`RELAY_URL`), never the request's Host. The code
//!    is 128 random bits, works once and expires after ten minutes.
//! 2. On the other computer the agent calls `POST /pair/claim` (no account
//!    session) with the code, its key, that computer's device key and both
//!    keys' signatures over [`kuzz_auth::pairing::pair_claim_message`]. The
//!    first valid claim connects at once (no approval step): the computer
//!    becomes an `external` device of the owner and the agent key is bound
//!    to it. The reply names the owner's account key (`owner_pubkey`). Any
//!    later claim of the code is refused (`code_used`).
//! 3. The app lists `GET /auth/pairing` ("<agent> on <device> connected",
//!    with the claim time). The owner disconnects the computer with the
//!    device revoke call (`DELETE /auth/devices/{device_id}`).
//!
//! Key admission then accepts that key from that computer only, with no
//! login there. A claim is refused once the device that started the pairing
//! is revoked or the owner's account is disabled; after a disconnect the
//! same owner may pair the agent key again (on a new external device).
//! Claims are rate limited per IPv4 address and per IPv6 /64 only.
//!
//! The `/auth/pairing` calls are Bearer + device proof of a key-backup
//! account in every auth surface mode, like `/auth/agents/*`.
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
use super::{auth_error, bad_request, client_ip, rate_limit, unavailable};
use crate::state::AppState;

/// Version of this HTTP contract.
const PAIRING_API_VERSION: u16 = 1;
/// Claims per client per minute: per IPv4 address, per IPv6 /64.
const CLAIM_PER_IP: u64 = 30;
/// Claims per minute for all clients without a known IP.
const CLAIM_GLOBAL: u64 = 300;

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

/// The command the owner runs on the other computer.
fn pair_command(relay_url: &str, code: &str) -> String {
    format!("kuzz pair --relay {relay_url} {code}")
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
            "command": pair_command(&state.config.relay_url, &code),
            "relay_url": state.config.relay_url,
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

/// The rate limit bucket of a client address: an IPv4 address as is, an
/// IPv6 address by its /64 (one host usually holds a whole /64). There is no
/// deployment-wide failure lock: a few clients must not lock pairing for
/// everyone, and 128-bit codes cannot be guessed anyway.
fn claim_bucket(ip: &str) -> String {
    match ip.parse::<std::net::IpAddr>().map(|ip| ip.to_canonical()) {
        Ok(std::net::IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        Ok(std::net::IpAddr::V4(v4)) => v4.to_string(),
        Err(_) => ip.to_owned(),
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
    let limited = match client_ip(&state, &extensions, &headers) {
        Some(ip) => {
            let key = format!("pair:claim:{}", claim_bucket(&ip));
            rate_limit(&state, &key, 60, CLAIM_PER_IP).await
        }
        None => rate_limit(&state, "pair:claim:global", 60, CLAIM_GLOBAL).await,
    };
    if let Err(response) = limited {
        return response;
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
            owner_pubkey,
        } => {
            // A refusal cached before the pairing must not outlive it.
            state.identity.admissions.evict_key(&agent_bytes);
            tracing::info!(pairing = %pairing_id, device = %device_id, "auth.pairing_connected");
            axum::Json(json!({
                "pairing_id": pairing_id,
                "state": "connected",
                "device_id": device_id,
                "owner_pubkey": hex::encode(owner_pubkey),
                "version": PAIRING_API_VERSION,
            }))
            .into_response()
        }
        ClaimPairing::Used => conflict(
            "code_used",
            "this pairing code was used already; ask the owner for a new one",
        ),
        ClaimPairing::KeyInUse => conflict(
            "key_in_use",
            "this agent key is an account key or already registered",
        ),
        ClaimPairing::DeviceLimit => conflict(
            "device_limit",
            "this account already has the most paired computers it can",
        ),
        ClaimPairing::Invalid => invalid_code(),
    }
}

fn invalid_code() -> Response {
    auth_error(
        StatusCode::NOT_FOUND,
        "invalid_code",
        "this pairing code is unknown, expired or no longer valid",
    )
}

#[cfg(test)]
mod tests {
    use super::{claim_bucket, pair_command};

    #[test]
    fn the_command_names_the_relay_before_the_code() {
        assert_eq!(
            pair_command("wss://relay.example", "abc_DEF-123"),
            "kuzz pair --relay wss://relay.example abc_DEF-123"
        );
    }

    #[test]
    fn claims_are_limited_per_ipv4_address_and_per_ipv6_64() {
        assert_eq!(claim_bucket("203.0.113.7"), "203.0.113.7");
        assert_eq!(claim_bucket("::ffff:203.0.113.7"), "203.0.113.7");
        assert_eq!(
            claim_bucket("2001:db8:1:2:aaaa::1"),
            claim_bucket("2001:db8:1:2:bbbb:cccc:dddd:eeee")
        );
        assert_eq!(claim_bucket("2001:db8:1:2::9"), "2001:db8:1:2::/64");
        assert_ne!(
            claim_bucket("2001:db8:1:2::1"),
            claim_bucket("2001:db8:1:3::1")
        );
    }
}
