//! Custodial Nostr key storage. A Google account session authorizes this
//! surface only; the restored client key keeps signing the original routes.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use buzz_auth::{key_backup::EncryptedKeyBackup, TokenBinding};
use buzz_db::identity::{InitializeKeyBackup, KeyBackupRecord};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{auth_error, authenticate_user_session, bad_request, rate_limit};
use crate::{identity::kv, state::AppState};

const CHALLENGE_TTL: u64 = 120;

#[derive(Serialize, Deserialize)]
struct Challenge {
    account: String,
    session: Uuid,
    pubkey: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChallengeRequest {
    pubkey: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InitializeRequest {
    #[serde(deserialize_with = "deserialize_secret")]
    secret_key: Zeroizing<String>,
    proof: nostr::Event,
}

fn deserialize_secret<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(deserializer).map(Zeroizing::new)
}

#[derive(Serialize)]
struct RestoredKey<'a> {
    secret_key: &'a str,
    pubkey: String,
    version: u16,
}

fn unavailable() -> Response {
    auth_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "backup_unavailable",
        "key backup unavailable",
    )
}

fn invalid_proof() -> Response {
    auth_error(
        StatusCode::BAD_REQUEST,
        "invalid_proof",
        "key possession proof is invalid or expired",
    )
}

fn reauth_required() -> Response {
    auth_error(
        StatusCode::FORBIDDEN,
        "reauth_required",
        "sign in with Google again before accessing your key",
    )
}

async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    fresh: bool,
) -> Result<TokenBinding, Response> {
    if state.identity.config().key_backup_master.is_none() {
        return Err(super::not_found("key backup is not enabled"));
    }
    let binding = authenticate_user_session(state, headers).await?;
    match state.db.account_key_mode(&binding.principal).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(auth_error(
                StatusCode::CONFLICT,
                "account_mode_conflict",
                "this account uses token identity; automatic merging is not supported",
            ))
        }
        Err(_) => return Err(unavailable()),
    }
    rate_limit(
        state,
        &format!("auth:key-backup:{}", binding.principal),
        60,
        30,
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
    Ok(binding)
}

/// Only an authenticated successful `absent` response permits client creation.
pub(super) async fn status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let binding = match authorize(&state, &headers, false).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    match state.db.get_key_backup(&binding.principal).await {
        Ok(backup) => axum::Json(json!({
            "state": if backup.is_some() { "ready" } else { "absent" },
            "account_id": binding.principal.to_hex(),
            "pubkey": backup.map(|b| hex::encode(b.pubkey)),
            "version": 1,
        }))
        .into_response(),
        Err(_) => unavailable(),
    }
}

/// Issue one session/account/action-bound challenge for client key possession.
pub(super) async fn challenge(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authorize(&state, &headers, true).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let request: ChallengeRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return bad_request("pubkey is required"),
    };
    let pubkey = match nostr::PublicKey::from_hex(&request.pubkey) {
        Ok(pubkey) if pubkey.to_hex() == request.pubkey => pubkey.to_hex(),
        _ => return bad_request("pubkey must be a canonical lowercase public key"),
    };
    let Some(session) = binding.session_id else {
        return reauth_required();
    };
    let record = Challenge {
        account: binding.principal.to_hex(),
        session,
        pubkey,
        url: format!("{}/auth/key-backup", state.identity.config().public_url),
    };
    let challenge = hex::encode(rand::random::<[u8; 32]>());
    let json = match serde_json::to_string(&record) {
        Ok(json) => json,
        Err(_) => return unavailable(),
    };
    match kv::put_new(
        &state.redis_pool,
        "key-backup:challenge",
        &challenge,
        &json,
        CHALLENGE_TTL,
    )
    .await
    {
        Ok(true) => axum::Json(json!({
            "challenge": challenge, "account_id": record.account,
            "url": record.url, "expires_in": CHALLENGE_TTL,
        }))
        .into_response(),
        Ok(false) | Err(_) => unavailable(),
    }
}

