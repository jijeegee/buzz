//! Software device keys and where they are kept: [`SoftwareKeyStore`]
//! (each app brings its own keyring; this crate ships only [`FileStore`]).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use p256::ecdsa::SigningKey;
use zeroize::Zeroizing;

use crate::{validate_scope, DeviceKeyError, Result};

/// A software device key's 32-byte P-256 secret, wiped on drop.
pub type SoftwareSecret = Zeroizing<[u8; 32]>;

/// What [`SoftwareKeyStore::store_new`] did.
pub enum StoreOutcome {
    /// The new secret is now the scope's key.
    Stored,
    /// The scope already had a key (a racing creator won); use this one.
    Existing(SoftwareSecret),
}

/// Keeps software device keys, one per scope. Apps implement it with their
/// own OS keyring version (one keyring major per app); this crate provides
/// [`FileStore`]. Report backend failures as [`DeviceKeyError::Store`].
pub trait SoftwareKeyStore {
    /// The key of `scope`, or `None` when the scope has none.
    fn load(&self, scope: &str) -> Result<Option<SoftwareSecret>>;

    /// Store `secret` as the key of `scope` only if the scope has none
    /// (create-if-absent), else return the existing key. A backend without
    /// an atomic create-if-absent (most OS keyrings) should load, write when
    /// absent, then load again and return [`StoreOutcome::Existing`] when
    /// the read-back differs. Its residual race: two creators both see no
    /// key, both write, and the first may already have used its own key
    /// before the second overwrites it; that device key then changes once.
    fn store_new(&self, scope: &str, secret: &SoftwareSecret) -> Result<StoreOutcome>;
}

/// Software keys as files `<dir>/<scope>.key` (hex secret, Unix mode 0600).
/// Creation is atomic create-if-absent, so racing creators agree on one key.
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
        validate_scope(scope)?;
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
}

/// A fresh random secret that is a valid P-256 scalar.
pub(crate) fn generate() -> SoftwareSecret {
    loop {
        let secret = Zeroizing::new(rand::random::<[u8; 32]>());
        // Fails only for zero or a value >= the curve order (~2^-128).
        if SigningKey::from_slice(secret.as_slice()).is_ok() {
            return secret;
        }
    }
}

/// The signing key of a stored secret.
pub(crate) fn signing_key(secret: &SoftwareSecret) -> Result<SigningKey> {
    SigningKey::from_slice(secret.as_slice()).map_err(|_| DeviceKeyError::CorruptKey)
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
    let mut secret = Zeroizing::new([0u8; 32]);
    hex::decode_to_slice(stored.trim(), secret.as_mut_slice())
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
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temp = temp_path(path);
    let encoded = Zeroizing::new(hex::encode(secret.as_slice()));
    let linked = write_private(&temp, &encoded).and_then(|()| std::fs::hard_link(&temp, path));
    // The temporary name is ours alone; it goes whatever happened.
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => Ok(StoreOutcome::Stored),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => file_load(path)?
            .map(StoreOutcome::Existing)
            .ok_or(DeviceKeyError::CorruptKey),
        Err(error) => Err(error.into()),
    }
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
