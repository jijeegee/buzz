//! OIDC login: `start` → provider → `callback` → client → `complete`
//! (plan §3.2). The relay is the relying party; the client proves with PKCE
//! that it started the login it completes.
//!
//! Confirmed signup: a key-backup client that sends `signup=confirm` to
//! `start` never gets an account created by the callback. An unknown Google
//! account comes back from `complete` as `needs_signup` with a one-time
//! `signup_token`; the account exists only once the client posts that token
//! to `signup`. Dropping the token leaves nothing behind. Clients that do not
//! send the flag keep the original create-on-callback behavior.
//!
//! App sign-in: an app that signs in with the platform's own account picker
//! (Android Credential Manager, Sign in with Apple on iOS) calls `start` with
//! `response=json`, gets the nonce and client id to hand that picker, and posts the
//! id_token it returns to `id-token` instead of opening a browser. From the
//! login code on, both paths are the same.

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{header, Extensions, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use kuzz_auth::token::{generate_token, verify_pkce, TokenKind};
use kuzz_core::principal::PrincipalId;
use kuzz_db::identity::IssuedToken;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{auth_error, bad_request, internal, json_object, rate_limit_client, unavailable};
use crate::identity::{client_version, kv};
use crate::state::AppState;

/// Pending-login lifetime (`state` → callback).
const PENDING_TTL_SECS: u64 = 600;
/// One-time login code lifetime (callback → complete).
const LOGIN_CODE_TTL_SECS: u64 = 60;
/// Lifetime of a confirmed-signup ticket (`complete` → `signup`): the user
/// is reading the "Create your Kuzz account" screen.
const SIGNUP_TTL_SECS: u64 = 600;
/// Bound on the `state` value echoed through the provider.
const MAX_STATE_LEN: usize = 256;
/// Bound on an app sign-in id_token (Google's and Apple's are ~1-2 KB).
const MAX_ID_TOKEN_LEN: usize = 8 * 1024;
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
    /// Device key the client will prove at `complete`/`signup` (normalized
    /// hex; set only while the auth device surface is on).
    #[serde(default)]
    device_pubkey: Option<String>,
    nonce: String,
    /// The client asks before an unknown identity becomes an account.
    #[serde(default)]
    confirm_signup: bool,
    /// Started for app sign-in: only `id-token` can finish it.
    #[serde(default)]
    app_sign_in: bool,
}

/// One-time login code record stored between `callback` and `complete`.
#[derive(Serialize, Deserialize)]
struct LoginCodeRecord {
    #[serde(default = "token_identity_mode")]
    identity_mode: String,
    /// Empty while `signup` waits for confirmation.
    #[serde(default)]
    principal: String,
    code_challenge: String,
    client: String,
    device_name: String,
    #[serde(default)]
    install_id: Option<String>,
    /// Device key the client will prove at `complete`/`signup` (normalized
    /// hex; set only while the auth device surface is on).
    #[serde(default)]
    device_pubkey: Option<String>,
    /// The provider's email for the signed-in identity, when it sent one.
    #[serde(default)]
    email: Option<String>,
    /// Set when the identity has no account yet and the client confirms
    /// signups: nothing was created.
    #[serde(default)]
    signup: Option<PendingSignup>,
}

/// An identity waiting for the user to confirm account creation.
#[derive(Clone, Serialize, Deserialize)]
struct PendingSignup {
    provider: String,
    subject: String,
    email: Option<String>,
    display_name: String,
    avatar_url: Option<String>,
}

