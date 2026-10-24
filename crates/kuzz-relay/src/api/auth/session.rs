//! Session endpoints: refresh rotation, logout, devices, profile, account.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use kuzz_auth::device_proof::{rotate_binding, DevicePubkey};
use kuzz_db::identity::{
    DeviceKey, DeviceKeyRotation, ProfileUpdate, RefreshOutcome, RevokeReason,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::oidc::{issue_session_tokens, refresh_cookie};
use super::{
    auth_error, authenticate_session, authenticate_user_session, authenticate_user_token,
    bad_request, internal, json_object, not_found, optional_string, rate_limit, unavailable,
    validate_avatar_url, validate_device_name, validate_display_name,
};
use crate::identity::{client_version, kv, publish_key_revocation, publish_revocations};
use crate::state::AppState;

/// Refresh replay cache lifetime (plan §3.6): a retry of the same refresh
/// token within this window gets the same rotation result.
pub(super) const REFRESH_REPLAY_TTL_SECS: u64 = 10;

/// Rotation result cached for retries (plaintext, 10 s TTL — plan §6.5).
#[derive(Serialize, Deserialize)]
struct CachedRotation {
    access: String,
    refresh: String,
    expires_in: u64,
}

fn cookie_refresh(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().strip_prefix("kuzz_refresh="))
        .map(str::to_owned)
        .find(|value| !value.is_empty())
}

fn rotation_response(rotation: &CachedRotation, web: bool, refresh_ttl: u64) -> Response {
    if web {
        return (
            StatusCode::OK,
            [(
                header::SET_COOKIE,
                refresh_cookie(&rotation.refresh, refresh_ttl),
            )],
            axum::Json(json!({
                "access": rotation.access,
                "expires_in": rotation.expires_in,
            })),
        )
            .into_response();
    }
    axum::Json(json!({
        "access": rotation.access,
        "refresh": rotation.refresh,
        "expires_in": rotation.expires_in,
    }))
    .into_response()
}

async fn cached_rotation(
    state: &AppState,
    old_hash: &str,
) -> Result<Option<CachedRotation>, Response> {
    match kv::get(&state.redis_pool, "refresh:replay", old_hash).await {
        Ok(cached) => Ok(cached.and_then(|json| serde_json::from_str(&json).ok())),
        Err(error) => {
            tracing::warn!(%error, "refresh replay cache unavailable");
            Err(unavailable())
        }
    }
}

/// The minimum-version gate for a refresh. A request that names no app is
/// judged by its device's client kind, looked up only when a desktop or
/// mobile minimum could refuse it; the web cookie path is the web app.
async fn refresh_version_gate(
    state: &AppState,
    headers: &HeaderMap,
    old_hash: &[u8; 32],
    web: bool,
) -> Result<(), Response> {
    let Some(manifest) = crate::api::app_update::load().await else {
        return Ok(());
    };
    let declared = client_version::declared(headers);
    let client = if web {
        Some("web".to_owned())
    } else if client_version::names_app(declared)
        || !(client_version::gates_unnamed(&manifest, "desktop")
            || client_version::gates_unnamed(&manifest, "mobile"))
    {
        None
    } else {
        match state.db.refresh_device_platform(old_hash).await {
            Ok(platform) => platform,
            Err(error) => return Err(internal("refresh_device_platform", &error)),
        }
    };
    client_version::check(&manifest, declared, client.as_deref())
        .map_err(|min| client_version::update_required_response(&min))
}