/// Atomically bind and back up the submitted client-generated signing key.
pub(super) async fn initialize(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authorize(&state, &headers, true).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    // Wipe owned parsing/secret buffers on every return path (the HTTP stack
    // owns the original request allocation). Never log parse or crypto errors.
    let body = Zeroizing::new(body.to_vec());
    let request: InitializeRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return invalid_proof(),
    };
    let secret = request.secret_key;
    let proof = request.proof;
    let challenge = match proof.tags.iter().find_map(|tag| {
        let values = tag.as_slice();
        (values.len() == 2 && values[0] == "challenge").then(|| values[1].as_str())
    }) {
        Some(value) if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) => value,
        _ => return invalid_proof(),
    };
    // GETDEL consumes the challenge before every signature/key check, so bad
    // proofs and concurrent replays never get another attempt.
    let record: Challenge =
        match kv::take(&state.redis_pool, "key-backup:challenge", challenge).await {
            Ok(Some(json)) => match serde_json::from_str(&json) {
                Ok(record) => record,
                Err(_) => return invalid_proof(),
            },
            Ok(None) => return invalid_proof(),
            Err(_) => return unavailable(),
        };
    if record.account != binding.principal.to_hex() || Some(record.session) != binding.session_id {
        return invalid_proof();
    }
    if buzz_auth::key_backup::validate_link_proof(
        &proof,
        &record.account,
        &record.pubkey,
        challenge,
        &record.url,
        nostr::Timestamp::now().as_secs(),
    )
    .is_err()
    {
        return invalid_proof();
    }
    let Some(master) = &state.identity.config().key_backup_master else {
        return unavailable();
    };
    let encrypted = match master.encrypt(binding.principal.as_bytes(), &secret) {
        Ok(encrypted) if hex::encode(encrypted.pubkey) == record.pubkey => encrypted,
        _ => return invalid_proof(),
    };
    let backup = KeyBackupRecord {
        pubkey: encrypted.pubkey,
        version: encrypted.version,
        key_id: encrypted.key_id,
        nonce: encrypted.nonce,
        ciphertext: encrypted.ciphertext,
    };
    match state
        .db
        .initialize_key_backup(&binding.principal, record.session, &backup)
        .await
    {
        Ok(InitializeKeyBackup::Created) => {
            axum::Json(json!({"pubkey": record.pubkey, "version": 1})).into_response()
        }
        Ok(InitializeKeyBackup::BackupExists) => auth_error(
            StatusCode::CONFLICT,
            "backup_exists",
            "this account already has a key backup",
        ),
        Ok(InitializeKeyBackup::PubkeyInUse) => auth_error(
            StatusCode::CONFLICT,
            "key_already_linked",
            "this key is already linked to another account",
        ),
        Ok(InitializeKeyBackup::SessionNotFresh) => reauth_required(),
        Ok(InitializeKeyBackup::AccountConflict) => auth_error(
            StatusCode::CONFLICT,
            "account_mode_conflict",
            "account mode does not support key backup",
        ),
        Err(_) => unavailable(),
    }
}

/// Export only the caller's bound key, after a fresh Google authentication.
pub(super) async fn restore(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authorize(&state, &headers, true).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    if !matches!(super::json_object(&body), Ok(map) if map.is_empty()) {
        return bad_request("restore does not accept account selectors");
    }
    if let Err(response) = rate_limit(
        &state,
        &format!("auth:key-export:{}", binding.principal),
        60,
        5,
    )
    .await
    {
        return response;
    }
    let backup = match state.db.get_key_backup(&binding.principal).await {
        Ok(Some(backup)) => backup,
        Ok(None) => {
            return auth_error(
                StatusCode::NOT_FOUND,
                "backup_missing",
                "no key backup exists",
            )
        }
        Err(_) => return unavailable(),
    };
    let envelope = EncryptedKeyBackup {
        pubkey: backup.pubkey,
        version: backup.version,
        key_id: backup.key_id,
        nonce: backup.nonce,
        ciphertext: backup.ciphertext,
    };
    let Some(master) = &state.identity.config().key_backup_master else {
        return unavailable();
    };
    let secret = match master.decrypt(binding.principal.as_bytes(), &envelope) {
        Ok(secret) => secret,
        Err(_) => return unavailable(),
    };
    axum::Json(RestoredKey {
        secret_key: secret.as_str(),
        pubkey: hex::encode(envelope.pubkey),
        version: envelope.version,
    })
    .into_response()
}
