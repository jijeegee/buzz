//! The Kuzz device key: one P-256 key pair per install and scope, kept in
//! the TPM where Windows has one and otherwise in software, and the device
//! proofs it signs for the relay (wire format in
//! [`kuzz_auth::device_proof`]).
//!
//! - [`DeviceKey::open_or_create`]: Windows TPM (CNG Microsoft Platform
//!   Crypto Provider, key `Kuzz-Device-<scope>`, non-exportable, per Windows
//!   user); without a usable TPM, or off Windows, a software key in the OS
//!   keyring.
//! - [`DeviceKey::open_or_create_file`]: a software key in a file the caller
//!   names (headless CLI), mode 0600 on Unix.
//! - [`DeviceKey::add_event_tag`]: the `["device", <pubkey>, <sig>]` tag of
//!   NIP-42 / NIP-98 / Blossom auth events, added before the nostr signature.
//! - [`DeviceKey::header_proof`]: the `X-Kuzz-Device-Proof` header value.
//!
//! An existing key is never replaced: a missing key is created, any other
//! failure to open one is an error.
//!
//! Signing with a TPM key blocks for a TPM round trip; call it from
//! `spawn_blocking` in async code.

#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

mod software;
#[cfg(windows)]
mod tpm;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Arc;
#[cfg(windows)]
use std::sync::Mutex;

use kuzz_auth::device_proof::{event_proof_message, proof_message, ProofScope, DEVICE_TAG};
pub use kuzz_auth::device_proof::{DevicePubkey, DEVICE_PUBKEY_LEN};
use nostr::{Tag, UnsignedEvent};
use sha2::Digest as _;

/// Why a device key could not be opened, created or used.
#[derive(Debug, thiserror::Error)]
pub enum DeviceKeyError {
    /// The scope is empty, too long or has characters other than ASCII
    /// letters, digits, `-`, `_` and `.`.
    #[error("invalid device key scope")]
    InvalidScope,
    /// A CNG call failed.
    #[error("TPM key operation {op} failed: 0x{code:08x}")]
    Tpm {
        /// The failing call.
        op: &'static str,
        /// Its status code (HRESULT bits).
        code: u32,
    },
    /// The OS keyring failed.
    #[error("keyring: {0}")]
    Keyring(String),
    /// Reading or writing the key file failed.
    #[error("device key file: {0}")]
    Io(#[from] std::io::Error),
    /// A stored key does not parse; it is left in place, never replaced.
    #[error("stored device key is corrupt")]
    CorruptKey,
    /// The event already has a `device` tag.
    #[error("event already has a device tag")]
    AlreadyTagged,
    /// The device tag could not be built.
    #[error("device tag: {0}")]
    Tag(String),
}

/// Result of device key operations.
pub type Result<T> = std::result::Result<T, DeviceKeyError>;

/// Where the private key lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKeyKind {
    /// Non-exportable key in the TPM.
    Tpm,
    /// Software key (OS keyring or file).
    Software,
}

impl DeviceKeyKind {
    /// The login body's `device_key_kind` value: `hardware` or `software`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Tpm => "hardware",
            Self::Software => "software",
        }
    }
}

/// Signs device proofs. Cheap to clone; clones share one key handle.
#[derive(Clone)]
pub struct DeviceKey {
    backend: Arc<Backend>,
    pubkey: DevicePubkey,
}

enum Backend {
    Software(p256::ecdsa::SigningKey),
    // One TPM call at a time per handle.
    #[cfg(windows)]
    Tpm(Mutex<tpm::TpmKey>),
}

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceKey")
            .field("kind", &self.kind())
            .field("pubkey", &self.pubkey)
            .finish()
    }
}

/// Longest accepted scope.
const MAX_SCOPE_LEN: usize = 64;

fn validate_scope(scope: &str) -> Result<()> {
    let ok = (1..=MAX_SCOPE_LEN).contains(&scope.len())
        && scope
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok {
        Ok(())
    } else {
        Err(DeviceKeyError::InvalidScope)
    }
}

/// The CNG key name of `scope`.
#[cfg(windows)]
fn tpm_key_name(scope: &str) -> String {
    format!("Kuzz-Device-{scope}")
}

impl DeviceKey {
    /// Open the device key of `scope` (ASCII letters, digits, `-`, `_`,
    /// `.`; at most 64 bytes), creating it when there is none.
    ///
    /// On Windows an existing TPM key wins, then an existing keyring key
    /// (made earlier without a TPM); otherwise a new TPM key is created, or
    /// a keyring key when the TPM cannot make one. Elsewhere the keyring key.
    pub fn open_or_create(scope: &str) -> Result<Self> {
        validate_scope(scope)?;
        #[cfg(windows)]
        {
            let name = tpm_key_name(scope);
            match tpm::open(&name) {
                Ok(Some(key)) => return Self::from_tpm(key),
                Ok(None) => {}
                Err(tpm::OpenError::NoProvider) => {
                    return Self::software(software::keyring_open_or_create(scope)?)
                }
                Err(tpm::OpenError::Failed(error)) => return Err(error),
            }
            if let Some(key) = software::keyring_open(scope)? {
                return Self::software(key);
            }
            match tpm::create(&name) {
                Ok(key) => Self::from_tpm(key),
                // No key exists yet, so falling back replaces nothing; the
                // relay sees `software` from `kind()`.
                Err(_) => Self::software(software::keyring_open_or_create(scope)?),
            }
        }
        #[cfg(not(windows))]
        {
            Self::software(software::keyring_open_or_create(scope)?)
        }
    }

    /// Open the software key stored in `path`, creating it (and its parent
    /// directories) when the file does not exist. Unix mode 0600. For
    /// headless use without a TPM or keyring.
    pub fn open_or_create_file(path: &Path) -> Result<Self> {
        Self::software(software::file_open_or_create(path)?)
    }

