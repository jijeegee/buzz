//! Tauri commands for Google sign-in and account/device management.
//!
//! Every command acts on the **current** community (its relay origin) and
//! captures that origin once, so a community switch mid-call cannot retarget it.

use nostr::PublicKey;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use zeroize::Zeroizing;

use super::api::{self, ApiError};
use super::loopback::{LoopbackListener, LOGIN_TIMEOUT};
use super::refresh::{self, RefreshFailure};
use super::{CredentialMode, KeyringRefreshStore, OriginAuth, RefreshStore, UserSession};
use crate::app_state::AppState;

/// Refresh before handing out an access token this close to expiry.
const FRAME_MIN_REMAINING_SECS: i64 = 60;

/// Token-auth status of the current community, for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenAuthStatus {
    /// Relay HTTP origin the status describes.
    pub origin: String,
    /// Whether the relay advertises token auth (NIP-11 `buzz_token_auth`).
    pub supported: bool,
    /// Sign-in providers the relay offers.
    pub providers: Vec<String>,
    /// `signed_out | restoring | active | needs_login`.
    pub state: &'static str,
    /// The signed-in principal (hex).
    pub principal: Option<String>,
    /// This device's id on the relay.
    pub device_id: Option<String>,
    /// Why a session ended (`needs_login`).
    pub reason: Option<String>,
    /// This relay supports custodial backup without token messaging.
    pub key_backup_supported: bool,
    /// The current account backs up a Nostr key, independently of sessions.
    pub key_backup: bool,
    /// Known pre-existing token account, including after local logout.
    pub legacy_token_account: bool,
    /// Messaging identity, never the backup authorization account id.
    pub signing_pubkey: Option<String>,
}

fn status_of(
    origin: &str,
    providers: Option<Vec<String>>,
    current: Option<OriginAuth>,
) -> TokenAuthStatus {
    let (state, principal, device_id, reason) = match current {
        None | Some(OriginAuth::SignedOut) => ("signed_out", None, None, None),
        Some(OriginAuth::Restoring) => ("restoring", None, None, None),
        Some(OriginAuth::Active(session)) => (
            "active",
            Some(session.principal.to_hex()),
            session.device_id,
            None,
        ),
        Some(OriginAuth::NeedsLogin(code)) => ("needs_login", None, None, Some(code)),
    };
    TokenAuthStatus {
        origin: origin.to_owned(),
        supported: providers.is_some(),
        providers: providers.unwrap_or_default(),
        state,
        principal,
        device_id,
        reason,
        key_backup_supported: false,
        key_backup: false,
        legacy_token_account: false,
        signing_pubkey: None,
    }
}

fn current_status(
    state: &AppState,
    origin: &str,
    providers: Option<Vec<String>>,
) -> TokenAuthStatus {
    let mut status = status_of(origin, providers, state.token_auth.get(origin).map(|e| e.1));
    status.key_backup_supported = state.token_auth.supports_key_backup(origin);
    status.key_backup = state.token_auth.signing_pubkey(origin).is_some();
    status.legacy_token_account = !status.key_backup
        && (!matches!(
            state.token_auth.get(origin),
            None | Some((_, OriginAuth::SignedOut))
        ) || matches!(KeyringRefreshStore.legacy_token_account(origin), Ok(true)));
    status.signing_pubkey = status
        .key_backup
        .then(|| {
            state
                .keys
                .lock()
                .ok()
                .map(|keys| keys.public_key().to_hex())
        })
        .flatten();
    status
}

use super::restore::{emit_changed, spawn_refresh_loop};

