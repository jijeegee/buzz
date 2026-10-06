//! Google account sessions for Desktop: Nostr key recovery and legacy tokens.
//!
//! Token mode is opt-in **per community**: it is active only when the
//! community's relay advertises `buzz_token_auth` in NIP-11 *and* the user has
//! signed in with a legacy token account for that relay. Google key-backup
//! accounts always use the original local signer, including on expiry. Each relay is its own account
//! database, so sessions are keyed by the relay's HTTP origin. The refresh
//! token lives in the OS keyring (`auth.refresh.<origin>`); the access token
//! lives only in memory here.
//!
//! Everywhere else stays on key auth unchanged: with no session for the
//! current relay [`TokenAuthState::mode`] answers [`CredentialMode::Keys`].
//!
//! - [`credential`]: the identity seam (`AppState::user_credential`) every
//!   signing / NIP-98 site uses — principal drafts + `Bearer` in token mode.
//! - [`refresh`]: background refresh, terminal "sign in again".
//! - [`commands`]: Tauri commands for login, devices, account actions.
//! - [`bots`]: managed-agent bot registration, bot tokens, exit-78 restarts.

pub(crate) mod api;
pub(crate) mod bots;
pub(crate) mod commands;
pub(crate) mod credential;
pub(crate) mod key_recovery;
pub(crate) mod loopback;
pub(crate) mod pkce;
pub(crate) mod redact;
pub(crate) mod refresh;
pub(crate) mod restore;
pub(crate) mod watchdog;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use nostr::PublicKey;
use zeroize::Zeroizing;

/// Tauri event emitted whenever a community's token-auth state changes. The
/// payload is the relay origin.
pub(crate) const TOKEN_AUTH_CHANGED_EVENT: &str = "token-auth-changed";

/// How long a NIP-11 support probe is trusted.
const SUPPORT_TTL: Duration = Duration::from_secs(60);

/// Keyring key holding the refresh token for `origin`.
pub(crate) fn refresh_key(origin: &str) -> String {
    format!("auth.refresh.{origin}")
}

