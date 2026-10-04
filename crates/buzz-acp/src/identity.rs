//! Who this harness is on the relay, for both auth models (centralized
//! identity, Phase 1).
//!
//! - Key mode: `BUZZ_PRIVATE_KEY`; events are Schnorr-signed, HTTP uses NIP-98
//!   and the WebSocket answers NIP-42 challenges. Unchanged behaviour.
//! - Token mode: `BUZZ_BOT_TOKEN` (a Desktop-issued `bzb_` bot token). Events
//!   are drafts attributed to the server-issued principal (see
//!   [`buzz_sdk::signer`]), HTTP sends `Authorization: Bearer`, and the
//!   WebSocket authenticates with `["AUTH", {"token": …}]`. The token is held
//!   only in [`BotToken`] (zeroized on drop) and refreshed in place by
//!   [`crate::token_refresh`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use buzz_sdk::signer::{EventSigner, SignerError};
use nostr::{Event, EventBuilder, Keys, PublicKey};
use zeroize::Zeroizing;

/// Process exit code for an unrecoverable token-auth failure (`EX_CONFIG`).
/// Buzz Desktop reissues the bot token and restarts the harness on it.
pub const EXIT_AUTH_TERMINAL: i32 = 78;

#[derive(Clone)]
struct CurrentToken {
    secret: Zeroizing<String>,
    expires_at: i64,
}

/// The harness's current bot access token. Cheap to clone (shared state).
///
/// Every reader sees the newest adopted token; [`BotToken::adopt`] accepts a
/// replacement only for the generation it was derived from, so a slow,
/// stale refresh can never overwrite a newer token (Rule 2).
#[derive(Clone)]
pub struct BotToken {
    current: Arc<RwLock<CurrentToken>>,
    generation: Arc<AtomicU64>,
}

impl std::fmt::Debug for BotToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotToken")
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at())
            .field("generation", &self.generation())
            .finish()
    }
}

