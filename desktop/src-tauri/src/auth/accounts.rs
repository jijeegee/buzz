//! Buzz accounts on this computer.
//!
//! Several accounts can be signed in at once; exactly one is active, and its
//! key is `AppState::keys`. An account is a (relay origin, signing pubkey)
//! pair.
//!
//! The source of truth for an account's secrets is its own slots in OS secure
//! storage: `account.{pubkey}.identity` and `account.{pubkey}.refresh.{origin}`.
//! `accounts.json` in the app data directory lists the accounts (no secrets),
//! and the `account.index.{n}` secure-storage entries mirror that list so it
//! can be rebuilt when the file is lost (and so reset can find every slot).
//!
//! The rest of the app keeps reading the original single-account slots
//! (`identity`, `auth.refresh.{origin}`, `auth.key-backup.{origin}`). Those are
//! a copy of the active account: always written *after* the account's own
//! slots succeed, and refilled from them at launch, so a failure between the
//! two writes can never leave an account with a stale (server-consumed)
//! refresh token.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use nostr::{Keys, PublicKey, ToBech32};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// File name of the account list inside the app data directory.
pub(crate) const FILE_NAME: &str = "accounts.json";

/// Secure-storage entries mirroring the account list, in chunks small enough
/// for one Windows credential each.
pub(crate) fn index_chunk_key(n: usize) -> String {
    format!("account.index.{n}")
}

/// Characters per index chunk (a Windows credential holds 1,280 UTF-16 units).
const INDEX_CHUNK_CHARS: usize = 1000;

/// Upper bound on index chunks read or cleaned up.
pub(crate) const MAX_INDEX_CHUNKS: usize = 64;

/// Serialises every read-modify-write of the account list in this process.
static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

fn registry_lock() -> std::sync::MutexGuard<'static, ()> {
    REGISTRY_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Origin recorded for an account that is not bound to any relay login (a
/// key-only identity from before Google sign-in).
pub(crate) const ANY_ORIGIN: &str = "*";

const VERSION: u8 = 1;

/// The account's signing key slot.
pub(crate) fn identity_slot(pubkey: &PublicKey) -> String {
    format!("account.{}.identity", pubkey.to_hex())
}

/// The account's refresh-token slot for `origin`.
pub(crate) fn refresh_slot(pubkey: &PublicKey, origin: &str) -> String {
    format!("account.{}.refresh.{origin}", pubkey.to_hex())
}

/// Whether an account is usable on its relay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountState {
    /// Signed in: its session is kept fresh in the background.
    SignedIn,
    /// Signed out on this computer: kept in the list, never refreshed.
    SignedOut,
    /// A local key with no relay login (pre-Google identity).
    KeyOnly,
    /// Its background session ended (revoked elsewhere, expired): listed, not
    /// refreshed, until it signs in again.
    NeedsLogin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Account {
    pub pubkey: String,
    pub origin: String,
    pub state: AccountState,
}

/// A key generated for a login method that had no account yet, kept until its
/// backup upload is confirmed so a retry uploads the same key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingKey {
    pub origin: String,
    pub account_id: String,
    pub pubkey: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Registry {
    pub version: u8,
    /// Hex pubkey of the active account.
    pub active: Option<String>,
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub pending: Vec<PendingKey>,
    /// Older agent records got their owner written once.
    #[serde(default)]
    pub owners_migrated: bool,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: VERSION,
            active: None,
            accounts: Vec::new(),
            pending: Vec::new(),
            owners_migrated: false,
        }
    }
}

impl Registry {
    /// Insert or update the (origin, pubkey) account. A key-only placeholder
    /// for the same pubkey is replaced by its first relay login.
    pub(crate) fn upsert(&mut self, origin: &str, pubkey: &PublicKey, state: AccountState) {
        let hex = pubkey.to_hex();
        if origin != ANY_ORIGIN {
            self.accounts
                .retain(|a| !(a.pubkey == hex && a.origin == ANY_ORIGIN));
        }
        match self
            .accounts
            .iter_mut()
            .find(|a| a.pubkey == hex && a.origin == origin)
        {
            Some(account) => account.state = state,
            None => self.accounts.push(Account {
                pubkey: hex,
                origin: origin.to_owned(),
                state,
            }),
        }
    }

    pub(crate) fn get(&self, origin: &str, pubkey: &PublicKey) -> Option<&Account> {
        let hex = pubkey.to_hex();
        self.accounts
            .iter()
            .find(|a| a.pubkey == hex && a.origin == origin)
    }

    pub(crate) fn contains_pubkey(&self, pubkey: &PublicKey) -> bool {
        let hex = pubkey.to_hex();
        self.accounts.iter().any(|a| a.pubkey == hex)
    }

    pub(crate) fn active_pubkey(&self) -> Option<PublicKey> {
        self.active
            .as_deref()
            .and_then(|hex| PublicKey::from_hex(hex).ok())
    }

    /// Signed-in accounts other than the active one: the ones refreshed in
    /// the background.
    pub(crate) fn inactive_signed_in(&self) -> Vec<(String, PublicKey)> {
        self.accounts
            .iter()
            .filter(|a| a.state == AccountState::SignedIn)
            .filter(|a| Some(&a.pubkey) != self.active.as_ref())
            .filter_map(|a| {
                PublicKey::from_hex(&a.pubkey)
                    .ok()
                    .map(|key| (a.origin.clone(), key))
            })
            .collect()
    }

    fn pending_for(&self, origin: &str, account_id: &str) -> Option<PublicKey> {
        self.pending
            .iter()
            .find(|p| p.origin == origin && p.account_id == account_id)
            .and_then(|p| PublicKey::from_hex(&p.pubkey).ok())
    }

    fn clear_pending(&mut self, origin: &str, pubkey: &PublicKey) {
        let hex = pubkey.to_hex();
        self.pending
            .retain(|p| !(p.origin == origin && p.pubkey == hex));
    }

    /// The compact mirror stored in the `account.index.{n}` entries: one `pubkey origin state`
    /// line per account, active account first.
    fn index_text(&self) -> String {
        let mut lines = Vec::new();
        let mut accounts: Vec<&Account> = self.accounts.iter().collect();
        accounts.sort_by_key(|a| (Some(&a.pubkey) != self.active.as_ref(), a.pubkey.clone()));
        for account in accounts {
            let state = match account.state {
                AccountState::SignedIn => "in",
                AccountState::SignedOut => "out",
                AccountState::KeyOnly => "key",
                AccountState::NeedsLogin => "need",
            };
            lines.push(format!("{} {} {}", account.pubkey, account.origin, state));
        }
        // Pending keys are listed so a reset can find their slots too.
        for pending in &self.pending {
            lines.push(format!(
                "{} {} pending {}",
                pending.pubkey, pending.origin, pending.account_id
            ));
        }
        lines.join("\n")
    }

