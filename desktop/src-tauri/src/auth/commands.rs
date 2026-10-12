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
    /// The active account is signed out on this computer: chats stay closed
    /// until an account signs in. Keys and data are kept.
    pub device_signed_out: bool,
    /// This sign-in opened a different account than the one active before
    /// (another Google account, or a "connect" whose Google account already
    /// held an account).
    pub account_changed: bool,
    /// The account a sign-out of the active account switched to (hex).
    pub switched_to: Option<String>,
    /// Agents a sign-out could not confirm stopped (still stopping, or the
    /// stop failed). The sign-out itself went through.
    pub agents_not_stopped: usize,
    /// The chosen Google account has no Buzz account yet: nothing was
    /// created; the UI asks, then calls `confirm_signup` or `cancel_signup`.
    pub signup: Option<SignupPrompt>,
}

/// What the "Create your Buzz account" screen shows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignupPrompt {
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// How long a confirmation may wait; the relay keeps its ticket 10 minutes.
const SIGNUP_CONFIRM_WINDOW: std::time::Duration = std::time::Duration::from_secs(9 * 60);

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
        device_signed_out: false,
        switched_to: None,
        agents_not_stopped: 0,
        account_changed: false,
        signup: None,
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
    status.device_signed_out = state.token_auth.device_signed_out(origin);
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
            // A signed-out device must keep its only way back (Rule 6).
            let signed_out = state.token_auth.device_signed_out(origin);
            (has_session || has_stored || signed_out).then(|| vec!["google".to_owned()])
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
        "Kuzz Desktop".to_owned()
    } else {
        format!("Kuzz Desktop ({})", host.trim())
    }
}

/// Sign in to the current community with Google: system browser + loopback
/// redirect + PKCE (plan §3.2). Resolves once the session is active.
///
/// The chosen Google account decides the Buzz account; it becomes the active
/// account and accounts already signed in here stay signed in. `connect`
/// links the current key to the Google account instead (a key-only account
/// protecting itself). `allow_restore` is accepted for older callers and no
/// longer changes anything: a backed-up account is always restored.
#[tauri::command]
pub async fn login_with_google(
    app: AppHandle,
    allow_restore: Option<bool>,
    existing_token_account: Option<bool>,
    attempt_id: Option<String>,
    connect: Option<bool>,
) -> Result<TokenAuthStatus, String> {
    let _ = allow_restore;
    let state = app.state::<AppState>();
    let id = attempt_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    if id.is_empty() || id.len() > 128 {
        return Err("invalid sign-in attempt".into());
    }
    let mut attempt = state.token_auth.login.begin(id.clone())?;
    // A new sign-in replaces one still waiting for confirmation.
    drop(state.token_auth.take_signup());
    let _ = app.emit(
        "google-login-progress",
        serde_json::json!({"attemptId": id, "phase": "waiting"}),
    );
    login_inner(
        &app,
        connect == Some(true),
        existing_token_account == Some(true),
        &id,
        &mut attempt,
    )
    .await
}

/// Create the Buzz account the user confirmed on the "Create your Buzz
/// account" screen, then finish signing in to it.
#[tauri::command]
pub async fn confirm_signup(app: AppHandle) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let expired = "This sign-in has expired. Sign in with Google again.";
    let pending = state.token_auth.take_signup().ok_or(expired)?;
    let origin = state.current_auth_origin();
    if pending.origin != origin || pending.parked_at.elapsed() > SIGNUP_CONFIRM_WINDOW {
        return Err(expired.into());
    }
    // One sign-in at a time, like a browser attempt.
    let _attempt = state
        .token_auth
        .login
        .begin(uuid::Uuid::new_v4().to_string())?;
    let tokens = api::signup(
        &state.http_client,
        &origin,
        &pending.ticket,
        &pending.verifier,
    )
    .await
    .map_err(|e| format!("your account could not be created: {e}"))?;
    finish_sign_in(
        &app,
        &origin,
        pending.providers,
        true,
        false,
        pending.existing_token_account,
        tokens,
    )
    .await
}

/// Drop a sign-in waiting for confirmation ("Use a different account", or
/// the screen closed). The relay created nothing; its ticket expires.
#[tauri::command]
pub fn cancel_signup(app: AppHandle) {
    drop(app.state::<AppState>().token_auth.take_signup());
}

/// Cancel only this browser attempt; completed or unrelated attempts are untouched.
#[tauri::command]
pub fn cancel_google_login(app: AppHandle, attempt_id: String) -> bool {
    app.state::<AppState>().token_auth.login.cancel(&attempt_id)
}

