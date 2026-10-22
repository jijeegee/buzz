//! Windows TPM device keys through CNG with the Microsoft Platform Crypto
//! Provider: ECDSA P-256, persisted per Windows user, non-exportable.
//!
//! The only module allowed `unsafe` (akak decision K1, plan §14.8): every
//! block is one FFI call into `ncrypt.dll`, with its SAFETY reasoning.

#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]

use windows_sys::core::HRESULT;
use windows_sys::Win32::Foundation::{
    NTE_BAD_ALGID, NTE_BAD_KEYSET, NTE_BAD_PROVIDER, NTE_DEVICE_NOT_FOUND, NTE_EXISTS,
    NTE_NOT_SUPPORTED, NTE_PROVIDER_DLL_FAIL, NTE_PROV_DLL_NOT_FOUND, TBS_E_SERVICE_DISABLED,
    TBS_E_TPM_NOT_FOUND, TPM_E_DEACTIVATED, TPM_E_DISABLED,
};
use windows_sys::Win32::Security::Cryptography::{
    NCryptCreatePersistedKey, NCryptExportKey, NCryptFinalizeKey, NCryptFreeObject, NCryptOpenKey,
    NCryptOpenStorageProvider, NCryptSetProperty, NCryptSignHash, BCRYPT_ECCPUBLIC_BLOB,
    MS_PLATFORM_CRYPTO_PROVIDER, NCRYPT_ECDSA_P256_ALGORITHM, NCRYPT_EXPORT_POLICY_PROPERTY,
    NCRYPT_SILENT_FLAG,
};

#[cfg(test)]
use windows_sys::Win32::Security::Cryptography::NCryptDeleteKey;

use crate::{DeviceKeyError, Result};

/// Coordinate length of a P-256 key.
const COORD_LEN: usize = 32;
/// `BCRYPT_ECCKEY_BLOB` header: magic and key length, both `u32`.
const BLOB_HEADER_LEN: usize = 8;

/// An NCrypt provider or key handle, freed on drop.
struct Handle(usize);

impl Drop for Handle {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: `self.0` is a handle NCrypt returned to us and nothing
            // else frees it (deletion zeroes it first). The status is
            // ignored: there is nothing to do about a failed free.
            unsafe {
                NCryptFreeObject(self.0);
            }
        }
    }
}

/// A TPM key. Fields drop in order: the key handle before its provider.
pub(crate) struct TpmKey {
    key: Handle,
    _provider: Handle,
    #[cfg(test)]
    name: String,
}

/// A failed TPM call, split by whether a software key may stand in.
#[derive(Debug)]
pub(crate) enum TpmError {
    /// This machine has no usable TPM for P-256 keys ([`UNAVAILABLE`]).
    Unavailable(DeviceKeyError),
    /// Anything else (busy, not ready, access denied, lockout, ...): the
    /// caller retries later; no software key is made.
    Failed(DeviceKeyError),
}

impl TpmError {
    fn into_error(self) -> DeviceKeyError {
        match self {
            Self::Unavailable(error) | Self::Failed(error) => error,
        }
    }
}

/// Status codes of `NCryptOpenStorageProvider`, `NCryptCreatePersistedKey`
/// and `NCryptFinalizeKey` meaning "no usable TPM here", the only ones on
/// which a software key replaces the TPM:
///
/// - `NTE_PROV_DLL_NOT_FOUND`, `NTE_PROVIDER_DLL_FAIL`, `NTE_BAD_PROVIDER`:
///   the Platform Crypto Provider is not installed or does not load.
/// - `NTE_DEVICE_NOT_FOUND`, `TBS_E_TPM_NOT_FOUND`: no TPM.
/// - `TBS_E_SERVICE_DISABLED`: the TPM Base Services are disabled.
/// - `TPM_E_DISABLED`, `TPM_E_DEACTIVATED`: the TPM is off in firmware.
/// - `NTE_NOT_SUPPORTED`, `NTE_BAD_ALGID`: the TPM cannot make ECDSA P-256
///   keys (TPM 1.2).
///
/// Not in the list, so an error: for example `NTE_DEVICE_NOT_READY`,
/// `TBS_E_SERVICE_NOT_RUNNING`, `TPM_E_PCP_DEVICE_NOT_READY`,
/// `E_ACCESSDENIED`, `NTE_PERM` and TPM lockout codes.
pub(crate) const UNAVAILABLE: [HRESULT; 10] = [
    NTE_PROV_DLL_NOT_FOUND,
    NTE_PROVIDER_DLL_FAIL,
    NTE_BAD_PROVIDER,
    NTE_DEVICE_NOT_FOUND,
    TBS_E_TPM_NOT_FOUND,
    TBS_E_SERVICE_DISABLED,
    TPM_E_DISABLED,
    TPM_E_DEACTIVATED,
    NTE_NOT_SUPPORTED,
    NTE_BAD_ALGID,
];

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn check(op: &'static str, status: HRESULT) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(DeviceKeyError::Tpm {
            op,
            code: status as u32,
        })
    }
}