    fn from_index(text: &str) -> Result<Self, String> {
        let mut registry = Registry::default();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let mut parts = line.split(' ');
            let (Some(pubkey), Some(origin), Some(state), extra) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return Err("invalid account index".into());
            };
            let pubkey = PublicKey::from_hex(pubkey).map_err(|_| "invalid account index")?;
            if state == "pending" {
                let account_id = extra.ok_or("invalid account index")?;
                registry.pending.push(PendingKey {
                    origin: origin.to_owned(),
                    account_id: account_id.to_owned(),
                    pubkey: pubkey.to_hex(),
                });
                continue;
            }
            if extra.is_some() {
                return Err("invalid account index".into());
            }
            let state = match state {
                "in" => AccountState::SignedIn,
                "out" => AccountState::SignedOut,
                "key" => AccountState::KeyOnly,
                "need" => AccountState::NeedsLogin,
                _ => return Err("invalid account index".into()),
            };
            if registry.active.is_none() {
                registry.active = Some(pubkey.to_hex());
            }
            registry.upsert(origin, &pubkey, state);
        }
        Ok(registry)
    }
}

/// Secure storage as the account code needs it. Abstracted so tests never
/// touch the OS keyring.
pub(crate) trait Secrets {
    fn load(&self, key: &str) -> Result<Option<String>, String>;
    fn store(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
    /// Atomically store the active-account copy (one blob mutation).
    fn store_all(&self, entries: &HashMap<String, String>) -> Result<(), String>;
    /// Every key held in the shared blob (standalone credentials included as
    /// their markers).
    fn blob_keys(&self) -> Result<Vec<String>, String>;
}

impl Secrets for crate::secret_store::SecretStore {
    fn load(&self, key: &str) -> Result<Option<String>, String> {
        crate::secret_store::SecretStore::load(self, key)
    }
    fn store(&self, key: &str, value: &str) -> Result<(), String> {
        crate::secret_store::SecretStore::store(self, key, value)
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        crate::secret_store::SecretStore::delete(self, key)
    }
    fn store_all(&self, entries: &HashMap<String, String>) -> Result<(), String> {
        crate::secret_store::SecretStore::store_all(self, entries)
    }
    fn blob_keys(&self) -> Result<Vec<String>, String> {
        Ok(self
            .load_all_readonly()?
            .map(|map| map.into_keys().collect())
            .unwrap_or_default())
    }
}

/// The OS secure storage of this build.
pub(crate) fn shared_secrets() -> &'static crate::secret_store::SecretStore {
    crate::secret_store::SecretStore::shared(crate::app_state::keyring_service())
}

fn store_verified(secrets: &dyn Secrets, key: &str, value: &str) -> Result<(), String> {
    secrets.store(key, value)?;
    if secrets.load(key)?.as_deref() != Some(value) {
        return Err("OS secure storage verification failed".into());
    }
    Ok(())
}

fn nsec_of(keys: &Keys) -> Result<Zeroizing<String>, String> {
    keys.secret_key()
        .to_bech32()
        .map(Zeroizing::new)
        .map_err(|_| "could not encode signing key".to_owned())
}

/// Read `account`'s signing key from its own slot.
pub(crate) fn account_keys(secrets: &dyn Secrets, pubkey: &PublicKey) -> Result<Keys, String> {
    let nsec = Zeroizing::new(
        secrets
            .load(&identity_slot(pubkey))?
            .ok_or("This account's key is not on this computer.")?,
    );
    let keys = Keys::parse(nsec.as_str()).map_err(|_| "This account's stored key is invalid.")?;
    if keys.public_key() != *pubkey {
        return Err("This account's stored key is invalid.".into());
    }
    Ok(keys)
}

// ── Registry file ──────────────────────────────────────────────────────────

pub(crate) fn registry_path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// Load `accounts.json`. `Ok(None)` when it does not exist.
pub(crate) fn load_registry(data_dir: &Path) -> Result<Option<Registry>, String> {
    let bytes = match std::fs::read(registry_path(data_dir)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("account list unreadable: {error}")),
    };
    let registry: Registry =
        serde_json::from_slice(&bytes).map_err(|e| format!("account list invalid: {e}"))?;
    if registry.version != VERSION {
        return Err(format!(
            "account list has unsupported version {}",
            registry.version
        ));
    }
    Ok(Some(registry))
}

fn write_registry_file(data_dir: &Path, registry: &Registry) -> Result<(), String> {
    use atomic_write_file::AtomicWriteFile;
    use std::io::Write;

    let json = serde_json::to_vec_pretty(registry).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(data_dir).map_err(|e| format!("account list: {e}"))?;
    let mut file =
        AtomicWriteFile::open(registry_path(data_dir)).map_err(|e| format!("account list: {e}"))?;
    file.write_all(&json)
        .map_err(|e| format!("account list: {e}"))?;
    file.commit().map_err(|e| format!("account list: {e}"))
}

/// Persist the account list: the secure-storage index first (it is what
/// rebuilds a lost file), then the file.
pub(crate) fn save_registry(
    secrets: &dyn Secrets,
    data_dir: &Path,
    registry: &Registry,
) -> Result<(), String> {
    write_index(secrets, &registry.index_text())?;
    write_registry_file(data_dir, registry)
}

/// Write the index in chunks (new chunks first, then drop leftovers).
fn write_index(secrets: &dyn Secrets, text: &str) -> Result<(), String> {
    let mut chunks: Vec<String> = vec![String::new()];
    for line in text.lines() {
        let last = chunks.last_mut().expect("one chunk");
        if !last.is_empty() && last.len() + 1 + line.len() > INDEX_CHUNK_CHARS {
            chunks.push(String::new());
        }
        let last = chunks.last_mut().expect("one chunk");
        if !last.is_empty() {
            last.push('\n');
        }
        last.push_str(line);
    }
    if chunks.len() > MAX_INDEX_CHUNKS {
        return Err("Too many accounts on this computer.".into());
    }
    for (n, chunk) in chunks.iter().enumerate() {
        store_verified(secrets, &index_chunk_key(n), chunk)?;
    }
    for n in chunks.len()..MAX_INDEX_CHUNKS {
        if secrets.load(&index_chunk_key(n))?.is_none() {
            break;
        }
        secrets.delete(&index_chunk_key(n))?;
    }
    Ok(())
}

/// The whole index text, or `None` when there is none.
fn read_index(secrets: &dyn Secrets) -> Result<Option<String>, String> {
    let mut parts = Vec::new();
    for n in 0..MAX_INDEX_CHUNKS {
        match secrets.load(&index_chunk_key(n))? {
            Some(chunk) => parts.push(chunk),
            None => break,
        }
    }
    let text = parts.join("\n");
    Ok((!text.trim().is_empty()).then_some(text))
}