impl BotToken {
    /// Wrap the initial token and its expiry (unix seconds).
    pub fn new(secret: String, expires_at: i64) -> Self {
        Self {
            current: Arc::new(RwLock::new(CurrentToken {
                secret: Zeroizing::new(secret),
                expires_at,
            })),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    fn read(&self) -> CurrentToken {
        match self.current.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// The current token. Callers must not log it.
    pub fn secret(&self) -> Zeroizing<String> {
        self.read().secret
    }

    /// Expiry of the current token (unix seconds).
    pub fn expires_at(&self) -> i64 {
        self.read().expires_at
    }

    /// Generation of the current token (0 for the initial one).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Adopt `secret` as the successor of generation `from_generation`.
    /// Returns the new generation, or `None` when a newer token was already
    /// adopted (the caller's result is stale and is discarded).
    pub fn adopt(&self, from_generation: u64, secret: String, expires_at: i64) -> Option<u64> {
        let mut guard = match self.current.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let next = from_generation.checked_add(1)?;
        if self
            .generation
            .compare_exchange(from_generation, next, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        *guard = CurrentToken {
            secret: Zeroizing::new(secret),
            expires_at,
        };
        Some(next)
    }

    /// `Authorization` header value for the current token.
    pub fn bearer_header(&self) -> Zeroizing<String> {
        Zeroizing::new(format!("Bearer {}", self.secret().as_str()))
    }

    /// The WS token AUTH frame for the current token.
    pub fn auth_frame(&self) -> Zeroizing<String> {
        Zeroizing::new(serde_json::json!(["AUTH", { "token": self.secret().as_str() }]).to_string())
    }
}

/// The harness identity: how events are produced and how the relay is
/// authenticated. Cheap to clone.
#[derive(Clone, Debug)]
pub struct AgentIdentity {
    signer: EventSigner,
    token: Option<BotToken>,
}

impl From<Keys> for AgentIdentity {
    fn from(keys: Keys) -> Self {
        Self::keys(keys)
    }
}

impl AgentIdentity {
    /// Key mode.
    pub fn keys(keys: Keys) -> Self {
        Self {
            signer: EventSigner::Keys(keys),
            token: None,
        }
    }

    /// Token mode for `principal`, authenticated by `token`.
    pub fn token(principal: PublicKey, token: BotToken) -> Self {
        Self {
            signer: EventSigner::Principal(principal),
            token: Some(token),
        }
    }

    /// The agent's identity on the relay (key pubkey or principal id).
    pub fn public_key(&self) -> PublicKey {
        self.signer.public_key()
    }

    /// The bot token, in token mode.
    pub fn bot_token(&self) -> Option<&BotToken> {
        self.token.as_ref()
    }

    /// Whether this identity authenticates with a bearer token.
    pub fn is_token(&self) -> bool {
        self.token.is_some()
    }

    /// The secret keys, for key-only operations (NIP-44, NIP-98, NIP-42).
    pub fn secret_keys(&self, what: &'static str) -> Result<&Keys, SignerError> {
        self.signer.keys(what)
    }

    /// Produce an event from `builder` (signed, or a token-mode draft).
    pub fn sign(&self, builder: EventBuilder) -> Result<Event, SignerError> {
        self.signer.sign(builder)
    }

    /// The underlying signer.
    pub fn signer(&self) -> &EventSigner {
        &self.signer
    }
}

/// What `GET /auth/me` says about the token's principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenPrincipal {
    /// The principal id (hex x-only pubkey).
    pub principal: PublicKey,
    /// The bot's owner (hex), when the principal is a bot.
    pub owner: Option<String>,
}

/// Parse a `GET /auth/me` body.
pub fn parse_me_response(body: &serde_json::Value) -> Result<TokenPrincipal, String> {
    let principal = body
        .get("principal_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing principal_id")?;
    let principal = PublicKey::from_hex(principal).map_err(|e| format!("bad principal_id: {e}"))?;
    let owner = body
        .get("bot")
        .and_then(|bot| bot.get("owner"))
        .and_then(serde_json::Value::as_str)
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase);
    Ok(TokenPrincipal { principal, owner })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopt_accepts_only_the_next_generation() {
        let token = BotToken::new("bzb_a".into(), 100);
        assert_eq!(token.adopt(0, "bzb_b".into(), 200), Some(1));
        assert_eq!(token.secret().as_str(), "bzb_b");
        // A stale refresh derived from generation 0 is discarded.
        assert_eq!(token.adopt(0, "bzb_stale".into(), 300), None);
        assert_eq!(token.secret().as_str(), "bzb_b");
        assert_eq!(token.expires_at(), 200);
        assert_eq!(token.generation(), 1);
    }

    #[test]
    fn debug_and_frames_do_not_leak_beyond_their_purpose() {
        let token = BotToken::new("bzb_secretvalue".into(), 1);
        assert!(!format!("{token:?}").contains("secretvalue"));
        assert_eq!(token.bearer_header().as_str(), "Bearer bzb_secretvalue");
        let frame: serde_json::Value = serde_json::from_str(&token.auth_frame()).unwrap();
        assert_eq!(
            frame,
            serde_json::json!(["AUTH", {"token": "bzb_secretvalue"}])
        );
    }

    #[test]
    fn me_response_parses_principal_and_owner() {
        let pk = Keys::generate().public_key();
        let owner = Keys::generate().public_key().to_hex();
        let parsed = parse_me_response(&serde_json::json!({
            "principal_id": pk.to_hex(), "kind": "bot", "bot": {"owner": owner, "host": null}
        }))
        .unwrap();
        assert_eq!(parsed.principal, pk);
        assert_eq!(parsed.owner.as_deref(), Some(owner.as_str()));
        assert!(parse_me_response(&serde_json::json!({})).is_err());
    }

    #[test]
    fn token_identity_signs_drafts() {
        let pk = Keys::generate().public_key();
        let id = AgentIdentity::token(pk, BotToken::new("bzb_x".into(), 0));
        let event = id.sign(EventBuilder::text_note("hi")).unwrap();
        assert_eq!(event.pubkey, pk);
        assert!(buzz_core::draft::verify_served_event(&event));
        assert!(id.secret_keys("nip44").is_err());
    }
}
