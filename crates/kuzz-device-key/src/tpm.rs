//! Windows TPM device keys through CNG with the Microsoft Platform Crypto
//! Provider: ECDSA P-256, persisted per Windows user, non-exportable.
//!
//! The only module allowed `unsafe` (akak decision K1, plan §14.8): every
//! block is one FFI call into `ncrypt.dll`, with its SAFETY reasoning.

#![allow(unsafe_code)]
#![deny(clippy::undocumented_unsafe_blocks)]

use windows_sys::core::HRESULT;
use windows_sys::Win32::Foundation::{NTE_BAD_KEYSET, NTE_EXISTS};
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

/// Why [`open`] found no key.
pub(crate) enum OpenError {
    /// The platform crypto provider is not available (no usable TPM).
    NoProvider,
    /// Opening an existing key failed.
    Failed(DeviceKeyError),
}

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

fn provider() -> Result<Handle> {
    let mut handle = 0;
    // SAFETY: `handle` is a valid out-pointer for the call's duration and the
    // provider name is a static NUL-terminated UTF-16 string.
    let status = unsafe { NCryptOpenStorageProvider(&mut handle, MS_PLATFORM_CRYPTO_PROVIDER, 0) };
    // Wrap first so a handle is freed even on an unexpected error status.
    let handle = Handle(handle);
    check("NCryptOpenStorageProvider", status)?;
    Ok(handle)
}

/// Open the TPM key `name` of this Windows user; `Ok(None)` when it does not
/// exist.
pub(crate) fn open(name_text: &str) -> std::result::Result<Option<TpmKey>, OpenError> {
    let provider = provider().map_err(|_| OpenError::NoProvider)?;
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
    check("NCryptOpenKey", status).map_err(OpenError::Failed)?;
    Ok(Some(TpmKey {
        key,
        _provider: provider,
        #[cfg(test)]
        name: name_text.to_owned(),
    }))
}

/// Create the TPM key `name` (non-exportable, per Windows user). When
/// another process created it first, open that one.
pub(crate) fn create(name: &str) -> Result<TpmKey> {
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
    check("NCryptCreatePersistedKey", status)?;
    let policy: u32 = 0; // no export of any kind
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
    check("NCryptSetProperty(Export Policy)", status)?;
    // SAFETY: `key.0` is a live, not yet finalized key handle.
    let status = unsafe { NCryptFinalizeKey(key.0, NCRYPT_SILENT_FLAG) };
    if status == NTE_EXISTS {
        drop(key);
        drop(provider);
        return match open(name) {
            Ok(Some(existing)) => Ok(existing),
            Ok(None) | Err(OpenError::NoProvider) => Err(DeviceKeyError::Tpm {
                op: "NCryptFinalizeKey",
                code: NTE_EXISTS as u32,
            }),
            Err(OpenError::Failed(error)) => Err(error),
        };
    }
    check("NCryptFinalizeKey", status)?;
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