/// Normalised HTTP origin for a relay WebSocket (or HTTP) URL.
pub(crate) fn origin_for(relay_url: &str) -> String {
    crate::relay::relay_http_base_url(relay_url)
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// A signed-in user session for one relay.
#[derive(Clone)]
pub(crate) struct UserSession {
    /// The user's principal id (their pubkey on this relay).
    pub principal: PublicKey,
    /// This device's id on the relay, when known.
    pub device_id: Option<String>,
    /// Current access token (`bzs_…`). Never logged.
    pub access: Zeroizing<String>,
    /// Current refresh token (`bzr_…`), mirrored from the keyring.
    pub refresh: Zeroizing<String>,
    /// When the access token was issued (unix seconds).
    pub access_issued_at: i64,
    /// When the access token expires (unix seconds).
    pub access_expires_at: i64,
}

/// Token-auth state of one relay origin.
#[derive(Clone)]
pub(crate) enum OriginAuth {
    /// No session; key auth applies.
    SignedOut,
    /// A stored refresh token is being exchanged for an access token.
    Restoring,
    /// Signed in.
    Active(UserSession),
    /// The session ended (code); the user must sign in again.
    NeedsLogin(String),
}

/// What the identity seam should use for the current relay.
pub(crate) enum CredentialMode {
    /// Key auth (relay without token auth, or not signed in).
    Keys,
    /// Token auth with this session.
    Token(UserSession),
    /// Token identity exists but is unusable right now (restoring / needs
    /// login). Signing is refused rather than silently falling back to the
    /// key identity, which would split the user's identity on this relay.
    Blocked(String),
}

/// Durable home of refresh tokens. Abstracted so tests never touch the OS
/// keyring.
pub(crate) trait RefreshStore: Send + Sync {
    fn load(&self, origin: &str) -> Result<Option<String>, String>;
    fn store(&self, origin: &str, token: &str) -> Result<(), String>;
    fn delete(&self, origin: &str) -> Result<(), String>;
    fn signing_pubkey(&self, _origin: &str) -> Result<Option<PublicKey>, String> {
        Ok(None)
    }
    fn legacy_token_account(&self, _origin: &str) -> Result<bool, String> {
        Ok(false)
    }
    fn remember_legacy_token_account(&self, _origin: &str) -> Result<(), String> {
        Ok(())
    }
}

/// The OS-keyring refresh store (shared desktop secret blob).
pub(crate) struct KeyringRefreshStore;

impl RefreshStore for KeyringRefreshStore {
    fn legacy_token_account(&self, origin: &str) -> Result<bool, String> {
        match crate::secret_store::SecretStore::shared(crate::app_state::keyring_service())
            .load(&format!("auth.legacy-token.{origin}"))?
            .as_deref()
        {
            None => Ok(false),
            Some("token") => Ok(true),
            Some(_) => Err("invalid stored account mode".into()),
        }
    }
    fn remember_legacy_token_account(&self, origin: &str) -> Result<(), String> {
        crate::secret_store::SecretStore::shared(crate::app_state::keyring_service())
            .store(&format!("auth.legacy-token.{origin}"), "token")
    }
    fn signing_pubkey(&self, origin: &str) -> Result<Option<PublicKey>, String> {
        crate::secret_store::SecretStore::shared(crate::app_state::keyring_service())
            .load(&key_recovery::binding_key(origin))?
            .map(|value| {
                PublicKey::from_hex(&value).map_err(|_| "invalid stored backup binding".into())
            })
            .transpose()
    }
    fn load(&self, origin: &str) -> Result<Option<String>, String> {
        crate::secret_store::SecretStore::shared(crate::app_state::keyring_service())
            .load(&refresh_key(origin))
    }
    fn store(&self, origin: &str, token: &str) -> Result<(), String> {
        crate::secret_store::SecretStore::shared(crate::app_state::keyring_service())
            .store(&refresh_key(origin), token)
    }
    fn delete(&self, origin: &str) -> Result<(), String> {
        crate::secret_store::SecretStore::shared(crate::app_state::keyring_service())
            .delete(&refresh_key(origin))
    }
}

/// A cached NIP-11 probe: when it was taken and the advertised providers.
type SupportEntry = (Instant, Option<Vec<String>>);

/// Per-process token-auth state held on `AppState`.
#[derive(Default)]
pub struct TokenAuthState {
    /// Recovery accounts never change messaging credentials, including while
    /// their Google session is expired or being refreshed.
    key_backups: Mutex<HashMap<String, PublicKey>>,
    origins: Mutex<HashMap<String, (u64, OriginAuth)>>,
    next_generation: AtomicU64,
    support: Mutex<HashMap<String, SupportEntry>>,
    backup_support: Mutex<HashMap<String, bool>>,
    refresh_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    login_in_flight: Mutex<bool>,
    /// A rotated refresh token not yet folded into an `Active` session, keyed
    /// by origin and fenced by generation. Covers the window between
    /// `/auth/refresh` and `/auth/me` while restoring, so a failed `/auth/me`
    /// (or a keyring write failure) never makes the next attempt replay the
    /// consumed token.
    pending_refresh: Mutex<HashMap<String, (u64, Zeroizing<String>)>>,
    /// Managed-agent bot tokens and exit-78 restart accounting.
    pub(crate) bots: bots::BotRuntime,
}

impl TokenAuthState {
    pub(crate) fn record_backup_support(&self, origin: &str, enabled: bool) {
        self.backup_support
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(origin.to_owned(), enabled);
    }

    pub(crate) fn supports_key_backup(&self, origin: &str) -> bool {
        self.backup_support
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .copied()
            .unwrap_or(false)
    }
    pub(crate) fn set_key_backup(&self, origin: &str, pubkey: PublicKey) {
        self.key_backups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(origin.to_owned(), pubkey);
    }

    pub(crate) fn signing_pubkey(&self, origin: &str) -> Option<PublicKey> {
        self.key_backups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .copied()
    }
    /// `(generation, state)` of `origin`.
    pub(crate) fn get(&self, origin: &str) -> Option<(u64, OriginAuth)> {
        self.origins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .cloned()
    }

    /// Start a new login generation for `origin` with `state`.
    pub(crate) fn set(&self, origin: &str, state: OriginAuth) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.origins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(origin.to_owned(), (generation, state));
        generation
    }

    /// Replace `origin`'s state only while it is still `generation`.
    pub(crate) fn replace_if(&self, origin: &str, generation: u64, state: OriginAuth) -> bool {
        let mut origins = self.origins.lock().unwrap_or_else(PoisonError::into_inner);
        match origins.get_mut(origin) {
            Some(entry) if entry.0 == generation => {
                entry.1 = state;
                true
            }
            _ => false,
        }
    }

    /// Whether `origin` is still on login `generation`.
    pub(crate) fn is_current(&self, origin: &str, generation: u64) -> bool {
        self.get(origin).is_some_and(|(g, _)| g == generation)
    }

    /// Remember a rotated refresh token for `origin`'s `generation`.
    pub(crate) fn set_pending_refresh(&self, origin: &str, generation: u64, token: &str) {
        self.pending_refresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                origin.to_owned(),
                (generation, Zeroizing::new(token.to_owned())),
            );
    }

    /// The rotated refresh token remembered for `origin`'s `generation`.
    pub(crate) fn pending_refresh(
        &self,
        origin: &str,
        generation: u64,
    ) -> Option<Zeroizing<String>> {
        self.pending_refresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .filter(|(g, _)| *g == generation)
            .map(|(_, token)| token.clone())
    }

    /// Drop any remembered rotated refresh token for `origin`.
    pub(crate) fn clear_pending_refresh(&self, origin: &str) {
        self.pending_refresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(origin);
    }

    /// Every origin with in-memory token-auth state.
    pub(crate) fn known_origins(&self) -> Vec<String> {
        self.origins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// The async lock serialising refreshes for `origin`.
    pub(crate) fn refresh_lock(&self, origin: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.refresh_locks
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(origin.to_owned())
                .or_default(),
        )
    }

    /// Cached NIP-11 support for `origin`: `Some(Some(providers))` supported,
    /// `Some(None)` unsupported, `None` unknown / stale.
    pub(crate) fn cached_support(&self, origin: &str) -> Option<Option<Vec<String>>> {
        self.support
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(origin)
            .filter(|(at, _)| at.elapsed() < SUPPORT_TTL)
            .map(|(_, support)| support.clone())
    }

    pub(crate) fn record_support(&self, origin: &str, support: Option<Vec<String>>) {
        self.support
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(origin.to_owned(), (Instant::now(), support));
    }

    /// Claim the single login slot; `false` when a login is already running.
    pub(crate) fn begin_login(&self) -> bool {
        let mut flag = self
            .login_in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *flag {
            return false;
        }
        *flag = true;
        true
    }

    pub(crate) fn end_login(&self) {
        *self
            .login_in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = false;
    }

    /// The identity mode for `origin`.
    pub(crate) fn mode(&self, origin: &str) -> CredentialMode {
        if self.signing_pubkey(origin).is_some() {
            return CredentialMode::Keys;
        }
        match self.get(origin) {
            None | Some((_, OriginAuth::SignedOut)) => CredentialMode::Keys,
            Some((_, OriginAuth::Active(session))) => CredentialMode::Token(session),
            Some((_, OriginAuth::Restoring)) => CredentialMode::Blocked(
                "restoring your Google sign-in for this community; try again shortly".into(),
            ),
            Some((_, OriginAuth::NeedsLogin(_))) => CredentialMode::Blocked(
                "your sign-in for this community has ended; sign in with Google again".into(),
            ),
        }
    }

    /// Every signed-in `(origin, session)`.
    pub(crate) fn active_sessions(&self) -> Vec<(String, UserSession)> {
        self.origins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(|(origin, (_, state))| match state {
                OriginAuth::Active(session) => Some((origin.clone(), session.clone())),
                _ => None,
            })
            .collect()
    }

    /// Forget every in-memory session (sign-out of the whole app / reset).
    pub(crate) fn clear_all(&self) {
        self.key_backups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.origins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.pending_refresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.support
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_normalises_ws_and_case() {
        assert_eq!(origin_for("wss://Relay.Example/"), "https://relay.example");
        assert_eq!(origin_for("ws://127.0.0.1:3000"), "http://127.0.0.1:3000");
    }

    #[test]
    fn generations_fence_writes() {
        let auth = TokenAuthState::default();
        let first = auth.set("o", OriginAuth::Restoring);
        let second = auth.set("o", OriginAuth::SignedOut);
        assert!(second > first);
        assert!(!auth.replace_if("o", first, OriginAuth::NeedsLogin("x".into())));
        assert!(matches!(auth.get("o"), Some((_, OriginAuth::SignedOut))));
        assert!(auth.replace_if("o", second, OriginAuth::Restoring));
    }

    #[test]
    fn mode_is_keys_without_a_session_and_blocked_while_unusable() {
        let auth = TokenAuthState::default();
        assert!(matches!(auth.mode("o"), CredentialMode::Keys));
        auth.set("o", OriginAuth::Restoring);
        assert!(matches!(auth.mode("o"), CredentialMode::Blocked(_)));
        auth.set("o", OriginAuth::NeedsLogin("refresh_reused".into()));
        assert!(matches!(auth.mode("o"), CredentialMode::Blocked(_)));
        auth.set("o", OriginAuth::SignedOut);
        assert!(matches!(auth.mode("o"), CredentialMode::Keys));
    }

    #[test]
    fn single_login_slot() {
        let auth = TokenAuthState::default();
        assert!(auth.begin_login());
        assert!(!auth.begin_login());
        auth.end_login();
        assert!(auth.begin_login());
    }
}