/// Local agents in the agent store under `data_dir` (0 when there is none).
pub(crate) fn local_agent_count(data_dir: &Path) -> usize {
    std::fs::read(data_dir.join("agents").join("managed-agents.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<serde_json::Value>>(&bytes).ok())
        .map(|records| {
            records
                .iter()
                .filter(|r| r["pubkey"].as_str().is_some_and(|p| !p.is_empty()))
                .count()
        })
        .unwrap_or(0)
}

/// Record, once, the owner of older agents that carry no attestation: the key
/// that is active right now, before any other account can become active.
fn ensure_owners_recorded(data_dir: &Path, registry: &mut Registry, owner: &PublicKey) {
    if registry.owners_migrated {
        return;
    }
    let agents_dir = data_dir.join("agents");
    let records = std::fs::read(agents_dir.join("managed-agents.json"))
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<Vec<crate::managed_agents::ManagedAgentRecord>>(&bytes).ok()
        })
        .unwrap_or_default();
    let path = agents_dir.join(crate::managed_agents::AGENT_OWNERS_FILE);
    match crate::managed_agents::record_missing_owners(&path, &records, owner) {
        Ok(_) => registry.owners_migrated = true,
        Err(error) => eprintln!("buzz-desktop: accounts: agent owners not recorded: {error}"),
    }
}

/// Keep the key that is about to stop being active: its own slot and a place
/// in the list (key-only when it is bound nowhere), so replacing the active
/// copy never loses it. An unused first-install key is not kept.
fn preserve_outgoing(
    secrets: &dyn Secrets,
    data_dir: &Path,
    registry: &mut Registry,
    outgoing: &Keys,
) -> Result<(), String> {
    let pubkey = outgoing.public_key();
    ensure_owners_recorded(data_dir, registry, &pubkey);
    let marker = secrets.load("identity.bootstrap")?;
    if is_unused_bootstrap_key(
        marker.as_deref(),
        &pubkey,
        registry.accounts.len(),
        local_agent_count(data_dir),
    ) {
        return Ok(());
    }
    if !registry.contains_pubkey(&pubkey) {
        // Listed first, so reset can always find the slots written next.
        let bindings = bound_origins(secrets, &pubkey)?;
        if bindings.is_empty() {
            registry.upsert(ANY_ORIGIN, &pubkey, AccountState::KeyOnly);
        }
        for origin in &bindings {
            registry.upsert(origin, &pubkey, AccountState::SignedOut);
        }
        save_registry(secrets, data_dir, registry)?;
        store_verified(secrets, &identity_slot(&pubkey), &nsec_of(outgoing)?)?;
        for origin in &bindings {
            let state = copy_refresh_into_slot(secrets, &pubkey, origin)?;
            registry.upsert(origin, &pubkey, state);
        }
    } else if secrets.load(&identity_slot(&pubkey))?.is_none() {
        store_verified(secrets, &identity_slot(&pubkey), &nsec_of(outgoing)?)?;
    }
    Ok(())
}

// ── First-install key ──────────────────────────────────────────────────────

/// Whether `pubkey` is the throwaway key a fresh install generated and never
/// used: the bootstrap marker still names it (the app clears it on the first
/// meaningful publication), no account is registered on this computer, and no
/// agent was created under it. Only such a key may become a new account's key
/// on sign-in; every established key keeps its own account.
pub(crate) fn is_unused_bootstrap_key(
    bootstrap_marker: Option<&str>,
    pubkey: &PublicKey,
    registered_accounts: usize,
    agents: usize,
) -> bool {
    bootstrap_marker == Some(pubkey.to_hex().as_str()) && registered_accounts == 0 && agents == 0
}

// ── Sign-in commit and new keys ────────────────────────────────────────────

/// The key for a login method that has no account yet: the key generated by
/// an earlier unconfirmed attempt for the same (origin, account id), or a new
/// one. The key is in its own secure slot (verified) and listed as pending
/// before it is returned, so an uncertain upload never strands it.
pub(crate) fn pending_new_key(
    secrets: &dyn Secrets,
    data_dir: &Path,
    origin: &str,
    account_id: &str,
) -> Result<Keys, String> {
    let _lock = registry_lock();
    let mut registry = load_registry(data_dir)?.unwrap_or_default();
    if let Some(pubkey) = registry.pending_for(origin, account_id) {
        if let Ok(keys) = account_keys(secrets, &pubkey) {
            return Ok(keys);
        }
    }
    let keys = Keys::generate();
    let nsec = nsec_of(&keys)?;
    // Listed first, so reset can always find the slot written next.
    registry
        .pending
        .retain(|p| !(p.origin == origin && p.account_id == account_id));
    registry.pending.push(PendingKey {
        origin: origin.to_owned(),
        account_id: account_id.to_owned(),
        pubkey: keys.public_key().to_hex(),
    });
    save_registry(secrets, data_dir, &registry)?;
    store_verified(secrets, &identity_slot(&keys.public_key()), &nsec)?;
    Ok(keys)
}

/// An `identity.key` file left from file-based storage that holds another key
/// would block a sign-in (`prepare_recovered_identity`). Keep that key as an
/// account of its own first, then set the file aside, so signing in to any
/// account never compares against it or loses it.
pub(crate) fn preserve_legacy_key_file(
    secrets: &dyn Secrets,
    data_dir: &Path,
    incoming: &Keys,
) -> Result<(), String> {
    let path = data_dir.join("identity.key");
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let contents = Zeroizing::new(contents);
    let Ok(file_keys) = Keys::parse(contents.trim()) else {
        return Ok(());
    };
    if file_keys.public_key() == incoming.public_key() {
        return Ok(());
    }
    let _lock = registry_lock();
    let mut registry = load_registry(data_dir)?.unwrap_or_default();
    let pubkey = file_keys.public_key();
    if !registry.contains_pubkey(&pubkey) {
        registry.upsert(ANY_ORIGIN, &pubkey, AccountState::KeyOnly);
        save_registry(secrets, data_dir, &registry)?;
    }
    if secrets.load(&identity_slot(&pubkey))?.is_none() {
        store_verified(secrets, &identity_slot(&pubkey), &nsec_of(&file_keys)?)?;
    }
    let aside = data_dir.join(format!("identity.key.account-{}", &pubkey.to_hex()[..16]));
    std::fs::rename(&path, &aside).map_err(|e| format!("could not set aside identity.key: {e}"))
}

