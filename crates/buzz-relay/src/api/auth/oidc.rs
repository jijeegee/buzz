//! OIDC login: `start` → provider → `callback` → client → `complete`
//! (plan §3.2). The relay is the relying party; the client proves with PKCE
//! that it started the login it completes.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{header, Extensions, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use buzz_auth::token::{generate_token, verify_pkce, TokenKind};
use buzz_core::principal::PrincipalId;
use buzz_db::identity::{IssuedToken, RevokeReason};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{auth_error, bad_request, internal, json_object, rate_limit_client, unavailable};
use crate::identity::{kv, publish_revocations};
use crate::state::AppState;

/// Pending-login lifetime (`state` → callback).
const PENDING_TTL_SECS: u64 = 600;
/// One-time login code lifetime (callback → complete).
const LOGIN_CODE_TTL_SECS: u64 = 60;
/// Bound on the `state` value echoed through the provider.
const MAX_STATE_LEN: usize = 256;
/// Minimum client `state` entropy (characters).
const MIN_STATE_LEN: usize = 16;
/// `start` requests per minute per peer IP.
const LOGIN_START_PER_IP: u64 = 30;
/// `start` requests per minute for the whole deployment when the peer IP is
/// unknown (UDS listener).
const LOGIN_START_GLOBAL: u64 = 600;
/// `complete` requests per minute per peer IP.
const LOGIN_COMPLETE_PER_IP: u64 = 10;
/// `complete` requests per minute for the whole deployment when the peer IP
/// is unknown (UDS listener).
const LOGIN_COMPLETE_GLOBAL: u64 = 300;

/// Pending login stored between `start` and `callback`.
#[derive(Serialize, Deserialize)]
struct PendingLogin {
    #[serde(default = "token_identity_mode")]
    identity_mode: String,
    provider: String,
    code_challenge: String,
    client: String,
    redirect_uri: String,
    device_name: String,
    /// The client's stable per-install id; see [`install_id_from_query`].
    #[serde(default)]
    install_id: Option<String>,
    nonce: String,
}

/// One-time login code record stored between `callback` and `complete`.
#[derive(Serialize, Deserialize)]
struct LoginCodeRecord {
    #[serde(default = "token_identity_mode")]
    identity_mode: String,
    principal: String,
    code_challenge: String,
    client: String,
    device_name: String,
    #[serde(default)]
    install_id: Option<String>,
}

#[derive(Deserialize, Default)]
struct StartQuery {
    identity_mode: Option<String>,
    state: Option<String>,
    code_challenge: Option<String>,
    client: Option<String>,
    redirect_uri: Option<String>,
    device_name: Option<String>,
    install_id: Option<String>,
}

fn token_identity_mode() -> String {
    "token".into()
}

