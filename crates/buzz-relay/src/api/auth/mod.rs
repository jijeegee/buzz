//! `/auth/*` — centralized-identity HTTP surface (plan §3.2–3.3).
//!
//! Authentication itself cannot be a Nostr event (an event needs auth
//! first), and OIDC needs browser redirects, so this is an HTTP-only surface
//! like health and NIP-11. The router is mounted only when
//! `AUTH_TOKEN_ENABLED=true`; otherwise `/auth/*` is unrouted and answers
//! exactly like any unknown path (403 from the fallback).
//!
//! Errors are `{"error": <message>, "code": <code>}`; 401 codes are
//! `invalid_token | token_expired | token_revoked | refresh_reused |
//! principal_disabled`, plus 409 `token_superseded`.

// Handlers return axum's `Response` as the error type (as `nip_fi_http.rs`
// does); boxing it would only add an allocation per rejection.
#![allow(clippy::result_large_err)]

mod bots;
mod oidc;
mod operators;
mod session;

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod router_tests;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    extract::ConnectInfo,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Router,
};
use buzz_auth::{TokenBinding, TokenSecret};
use buzz_core::principal::AccessTokenKind;
use serde_json::{json, Value};
use tower_http::limit::RequestBodyLimitLayer;

use crate::identity::{verify_access_token, TokenRejection};
use crate::state::AppState;

/// Bound on `/auth/*` request bodies.
const AUTH_BODY_LIMIT: usize = 16 * 1024;

/// Build the `/auth/*` router. Callers mount it only when token auth is on.
pub(crate) fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/auth/oidc/{provider}/start", get(oidc::start))
        .route("/auth/oidc/{provider}/callback", get(oidc::callback))
        .route("/auth/oidc/complete", post(oidc::complete))
        .route("/auth/refresh", post(session::refresh))
        .route("/auth/logout", post(session::logout))
        .route("/auth/devices", get(session::list_devices))
        .route("/auth/devices/{id}", delete(session::revoke_device))
        .route("/auth/sessions/revoke-others", post(session::revoke_others))
        .route("/auth/me", get(session::me))
        .route("/auth/profile", patch(session::update_profile))
        .route("/auth/account", delete(session::delete_account))
        .route("/auth/bots", post(bots::create))
        .route("/auth/bots/revoke-all", post(bots::revoke_all))
        .route("/auth/bots/{id}", delete(bots::delete_bot))
        .route("/auth/bots/{id}/profile", patch(bots::update_profile))
        .route("/auth/bots/{id}/token", post(bots::issue_token))
        .route("/auth/bots/{id}/revoke", post(bots::stop))
        .route("/auth/bots/{id}/headless-token", post(bots::issue_headless))
        .route(
            "/auth/bots/{id}/headless-token/{hash_prefix}",
            delete(bots::revoke_headless),
        )
        .route("/auth/token/exchange", post(bots::exchange))
        .route("/auth/operators", get(operators::list))
        .route(
            "/auth/operators/{principal}",
            put(operators::grant).delete(operators::revoke),
        )
        .layer(RequestBodyLimitLayer::new(AUTH_BODY_LIMIT))
        .with_state(state)
}

/// `{"error", "code"}` response.
pub(crate) fn auth_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        axum::Json(json!({ "error": message, "code": code })),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    auth_error(StatusCode::BAD_REQUEST, "invalid_request", message)
}

fn forbidden(message: &str) -> Response {
    auth_error(StatusCode::FORBIDDEN, "forbidden", message)
}

fn not_found(message: &str) -> Response {
    auth_error(StatusCode::NOT_FOUND, "not_found", message)
}

fn unavailable() -> Response {
    auth_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
        "authentication backend unavailable",
    )
}

fn internal(context: &str, error: &dyn std::fmt::Display) -> Response {
    tracing::error!(%error, "{context}");
    auth_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "internal server error",
    )
}

