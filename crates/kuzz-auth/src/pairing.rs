//! External agent pairing (device-bound keys plan §14.7): an owner's app
//! shows `kuzz pair <code>`, and an agent on another computer claims the code
//! with its Nostr key and that computer's device key.
//!
//! - code: 128 random bits, base64url without padding ([`PAIR_CODE_LEN`]
//!   characters); the server stores only [`pair_code_hash`].
//! - claim: the agent and the device each sign [`pair_claim_message`], which
//!   binds the code, the agent key and the device key, so neither key can be
//!   swapped for another after the fact. The agent signs SHA-256 of the
//!   message with BIP-340 Schnorr (as in agent registration), the device signs
//!   the message bytes with ECDSA P-256/SHA-256, raw `r || s` (as in device
//!   proofs).

use base64::Engine as _;
use p256::ecdsa::signature::Verifier as _;
use sha2::{Digest as _, Sha256};

use crate::device_proof::DevicePubkey;

/// Length of a pairing code (16 random bytes, base64url without padding).
pub const PAIR_CODE_LEN: usize = 22;
/// Domain of the claim signatures ([`pair_claim_message`]).
pub const PAIR_CLAIM_DOMAIN: &str = "kuzz-pair-claim/v1";

/// Why a claim's signatures were refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairClaimError {
    /// The agent's signature is malformed or does not verify.
    AgentSignature,
    /// The device's signature is malformed or does not verify.
    DeviceSignature,
}

/// A new pairing code and its SHA-256.
pub fn generate_pair_code() -> (String, [u8; 32]) {
    let code = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
    let hash = pair_code_hash(&code);
    (code, hash)
}