/// `POST /auth/refresh` `{refresh}` (or the web `kuzz_refresh` cookie).
///
/// Order (plan §3.6): replay cache → DB rotation. A consumed token presented
/// again after the replay window revokes the whole session (`refresh_reused`).
pub(super) async fn refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Rate limited per refresh token only: a per-IP bucket collapses to one
    // deployment-wide bucket behind a proxy or on the UDS listener, where
    // every session's hourly refresh would share it.
    let body = match json_object(&body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let (refresh_token, web) = match body.get("refresh").and_then(|v| v.as_str()) {
        Some(token) => (token.to_owned(), false),
        None => match cookie_refresh(&headers) {
            Some(token) => (token, true),
            None => return bad_request("refresh is required"),
        },
    };
    let token = kuzz_auth::TokenSecret::new(refresh_token);
    if token.kind() != Some(kuzz_auth::TokenKind::UserRefresh) {
        return auth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "authentication failed",
        );
    }
    let old_hash = token.hash();
    let old_hash_hex = hex::encode(old_hash);
    if let Err(response) = rate_limit(
        &state,
        &format!("auth:refresh:token:{old_hash_hex}"),
        60,
        10,
    )
    .await
    {
        return response;
    }
    if let Err(response) =
        crate::identity::device::check_refresh_proof(&state, &headers, &old_hash).await
    {
        return response;
    }
    // Before rotation, so a refused app keeps its refresh token and is let
    // back in once updated.
    if let Err(response) = refresh_version_gate(&state, &headers, &old_hash, web).await {
        return response;
    }
    let refresh_ttl = state.identity.config().refresh_ttl.as_secs();
    match cached_rotation(&state, &old_hash_hex).await {
        Ok(Some(rotation)) => return rotation_response(&rotation, web, refresh_ttl),
        Ok(None) => {}
        Err(response) => return response,
    }

    let (access, refresh, access_issued, refresh_issued) = issue_session_tokens(&state);
    let grace = chrono::Duration::seconds(REFRESH_REPLAY_TTL_SECS as i64);
    let outcome = match state
        .db
        .rotate_refresh_token(&old_hash, refresh_issued, access_issued, grace)
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return internal("rotate_refresh_token", &error),
    };
    match outcome {
        RefreshOutcome::Rotated { session_id, .. } => {
            let rotation = CachedRotation {
                access: access.expose().to_owned(),
                refresh: refresh.expose().to_owned(),
                expires_in: state.identity.config().access_ttl.as_secs(),
            };
            if let Ok(json) = serde_json::to_string(&rotation) {
                if let Err(error) = kv::put_replay(
                    &state.redis_pool,
                    "refresh:replay",
                    &old_hash_hex,
                    &json,
                    REFRESH_REPLAY_TTL_SECS,
                )
                .await
                {
                    tracing::warn!(%error, "refresh replay cache write failed");
                }
            }
            tracing::debug!(%session_id, "auth.refresh");
            rotation_response(&rotation, web, refresh_ttl)
        }
        RefreshOutcome::RecentlyRotated => {
            // A retry racing its own rotation: the winner writes the cache
            // right after commit; wait for it briefly.
            for _ in 0..20 {
                match cached_rotation(&state, &old_hash_hex).await {
                    Ok(Some(rotation)) => return rotation_response(&rotation, web, refresh_ttl),
                    Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
                    Err(response) => return response,
                }
            }
            // The rotation committed but its result never reached the cache
            // (writer still running, or its write failed). Not a credential
            // failure: tell the client to retry rather than log out.
            tracing::warn!("auth.refresh_rotation_unrecoverable_from_cache");
            unavailable()
        }
        RefreshOutcome::NotFound => auth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "authentication failed",
        ),
        RefreshOutcome::Reused {
            session_id,
            revoked,
        } => {
            tracing::warn!(%session_id, "auth.refresh_reuse_detected");
            publish_revocations(&state, &revoked, RevokeReason::RefreshReused.as_str(), None).await;
            auth_error(
                StatusCode::UNAUTHORIZED,
                "refresh_reused",
                "refresh token reuse detected",
            )
        }
        RefreshOutcome::Expired => auth_error(
            StatusCode::UNAUTHORIZED,
            "token_expired",
            "refresh token expired",
        ),
        RefreshOutcome::SessionRevoked => {
            auth_error(StatusCode::UNAUTHORIZED, "token_revoked", "session revoked")
        }
        RefreshOutcome::PrincipalDisabled => auth_error(
            StatusCode::UNAUTHORIZED,
            "principal_disabled",
            "account disabled",
        ),
    }
}

fn clear_cookie() -> [(header::HeaderName, String); 1] {
    [(header::SET_COOKIE, refresh_cookie("", 0))]
}

