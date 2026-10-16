//! Custodial key backup: server encryption and account-bound link proofs.

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use nostr::{Event, Keys, Kind, SecretKey};
use zeroize::Zeroizing;

/// Envelope contract version. Changing the AAD requires a new version.
pub const BACKUP_VERSION: u16 = 1;
const AAD_DOMAIN: &[u8] = b"buzz/custodial-key-backup/v1";

/// Failure categories intentionally contain no caller-controlled secret data.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BackupError {
    /// Invalid server master key or key identifier.
    #[error("invalid key backup configuration")]
    Configuration,
    /// Input is not a valid 32-byte secp256k1 secret scalar.
    #[error("invalid signing key")]
    InvalidSecret,
    /// The ciphertext, envelope or authenticated context is invalid.
    #[error("key backup could not be authenticated")]
    Decryption,
    /// AEAD could not produce a ciphertext.
    #[error("key backup encryption failed")]
    Encryption,
    /// The proof is not valid for this specific account/action/key/challenge.
    #[error("invalid key possession proof")]
    InvalidProof,
}

/// Encrypted at-rest envelope. Plaintext keys are never persisted here.
#[derive(Clone)]
pub struct EncryptedKeyBackup {
    /// Stable Nostr signing public key.
    pub pubkey: [u8; 32],
    /// Encryption/AAD version.
    pub version: u16,
    /// Operator-managed nonsecret master-key version label.
    pub key_id: String,
    /// Random 96-bit AES-GCM nonce.
    pub nonce: Vec<u8>,
    /// Encrypted secret scalar and 128-bit authentication tag.
    pub ciphertext: Vec<u8>,
}

/// Single server master key held outside the database. Zeroized on drop.
#[derive(Clone)]
pub struct BackupMasterKey {
    key_id: String,
    key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for BackupMasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackupMasterKey")
            .field("key_id", &self.key_id)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl BackupMasterKey {
    /// Parse a 32-byte hex master key and a 1–64 character ASCII version label.
    pub fn from_hex(key_id: &str, raw: &str) -> Result<Self, BackupError> {
        if key_id.is_empty()
            || key_id.len() > 64
            || !key_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || raw.len() != 64
        {
            return Err(BackupError::Configuration);
        }
        let mut key = Zeroizing::new([0u8; 32]);
        hex::decode_to_slice(raw, key.as_mut()).map_err(|_| BackupError::Configuration)?;
        Ok(Self {
            key_id: key_id.to_owned(),
            key,
        })
    }

    /// Encrypt a valid client-generated key, binding account and public key.
    pub fn encrypt(
        &self,
        account: &[u8; 32],
        secret_hex: &str,
    ) -> Result<EncryptedKeyBackup, BackupError> {
        let secret = parse_secret(secret_hex)?;
        let keys = Keys::new(secret);
        let raw = Zeroizing::new(keys.secret_key().to_secret_bytes());
        // rand's thread RNG is securely seeded by the OS, as with OIDC tokens.
        let nonce = rand::random::<[u8; 12]>().to_vec();
        let mut envelope = EncryptedKeyBackup {
            pubkey: keys.public_key().to_bytes(),
            version: BACKUP_VERSION,
            key_id: self.key_id.clone(),
            nonce,
            ciphertext: Vec::new(),
        };
        let aad = authenticated_data(account, &envelope);
        let cipher =
            Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(|_| BackupError::Configuration)?;
        envelope.ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&envelope.nonce),
                Payload {
                    msg: raw.as_ref(),
                    aad: &aad,
                },
            )
            .map_err(|_| BackupError::Encryption)?;
        Ok(envelope)
    }

    /// Authenticate and recover a key. There is no fallback or regeneration.
    pub fn decrypt(
        &self,
        account: &[u8; 32],
        envelope: &EncryptedKeyBackup,
    ) -> Result<Zeroizing<String>, BackupError> {
        if envelope.version != BACKUP_VERSION
            || envelope.key_id != self.key_id
            || envelope.nonce.len() != 12
            || envelope.ciphertext.len() != 48
        {
            return Err(BackupError::Decryption);
        }
        let cipher =
            Aes256Gcm::new_from_slice(self.key.as_ref()).map_err(|_| BackupError::Configuration)?;
        let aad = authenticated_data(account, envelope);
        let raw = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(&envelope.nonce),
                    Payload {
                        msg: &envelope.ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| BackupError::Decryption)?,
        );
        let secret = SecretKey::from_slice(raw.as_slice()).map_err(|_| BackupError::Decryption)?;
        if Keys::new(secret).public_key().to_bytes() != envelope.pubkey {
            return Err(BackupError::Decryption);
        }
        Ok(Zeroizing::new(hex::encode(raw.as_slice())))
    }
}

fn parse_secret(raw: &str) -> Result<SecretKey, BackupError> {
    if raw.len() != 64 {
        return Err(BackupError::InvalidSecret);
    }
    SecretKey::from_hex(raw).map_err(|_| BackupError::InvalidSecret)
}

fn authenticated_data(account: &[u8; 32], envelope: &EncryptedKeyBackup) -> Vec<u8> {
    let mut aad = Vec::with_capacity(160);
    // Fixed-width fields followed by a length-prefixed variable-width key id.
    aad.extend_from_slice(AAD_DOMAIN);
    aad.extend_from_slice(&envelope.version.to_be_bytes());
    aad.extend_from_slice(account);
    aad.extend_from_slice(&envelope.pubkey);
    aad.extend_from_slice(&(envelope.key_id.len() as u32).to_be_bytes());
    aad.extend_from_slice(envelope.key_id.as_bytes());
    aad
}

/// Verify the signed, short-lived initialization proof after atomically taking
/// its server challenge. The caller must separately check that the consumed
/// challenge belongs to the current session and derive the uploaded key's pubkey.
pub fn validate_link_proof(
    proof: &Event,
    account: &str,
    pubkey: &str,
    challenge: &str,
    url: &str,
    now: u64,
) -> Result<(), BackupError> {
    if proof.kind != Kind::HttpAuth
        || !proof.content.is_empty()
        || proof.pubkey.to_hex() != pubkey
        || proof.created_at.as_secs().abs_diff(now) > 60
        || proof.verify().is_err()
    {
        return Err(BackupError::InvalidProof);
    }
    let required = [
        ["u", url],
        ["method", "POST"],
        ["challenge", challenge],
        ["account", account],
        ["action", "initialize"],
    ];
    if proof.tags.len() != required.len()
        || required.iter().any(|required| {
            proof
                .tags
                .iter()
                .filter(|tag| {
                    let tag = tag.as_slice();
                    tag.len() == 2 && tag[0] == required[0] && tag[1] == required[1]
                })
                .count()
                != 1
        })
    {
        return Err(BackupError::InvalidProof);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
