//! The human login stored by `buzz auth login` (centralized identity, Phase 2).
//!
//! One session per relay origin. The short-lived access token (`bzs_…`, 1 h)
//! is cached in `session.json` (`<config dir>/buzz/session.json`, or
//! `BUZZ_SESSION_FILE`); the 90-day rotating refresh token (`bzr_…`) lives in
//! the OS credential store. When no credential store is available (a headless
//! Linux box without Secret Service), the refresh token falls back to the same
//! `0600` file, as `gh` does, and the login result says so. On Windows the file
//! has no explicit ACL: it inherits the per-user profile directory's
//! (`%APPDATA%`), which only the user and administrators can read.
//!
//! The stored session is the **last** credential source: `BUZZ_BOT_TOKEN`,
//! `BUZZ_ACCESS_TOKEN`, the acp token broker and a configured private key all
//! win over it, so an ambient login can never replace an identity a process
//! was explicitly given.
//!
//! Refresh rotation persists the new refresh token before anything else
//! (Rule 5: every prefix of the writes is consistent). If it cannot be
//! persisted anywhere the consumed session is forgotten and the command fails
//! with an auth error — never a silent success that loses the login.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use buzz_token_broker::Secret;
use serde::{Deserialize, Serialize};

use crate::error::CliError;

/// Overrides the session file path.
pub const SESSION_FILE_ENV: &str = "BUZZ_SESSION_FILE";
/// OS credential store service name for refresh tokens.
pub const KEYRING_SERVICE: &str = "buzz-cli";
/// Refresh when the cached access token has less than this many seconds left.
pub const REFRESH_SKEW_SECS: i64 = 120;
/// 401 codes from `POST /auth/refresh` that end the stored session.
pub const TERMINAL_REFRESH_CODES: [&str; 5] = [
    "invalid_token",
    "token_expired",
    "token_revoked",
    "refresh_reused",
    "principal_disabled",
];

const FILE_VERSION: u32 = 1;

/// Where a session's refresh token is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshStorage {
    /// The OS credential store.
    Keyring,
    /// The session file (credential store unavailable).
    File,
}

impl RefreshStorage {
    /// Stable name for JSON output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyring => "keyring",
            Self::File => "file",
        }
    }
}

/// One relay origin's stored login.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// The signed-in principal (hex).
    pub principal_id: String,
    /// This device's id on the relay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Cached access token (`bzs_…`).
    pub access: String,
    /// Access expiry, unix seconds.
    pub access_expires_at: i64,
    /// Refresh token, only when [`RefreshStorage::File`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSession")
            .field("principal_id", &self.principal_id)
            .field("device_id", &self.device_id)
            .field("access_expires_at", &self.access_expires_at)
            .field("refresh_in_file", &self.refresh.is_some())
            .finish_non_exhaustive()
    }
}