/// `POST /auth/logout` — revoke this device, its sessions and hosted bots.
pub(super) async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let binding = match authenticate_user_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let Some(device_id) = binding.device_id else {
        return internal("logout", &"user token without device");
    };
    match state
        .db
        .revoke_device(&binding.principal, device_id, RevokeReason::Logout)
        .await
    {
        Ok(Some(revoked)) => {
            publish_revocations(&state, &revoked, RevokeReason::Logout.as_str(), None).await;
            // Key sockets signed in from this device close too (A4b).
            publish_key_revocation(&state, &[device_id], &[], RevokeReason::Logout.as_str()).await;
            tracing::info!(principal = %binding.principal, device = %device_id, "auth.logout");
            (StatusCode::NO_CONTENT, clear_cookie()).into_response()
        }
        Ok(None) => not_found("device not found"),
        Err(error) => internal("logout", &error),
    }
}

/// `POST /auth/sessions/end` — end only the calling session (its tokens),
/// keeping the device, its key and its other sessions. Apps use this to clean
/// up a sign-in that was replaced or never stored; `/auth/logout` would sign
/// the whole device out, including the session that replaced it.
pub(super) async fn end_session(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let Some(session_id) = binding.session_id else {
        return internal("end_session", &"user token without session");
    };
    match state
        .db
        .revoke_session(&binding.principal, session_id, RevokeReason::Logout)
        .await
    {
        Ok(Some(revoked)) => {
            publish_revocations(&state, &revoked, RevokeReason::Logout.as_str(), None).await;
            tracing::info!(principal = %binding.principal, session = %session_id, "auth.session_end");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => not_found("session not found"),
        Err(error) => internal("end_session", &error),
    }
}

fn device_json(
    device: &kuzz_db::identity::DeviceRecord,
    current: Option<Uuid>,
) -> serde_json::Value {
    json!({
        "id": device.id,
        "name": device.name,
        "platform": device.platform,
        "last_seen_at": device.last_seen_at,
        "current": Some(device.id) == current,
    })
}

/// `GET /auth/devices`.
pub(super) async fn list_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    match state.db.list_devices(&binding.principal).await {
        Ok(devices) => axum::Json(
            devices
                .iter()
                .map(|device| device_json(device, binding.device_id))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => internal("list_devices", &error),
    }
}

/// `PATCH /auth/devices/{id}` — `{"name": "..."}`; any of the caller's live
/// devices, not only the current one.
pub(super) async fn rename_device(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authenticate_user_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let body = match json_object(&body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let name = match body.get("name").and_then(serde_json::Value::as_str) {
        Some(name) => name.trim(),
        None => return bad_request("name must be a string"),
    };
    if let Err(response) = validate_device_name(name) {
        return response;
    }
    match state
        .db
        .rename_device(&binding.principal, device_id, name)
        .await
    {
        Ok(Some(device)) => axum::Json(device_json(&device, binding.device_id)).into_response(),
        Ok(None) => not_found("device not found"),
        Err(error) => internal("rename_device", &error),
    }
}

/// `DELETE /auth/devices/{id}` — remote logout.
pub(super) async fn revoke_device(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    match state
        .db
        .revoke_device(&binding.principal, device_id, RevokeReason::DeviceRevoked)
        .await
    {
        Ok(Some(revoked)) => {
            publish_revocations(&state, &revoked, RevokeReason::DeviceRevoked.as_str(), None).await;
            publish_key_revocation(
                &state,
                &[device_id],
                &[],
                RevokeReason::DeviceRevoked.as_str(),
            )
            .await;
            tracing::info!(principal = %binding.principal, device = %device_id, "auth.device_revoked");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => not_found("device not found"),
        Err(error) => internal("revoke_device", &error),
    }
}

/// `POST /auth/sessions/revoke-others` — human sessions only (bots untouched).
pub(super) async fn revoke_others(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let Some(session_id) = binding.session_id else {
        return internal("revoke_others", &"user token without session");
    };
    match state
        .db
        .revoke_other_sessions(&binding.principal, session_id)
        .await
    {
        Ok((revoked, devices)) => {
            publish_revocations(&state, &revoked, RevokeReason::RevokeAll.as_str(), None).await;
            publish_key_revocation(&state, &devices, &[], RevokeReason::RevokeAll.as_str()).await;
            tracing::info!(principal = %binding.principal, "auth.sessions_revoke_others");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => internal("revoke_other_sessions", &error),
    }
}

/// `POST /auth/device/rotate` `{device_pubkey, device_key_kind, proof}`:
/// move this session's device from its current key to a new hardware key
/// (a software key upgraded to the TPM or Secure Enclave). The request is
/// proven by the current key in every auth surface mode, and `proof` by the
/// new key (purpose `rotate` over [`rotate_binding`] and the access token
/// hash). Sessions and agents stay bound to the device, so they move with
/// it; no one signs in again.
pub(super) async fn rotate_device_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authenticate_user_token(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    // A stored key that no longer parses counts as keyless (fails closed).
    let old = binding
        .device_pubkey
        .and_then(|key| DevicePubkey::from_sec1(&key).ok());
    let (device_id, old) = match (binding.device_id, old) {
        (Some(device), Some(old)) => (device, old),
        _ => return conflict("device_key_required", "this device has no device key"),
    };
    if let Err(rejection) = crate::identity::device::check_request_proof_always(
        &state,
        &old,
        &headers,
        &binding.token_hash,
    )
    .await
    {
        return super::rejection_response(rejection);
    }
    if let Err(response) = rate_limit(
        &state,
        &format!("device-rotate:{}", binding.principal),
        60,
        10,
    )
    .await
    {
        return response;
    }
    let body = match json_object(&body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(new) = body
        .get("device_pubkey")
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| DevicePubkey::from_hex(raw).ok())
    else {
        return bad_request("device_pubkey must be a compressed P-256 key in hex");
    };
    if body
        .get("device_key_kind")
        .and_then(serde_json::Value::as_str)
        != Some("hardware")
    {
        return bad_request("device_key_kind must be hardware");
    }
    if new == old {
        return conflict("device_key_unchanged", "the new key is the current key");
    }
    let Some(proof) = body.get("proof").and_then(serde_json::Value::as_str) else {
        return bad_request("proof is required");
    };
    let message = rotate_binding(&device_id.to_string(), &old, &new);
    if let Err(reason) = crate::identity::device::check_rotation_proof(
        &state,
        &new,
        proof,
        &message,
        &binding.token_hash,
    )
    .await
    {
        tracing::info!(principal = %binding.principal, device = %device_id, reason, "auth.device_rotate_refused");
        return auth_error(
            StatusCode::FORBIDDEN,
            "device_proof_invalid",
            "the new key did not prove the rotation",
        );
    }
    let new_key = DeviceKey {
        pubkey: new.as_bytes(),
        kind: "hardware",
    };
    match state
        .db
        .rotate_device_key(&binding.principal, device_id, old.as_bytes(), new_key)
        .await
    {
        Ok(DeviceKeyRotation::Rotated) => {
            // Cached admissions of the old key go now; cached refusals of
            // the new key (a race with the client switching) too.
            state.identity.admissions.revoke(&[device_id], &[]);
            crate::identity::admission::forget_login(&state, None, Some(*new.as_bytes()));
            tracing::info!(
                principal = %binding.principal,
                device = %device_id,
                old = %&old.to_hex()[..16],
                new = %&new.to_hex()[..16],
                "auth.device_rotate"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(DeviceKeyRotation::NotFound) => not_found("device not found"),
        Ok(DeviceKeyRotation::Keyless) => {
            conflict("device_key_required", "this device has no device key")
        }
        Ok(DeviceKeyRotation::Changed) => {
            conflict("device_key_changed", "the device key changed meanwhile")
        }
        Ok(DeviceKeyRotation::InUse) => {
            conflict("device_key_in_use", "another device holds that key")
        }
        Err(error) => internal("rotate_device_key", &error),
    }
}

fn conflict(code: &str, message: &str) -> Response {
    auth_error(StatusCode::CONFLICT, code, message)
}

/// `GET /auth/me` — any access token.
pub(super) async fn me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let binding = match authenticate_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let principal = match state.db.get_principal(&binding.principal).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return not_found("principal not found"),
        Err(error) => return internal("get_principal", &error),
    };
    let key_mode = match state.db.account_key_mode(&binding.principal).await {
        Ok(mode) => mode,
        Err(_) => return unavailable(),
    };
    let signing_pubkey = if key_mode {
        match state.db.get_key_backup(&binding.principal).await {
            Ok(backup) => backup.map(|backup| hex::encode(backup.pubkey)),
            Err(_) => return unavailable(),
        }
    } else {
        None
    };
    // The Google email of a person's session; bot tokens have none.
    let email = if binding.bot_id.is_none() {
        match state.db.principal_email(&binding.principal).await {
            Ok(email) => email,
            Err(_) => return unavailable(),
        }
    } else {
        None
    };
    let bot = match binding.bot_id {
        Some(bot_id) => match state.db.get_bot(&bot_id).await {
            Ok(Some(bot)) => Some(json!({
                "owner": bot.owner.to_hex(),
                "host": bot.host_device_id,
            })),
            Ok(None) => None,
            Err(error) => return internal("get_bot", &error),
        },
        None => None,
    };
    axum::Json(json!({
        "principal_id": principal.id.to_hex(),
        "identity_mode": if key_mode { "key_backup" } else { "token" },
        "signing_pubkey": signing_pubkey,
        "kind": principal.kind.as_str(),
        "display_name": principal.display_name,
        "avatar_url": principal.avatar_url,
        "username": principal.username,
        "discoverable": principal.discoverable,
        "locale": principal.locale,
        "email": email,
        "device_id": binding.device_id,
        "bot": bot,
        "operator": binding.is_operator,
        // Always 1 until message keys can be replaced; clients compare it.
        "key_generation": 1,
    }))
    .into_response()
}

fn validate_username(username: &str) -> Result<(), Response> {
    let ok = (3..=32).contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !ok {
        return Err(bad_request("username must be 3-32 of [a-z0-9_]"));
    }
    Ok(())
}

/// A BCP 47 tag as the apps write it: `en`, `ko`, `zh-Hant`, `pt-BR`, `fil`.
/// Mirrors the `principals_locale` CHECK.
fn validate_locale(locale: &str) -> Result<(), Response> {
    let mut parts = locale.split('-');
    let language_ok = parts
        .next()
        .is_some_and(|l| (2..=3).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_lowercase()));
    let rest: Vec<&str> = parts.collect();
    let rest_ok = rest.len() <= 2
        && rest
            .iter()
            .all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    if !(language_ok && rest_ok) {
        return Err(bad_request(
            "locale must be a language tag like en, ko or zh-Hant",
        ));
    }
    Ok(())
}

/// Parse and validate a profile update body. `allow_username` also gates
/// `discoverable` and `locale` (person accounts only; bots have none).
pub(super) fn parse_profile_update(
    body: &Bytes,
    allow_username: bool,
) -> Result<ProfileUpdate, Response> {
    let body = json_object(body)?;
    let display_name = match optional_string(&body, "display_name")? {
        None => None,
        Some(Some(name)) => {
            validate_display_name(&name)?;
            Some(name)
        }
        Some(None) => return Err(bad_request("display_name cannot be null")),
    };
    let avatar_url = optional_string(&body, "avatar_url")?;
    if let Some(Some(url)) = &avatar_url {
        validate_avatar_url(url)?;
    }
    let username = if allow_username {
        let username = optional_string(&body, "username")?;
        if let Some(Some(name)) = &username {
            validate_username(name)?;
        }
        username
    } else if body.contains_key("username") {
        return Err(bad_request("username cannot be set here"));
    } else {
        None
    };
    let discoverable = match body.get("discoverable") {
        None => None,
        Some(_) if !allow_username => {
            return Err(bad_request("discoverable cannot be set here"));
        }
        Some(serde_json::Value::Bool(value)) => Some(*value),
        Some(_) => return Err(bad_request("discoverable must be a boolean")),
    };
    let locale = if allow_username {
        let locale = optional_string(&body, "locale")?;
        if let Some(Some(tag)) = &locale {
            validate_locale(tag)?;
        }
        locale
    } else if body.contains_key("locale") {
        return Err(bad_request("locale cannot be set here"));
    } else {
        None
    };
    Ok(ProfileUpdate {
        display_name,
        avatar_url,
        username,
        discoverable,
        locale,
    })
}

fn profile_json(record: &kuzz_db::identity::PrincipalRecord) -> Response {
    axum::Json(json!({
        "principal_id": record.id.to_hex(),
        "display_name": record.display_name,
        "avatar_url": record.avatar_url,
        "username": record.username,
        "discoverable": record.discoverable,
        "locale": record.locale,
    }))
    .into_response()
}

/// `PATCH|POST /auth/profile` — global profile (projected into every
/// community's `users` row in the same transaction).
///
/// Key-backup accounts sign their own kind:0, so their session may set only
/// account-level fields (`username`, `discoverable`, `locale`); display name and avatar
/// stay with the client-signed profile.
pub(super) async fn update_profile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authenticate_user_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let update = match parse_profile_update(&body, true) {
        Ok(update) => update,
        Err(response) => return response,
    };
    let key_mode = match state.db.account_key_mode(&binding.principal).await {
        Ok(mode) => mode,
        Err(_) => return unavailable(),
    };
    if key_mode && (update.display_name.is_some() || update.avatar_url.is_some()) {
        return super::forbidden(
            "display_name and avatar_url are set by the signed profile for this account",
        );
    }
    match state
        .db
        .update_principal_profile(&binding.principal, &update)
        .await
    {
        Ok(Some(record)) => {
            // The language is account-only; it is not part of the public profile.
            let locale_only = update.locale.is_some()
                && update.display_name.is_none()
                && update.avatar_url.is_none()
                && update.username.is_none()
                && update.discoverable.is_none();
            if !key_mode && !locale_only {
                crate::identity::profile::spawn_publish_everywhere(&state, record.id);
            }
            profile_json(&record)
        }
        Ok(None) => not_found("principal not found"),
        Err(kuzz_db::DbError::AccessDenied(message)) => {
            auth_error(StatusCode::CONFLICT, "username_taken", &message)
        }
        Err(error) => internal("update_profile", &error),
    }
}

/// Close the open sockets of a Google key account and of its agents in
/// every community this relay serves. Best effort: the refusal at the
/// next AUTH is the durable backstop.
async fn close_key_account_sockets(
    state: &AppState,
    principal: &kuzz_core::principal::PrincipalId,
) {
    let pubkey = match state.db.account_pubkey(principal).await {
        Ok(Some(pubkey)) => pubkey,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, "account key lookup failed; open sockets close at their next AUTH");
            return;
        }
    };
    let communities = match state.db.usage_community_hosts().await {
        Ok(communities) => communities,
        Err(error) => {
            tracing::warn!(%error, "community scan failed; open sockets close at their next AUTH");
            return;
        }
    };
    for community in communities {
        let tenant = kuzz_core::TenantContext::resolved(
            kuzz_core::CommunityId::from_uuid(community.id),
            community.host,
        );
        if let Err(error) = state
            .revoke_live_access(&tenant, &pubkey, "", "account deleted")
            .await
        {
            tracing::warn!(%error, "closing a deleted account's agent sockets failed");
        }
    }
}