/// Durably record a completed sign-in of `keys` on `origin` and make it the
/// active account in place of `outgoing` (`None` while recovering a lost key).
/// Order: `outgoing` is kept (listed, own slot), the new account is listed,
/// its own key and refresh slots are
/// written and verified, then it becomes active in the list, and only then is
/// the active copy replaced. Every other account stays as it is.
pub(crate) fn commit_sign_in(
    secrets: &dyn Secrets,
    data_dir: &Path,
    origin: &str,
    keys: &Keys,
    refresh_token: &str,
    outgoing: Option<&Keys>,
) -> Result<Registry, String> {
    let _lock = registry_lock();
    let pubkey = keys.public_key();
    let nsec = nsec_of(keys)?;
    let mut registry = load_registry(data_dir)?.unwrap_or_default();
    match outgoing {
        Some(outgoing) if outgoing.public_key() != pubkey => {
            preserve_outgoing(secrets, data_dir, &mut registry, outgoing)?;
        }
        // Same account, or recovering a lost key: the in-memory key was a
        // stand-in nobody owns, so never keep it or record it as an owner.
        _ => ensure_owners_recorded(data_dir, &mut registry, &pubkey),
    }
    if registry.get(origin, &pubkey).is_none() {
        registry.upsert(origin, &pubkey, AccountState::SignedOut);
        save_registry(secrets, data_dir, &registry)?;
    }
    store_verified(secrets, &identity_slot(&pubkey), &nsec)?;
    store_verified(secrets, &refresh_slot(&pubkey, origin), refresh_token)?;
    registry.upsert(origin, &pubkey, AccountState::SignedIn);
    registry.active = Some(pubkey.to_hex());
    registry.clear_pending(origin, &pubkey);
    save_registry(secrets, data_dir, &registry)?;
    write_active_copy(secrets, keys, Some((origin, refresh_token)))?;
    Ok(registry)
}

/// What a sign-in of an account on an origin may change, read before it
/// commits so a failed commit can be undone ([`roll_back_sign_in`]).
pub(crate) struct SignInBefore {
    active: Option<String>,
    state: Option<AccountState>,
    token: Option<Zeroizing<String>>,
}

impl SignInBefore {
    /// The account's refresh token on the origin before the sign-in.
    pub(crate) fn old_token(&self) -> Option<&str> {
        self.token.as_ref().map(|token| token.as_str())
    }
}

pub(crate) fn sign_in_before(
    secrets: &dyn Secrets,
    data_dir: &Path,
    origin: &str,
    pubkey: &PublicKey,
) -> Result<SignInBefore, String> {
    let _lock = registry_lock();
    let registry = load_registry(data_dir)?.unwrap_or_default();
    Ok(SignInBefore {
        active: registry.active.clone(),
        state: registry.get(origin, pubkey).map(|a| a.state),
        token: secrets
            .load(&refresh_slot(pubkey, origin))?
            .map(Zeroizing::new),
    })
}

/// Undo what a failed [`commit_sign_in`] of `pubkey` on `origin` left behind,
/// as far as it got: its own token slot holds the old token again (or, with
/// none before, not the new one, whose session the caller revokes), the list
/// no longer makes it active unless it was, and its state is the old one
/// (`SignedOut` when no old session is left). Returns whether the old session
/// is back in its slot.
pub(crate) fn roll_back_sign_in(
    secrets: &dyn Secrets,
    data_dir: &Path,
    origin: &str,
    pubkey: &PublicKey,
    new_token: &str,
    before: &SignInBefore,
) -> Result<bool, String> {
    let _lock = registry_lock();
    let slot = refresh_slot(pubkey, origin);
    let current = secrets.load(&slot)?;
    let restored = match &before.token {
        Some(old) => {
            if current.as_deref() != Some(old.as_str()) {
                store_verified(secrets, &slot, old)?;
            }
            true
        }
        None => {
            if current.as_deref() == Some(new_token) {
                secrets.delete(&slot)?;
            }
            false
        }
    };
    let Some(mut registry) = load_registry(data_dir)? else {
        return Ok(restored);
    };
    let hex = pubkey.to_hex();
    let mut changed = false;
    if registry.active.as_deref() == Some(hex.as_str())
        && before.active.as_deref() != Some(hex.as_str())
    {
        registry.active = before.active.clone();
        changed = true;
    }
    let state = match before.state {
        Some(state) if restored || state != AccountState::SignedIn => state,
        _ => AccountState::SignedOut,
    };
    if registry
        .get(origin, pubkey)
        .is_some_and(|account| account.state != state)
    {
        registry.upsert(origin, pubkey, state);
        changed = true;
    }
    if changed {
        save_registry(secrets, data_dir, &registry)?;
    }
    Ok(restored)
}

/// Write the active-account copy the rest of the app reads: the key, and for
/// `signed_in` = (origin, refresh token) that origin's binding and token. Other
/// origins are handed over under their own refresh locks
/// ([`hand_over_origins`]) or at launch ([`refill_copies`]), never here, so a
/// session still running there can never write into the wrong account.
fn write_active_copy(
    secrets: &dyn Secrets,
    keys: &Keys,
    signed_in: Option<(&str, &str)>,
) -> Result<(), String> {
    let pubkey = keys.public_key();
    let nsec = nsec_of(keys)?;
    let mut entries = HashMap::from([
        ("identity".to_owned(), nsec.to_string()),
        ("identity.bootstrap".to_owned(), String::new()),
    ]);
    if let Some((origin, token)) = signed_in {
        entries.insert(super::key_recovery::binding_key(origin), pubkey.to_hex());
        entries.insert(super::refresh_key(origin), token.to_owned());
    }
    let result = secrets.store_all(&entries);
    for value in entries.values_mut() {
        zeroize::Zeroize::zeroize(value);
    }
    result?;
    if secrets.load("identity")?.as_deref() != Some(nsec.as_str()) {
        return Err("OS secure storage verification failed; retry sign-in.".into());
    }
    Ok(())
}

/// A rotated refresh token of the active account on `origin`: its own slot
/// first (the truth; failure is returned, so the caller drops the consumed
/// token), then the active copy. A failed copy write is not an error: the copy
/// is dropped instead and refilled from the own slot at the next launch, so the
/// caller never deletes the freshly rotated own slot.
pub(crate) fn store_active_refresh(
    secrets: &dyn Secrets,
    origin: &str,
    active: Option<PublicKey>,
    token: &str,
) -> Result<(), String> {
    let Some(pubkey) = active else {
        return secrets.store(&super::refresh_key(origin), token);
    };
    store_verified(secrets, &refresh_slot(&pubkey, origin), token)?;
    if let Err(error) = secrets.store(&super::refresh_key(origin), token) {
        eprintln!("buzz-desktop: accounts: active token copy not written ({error}); dropped");
        let _ = secrets.delete(&super::refresh_key(origin));
    }
    Ok(())
}

