//! macOS Secure Enclave device keys through Security.framework: ECDSA
//! P-256, generated in the Secure Enclave (the private key never leaves
//! it), kept in the data protection keychain under the label
//! `Kuzz-Device-<scope>`, usable only on this Mac
//! (`AfterFirstUnlockThisDeviceOnly`).
//!
//! The data protection keychain needs a signed binary with a keychain
//! access group (`keychain-access-groups` or `application-identifier`
//! entitlement, from a provisioning profile). Without one, as in unsigned
//! or ad-hoc dev builds and command-line tools, calls fail with
//! `errSecMissingEntitlement`: the Secure Enclave is unavailable to that
//! binary (see [`UNAVAILABLE`]).
//!
//! The same interface as the Windows `tpm` module, so `lib.rs` treats both
//! alike. Safe code only: `security-framework` wraps every FFI call.

use core_foundation::error::CFError;
use security_framework::access_control::{ProtectionMode, SecAccessControl};
use security_framework::item::{
    ItemClass, ItemSearchOptions, KeyClass, Limit, Location, Reference, SearchResult,
};
use security_framework::key::{Algorithm, GenerateKeyOptions, KeyType, SecKey, Token};
use security_framework_sys::access_control::kSecAccessControlPrivateKeyUsage;

use crate::{raw_signature_from_der, DeviceKeyError, Result};

/// Coordinate length of a P-256 key.
const COORD_LEN: usize = 32;

/// `errSecItemNotFound`: no such keychain item.
const ERR_ITEM_NOT_FOUND: i32 = -25300;

/// Status codes meaning "no usable Secure Enclave for this binary", on which
/// a software key may replace it:
///
/// - `errSecMissingEntitlement` (-34018): the binary has no keychain access
///   group (unsigned or ad-hoc dev builds, command-line tools).
/// - `errSecUnimplemented` (-4): no Secure Enclave.
/// - `errSecNotAvailable` (-25291): no keychain is available.
///
/// Not in the list, so an error to retry later: for example
/// `errSecInteractionNotAllowed` (-25308, keychain locked before the first
/// unlock) and `errSecAuthFailed` (-25293).
pub(crate) const UNAVAILABLE: [i32; 3] = [-34018, -4, -25291];

/// A Secure Enclave key.
pub(crate) struct HwKey {
    key: SecKey,
    name: String,
}

/// A failed Secure Enclave call, split by whether a software key may stand
/// in (same meaning as the Windows `TpmError`).
#[derive(Debug)]
pub(crate) enum HwError {
    /// This binary cannot use the Secure Enclave ([`UNAVAILABLE`]).
    Unavailable(DeviceKeyError),
    /// Anything else: the caller retries later; no software key is made.
    Failed(DeviceKeyError),
}

fn error(op: &'static str, code: i32) -> DeviceKeyError {
    DeviceKeyError::SecureEnclave { op, code }
}

/// Classify `code` from `op` by [`UNAVAILABLE`]. Key creation failures on
/// Intel Macs always count as unavailable: those without a T2 chip have no
/// Secure Enclave and report that with differing codes.
fn classify(op: &'static str, code: i32, creating: bool) -> HwError {
    if UNAVAILABLE.contains(&code) || (creating && cfg!(target_arch = "x86_64")) {
        HwError::Unavailable(error(op, code))
    } else {
        HwError::Failed(error(op, code))
    }
}

/// A `CFError` code as an `OSStatus` (Security.framework reports keychain
/// and Secure Enclave failures in `NSOSStatusErrorDomain`).
fn cf_code(failure: &CFError) -> i32 {
    i32::try_from(failure.code()).unwrap_or(i32::MIN)
}

/// Every private key labelled `name` in this binary's keychain access
/// groups.
fn find(name: &str) -> std::result::Result<Vec<SecKey>, HwError> {
    let results = ItemSearchOptions::new()
        .class(ItemClass::key())
        .key_class(KeyClass::private())
        .label(name)
        .ignore_legacy_keychains()
        .load_refs(true)
        .limit(Limit::All)
        .search();
    match results {
        Ok(results) => Ok(results
            .into_iter()
            .filter_map(|result| match result {
                SearchResult::Ref(Reference::Key(key)) => Some(key),
                _ => None,
            })
            .collect()),
        Err(status) if status.code() == ERR_ITEM_NOT_FOUND => Ok(Vec::new()),
        Err(status) => Err(classify("SecItemCopyMatching", status.code(), false)),
    }
}

