//! The user identity seam: one place decides whether the current community
//! signs with the local key (NIP-98 / Schnorr) or with the Google-session
//! principal (`Authorization: Bearer` + server-stamped drafts).

use nostr::{Event, EventBuilder, Keys, PublicKey};
use reqwest::Method;
use zeroize::Zeroizing;

use super::CredentialMode;
use crate::app_state::AppState;

/// Message for operations that still require a local secret key.
pub(crate) fn key_required_message(what: &str) -> String {
    format!(
        "{what} needs a local signing key and is not available while signed in with Google \
         for this community yet"
    )
}

/// How the current user authenticates to the current community's relay.
#[derive(Clone)]
pub(crate) enum UserCredential {
    /// Key auth: Schnorr-signed events, NIP-98 HTTP auth.
    Keys(Keys),
    /// Token auth: drafts attributed to `principal`, bearer HTTP auth.
    Token {
        principal: PublicKey,
        access: Zeroizing<String>,
    },
}

impl UserCredential {
    /// The identity events are attributed to.
    pub(crate) fn public_key(&self) -> PublicKey {
        match self {
            Self::Keys(keys) => keys.public_key(),
            Self::Token { principal, .. } => *principal,
        }
    }

    /// Whether this is the token model.
    pub(crate) fn is_token(&self) -> bool {
        matches!(self, Self::Token { .. })
    }

    /// Produce an event: signed in key mode, a sentinel draft for the
    /// principal in token mode (the relay stamps it and recomputes the id).
    pub(crate) fn sign(&self, builder: EventBuilder) -> Result<Event, String> {
        match self {
            Self::Keys(keys) => builder
                .sign_with_keys(keys)
                .map_err(|e| format!("sign failed: {e}")),
            Self::Token { principal, .. } => {
                buzz_sdk_pkg::signer::draft_event(builder, *principal).map_err(|e| e.to_string())
            }
        }
    }

    /// The `Authorization` header value for a relay HTTP request.
    pub(crate) fn http_auth(
        &self,
        method: &Method,
        url: &str,
        body: &[u8],
    ) -> Result<String, String> {
        match self {
            Self::Keys(keys) => {
                crate::relay::build_nip98_auth_header_for_keys(keys, method, url, body)
            }
            Self::Token { access, .. } => Ok(format!("Bearer {}", access.as_str())),
        }
    }
}

/// Anything that can author relay events and authenticate relay HTTP calls:
/// a raw key (agents, legacy callers) or the user's [`UserCredential`].
/// Relay submit/query helpers take `&impl RelaySigner`, so a call site moves
/// to token mode by passing `state.user_credential()?` instead of keys.
pub(crate) trait RelaySigner: Send + Sync {
    /// The identity events are attributed to.
    fn signer_pubkey(&self) -> PublicKey;
    /// Produce the event for `builder`.
    fn sign_builder(&self, builder: EventBuilder) -> Result<Event, String>;
    /// `Authorization` header value for a relay HTTP request.
    fn relay_http_auth(&self, method: &Method, url: &str, body: &[u8]) -> Result<String, String>;
}

impl RelaySigner for Keys {
    fn signer_pubkey(&self) -> PublicKey {
        self.public_key()
    }
    fn sign_builder(&self, builder: EventBuilder) -> Result<Event, String> {
        builder
            .sign_with_keys(self)
            .map_err(|e| format!("failed to sign event: {e}"))
    }
    fn relay_http_auth(&self, method: &Method, url: &str, body: &[u8]) -> Result<String, String> {
        crate::relay::build_nip98_auth_header_for_keys(self, method, url, body)
    }
}

impl RelaySigner for UserCredential {
    fn signer_pubkey(&self) -> PublicKey {
        self.public_key()
    }
    fn sign_builder(&self, builder: EventBuilder) -> Result<Event, String> {
        self.sign(builder)
    }
    fn relay_http_auth(&self, method: &Method, url: &str, body: &[u8]) -> Result<String, String> {
        self.http_auth(method, url, body)
    }
}

impl AppState {
    /// HTTP origin of the current workspace relay (the token-session key).
    pub(crate) fn current_auth_origin(&self) -> String {
        super::origin_for(&crate::relay::relay_ws_url_with_override(self))
    }

    /// The token-auth mode of the current workspace relay.
    pub(crate) fn current_credential_mode(&self) -> CredentialMode {
        self.token_auth.mode(&self.current_auth_origin())
    }

