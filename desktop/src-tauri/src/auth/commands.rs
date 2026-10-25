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
    /// Whether the relay advertises token auth (NIP-11 `kuzz_token_auth`).
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
    /// The chosen Google account has no Kuzz account yet: nothing was
    /// created; the UI asks, then calls `confirm_signup` or `cancel_signup`.
    pub signup: Option<SignupPrompt>,
}

/// What the "Create your Kuzz account" screen shows.
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
    status.device_signed_out =
        state.token_auth.device_signed_out(origin) || state.held_key_unpaired(origin);
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

/// Whether the relay refused this build as too old during this run (the
/// "Update Kuzz" screen may have mounted after the event).
#[tauri::command]
pub fn auth_update_required() -> bool {
    api::update_required_seen()
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

/// Sign in to the current community with Google: system browser + loopback
/// redirect + PKCE (plan §3.2). Resolves once the session is active.
///
/// The chosen Google account decides the Kuzz account; it becomes the active
/// account and accounts already signed in here stay signed in. `connect`
/// links the current key to the Google account instead (a key-only account
/// protecting itself). `allow_restore` is accepted for older callers and no
/// longer changes anything: a backed-up account is always restored.
/// `provider`: "google" (default) or "apple", when the community offers it.
#[tauri::command]
pub async fn login_with_google(
    app: AppHandle,
    allow_restore: Option<bool>,
    existing_token_account: Option<bool>,
    attempt_id: Option<String>,
    connect: Option<bool>,
    provider: Option<String>,
) -> Result<TokenAuthStatus, String> {
    let _ = allow_restore;
    let provider = match provider.as_deref() {
        None | Some("google") => "google",
        Some("apple") => "apple",
        Some(_) => return Err("unknown sign-in method".into()),
    };
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
        provider,
        connect == Some(true),
        existing_token_account == Some(true),
        &id,
        &mut attempt,
    )
    .await
}

/// Create the Kuzz account the user confirmed on the "Create your Kuzz
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
    let device = crate::device_key::ensure_within(crate::device_key::USER_WAIT).await;
    let tokens = api::signup(
        &state.http_client,
        &origin,
        &pending.ticket,
        &pending.verifier,
        device.as_ref(),
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
    provider: &str,
    connect: bool,
    existing_token_account: bool,
    attempt_id: &str,
    attempt: &mut super::login_attempt::LoginAttempt,
) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    // The device key named at start and bound at completion; none: as before.
    let device = crate::device_key::ensure_within(crate::device_key::USER_WAIT).await;
    let mut restarted = false;
    let (providers, pkce, backup, completed) = loop {
        let (providers, pkce, backup, code) = attempt
            .wait(async {
                let unsupported = if provider == "apple" {
                    "this community does not offer Sign in with Apple"
                } else {
                    "this community does not offer Google sign-in"
                };
                let providers = support_for(&state, &origin).await.ok_or(unsupported)?;
                if !providers.iter().any(|p| p == provider) {
                    return Err(unsupported.into());
                }
                let pkce = super::pkce::pkce_pair()?;
                let oauth_state = super::pkce::new_state()?;
                let listener = LoopbackListener::bind().await.map_err(|e| e.to_string())?;
                let mut url = api::start_url(
                    &origin,
                    provider,
                    &oauth_state,
                    &pkce.challenge,
                    &listener.redirect_uri(),
                    &super::install_id::device_name(),
                    super::install_id::load_or_create(&app).as_deref(),
                    device.as_ref(),
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
                    .map_err(|e| {
                        let refused =
                            super::loopback::LoopbackError::Denied(api::UPDATE_REQUIRED.into());
                        if e == refused {
                            api::note_update_required();
                        }
                        e.to_string()
                    })?;
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
                eprintln!("kuzz-desktop: could not activate sign-in window: {error}");
            }
        }
        let completed = api::complete_login(
            &state.http_client,
            &origin,
            &code,
            &pkce.verifier,
            device.as_ref(),
        )
        .await;
        // The relay refused the device proof's time (the login code is spent):
        // its clock is now known, so sign in once more from the start.
        if let Err(error) = &completed {
            if !restarted && error.code.as_deref() == Some("device_proof_stale") && attempt.reopen()
            {
                restarted = true;
                let _ = app.emit(
                    "google-login-progress",
                    serde_json::json!({"attemptId": attempt_id, "phase": "waiting"}),
                );
                continue;
            }
        }
        break (providers, pkce, backup, completed);
    };
    let tokens = match completed.map_err(|e| format!("sign-in could not be completed: {e}"))? {
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
    let principal = PublicKey::from_hex(&tokens.principal_id).map_err(|e| {
        eprintln!("kuzz-desktop: the relay returned an invalid account id: {e}");
        "Couldn't sign in. Try again.".to_string()
    })?;
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
                // The session is never stored on failure: end it
                // server-side instead of leaving it alive. Only this
                // session: a logout would also sign out another session of
                // this account on this device.
                if let Err(revoke) =
                    api::end_session(&state.http_client, &origin, &tokens.access).await
                {
                    eprintln!("kuzz-desktop: auth: revoking a failed key-backup sign-in: {revoke}");
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
            eprintln!("kuzz-desktop: auth: could not store the session in the keyring: {error}");
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
    // This device's agent keys that left at the last sign-out come back
    // first (the sign-in is fresh, as the relay requires).
    if backup {
        // Keys that come back only after the agents started: start those.
        let late_app = app.clone();
        let late_resume = resume.clone();
        super::agent_hosting::restore_after_sign_in(
            app,
            &origin,
            owner,
            Box::new(move |restored| {
                start_owner_agents_among(&late_app, owner, late_resume, Some(restored));
            }),
        )
        .await;
    }
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
                me.email,
            ),
            Err(error) => eprintln!("kuzz-desktop: accounts: profile not fetched: {error}"),
        }
    });
}

