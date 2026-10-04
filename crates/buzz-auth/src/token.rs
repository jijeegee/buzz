//! Opaque bearer tokens for the centralized-identity model.
//!
//! Every token is 32 bytes from a CSPRNG, base64url-encoded without padding,
//! behind a kind prefix (`bzl_`, `bzs_`, `bzr_`, `bzb_`, `bzk_`). The server
//! stores only the SHA-256 of the full token string; the plaintext exists in
//! memory only while it is being returned to the client. JWTs are deliberately
//! not used: revocation needs a lookup anyway, and opaque tokens need no
//! signing-key lifecycle.

use base64::Engine as _;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

/// Kind of opaque token, distinguished by its prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// `bzl_` — one-time OIDC login code (60 s).
    LoginCode,
    /// `bzs_` — user access token (1 h).
    UserAccess,
    /// `bzr_` — user refresh token (90 d sliding, rotated on use).
    UserRefresh,
    /// `bzb_` — desktop-hosted bot access token (1 h, exchangeable).
    BotAccess,
    /// `bzk_` — headless bot token (no expiry, revocable).
    BotHeadless,
}

impl TokenKind {
    /// The token's string prefix, including the underscore.
    pub fn prefix(&self) -> &'static str {
        match self {
            Self::LoginCode => "bzl_",
            Self::UserAccess => "bzs_",
            Self::UserRefresh => "bzr_",
            Self::BotAccess => "bzb_",
            Self::BotHeadless => "bzk_",
        }
    }

    /// Whether this kind authenticates requests (vs. login codes / refresh).
    pub fn is_access(&self) -> bool {
        matches!(self, Self::UserAccess | Self::BotAccess | Self::BotHeadless)
    }
}

/// Number of random bytes in every token.
pub const TOKEN_RANDOM_BYTES: usize = 32;

/// Length of the base64url body of a token (32 bytes, unpadded).
const TOKEN_BODY_LEN: usize = 43;

/// A token plaintext. `Debug` is redacted and the buffer is zeroized on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenSecret(String);

impl TokenSecret {
    /// Wrap a token string received from a client.
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// The token plaintext. Callers must never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// SHA-256 of the token string — the only form the server stores.
    pub fn hash(&self) -> [u8; 32] {
        hash_token(&self.0)
    }

    /// The token kind, if the prefix and body are well-formed.
    pub fn kind(&self) -> Option<TokenKind> {
        parse_prefix(&self.0)
    }
}

impl std::fmt::Debug for TokenSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenSecret([REDACTED])")
    }
}

impl Drop for TokenSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Generate a new token of `kind`, returning the plaintext and its hash.
pub fn generate_token(kind: TokenKind) -> (TokenSecret, [u8; 32]) {
    let mut random: [u8; TOKEN_RANDOM_BYTES] = rand::random();
    let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
    random.zeroize();
    let token = TokenSecret(format!("{}{body}", kind.prefix()));
    let hash = token.hash();
    (token, hash)
}

/// SHA-256 of a token string.
pub fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Classify a token by prefix. Returns `None` for an unknown prefix or a body
/// that is not 43 base64url characters.
pub fn parse_prefix(token: &str) -> Option<TokenKind> {
    let kind = [
        TokenKind::LoginCode,
        TokenKind::UserAccess,
        TokenKind::UserRefresh,
        TokenKind::BotAccess,
        TokenKind::BotHeadless,
    ]
    .into_iter()
    .find(|kind| token.starts_with(kind.prefix()))?;
    let body = &token[kind.prefix().len()..];
    let well_formed = body.len() == TOKEN_BODY_LEN
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    well_formed.then_some(kind)
}

/// Constant-time byte comparison.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq as _;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// RFC 7636 S256 code challenge for `verifier`.
pub fn pkce_s256_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Whether `verifier` is a syntactically valid RFC 7636 code verifier
/// (43–128 characters from the unreserved set).
pub fn is_valid_pkce_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

/// Verify a PKCE `verifier` against the S256 `challenge` stored at login start.
pub fn verify_pkce(verifier: &str, challenge: &str) -> bool {
    is_valid_pkce_verifier(verifier)
        && constant_time_eq(
            pkce_s256_challenge(verifier).as_bytes(),
            challenge.as_bytes(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_have_prefix_and_parse_back() {
        for kind in [
            TokenKind::LoginCode,
            TokenKind::UserAccess,
            TokenKind::UserRefresh,
            TokenKind::BotAccess,
            TokenKind::BotHeadless,
        ] {
            let (token, hash) = generate_token(kind);
            assert!(token.expose().starts_with(kind.prefix()));
            assert_eq!(parse_prefix(token.expose()), Some(kind));
            assert_eq!(hash, hash_token(token.expose()));
            assert_eq!(format!("{token:?}"), "TokenSecret([REDACTED])");
        }
        let (a, _) = generate_token(TokenKind::UserAccess);
        let (b, _) = generate_token(TokenKind::UserAccess);
        assert_ne!(a.expose(), b.expose(), "tokens are random");
    }

    #[test]
    fn malformed_tokens_do_not_parse() {
        assert_eq!(parse_prefix("bzs_short"), None);
        assert_eq!(parse_prefix(&format!("bzx_{}", "a".repeat(43))), None);
        assert_eq!(parse_prefix(&format!("bzs_{}", "a".repeat(42) + "!")), None);
        assert_eq!(
            parse_prefix(&format!("bzs_{}", "a".repeat(43))),
            Some(TokenKind::UserAccess)
        );
    }

    #[test]
    fn pkce_round_trip_and_rejections() {
        let verifier = "a".repeat(43);
        let challenge = pkce_s256_challenge(&verifier);
        assert!(verify_pkce(&verifier, &challenge));
        assert!(!verify_pkce(&"b".repeat(43), &challenge));
        assert!(!verify_pkce("short", &pkce_s256_challenge("short")));
        // RFC 7636 appendix B vector.
        assert_eq!(
            pkce_s256_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