    /// The credential for the current community: the Google-session
    /// principal when signed in there, otherwise the local key (subject to
    /// the same recovery-mode gate as [`AppState::signing_keys`]).
    pub(crate) fn user_credential(&self) -> Result<UserCredential, String> {
        match self.current_credential_mode() {
            CredentialMode::Token(session) => Ok(UserCredential::Token {
                principal: session.principal,
                access: session.access,
            }),
            CredentialMode::Blocked(reason) => Err(reason),
            CredentialMode::Keys => self.signing_keys().map(UserCredential::Keys),
        }
    }

    /// Native relay session auth for the current community.
    pub(crate) fn native_auth(&self) -> Result<crate::native_relay_client::NativeAuth, String> {
        match self.current_credential_mode() {
            CredentialMode::Token(session) => Ok(crate::native_relay_client::NativeAuth::Token {
                principal: session.principal,
                origin: self.current_auth_origin(),
                auth: std::sync::Arc::clone(&self.token_auth),
            }),
            CredentialMode::Blocked(reason) => Err(reason),
            CredentialMode::Keys => self
                .signing_keys()
                .map(crate::native_relay_client::NativeAuth::Keys),
        }
    }

    /// The pubkey the UI should treat as "me" for the current community.
    pub(crate) fn current_identity_pubkey(&self) -> Result<PublicKey, String> {
        match self.current_credential_mode() {
            CredentialMode::Token(session) => Ok(session.principal),
            _ => self
                .keys
                .lock()
                .map(|keys| keys.public_key())
                .map_err(|e| e.to_string()),
        }
    }

    /// `Some(reason)` when local-key signing must be refused for the current
    /// community because it is in (or entering) token mode.
    pub(crate) fn key_signing_blocked(&self) -> Option<String> {
        match self.current_credential_mode() {
            CredentialMode::Keys => None,
            CredentialMode::Token(_) => Some(key_required_message("This action")),
            CredentialMode::Blocked(reason) => Some(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{OriginAuth, UserSession};

    fn token_state() -> (AppState, PublicKey) {
        let state = crate::app_state::build_app_state();
        let principal = Keys::generate().public_key();
        let origin = state.current_auth_origin();
        state.token_auth.set(
            &origin,
            OriginAuth::Active(UserSession {
                principal,
                device_id: None,
                access: Zeroizing::new("bzs_test".into()),
                refresh: Zeroizing::new("bzr_test".into()),
                access_issued_at: 0,
                access_expires_at: i64::MAX,
            }),
        );
        (state, principal)
    }

    #[test]
    fn key_mode_is_unchanged_without_a_session() {
        let state = crate::app_state::build_app_state();
        let credential = state.user_credential().unwrap();
        assert!(!credential.is_token());
        let keys = state.keys.lock().unwrap().public_key();
        assert_eq!(credential.public_key(), keys);
        let header = credential
            .http_auth(&Method::GET, "http://h/query", b"")
            .unwrap();
        assert!(header.starts_with("Nostr "));
        assert!(state.key_signing_blocked().is_none());
        assert!(state.signing_keys().is_ok());
    }

    #[test]
    fn token_mode_signs_principal_drafts_with_bearer() {
        let (state, principal) = token_state();
        let credential = state.user_credential().unwrap();
        assert!(credential.is_token());
        let event = credential
            .sign(EventBuilder::new(nostr::Kind::Custom(40002), "hi"))
            .unwrap();
        assert_eq!(event.pubkey, principal);
        assert!(buzz_core_pkg::draft::is_sentinel_sig(event.sig.as_ref()));
        assert_eq!(
            credential
                .http_auth(&Method::POST, "http://h/query", b"x")
                .unwrap(),
            "Bearer bzs_test"
        );
        assert_eq!(state.current_identity_pubkey().unwrap(), principal);
        // Local-key signing is refused so the identity never splits.
        assert!(state.signing_keys().is_err());
    }

    #[test]
    fn restoring_blocks_instead_of_falling_back_to_the_key() {
        let state = crate::app_state::build_app_state();
        let origin = state.current_auth_origin();
        state.token_auth.set(&origin, OriginAuth::Restoring);
        assert!(state.user_credential().is_err());
        assert!(state.signing_keys().is_err());
    }
}