/// Start `owner`'s local agents that start with the app, plus `resume` (the
/// ones its last sign-out stopped), on the current community. In the
/// background; failures are logged per agent.
fn start_owner_agents(app: &AppHandle, owner: PublicKey, resume: Vec<String>) {
    start_owner_agents_among(app, owner, resume, None);
}

/// [`start_owner_agents`], limited to `among` when given.
fn start_owner_agents_among(
    app: &AppHandle,
    owner: PublicKey,
    resume: Vec<String>,
    among: Option<Vec<String>>,
) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let relay_url = crate::relay::relay_ws_url_with_override(&state);
        let records = match crate::managed_agents::load_managed_agents(&app) {
            Ok(records) => records,
            Err(error) => {
                eprintln!("kuzz-desktop: auth: agents not started after sign-in: {error}");
                return;
            }
        };
        for pubkey in owner_agents_to_start(&records, &owner.to_hex(), &resume, |record| {
            crate::managed_agents::agent_owner_hex(&app, record)
        })
        .into_iter()
        .filter(|pubkey| {
            among
                .as_ref()
                .is_none_or(|among| among.iter().any(|p| p.eq_ignore_ascii_case(pubkey)))
        }) {
            if let Err(error) = crate::managed_agents::start_managed_agent_runtime(
                pubkey.clone(),
                relay_url.clone(),
                app.clone(),
            ) {
                eprintln!("kuzz-desktop: auth: agent {pubkey} not started after sign-in: {error}");
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
pub(super) async fn fresh_session(state: &AppState, origin: &str) -> Result<UserSession, String> {
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
    if state.held_key_unpaired(&origin) {
        return Err(super::signed_out::SIGNED_OUT_MESSAGE.into());
    }
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
/// on this computer: stop its agents, revoke its session server-side, durably
/// mark it signed out, remove the agent keys the relay holds a copy of for
/// this device, and when it was the active account switch to another
/// account still signed in here (`switchedTo`), else the signed-out screen.
/// The account's key and its stored messages on this server leave this
/// computer (the server restores both on the next sign-in). A network failure
/// keeps everything so the user can retry (the server-side session would
/// otherwise stay alive unrevoked).
#[tauri::command]
pub async fn logout(app: AppHandle, account: Option<String>) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let account = account
        .map(|hex| PublicKey::from_hex(&hex).map_err(|_| "invalid account".to_string()))
        .transpose()?;
    let signed_out = super::user_sign_out::sign_out_by_user(&app, &origin, account, true).await?;
    let support = state.token_auth.cached_support(&origin).flatten();
    let mut status = current_status(&state, &origin, support);
    status.switched_to = signed_out.switched_to.map(|key| key.to_hex());
    status.agents_not_stopped = signed_out.agents_not_stopped;
    Ok(status)
}

/// Accounts with a remote sign-out check in flight (origin, account).
static REMOTE_SIGN_OUT_CHECKS: std::sync::Mutex<Vec<(String, PublicKey)>> =
    std::sync::Mutex::new(Vec::new());

/// The relay refused `account`'s socket as signed out (`restricted:
/// signed_out`, e.g. this device was signed out from another one). Ask
/// `/auth/me` with the account's session: only a 401 saying the session
/// ended (`token_revoked`, `principal_disabled`, `invalid_token`) signs that
/// one account out here; other accounts stay signed in. Returns whether it
/// signed out. `account` defaults to the active account, `relay_url` to the
/// current community.
#[tauri::command]
pub async fn check_remote_sign_out(
    app: AppHandle,
    account: Option<String>,
    relay_url: Option<String>,
) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let origin = match relay_url {
        Some(relay_url) => super::origin_for(&relay_url),
        None => state.current_auth_origin(),
    };
    let account = match account {
        Some(hex) => PublicKey::from_hex(&hex).map_err(|_| "invalid account".to_string())?,
        None => match state.token_auth.signing_pubkey(&origin) {
            Some(account) => account,
            None => return Ok(false),
        },
    };
    let slot = (origin.clone(), account);
    {
        let mut checks = REMOTE_SIGN_OUT_CHECKS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if checks.contains(&slot) {
            return Ok(false);
        }
        checks.push(slot.clone());
    }
    let result = remote_sign_out(&app, &origin, account).await;
    REMOTE_SIGN_OUT_CHECKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|check| *check != slot);
    result
}