/// NIP-11 support for `origin`, cached briefly. A fetch failure keeps the last
/// known answer, and assumes support when a session or stored refresh token
/// exists (an outage must not silently drop a signed-in user to key auth).
async fn support_for(state: &AppState, origin: &str) -> Option<Vec<String>> {
    if let Some(cached) = state.token_auth.cached_support(origin) {
        return cached;
    }
    match api::fetch_token_auth_support(&state.http_client, origin).await {
        Ok((support, backup)) => {
            state.token_auth.record_backup_support(origin, backup);
            state.token_auth.record_support(origin, support.clone());
            support
        }
        Err(_) => {
            let has_session = !matches!(
                state.token_auth.get(origin),
                None | Some((_, OriginAuth::SignedOut))
            );
            let has_stored = matches!(KeyringRefreshStore.load(origin), Ok(Some(_)));
            (has_session || has_stored).then(|| vec!["google".to_owned()])
        }
    }
}

/// Report (and, on first call per community, restore) the token-auth state of
/// the current community. Restores a keyring session by refreshing it.
#[tauri::command]
pub async fn get_token_auth_status(app: AppHandle) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let support = support_for(&state, &origin).await;
    if support.is_none() {
        // The relay does not (or no longer does) offer token auth: key auth.
        if let Some((generation, OriginAuth::Active(_) | OriginAuth::Restoring)) =
            state.token_auth.get(&origin)
        {
            state
                .token_auth
                .replace_if(&origin, generation, OriginAuth::SignedOut);
        }
        return Ok(current_status(&state, &origin, None));
    }
    super::restore::restore_if_stored(&app, &origin).await;
    Ok(current_status(&state, &origin, support))
}

fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let host: String = host.chars().filter(|c| !c.is_control()).take(48).collect();
    if host.trim().is_empty() {
        "Buzz Desktop".to_owned()
    } else {
        format!("Buzz Desktop ({})", host.trim())
    }
}

/// Sign in to the current community with Google: system browser + loopback
/// redirect + PKCE (plan §3.2). Resolves once the session is active.
#[tauri::command]
pub async fn login_with_google(
    app: AppHandle,
    allow_restore: Option<bool>,
    existing_token_account: Option<bool>,
    attempt_id: Option<String>,
) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let id = attempt_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if id.is_empty() || id.len() > 128 {
        return Err("invalid sign-in attempt".into());
    }
    let mut attempt = state.token_auth.login.begin(id.clone())?;
    let _ = app.emit(
        "google-login-progress",
        serde_json::json!({"attemptId": id, "phase": "waiting"}),
    );
    login_inner(
        &app,
        allow_restore == Some(true),
        existing_token_account == Some(true),
        &id,
        &mut attempt,
    )
    .await
}

/// Cancel only this browser attempt; completed or unrelated attempts are untouched.
#[tauri::command]
pub fn cancel_google_login(app: AppHandle, attempt_id: String) -> bool {
    app.state::<AppState>().token_auth.login.cancel(&attempt_id)
}