/// A client's stable per-install id (random, kept for the life of the app
/// install): a login that carries one the principal already used reuses that
/// device. Absent or empty means "new device" (older clients).
fn install_id_from_query(value: Option<String>) -> Result<Option<String>, ()> {
    let Some(value) = value.map(|value| value.trim().to_ascii_lowercase()) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    let valid = (16..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if valid {
        Ok(Some(value))
    } else {
        Err(())
    }
}

#[derive(Deserialize, Default)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

fn is_url_safe(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// Whether `redirect_uri` is allowed for `client` (plan §3.2 table).
pub(super) fn redirect_allowed(state: &AppState, client: &str, redirect_uri: &str) -> bool {
    let config = state.identity.config();
    match client {
        "desktop" | "cli" => url::Url::parse(redirect_uri).is_ok_and(|url| {
            url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some()
                && url.path() == "/cb"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
        }),
        "mobile" => config
            .mobile_redirect_schemes
            .iter()
            .any(|scheme| redirect_uri == format!("{scheme}://auth/cb")),
        "web" => redirect_uri == format!("{}/auth/cb", config.public_url),
        _ => false,
    }
}

fn redirect(location: &str) -> Response {
    (StatusCode::FOUND, [(header::LOCATION, location.to_owned())]).into_response()
}

fn client_redirect(redirect_uri: &str, params: &[(&str, &str)]) -> Response {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        query.append_pair(key, value);
    }
    redirect(&format!("{redirect_uri}?{}", query.finish()))
}

/// `GET /auth/oidc/{provider}/start`.
pub(super) async fn start(
    State(state): State<Arc<AppState>>,
    Path(provider_name): Path<String>,
    extensions: Extensions,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Response {
    if let Err(response) = rate_limit_client(
        &state,
        &extensions,
        &headers,
        "auth:login:start",
        LOGIN_START_PER_IP,
        LOGIN_START_GLOBAL,
    )
    .await
    {
        return response;
    }
    let Some(provider) = state.identity.provider(&provider_name) else {
        return super::not_found("unknown provider");
    };
    let query: StartQuery = match serde_urlencoded::from_str(raw.as_deref().unwrap_or("")) {
        Ok(query) => query,
        Err(_) => return bad_request("invalid query"),
    };
    let Some(client_state) = query
        .state
        .filter(|s| (MIN_STATE_LEN..=MAX_STATE_LEN).contains(&s.len()) && is_url_safe(s))
    else {
        return bad_request("state is required (16-256 URL-safe characters)");
    };
    let Some(code_challenge) = query
        .code_challenge
        .filter(|c| c.len() == 43 && is_url_safe(c))
    else {
        return bad_request("code_challenge must be an S256 challenge");
    };
    let Some(client) = query
        .client
        .filter(|c| matches!(c.as_str(), "desktop" | "mobile" | "web" | "cli"))
    else {
        return bad_request("client must be desktop, mobile, web or cli");
    };
    let identity_mode = query.identity_mode.unwrap_or_else(|| {
        if state.identity.enabled() {
            "token".into()
        } else {
            "key_backup".into()
        }
    });
    match identity_mode.as_str() {
        "key_backup" if state.identity.config().key_backup_master.is_some() => {
            if !matches!(client.as_str(), "desktop" | "mobile") {
                return auth_error(StatusCode::BAD_REQUEST, "unsupported_client", "Google key backup signup and recovery require Buzz desktop or mobile; existing CLI key import remains supported");
            }
        }
        "token" if state.identity.enabled() => {}
        _ => return bad_request("requested identity_mode is not enabled"),
    }
    let Some(redirect_uri) = query
        .redirect_uri
        .filter(|uri| redirect_allowed(&state, &client, uri))
    else {
        return bad_request("redirect_uri is not allowed for this client");
    };
    let device_name = match query.device_name {
        Some(name) if name.chars().count() <= 64 && !name.chars().any(char::is_control) => {
            if name.trim().is_empty() {
                format!("{client} device")
            } else {
                name.trim().to_owned()
            }
        }
        None => format!("{client} device"),
        Some(_) => return bad_request("device_name must be at most 64 printable characters"),
    };
    let Ok(install_id) = install_id_from_query(query.install_id) else {
        return bad_request("install_id must be 16-64 letters, digits or hyphens");
    };
    let nonce = {
        let bytes: [u8; 24] = rand::random();
        hex::encode(bytes)
    };
    let pending = PendingLogin {
        identity_mode,
        provider: provider_name.clone(),
        code_challenge,
        client,
        redirect_uri,
        device_name,
        install_id,
        nonce: nonce.clone(),
    };
    let Ok(pending_json) = serde_json::to_string(&pending) else {
        return unavailable();
    };
    match kv::put_new(
        &state.redis_pool,
        "oidc:pending",
        &client_state,
        &pending_json,
        PENDING_TTL_SECS,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return bad_request("state already in use"),
        Err(error) => {
            tracing::warn!(%error, "OIDC pending store unavailable");
            return unavailable();
        }
    }
    let callback_url = state.identity.config().callback_url(&provider_name);
    redirect(&provider.authorization_url(&callback_url, &client_state, &nonce))
}

/// `GET /auth/oidc/{provider}/callback`.
pub(super) async fn callback(
    State(state): State<Arc<AppState>>,
    Path(provider_name): Path<String>,
    RawQuery(raw): RawQuery,
) -> Response {
    let raw = raw.unwrap_or_default();
    if raw.len() > 8 * 1024 {
        return bad_request("callback query too large");
    }
    let query: CallbackQuery = match serde_urlencoded::from_str(&raw) {
        Ok(query) => query,
        Err(_) => return bad_request("invalid query"),
    };
    let Some(client_state) = query.state.filter(|s| s.len() <= MAX_STATE_LEN) else {
        return bad_request("missing state");
    };
    let pending = match kv::take(&state.redis_pool, "oidc:pending", &client_state).await {
        Ok(Some(json)) => match serde_json::from_str::<PendingLogin>(&json) {
            Ok(pending) => pending,
            Err(_) => return bad_request("unknown or expired login state"),
        },
        Ok(None) => return bad_request("unknown or expired login state"),
        Err(error) => {
            tracing::warn!(%error, "OIDC pending store unavailable");
            return unavailable();
        }
    };
    if pending.provider != provider_name {
        return bad_request("login state belongs to another provider");
    }
    let fail = |error: &str| {
        client_redirect(
            &pending.redirect_uri,
            &[("error", error), ("state", &client_state)],
        )
    };
    if query.error.is_some() {
        return fail("access_denied");
    }
    let (Some(code), Some(provider)) = (query.code, state.identity.provider(&provider_name)) else {
        return fail("login_failed");
    };
    let callback_url = state.identity.config().callback_url(&provider_name);
    let identity = match provider
        .exchange_code(&code, &callback_url, &pending.nonce)
        .await
    {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(%error, provider = %provider_name, "OIDC code exchange failed");
            return fail("login_failed");
        }
    };

    let display_name = identity
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "user".to_owned());
    let name = display_name.chars().take(64).collect::<String>();
    let result = if pending.identity_mode == "key_backup" {
        state
            .db
            .login_key_identity(
                &provider_name,
                &identity.subject,
                identity.email.as_deref(),
                &name,
                identity.picture.as_deref(),
            )
            .await
    } else if state.identity.config().key_backup_master.is_some() {
        // On deployments offering custody, legacy clients can still sign into
        // an existing token account, but cannot silently create a new identity.
        state
            .db
            .login_existing_token_identity(
                &provider_name,
                &identity.subject,
                identity.email.as_deref(),
                &name,
                identity.picture.as_deref(),
            )
            .await
    } else {
        state
            .db
            .login_identity(
                &provider_name,
                &identity.subject,
                identity.email.as_deref(),
                &name,
                identity.picture.as_deref(),
            )
            .await
    };
    let login = match result {
        Ok(login) => login,
        Err(buzz_db::DbError::AccessDenied(message)) if message == "account_mode_conflict" => {
            return fail("account_mode_conflict");
        }
        Err(buzz_db::DbError::AccessDenied(message)) if message == "unsupported_client" => {
            return fail("unsupported_client");
        }
        Err(error) => {
            tracing::error!(%error, "identity login failed");
            return fail("server_error");
        }
    };
    if login.disabled {
        return fail("principal_disabled");
    }
    if login.created {
        tracing::info!(principal = %login.principal, "auth.account_created");
    }
    if pending.identity_mode == "token" {
        bootstrap_operator(&state, &login.principal, &identity).await;
    }

    let (code_token, code_hash) = generate_token(TokenKind::LoginCode);
    let record = LoginCodeRecord {
        identity_mode: pending.identity_mode,
        principal: login.principal.to_hex(),
        code_challenge: pending.code_challenge.clone(),
        client: pending.client.clone(),
        device_name: pending.device_name.clone(),
        install_id: pending.install_id.clone(),
    };
    let Ok(record_json) = serde_json::to_string(&record) else {
        return fail("server_error");
    };
    match kv::put_new(
        &state.redis_pool,
        "login",
        &hex::encode(code_hash),
        &record_json,
        LOGIN_CODE_TTL_SECS,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) | Err(_) => return fail("server_error"),
    }
    client_redirect(
        &pending.redirect_uri,
        &[("code", code_token.expose()), ("state", &client_state)],
    )
}