impl StoredSession {
    /// Where this session's refresh token lives.
    pub fn refresh_storage(&self) -> RefreshStorage {
        if self.refresh.is_some() {
            RefreshStorage::File
        } else {
            RefreshStorage::Keyring
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct SessionFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    sessions: BTreeMap<String, StoredSession>,
}

/// Tokens returned by `POST /auth/oidc/complete` or `POST /auth/refresh`.
#[derive(Debug, Clone)]
pub struct IssuedTokens {
    /// New access token.
    pub access: Secret,
    /// New refresh token.
    pub refresh: Secret,
    /// Access lifetime in seconds.
    pub expires_in: i64,
}

/// A secret store for refresh tokens, keyed by relay origin.
pub trait RefreshStore: Send + Sync {
    /// The stored refresh token for `origin`, if any.
    fn load(&self, origin: &str) -> Result<Option<Secret>, String>;
    /// Store (replace) the refresh token for `origin`.
    fn save(&self, origin: &str, refresh: &Secret) -> Result<(), String>;
    /// Remove the refresh token for `origin` (absent is not an error).
    fn delete(&self, origin: &str) -> Result<(), String>;
}

/// The OS credential store (`keyring`).
pub struct KeyringStore;

fn keyring_entry(origin: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, &format!("refresh:{origin}")).map_err(|e| e.to_string())
}

impl RefreshStore for KeyringStore {
    fn load(&self, origin: &str) -> Result<Option<Secret>, String> {
        match keyring_entry(origin)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save(&self, origin: &str, refresh: &Secret) -> Result<(), String> {
        keyring_entry(origin)?
            .set_password(refresh.expose())
            .map_err(|e| e.to_string())
    }

    fn delete(&self, origin: &str) -> Result<(), String> {
        match keyring_entry(origin)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

/// An in-process store (tests; never touches the OS credential store).
#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<BTreeMap<String, String>>,
    /// When set, every `save` fails (simulates an unavailable keyring).
    pub fail_saves: std::sync::atomic::AtomicBool,
}

impl RefreshStore for MemoryStore {
    fn load(&self, origin: &str) -> Result<Option<Secret>, String> {
        let entries = self.entries.lock().map_err(|e| e.to_string())?;
        Ok(entries.get(origin).cloned().map(Secret::new))
    }

    fn save(&self, origin: &str, refresh: &Secret) -> Result<(), String> {
        if self.fail_saves.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("credential store unavailable".into());
        }
        let mut entries = self.entries.lock().map_err(|e| e.to_string())?;
        entries.insert(origin.to_owned(), refresh.expose().to_owned());
        Ok(())
    }

    fn delete(&self, origin: &str) -> Result<(), String> {
        let mut entries = self.entries.lock().map_err(|e| e.to_string())?;
        entries.remove(origin);
        Ok(())
    }
}

/// The relay origin a session is keyed by (`scheme://host[:port]`).
pub fn relay_origin(relay_url: &str) -> Result<String, CliError> {
    let url = url::Url::parse(relay_url)
        .map_err(|e| CliError::Usage(format!("invalid relay URL {relay_url:?}: {e}")))?;
    match url.origin() {
        url::Origin::Tuple(..) => Ok(url.origin().ascii_serialization()),
        url::Origin::Opaque(_) => Err(CliError::Usage(format!(
            "relay URL {relay_url:?} has no origin"
        ))),
    }
}

/// `BUZZ_SESSION_FILE`, else `<config dir>/buzz/session.json`.
pub fn default_session_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SESSION_FILE_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    dirs::config_dir().map(|dir| dir.join("buzz").join("session.json"))
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Session file + refresh store for every relay origin.
#[derive(Clone)]
pub struct SessionStore {
    path: PathBuf,
    refresh: Arc<dyn RefreshStore>,
}

impl SessionStore {
    /// A store at `path` with refresh tokens in `refresh`.
    pub fn new(path: PathBuf, refresh: Arc<dyn RefreshStore>) -> Self {
        Self { path, refresh }
    }

    /// The default store: [`default_session_path`] + the OS credential store.
    pub fn open_default() -> Option<Self> {
        default_session_path().map(|path| Self::new(path, Arc::new(KeyringStore)))
    }

