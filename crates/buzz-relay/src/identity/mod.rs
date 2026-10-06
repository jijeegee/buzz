//! Centralized identity (Phase 0): token authentication beside key auth.
//!
//! `AUTH_TOKEN_ENABLED` gates token messaging and bots. Independently,
//! `AUTH_KEY_BACKUP_ENABLED` mounts account sessions and Google backup routes
//! while preserving signed NIP-42/NIP-98 messaging. Backup sessions cannot
//! authorize token messaging, token profiles, bots or account deletion.
//!
//! - [`verify_access_token`] is the single token → principal seam (WS AUTH and
//!   HTTP `Bearer`).
//! - [`ws`] binds a connection to one token hash, swaps it on same-principal
//!   re-AUTH, and enforces the bound token's deadline.
//! - [`publish_revocations`] closes bound connections on this instance and
//!   fans the revoked hashes out over Redis after the revoking commit.

pub mod client_ip;
pub mod config;
pub(crate) mod kv;
pub(crate) mod profile;
pub(crate) mod ws;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use buzz_auth::oidc::OidcProvider;
use buzz_auth::{TokenBinding, TokenKind, TokenSecret};
use buzz_core::principal::{AccessTokenKind, PrincipalId};
use buzz_db::identity::AccessTokenRejection;
use buzz_pubsub::AuthRevocation;

use crate::state::AppState;

pub use config::AuthTokenConfig;

/// Why a presented access token does not authenticate. [`Self::code`] is the
/// stable client-visible code (plan §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRejection {
    /// Unknown, malformed, wrong-kind, or headless-for-hosted-bot token.
    InvalidToken,
    /// Expired (including an elapsed exchange grace period).
    TokenExpired,
    /// Revoked, session revoked, or bot deleted.
    TokenRevoked,
    /// The principal is disabled.
    PrincipalDisabled,
    /// The token store could not be read; fail closed.
    Unavailable,
}

impl TokenRejection {
    /// Stable machine-readable code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidToken => "invalid_token",
            Self::TokenExpired => "token_expired",
            Self::TokenRevoked => "token_revoked",
            Self::PrincipalDisabled => "principal_disabled",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Per-process centralized-identity runtime held on [`AppState`].
pub struct IdentityRuntime {
    config: AuthTokenConfig,
    relay_principal: OnceLock<PrincipalId>,
    providers: RwLock<HashMap<String, Arc<dyn OidcProvider>>>,
    /// Token-authenticated connections on this instance.
    pub(crate) sessions: ws::TokenSessionRegistry,
    /// Per-process memory of write-triggered kind:0 reconciles.
    pub(crate) write_triggers: profile::WriteTriggers,
    /// Bound on concurrent kind:0 publishes (see [`profile::PublishSlots`]).
    pub(crate) publish_slots: profile::PublishSlots,
}

impl std::fmt::Debug for IdentityRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityRuntime")
            .field("config", &self.config)
            .field("relay_principal", &self.relay_principal.get())
            .finish_non_exhaustive()
    }
}

impl IdentityRuntime {
    /// Build the runtime and its OIDC provider registry from `config`.
    pub fn new(config: AuthTokenConfig) -> Self {
        let mut providers: HashMap<String, Arc<dyn OidcProvider>> = HashMap::new();
        if config.sessions_enabled() {
            if let Some(google) = &config.google {
                match buzz_auth::oidc::GoogleProvider::new(
                    google.client_id.clone(),
                    google.client_secret.clone(),
                ) {
                    Ok(provider) => {
                        providers.insert("google".into(), Arc::new(provider));
                    }
                    Err(error) => tracing::error!(%error, "Google OIDC provider unavailable"),
                }
            }
            #[cfg(any(test, feature = "dev"))]
            if config.fake_oidc {
                providers.insert("google".into(), Arc::new(FakeOidcProvider));
            }
        }
        Self {
            config,
            relay_principal: OnceLock::new(),
            providers: RwLock::new(providers),
            sessions: ws::TokenSessionRegistry::default(),
            write_triggers: profile::WriteTriggers::default(),
            publish_slots: profile::PublishSlots::default(),
        }
    }