async fn remote_sign_out(
    app: &AppHandle,
    origin: &str,
    account: PublicKey,
) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let session = if state.token_auth.signing_pubkey(origin) == Some(account) {
        match state.token_auth.get(origin) {
            Some((_, OriginAuth::Active(session))) => Some(session),
            _ => None,
        }
    } else {
        state
            .token_auth
            .inactive
            .on_origin(origin)
            .into_iter()
            .find(|(key, _)| *key == account)
            .and_then(|(_, auth)| match auth.get(origin) {
                Some((_, OriginAuth::Active(session))) => Some(session),
                _ => None,
            })
    };
    // No session here (a key-only account): nothing the relay could confirm.
    let Some(session) = session else {
        return Ok(false);
    };
    match api::me(&state.http_client, origin, &session.access).await {
        Err(error) if session_ended_remotely(&error) => {
            eprintln!(
                "kuzz-desktop: auth: signed out from another device ({})",
                error.code.as_deref().unwrap_or_default()
            );
            sign_out_account(app, origin, Some(account), false).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Whether `/auth/me` says the session ended for good (mobile PR #198). Pure.
fn session_ended_remotely(error: &api::ApiError) -> bool {
    error.status == Some(401)
        && matches!(
            error.code.as_deref(),
            Some("token_revoked" | "principal_disabled" | "invalid_token")
        )
}

/// Sign the active account out on this computer only, for the startup error
/// screens (the server may be unreachable): the same local sign-out as
/// [`logout`], without waiting on the network. Ending the session on the
/// server is best effort: it is attempted once in the background, after the
/// local sign-out, and a failure is only logged.
#[tauri::command]
pub async fn logout_on_this_device(app: AppHandle) -> Result<TokenAuthStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    // Read the session after any in-flight refresh settles, so the token
    // revoked below is the newest one.
    drop(state.token_auth.refresh_lock(&origin).lock_owned().await);
    let pending = pending_revocation(&state, &origin);
    let signed_out = super::user_sign_out::sign_out_by_user(&app, &origin, None, false).await?;
    if let Some(pending) = pending {
        let client = state.http_client.clone();
        let revoke_origin = origin.clone();
        tauri::async_runtime::spawn(async move {
            let revoke = async {
                match pending {
                    PendingRevocation::Session(access) => {
                        revoke_session(&client, &revoke_origin, &access).await
                    }
                    // The stored token is already forgotten here: a rotated
                    // one is not kept.
                    PendingRevocation::Stored(refresh) => {
                        revoke_stored(&client, &revoke_origin, &refresh, |_| Ok(())).await
                    }
                }
            };
            match tokio::time::timeout(std::time::Duration::from_secs(30), revoke).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    eprintln!("kuzz-desktop: auth: device-only sign-out not revoked: {error}")
                }
                Err(_) => eprintln!("kuzz-desktop: auth: device-only sign-out revoke timed out"),
            }
        });
    }
    let support = state.token_auth.cached_support(&origin).flatten();
    let mut status = current_status(&state, &origin, support);
    status.switched_to = signed_out.switched_to.map(|key| key.to_hex());
    status.agents_not_stopped = signed_out.agents_not_stopped;
    Ok(status)
}