/// Delete the key pair `name` whose application label (the hash of its
/// public key) is `application_label`: its private and public items in the
/// data protection keychain. A key that is already gone is fine.
fn delete_pair(name: &str, application_label: &[u8]) -> Result<()> {
    let deleted = ItemSearchOptions::new()
        .class(ItemClass::key())
        .label(name)
        .application_label(application_label)
        .ignore_legacy_keychains()
        .delete();
    match deleted {
        Ok(()) => Ok(()),
        Err(status) if status.code() == ERR_ITEM_NOT_FOUND => Ok(()),
        Err(status) => Err(error("SecItemDelete", status.code())),
    }
}

/// The one key of `keys` every process agrees on: the smallest application
/// label (the hash of the public key).
fn pick(keys: Vec<SecKey>) -> Option<SecKey> {
    keys.into_iter()
        .min_by_key(|key| key.application_label().unwrap_or_default())
}

/// Open the Secure Enclave key `name`; `Ok(None)` when it does not exist.
/// A failure to look is [`HwError::Failed`] unless the binary has no
/// keychain access at all.
pub(crate) fn open(name: &str) -> std::result::Result<Option<HwKey>, HwError> {
    Ok(pick(find(name)?).map(|key| HwKey {
        key,
        name: name.to_owned(),
    }))
}

/// Create the Secure Enclave key `name` (non-exportable, this Mac only).
/// When a racing creator made one too, both settle on the same key
/// ([`pick`]) and ours is deleted if it lost.
pub(crate) fn create(name: &str) -> std::result::Result<HwKey, HwError> {
    let access = SecAccessControl::create_with_protection(
        Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
        kSecAccessControlPrivateKeyUsage,
    )
    .map_err(|status| classify("SecAccessControlCreateWithFlags", status.code(), true))?;
    let mut options = GenerateKeyOptions::default();
    options
        .set_key_type(KeyType::ec_sec_prime_random())
        .set_size_in_bits(256)
        .set_label(name)
        .set_token(Token::SecureEnclave)
        .set_location(Location::DataProtectionKeychain)
        .set_access_control(access);
    let created = SecKey::new(&options)
        .map_err(|failure| classify("SecKeyCreateRandomKey", cf_code(&failure), true))?;
    let created_label = created.application_label();
    let Some(winner) = open(name)? else {
        // Created but not found again: the keychain did not keep it.
        return Err(HwError::Failed(error(
            "SecItemCopyMatching",
            ERR_ITEM_NOT_FOUND,
        )));
    };
    if let Some(label) = created_label {
        if winner.key.application_label().as_deref() != Some(label.as_slice()) {
            // Best effort: an orphan key only costs a keychain entry.
            let _ = delete_pair(name, &label);
        }
    }
    Ok(winner)
}

impl HwKey {
    /// The public key as SEC1 uncompressed bytes (`04 || X || Y`).
    pub(crate) fn public_sec1(&self) -> Result<[u8; 1 + 2 * COORD_LEN]> {
        let public = self
            .key
            .public_key()
            .ok_or_else(|| error("SecKeyCopyPublicKey", 0))?;
        let data = public
            .external_representation()
            .ok_or_else(|| error("SecKeyCopyExternalRepresentation", 0))?;
        // An EC public key's external form is X9.63, i.e. SEC1 uncompressed.
        <[u8; 1 + 2 * COORD_LEN]>::try_from(data.bytes()).map_err(|_| DeviceKeyError::CorruptKey)
    }

    /// Raw `r || s` signature of a SHA-256 digest.
    pub(crate) fn sign_digest(&self, digest: &[u8; 32]) -> Result<[u8; 64]> {
        let der = self
            .key
            .create_signature(Algorithm::ECDSASignatureDigestX962SHA256, digest)
            .map_err(|failure| error("SecKeyCreateSignature", cf_code(&failure)))?;
        raw_signature_from_der(&der)
    }

    /// Delete this key from the keychain (and so the Secure Enclave). Only
    /// Kuzz device keys (`Kuzz-Device-*`): never another program's key.
    pub(crate) fn delete(self) -> Result<()> {
        if !self.name.starts_with("Kuzz-Device-") {
            return Err(DeviceKeyError::InvalidScope);
        }
        let label = self
            .key
            .application_label()
            .ok_or_else(|| error("SecKeyCopyAttributes", 0))?;
        delete_pair(&self.name, &label)
    }
}