/// [`check`], classifying a failure by [`UNAVAILABLE`].
fn classify(op: &'static str, status: HRESULT) -> std::result::Result<(), TpmError> {
    check(op, status).map_err(|error| {
        if UNAVAILABLE.contains(&status) {
            TpmError::Unavailable(error)
        } else {
            TpmError::Failed(error)
        }
    })
}

fn provider() -> std::result::Result<Handle, TpmError> {
    let mut handle = 0;
    // SAFETY: `handle` is a valid out-pointer for the call's duration and the
    // provider name is a static NUL-terminated UTF-16 string.
    let status = unsafe { NCryptOpenStorageProvider(&mut handle, MS_PLATFORM_CRYPTO_PROVIDER, 0) };
    // Wrap first so a handle is freed even on an unexpected error status.
    let handle = Handle(handle);
    classify("NCryptOpenStorageProvider", status)?;
    Ok(handle)
}

/// Open the TPM key `name` of this Windows user; `Ok(None)` when it does not
/// exist. A failure to open an existing key is always [`TpmError::Failed`].
pub(crate) fn open(name_text: &str) -> std::result::Result<Option<TpmKey>, TpmError> {
    let provider = provider()?;
    let name = wide(name_text);
    let mut key = 0;
    // SAFETY: `provider.0` is a live provider handle, `key` a valid
    // out-pointer and `name` a NUL-terminated UTF-16 buffer that outlives the
    // call.
    let status =
        unsafe { NCryptOpenKey(provider.0, &mut key, name.as_ptr(), 0, NCRYPT_SILENT_FLAG) };
    let key = Handle(key);
    if status == NTE_BAD_KEYSET {
        return Ok(None);
    }
    check("NCryptOpenKey", status).map_err(TpmError::Failed)?;
    Ok(Some(TpmKey {
        key,
        _provider: provider,
        #[cfg(test)]
        name: name_text.to_owned(),
    }))
}

/// Create the TPM key `name` (non-exportable, per Windows user). When
/// another process created it first, open that one.
pub(crate) fn create(name: &str) -> std::result::Result<TpmKey, TpmError> {
    let provider = provider()?;
    let wide_name = wide(name);
    let mut key = 0;
    // SAFETY: `provider.0` is a live provider handle, `key` a valid
    // out-pointer, the algorithm id a static NUL-terminated UTF-16 string and
    // `wide_name` a NUL-terminated buffer that outlives the call. No
    // overwrite flag: an existing key is never replaced.
    let status = unsafe {
        NCryptCreatePersistedKey(
            provider.0,
            &mut key,
            NCRYPT_ECDSA_P256_ALGORITHM,
            wide_name.as_ptr(),
            0,
            NCRYPT_SILENT_FLAG,
        )
    };
    let key = Handle(key);
    classify("NCryptCreatePersistedKey", status)?;
    // No export of any kind.
    let policy: u32 = 0;
    // SAFETY: `key.0` is a live, not yet finalized key handle; the property
    // name is a static NUL-terminated UTF-16 string; the input pointer and
    // length describe `policy`, which outlives the call.
    let status = unsafe {
        NCryptSetProperty(
            key.0,
            NCRYPT_EXPORT_POLICY_PROPERTY,
            std::ptr::from_ref(&policy).cast::<u8>(),
            std::mem::size_of::<u32>() as u32,
            NCRYPT_SILENT_FLAG,
        )
    };
    check("NCryptSetProperty(Export Policy)", status).map_err(TpmError::Failed)?;
    // SAFETY: `key.0` is a live, not yet finalized key handle.
    let status = unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) };
    if status == NTE_EXISTS {
        drop(key);
        drop(provider);
        return match open(name) {
            Ok(Some(existing)) => Ok(existing),
            Ok(None) => Err(TpmError::Failed(DeviceKeyError::Tpm {
                op: "NCryptFinalizeKey",
                code: NTE_EXISTS as u32,
            })),
            Err(error) => Err(TpmError::Failed(error.into_error())),
        };
    }
    classify("NCryptFinalizeKey", status)?;
    Ok(TpmKey {
        key,
        _provider: provider,
        #[cfg(test)]
        name: name.to_owned(),
    })
}