/// One-time ticket stored between `complete` (`needs_signup`) and `signup`.
#[derive(Serialize, Deserialize)]
struct SignupTicket {
    identity_mode: String,
    signup: PendingSignup,
    code_challenge: String,
    client: String,
    device_name: String,
    #[serde(default)]
    install_id: Option<String>,
    /// Device key the client will prove at `complete`/`signup` (normalized
    /// hex; set only while the auth device surface is on).
    #[serde(default)]
    device_pubkey: Option<String>,
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
    /// SEC1 hex P-256 device key the login will prove (newer apps).
    device_pubkey: Option<String>,
    /// `confirm`: ask before creating an account (key-backup logins only).
    signup: Option<String>,
    /// `<app>/<version>`: the `X-Kuzz-Client` value of an app whose browser
    /// cannot send the header.
    kuzz_client: Option<String>,
    /// `json`: app sign-in; answer with the nonce instead of redirecting.
    response: Option<String>,
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
    /// Sign in with Apple's first-sign-in user JSON (`form_post` only).
    user: Option<String>,
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
                return auth_error(StatusCode::BAD_REQUEST, "unsupported_client", "Google key backup signup and recovery require Kuzz desktop or mobile; existing CLI key import remains supported");
            }
        }
        "token" if state.identity.enabled() => {}
        _ => return bad_request("requested identity_mode is not enabled"),
    }
    let confirm_signup = match query.signup.as_deref() {
        None => false,
        Some("confirm") if identity_mode == "key_backup" => true,
        Some(_) => return bad_request("signup=confirm requires identity_mode=key_backup"),
    };
    let Some(redirect_uri) = query
        .redirect_uri
        .filter(|uri| redirect_allowed(&state, &client, uri))
    else {
        return bad_request("redirect_uri is not allowed for this client");
    };
    let app_client_id = match query.response.as_deref() {
        None => None,
        Some("json") if client == "mobile" => match provider.app_sign_in_client_id() {
            Some(id) => Some(id.to_owned()),
            None => return bad_request("provider has no app sign-in"),
        },
        Some(_) => return bad_request("response=json requires client=mobile"),
    };
    let app_sign_in = app_client_id.is_some();
    // Too old to sign in: back to the app, which shows "Update Kuzz".
    let declared = client_version::declared(&headers).or(query.kuzz_client.as_deref());
    if let Err(min) = client_version::check_current(declared, Some(client.as_str())).await {
        if app_sign_in {
            return client_version::update_required_response(&min);
        }
        return client_redirect(
            &redirect_uri,
            &[
                ("error", client_version::UPDATE_REQUIRED),
                ("min_version", &min),
                ("state", &client_state),
            ],
        );
    }
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
    let device_pubkey =
        match crate::identity::device::start_device_key(&state, query.device_pubkey.as_deref()) {
            Ok(key) => key,
            Err(response) => return response,
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
        device_pubkey,
        nonce: nonce.clone(),
        confirm_signup,
        app_sign_in,
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
    if app_sign_in {
        return axum::Json(json!({ "nonce": nonce, "client_id": app_client_id }))
            .into_response();
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
    finish_callback(state, provider_name, query).await
}

/// `POST /auth/oidc/{provider}/callback`: the same callback as a form, as
/// Sign in with Apple sends it (`response_mode=form_post`).
pub(super) async fn callback_form(
    State(state): State<Arc<AppState>>,
    Path(provider_name): Path<String>,
    body: Bytes,
) -> Response {
    if body.len() > 8 * 1024 {
        return bad_request("callback form too large");
    }
    let query: CallbackQuery = match serde_urlencoded::from_bytes(&body) {
        Ok(query) => query,
        Err(_) => return bad_request("invalid form"),
    };
    finish_callback(state, provider_name, query).await
}

async fn finish_callback(
    state: Arc<AppState>,
    provider_name: String,
    query: CallbackQuery,
) -> Response {
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
    if pending.app_sign_in {
        return bad_request("login state was started for app sign-in");
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
    let mut identity = match provider
        .exchange_code(&code, &callback_url, &pending.nonce)
        .await
    {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(%error, provider = %provider_name, "OIDC code exchange failed");
            return fail("login_failed");
        }
    };
    if identity.name.is_none() {
        identity.name = query
            .user
            .as_deref()
            .and_then(kuzz_auth::oidc::first_sign_in_name);
    }
    match issue_login_code(&state, &provider_name, &pending, &identity).await {
        Ok(code_token) => client_redirect(
            &pending.redirect_uri,
            &[("code", code_token.expose()), ("state", &client_state)],
        ),
        Err(error) => fail(error),
    }
}

/// `POST /auth/oidc/{provider}/id-token` `{state, id_token, name?}`: finish
/// an app sign-in `start`ed with `response=json`. Answers `{login_code}` for
/// `complete`, exactly what `callback` hands a browser client. `name`: the
/// display name the platform gave the app when the token carries none
/// (Apple sends it only on the first sign-in).
pub(super) async fn id_token(
    State(state): State<Arc<AppState>>,
    Path(provider_name): Path<String>,
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
    let (Some(client_state), Some(id_token)) = (
        body.get("state")
            .and_then(|v| v.as_str())
            .filter(|s| s.len() <= MAX_STATE_LEN),
        body.get("id_token")
            .and_then(|v| v.as_str())
            .filter(|t| t.len() <= MAX_ID_TOKEN_LEN),
    ) else {
        return bad_request("state and id_token are required");
    };
    let pending = match kv::take(&state.redis_pool, "oidc:pending", client_state).await {
        Ok(Some(json)) => serde_json::from_str::<PendingLogin>(&json).ok(),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "OIDC pending store unavailable");
            return unavailable();
        }
    };
    // Consumed above: one id_token per nonce.
    let Some(pending) = pending.filter(|p| p.app_sign_in && p.provider == provider_name) else {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "unknown or expired login state",
        );
    };
    let Some(provider) = state.identity.provider(&provider_name) else {
        return super::not_found("unknown provider");
    };
    let mut identity = match provider.verify_id_token(id_token, &pending.nonce).await {
        Ok(identity) => identity,
        Err(kuzz_auth::oidc::OidcError::Unavailable(error)) => {
            tracing::warn!(%error, provider = %provider_name, "OIDC provider unavailable");
            return unavailable();
        }
        Err(error) => {
            tracing::warn!(%error, provider = %provider_name, "app sign-in id_token rejected");
            return auth_error(
                StatusCode::UNAUTHORIZED,
                "login_failed",
                "the sign-in could not be verified",
            );
        }
    };
    if identity.name.is_none() {
        identity.name = body
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
            .map(|name| name.chars().take(64).collect());
    }
    match issue_login_code(&state, &provider_name, &pending, &identity).await {
        Ok(code_token) => axum::Json(json!({ "login_code": code_token.expose() })).into_response(),
        Err(error) => {
            let status = match error {
                "principal_disabled" => StatusCode::FORBIDDEN,
                "account_mode_conflict" => StatusCode::CONFLICT,
                "unsupported_client" => StatusCode::BAD_REQUEST,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            };
            auth_error(status, error, "sign-in failed")
        }
    }
}