    /// The session file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_file(&self) -> Result<SessionFile, CliError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                CliError::Other(format!(
                    "session file {} is unreadable ({e}); remove it and run `buzz auth login`",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SessionFile::default())
            }
            Err(error) => Err(CliError::Other(format!(
                "cannot read session file {}: {error}",
                self.path.display()
            ))),
        }
    }

    /// Replace the file atomically (temp file + rename), owner-only on unix
    /// (file `0600`, a newly created directory `0700`). On Windows it relies on
    /// the user profile directory's inherited ACL (see the module doc).
    fn write_file(&self, file: &SessionFile) -> Result<(), CliError> {
        let fail = |error: std::io::Error| {
            CliError::Other(format!(
                "cannot write session file {}: {error}",
                self.path.display()
            ))
        };
        if let Some(parent) = self.path.parent() {
            let mut dirs = std::fs::DirBuilder::new();
            dirs.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt as _;
                dirs.mode(0o700);
            }
            dirs.create(parent).map_err(fail)?;
        }
        let bytes = serde_json::to_vec_pretty(file)
            .map_err(|e| CliError::Other(format!("cannot encode session file: {e}")))?;
        let tmp = self
            .path
            .with_extension(format!("json.tmp{}", std::process::id()));
        {
            use std::io::Write as _;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut out = options.open(&tmp).map_err(fail)?;
            // `mode` applies only on create; a stale temp file keeps its own.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                out.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(fail)?;
            }
            out.write_all(&bytes).map_err(fail)?;
            out.sync_all().map_err(fail)?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|error| {
            let _ = std::fs::remove_file(&tmp);
            fail(error)
        })
    }

    /// The stored session for `origin`.
    pub fn get(&self, origin: &str) -> Result<Option<StoredSession>, CliError> {
        Ok(self.read_file()?.sessions.remove(origin))
    }

    async fn refresh_op<T, F>(&self, op: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&dyn RefreshStore) -> Result<T, String> + Send + 'static,
    {
        // Credential-store backends may block (D-Bus, Keychain prompts).
        let store = Arc::clone(&self.refresh);
        tokio::task::spawn_blocking(move || op(store.as_ref()))
            .await
            .map_err(|e| format!("credential store task failed: {e}"))?
    }

    /// The refresh token of `session` at `origin`.
    pub async fn load_refresh(
        &self,
        origin: &str,
        session: &StoredSession,
    ) -> Result<Option<Secret>, CliError> {
        if let Some(refresh) = &session.refresh {
            return Ok(Some(Secret::new(refresh.clone())));
        }
        let origin = origin.to_owned();
        self.refresh_op(move |store| store.load(&origin))
            .await
            .map_err(|e| CliError::Auth(format!("cannot read the stored login: {e}")))
    }

    /// Persist a token snapshot so every prefix of the writes is recoverable
    /// (Rule 5), keeping the refresh token out of the file whenever the
    /// credential store works. Returns the authoritative storage.
    ///
    /// - Rotation over a verified keyring pointer (same principal and device,
    ///   refresh not in the file): keyring first, then the file with the new
    ///   access and no refresh. The refresh never touches the file.
    /// - First login, migration from the file, or a keyring that fails: a
    ///   complete snapshot (with refresh) is committed to the file first, then
    ///   the refresh moves to the keyring and the plaintext copy is removed
    ///   from the file. It stays only if the keyring is unavailable, or if that
    ///   final rewrite fails (reported as [`RefreshStorage::File`]).
    pub async fn save_tokens(
        &self,
        origin: &str,
        principal_id: &str,
        device_id: Option<String>,
        tokens: &IssuedTokens,
    ) -> Result<RefreshStorage, CliError> {
        let session = StoredSession {
            principal_id: principal_id.to_owned(),
            device_id,
            access: tokens.access.expose().to_owned(),
            access_expires_at: now_secs() + tokens.expires_in.max(0),
            refresh: Some(tokens.refresh.expose().to_owned()),
        };
        let mut file = self.read_file()?;
        file.version = FILE_VERSION;
        let has_pointer = file.sessions.get(origin).is_some_and(|old| {
            old.refresh.is_none()
                && old.principal_id == session.principal_id
                && old.device_id == session.device_id
        });
        // A verified existing keyring pointer can take the new refresh first:
        // a crash before the file rewrite leaves pointer -> new refresh. Never
        // for first login, another account/device, or a file-backed refresh.
        if has_pointer {
            let key = origin.to_owned();
            let refresh = tokens.refresh.clone();
            if self
                .refresh_op(move |store| store.save(&key, &refresh))
                .await
                .is_ok()
            {
                file.sessions.insert(
                    origin.to_owned(),
                    StoredSession {
                        refresh: None,
                        ..session
                    },
                );
                if let Err(error) = self.write_file(&file) {
                    // The pointer still resolves to the new refresh; only the
                    // access cache is stale.
                    eprintln!("warning: {error}; rotated refresh retained in credential store");
                }
                return Ok(RefreshStorage::Keyring);
            }
        }
        // Otherwise commit one complete recoverable snapshot before touching
        // the keyring (first login, file -> keyring migration, failed keyring).
        file.sessions.insert(origin.to_owned(), session);
        self.write_file(&file)?;
        let refresh = tokens.refresh.clone();
        let key = origin.to_owned();
        let in_keyring = self
            .refresh_op(move |store| store.save(&key, &refresh))
            .await
            .is_ok();
        if in_keyring {
            if let Some(session) = file.sessions.get_mut(origin) {
                session.refresh = None;
            }
            if let Err(error) = self.write_file(&file) {
                // The committed file snapshot still contains the NEW refresh
                // token, so report its actual storage location, not Keyring.
                eprintln!("warning: {error}; keeping refresh token in session file");
                return Ok(RefreshStorage::File);
            }
        } else {
            // A stale keyring entry must not shadow the file copy later.
            let key = origin.to_owned();
            let _ = self.refresh_op(move |store| store.delete(&key)).await;
        }
        Ok(if in_keyring {
            RefreshStorage::Keyring
        } else {
            RefreshStorage::File
        })
    }

    /// Forget `origin`'s session (file entry and credential-store entry).
    pub async fn forget(&self, origin: &str) -> Result<(), CliError> {
        let key = origin.to_owned();
        let keyring = self.refresh_op(move |store| store.delete(&key)).await;
        let mut file = self.read_file()?;
        if file.sessions.remove(origin).is_some() {
            self.write_file(&file)?;
        }
        keyring.map_err(|e| CliError::Other(format!("cannot remove the stored login: {e}")))
    }
}