/// Shared by bot profile updates.
pub(super) fn profile_response(record: &kuzz_db::identity::PrincipalRecord) -> Response {
    profile_json(record)
}

/// `DELETE /auth/account` — disable now, purge after 30 days; logging in
/// again inside the window restores the account. Any account session may
/// ask, a Google key account's included: its key and its agents' NIP-OA
/// tags are refused from then on ([`crate::handlers::auth::community_ban_outcome`])
/// and their open sockets are closed here.
pub(super) async fn delete_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user_session(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    match state
        .db
        .disable_principal(&binding.principal, Some(chrono::Duration::days(30)))
        .await
    {
        Ok(revoked) => {
            publish_revocations(
                &state,
                &revoked,
                RevokeReason::AccountDisabled.as_str(),
                None,
            )
            .await;
            // Every device of the account (rows stay; the account is
            // disabled): their key sockets close as `restricted: revoked`.
            // No read at all while key sockets are off.
            let devices = if crate::identity::key_sockets::enabled(&state) {
                Some(state.db.list_devices(&binding.principal).await)
            } else {
                None
            };
            match devices {
                None => {}
                Some(Ok(devices)) => {
                    let ids: Vec<Uuid> = devices.iter().map(|device| device.id).collect();
                    publish_key_revocation(
                        &state,
                        &ids,
                        &[],
                        RevokeReason::AccountDisabled.as_str(),
                    )
                    .await;
                }
                Some(Err(error)) => {
                    tracing::warn!(%error, "account devices unreadable; key socket recheck backstops");
                }
            }
            close_key_account_sockets(&state, &binding.principal).await;
            tracing::info!(principal = %binding.principal, "auth.account_deleted");
            (StatusCode::NO_CONTENT, clear_cookie()).into_response()
        }
        Err(error) => internal("disable_principal", &error),
    }
}