/// The provider vouched for `identity`: sign it in (or hold it for a
/// confirmed signup) and store the one-time login code `complete` redeems.
/// `Err` is the error code the client is shown.
async fn issue_login_code(
    state: &Arc<AppState>,
    provider_name: &str,
    pending: &PendingLogin,
    identity: &kuzz_auth::oidc::OidcIdentity,
) -> Result<kuzz_auth::TokenSecret, &'static str> {
    let display_name = identity
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "user".to_owned());
    let name = display_name.chars().take(64).collect::<String>();
    let result = if pending.identity_mode == "key_backup" && pending.confirm_signup {
        state
            .db
            .login_existing_key_identity(
                provider_name,
                &identity.subject,
                identity.email.as_deref(),
                &name,
                identity.picture.as_deref(),
            )
            .await
    } else if pending.identity_mode == "key_backup" {
        state
            .db
            .login_key_identity(
                provider_name,
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
                provider_name,
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
                provider_name,
                &identity.subject,
                identity.email.as_deref(),
                &name,
                identity.picture.as_deref(),
            )
            .await
    };
    let login = match result {
        Ok(login) => Some(login),
        // Nothing was created: `complete` hands the client a signup ticket.
        Err(kuzz_db::DbError::AccessDenied(message)) if message == "signup_required" => None,
        Err(kuzz_db::DbError::AccessDenied(message)) if message == "account_mode_conflict" => {
            return Err("account_mode_conflict");
        }
        Err(kuzz_db::DbError::AccessDenied(message)) if message == "unsupported_client" => {
            return Err("unsupported_client");
        }
        Err(error) => {
            tracing::error!(%error, "identity login failed");
            return Err("server_error");
        }
    };
    if let Some(login) = &login {
        if login.disabled {
            return Err("principal_disabled");
        }
        if login.created {
            tracing::info!(principal = %login.principal, "auth.account_created");
        }
        if pending.identity_mode == "token" {
            bootstrap_operator(state, &login.principal, identity).await;
        }
    }

    let (code_token, code_hash) = generate_token(TokenKind::LoginCode);
    let record = LoginCodeRecord {
        identity_mode: pending.identity_mode.clone(),
        principal: login
            .as_ref()
            .map(|login| login.principal.to_hex())
            .unwrap_or_default(),
        code_challenge: pending.code_challenge.clone(),
        client: pending.client.clone(),
        device_name: pending.device_name.clone(),
        install_id: pending.install_id.clone(),
        device_pubkey: pending.device_pubkey.clone(),
        email: identity.email.clone(),
        signup: login.is_none().then(|| PendingSignup {
            provider: provider_name.to_owned(),
            subject: identity.subject.clone(),
            email: identity.email.clone(),
            display_name: name.clone(),
            avatar_url: identity.picture.clone(),
        }),
    };
    let Ok(record_json) = serde_json::to_string(&record) else {
        return Err("server_error");
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
        Ok(false) | Err(_) => return Err("server_error"),
    }
    Ok(code_token)
}