async fn login_inner(
    app: &AppHandle,
    allow_restore: bool,
    existing_token_account: bool,
    attempt_id: &str,
    attempt: &mut super::login_attempt::LoginAttempt,
) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let (providers, pkce, backup, code) = attempt
        .wait(async {
            let providers = support_for(&state, &origin)
                .await
                .ok_or("this community does not offer Google sign-in")?;
            if !providers.iter().any(|p| p == "google") {
                return Err("this community does not offer Google sign-in".into());
            }
            let pkce = super::pkce::pkce_pair()?;
            let oauth_state = super::pkce::new_state()?;
            let listener = LoopbackListener::bind().await.map_err(|e| e.to_string())?;
            let mut url = api::start_url(
                &origin,
                "google",
                &oauth_state,
                &pkce.challenge,
                &listener.redirect_uri(),
                &device_name(),
            );
            let backup = requested_login_mode(
                &state.token_auth,
                &origin,
                &KeyringRefreshStore,
                existing_token_account,
            )?;
            if existing_token_account {
                let _guard = state
                    .managed_agents_store_lock
                    .lock()
                    .map_err(|_| "agent storage is busy")?;
                ensure_existing_token_agents(
                    &origin,
                    &crate::managed_agents::load_managed_agents(app)?,
                )?;
            }
            if backup {
                url.push_str("&identity_mode=key_backup");
            } else {
                url.push_str("&identity_mode=token");
            }
            app.opener()
                .open_url(url.as_str(), None::<&str>)
                .map_err(|e| format!("could not open the browser: {e}"))?;
            let code = listener
                .wait_for_code(&oauth_state, LOGIN_TIMEOUT)
                .await
                .map_err(|e| e.to_string())?;
            Ok((providers, pkce, backup, code))
        })
        .await?;
    let _ = app.emit(
        "google-login-progress",
        serde_json::json!({"attemptId": attempt_id, "phase": "completing"}),
    );
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window
            .unminimize()
            .and_then(|_| window.show())
            .and_then(|_| window.set_focus())
        {
            eprintln!("buzz-desktop: could not activate sign-in window: {error}");
        }
    }
    let tokens = api::complete_login(&state.http_client, &origin, &code, &pkce.verifier)
        .await
        .map_err(|e| format!("sign-in could not be completed: {e}"))?;
    verify_login_mode(backup, tokens.identity_mode.as_deref())?;
    let principal = PublicKey::from_hex(&tokens.principal_id)
        .map_err(|_| "the relay returned an invalid account id".to_string())?;
    // Under the refresh lock: an in-flight refresh of an older generation can
    // neither write its token over this one nor revive the old session.
    let _refresh_guard = state.token_auth.refresh_lock(&origin).lock_owned().await;
    if !backup
        && requested_login_mode(
            &state.token_auth,
            &origin,
            &KeyringRefreshStore,
            existing_token_account,
        )?
    {
        return Err("A key-backup binding appeared during sign-in. Your existing session was preserved; sign in again.".into());
    }
    if backup {
        super::key_recovery::complete(app, &origin, &tokens, allow_restore).await?;
    }
    let agent_store_guard = if !backup && existing_token_account {
        let guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|_| "agent storage is busy")?;
        ensure_existing_token_agents(&origin, &crate::managed_agents::load_managed_agents(app)?)?;
        Some(guard)
    } else {
        None
    };
    if !backup {
        // This non-secret account mode survives logout, so a later sign-in
        // cannot silently select custody for a known legacy account.
        KeyringRefreshStore.remember_legacy_token_account(&origin)?;
        if let Err(error) = KeyringRefreshStore.store(&origin, &tokens.refresh) {
            // The session still works for this run; the next launch asks again.
            eprintln!("buzz-desktop: auth: could not store the session in the keyring: {error}");
        }
    }
    state.token_auth.clear_pending_refresh(&origin);
    let now = chrono::Utc::now().timestamp();
    let session = UserSession {
        principal,
        device_id: tokens.device_id.clone(),
        access: Zeroizing::new(tokens.access.0.to_string()),
        refresh: Zeroizing::new(tokens.refresh.0.to_string()),
        access_issued_at: now,
        access_expires_at: now + tokens.expires_in.max(0),
    };
    let generation = state.token_auth.set(&origin, OriginAuth::Active(session));
    drop(agent_store_guard);
    drop(_refresh_guard);
    spawn_refresh_loop(app, &origin, generation);
    emit_changed(app, &origin);
    Ok(current_status(&state, &origin, Some(providers)))
}

fn ensure_existing_token_agents(
    origin: &str,
    records: &[crate::managed_agents::ManagedAgentRecord],
) -> Result<(), String> {
    // Match runtime relay resolution: creation-era relay pins do not isolate
    // key agents from the active community's token adoption path.
    if records.iter().any(|record| {
        !record.pubkey.is_empty()
            && record.bot_origin.is_none()
            && super::origin_for(&crate::relay::effective_agent_relay_url(
                &record.relay_url,
                origin,
            )) == origin
    }) {
        return Err("This community has existing agents with signing keys. Opening a separate token account here could replace their identities, so sign-in was stopped. Your keys, agents, and account data were preserved. Keep using key authentication or link this identity with Google key backup.".into());
    }
    Ok(())
}