/// Operator bootstrap (plan §3.3 B8), evaluated on **every** login: verified
/// email matching `RELAY_OPERATOR_BOOTSTRAP_EMAIL` (case-insensitive) while the
/// DB roster has no operator. A failure is logged, never fatal to the login.
pub(super) async fn bootstrap_operator(
    state: &AppState,
    principal: &PrincipalId,
    identity: &buzz_auth::oidc::OidcIdentity,
) {
    let Some(configured) = state.identity.config().operator_bootstrap_email.as_deref() else {
        return;
    };
    let matches = identity.email_verified
        && identity
            .email
            .as_deref()
            .is_some_and(|email| email.eq_ignore_ascii_case(configured));
    if !matches {
        return;
    }
    let Some(relay) = state.identity.relay_principal() else {
        tracing::warn!("operator bootstrap skipped: relay principal not initialized");
        return;
    };
    match state
        .db
        .bootstrap_relay_operator(principal.as_bytes(), relay.as_bytes())
        .await
    {
        Ok(true) => tracing::info!(principal = %principal, "auth.operator_bootstrapped"),
        Ok(false) => {}
        Err(error) => tracing::error!(%error, "operator bootstrap failed"),
    }
}

/// Issue a fresh access + refresh pair. Returns `(access, refresh, issued
/// access, issued refresh)`.
pub(super) fn issue_session_tokens(
    state: &AppState,
) -> (
    buzz_auth::TokenSecret,
    buzz_auth::TokenSecret,
    IssuedToken,
    IssuedToken,
) {
    let config = state.identity.config();
    let now = Utc::now();
    let (access, access_hash) = generate_token(TokenKind::UserAccess);
    let (refresh, refresh_hash) = generate_token(TokenKind::UserRefresh);
    let access_issued = IssuedToken {
        hash: access_hash,
        expires_at: Some(now + chrono::Duration::from_std(config.access_ttl).unwrap_or_default()),
    };
    let refresh_issued = IssuedToken {
        hash: refresh_hash,
        expires_at: Some(now + chrono::Duration::from_std(config.refresh_ttl).unwrap_or_default()),
    };
    (access, refresh, access_issued, refresh_issued)
}