/// Mark `pubkey`'s background session on `origin` as ended.
pub(crate) fn mark_needs_login(
    secrets: &dyn Secrets,
    data_dir: &Path,
    origin: &str,
    pubkey: &PublicKey,
) -> Result<(), String> {
    let _lock = registry_lock();
    let mut registry = load_registry(data_dir)?.unwrap_or_default();
    if registry.get(origin, pubkey).map(|a| a.state) != Some(AccountState::SignedIn) {
        return Ok(());
    }
    registry.upsert(origin, pubkey, AccountState::NeedsLogin);
    save_registry(secrets, data_dir, &registry)
}

/// Mark `origin`'s account `pubkey` signed out in the list (its refresh slot
/// is dropped by the caller with the session).
pub(crate) fn mark_signed_out(
    secrets: &dyn Secrets,
    data_dir: &Path,
    origin: &str,
    pubkey: &PublicKey,
) -> Result<(), String> {
    let _lock = registry_lock();
    let mut registry = load_registry(data_dir)?.unwrap_or_default();
    if registry.get(origin, pubkey).is_none() && !registry.contains_pubkey(pubkey) {
        return Ok(());
    }
    registry.upsert(origin, pubkey, AccountState::SignedOut);
    let _ = secrets.delete(&refresh_slot(pubkey, origin));
    save_registry(secrets, data_dir, &registry)
}

// ── Launch reconciliation and migration ────────────────────────────────────

/// What launch reconciliation did (for logs and tests).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Reconciled {
    /// Nothing to do: no identity on this computer yet.
    Empty,
    /// The list was loaded and the active copy matches it.
    Loaded,
    /// The active copy was refilled from the active account's slots.
    Refilled,
    /// The list was rebuilt from the secure-storage index.
    RebuiltFromIndex,
    /// The list was created from the single-account slots of older builds.
    Migrated,
    /// An identity written outside the list (e.g. key import) was adopted.
    Adopted,
}

/// Bring the account list and the active-account copy into agreement at
/// launch, before the identity is resolved.
pub(crate) fn reconcile(secrets: &dyn Secrets, data_dir: &Path) -> Result<Reconciled, String> {
    let _lock = registry_lock();
    // An unreadable or newer list is an error, never silently replaced.
    let (mut registry, mut outcome) = match load_registry(data_dir)? {
        Some(registry) => (registry, Reconciled::Loaded),
        None => match read_index(secrets)? {
            Some(text) => {
                let mut registry = Registry::from_index(&text)?;
                // Owners were recorded when the lost list was created.
                registry.owners_migrated = true;
                write_registry_file(data_dir, &registry)?;
                (registry, Reconciled::RebuiltFromIndex)
            }
            None => match migrate_single_account(secrets, data_dir)? {
                Some(registry) => (registry, Reconciled::Migrated),
                None => return Ok(Reconciled::Empty),
            },
        },
    };

    let legacy = secrets
        .load("identity")?
        .map(Zeroizing::new)
        .and_then(|nsec| Keys::parse(nsec.as_str()).ok());
    let active = registry.active_pubkey();
    match (legacy, active) {
        // Written outside the list (key import): keep every listed account
        // and adopt it as the active one.
        (Some(keys), _) if !registry.contains_pubkey(&keys.public_key()) => {
            let pubkey = keys.public_key();
            if let Some(previous) = active {
                ensure_owners_recorded(data_dir, &mut registry, &previous);
            }
            let bindings = bound_origins(secrets, &pubkey)?;
            if bindings.is_empty() {
                registry.upsert(ANY_ORIGIN, &pubkey, AccountState::KeyOnly);
            }
            for origin in &bindings {
                registry.upsert(origin, &pubkey, AccountState::SignedOut);
            }
            // Listed first, so reset can always find the slots written next.
            save_registry(secrets, data_dir, &registry)?;
            store_verified(secrets, &identity_slot(&pubkey), &nsec_of(&keys)?)?;
            for origin in &bindings {
                let state = copy_refresh_into_slot(secrets, &pubkey, origin)?;
                registry.upsert(origin, &pubkey, state);
            }
            registry.active = Some(pubkey.to_hex());
            save_registry(secrets, data_dir, &registry)?;
            refill_copies(secrets, &registry, &pubkey)?;
            outcome = Reconciled::Adopted;
        }
        (legacy, Some(active)) => {
            let keys = match account_keys(secrets, &active) {
                Ok(keys) => keys,
                // The active account's slot is gone (reset wiped secure
                // storage): the list is stale.
                Err(_) if legacy.is_none() => {
                    let _ = std::fs::remove_file(registry_path(data_dir));
                    return Ok(Reconciled::Empty);
                }
                Err(error) => return Err(error),
            };
            let mut refilled = legacy.as_ref().map(Keys::public_key) != Some(active);
            if refilled {
                write_active_copy(secrets, &keys, None)?;
            }
            if refill_copies(secrets, &registry, &active)? {
                refilled = true;
            }
            if refilled && outcome == Reconciled::Loaded {
                outcome = Reconciled::Refilled;
            }
        }
        (Some(keys), None) => {
            registry.active = Some(keys.public_key().to_hex());
            save_registry(secrets, data_dir, &registry)?;
        }
        (None, None) => {}
    }
    Ok(outcome)
}

/// Make every origin's key-backup binding and refresh copy belong to `active`:
/// another account's binding or token is dropped (its own slot keeps it), and
/// `active`'s own token is copied in where it is signed in. Returns whether
/// anything changed.
fn refill_copies(
    secrets: &dyn Secrets,
    registry: &Registry,
    active: &PublicKey,
) -> Result<bool, String> {
    const PREFIX: &str = "auth.key-backup.";
    let hex = active.to_hex();
    let mut changed = false;
    for key in secrets.blob_keys()? {
        let Some(origin) = key.strip_prefix(PREFIX) else {
            continue;
        };
        if secrets.load(&key)?.as_deref() != Some(hex.as_str()) {
            secrets.delete(&super::refresh_key(origin))?;
            secrets.delete(&key)?;
            changed = true;
        }
    }
    for account in registry
        .accounts
        .iter()
        .filter(|a| a.pubkey == hex && a.origin != ANY_ORIGIN)
    {
        let binding = super::key_recovery::binding_key(&account.origin);
        if secrets.load(&binding)?.as_deref() != Some(hex.as_str()) {
            secrets.store(&binding, &hex)?;
            changed = true;
        }
        let copy = secrets.load(&super::refresh_key(&account.origin))?;
        let own = match account.state {
            AccountState::SignedIn => secrets
                .load(&refresh_slot(active, &account.origin))?
                .map(Zeroizing::new),
            _ => None,
        };
        match own {
            Some(token) if copy.as_deref() != Some(token.as_str()) => {
                secrets.store(&super::refresh_key(&account.origin), &token)?;
                changed = true;
            }
            None if copy.is_some() => {
                secrets.delete(&super::refresh_key(&account.origin))?;
                changed = true;
            }
            _ => {}
        }
    }
    Ok(changed)
}

