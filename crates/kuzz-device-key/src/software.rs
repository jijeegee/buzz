//! Software device keys and where they are kept: [`SoftwareKeyStore`]
//! (each app brings its own keyring; this crate ships only [`FileStore`]).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use p256::ecdsa::SigningKey;
use zeroize::Zeroizing;

use crate::{validate_store_scope, DeviceKeyError, Result};

/// A 32-byte secret kept in a [`SoftwareKeyStore`]: a software device key's
/// P-256 secret, or a TPM marker. Wiped on drop; `Debug` shows no bytes.
#[derive(Clone)]
pub struct SoftwareSecret(Zeroizing<[u8; 32]>);

impl SoftwareSecret {
    /// Wrap secret bytes.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The secret bytes, for the store to persist.
    pub fn expose_secret(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SoftwareSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

/// What [`SoftwareKeyStore::store_new`] did.
#[derive(Debug)]
pub enum StoreOutcome {
    /// The new secret is now the scope's entry.
    Stored,
    /// The scope already had an entry (a racing creator won); use this one.
    Existing(SoftwareSecret),
}

/// Keeps 32-byte secrets by scope: software device keys under `<scope>` and
/// TPM markers under `<scope>#tpm` (a store must accept both forms). Apps
/// implement it with their own OS keyring version (one keyring major per
/// app); this crate provides [`FileStore`]. Report backend failures as
/// [`DeviceKeyError::Store`].
pub trait SoftwareKeyStore {
    /// The entry of `scope`, or `None` when the scope has none.
    fn load(&self, scope: &str) -> Result<Option<SoftwareSecret>>;

    /// Store `secret` as the entry of `scope` only if the scope has none
    /// (create-if-absent), else return the existing entry. A backend without
    /// an atomic create-if-absent (most OS keyrings) should load, write when
    /// absent, then load again and return [`StoreOutcome::Existing`] when
    /// the read-back differs. Its residual race: two creators both see no
    /// key, both write, and the first may already have used its own key
    /// before the second overwrites it; that device key then changes once.
    fn store_new(&self, scope: &str, secret: &SoftwareSecret) -> Result<StoreOutcome>;

    /// Delete the entry of `scope`; `Ok` when there is none. Used by
    /// [`crate::DeviceKey::reset`]. The default refuses.
    fn delete(&self, scope: &str) -> Result<()> {
        let _ = scope;
        Err(DeviceKeyError::Store(
            "this device key store does not support delete".to_owned(),
        ))
    }
}

/// Entries as files `<dir>/<scope>.key` (hex, Unix mode 0600). Creation is
/// atomic create-if-absent where the file system has hard links, so racing
/// creators agree on one key.
#[derive(Debug, Clone)]
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    /// A store keeping its files in `dir` (created on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, scope: &str) -> Result<PathBuf> {
        validate_store_scope(scope)?;
        Ok(self.dir.join(format!("{scope}.key")))
    }
}

impl SoftwareKeyStore for FileStore {
    fn load(&self, scope: &str) -> Result<Option<SoftwareSecret>> {
        file_load(&self.path(scope)?)
    }

    fn store_new(&self, scope: &str, secret: &SoftwareSecret) -> Result<StoreOutcome> {
        file_store_new(&self.path(scope)?, secret)
    }