/// Operator bootstrap (plan §3.3 B8), evaluated on **every** login: verified
/// email matching `RELAY_OPERATOR_BOOTSTRAP_EMAIL` (case-insensitive) while the
/// DB roster has no operator. A failure is logged, never fatal to the login.
pub(super) async fn bootstrap_operator(
    state: &AppState,
    principal: &PrincipalId,
    identity: &kuzz_auth::oidc::OidcIdentity,
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
    kuzz_auth::TokenSecret,
    kuzz_auth::TokenSecret,
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
        "kuzz_refresh={value}; HttpOnly; Secure; SameSite=Strict; Path=/auth/refresh; Max-Age={max_age_secs}"
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
    let code_hash = kuzz_auth::hash_token(login_code);
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
    if let Err(response) = version_gate(&headers, &record.client).await {
        return response;
    }
    if record.signup.is_some() {
        return signup_needed(&state, record).await;
    }
    let Ok(principal) = PrincipalId::from_hex(&record.principal) else {
        return internal("login code principal", &"invalid principal id");
    };
    let device_key = match crate::identity::device::login_device_key(
        &state,
        &body,
        &format!("login:{}", hex::encode(code_hash)),
        record.device_pubkey.as_deref(),
    )
    .await
    {
        Ok(key) => key,
        Err(response) => return response,
    };
    finish_login(
        &state,
        &headers,
        &principal,
        &record.identity_mode,
        &record.client,
        &record.device_name,
        record.install_id.as_deref(),
        device_key.as_ref(),
        record.email.as_deref(),
    )
    .await
}

/// Refuse an app below its minimum version (`client`: the kind named at
/// `start`). The login code or ticket is already spent; after updating the
/// user signs in again.
async fn version_gate(headers: &HeaderMap, client: &str) -> Result<(), Response> {
    client_version::check_current(client_version::declared(headers), Some(client))
        .await
        .map_err(|min| client_version::update_required_response(&min))
}