/// Origins whose key-backup binding names `pubkey`.
fn bound_origins(secrets: &dyn Secrets, pubkey: &PublicKey) -> Result<Vec<String>, String> {
    const PREFIX: &str = "auth.key-backup.";
    let mut origins = BTreeSet::new();
    for key in secrets.blob_keys()? {
        if let Some(origin) = key.strip_prefix(PREFIX) {
            if secrets.load(&key)?.as_deref() == Some(pubkey.to_hex().as_str()) {
                origins.insert(origin.to_owned());
            }
        }
    }
    Ok(origins.into_iter().collect())
}

/// Copy the active refresh token for `origin` into `pubkey`'s slot. Returns
/// the resulting account state.
fn copy_refresh_into_slot(
    secrets: &dyn Secrets,
    pubkey: &PublicKey,
    origin: &str,
) -> Result<AccountState, String> {
    match secrets.load(&super::refresh_key(origin))? {
        Some(token) => {
            let token = Zeroizing::new(token);
            store_verified(secrets, &refresh_slot(pubkey, origin), &token)?;
            Ok(AccountState::SignedIn)
        }
        None => Ok(AccountState::SignedOut),
    }
}

/// Create the list from an older build's single-account slots. An unused
/// first-install key is not an account. Old slots are kept as the active copy.
fn migrate_single_account(
    secrets: &dyn Secrets,
    data_dir: &Path,
) -> Result<Option<Registry>, String> {
    let Some(nsec) = secrets.load("identity")?.map(Zeroizing::new) else {
        return Ok(None);
    };
    let keys = Keys::parse(nsec.as_str()).map_err(|_| "the stored identity is invalid")?;
    let pubkey = keys.public_key();
    let bindings = bound_origins(secrets, &pubkey)?;
    let bootstrap = secrets.load("identity.bootstrap")?;
    if bindings.is_empty()
        && is_unused_bootstrap_key(
            bootstrap.as_deref(),
            &pubkey,
            0,
            local_agent_count(data_dir),
        )
    {
        return Ok(None);
    }
    let mut registry = Registry::default();
    ensure_owners_recorded(data_dir, &mut registry, &pubkey);
    if bindings.is_empty() {
        registry.upsert(ANY_ORIGIN, &pubkey, AccountState::KeyOnly);
    }
    for origin in &bindings {
        registry.upsert(origin, &pubkey, AccountState::SignedOut);
    }
    registry.active = Some(pubkey.to_hex());
    // Listed first, so reset can always find the slots written next.
    save_registry(secrets, data_dir, &registry)?;
    store_verified(secrets, &identity_slot(&pubkey), &nsec)?;
    for origin in &bindings {
        let state = copy_refresh_into_slot(secrets, &pubkey, origin)?;
        registry.upsert(origin, &pubkey, state);
    }
    save_registry(secrets, data_dir, &registry)?;
    Ok(Some(registry))
}

// ── Background refresh of inactive accounts ────────────────────────────────

/// Refresh-token storage of one (inactive) account.
pub(crate) struct AccountRefreshStore {
    pub pubkey: PublicKey,
}

impl super::RefreshStore for AccountRefreshStore {
    fn load(&self, origin: &str) -> Result<Option<String>, String> {
        shared_secrets().load(&refresh_slot(&self.pubkey, origin))
    }
    fn store(&self, origin: &str, token: &str) -> Result<(), String> {
        store_verified(shared_secrets(), &refresh_slot(&self.pubkey, origin), token)
    }
    fn delete(&self, origin: &str) -> Result<(), String> {
        shared_secrets().delete(&refresh_slot(&self.pubkey, origin))
    }
    fn signing_pubkey(&self, _origin: &str) -> Result<Option<PublicKey>, String> {
        Ok(Some(self.pubkey))
    }
}

/// Sessions of signed-in accounts that are not active, each in its own
/// token-auth state so the ordinary restore/refresh loop keeps it alive.
#[derive(Default)]
pub(crate) struct InactiveSessions {
    sessions: Mutex<HashMap<(String, String), Arc<super::TokenAuthState>>>,
}

impl InactiveSessions {
    fn key(origin: &str, pubkey: &PublicKey) -> (String, String) {
        (origin.to_owned(), pubkey.to_hex())
    }

    /// Start keeping `pubkey`'s session on `origin` alive (idempotent). When
    /// the session ends for good (revoked elsewhere, expired), the account is
    /// marked `needs_login` in the list under `data_dir`.
    pub(crate) fn start(
        &self,
        client: &reqwest::Client,
        data_dir: Option<PathBuf>,
        origin: &str,
        pubkey: PublicKey,
    ) {
        let auth = {
            let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
            let key = Self::key(origin, &pubkey);
            if sessions.contains_key(&key) {
                return;
            }
            let auth = Arc::new(super::TokenAuthState::default());
            sessions.insert(key, Arc::clone(&auth));
            auth
        };
        let client = client.clone();
        let origin = origin.to_owned();
        tauri::async_runtime::spawn(async move {
            let store = Arc::new(AccountRefreshStore { pubkey });
            if let Some((generation, attempt)) =
                super::refresh::restore_session(&auth, &client, &origin, store.as_ref()).await
            {
                if !matches!(attempt, Err(super::refresh::RefreshFailure::Terminal(_))) {
                    super::refresh::run_refresh_loop(
                        Arc::clone(&auth),
                        client,
                        origin.clone(),
                        generation,
                        store,
                        |_| {},
                    )
                    .await;
                }
            }
            // Stopped by a hand-over leaves `SignedOut`; only a real end of
            // the session is recorded.
            if let Some((_, super::OriginAuth::NeedsLogin(code))) = auth.get(&origin) {
                eprintln!(
                    "buzz-desktop: accounts: background session of {} on {origin} ended ({code})",
                    &pubkey.to_hex()[..8]
                );
                if let Some(dir) = data_dir {
                    if let Err(error) = mark_needs_login(shared_secrets(), &dir, &origin, &pubkey) {
                        eprintln!("buzz-desktop: accounts: could not record it: {error}");
                    }
                }
            }
        });
    }

    /// Stop `pubkey`'s background session on `origin`, waiting out a refresh
    /// in flight so its rotated token is in the slot before the caller reads
    /// it. The generation fence ends the loop. Returns whether one was
    /// running.
    pub(crate) async fn stop(&self, origin: &str, pubkey: &PublicKey) -> bool {
        let auth = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&Self::key(origin, pubkey));
        let Some(auth) = auth else {
            return false;
        };
        let _guard = auth.refresh_lock(origin).lock_owned().await;
        auth.set(origin, super::OriginAuth::SignedOut);
        true
    }
}

