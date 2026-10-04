//! Session endpoints: refresh rotation, logout, devices, profile, account.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use buzz_db::identity::{ProfileUpdate, RefreshOutcome, RevokeReason};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::oidc::{issue_session_tokens, refresh_cookie};
use super::{
    auth_error, authenticate, authenticate_user, bad_request, internal, json_object, not_found,
    optional_string, rate_limit, unavailable, validate_avatar_url, validate_display_name,
};
use crate::identity::{kv, publish_revocations};
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
        .filter_map(|pair| pair.trim().strip_prefix("buzz_refresh="))
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

/// `POST /auth/refresh` `{refresh}` (or the web `buzz_refresh` cookie).
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
    let token = buzz_auth::TokenSecret::new(refresh_token);
    if token.kind() != Some(buzz_auth::TokenKind::UserRefresh) {
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
    let binding = match authenticate_user(&state, &headers).await {
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
            tracing::info!(principal = %binding.principal, device = %device_id, "auth.logout");
            (StatusCode::NO_CONTENT, clear_cookie()).into_response()
        }
        Ok(None) => not_found("device not found"),
        Err(error) => internal("logout", &error),
    }
}

/// `GET /auth/devices`.
pub(super) async fn list_devices(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    match state.db.list_devices(&binding.principal).await {
        Ok(devices) => axum::Json(
            devices
                .into_iter()
                .map(|device| {
                    json!({
                        "id": device.id,
                        "name": device.name,
                        "platform": device.platform,
                        "last_seen_at": device.last_seen_at,
                        "current": Some(device.id) == binding.device_id,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => internal("list_devices", &error),
    }
}

/// `DELETE /auth/devices/{id}` — remote logout.
pub(super) async fn revoke_device(
    State(state): State<Arc<AppState>>,
    Path(device_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
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
    let binding = match authenticate_user(&state, &headers).await {
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
        Ok(revoked) => {
            publish_revocations(&state, &revoked, RevokeReason::RevokeAll.as_str(), None).await;
            tracing::info!(principal = %binding.principal, "auth.sessions_revoke_others");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => internal("revoke_other_sessions", &error),
    }
}

/// `GET /auth/me` — any access token.
pub(super) async fn me(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let binding = match authenticate(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let principal = match state.db.get_principal(&binding.principal).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return not_found("principal not found"),
        Err(error) => return internal("get_principal", &error),
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
        "kind": principal.kind.as_str(),
        "display_name": principal.display_name,
        "avatar_url": principal.avatar_url,
        "username": principal.username,
        "device_id": binding.device_id,
        "bot": bot,
        "operator": binding.is_operator,
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

/// Parse and validate a profile update body.
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
    Ok(ProfileUpdate {
        display_name,
        avatar_url,
        username,
    })
}

fn profile_json(record: &buzz_db::identity::PrincipalRecord) -> Response {
    axum::Json(json!({
        "principal_id": record.id.to_hex(),
        "display_name": record.display_name,
        "avatar_url": record.avatar_url,
        "username": record.username,
    }))
    .into_response()
}

/// `PATCH /auth/profile` — global profile (projected into every community's
/// `users` row in the same transaction).
pub(super) async fn update_profile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let update = match parse_profile_update(&body, true) {
        Ok(update) => update,
        Err(response) => return response,
    };
    match state
        .db
        .update_principal_profile(&binding.principal, &update)
        .await
    {
        Ok(Some(record)) => {
            crate::identity::profile::spawn_publish_everywhere(&state, record.id);
            profile_json(&record)
        }
        Ok(None) => not_found("principal not found"),
        Err(buzz_db::DbError::AccessDenied(message)) => {
            auth_error(StatusCode::CONFLICT, "username_taken", &message)
        }
        Err(error) => internal("update_profile", &error),
    }
}

/// Shared by bot profile updates.
pub(super) fn profile_response(record: &buzz_db::identity::PrincipalRecord) -> Response {
    profile_json(record)
}

/// `DELETE /auth/account` — disable now, purge after 30 days; logging in
/// again inside the window restores the account.
pub(super) async fn delete_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
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
            tracing::info!(principal = %binding.principal, "auth.account_deleted");
            (StatusCode::NO_CONTENT, clear_cookie()).into_response()
        }
        Err(error) => internal("disable_principal", &error),
    }
}