    /// Whether token auth is enabled (`AUTH_TOKEN_ENABLED`).
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Whether account sessions are enabled for token accounts or custody.
    pub fn sessions_enabled(&self) -> bool {
        self.config.sessions_enabled()
    }

    /// The token-auth configuration.
    pub fn config(&self) -> &AuthTokenConfig {
        &self.config
    }

    /// The deployment's relay principal, once startup created it.
    pub fn relay_principal(&self) -> Option<PrincipalId> {
        self.relay_principal.get().copied()
    }

    /// Record the relay principal resolved at startup.
    pub fn set_relay_principal(&self, principal: PrincipalId) {
        let _ = self.relay_principal.set(principal);
    }

    /// The OIDC provider registered under `name`.
    pub fn provider(&self, name: &str) -> Option<Arc<dyn OidcProvider>> {
        self.providers
            .read()
            .ok()
            .and_then(|providers| providers.get(name).cloned())
    }

    /// Names of the registered OIDC providers, sorted.
    pub fn provider_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .providers
            .read()
            .map(|providers| providers.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Register (or replace) an OIDC provider. Used by tests to inject a fake.
    pub fn register_provider(&self, name: &str, provider: Arc<dyn OidcProvider>) {
        if let Ok(mut providers) = self.providers.write() {
            providers.insert(name.to_owned(), provider);
        }
    }
}

/// Startup: make the relay signing key the relay principal (re-keying an
/// older relay row) and record it on the runtime. One identity for every
/// relay-authored event — signed today, server-stamped after Phase 3.
pub async fn init_relay_principal(state: &AppState) -> anyhow::Result<PrincipalId> {
    let key = PrincipalId::from_slice(&state.relay_keypair.public_key().to_bytes())
        .map_err(|error| anyhow::anyhow!("relay key is not a valid principal id: {error}"))?;
    let relay = state
        .db
        .ensure_relay_principal(&key)
        .await
        .map_err(|error| anyhow::anyhow!("failed to ensure relay principal: {error}"))?;
    state.identity.set_relay_principal(relay);
    Ok(relay)
}

/// Resolve a presented access token to its binding. The only token →
/// principal seam: WS AUTH and HTTP `Bearer` both go through here.
pub(crate) async fn verify_access_token(
    state: &AppState,
    token: &TokenSecret,
) -> Result<TokenBinding, TokenRejection> {
    let binding = verify_session_token(state, token).await?;
    if !state.identity.enabled()
        || state
            .db
            .account_key_mode(&binding.principal)
            .await
            .map_err(|_| TokenRejection::Unavailable)?
    {
        return Err(TokenRejection::InvalidToken);
    }
    Ok(binding)
}

/// Verify a server-issued session for account management. Custodial sessions
/// deliberately cannot authenticate the signed-message or token-bot surfaces.
pub(crate) async fn verify_session_token(
    state: &AppState,
    token: &TokenSecret,
) -> Result<TokenBinding, TokenRejection> {
    let kind = token
        .kind()
        .filter(TokenKind::is_access)
        .ok_or(TokenRejection::InvalidToken)?;
    let hash = token.hash();
    let record = state
        .db
        .lookup_access_token(&hash)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "access-token lookup failed; denying (fail-closed)");
            TokenRejection::Unavailable
        })?
        .ok_or(TokenRejection::InvalidToken)?;
    let prefix_matches = matches!(
        (kind, record.kind),
        (TokenKind::UserAccess, AccessTokenKind::User)
            | (TokenKind::BotAccess, AccessTokenKind::Bot)
            | (TokenKind::BotHeadless, AccessTokenKind::BotHeadless)
    );
    if !prefix_matches {
        return Err(TokenRejection::InvalidToken);
    }
    record
        .check(chrono::Utc::now())
        .map_err(|rejection| match rejection {
            AccessTokenRejection::Expired => TokenRejection::TokenExpired,
            AccessTokenRejection::Revoked => TokenRejection::TokenRevoked,
            AccessTokenRejection::PrincipalDisabled => TokenRejection::PrincipalDisabled,
            AccessTokenRejection::InvalidForBot => TokenRejection::InvalidToken,
        })?;
    Ok(TokenBinding {
        principal: record.principal,
        kind: record.kind,
        token_hash: record.token_hash,
        expires_at: record.expires_at,
        device_id: record.device_id,
        session_id: record.session_id,
        bot_id: record.bot_id,
        bot_owner: record.bot_owner,
        is_operator: record.is_operator,
    })
}