// ── App wiring ─────────────────────────────────────────────────────────────

/// Tauri event emitted after the active account changed. Payload: its pubkey.
pub(crate) const ACTIVE_ACCOUNT_CHANGED_EVENT: &str = "active-account-changed";

/// Launch: reconcile the list with secure storage before the identity is
/// resolved. Failures are logged; the single-account slots still work.
pub(crate) fn reconcile_on_launch(data_dir: &Path) {
    match reconcile(shared_secrets(), data_dir) {
        Ok(outcome) => eprintln!("buzz-desktop: accounts: {outcome:?}"),
        Err(error) => eprintln!("buzz-desktop: accounts: reconcile failed: {error}"),
    }
}

/// Launch: keep every signed-in account that is not active alive.
pub(crate) fn start_background_sessions<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    let Ok(Some(registry)) = load_registry(&data_dir) else {
        return;
    };
    let state = app.state::<crate::app_state::AppState>();
    // Never refresh the active account twice (one refresh token, one user):
    // guard on the key actually in use too, in case reconcile failed.
    let in_use = state.keys.lock().ok().map(|keys| keys.public_key());
    for (origin, pubkey) in registry.inactive_signed_in() {
        if Some(pubkey) == in_use {
            continue;
        }
        state.token_auth.inactive.start(
            &state.http_client,
            Some(data_dir.clone()),
            &origin,
            pubkey,
        );
    }
}

/// Launch: record (once) the owner of older unattested agents as the account
/// active now, for lists created before owners were recorded.
pub(crate) fn record_owners_on_launch(data_dir: &Path) {
    let _lock = registry_lock();
    let Ok(Some(mut registry)) = load_registry(data_dir) else {
        return;
    };
    let Some(active) = registry.active_pubkey() else {
        return;
    };
    if registry.owners_migrated {
        return;
    }
    ensure_owners_recorded(data_dir, &mut registry, &active);
    if let Err(error) = save_registry(shared_secrets(), data_dir, &registry) {
        eprintln!("buzz-desktop: accounts: owner migration not saved: {error}");
    }
}

/// Every origin with account or session state.
fn known_origins(registry: &Registry, auth: &super::TokenAuthState) -> BTreeSet<String> {
    let mut origins: BTreeSet<String> = registry
        .accounts
        .iter()
        .filter(|a| a.origin != ANY_ORIGIN)
        .map(|a| a.origin.clone())
        .collect();
    origins.extend(auth.known_origins());
    origins
}

/// Hand every origin over from `previous` to `current` (now the active key):
/// under each origin's refresh lock, the active session is fenced, the active
/// copy (binding + refresh token) becomes `current`'s, `current`'s background
/// session stops and `previous`'s starts where it is signed in. One refresh
/// token is therefore only ever refreshed by one loop. `skip` is the origin a
/// sign-in just activated (already handed over by the sign-in itself); `held`
/// is an origin whose refresh lock the caller already holds.
async fn hand_over_origins<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    previous: PublicKey,
    current: PublicKey,
    skip: Option<&str>,
    held: Option<&str>,
) -> Result<(), String> {
    hand_over_origins_with(app, shared_secrets(), previous, current, skip, held).await
}

async fn hand_over_origins_with<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    secrets: &(dyn Secrets + Sync),
    previous: PublicKey,
    current: PublicKey,
    skip: Option<&str>,
    held: Option<&str>,
) -> Result<(), String> {
    use tauri::Manager;
    let state = app.state::<crate::app_state::AppState>();
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "account storage is unavailable")?;
    let registry = {
        let _lock = registry_lock();
        load_registry(&data_dir)?.unwrap_or_default()
    };
    // Every origin is handed over even when one fails, so no origin keeps a
    // session of the wrong account; the failures are reported together.
    let mut failures = Vec::new();
    for origin in known_origins(&registry, &state.token_auth) {
        state.token_auth.inactive.stop(&origin, &current).await;
        if Some(origin.as_str()) != skip {
            let _guard = if Some(origin.as_str()) == held {
                None
            } else {
                Some(state.token_auth.refresh_lock(&origin).lock_owned().await)
            };
            state.token_auth.clear_pending_refresh(&origin);
            state.token_auth.set(&origin, super::OriginAuth::SignedOut);
            let here = registry.get(&origin, &current).map(|a| a.state);
            match hand_over_copy(secrets, &origin, &current, here) {
                Ok(true) => state.token_auth.set_key_backup(&origin, current),
                Ok(false) => state.token_auth.clear_key_backup(&origin),
                Err(error) => {
                    state.token_auth.clear_key_backup(&origin);
                    failures.push(format!("{origin}: {error}"));
                }
            }
        }
        if registry.get(&origin, &previous).map(|a| a.state) == Some(AccountState::SignedIn) {
            state.token_auth.inactive.start(
                &state.http_client,
                Some(data_dir.clone()),
                &origin,
                previous,
            );
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// Point `origin`'s active copy at `current`. Order matters: the old token
/// copy goes first, then the binding, and `current`'s own token is copied in
/// last, so a failure part-way leaves no token that a restore could refresh
/// under the wrong binding. Returns whether `current` is bound there.
fn hand_over_copy(
    secrets: &dyn Secrets,
    origin: &str,
    current: &PublicKey,
    here: Option<AccountState>,
) -> Result<bool, String> {
    let binding = super::key_recovery::binding_key(origin);
    secrets.delete(&super::refresh_key(origin))?;
    match here {
        Some(AccountState::SignedIn | AccountState::SignedOut | AccountState::NeedsLogin) => {
            secrets.store(&binding, &current.to_hex())?;
            if here == Some(AccountState::SignedIn) {
                if let Some(token) = secrets.load(&refresh_slot(current, origin))? {
                    secrets.store(&super::refresh_key(origin), &Zeroizing::new(token))?;
                }
            }
            Ok(true)
        }
        _ => {
            secrets.delete(&binding)?;
            Ok(false)
        }
    }
}

/// After a sign-in on `origin` made `current` the active account in place of
/// `previous`: hand the other origins over and notify the UI.
pub(crate) async fn after_active_changed<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    origin: &str,
    previous: PublicKey,
    current: PublicKey,
) {
    use tauri::Emitter;
    if let Err(error) = hand_over_origins(app, previous, current, Some(origin), None).await {
        eprintln!("buzz-desktop: accounts: hand-over after sign-in incomplete: {error}");
    }
    let _ = app.emit(ACTIVE_ACCOUNT_CHANGED_EVENT, current.to_hex());
}

/// One account on this computer, for the UI. Never carries secrets.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub pubkey: String,
    pub origin: String,
    /// `signed_in | signed_out | key_only`
    pub state: AccountState,
    pub active: bool,
}

