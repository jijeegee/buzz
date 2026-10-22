//! The Kuzz device key: one P-256 key pair per install and scope, kept in
//! the TPM where Windows has one and otherwise in software, and the device
//! proofs it signs for the relay (wire format in
//! [`kuzz_auth::device_proof`]).
//!
//! - [`DeviceKey::open_or_create`]: Windows TPM (CNG Microsoft Platform
//!   Crypto Provider, key `Kuzz-Device-<scope>`, ECDSA P-256,
//!   non-exportable, per Windows user), else a software key in the caller's
//!   [`SoftwareKeyStore`].
//! - [`DeviceKey::open_or_create_file`]: a software key in a file the caller
//!   names (headless CLI), mode 0600 on Unix.
//! - [`DeviceKey::add_event_tag`]: the `["device", <pubkey>, <sig>]` tag of
//!   NIP-42 / NIP-98 / Blossom auth events, added before the nostr signature.
//! - [`DeviceKey::header_proof`]: the `X-Kuzz-Device-Proof` header value.
//!
//! # When the key is software
//!
//! - Off Windows (until Secure Enclave / Keystore backends exist).
//! - On Windows only when the TPM is truly unavailable: the Platform Crypto
//!   Provider does not open, there is no TPM, it is disabled, or it cannot
//!   make P-256 keys (the exact status codes are listed on
//!   [`DeviceKey::open_or_create`]). Any other TPM failure (busy, not ready,
//!   access denied, lockout) is an error for the caller to retry later; it
//!   never quietly produces a software key.
//! - When the scope already has a software key in the store (made earlier
//!   on a machine without a usable TPM): an existing key is never replaced.
//! - [`DeviceKey::open_or_create_file`], always.
//!
//! Never after a TPM key was used: opening or creating a TPM key records a
//! marker (scope `<scope>#tpm`, the SHA-256 of the TPM public key) in the
//! store, and with a marker present an unavailable TPM is an error, not a
//! software key. [`DeviceKey::reset`] (with the user's consent, e.g. after
//! the TPM was cleared) removes the TPM key, the store key and the marker.
//!
//! # Cost
//!
//! TPM calls block for a TPM round trip: about 7 ms to open, about 13 ms
//! (median) to sign, about 100 ms to create, measured on a Windows 11 PC
//! (B0). Call [`DeviceKey::sign`] and the proof helpers from
//! `spawn_blocking` in async code. The TPM serialises signatures across the
//! whole machine, about 79 signatures per second in total, however many
//! processes or handles sign; a software signature costs about 1-2 ms.

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
pub use software::{FileStore, SoftwareKeyStore, SoftwareSecret, StoreOutcome};

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
    /// A [`SoftwareKeyStore`] backend (e.g. an app's OS keyring) failed.
    #[error("device key store: {0}")]
    Store(String),
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
    /// Software key (a [`SoftwareKeyStore`] or a file).
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

/// The scope every Kuzz app and CLI on a machine uses for its device key.
pub const DEVICE_KEY_SCOPE: &str = "kuzz-device-v1";

/// Longest accepted scope.
const MAX_SCOPE_LEN: usize = 64;

/// Suffix of the store scope recording that `<scope>` has a TPM key.
const TPM_MARKER_SUFFIX: &str = "#tpm";

/// The store scope of `scope`'s TPM marker.
fn tpm_marker_scope(scope: &str) -> String {
    format!("{scope}{TPM_MARKER_SUFFIX}")
}

/// Validate a store scope: a device key scope, or its TPM marker scope.
fn validate_store_scope(scope: &str) -> Result<()> {
    validate_scope(scope.strip_suffix(TPM_MARKER_SUFFIX).unwrap_or(scope))
}

/// Record that `scope` has the TPM key `pubkey` (create-if-absent).
#[cfg(any(windows, test))]
fn record_tpm_marker(
    store: &dyn SoftwareKeyStore,
    scope: &str,
    pubkey: &DevicePubkey,
) -> Result<()> {
    let marker = tpm_marker_scope(scope);
    if store.load(&marker)?.is_none() {
        let digest: [u8; 32] = sha2::Sha256::digest(pubkey.as_bytes()).into();
        store.store_new(&marker, &SoftwareSecret::new(digest))?;
    }
    Ok(())
}