/// Whether `code` has the shape of a pairing code.
pub fn is_pair_code(code: &str) -> bool {
    code.len() == PAIR_CODE_LEN
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// SHA-256 of a pairing code, as stored and as signed.
pub fn pair_code_hash(code: &str) -> [u8; 32] {
    Sha256::digest(code.as_bytes()).into()
}

/// The exact text the agent and the device sign to claim a pairing:
///
/// ```text
/// kuzz-pair-claim/v1
/// <sha256(code) hex>
/// <agent pubkey hex>
/// <device pubkey hex, SEC1 compressed>
/// ```
///
/// The code itself is not in the message, so a signer's log never holds it.
pub fn pair_claim_message(code_hash: &[u8; 32], agent_hex: &str, device_hex: &str) -> String {
    format!(
        "{PAIR_CLAIM_DOMAIN}\n{}\n{agent_hex}\n{device_hex}",
        hex::encode(code_hash)
    )
}

/// Verify both claim signatures (128 hex characters each) for `code`.
pub fn verify_pair_claim(
    code: &str,
    agent: &nostr::PublicKey,
    agent_signature_hex: &str,
    device: &DevicePubkey,
    device_signature_hex: &str,
) -> Result<(), PairClaimError> {
    use nostr::hashes::{sha256::Hash as Sha256Hash, Hash as _};
    use nostr::secp256k1::{schnorr::Signature, Message};
    use std::str::FromStr as _;

    let message = pair_claim_message(&pair_code_hash(code), &agent.to_hex(), &device.to_hex());

    if agent_signature_hex.len() != 128 {
        return Err(PairClaimError::AgentSignature);
    }
    let signature =
        Signature::from_str(agent_signature_hex).map_err(|_| PairClaimError::AgentSignature)?;
    let digest = Sha256Hash::hash(message.as_bytes());
    let xonly = agent.xonly().map_err(|_| PairClaimError::AgentSignature)?;
    nostr::SECP256K1
        .verify_schnorr(
            &signature,
            &Message::from_digest(digest.to_byte_array()),
            &xonly,
        )
        .map_err(|_| PairClaimError::AgentSignature)?;

    let raw: [u8; 64] = hex::decode(device_signature_hex)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(PairClaimError::DeviceSignature)?;
    let signature =
        p256::ecdsa::Signature::from_slice(&raw).map_err(|_| PairClaimError::DeviceSignature)?;
    device
        .verifying_key()
        .map_err(|_| PairClaimError::DeviceSignature)?
        .verify(message.as_bytes(), &signature)
        .map_err(|_| PairClaimError::DeviceSignature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Signer as _;
    use p256::ecdsa::SigningKey;

    fn device() -> (SigningKey, DevicePubkey) {
        let signing = SigningKey::from_slice(&[7u8; 32]).expect("key");
        let public =
            DevicePubkey::from_sec1(signing.verifying_key().to_encoded_point(true).as_bytes())
                .expect("public");
        (signing, public)
    }

    fn sign(
        code: &str,
        agent: &nostr::Keys,
        device: &SigningKey,
        public: &DevicePubkey,
    ) -> (String, String) {
        use nostr::hashes::{sha256::Hash as Sha256Hash, Hash as _};
        let message = pair_claim_message(
            &pair_code_hash(code),
            &agent.public_key().to_hex(),
            &public.to_hex(),
        );
        let digest = Sha256Hash::hash(message.as_bytes());
        let agent_sig = agent
            .sign_schnorr(&nostr::secp256k1::Message::from_digest(
                digest.to_byte_array(),
            ))
            .to_string();
        let device_sig: p256::ecdsa::Signature = device.sign(message.as_bytes());
        (agent_sig, hex::encode(device_sig.to_bytes()))
    }

    #[test]
    fn codes_are_long_url_safe_and_hashed() {
        let (code, hash) = generate_pair_code();
        assert!(is_pair_code(&code), "{code}");
        assert_eq!(hash, pair_code_hash(&code));
        assert_ne!(generate_pair_code().0, code);
        assert!(!is_pair_code(&code[1..]));
        assert!(!is_pair_code(&format!("{}+", &code[1..])));
    }

    /// Fixed inputs → fixed message: clients (`kuzz pair`) test against this.
    #[test]
    fn claim_message_golden_vector() {
        let code = "AAAAAAAAAAAAAAAAAAAAAA";
        let agent = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
        let (_, device) = device();
        assert_eq!(
            pair_claim_message(&pair_code_hash(code), agent, &device.to_hex()),
            "kuzz-pair-claim/v1\n\
             8a5bdb4cc15164126c6ef2668de9dd240d299ce6397a42c95a9411b93d080ed8\n\
             79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798\n\
             031e18532fd4754c02f3041d9c75ceb33b83ffd81ac7ce4fe882ccb1c98bc5896e"
        );
    }

    #[test]
    fn claim_signatures_bind_code_agent_and_device() {
        let (signing, public) = device();
        let agent = nostr::Keys::generate();
        let (code, _) = generate_pair_code();
        let (agent_sig, device_sig) = sign(&code, &agent, &signing, &public);
        let pubkey = agent.public_key();
        assert_eq!(
            verify_pair_claim(&code, &pubkey, &agent_sig, &public, &device_sig),
            Ok(())
        );

        // Another code, agent or device fails.
        let (other_code, _) = generate_pair_code();
        assert_eq!(
            verify_pair_claim(&other_code, &pubkey, &agent_sig, &public, &device_sig),
            Err(PairClaimError::AgentSignature)
        );
        let stranger = nostr::Keys::generate().public_key();
        assert_eq!(
            verify_pair_claim(&code, &stranger, &agent_sig, &public, &device_sig),
            Err(PairClaimError::AgentSignature)
        );
        let other_device = SigningKey::from_slice(&[9u8; 32]).expect("key");
        let other_public = DevicePubkey::from_sec1(
            other_device
                .verifying_key()
                .to_encoded_point(true)
                .as_bytes(),
        )
        .expect("public");
        assert_eq!(
            verify_pair_claim(&code, &pubkey, &agent_sig, &other_public, &device_sig),
            Err(PairClaimError::AgentSignature)
        );
        // The agent signed for this device, but the device key did not sign.
        let (agent_for_other, _) = sign(&code, &agent, &other_device, &other_public);
        assert_eq!(
            verify_pair_claim(&code, &pubkey, &agent_for_other, &other_public, &device_sig),
            Err(PairClaimError::DeviceSignature)
        );
        // Malformed signatures.
        assert_eq!(
            verify_pair_claim(&code, &pubkey, "00", &public, &device_sig),
            Err(PairClaimError::AgentSignature)
        );
        assert_eq!(
            verify_pair_claim(&code, &pubkey, &agent_sig, &public, "00"),
            Err(PairClaimError::DeviceSignature)
        );
        // Signatures swapped between the two keys.
        assert!(verify_pair_claim(&code, &pubkey, &device_sig, &public, &agent_sig).is_err());
    }
}