/// Accounts on this computer for the current community (and key-only ones).
#[tauri::command]
pub fn list_accounts(app: tauri::AppHandle) -> Result<Vec<AccountSummary>, String> {
    use tauri::Manager;
    let state = app.state::<crate::app_state::AppState>();
    let origin = state.current_auth_origin();
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "account storage is unavailable")?;
    let registry = load_registry(&data_dir)?.unwrap_or_default();
    Ok(registry
        .accounts
        .iter()
        .filter(|a| a.origin == origin || a.origin == ANY_ORIGIN)
        .map(|a| AccountSummary {
            pubkey: a.pubkey.clone(),
            origin: a.origin.clone(),
            state: a.state,
            active: registry.active.as_ref() == Some(&a.pubkey),
        })
        .collect())
}

/// Make `pubkey` the active account. The previous account stays signed in
/// (its session keeps refreshing in the background) and its agents keep
/// running; the UI reloads on [`ACTIVE_ACCOUNT_CHANGED_EVENT`]. Once the key
/// has changed the switch is complete: a server whose session could not be
/// handed over is returned as a warning (`Ok(Some(..))`), not an error.
#[tauri::command]
pub async fn switch_account(
    app: tauri::AppHandle,
    pubkey: String,
) -> Result<Option<String>, String> {
    use tauri::{Emitter, Manager};
    let state = app.state::<crate::app_state::AppState>();
    let target = PublicKey::from_hex(&pubkey).map_err(|_| "invalid account")?;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "account storage is unavailable")?;
    let origin = state.current_auth_origin();
    let secrets = shared_secrets();
    // A sign-in completing on this origin holds its refresh lock: wait for it,
    // then read the list fresh.
    let refresh_guard = state.token_auth.refresh_lock(&origin).lock_owned().await;
    let (previous, keys) = {
        let _identity = state
            .identity_mutation
            .lock()
            .map_err(|_| "identity storage is busy")?;
        let _lock = registry_lock();
        let mut registry = load_registry(&data_dir)?.ok_or("No accounts on this computer.")?;
        if !registry.contains_pubkey(&target) {
            return Err("This account is not on this computer.".into());
        }
        let outgoing = state
            .keys
            .lock()
            .map_err(|_| "identity storage is busy")?
            .clone();
        if outgoing.public_key() == target {
            return Ok(None);
        }
        let keys = account_keys(secrets, &target)?;
        preserve_outgoing(secrets, &data_dir, &mut registry, &outgoing)?;
        registry.active = Some(target.to_hex());
        save_registry(secrets, &data_dir, &registry)?;
        write_active_copy(secrets, &keys, None)?;
        *state.keys.lock().map_err(|_| "identity storage is busy")? = keys.clone();
        (outgoing.public_key(), keys)
    };
    let _ = keys;
    // This origin's lock is still held: no session here can rotate until it
    // is handed over.
    let handed = hand_over_origins(&app, previous, target, None, Some(&origin)).await;
    drop(refresh_guard);
    // The backend already signs as `target`: the UI must follow even when a
    // server's session could not be handed over.
    super::restore::restore_if_stored(&app, &origin).await;
    let _ = app.emit(ACTIVE_ACCOUNT_CHANGED_EVENT, target.to_hex());
    super::restore::emit_changed(&app, &origin);
    Ok(handed.err().map(|error| {
        eprintln!("buzz-desktop: accounts: switch hand-over incomplete: {error}");
        error
    }))
}

/// The key of an account on this computer for read-only background work as
/// that account (its unread observer). With `relay_url`, the account must be
/// signed in there (or key-only) and not signed out on this device; without
/// it, it must be usable somewhere.
fn usable_account_keys<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    account: &str,
    relay_url: Option<&str>,
) -> Result<Keys, String> {
    use tauri::Manager;
    let pubkey = PublicKey::from_hex(account).map_err(|_| "invalid account")?;
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "account storage is unavailable")?;
    let registry = {
        let _lock = registry_lock();
        load_registry(&data_dir)?.unwrap_or_default()
    };
    let hex = pubkey.to_hex();
    let key_only = registry
        .accounts
        .iter()
        .any(|a| a.pubkey == hex && a.state == AccountState::KeyOnly);
    let usable = match relay_url {
        Some(url) => {
            let origin = super::origin_for(url);
            let state = app.state::<crate::app_state::AppState>();
            !state
                .token_auth
                .signed_out
                .is_signed_out(&origin, Some(pubkey))
                && (key_only
                    || registry.get(&origin, &pubkey).map(|a| a.state)
                        == Some(AccountState::SignedIn))
        }
        None => {
            let state = app.state::<crate::app_state::AppState>();
            key_only
                || registry.accounts.iter().any(|a| {
                    a.pubkey == hex
                        && a.state == AccountState::SignedIn
                        && !state
                            .token_auth
                            .signed_out
                            .is_signed_out(&a.origin, Some(pubkey))
                })
        }
    };
    if !usable {
        return Err("This account is not signed in on this computer.".into());
    }
    account_keys(shared_secrets(), &pubkey)
}

/// NIP-42 AUTH event signed by `account` (not necessarily the active one).
#[tauri::command]
pub async fn create_auth_event_as(
    app: tauri::AppHandle,
    account: String,
    challenge: String,
    relay_url: String,
) -> Result<String, String> {
    use nostr::{EventBuilder, JsonUtil, Kind, Tag};
    let keys = usable_account_keys(&app, &account, Some(&relay_url))?;
    tauri::async_runtime::spawn_blocking(move || {
        let tags = vec![
            Tag::parse(vec!["relay", &relay_url])
                .map_err(|error| format!("relay tag failed: {error}"))?,
            Tag::parse(vec!["challenge", &challenge])
                .map_err(|error| format!("challenge tag failed: {error}"))?,
        ];
        EventBuilder::new(Kind::Custom(22242), "")
            .tags(tags)
            .sign_with_keys(&keys)
            .map(|event| event.as_json())
            .map_err(|error| format!("sign failed: {error}"))
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

/// Decrypt a NIP-44 payload `account` encrypted to itself.
#[tauri::command]
pub async fn nip44_decrypt_as(
    app: tauri::AppHandle,
    account: String,
    ciphertext: String,
) -> Result<String, String> {
    let keys = usable_account_keys(&app, &account, None)?;
    tauri::async_runtime::spawn_blocking(move || {
        nostr::nips::nip44::decrypt(keys.secret_key(), &keys.public_key(), &ciphertext)
            .map_err(|e| format!("nip44 decrypt failed: {e}"))
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

#[cfg(test)]
mod tests;
