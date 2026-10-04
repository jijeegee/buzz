//! PKCE (RFC 7636, S256) and OAuth `state` generation for the Google login.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// A PKCE verifier and its S256 challenge.
pub(crate) struct PkcePair {
    /// 43-character URL-safe verifier, sent only to `/auth/oidc/complete`.
    pub verifier: Zeroizing<String>,
    /// `base64url(sha256(verifier))`, sent on the browser start URL.
    pub challenge: String,
}

/// `n` bytes of OS entropy as unpadded base64url.
pub(crate) fn random_urlsafe(n: usize) -> Result<String, String> {
    let mut bytes = Zeroizing::new(vec![0u8; n]);
    getrandom::getrandom(&mut bytes).map_err(|e| format!("entropy source: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes.as_slice()))
}

/// S256 challenge for `verifier`.
pub(crate) fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A fresh verifier (32 random bytes → 43 chars) and its challenge.
pub(crate) fn pkce_pair() -> Result<PkcePair, String> {
    let verifier = Zeroizing::new(random_urlsafe(32)?);
    let challenge = challenge_for(&verifier);
    Ok(PkcePair {
        verifier,
        challenge,
    })
}

/// A fresh client `state` (43 URL-safe chars; the relay requires 16–256).
pub(crate) fn new_state() -> Result<String, String> {
    random_urlsafe(32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_appendix_b_vector() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn pair_and_state_have_relay_accepted_shapes() {
        let pair = pkce_pair().unwrap();
        assert_eq!(pair.verifier.len(), 43);
        assert_eq!(pair.challenge.len(), 43);
        assert_eq!(pair.challenge, challenge_for(&pair.verifier));
        let state = new_state().unwrap();
        assert!((16..=256).contains(&state.len()));
        assert!(state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(new_state().unwrap(), state);
    }
}