/// Choose the account mode used by the actual OIDC start and completion path.
fn login_uses_key_backup(
    auth: &super::TokenAuthState,
    origin: &str,
    store: &dyn RefreshStore,
) -> Result<bool, String> {
    // Durable/in-memory identity state outranks discovery. An outage or a
    // changed descriptor must never downgrade a backup account to token mode.
    if auth.signing_pubkey(origin).is_some() {
        return Ok(true);
    }
    if let Some(pubkey) = store.signing_pubkey(origin)? {
        auth.set_key_backup(origin, pubkey);
        return Ok(true);
    }
    match auth.get(origin) {
        Some((_, OriginAuth::NeedsLogin(reason))) if reason.starts_with("backup_") => {
            Err("The stored key-backup binding is unavailable. Restore access to OS secure storage and retry; token login cannot replace it.".into())
        }
        Some((_, OriginAuth::Active(_) | OriginAuth::Restoring | OriginAuth::NeedsLogin(_))) => Ok(false),
        _ if store.legacy_token_account(origin)? || store.load(origin)?.is_some() => Ok(false),
        _ => Ok(auth.supports_key_backup(origin)),
    }
}

fn requested_login_mode(
    auth: &super::TokenAuthState,
    origin: &str,
    store: &dyn RefreshStore,
    existing_token_account: bool,
) -> Result<bool, String> {
    let backup = login_uses_key_backup(auth, origin, store)?;
    if existing_token_account {
        if auth.signing_pubkey(origin).is_some() {
            return Err("This device has a Google key-backup binding. An existing token account cannot replace it.".into());
        }
        // Explicit route for a pre-upgrade user who already signed out. The
        // custody-enabled relay accepts only an existing token account here.
        Ok(false)
    } else {
        Ok(backup)
    }
}

fn verify_login_mode(backup: bool, returned_mode: Option<&str>) -> Result<(), String> {
    let matches = if backup {
        returned_mode == Some("key_backup")
    } else {
        // Older token-only relays predate the explicit mode field.
        matches!(returned_mode, None | Some("token"))
    };
    if !matches {
        return Err("The relay returned a different account mode. Your existing identity and session were preserved; accounts cannot be merged automatically.".into());
    }
    Ok(())
}

/// The current community's live session, refreshed first when it is about to
/// expire. Errors when the community is not signed in.
async fn fresh_session(state: &AppState, origin: &str) -> Result<UserSession, String> {
    let (generation, session) = match state.token_auth.get(origin) {
        Some((generation, OriginAuth::Active(session))) => (generation, session),
        _ => {
            return Err(match state.token_auth.mode(origin) {
                CredentialMode::Blocked(reason) => reason,
                _ => "not signed in with Google for this community".into(),
            })
        }
    };
    if session.access_expires_at - chrono::Utc::now().timestamp() > FRAME_MIN_REMAINING_SECS {
        return Ok(session);
    }
    refresh::refresh_now(
        &state.token_auth,
        &state.http_client,
        origin,
        generation,
        &KeyringRefreshStore,
        Some(session.access_expires_at),
    )
    .await
    .map_err(|failure| match failure {
        RefreshFailure::Transient => "could not reach the relay to renew your sign-in".into(),
        RefreshFailure::Terminal(_) => {
            "your sign-in for this community has ended; sign in with Google again".into()
        }
    })
}