impl TpmKey {
    /// The public key as SEC1 uncompressed bytes (`04 || X || Y`).
    pub(crate) fn public_sec1(&self) -> Result<[u8; 1 + 2 * COORD_LEN]> {
        let mut blob = [0u8; BLOB_HEADER_LEN + 2 * COORD_LEN];
        let mut written = 0u32;
        // SAFETY: `self.key.0` is a live key handle; the output pointer and
        // length describe `blob`, and `written` is a valid out-pointer, all
        // outliving the call. The blob type is a static NUL-terminated UTF-16
        // string; no export key and no parameter list.
        let status = unsafe {
            NCryptExportKey(
                self.key.0,
                0,
                BCRYPT_ECCPUBLIC_BLOB,
                std::ptr::null(),
                blob.as_mut_ptr(),
                blob.len() as u32,
                &mut written,
                NCRYPT_SILENT_FLAG,
            )
        };
        check("NCryptExportKey", status)?;
        let key_len = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]);
        if written as usize != blob.len() || key_len as usize != COORD_LEN {
            return Err(DeviceKeyError::CorruptKey);
        }
        let mut sec1 = [0u8; 1 + 2 * COORD_LEN];
        sec1[0] = 0x04;
        sec1[1..].copy_from_slice(&blob[BLOB_HEADER_LEN..]);
        Ok(sec1)
    }

    /// Raw `r || s` signature of a SHA-256 digest.
    pub(crate) fn sign_digest(&self, digest: &[u8; 32]) -> Result<[u8; 64]> {
        let mut signature = [0u8; 64];
        let mut written = 0u32;
        // SAFETY: `self.key.0` is a live key handle; ECDSA takes no padding
        // info (null); the input pointer and length describe `digest`, the
        // output pointer and length describe `signature`, and `written` is a
        // valid out-pointer, all outliving the call.
        let status = unsafe {
            NCryptSignHash(
                self.key.0,
                std::ptr::null(),
                digest.as_ptr(),
                digest.len() as u32,
                signature.as_mut_ptr(),
                signature.len() as u32,
                &mut written,
                NCRYPT_SILENT_FLAG,
            )
        };
        check("NCryptSignHash", status)?;
        if written as usize != signature.len() {
            return Err(DeviceKeyError::CorruptKey);
        }
        Ok(signature)
    }

    /// Delete this key from the TPM. Only for keys tests made: the name must
    /// start with `Kuzz-Device-test-`.
    #[cfg(test)]
    pub(crate) fn delete(mut self) -> Result<()> {
        assert!(
            self.name.starts_with("Kuzz-Device-test-"),
            "refusing to delete a non-test key"
        );
        // SAFETY: `self.key.0` is a live key handle. On success the call
        // frees it, so it is zeroed below before `Drop` runs. The Platform
        // Crypto Provider rejects NCRYPT_SILENT_FLAG here (NTE_BAD_FLAGS).
        let status = unsafe { NCryptDeleteKey(self.key.0, 0) };
        if status == 0 {
            self.key.0 = 0;
        }
        check("NCryptDeleteKey", status)
    }
}