/// `complete` for an identity with no account yet: nothing is created; the
/// client gets a one-time `signup_token` to confirm with.
async fn signup_needed(state: &AppState, record: LoginCodeRecord) -> Response {
    let Some(signup) = record.signup else {
        return unavailable();
    };
    let (ticket_token, ticket_hash) = generate_token(TokenKind::LoginCode);
    let ticket = SignupTicket {
        identity_mode: record.identity_mode,
        signup: signup.clone(),
        code_challenge: record.code_challenge,
        client: record.client,
        device_name: record.device_name,
        install_id: record.install_id,
        device_pubkey: record.device_pubkey,
    };
    let Ok(ticket_json) = serde_json::to_string(&ticket) else {
        return unavailable();
    };
    match kv::put_new(
        &state.redis_pool,
        "oidc:signup",
        &hex::encode(ticket_hash),
        &ticket_json,
        SIGNUP_TTL_SECS,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return unavailable(),
        Err(error) => {
            tracing::warn!(%error, "signup ticket store unavailable");
            return unavailable();
        }
    }
    axum::Json(json!({
        "needs_signup": true,
        "signup_token": ticket_token.expose(),
        "email": signup.email,
        "display_name": signup.display_name,
        "avatar_url": signup.avatar_url,
    }))
    .into_response()
}

/// `POST /auth/oidc/signup` `{signup_token, code_verifier}`: create the
/// account the user confirmed, then sign in exactly as `complete` does.
pub(super) async fn signup(
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
    let (Some(signup_token), Some(verifier)) = (
        body.get("signup_token").and_then(|v| v.as_str()),
        body.get("code_verifier").and_then(|v| v.as_str()),
    ) else {
        return bad_request("signup_token and code_verifier are required");
    };
    let ticket_hash = kuzz_auth::hash_token(signup_token);
    let ticket = match kv::take(&state.redis_pool, "oidc:signup", &hex::encode(ticket_hash)).await
    {
        Ok(Some(json)) => serde_json::from_str::<SignupTicket>(&json).ok(),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "signup ticket store unavailable");
            return unavailable();
        }
    };
    // Consumed above: one attempt only, as for login codes.
    let Some(ticket) = ticket else {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "invalid or expired signup",
        );
    };
    if !verify_pkce(verifier, &ticket.code_challenge) {
        return auth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "PKCE verification failed",
        );
    }
    if let Err(response) = version_gate(&headers, &ticket.client).await {
        return response;
    }
    let device_key = match crate::identity::device::login_device_key(
        &state,
        &body,
        &format!("signup:{}", hex::encode(ticket_hash)),
        ticket.device_pubkey.as_deref(),
    )
    .await
    {
        Ok(key) => key,
        Err(response) => return response,
    };
    let signup = &ticket.signup;
    // Creates the account; one created meanwhile (the same Google account
    // finishing a signup elsewhere) is signed into instead.
    let login = match state
        .db
        .login_key_identity(
            &signup.provider,
            &signup.subject,
            signup.email.as_deref(),
            &signup.display_name,
            signup.avatar_url.as_deref(),
        )
        .await
    {
        Ok(login) => login,
        Err(kuzz_db::DbError::AccessDenied(message)) if message == "account_mode_conflict" => {
            return auth_error(
                StatusCode::CONFLICT,
                "account_mode_conflict",
                "account identity mode changed",
            );
        }
        Err(error) => return internal("signup", &error),
    };
    if login.disabled {
        return auth_error(
            StatusCode::FORBIDDEN,
            "principal_disabled",
            "account is disabled",
        );
    }
    if login.created {
        tracing::info!(principal = %login.principal, "auth.account_created");
    }
    finish_login(
        &state,
        &headers,
        &login.principal,
        &ticket.identity_mode,
        &ticket.client,
        &ticket.device_name,
        ticket.install_id.as_deref(),
        device_key.as_ref(),
        signup.email.as_deref(),
    )
    .await
}