/// Exchange `refresh` at `POST /auth/refresh`. A terminal 401 is
/// [`RefreshError::Terminal`]; everything else is transient.
pub async fn post_refresh(
    http: &reqwest::Client,
    relay_url: &str,
    refresh: &Secret,
) -> Result<IssuedTokens, RefreshError> {
    let resp = http
        .post(format!("{relay_url}/auth/refresh"))
        .json(&serde_json::json!({ "refresh": refresh.expose() }))
        .send()
        .await
        .map_err(|e| RefreshError::Transient(CliError::Network(e)))?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .await
        .map_err(|e| RefreshError::Transient(CliError::Network(e)))?;
    let parsed = serde_json::from_str::<serde_json::Value>(&body).ok();
    if status == 401 {
        let code = parsed
            .as_ref()
            .and_then(|v| v.get("code"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        if TERMINAL_REFRESH_CODES.contains(&code) {
            return Err(RefreshError::Terminal(code.to_owned()));
        }
    }
    if !(200..300).contains(&status) {
        return Err(RefreshError::Transient(CliError::Relay {
            status,
            body: format!("token refresh failed: {body}"),
        }));
    }
    let get = |field: &str| {
        parsed
            .as_ref()
            .and_then(|v| v.get(field))
            .and_then(|v| v.as_str())
            .map(|s| Secret::new(s.to_owned()))
    };
    match (get("access"), get("refresh")) {
        (Some(access), Some(refresh)) => Ok(IssuedTokens {
            access,
            refresh,
            expires_in: parsed
                .as_ref()
                .and_then(|v| v.get("expires_in"))
                .and_then(|v| v.as_i64())
                .unwrap_or(3600),
        }),
        _ => Err(RefreshError::Transient(CliError::Other(
            "relay returned an incomplete refresh response".into(),
        ))),
    }
}

/// Why a refresh did not produce tokens.
#[derive(Debug)]
pub enum RefreshError {
    /// The session is over (401 with a terminal code).
    Terminal(String),
    /// Retry later; the stored session is kept.
    Transient(CliError),
}

/// The terminal "log in again" error.
pub fn login_required(reason: &str) -> CliError {
    CliError::Auth(format!(
        "{reason}: the stored login has ended; run `buzz auth login`"
    ))
}

/// Why the stored session could not produce an access token.
#[derive(Debug)]
pub enum SessionError {
    /// The relay ended the session (terminal refresh code); it was forgotten.
    Ended(String),
    /// Anything else (network, relay, storage); the session is kept unless
    /// the rotated login could not be stored.
    Failed(CliError),
}

impl From<CliError> for SessionError {
    fn from(error: CliError) -> Self {
        Self::Failed(error)
    }
}

impl From<SessionError> for CliError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Ended(code) => login_required(&code),
            SessionError::Failed(error) => error,
        }
    }
}

/// An access token for `relay_url` from the stored session, refreshing it
/// when it is within [`REFRESH_SKEW_SECS`] of expiry (or `force_refresh`).
/// `Ok(None)` when no session is stored for this relay.
///
/// A terminal refresh rejection forgets the session and is an auth error
/// (exit 3); a network or relay failure keeps it and is retryable (exit 2).
pub async fn session_access_token(
    http: &reqwest::Client,
    relay_url: &str,
    store: &SessionStore,
    force_refresh: bool,
) -> Result<Option<(Secret, StoredSession)>, CliError> {
    session_access_token_detailed(http, relay_url, store, force_refresh)
        .await
        .map_err(CliError::from)
}

/// [`session_access_token`] with the "session ended" outcome kept distinct.
pub async fn session_access_token_detailed(
    http: &reqwest::Client,
    relay_url: &str,
    store: &SessionStore,
    force_refresh: bool,
) -> Result<Option<(Secret, StoredSession)>, SessionError> {
    let origin = relay_origin(relay_url)?;
    let Some(session) = store.get(&origin)? else {
        return Ok(None);
    };
    if !force_refresh && session.access_expires_at - now_secs() > REFRESH_SKEW_SECS {
        return Ok(Some((Secret::new(session.access.clone()), session)));
    }
    let Some(refresh) = store.load_refresh(&origin, &session).await? else {
        store.forget(&origin).await?;
        return Err(SessionError::Ended("no refresh token is stored".into()));
    };
    match post_refresh(http, relay_url, &refresh).await {
        Ok(tokens) => {
            if let Err(error) = store
                .save_tokens(
                    &origin,
                    &session.principal_id,
                    session.device_id.clone(),
                    &tokens,
                )
                .await
            {
                // The old refresh token is consumed and the new one could not
                // be stored anywhere: the login is lost. Drop the consumed
                // copy so a later run cannot present it (that would trip the
                // relay's reuse detection).
                let _ = store.forget(&origin).await;
                return Err(SessionError::Failed(CliError::Auth(format!(
                    "cannot store the rotated login ({error}); run `buzz auth login`"
                ))));
            }
            let session = store.get(&origin)?.unwrap_or(session);
            Ok(Some((tokens.access, session)))
        }
        Err(RefreshError::Terminal(code)) => {
            store.forget(&origin).await?;
            Err(SessionError::Ended(code))
        }
        Err(RefreshError::Transient(error)) => Err(SessionError::Failed(error)),
    }
}