/// `Set-Cookie` for the web refresh token (HttpOnly, path-scoped).
pub(super) fn refresh_cookie(value: &str, max_age_secs: u64) -> String {
    format!(
        "buzz_refresh={value}; HttpOnly; Secure; SameSite=Strict; Path=/auth/refresh; Max-Age={max_age_secs}"
    )
}

/// `POST /auth/oidc/complete` `{login_code, code_verifier}`.
pub(super) async fn complete(
    State(state): State<Arc<AppState>>,
    extensions: Extensions,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = rate_limit_client(
        &state,
        &extensions,
        &headers,
        "auth:login:complete",
        LOGIN_COMPLETE_PER_IP,
        LOGIN_COMPLETE_GLOBAL,
    )
    .await
    {
        return response;
    }
    let body = match json_object(&body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let (Some(login_code), Some(verifier)) = (
        body.get("login_code").and_then(|v| v.as_str()),
        body.get("code_verifier").and_then(|v| v.as_str()),
    ) else {
        return bad_request("login_code and code_verifier are required");
    };
    let code_hash = buzz_auth::hash_token(login_code);
    let record = match kv::take(&state.redis_pool, "login", &hex::encode(code_hash)).await {
        Ok(Some(json)) => serde_json::from_str::<LoginCodeRecord>(&json).ok(),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "login code store unavailable");
            return unavailable();
        }
    };
    // The code is consumed above whatever happens next: one attempt only.
    let Some(record) = record else {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "invalid or expired login code",
        );
    };
    if !verify_pkce(verifier, &record.code_challenge) {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "PKCE verification failed",
        );
    }
    let Ok(principal) = PrincipalId::from_hex(&record.principal) else {
        return internal("login code principal", &"invalid principal id");
    };
    let key_mode = match state.db.account_key_mode(&principal).await {
        Ok(mode) => mode,
        Err(_) => return unavailable(),
    };
    if key_mode != (record.identity_mode == "key_backup") {
        return auth_error(
            StatusCode::CONFLICT,
            "account_mode_conflict",
            "account identity mode changed",
        );
    }
    let signing_pubkey = if key_mode {
        match state.db.get_key_backup(&principal).await {
            Ok(backup) => backup.map(|backup| hex::encode(backup.pubkey)),
            Err(_) => return unavailable(),
        }
    } else {
        None
    };
    let (access, refresh, access_issued, refresh_issued) = issue_session_tokens(&state);
    let session = match state
        .db
        .complete_login(
            &principal,
            &record.device_name,
            &record.client,
            record.install_id.as_deref(),
            refresh_issued,
            access_issued,
        )
        .await
    {
        Ok(session) => session,
        Err(buzz_db::DbError::AccessDenied(message)) => {
            return auth_error(StatusCode::FORBIDDEN, "limit_reached", &message)
        }
        Err(error) => return internal("complete_login", &error),
    };
    if !session.revoked.is_empty() {
        publish_revocations(
            &state,
            &session.revoked,
            RevokeReason::Relogin.as_str(),
            None,
        )
        .await;
    }
    tracing::info!(principal = %principal, device = %session.device_id, "auth.login");
    let expires_in = state.identity.config().access_ttl.as_secs();
    if record.client == "web" {
        let cookie = refresh_cookie(
            refresh.expose(),
            state.identity.config().refresh_ttl.as_secs(),
        );
        return (
            StatusCode::OK,
            [(header::SET_COOKIE, cookie)],
            axum::Json(json!({
                "principal_id": principal.to_hex(),
                "identity_mode": record.identity_mode,
                "signing_pubkey": signing_pubkey,
                "device_id": session.device_id,
                "access": access.expose(),
                "expires_in": expires_in,
            })),
        )
            .into_response();
    }
    axum::Json(json!({
        "principal_id": principal.to_hex(),
        "identity_mode": record.identity_mode,
        "signing_pubkey": signing_pubkey,
        "device_id": session.device_id,
        "access": access.expose(),
        "refresh": refresh.expose(),
        "expires_in": expires_in,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::install_id_from_query;

    #[test]
    fn install_id_is_optional_normalized_and_bounded() {
        assert_eq!(install_id_from_query(None), Ok(None));
        assert_eq!(install_id_from_query(Some("  ".into())), Ok(None));
        assert_eq!(
            install_id_from_query(Some(" 5F0C6B1E-2A4D-4C8E-9B7A-1D2E3F4A5B6C ".into())),
            Ok(Some("5f0c6b1e-2a4d-4c8e-9b7a-1d2e3f4a5b6c".into()))
        );
        assert!(install_id_from_query(Some("short".into())).is_err());
        assert!(install_id_from_query(Some("a".repeat(65))).is_err());
        assert!(install_id_from_query(Some("not/an/install/id!".into())).is_err());
    }
}