/// Open the session for `principal` and answer with its tokens (the end of
/// `complete` and `signup`). A login that created a new device row also
/// sends the account a "New sign-in" notice (`headers` name its community).
#[allow(clippy::too_many_arguments)]
async fn finish_login(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    principal: &PrincipalId,
    identity_mode: &str,
    client: &str,
    device_name: &str,
    install_id: Option<&str>,
    device_key: Option<&crate::identity::device::LoginDeviceKey>,
    email: Option<&str>,
) -> Response {
    let key_mode = match state.db.account_key_mode(principal).await {
        Ok(mode) => mode,
        Err(_) => return unavailable(),
    };
    if key_mode != (identity_mode == "key_backup") {
        return auth_error(
            StatusCode::CONFLICT,
            "account_mode_conflict",
            "account identity mode changed",
        );
    }
    let signing_pubkey = if key_mode {
        match state.db.get_key_backup(principal).await {
            Ok(backup) => backup.map(|backup| hex::encode(backup.pubkey)),
            Err(_) => return unavailable(),
        }
    } else {
        None
    };
    let (access, refresh, access_issued, refresh_issued) = issue_session_tokens(state);
    let session = match state
        .db
        .complete_login_with_device_key(
            principal,
            device_name,
            client,
            install_id,
            device_key.map(|device| kuzz_db::identity::DeviceKey {
                pubkey: device.key.as_bytes(),
                kind: device.kind,
            }),
            refresh_issued,
            access_issued,
        )
        .await
    {
        Ok(session) => session,
        Err(kuzz_db::DbError::AccessDenied(message)) => {
            return auth_error(StatusCode::FORBIDDEN, "limit_reached", &message)
        }
        Err(error) => return internal("complete_login", &error),
    };
    tracing::info!(principal = %principal, device = %session.device_id, "auth.login");
    // Device-bound keys: a refusal cached before this login must not outlive it.
    crate::identity::admission::forget_login(
        state,
        signing_pubkey
            .as_deref()
            .and_then(|key| hex::decode(key).ok())
            .and_then(|key| key.try_into().ok()),
        device_key.map(|device| *device.key.as_bytes()),
    );
    if session.new_device {
        spawn_new_signin_notice(state, headers, *principal, device_name, session.device_id);
    }
    let expires_in = state.identity.config().access_ttl.as_secs();
    if client == "web" {
        let cookie = refresh_cookie(
            refresh.expose(),
            state.identity.config().refresh_ttl.as_secs(),
        );
        return (
            StatusCode::OK,
            [(header::SET_COOKIE, cookie)],
            axum::Json(json!({
                "principal_id": principal.to_hex(),
                "identity_mode": identity_mode,
                "signing_pubkey": signing_pubkey,
                "device_id": session.device_id,
                "access": access.expose(),
                "expires_in": expires_in,
                "email": email,
            })),
        )
            .into_response();
    }
    axum::Json(json!({
        "principal_id": principal.to_hex(),
        "identity_mode": identity_mode,
        "signing_pubkey": signing_pubkey,
        "device_id": session.device_id,
        "access": access.expose(),
        "refresh": refresh.expose(),
        "expires_in": expires_in,
        "email": email,
    }))
    .into_response()
}

/// Send the "New sign-in: <device name>" notice (device-bound keys plan §5,
/// PR A4b) to the account's own message key in the community `headers`
/// name. Informational only, no approval step: it runs on its own task, so
/// it never delays or fails the login, and errors are only logged. An
/// account without a message key gets none.
fn spawn_new_signin_notice(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    principal: PrincipalId,
    device_name: &str,
    device_id: uuid::Uuid,
) {
    let state = Arc::clone(state);
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let device_name = device_name.to_owned();
    tokio::spawn(async move {
        let tenant = match crate::tenant::bind_community(&state.db, &host).await {
            Ok(tenant) => tenant,
            Err(crate::tenant::BindError::UnmappedHost) => return,
            Err(crate::tenant::BindError::Lookup(error)) => {
                tracing::warn!(%error, "new sign-in notice: community lookup failed");
                return;
            }
        };
        let pubkey = match state.db.principal_messaging_pubkey(&principal).await {
            Ok(Some(pubkey)) => pubkey,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "new sign-in notice: account key lookup failed");
                return;
            }
        };
        if let Err(error) = crate::handlers::side_effects::publish_new_signin_notice(
            &tenant,
            &state,
            &pubkey,
            &device_name,
            device_id,
        )
        .await
        {
            tracing::warn!(%error, "new sign-in notice failed");
        }
    });
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