/// What a device-only sign-out still tries to end on the server.
enum PendingRevocation {
    /// The live session's access token.
    Session(String),
    /// The stored refresh token (restoring or retrying).
    Stored(String),
}

fn pending_revocation(state: &AppState, origin: &str) -> Option<PendingRevocation> {
    match state.token_auth.get(origin) {
        Some((_, OriginAuth::Active(session))) => Some(PendingRevocation::Session(
            session.access.as_str().to_owned(),
        )),
        _ => KeyringRefreshStore
            .load(origin)
            .ok()
            .flatten()
            .map(PendingRevocation::Stored),
    }
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
    // While the session still works, its device here stops listing agents;
    // a sign-out that fails leaves the account signed in, so list them again.
    let state = app.state::<AppState>();
    let target = account.or_else(|| state.token_auth.signing_pubkey(origin));
    crate::device_robot::clear_agent_host_devices(&state, origin, target).await;
    let result = sign_out_account_now(app, origin, account, revoke).await;
    if result.is_err() {
        crate::device_robot::spawn_publish_agent_host_devices(app);
    }
    result
}

/// [`sign_out_account`] after its device list was cleared.
async fn sign_out_account_now(
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
    // A user sign-out stops the agents (and their relay sockets) before the
    // relay ends the session; a logout that fails starts them again.
    let stopped_first = revoke && !pairs.is_empty();
    if stopped_first {
        let first = std::mem::take(&mut pairs);
        let attempted = first.len();
        let stop_app = app.clone();
        let stopping = tauri::async_runtime::spawn_blocking(move || {
            crate::managed_agents::stop_pairs(&stop_app, first)
        });
        await_agent_stops(
            async { stopping.await.map_err(|error| error.to_string()) },
            attempted,
            SIGN_OUT_STOP_LIMIT,
        )
        .await;
    }
    // Agent keys may leave with a user sign-out only once the relay has just
    // confirmed it holds their copy for this device (before the session
    // ends). Otherwise (no answer, or a sign-out from elsewhere) they stay.
    let confirmed = if revoke {
        super::agent_hosting::confirm_copies(app, origin, &owners).await
    } else {
        std::collections::HashSet::new()
    };
    let restart_stopped = || {
        if stopped_first {
            for owner in &owners {
                start_owner_agents(app, *owner, stopping.clone());
            }
        }
    };
    if is_active {
        // Let an in-flight restore or refresh settle first (sign-out is
        // offered while restoring, Rule 6), so a session it just activated is
        // revoked server-side rather than only forgotten locally.
        drop(state.token_auth.refresh_lock(origin).lock_owned().await);
        if revoke {
            let revoked = match state.token_auth.get(origin) {
                Some((_, OriginAuth::Active(session))) => {
                    revoke_session(&state.http_client, origin, &session.access).await
                }
                // Restoring or retrying: revoke with the stored token.
                _ => match KeyringRefreshStore.load(origin) {
                    Ok(Some(token)) => {
                        revoke_stored(&state.http_client, origin, &token, |rotated| {
                            KeyringRefreshStore.store(origin, rotated)
                        })
                        .await
                    }
                    _ => Ok(()),
                },
            };
            if let Err(error) = revoked {
                restart_stopped();
                return Err(error);
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
            if let Err(error) = revoked {
                // Nothing changed locally: keep it alive for a retry.
                restart_stopped();
                state.token_auth.inactive.start(
                    &state.http_client,
                    data_dir.clone(),
                    origin,
                    account,
                );
                return Err(error);
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
            eprintln!("kuzz-desktop: auth: account list not updated on sign-out: {error}");
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
    // Agent keys whose copy the relay holds for this device leave with the
    // sign-out (restored at the next sign-in); the device key always stays.
    forget_owner_agent_keys(app, origin, &owners, &confirmed);
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
                eprintln!("kuzz-desktop: auth: switched after sign-out with: {warning}")
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("kuzz-desktop: auth: no switch after sign-out: {error}");
                forget_key_after_sign_out(app, account, data_dir.as_deref());
                emit_changed(app, origin);
                return Ok(SignedOut {
                    switched_to: None,
                    agents_not_stopped,
                });
            }
        }
        forget_key_after_sign_out(app, account, data_dir.as_deref());
        return Ok(SignedOut {
            switched_to: Some(next),
            agents_not_stopped,
        });
    }
    forget_key_after_sign_out(app, account, data_dir.as_deref());
    emit_changed(app, origin);
    Ok(SignedOut {
        switched_to: None,
        agents_not_stopped,
    })
}

/// Remove the confirmed-copy agent keys of `owners` on `origin` (see
/// [`super::agent_hosting::forget_confirmed_keys`]). Best effort.
fn forget_owner_agent_keys(
    app: &AppHandle,
    origin: &str,
    owners: &[PublicKey],
    confirmed: &std::collections::HashSet<String>,
) {
    let owned: Vec<String> = crate::managed_agents::load_managed_agents_without_keys(app)
        .unwrap_or_default()
        .into_iter()
        .filter(|record| record.bot_origin.is_none())
        .filter(|record| {
            let owner = crate::managed_agents::agent_owner_hex(app, record);
            owners
                .iter()
                .any(|o| owner.as_deref() == Some(o.to_hex().as_str()))
        })
        .map(|record| record.pubkey)
        .collect();
    let removed = super::agent_hosting::forget_confirmed_keys(app, origin, &owned, confirmed);
    if removed > 0 {
        eprintln!("kuzz-desktop: auth: {removed} agent key(s) left this computer at sign-out");
    }
}

/// The signed-out account's key leaves this computer (after any switch, so
/// the active copy has moved on): a copy left behind could still sign as the
/// person. Signing in again restores it from the server backup. Best effort: a
/// failure is logged, the sign-out itself already happened.
fn forget_key_after_sign_out(
    app: &AppHandle,
    account: Option<PublicKey>,
    data_dir: Option<&std::path::Path>,
) {
    let (Some(account), Some(data_dir)) = (account, data_dir) else {
        return;
    };
    let state = app.state::<AppState>();
    let Ok(_guard) = state.identity_mutation.lock() else {
        eprintln!("kuzz-desktop: auth: signed-out key kept: identity storage is busy");
        return;
    };
    match super::accounts::forget_signed_out_key(
        super::accounts::shared_secrets(),
        data_dir,
        &account,
    ) {
        Ok(Some(placeholder)) => {
            if let Ok(mut keys) = state.keys.lock() {
                if keys.public_key() == account {
                    *keys = placeholder;
                }
            }
        }
        Ok(None) => {}
        Err(error) => eprintln!("kuzz-desktop: auth: signed-out key not removed: {error}"),
    }
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
                eprintln!("kuzz-desktop: auth: agent not stopped on sign-out: {failure}");
            }
            failures.len()
        }
        Ok(Err(error)) => {
            eprintln!("kuzz-desktop: auth: agents not stopped on sign-out: {error}");
            attempted
        }
        Err(_) => {
            eprintln!(
                "kuzz-desktop: auth: {attempted} agent(s) still stopping after {}s; signing out anyway",
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
                eprintln!("kuzz-desktop: auth: the relay ended a session on {origin} ({code}); signing it out");
                if let Err(error) = sign_out_account(&app, &origin, account, false).await {
                    eprintln!("kuzz-desktop: auth: sign-out after {code} failed: {error}");
                    // The UI still learns the session ended.
                    emit_changed(&app, &origin);
                }
            });
        }));
}

/// Install the hook that tells each background account's owner which agents
/// this device hosts for them once its session (and so its device) is known.
pub(crate) fn install_session_restored_hook(app: &AppHandle) {
    let hook_app = app.clone();
    app.state::<AppState>()
        .token_auth
        .inactive
        .set_restored_hook(std::sync::Arc::new(move || {
            crate::device_robot::spawn_publish_agent_host_devices(&hook_app);
            // An account back on this computer: keys of its agents that are
            // still away may come back (when its sign-in is recent).
            super::agent_hosting::spawn_restore_pending(&hook_app);
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
                eprintln!("kuzz-desktop: auth: rotated token not kept before sign-out: {error}");
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
    super::user_sign_out::forget_messages(&app, &origin, Some(session.principal)).await;
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
            Ok(Err(error)) => eprintln!("kuzz-desktop: auth: sign-out of {origin}: {error}"),
            Err(_) => eprintln!("kuzz-desktop: auth: sign-out of {origin} timed out"),
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
#[path = "commands_tests.rs"]
mod tests;