    fn delete(&self, scope: &str) -> Result<()> {
        let path = self.path(scope)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {
                sync_parent(&path);
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// A fresh random secret that is a valid P-256 scalar.
pub(crate) fn generate() -> SoftwareSecret {
    loop {
        let secret = SoftwareSecret::new(rand::random::<[u8; 32]>());
        // Fails only for zero or a value >= the curve order (~2^-128).
        if SigningKey::from_slice(secret.expose_secret()).is_ok() {
            return secret;
        }
    }
}

/// The signing key of a stored secret.
pub(crate) fn signing_key(secret: &SoftwareSecret) -> Result<SigningKey> {
    SigningKey::from_slice(secret.expose_secret()).map_err(|_| DeviceKeyError::CorruptKey)
}

/// The key of `scope` in `store`, created when there is none.
pub(crate) fn open_or_create(store: &dyn SoftwareKeyStore, scope: &str) -> Result<SigningKey> {
    if let Some(secret) = store.load(scope)? {
        return signing_key(&secret);
    }
    let secret = generate();
    match store.store_new(scope, &secret)? {
        StoreOutcome::Stored => signing_key(&secret),
        StoreOutcome::Existing(existing) => signing_key(&existing),
    }
}

/// Parse a stored hex secret.
pub(crate) fn decode(stored: &str) -> Result<SoftwareSecret> {
    let mut secret = SoftwareSecret::new([0u8; 32]);
    hex::decode_to_slice(stored.trim(), secret.0.as_mut_slice())
        .map_err(|_| DeviceKeyError::CorruptKey)?;
    Ok(secret)
}

fn file_load(path: &Path) -> Result<Option<SoftwareSecret>> {
    match std::fs::read_to_string(path) {
        Ok(stored) => decode(&Zeroizing::new(stored)).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Create-if-absent: write a private temporary file, then hard-link it into
/// place, so a reader never sees a partial file and a racing creator never
/// overwrites the winner.
fn file_store_new(path: &Path, secret: &SoftwareSecret) -> Result<StoreOutcome> {
    file_store_new_with(path, secret, |from, to| std::fs::hard_link(from, to))
}

/// [`file_store_new`] with the link step injectable (tests).
pub(crate) fn file_store_new_with(
    path: &Path,
    secret: &SoftwareSecret,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<StoreOutcome> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temp = temp_path(path);
    let encoded = Zeroizing::new(hex::encode(secret.expose_secret()));
    let linked = write_private(&temp, &encoded).and_then(|()| link(&temp, path));
    // The temporary name is ours alone; it goes whatever happened.
    let _ = std::fs::remove_file(&temp);
    let created = match linked {
        Err(error) if link_unsupported(&error) => create_direct(path, &encoded),
        other => other,
    };
    match created {
        Ok(()) => {
            sync_parent(path);
            Ok(StoreOutcome::Stored)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => file_load(path)?
            .map(StoreOutcome::Existing)
            .ok_or(DeviceKeyError::CorruptKey),
        Err(error) => Err(error.into()),
    }
}

/// Whether `hard_link` failed because the file system has no hard links
/// (FAT/exFAT, some network and FUSE file systems).
fn link_unsupported(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::Unsupported {
        return true;
    }
    // ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED on Windows; EPERM on Linux
    // ("the file system does not support hard links"; the source is our own
    // fresh file, so no other EPERM cause applies).
    #[cfg(windows)]
    let codes: &[i32] = &[1, 50];
    #[cfg(target_os = "linux")]
    let codes: &[i32] = &[1];
    #[cfg(not(any(windows, target_os = "linux")))]
    let codes: &[i32] = &[];
    error
        .raw_os_error()
        .is_some_and(|code| codes.contains(&code))
}

/// Create `path` directly (no hard links): still create-if-absent, but a
/// concurrent reader may see the file before it is complete. A failed write
/// removes the partial file it made.
fn create_direct(path: &Path, contents: &str) -> std::io::Result<()> {
    write_private(path, contents).inspect_err(|error| {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(path);
        }
    })
}

/// The software key in file `path`, created when the file does not exist.
pub(crate) fn file_open_or_create(path: &Path) -> Result<SigningKey> {
    if let Some(secret) = file_load(path)? {
        return signing_key(&secret);
    }
    let secret = generate();
    match file_store_new(path, &secret)? {
        StoreOutcome::Stored => signing_key(&secret),
        StoreOutcome::Existing(existing) => signing_key(&existing),
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", hex::encode(rand::random::<[u8; 8]>())));
    path.with_file_name(name)
}

/// Create `path` (which must not exist) readable by the owner only, and
/// fsync it.
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

/// Best-effort fsync of `path`'s directory, so a new or removed name
/// survives a crash (Unix; Windows has no directory fsync).
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}