async fn login_inner(
    app: &AppHandle,
    connect: bool,
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
                super::install_id::load_or_create(&app).as_deref(),
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
                // Ask before an unknown Google account becomes an account.
                // Connecting a key-only account is that account: no prompt.
                if !connect {
                    url.push_str("&signup=confirm");
                }
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
    let tokens = match api::complete_login(&state.http_client, &origin, &code, &pkce.verifier)
        .await
        .map_err(|e| format!("sign-in could not be completed: {e}"))?
    {
        api::CompleteResponse::Tokens(tokens) => tokens,
        api::CompleteResponse::NeedsSignup(signup) => {
            if !backup || connect {
                return Err("sign-in could not be completed: unexpected new-account step".into());
            }
            state.token_auth.park_signup(super::PendingSignup {
                origin: origin.clone(),
                providers: providers.clone(),
                existing_token_account,
                verifier: Zeroizing::new(pkce.verifier.to_string()),
                ticket: signup.signup_token.0.clone(),
                parked_at: std::time::Instant::now(),
            });
            let mut status = current_status(&state, &origin, Some(providers));
            status.signup = Some(SignupPrompt {
                email: signup.email,
                display_name: signup.display_name,
                avatar_url: signup.avatar_url,
            });
            return Ok(status);
        }
    };
    finish_sign_in(
        app,
        &origin,
        providers,
        backup,
        connect,
        existing_token_account,
        tokens,
    )
    .await
}

/// The end of a sign-in once the relay issued its session: key recovery,
/// session storage, account switch, agents.
async fn finish_sign_in(
    app: &AppHandle,
    origin: &str,
    providers: Vec<String>,
    backup: bool,
    connect: bool,
    existing_token_account: bool,
    tokens: api::LoginTokens,
) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = origin.to_owned();
    verify_login_mode(backup, tokens.identity_mode.as_deref())?;
    let principal = PublicKey::from_hex(&tokens.principal_id)
        .map_err(|_| "Couldn't sign in. Try again.".to_string())?;
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
    let previous_active = state.keys.lock().ok().map(|keys| keys.public_key());
    let mut signed_in_account = None;
    if backup {
        match super::key_recovery::complete(app, &origin, &tokens, connect).await {
            Ok(pubkey) => signed_in_account = Some(pubkey),
            Err(error) => {
                // The session is never stored on failure: revoke it
                // server-side instead of leaving it alive.
                if let Err(revoke) = api::logout(&state.http_client, &origin, &tokens.access).await
                {
                    eprintln!("buzz-desktop: auth: revoking a failed key-backup sign-in: {revoke}");
                }
                return Err(error);
            }
        }
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
    // This account is signed in here now; other accounts' sign-outs stay. A
    // token account's marker names no account (or, from older builds, its
    // principal).
    let owner = signed_in_account.unwrap_or(principal);
    let resume = state
        .token_auth
        .signed_out
        .stopped_agents(&origin, Some(owner));
    state
        .token_auth
        .signed_out
        .clear(&origin, signed_in_account.or(Some(principal)))?;
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
    // Other servers are handed over while this origin's refresh lock is still
    // held: an account switch waits on it, so it can never land between this
    // sign-in and its hand-over (which would hand the servers to the wrong
    // account).
    let mut account_changed = false;
    if let (Some(previous), Some(current)) = (previous_active, signed_in_account) {
        if previous != current {
            account_changed = true;
            super::accounts::after_active_changed(app, &origin, previous, current).await;
        }
    }
    drop(_refresh_guard);
    spawn_refresh_loop(app, &origin, generation);
    if let Some(account) = signed_in_account {
        remember_signed_in_profile(app, &origin, account, &tokens);
    }
    emit_changed(app, &origin);
    // Signed in here again: like an app launch for this account, its agents
    // start (outside every sign-in lock; each start takes its own).
    start_owner_agents(app, owner, resume);
    let mut status = current_status(&state, &origin, Some(providers));
    status.account_changed = account_changed;
    Ok(status)
}

/// Record a sign-in of `account` for the account menu: used now, its email
/// (from relays that send it), and in the background its `/auth/me` name and
/// avatar. Best effort.
fn remember_signed_in_profile(
    app: &AppHandle,
    origin: &str,
    account: PublicKey,
    tokens: &api::LoginTokens,
) {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    super::account_profiles::touch(&data_dir, &account);
    super::account_profiles::learn(&data_dir, &account, None, None, tokens.email.clone());
    let client = app.state::<AppState>().http_client.clone();
    let origin = origin.to_owned();
    let access = Zeroizing::new(tokens.access.0.to_string());
    tauri::async_runtime::spawn(async move {
        match api::me(&client, &origin, &access).await {
            Ok(me) => super::account_profiles::learn(
                &data_dir,
                &account,
                me.display_name,
                me.avatar_url,
                None,
            ),
            Err(error) => eprintln!("buzz-desktop: accounts: profile not fetched: {error}"),
        }
    });
}