/// `origin`'s session for an agent spawn (sync callers): `None` in key mode,
/// refreshed first when the access token is about to expire, so a bot-token
/// reissue after sleep or a long backoff never goes out with an expired
/// access token. The refresh runs on the app runtime and is awaited, never
/// cancelled.
pub(crate) fn fresh_session_for_spawn(
    state: &AppState,
    origin: &str,
) -> Result<Option<UserSession>, String> {
    let session = match state.token_auth.mode(origin) {
        CredentialMode::Keys => return Ok(None),
        CredentialMode::Blocked(reason) => return Err(reason),
        CredentialMode::Token(session) => session,
    };
    if session.access_expires_at - chrono::Utc::now().timestamp() > FRAME_MIN_REMAINING_SECS {
        return Ok(Some(session));
    }
    let Some((generation, _)) = state.token_auth.get(origin) else {
        return Err("not signed in with Google for this community".into());
    };
    let auth = std::sync::Arc::clone(&state.token_auth);
    let client = state.http_client.clone();
    let origin = origin.to_owned();
    let expiry = session.access_expires_at;
    let handle = tauri::async_runtime::spawn(async move {
        refresh::refresh_now(
            &auth,
            &client,
            &origin,
            generation,
            &KeyringRefreshStore,
            Some(expiry),
        )
        .await
    });
    super::bots::block_on_isolated(handle)?
        .map_err(|e| format!("auth refresh task: {e}"))?
        .map(Some)
        .map_err(|failure| match failure {
            RefreshFailure::Transient => "could not reach the relay to renew your sign-in".into(),
            RefreshFailure::Terminal(_) => {
                "your sign-in for this community has ended; sign in with Google again".into()
            }
        })
}

/// WebSocket AUTH payload for the current community: `{"token": …}` in token
/// mode, `null` in key mode (the caller signs a NIP-42 event instead).
#[tauri::command]
pub async fn get_ws_auth_frame(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    match state.token_auth.mode(&origin) {
        CredentialMode::Keys => Ok(None),
        CredentialMode::Blocked(reason) => Err(reason),
        CredentialMode::Token(_) => {
            let session = fresh_session(&state, &origin).await?;
            Ok(Some(
                serde_json::json!({ "token": session.access.as_str() }),
            ))
        }
    }
}

fn api_error(error: ApiError) -> String {
    error.to_string()
}

/// Sign out of the current community: revoke the session server-side, then
/// forget it locally. A network failure keeps the session so the user can
/// retry (the server-side session would otherwise stay alive unrevoked).
#[tauri::command]
pub async fn logout(app: AppHandle) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    // Let an in-flight restore or refresh settle first (sign-out is offered
    // while restoring, Rule 6), so a session it just activated is revoked
    // server-side rather than only forgotten locally.
    drop(state.token_auth.refresh_lock(&origin).lock_owned().await);
    if let Some((_, OriginAuth::Active(session))) = state.token_auth.get(&origin) {
        match api::logout(&state.http_client, &origin, &session.access).await {
            Ok(())
            | Err(ApiError {
                status: Some(401), ..
            }) => {}
            Err(error) => return Err(format!("could not sign out: {error}")),
        }
    }
    forget_session(&state.token_auth, &origin, &KeyringRefreshStore).await?;
    emit_changed(&app, &origin);
    let support = state.token_auth.cached_support(&origin).flatten();
    Ok(current_status(&state, &origin, support))
}

/// Forget `origin`'s session locally: keyring token and in-memory state.
///
/// Takes the refresh lock first, so a refresh already in flight finishes (and
/// writes its rotated token) *before* the delete, and its generation fence then
/// stops it from writing anything afterwards: signing out can never be undone
/// by a late rotation.
pub(crate) async fn forget_session(
    auth: &super::TokenAuthState,
    origin: &str,
    store: &dyn RefreshStore,
) -> Result<(), String> {
    let _guard = auth.refresh_lock(origin).lock_owned().await;
    if auth.signing_pubkey(origin).is_none()
        && store.signing_pubkey(origin)?.is_none()
        && (!matches!(auth.get(origin), None | Some((_, OriginAuth::SignedOut)))
            || store.load(origin)?.is_some())
    {
        store.remember_legacy_token_account(origin)?;
    }
    store.delete(origin)?;
    auth.clear_pending_refresh(origin);
    auth.set(origin, OriginAuth::SignedOut);
    Ok(())
}