/// Apply a revocation message to this instance's bound connections. Returns
/// how many connections were closed or had their deadline lowered.
pub fn apply_revocation(state: &AppState, message: &AuthRevocation) -> usize {
    let hashes = message.hashes();
    state
        .identity
        .sessions
        .apply(&hashes, &message.reason, message.not_after)
}

/// Subscribe to the cross-instance revocation channel and apply every message
/// to this instance's bound connections. Spawns the Redis subscriber and the
/// consumer; both run for the life of the process.
pub fn spawn_revocation_consumer(state: Arc<AppState>) {
    // Subscribe before the Redis subscriber starts so nothing is dropped
    // between connect and the first `recv`.
    let mut revocation_rx = state.pubsub.subscribe_auth_revocations();
    let pubsub = Arc::clone(&state.pubsub);
    tokio::spawn(async move { pubsub.run_auth_revocation_subscriber().await });
    tokio::spawn(async move {
        loop {
            match revocation_rx.recv().await {
                Ok(message) => {
                    apply_revocation(&state, &message);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    metrics::counter!("buzz_auth_revocation_lag_total").increment(n);
                    tracing::warn!("auth-revocation consumer lagged by {n} messages");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::error!("auth-revocation broadcast channel closed");
                    break;
                }
            }
        }
    });
}

/// After a revoking transaction committed: close matching connections here,
/// then fan the hashes out to every instance.
///
/// The publish is best-effort by design — the revoked DB row is the durable
/// record, and a connection that misses the message is still closed by its
/// periodic DB recheck or its token's expiry (≤ 1 h).
pub(crate) async fn publish_revocations(
    state: &AppState,
    hashes: &[[u8; 32]],
    reason: &str,
    not_after: Option<i64>,
) {
    if hashes.is_empty() {
        return;
    }
    for message in AuthRevocation::batches(hashes, reason, not_after) {
        apply_revocation(state, &message);
        if let Err(error) = state.pubsub.publish_auth_revocation(&message).await {
            metrics::counter!("buzz_auth_revocation_publish_failures_total").increment(1);
            tracing::warn!(%error, reason, "auth revocation publish failed; DB recheck backstops");
        }
    }
}

/// A development-only OIDC provider: the "authorization URL" points straight
/// back at the relay callback with `code=fake:<subject>`, and the exchange
/// trusts that subject. Compiled only into tests and `dev` builds.
#[cfg(any(test, feature = "dev"))]
pub struct FakeOidcProvider;

#[cfg(any(test, feature = "dev"))]
impl OidcProvider for FakeOidcProvider {
    fn name(&self) -> &str {
        "google"
    }

    fn authorization_url(&self, callback_url: &str, state: &str, _nonce: &str) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("code", "fake:dev-user@example.test")
            .append_pair("state", state)
            .finish();
        format!("{callback_url}?{query}")
    }

    fn exchange_code<'a>(
        &'a self,
        code: &'a str,
        _callback_url: &'a str,
        _expected_nonce: &'a str,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<buzz_auth::oidc::OidcIdentity, buzz_auth::oidc::OidcError>,
    > {
        Box::pin(async move {
            let subject = code.strip_prefix("fake:").ok_or_else(|| {
                buzz_auth::oidc::OidcError::Exchange("fake code must start with fake:".into())
            })?;
            Ok(buzz_auth::oidc::OidcIdentity {
                subject: subject.to_owned(),
                email: subject.contains('@').then(|| subject.to_owned()),
                email_verified: true,
                name: Some(subject.split('@').next().unwrap_or(subject).to_owned()),
                picture: None,
            })
        })
    }
}