/// 401 for a rejected token (503 when the store is unavailable).
pub(crate) fn rejection_response(rejection: TokenRejection) -> Response {
    match rejection {
        TokenRejection::Unavailable => unavailable(),
        other => auth_error(
            StatusCode::UNAUTHORIZED,
            other.code(),
            "authentication failed",
        ),
    }
}

/// The token from `Authorization: Bearer <token>`, if present. The scheme
/// name is case-insensitive (RFC 7235 §2.1).
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<TokenSecret> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| TokenSecret::new(token.to_owned()))
}

/// Authenticate the request's bearer token.
pub(crate) async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<TokenBinding, Response> {
    let token = bearer_token(headers).ok_or_else(|| {
        auth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "missing bearer token",
        )
    })?;
    verify_access_token(state, &token)
        .await
        .map_err(rejection_response)
}

/// Authenticate and require a human (user session) token.
async fn authenticate_user(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<TokenBinding, Response> {
    let binding = authenticate(state, headers).await?;
    if binding.kind != AccessTokenKind::User {
        return Err(forbidden("a user session token is required"));
    }
    Ok(binding)
}

/// The client IP: the peer when the listener provides connect info (a UDS
/// listener does not), or — when the peer is a trusted reverse proxy
/// (`AUTH_TRUSTED_PROXY_CIDRS`) — the client its forwarding headers name.
fn client_ip(
    state: &AppState,
    extensions: &axum::http::Extensions,
    headers: &HeaderMap,
) -> Option<String> {
    let peer = extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    state
        .identity
        .config()
        .trusted_proxies
        .client_ip(peer, headers)
        .map(|ip| ip.to_string())
}

/// Per-client limit for an unauthenticated endpoint: a per-IP bucket when the
/// peer IP is known, otherwise one deployment-wide bucket sized for all
/// clients together (a per-IP limit there would let one client lock out
/// everyone).
async fn rate_limit_client(
    state: &AppState,
    extensions: &axum::http::Extensions,
    headers: &HeaderMap,
    scope: &str,
    per_ip_per_min: u64,
    global_per_min: u64,
) -> Result<(), Response> {
    match client_ip(state, extensions, headers) {
        Some(ip) => rate_limit(state, &format!("{scope}:{ip}"), 60, per_ip_per_min).await,
        None => rate_limit(state, &format!("{scope}:global"), 60, global_per_min).await,
    }
}

/// Shared Redis fixed-window limit. Fails closed when Redis is unavailable.
async fn rate_limit(
    state: &AppState,
    key: &str,
    window_secs: u64,
    limit: u64,
) -> Result<(), Response> {
    match state
        .admission_rate_limiter
        .check_key(key, window_secs, limit)
        .await
    {
        Ok(result) if result.allowed => Ok(()),
        Ok(result) => Err(auth_error(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            &format!("rate limited; retry in {}s", result.reset_in_secs),
        )),
        Err(error) => {
            tracing::warn!(%error, "auth rate limiter unavailable; denying");
            Err(unavailable())
        }
    }
}

/// Parse a JSON object body (empty body ⇒ `{}`).
fn json_object(body: &[u8]) -> Result<serde_json::Map<String, Value>, Response> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(bad_request("body must be a JSON object")),
    }
}

/// Optional string field: absent ⇒ `None`, `null` ⇒ `Some(None)`.
fn optional_string(
    body: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<Option<String>>, Response> {
    match body.get(field) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(value)) => Ok(Some(Some(value.clone()))),
        Some(_) => Err(bad_request(&format!("{field} must be a string or null"))),
    }
}

fn validate_display_name(name: &str) -> Result<(), Response> {
    let len = name.chars().count();
    if len == 0 || len > 64 || name.chars().any(char::is_control) {
        return Err(bad_request(
            "display_name must be 1-64 printable characters",
        ));
    }
    Ok(())
}

fn validate_avatar_url(url: &str) -> Result<(), Response> {
    let ok = url.len() <= 2048
        && url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "https" | "http"));
    if !ok {
        return Err(bad_request("avatar_url must be an http(s) URL"));
    }
    Ok(())
}
