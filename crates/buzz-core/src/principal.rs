//! Server-issued principal identities (centralized identity, Phase 0).
//!
//! A principal is a server account — a `user`, a `bot`, or the deployment's
//! single `relay` principal. Its id is 32 bytes that are **also a valid
//! secp256k1 x-only public key**: until Phase 4 replaces `nostr::Event`, every
//! stored event, `authors` filter and client parser still decodes the `pubkey`
//! field with `nostr::PublicKey`, and an arbitrary 32-byte value is a valid
//! x coordinate only about half the time. An invalid id would make rows decode
//! to `None` and silently disappear.
//!
//! [`PrincipalId`] therefore can only be constructed by [`PrincipalId::generate`]
//! (a fresh key pair whose secret is dropped immediately) or by validating
//! conversions. No secret key is ever stored or returned.

use std::fmt;

/// Error returned when bytes or hex do not form a valid principal id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalIdError {
    /// The input is not exactly 32 bytes (or 64 hex characters).
    #[error("principal id must be 32 bytes")]
    InvalidLength,
    /// The input is not hex.
    #[error("principal id is not valid hex")]
    InvalidHex,
    /// The bytes are not a valid secp256k1 x-only public key.
    #[error("principal id is not a valid x-only public key")]
    NotOnCurve,
}

/// A 32-byte server principal id that is guaranteed to parse as a
/// `nostr::PublicKey`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrincipalId(nostr::PublicKey);

impl PrincipalId {
    /// Generate a fresh principal id.
    ///
    /// Uses a random key pair and keeps only its x-only public key; the secret
    /// key is dropped before this function returns.
    pub fn generate() -> Self {
        Self(nostr::Keys::generate().public_key())
    }

    /// Validate raw bytes as a principal id.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, PrincipalIdError> {
        let array: [u8; 32] = bytes
            .try_into()
            .map_err(|_| PrincipalIdError::InvalidLength)?;
        Self::try_from(array)
    }

    /// Validate a 64-character hex string as a principal id.
    pub fn from_hex(hex_str: &str) -> Result<Self, PrincipalIdError> {
        if hex_str.len() != 64 {
            return Err(PrincipalIdError::InvalidLength);
        }
        let bytes = hex::decode(hex_str).map_err(|_| PrincipalIdError::InvalidHex)?;
        Self::from_slice(&bytes)
    }

    /// The raw 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Lower-case hex encoding (64 characters).
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }

    /// The same id as a `nostr::PublicKey` (transition adapter until Phase 4).
    pub fn as_public_key(&self) -> nostr::PublicKey {
        self.0
    }
}

impl TryFrom<[u8; 32]> for PrincipalId {
    type Error = PrincipalIdError;

    fn try_from(bytes: [u8; 32]) -> Result<Self, Self::Error> {
        // `nostr::PublicKey::from_slice` (0.44) only checks the length and
        // defers curve validation to signature-time `xonly()`, so validate
        // the point here explicitly.
        let public_key =
            nostr::PublicKey::from_slice(&bytes).map_err(|_| PrincipalIdError::NotOnCurve)?;
        public_key
            .xonly()
            .map_err(|_| PrincipalIdError::NotOnCurve)?;
        Ok(Self(public_key))
    }
}

impl From<nostr::PublicKey> for PrincipalId {
    fn from(public_key: nostr::PublicKey) -> Self {
        Self(public_key)
    }
}

impl fmt::Debug for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PrincipalId({})", self.to_hex())
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Kind of server principal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrincipalKind {
    /// A human account (Google login).
    User,
    /// A bot owned by a user.
    Bot,
    /// The deployment's own principal (sender of relay-published events).
    Relay,
}

impl PrincipalKind {
    /// The `principals.kind` column value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Bot => "bot",
            Self::Relay => "relay",
        }
    }

    /// Parse a `principals.kind` column value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "bot" => Some(Self::Bot),
            "relay" => Some(Self::Relay),
            _ => None,
        }
    }
}

/// Kind of an access token row (`access_tokens.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessTokenKind {
    /// `bzs_` user session access token.
    User,
    /// `bzb_` desktop-hosted bot access token.
    Bot,
    /// `bzk_` headless bot token.
    BotHeadless,
}

impl AccessTokenKind {
    /// The `access_tokens.kind` column value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Bot => "bot",
            Self::BotHeadless => "bot_headless",
        }
    }

    /// Parse an `access_tokens.kind` column value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "bot" => Some(Self::Bot),
            "bot_headless" => Some(Self::BotHeadless),
            _ => None,
        }
    }

    /// Whether the token authenticates a bot principal.
    pub fn is_bot(&self) -> bool {
        !matches!(self, Self::User)
    }
}

/// Seconds the relay replays a bot-token exchange (plan §4.6, §6.5).
///
/// Re-presenting an already-exchanged token within this window returns the
/// same successor instead of `token_superseded`. The relay sizes its replay
/// cache with it and buzz-acp keeps its exchange timeout below it, so an
/// exchange that timed out client-side is retried inside the window.
pub const TOKEN_EXCHANGE_REPLAY_WINDOW_SECS: u64 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    /// B1 invariant: every generated id decodes as a `nostr::PublicKey`, so a
    /// stamped event can never vanish at `row_to_stored_event`.
    #[test]
    fn generated_ids_are_always_valid_public_keys() {
        for _ in 0..1000 {
            let id = PrincipalId::generate();
            let public_key =
                nostr::PublicKey::from_slice(id.as_bytes()).expect("parses as PublicKey");
            assert!(public_key.xonly().is_ok(), "valid x-only point");
            assert_eq!(id.as_public_key().to_bytes(), *id.as_bytes());
        }
    }

    #[test]
    fn invalid_bytes_are_rejected() {
        // x = 0 is not a valid x coordinate on secp256k1.
        assert_eq!(
            PrincipalId::try_from([0u8; 32]),
            Err(PrincipalIdError::NotOnCurve)
        );
        assert_eq!(
            PrincipalId::from_slice(&[1u8; 31]),
            Err(PrincipalIdError::InvalidLength)
        );
        assert_eq!(
            PrincipalId::from_hex("zz"),
            Err(PrincipalIdError::InvalidLength)
        );
        assert_eq!(
            PrincipalId::from_hex(&"zz".repeat(32)),
            Err(PrincipalIdError::InvalidHex)
        );
    }

    #[test]
    fn hex_round_trip() {
        let id = PrincipalId::generate();
        assert_eq!(PrincipalId::from_hex(&id.to_hex()), Ok(id));
        assert_eq!(PrincipalKind::parse("bot"), Some(PrincipalKind::Bot));
        assert_eq!(PrincipalKind::Relay.as_str(), "relay");
    }
}