/// The software key standing in for an unavailable TPM (`error`): the
/// store key of `scope` (created when `create`), unless the scope's TPM
/// marker says a TPM key was used here, in which case `error`.
#[cfg(any(windows, test))]
fn software_fallback(
    store: &dyn SoftwareKeyStore,
    scope: &str,
    error: DeviceKeyError,
    create: bool,
) -> Result<Option<p256::ecdsa::SigningKey>> {
    if store.load(&tpm_marker_scope(scope))?.is_some() {
        return Err(error);
    }
    if create {
        software::open_or_create(store, scope).map(Some)
    } else {
        store
            .load(scope)?
            .map(|secret| software::signing_key(&secret))
            .transpose()
    }
}

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
    /// `.`; at most 64 bytes; normally [`DEVICE_KEY_SCOPE`]), creating it
    /// when there is none.
    ///
    /// On Windows, in order: an existing TPM key; an existing key of `scope`
    /// in `store`; a new TPM key. Only when the TPM is unavailable, and no
    /// TPM marker is in `store`, does a software key from `store` (loaded,
    /// or created there) take its place. Unavailable means
    /// `NCryptOpenStorageProvider` returned one of `NTE_PROV_DLL_NOT_FOUND`
    /// (0x8009001E), `NTE_PROVIDER_DLL_FAIL` (0x8009001D), `NTE_BAD_PROVIDER`
    /// (0x80090013), `NTE_DEVICE_NOT_FOUND` (0x80090035),
    /// `TBS_E_TPM_NOT_FOUND` (0x8028400F), `TBS_E_SERVICE_DISABLED`
    /// (0x80284010), `TPM_E_DISABLED` (0x80280007), `TPM_E_DEACTIVATED`
    /// (0x80280006); or `NCryptCreatePersistedKey` / `NCryptFinalizeKey`
    /// returned one of those or `NTE_NOT_SUPPORTED` (0x80090029),
    /// `NTE_BAD_ALGID` (0x80090008). Every other failure, including any
    /// failure to open an existing TPM key, is [`DeviceKeyError::Tpm`].
    /// Using a TPM key records its marker in `store`.
    ///
    /// Elsewhere: the key of `scope` in `store`, created when absent.
    pub fn open_or_create(scope: &str, store: &dyn SoftwareKeyStore) -> Result<Self> {
        validate_scope(scope)?;
        #[cfg(windows)]
        {
            let name = tpm_key_name(scope);
            match tpm::open(&name) {
                Ok(Some(key)) => return Self::from_tpm(key, scope, store),
                Ok(None) => {}
                Err(tpm::TpmError::Unavailable(error)) => {
                    return Self::from_fallback(software_fallback(store, scope, error, true)?)
                }
                Err(tpm::TpmError::Failed(error)) => return Err(error),
            }
            if let Some(secret) = store.load(scope)? {
                return Self::software(software::signing_key(&secret)?);
            }
            match tpm::create(&name) {
                Ok(key) => Self::from_tpm(key, scope, store),
                // No key exists yet, so the software key replaces nothing;
                // the relay sees `software` from `kind()`.
                Err(tpm::TpmError::Unavailable(error)) => {
                    Self::from_fallback(software_fallback(store, scope, error, true)?)
                }
                Err(tpm::TpmError::Failed(error)) => Err(error),
            }
        }
        #[cfg(not(windows))]
        {
            Self::software(software::open_or_create(store, scope)?)
        }
    }

    /// Open the existing device key of `scope` (TPM, else `store`) without
    /// ever creating one: for child processes of the app that made it.
    /// `Ok(None)` when there is none. An unavailable TPM with a TPM marker
    /// in `store` is an error, as in [`Self::open_or_create`].
    pub fn open(scope: &str, store: &dyn SoftwareKeyStore) -> Result<Option<Self>> {
        validate_scope(scope)?;
        #[cfg(windows)]
        {
            match tpm::open(&tpm_key_name(scope)) {
                Ok(Some(key)) => Self::from_tpm(key, scope, store).map(Some),
                Ok(None) => store
                    .load(scope)?
                    .map(|secret| Self::software(software::signing_key(&secret)?))
                    .transpose(),
                Err(tpm::TpmError::Unavailable(error)) => {
                    software_fallback(store, scope, error, false)?
                        .map(Self::software)
                        .transpose()
                }
                Err(tpm::TpmError::Failed(error)) => Err(error),
            }
        }
        #[cfg(not(windows))]
        {
            store
                .load(scope)?
                .map(|secret| Self::software(software::signing_key(&secret)?))
                .transpose()
        }
    }

    /// Delete the device key of `scope` everywhere: the TPM key (Windows),
    /// the store key and the TPM marker. Only with the user's consent, e.g.
    /// to recover after the TPM was cleared; the next
    /// [`Self::open_or_create`] makes a new key, which the relay sees as a
    /// new device. Missing pieces are fine; an unavailable TPM has no key.
    pub fn reset(scope: &str, store: &dyn SoftwareKeyStore) -> Result<()> {
        validate_scope(scope)?;
        #[cfg(windows)]
        match tpm::open(&tpm_key_name(scope)) {
            Ok(Some(key)) => key.delete()?,
            Ok(None) | Err(tpm::TpmError::Unavailable(_)) => {}
            Err(tpm::TpmError::Failed(error)) => return Err(error),
        }
        store.delete(scope)?;
        // The marker goes last: while any key piece may remain, it keeps
        // blocking a silent software fallback.
        store.delete(&tpm_marker_scope(scope))
    }

    /// Open the software key stored in `path`, creating it (and its parent
    /// directories) when the file does not exist. Unix mode 0600. For
    /// headless use without a TPM or keyring. Same as a [`FileStore`] key,
    /// but at a path the caller chooses.
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
    fn from_fallback(key: Option<p256::ecdsa::SigningKey>) -> Result<Self> {
        // `create` fallbacks always yield a key.
        Self::software(key.ok_or(DeviceKeyError::CorruptKey)?)
    }

    #[cfg(windows)]
    fn from_tpm(key: tpm::TpmKey, scope: &str, store: &dyn SoftwareKeyStore) -> Result<Self> {
        let pubkey =
            DevicePubkey::from_sec1(&key.public_sec1()?).map_err(|_| DeviceKeyError::CorruptKey)?;
        record_tpm_marker(store, scope, &pubkey)?;
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
