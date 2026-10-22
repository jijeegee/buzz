//! Software device keys: a P-256 secret, stored as lower-case hex, in the OS
//! keyring (service [`KEYRING_SERVICE`], user = scope) or in a file.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use p256::ecdsa::SigningKey;
use zeroize::Zeroizing;

use crate::{DeviceKeyError, Result};

/// Keyring service of software device keys.
pub(crate) const KEYRING_SERVICE: &str = "kuzz-device-key";

/// A fresh random signing key.
pub(crate) fn generate() -> SigningKey {
    loop {
        let secret = Zeroizing::new(rand::random::<[u8; 32]>());
        // Fails only for zero or a value >= the curve order (~2^-128).
        if let Ok(key) = SigningKey::from_slice(secret.as_slice()) {
            return key;
        }
    }
}

/// Parse a stored secret.
pub(crate) fn decode(stored: &str) -> Result<SigningKey> {
    let bytes = Zeroizing::new(hex::decode(stored.trim()).map_err(|_| DeviceKeyError::CorruptKey)?);
    SigningKey::from_slice(&bytes).map_err(|_| DeviceKeyError::CorruptKey)
}

/// The stored form of `key`.
pub(crate) fn encode(key: &SigningKey) -> Zeroizing<String> {
    Zeroizing::new(hex::encode(key.to_bytes()))
}

fn keyring_entry(scope: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, scope).map_err(keyring_error)
}

fn keyring_error(error: keyring::Error) -> DeviceKeyError {
    DeviceKeyError::Keyring(error.to_string())
}

/// The keyring key of `scope`, if there is one.
pub(crate) fn keyring_open(scope: &str) -> Result<Option<SigningKey>> {
    match keyring_entry(scope)?.get_password() {
        Ok(stored) => decode(&Zeroizing::new(stored)).map(Some),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(keyring_error(error)),
    }
}

/// The keyring key of `scope`, created when there is none. Of two racing
/// creators the last write wins; both return the stored key read back.
pub(crate) fn keyring_open_or_create(scope: &str) -> Result<SigningKey> {
    if let Some(key) = keyring_open(scope)? {
        return Ok(key);
    }
    keyring_entry(scope)?
        .set_password(&encode(&generate()))
        .map_err(keyring_error)?;
    keyring_open(scope)?.ok_or(DeviceKeyError::CorruptKey)
}

/// Remove the keyring key of `scope` (tests only).
#[cfg(test)]
pub(crate) fn keyring_delete(scope: &str) -> Result<()> {
    match keyring_entry(scope)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(keyring_error(error)),
    }
}

/// The key in `path`, created when the file does not exist. Creation writes
/// a private temporary file and hard-links it into place, so a concurrent
/// reader never sees a partial file and a concurrent creator never
/// overwrites the winner.
pub(crate) fn file_open_or_create(path: &Path) -> Result<SigningKey> {
    match std::fs::read_to_string(path) {
        Ok(stored) => return decode(&Zeroizing::new(stored)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temp = temp_path(path);
    let written = write_private(&temp, &encode(&generate()));
    let linked = written.and_then(|()| std::fs::hard_link(&temp, path));
    // The temporary name is ours alone; it goes whatever happened.
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    decode(&Zeroizing::new(std::fs::read_to_string(path)?))
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", hex::encode(rand::random::<[u8; 8]>())));
    path.with_file_name(name)
}

/// Create `path` (which must not exist) readable by the owner only.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}