/// Start `owner`'s local agents that start with the app, plus `resume` (the
/// ones its last sign-out stopped), on the current community. In the
/// background; failures are logged per agent.
fn start_owner_agents(app: &AppHandle, owner: PublicKey, resume: Vec<String>) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let relay_url = crate::relay::relay_ws_url_with_override(&state);
        let records = match crate::managed_agents::load_managed_agents(&app) {
            Ok(records) => records,
            Err(error) => {
                eprintln!("buzz-desktop: auth: agents not started after sign-in: {error}");
                return;
            }
        };
        for pubkey in owner_agents_to_start(&records, &owner.to_hex(), &resume, |record| {
            crate::managed_agents::agent_owner_hex(&app, record)
        }) {
            if let Err(error) = crate::managed_agents::start_managed_agent_runtime(
                pubkey.clone(),
                relay_url.clone(),
                app.clone(),
            ) {
                eprintln!("buzz-desktop: auth: agent {pubkey} not started after sign-in: {error}");
            }
        }
    });
}

/// The local agents of `owner` to start when it signs in: its auto-start
/// agents and the ones its sign-out stopped. Pure.
fn owner_agents_to_start(
    records: &[crate::managed_agents::ManagedAgentRecord],
    owner: &str,
    resume: &[String],
    owner_of: impl Fn(&crate::managed_agents::ManagedAgentRecord) -> Option<String>,
) -> Vec<String> {
    records
        .iter()
        .filter(|record| record.backend == crate::managed_agents::BackendKind::Local)
        .filter(|record| {
            record.start_on_app_launch
                || resume
                    .iter()
                    .any(|stopped| stopped.eq_ignore_ascii_case(&record.pubkey))
        })
        .filter(|record| owner_of(record).is_some_and(|o| o.eq_ignore_ascii_case(owner)))
        .map(|record| record.pubkey.clone())
        .collect()
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
        return Err("This account can't be added here. Your current account is unchanged.".into());
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
        RefreshFailure::Transient => "Couldn't renew your sign-in. Check your connection.".into(),
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
            RefreshFailure::Transient => {
                "Couldn't renew your sign-in. Check your connection.".into()
            }
            RefreshFailure::Terminal(_) => {
                "your sign-in for this community has ended; sign in with Google again".into()
            }
        })
}

/// The relay-socket credential mode for `origin`; refused outright while the
/// device is signed out there, so no frontend connection can authenticate.
pub(crate) fn ws_auth_mode(
    auth: &super::TokenAuthState,
    origin: &str,
) -> Result<CredentialMode, String> {
    if auth.device_signed_out(origin) {
        return Err(super::signed_out::SIGNED_OUT_MESSAGE.into());
    }
    Ok(auth.mode(origin))
}