/// `GET /auth/devices`.
#[tauri::command]
pub async fn list_devices(app: AppHandle) -> Result<Vec<api::DeviceInfo>, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::list_devices(&state.http_client, &origin, &session.access)
        .await
        .map_err(api_error)
}

/// Remote sign-out of another device.
#[tauri::command]
pub async fn revoke_device(app: AppHandle, device_id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::revoke_device(&state.http_client, &origin, &session.access, &device_id)
        .await
        .map_err(api_error)
}

/// "Sign out all other devices" (human sessions only; bots untouched).
#[tauri::command]
pub async fn revoke_other_sessions(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::post_empty(
        &state.http_client,
        &origin,
        &session.access,
        "/auth/sessions/revoke-others",
    )
    .await
    .map_err(api_error)
}

/// "Revoke all bot tokens": every agent disconnects; hosted agents get a new
/// token on their next start.
#[tauri::command]
pub async fn revoke_all_bot_tokens(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::post_empty(
        &state.http_client,
        &origin,
        &session.access,
        "/auth/bots/revoke-all",
    )
    .await
    .map_err(api_error)
}

/// Delete the account (30-day recovery by signing in again), then sign out.
#[tauri::command]
pub async fn delete_account(app: AppHandle) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::delete_account(&state.http_client, &origin, &session.access)
        .await
        .map_err(api_error)?;
    forget_session(&state.token_auth, &origin, &KeyringRefreshStore).await?;
    emit_changed(&app, &origin);
    let support = state.token_auth.cached_support(&origin).flatten();
    Ok(current_status(&state, &origin, support))
}

/// Edit the global profile (`PATCH /auth/profile`).
#[tauri::command]
pub async fn update_global_profile(
    app: AppHandle,
    display_name: Option<String>,
    avatar_url: Option<String>,
) -> Result<(), String> {
    patch_global_profile(&app.state::<AppState>(), display_name, avatar_url).await
}

/// `PATCH /auth/profile` for the current community's signed-in account.
pub(crate) async fn patch_global_profile(
    state: &AppState,
    display_name: Option<String>,
    avatar_url: Option<String>,
) -> Result<(), String> {
    let origin = state.current_auth_origin();
    let session = fresh_session(state, &origin).await?;
    let mut body = serde_json::Map::new();
    if let Some(name) = display_name {
        body.insert("display_name".into(), serde_json::Value::String(name));
    }
    if let Some(url) = avatar_url {
        body.insert("avatar_url".into(), serde_json::Value::String(url));
    }
    api::update_profile(
        &state.http_client,
        &origin,
        &session.access,
        serde_json::Value::Object(body),
    )
    .await
    .map_err(api_error)
}

/// Agents (record pubkeys) that got a new bot identity since the last call —
/// shown once as "this agent was re-registered" (history break, plan Q7).
#[tauri::command]
pub fn take_token_auth_notices(app: AppHandle) -> Vec<String> {
    app.state::<AppState>().token_auth.bots.take_notices()
}