    fn software(key: p256::ecdsa::SigningKey) -> Result<Self> {
        let pubkey = DevicePubkey::from_sec1(key.verifying_key().to_encoded_point(true).as_bytes())
            .map_err(|_| DeviceKeyError::CorruptKey)?;
        Ok(Self {
            backend: Arc::new(Backend::Software(key)),
            pubkey,
        })
    }

    #[cfg(windows)]
    fn from_tpm(key: tpm::TpmKey) -> Result<Self> {
        let pubkey =
            DevicePubkey::from_sec1(&key.public_sec1()?).map_err(|_| DeviceKeyError::CorruptKey)?;
        Ok(Self {
            backend: Arc::new(Backend::Tpm(Mutex::new(key))),
            pubkey,
        })
    }

    /// The public key (SEC1 compressed).
    pub fn pubkey(&self) -> DevicePubkey {
        self.pubkey
    }

    /// Where the private key lives.
    pub fn kind(&self) -> DeviceKeyKind {
        match *self.backend {
            Backend::Software(_) => DeviceKeyKind::Software,
            #[cfg(windows)]
            Backend::Tpm(_) => DeviceKeyKind::Tpm,
        }
    }

    /// ECDSA P-256 / SHA-256 signature of `msg`, raw `r || s`.
    pub fn sign(&self, msg: &[u8]) -> Result<[u8; 64]> {
        match &*self.backend {
            Backend::Software(key) => {
                use p256::ecdsa::signature::Signer as _;
                let signature: p256::ecdsa::Signature = key.sign(msg);
                Ok(signature.to_bytes().into())
            }
            #[cfg(windows)]
            Backend::Tpm(key) => {
                let digest: [u8; 32] = sha2::Sha256::digest(msg).into();
                // A poisoned lock only means another signer panicked; the
                // handle itself holds no Rust state to corrupt.
                let key = key.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                key.sign_digest(&digest)
            }
        }
    }

    /// Add this device's `["device", <pubkey hex>, <sig hex>]` tag to an
    /// auth event (NIP-42 22242, NIP-98 27235, Blossom 24242, huddle auth)
    /// whose other tags and content are final. Sign the result with the
    /// message key afterwards: the event id covers the tag. Any id already
    /// computed is cleared.
    pub fn add_event_tag(&self, mut unsigned: UnsignedEvent) -> Result<UnsignedEvent> {
        let mut tags: Vec<Vec<String>> = unsigned
            .tags
            .iter()
            .map(|tag| tag.as_slice().to_vec())
            .collect();
        if tags
            .iter()
            .any(|tag| tag.first().map(String::as_str) == Some(DEVICE_TAG))
        {
            return Err(DeviceKeyError::AlreadyTagged);
        }
        let pubkey_hex = self.pubkey.to_hex();
        tags.push(vec![
            DEVICE_TAG.to_owned(),
            pubkey_hex.clone(),
            String::new(),
        ]);
        let message = event_proof_message(
            &unsigned.pubkey.to_hex(),
            unsigned.created_at.as_secs(),
            unsigned.kind.as_u16(),
            &tags,
            &unsigned.content,
        );
        let signature = self.sign(message.as_bytes())?;
        let tag = Tag::parse([DEVICE_TAG.to_owned(), pubkey_hex, hex::encode(signature)])
            .map_err(|error| DeviceKeyError::Tag(error.to_string()))?;
        unsigned.tags.push(tag);
        unsigned.id = None;
        Ok(unsigned)
    }

    /// `X-Kuzz-Device-Proof` value (`<ts>.<nonce>.<sig>`) for a request
    /// without a body, signed now. `purpose`: `bind`, `auth` or `refresh`;
    /// `binding`: `<METHOD> <full URL>` or `login:<code sha256 hex>`;
    /// `token`: the bearer (or refresh) token the request carries.
    pub fn header_proof(
        &self,
        purpose: &str,
        binding: &str,
        token: Option<&str>,
    ) -> Result<String> {
        self.header_proof_with_body(purpose, binding, token, &[])
    }

    /// [`Self::header_proof`] for a request with `body` (empty: none). The
    /// relay binds proofs of `/auth/*` calls to the body's SHA-256.
    pub fn header_proof_with_body(
        &self,
        purpose: &str,
        binding: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> Result<String> {
        self.header_proof_at(unix_now(), purpose, binding, token, body)
    }

    /// [`Self::header_proof_with_body`] at unix time `ts` (for a client
    /// correcting its clock from the relay's server time header).
    pub fn header_proof_at(
        &self,
        ts: i64,
        purpose: &str,
        binding: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> Result<String> {
        let nonce = hex::encode(rand::random::<[u8; 16]>());
        self.header_proof_with_nonce(ts, &nonce, purpose, binding, token, body)
    }

    /// Sign the header proof for already hashed token and body.
    fn header_proof_hashed(&self, ts: i64, nonce: &str, scope: &ProofScope<'_>) -> Result<String> {
        let message = proof_message(scope, ts, nonce);
        let signature = self.sign(message.as_bytes())?;
        Ok(format!("{ts}.{nonce}.{}", hex::encode(signature)))
    }

    fn header_proof_with_nonce(
        &self,
        ts: i64,
        nonce: &str,
        purpose: &str,
        binding: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> Result<String> {
        let token_hash = token.map(kuzz_auth::hash_token);
        let body_hash: Option<[u8; 32]> =
            (!body.is_empty()).then(|| sha2::Sha256::digest(body).into());
        let scope = ProofScope {
            purpose,
            binding,
            token_hash: token_hash.as_ref(),
            body_hash: body_hash.as_ref(),
        };
        self.header_proof_hashed(ts, nonce, &scope)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}