/// WebSocket AUTH payload for the current community: `{"token": …}` in token
/// mode, `null` in key mode (the caller signs a NIP-42 event instead).
#[tauri::command]
pub async fn get_ws_auth_frame(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    match ws_auth_mode(&state.token_auth, &origin)? {
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

/// Sign `account` (default: the active account) out of the current community
/// on this computer: revoke its session server-side, durably mark it signed
/// out, stop its agents, and when it was the active account switch to another
/// account still signed in here (`switchedTo`), else the signed-out screen.
/// Keys, communities and history are never deleted. A network failure keeps
/// everything so the user can retry (the server-side session would otherwise
/// stay alive unrevoked).
#[tauri::command]
pub async fn logout(app: AppHandle, account: Option<String>) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let account = account
        .map(|hex| PublicKey::from_hex(&hex).map_err(|_| "invalid account".to_string()))
        .transpose()?;
    let signed_out = sign_out_account(&app, &origin, account, true).await?;
    let support = state.token_auth.cached_support(&origin).flatten();
    let mut status = current_status(&state, &origin, support);
    status.switched_to = signed_out.switched_to.map(|key| key.to_hex());
    status.agents_not_stopped = signed_out.agents_not_stopped;
    Ok(status)
}

/// Sign `account` out of `origin` on this computer: an agent runs while its
/// owner is signed in here, so this always stops the account's agents.
/// `revoke` ends the session server-side first (a user sign-out); a session
/// the relay already ended is not revoked again. `account` defaults to the
/// active account (`None` also covers a legacy token account). Returns the
/// account switched to when the active account signed out and another one is
/// still signed in here. The UI is told only after any switch, so it never
/// shows the signed-out screen on the way to the next account. Stopping the
/// agents is waited for at most [`SIGN_OUT_STOP_LIMIT`]: a stop that does not
/// finish is reported, never a reason to leave the sign-out half done.
pub(crate) async fn sign_out_account(
    app: &AppHandle,
    origin: &str,
    account: Option<PublicKey>,
    revoke: bool,
) -> Result<SignedOut, String> {
    let state = app.state::<AppState>();
    let active = state.keys.lock().ok().map(|keys| keys.public_key());
    let data_dir = app.path().app_data_dir().ok();
    let account = account.or_else(|| state.token_auth.signing_pubkey(origin));
    let is_active = account.is_none() || account == active;
    // Its running agents on this server, recorded with the sign-out
    // (informational) and stopped below. A legacy token account's agents are
    // owned by its principal, never by the local key; found whatever the
    // session state (revoked, expired, restoring).
    let owners = match account {
        Some(account) => vec![account],
        None => crate::managed_agents::token_account_owners(app, origin),
    };
    let running = |pairs: &mut Vec<crate::managed_agents::ManagedAgentRuntimeKey>| {
        for owner in &owners {
            for pair in crate::managed_agents::running_owner_pairs(app, origin, &owner.to_hex())
                .unwrap_or_default()
            {
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
    };
    let mut pairs = Vec::new();
    running(&mut pairs);
    let stopping: Vec<String> = pairs.iter().map(|key| key.pubkey.clone()).collect();
    if is_active {
        // Let an in-flight restore or refresh settle first (sign-out is
        // offered while restoring, Rule 6), so a session it just activated is
        // revoked server-side rather than only forgotten locally.
        drop(state.token_auth.refresh_lock(origin).lock_owned().await);
        if revoke {
            match state.token_auth.get(origin) {
                Some((_, OriginAuth::Active(session))) => {
                    revoke_session(&state.http_client, origin, &session.access).await?;
                }
                // Restoring or retrying: revoke with the stored token.
                _ => {
                    if let Ok(Some(token)) = KeyringRefreshStore.load(origin) {
                        revoke_stored(&state.http_client, origin, &token, |rotated| {
                            KeyringRefreshStore.store(origin, rotated)
                        })
                        .await?;
                    }
                }
            }
        }
        sign_out_device(&state.token_auth, origin, &KeyringRefreshStore, &stopping).await?;
    } else if let Some(account) = account {
        // A background account: its session lives in its own loop.
        let held = state.token_auth.inactive.take(origin, &account).await;
        if revoke {
            let revoked = match held.flatten() {
                Some(session) => revoke_session(&state.http_client, origin, &session.access).await,
                // No live session (retrying, or none running): revoke with
                // the account's stored token.
                None => {
                    let slot = super::accounts::refresh_slot(&account, origin);
                    match super::accounts::shared_secrets().load(&slot) {
                        Ok(Some(token)) => {
                            revoke_stored(&state.http_client, origin, &token, |rotated| {
                                super::accounts::shared_secrets().store(&slot, rotated)
                            })
                            .await
                        }
                        _ => Ok(()),
                    }
                }
            };
            {
                if let Err(error) = revoked {
                    // Nothing changed locally: keep it alive for a retry.
                    state.token_auth.inactive.start(
                        &state.http_client,
                        data_dir.clone(),
                        origin,
                        account,
                    );
                    return Err(error);
                }
            }
        }
        state
            .token_auth
            .signed_out
            .mark(origin, Some(account), &stopping)?;
    }
    if let (Some(account), Some(data_dir)) = (account, data_dir.as_deref()) {
        if let Err(error) = super::accounts::mark_signed_out(
            super::accounts::shared_secrets(),
            data_dir,
            origin,
            &account,
        ) {
            eprintln!("buzz-desktop: auth: account list not updated on sign-out: {error}");
        }
    }
    // Listed again now that the sign-out is recorded: a start that passed the
    // gate just before it registers its child under the same process-map lock
    // it checked the gate under, so it is either listed here or refused.
    running(&mut pairs);
    let stop_app = app.clone();
    let attempted = pairs.len();
    let stopping = tauri::async_runtime::spawn_blocking(move || {
        crate::managed_agents::stop_pairs(&stop_app, pairs)
    });
    let agents_not_stopped = await_agent_stops(
        async { stopping.await.map_err(|error| error.to_string()) },
        attempted,
        SIGN_OUT_STOP_LIMIT,
    )
    .await;
    let next = match (is_active, data_dir.as_deref()) {
        (true, Some(data_dir)) => {
            super::accounts::next_signed_in(data_dir, origin, account.or(active))
        }
        _ => None,
    }
    .filter(|next| Some(*next) != active);
    if let Some(next) = next {
        match super::accounts::switch_account(app.clone(), next.to_hex()).await {
            Ok(Some(warning)) => {
                eprintln!("buzz-desktop: auth: switched after sign-out with: {warning}")
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("buzz-desktop: auth: no switch after sign-out: {error}");
                emit_changed(app, origin);
                return Ok(SignedOut {
                    switched_to: None,
                    agents_not_stopped,
                });
            }
        }
        return Ok(SignedOut {
            switched_to: Some(next),
            agents_not_stopped,
        });
    }
    emit_changed(app, origin);
    Ok(SignedOut {
        switched_to: None,
        agents_not_stopped,
    })
}

/// What a sign-out did besides signing the account out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SignedOut {
    /// The account switched to when the active account signed out.
    pub switched_to: Option<PublicKey>,
    /// Agents not confirmed stopped.
    pub agents_not_stopped: usize,
}

/// How long a sign-out waits for its agents to stop before going on.
const SIGN_OUT_STOP_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Wait up to `limit` for a sign-out's agent stops (`attempted` of them) and
/// return how many are not confirmed stopped: the failures reported, or all
/// of them when the stop errored or is still running (it keeps running in
/// the background and finishes on its own).
async fn await_agent_stops(
    stopping: impl std::future::Future<Output = Result<Vec<String>, String>>,
    attempted: usize,
    limit: std::time::Duration,
) -> usize {
    match tokio::time::timeout(limit, stopping).await {
        Ok(Ok(failures)) => {
            for failure in &failures {
                eprintln!("buzz-desktop: auth: agent not stopped on sign-out: {failure}");
            }
            failures.len()
        }
        Ok(Err(error)) => {
            eprintln!("buzz-desktop: auth: agents not stopped on sign-out: {error}");
            attempted
        }
        Err(_) => {
            eprintln!(
                "buzz-desktop: auth: {attempted} agent(s) still stopping after {}s; signing out anyway",
                limit.as_secs()
            );
            attempted
        }
    }
}

/// Install the hook that signs an account out when the relay ended its
/// session for good (revoked from another device, disabled, reused or expired
/// refresh token), for the active account's loop and every background one.
pub(crate) fn install_session_ended_hook(app: &AppHandle) {
    let hook_app = app.clone();
    app.state::<AppState>()
        .token_auth
        .inactive
        .set_ended_hook(std::sync::Arc::new(move |origin, account, code| {
            let app = hook_app.clone();
            tauri::async_runtime::spawn(async move {
                eprintln!("buzz-desktop: auth: the relay ended a session on {origin} ({code}); signing it out");
                if let Err(error) = sign_out_account(&app, &origin, account, false).await {
                    eprintln!("buzz-desktop: auth: sign-out after {code} failed: {error}");
                    // The UI still learns the session ended.
                    emit_changed(&app, &origin);
                }
            });
        }));
}

/// Revoke the session behind a stored refresh token: exchange it, then revoke
/// the access token. A token the relay already refuses (4xx) is a session
/// that is gone; a network failure is an error so the user can retry. The
/// exchange consumes the stored token, so the rotated one is `save`d first:
/// a retry after a failed revoke then presents a live token, not a consumed
/// one (which would trip `refresh_reused`).
async fn revoke_stored(
    client: &reqwest::Client,
    origin: &str,
    refresh: &str,
    save: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    match api::refresh(client, origin, refresh).await {
        Ok(tokens) => {
            if let Err(error) = save(&tokens.refresh.0) {
                eprintln!("buzz-desktop: auth: rotated token not kept before sign-out: {error}");
            }
            revoke_session(client, origin, &tokens.access.0).await
        }
        Err(ApiError {
            status: Some(status),
            ..
        }) if (400..500).contains(&status) && status != 429 => Ok(()),
        Err(error) => Err(format!("could not sign out: {error}")),
    }
}

/// Revoke a session server-side; already gone (401) counts as revoked.
async fn revoke_session(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
) -> Result<(), String> {
    match api::logout(client, origin, access).await {
        Ok(())
        | Err(ApiError {
            status: Some(401), ..
        }) => Ok(()),
        Err(error) => Err(format!("could not sign out: {error}")),
    }
}

/// Device sign-out of `origin`: write the durable signed-out marker first (it
/// is what fences chats and relay auth), then forget the session. A local-only
/// account that Google cannot bring back gets no marker (Rule 6).
pub(crate) async fn sign_out_device(
    auth: &super::TokenAuthState,
    origin: &str,
    store: &dyn RefreshStore,
    stop_agents: &[String],
) -> Result<(), String> {
    // Without a marker the session is still forgotten (its agents are
    // stopped by the caller); they may auto-start again on the next launch.
    if let Some(account) = signed_out_account(auth, origin, store)? {
        auth.signed_out.mark(origin, account, stop_agents)?;
    }
    forget_session(auth, origin, store).await
}

/// The agents among `pubkeys` owned by the account active on this computer.
fn own_agents(app: &AppHandle, pubkeys: Vec<String>) -> Vec<String> {
    let state = app.state::<AppState>();
    let Ok(me) = state.current_identity_pubkey().map(|key| key.to_hex()) else {
        return Vec::new();
    };
    let records =
        crate::managed_agents::storage::load_managed_agents_without_keys(app).unwrap_or_default();
    owned_by(pubkeys, &records, &me, |record| {
        crate::managed_agents::agent_owner_hex(app, record)
    })
}

/// The agents among `pubkeys` whose owner is `me`; unknown agents and agents
/// without a known owner are never included.
fn owned_by(
    pubkeys: Vec<String>,
    records: &[crate::managed_agents::ManagedAgentRecord],
    me: &str,
    owner_of: impl Fn(&crate::managed_agents::ManagedAgentRecord) -> Option<String>,
) -> Vec<String> {
    pubkeys
        .into_iter()
        .filter(|pubkey| {
            records
                .iter()
                .find(|record| record.pubkey.eq_ignore_ascii_case(pubkey))
                .and_then(&owner_of)
                .is_some_and(|owner| owner.eq_ignore_ascii_case(me))
        })
        .collect()
}

/// The running agents among `pubkeys` that the active account owns, for the
/// sign-out dialog (it must never offer to stop another account's agents).
#[tauri::command]
pub fn own_agent_pubkeys(app: AppHandle, pubkeys: Vec<String>) -> Result<Vec<String>, String> {
    Ok(own_agents(&app, normalize_agent_pubkeys(pubkeys)?))
}

/// Validate agent pubkeys from the UI: bounded, lowercase hex.
fn normalize_agent_pubkeys(agents: Vec<String>) -> Result<Vec<String>, String> {
    if agents.len() > 1024 {
        return Err("too many agents".into());
    }
    agents
        .into_iter()
        .map(|agent| {
            if agent.len() == 64 && agent.bytes().all(|b| b.is_ascii_hexdigit()) {
                Ok(agent.to_ascii_lowercase())
            } else {
                Err("invalid agent id".to_owned())
            }
        })
        .collect()
}

/// The account a sign-out marks: `Some(Some(pubkey))` for a key-backup
/// account, `Some(None)` for a legacy token account (the marker covers the
/// origin), `None` when Google does not hold this account at all.
fn signed_out_account(
    auth: &super::TokenAuthState,
    origin: &str,
    store: &dyn RefreshStore,
) -> Result<Option<Option<PublicKey>>, String> {
    if let Some(bound) = auth.signing_pubkey(origin) {
        return Ok(Some(Some(bound)));
    }
    if let Some(bound) = store.signing_pubkey(origin)? {
        return Ok(Some(Some(bound)));
    }
    Ok(match auth.get(origin) {
        Some((_, OriginAuth::Active(_) | OriginAuth::Restoring | OriginAuth::NeedsLogin(_))) => {
            Some(None)
        }
        _ if store.legacy_token_account(origin)? || store.load(origin)?.is_some() => Some(None),
        _ => None,
    })
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

/// Rename one of this account's devices (any device, not only this one).
#[tauri::command]
pub async fn rename_device(
    app: AppHandle,
    device_id: String,
    name: String,
) -> Result<api::DeviceInfo, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::rename_device(
        &state.http_client,
        &origin,
        &session.access,
        &device_id,
        &name,
    )
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

    #[tokio::test(start_paused = true)]
    async fn a_sign_out_does_not_wait_forever_for_its_agents() {
        // The stop that never finishes (a harness that would not exit): the
        // sign-out goes on after the limit and reports every agent.
        let limit = std::time::Duration::from_secs(10);
        let started = tokio::time::Instant::now();
        let not_stopped = await_agent_stops(
            std::future::pending::<Result<Vec<String>, String>>(),
            2,
            limit,
        )
        .await;
        assert_eq!(not_stopped, 2);
        assert_eq!(started.elapsed(), limit);
    }

    #[tokio::test]
    async fn a_sign_out_reports_only_the_agents_that_did_not_stop() {
        let limit = std::time::Duration::from_secs(10);
        assert_eq!(await_agent_stops(async { Ok(vec![]) }, 3, limit).await, 0);
        assert_eq!(
            await_agent_stops(async { Ok(vec!["a: stuck".to_string()]) }, 3, limit).await,
            1
        );
        assert_eq!(
            await_agent_stops(async { Err("join failed".to_string()) }, 3, limit).await,
            3
        );
    }

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

    struct SignOutStore {
        binding: Option<PublicKey>,
        refresh: std::sync::Mutex<Option<String>>,
    }

    impl RefreshStore for SignOutStore {
        fn load(&self, _: &str) -> Result<Option<String>, String> {
            Ok(self.refresh.lock().unwrap().clone())
        }
        fn store(&self, _: &str, _: &str) -> Result<(), String> {
            panic!("sign-out cannot create a credential")
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            self.refresh.lock().unwrap().take();
            Ok(())
        }
        fn signing_pubkey(&self, _: &str) -> Result<Option<PublicKey>, String> {
            Ok(self.binding)
        }
    }

    fn loaded_auth(dir: &tempfile::TempDir) -> super::super::TokenAuthState {
        let auth = super::super::TokenAuthState::default();
        auth.signed_out.load(dir.path());
        auth
    }

    #[tokio::test]
    async fn sign_out_marks_the_device_and_keeps_the_key_binding() {
        let dir = tempfile::tempdir().unwrap();
        let origin = "https://relay.test";
        let identity = nostr::Keys::generate().public_key();
        let store = SignOutStore {
            binding: Some(identity),
            refresh: std::sync::Mutex::new(Some("test-refresh".into())),
        };
        let auth = loaded_auth(&dir);
        let agent = "cd".repeat(32);
        sign_out_device(&auth, origin, &store, std::slice::from_ref(&agent))
            .await
            .unwrap();
        assert!(auth.device_signed_out_with(origin, &store));
        assert!(auth.signed_out.suppresses_agent(&agent));
        assert!(store.load(origin).unwrap().is_none(), "session forgotten");
        assert_eq!(store.binding, Some(identity), "key binding kept");
        // Durable: a restarted app is still signed out.
        assert!(loaded_auth(&dir).device_signed_out_with(origin, &store));
        // Another account signing in here leaves this account signed out and
        // its stopped agents stopped; this account signing in clears both.
        let other = nostr::Keys::generate().public_key();
        auth.signed_out.clear(origin, Some(other)).unwrap();
        assert!(auth.signed_out.is_signed_out(origin, Some(identity)));
        assert!(!auth.signed_out.is_signed_out(origin, Some(other)));
        assert!(auth.signed_out.suppresses_agent(&agent));
        auth.signed_out.clear(origin, Some(identity)).unwrap();
        assert!(!loaded_auth(&dir).device_signed_out_with(origin, &store));
        assert!(!loaded_auth(&dir).signed_out.suppresses_agent(&agent));
    }

    #[tokio::test]
    async fn sign_out_of_an_active_token_account_covers_its_origin() {
        let dir = tempfile::tempdir().unwrap();
        let origin = "https://relay.test";
        let principal = nostr::Keys::generate().public_key();
        let auth = loaded_auth(&dir);
        auth.set(
            origin,
            OriginAuth::Active(UserSession {
                principal,
                device_id: None,
                access: Zeroizing::new("test-access".into()),
                refresh: Zeroizing::new("test-refresh".into()),
                access_issued_at: 0,
                access_expires_at: i64::MAX,
            }),
        );
        struct TokenStore(std::sync::atomic::AtomicBool);
        impl RefreshStore for TokenStore {
            fn load(&self, _: &str) -> Result<Option<String>, String> {
                Ok(None)
            }
            fn store(&self, _: &str, _: &str) -> Result<(), String> {
                unreachable!()
            }
            fn delete(&self, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn remember_legacy_token_account(&self, _: &str) -> Result<(), String> {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
        }
        sign_out_device(
            &auth,
            origin,
            &TokenStore(std::sync::atomic::AtomicBool::new(false)),
            &[],
        )
        .await
        .unwrap();
        // A token account has no local key: the marker covers the origin.
        assert!(auth.signed_out.is_signed_out(origin, None));
        assert!(auth.signed_out.is_signed_out(origin, Some(principal)));
        assert!(matches!(auth.get(origin), Some((_, OriginAuth::SignedOut))));
    }

    #[tokio::test]
    async fn local_only_accounts_get_no_marker_so_they_are_never_locked_out() {
        let dir = tempfile::tempdir().unwrap();
        let origin = "https://relay.test";
        let store = SignOutStore {
            binding: None,
            refresh: std::sync::Mutex::new(None),
        };
        let auth = loaded_auth(&dir);
        sign_out_device(&auth, origin, &store, &[]).await.unwrap();
        assert!(!auth.device_signed_out_with(origin, &store));
        // With running agents to stop: still signs out, still no marker.
        sign_out_device(&auth, origin, &store, &["ab".repeat(32)])
            .await
            .unwrap();
        assert!(!auth.device_signed_out_with(origin, &store));
        assert!(!auth.signed_out.suppresses_agent(&"ab".repeat(32)));
    }

    #[test]
    fn ws_auth_and_user_signing_are_refused_while_signed_out() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::app_state::build_app_state();
        state.token_auth.signed_out.load(dir.path());
        let origin = state.current_auth_origin();
        assert!(matches!(
            ws_auth_mode(&state.token_auth, &origin),
            Ok(CredentialMode::Keys)
        ));
        assert!(state.user_credential().is_ok());
        let identity = state.keys.lock().unwrap().public_key();
        state.token_auth.set_key_backup(&origin, identity);
        state
            .token_auth
            .signed_out
            .mark(&origin, Some(identity), &[])
            .unwrap();
        let message = super::super::signed_out::SIGNED_OUT_MESSAGE;
        assert_eq!(
            ws_auth_mode(&state.token_auth, &origin).err().as_deref(),
            Some(message)
        );
        assert_eq!(state.user_credential().err().as_deref(), Some(message));
        assert_eq!(state.signing_keys().err().as_deref(), Some(message));
        assert!(state.native_auth().is_err());
        // The retained key itself is untouched.
        assert_eq!(state.keys.lock().unwrap().public_key(), identity);
        state
            .token_auth
            .signed_out
            .clear(&origin, Some(identity))
            .unwrap();
        assert!(ws_auth_mode(&state.token_auth, &origin).is_ok());
        assert!(state.signing_keys().is_ok());
    }

    #[test]
    fn stop_agent_ids_are_validated() {
        assert_eq!(
            normalize_agent_pubkeys(vec!["AB".repeat(32)]).unwrap(),
            vec!["ab".repeat(32)]
        );
        assert!(normalize_agent_pubkeys(vec!["nope".into()]).is_err());
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
        assert_eq!(json.get("deviceSignedOut"), Some(&serde_json::json!(false)));
    }

    #[test]
    fn sign_out_only_stops_the_active_accounts_own_agents() {
        let record = |pubkey: &str| -> crate::managed_agents::ManagedAgentRecord {
            serde_json::from_value(serde_json::json!({
                "pubkey": pubkey, "name": "a", "relay_url": "wss://relay.test",
                "acp_command": "x", "agent_command": "x", "agent_args": [],
                "mcp_command": "x", "turn_timeout_seconds": 1,
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
            }))
            .unwrap()
        };
        let (mine, theirs, unowned) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        let me = "1".repeat(64);
        let records = [record(&mine), record(&theirs), record(&unowned)];
        let owner_of = |r: &crate::managed_agents::ManagedAgentRecord| match r.pubkey.as_str() {
            p if p == mine => Some(me.to_ascii_uppercase()),
            p if p == theirs => Some("2".repeat(64)),
            _ => None,
        };
        let asked = vec![
            mine.clone(),
            theirs.clone(),
            unowned.clone(),
            "d".repeat(64),
        ];
        assert_eq!(owned_by(asked, &records, &me, owner_of), vec![mine]);
    }

    #[test]
    fn signing_in_starts_that_accounts_auto_start_and_stopped_agents_only() {
        let record = |pubkey: &str, auto: bool| -> crate::managed_agents::ManagedAgentRecord {
            let mut record: crate::managed_agents::ManagedAgentRecord =
                serde_json::from_value(serde_json::json!({
                    "pubkey": pubkey, "name": "a", "relay_url": "wss://relay.test",
                    "acp_command": "x", "agent_command": "x", "agent_args": [],
                    "mcp_command": "x", "turn_timeout_seconds": 1,
                    "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
                }))
                .unwrap();
            record.start_on_app_launch = auto;
            record
        };
        let (auto, stopped, idle, theirs) = (
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
            "d".repeat(64),
        );
        let me = "1".repeat(64);
        let records = [
            record(&auto, true),
            record(&stopped, false),
            record(&idle, false),
            record(&theirs, true),
        ];
        let owner_of = |r: &crate::managed_agents::ManagedAgentRecord| {
            Some(if r.pubkey == theirs {
                "2".repeat(64)
            } else {
                me.clone()
            })
        };
        assert_eq!(
            owner_agents_to_start(&records, &me, &[stopped.to_ascii_uppercase()], owner_of),
            vec![auto, stopped]
        );
    }
}