/// Whole-app sign-out: revoke every signed-in community session server-side
/// (best effort, bounded) and forget them in memory. The keyring itself is
/// wiped by the reset that follows.
pub(crate) async fn logout_all_best_effort(state: &AppState) {
    for (origin, session) in state.token_auth.active_sessions() {
        let call = api::logout(&state.http_client, &origin, &session.access);
        match tokio::time::timeout(std::time::Duration::from_secs(5), call).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("buzz-desktop: auth: sign-out of {origin}: {error}"),
            Err(_) => eprintln!("buzz-desktop: auth: sign-out of {origin} timed out"),
        }
    }
    // Wait out any in-flight refresh so none writes a token after the reset
    // wipes the keyring (each one is generation-fenced after this).
    let mut guards = Vec::new();
    for origin in state.token_auth.known_origins() {
        guards.push(state.token_auth.refresh_lock(&origin).lock_owned().await);
    }
    state.token_auth.clear_all();
    drop(guards);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_token_login_preserves_existing_key_agents_and_allows_token_bots() {
        let origin = "https://relay.test";
        let mut record: crate::managed_agents::ManagedAgentRecord =
            serde_json::from_value(serde_json::json!({
                "pubkey": nostr::Keys::generate().public_key().to_hex(),
                "name": "Scout", "relay_url": "wss://relay.test", "acp_command": "buzz-acp",
                "agent_command": "goose", "agent_args": [], "mcp_command": "",
                "turn_timeout_seconds": 320, "system_prompt": null,
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
            }))
            .unwrap();
        let before = serde_json::to_value(&record).unwrap();
        assert!(
            ensure_existing_token_agents(origin, std::slice::from_ref(&record)).is_err(),
            "the compatibility route must not expose key agents to token adoption"
        );
        assert_eq!(serde_json::to_value(&record).unwrap(), before);
        record.relay_url = "wss://old-community.test".into();
        assert!(
            ensure_existing_token_agents(origin, std::slice::from_ref(&record)).is_err(),
            "legacy relay pins are ignored by production agent fan-out"
        );
        record.bot_origin = Some(origin.into());
        assert!(ensure_existing_token_agents(origin, std::slice::from_ref(&record)).is_ok());
        assert!(ensure_existing_token_agents(origin, &[]).is_ok());
        // A key record created while the browser was open must fail the same
        // guard when the final store-locked login commit rechecks the records.
        record.bot_origin = None;
        assert!(ensure_existing_token_agents(origin, &[record]).is_err());
    }

    struct LoginStore {
        binding: Option<PublicKey>,
        refresh: Option<String>,
    }

    impl RefreshStore for LoginStore {
        fn load(&self, _: &str) -> Result<Option<String>, String> {
            Ok(self.refresh.clone())
        }
        fn store(&self, _: &str, _: &str) -> Result<(), String> {
            panic!("selection must not change stored credentials")
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            panic!("selection must preserve stored credentials")
        }
        fn signing_pubkey(&self, _: &str) -> Result<Option<PublicKey>, String> {
            Ok(self.binding)
        }
    }

    #[test]
    fn google_relogin_keeps_known_token_accounts_on_dual_mode_relays() {
        let store = LoginStore {
            binding: None,
            refresh: None,
        };
        for state in [
            OriginAuth::Restoring,
            OriginAuth::NeedsLogin("token_expired".into()),
            OriginAuth::Active(UserSession {
                principal: nostr::Keys::generate().public_key(),
                device_id: None,
                access: Zeroizing::new("test-access".into()),
                refresh: Zeroizing::new("test-refresh".into()),
                access_issued_at: 0,
                access_expires_at: i64::MAX,
            }),
        ] {
            let auth = super::super::TokenAuthState::default();
            auth.record_backup_support("https://relay.test", true);
            auth.set("https://relay.test", state);
            assert!(
                !login_uses_key_backup(&auth, "https://relay.test", &store).unwrap(),
                "known token re-login must not select a different account mode"
            );
        }
    }

    #[test]
    fn google_relogin_backup_marker_wins_even_when_discovery_is_unavailable() {
        let auth = super::super::TokenAuthState::default();
        let pubkey = nostr::Keys::generate().public_key();
        let store = LoginStore {
            binding: Some(pubkey),
            refresh: Some("test-refresh".into()),
        };
        assert!(
            login_uses_key_backup(&auth, "https://relay.test", &store).unwrap(),
            "discovery failure must not choose token login for a backed-up key"
        );
        assert_eq!(store.binding, Some(pubkey));
        assert_eq!(store.refresh.as_deref(), Some("test-refresh"));
    }

    #[test]
    fn google_relogin_fresh_accounts_prefer_backup_and_stored_token_accounts_keep_their_mode() {
        let auth = super::super::TokenAuthState::default();
        auth.record_backup_support("https://relay.test", true);
        let fresh = LoginStore {
            binding: None,
            refresh: None,
        };
        assert!(login_uses_key_backup(&auth, "https://relay.test", &fresh).unwrap());
        let legacy = LoginStore {
            binding: None,
            refresh: Some("test-refresh".into()),
        };
        assert!(!login_uses_key_backup(&auth, "https://relay.test", &legacy).unwrap());
    }

    #[test]
    fn google_relogin_rejects_cross_mode_completion_before_session_storage() {
        assert!(verify_login_mode(false, Some("key_backup")).is_err());
        assert!(verify_login_mode(true, Some("token")).is_err());
        assert!(verify_login_mode(true, None).is_err());
        assert!(verify_login_mode(false, None).is_ok());
        assert!(verify_login_mode(false, Some("token")).is_ok());
        assert!(verify_login_mode(true, Some("key_backup")).is_ok());
    }

    #[test]
    fn google_relogin_explicit_legacy_route_cannot_replace_a_backup_binding() {
        let auth = super::super::TokenAuthState::default();
        let origin = "https://relay.test";
        auth.record_backup_support(origin, true);
        let fresh = LoginStore {
            binding: None,
            refresh: None,
        };
        assert!(!requested_login_mode(&auth, origin, &fresh, true).unwrap());
        assert!(requested_login_mode(&auth, origin, &fresh, false).unwrap());
        let bound = LoginStore {
            binding: Some(nostr::Keys::generate().public_key()),
            refresh: None,
        };
        assert!(requested_login_mode(&auth, origin, &bound, true).is_err());
        assert!(requested_login_mode(&auth, origin, &bound, false).unwrap());
    }

    #[tokio::test]
    async fn google_relogin_after_logout_and_app_restart_keeps_legacy_mode() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        };
        struct LogoutStore {
            refresh: Mutex<Option<String>>,
            legacy: AtomicBool,
        }
        impl RefreshStore for LogoutStore {
            fn load(&self, _: &str) -> Result<Option<String>, String> {
                Ok(self.refresh.lock().unwrap().clone())
            }
            fn store(&self, _: &str, _: &str) -> Result<(), String> {
                panic!("logout cannot create a credential")
            }
            fn delete(&self, _: &str) -> Result<(), String> {
                self.refresh.lock().unwrap().take();
                Ok(())
            }
            fn legacy_token_account(&self, _: &str) -> Result<bool, String> {
                Ok(self.legacy.load(Ordering::SeqCst))
            }
            fn remember_legacy_token_account(&self, _: &str) -> Result<(), String> {
                self.legacy.store(true, Ordering::SeqCst);
                Ok(())
            }
        }
        let origin = "https://relay.test";
        let store = LogoutStore {
            refresh: Mutex::new(Some("test-refresh".into())),
            legacy: AtomicBool::new(false),
        };
        let auth = super::super::TokenAuthState::default();
        auth.set(origin, OriginAuth::NeedsLogin("token_expired".into()));
        forget_session(&auth, origin, &store).await.unwrap();
        assert!(store.load(origin).unwrap().is_none());
        let restarted = super::super::TokenAuthState::default();
        restarted.record_backup_support(origin, true);
        assert!(
            !login_uses_key_backup(&restarted, origin, &store).unwrap(),
            "logging out deletes credentials, not knowledge of the existing account mode"
        );
    }

    #[test]
    fn status_shape() {
        let signed_out = status_of("http://o", Some(vec!["google".into()]), None);
        assert!(signed_out.supported);
        assert_eq!(signed_out.state, "signed_out");
        let unsupported = status_of("http://o", None, None);
        assert!(!unsupported.supported);
        let needs = status_of(
            "http://o",
            Some(vec![]),
            Some(OriginAuth::NeedsLogin("refresh_reused".into())),
        );
        assert_eq!(needs.state, "needs_login");
        assert_eq!(needs.reason.as_deref(), Some("refresh_reused"));
        let json = serde_json::to_value(&needs).unwrap();
        assert!(json.get("deviceId").is_some(), "camelCase wire shape");
    }
}
