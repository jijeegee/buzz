//! Kuzz agents hosted by this device (device-bound keys plan §4, PR C1).
//!
//! An agent is registered to the device that creates or starts it
//! (`/auth/agents/challenge` + `/auth/agents/register`); registering also
//! gives the relay its encrypted copy, so the key can leave this computer at
//! sign-out and come back with `GET /auth/agents/keys` at the next sign-in.
//! An agent the relay says runs on another of the owner's devices is listed
//! but not started here.
//!
//! Best effort throughout: without a device key, a relay without these
//! endpoints, or a network failure, agents behave as before.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use nostr::{Keys, PublicKey};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use zeroize::Zeroizing;

use super::api;
use crate::app_state::AppState;
use crate::managed_agents::ManagedAgentRecord;

const HOSTING_FILE: &str = "agent-hosting.json";

/// Shown when an agent hosted elsewhere is not started here.
pub(crate) const OTHER_DEVICE_REFUSAL: &str =
    "This agent runs on another of your devices, so it was not started here.";

/// Shown when an agent of this device is started while its key is away
/// (it left at a sign-out and has not come back yet).
pub(crate) const KEY_AWAY_REFUSAL: &str =
    "This agent left this computer when you signed out. Sign in again here to bring it back.";

/// The relay lists key copies only within 5 minutes of a sign-in; retries
/// after a sign-in stop a little before that.
const RESTORE_WINDOW: Duration = Duration::from_secs(270);
/// Longest a sign-in waits for its first restore before going on.
const RESTORE_FIRST_LIMIT: Duration = Duration::from_secs(8);
/// Longest one later restore attempt may take.
const RESTORE_TRY_LIMIT: Duration = Duration::from_secs(20);
/// Longest a restore waits for the device key to open.
const RESTORE_DEVICE_WAIT: Duration = Duration::from_secs(3);
/// Restores outside a sign-in (app start, agent start) at most this often.
const PENDING_RETRY_GAP: Duration = Duration::from_secs(60);
/// Longest a sign-out spends confirming key copies before it goes on.
const CONFIRM_LIMIT: Duration = Duration::from_secs(10);
/// Longest one confirmation may take.
const CONFIRM_ONE_LIMIT: Duration = Duration::from_secs(5);

/// Where the relay says an agent runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Hosting {
    /// Registered to this device; the relay holds its key copy.
    Here,
    /// Registered to another of the owner's devices.
    OtherDevice,
}

/// Agents with a registration attempt in flight or done this run.
static ATTEMPTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Serializes reads and writes of [`HOSTING_FILE`].
static FILE_LOCK: Mutex<()> = Mutex::new(());
/// `"{origin} {owner}"` with a restore retry loop running.
static RESTORING: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// When a restore outside a sign-in was last tried, per `"{origin} {owner}"`.
static PENDING_TRIED: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// What one registration attempt learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Registration {
    /// The relay holds the agent's copy for this device.
    Here,
    /// The agent is registered to another of the owner's devices.
    OtherDevice,
    /// Not registrable (no attestation, another owner, refused).
    Refused,
    /// No answer (no device key, no session, network, server error).
    NoAnswer,
}

fn entry_key(origin: &str, pubkey: &str) -> String {
    format!("{origin} {}", pubkey.to_ascii_lowercase())
}

fn hosting_path(data_dir: &Path) -> PathBuf {
    data_dir.join(HOSTING_FILE)
}

fn read_map(data_dir: &Path) -> BTreeMap<String, Hosting> {
    std::fs::read(hosting_path(data_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// What the relay last said about `pubkey` on `origin`. Pure apart from IO.
pub(crate) fn hosting_in(data_dir: &Path, origin: &str, pubkey: &str) -> Option<Hosting> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    read_map(data_dir).get(&entry_key(origin, pubkey)).copied()
}

/// Record (or with `None` forget) what the relay said about `pubkey`.
pub(crate) fn set_hosting_in(
    data_dir: &Path,
    origin: &str,
    pubkey: &str,
    hosting: Option<Hosting>,
) -> Result<(), String> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut map = read_map(data_dir);
    let key = entry_key(origin, pubkey);
    let changed = match hosting {
        Some(hosting) => map.insert(key, hosting) != Some(hosting),
        None => map.remove(&key).is_some(),
    };
    if !changed {
        return Ok(());
    }
    let json = serde_json::to_vec_pretty(&map).map_err(|e| e.to_string())?;
    crate::managed_agents::atomic_write_json(&hosting_path(data_dir), &json)
}

fn data_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
}

/// Whether `record` runs on another device per the relay (for the agent list).
pub(crate) fn hosted_elsewhere<R: tauri::Runtime>(
    app: &AppHandle<R>,
    relay_url: &str,
    pubkey: &str,
) -> bool {
    data_dir(app).is_some_and(|dir| {
        hosting_in(&dir, &super::origin_for(relay_url), pubkey) == Some(Hosting::OtherDevice)
    })
}

/// The start gate for Kuzz agents: refuse one registered to another device;
/// otherwise (and also then, to learn whether it moved) register it to this
/// device in the background. Server bots are not Kuzz agent keys.
pub(crate) fn start_gate<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    relay_url: &str,
    owner_hex: &str,
) -> Result<(), String> {
    if record.bot_origin.is_some() {
        return Ok(());
    }
    note_wrong_host_exit(app, record, relay_url);
    let origin = super::origin_for(relay_url);
    let elsewhere = data_dir(app)
        .is_some_and(|dir| hosting_in(&dir, &origin, &record.pubkey) == Some(Hosting::OtherDevice));
    spawn_register(app, record, &origin, owner_hex, elsewhere);
    if elsewhere {
        eprintln!(
            "kuzz-desktop: agents: {} runs on another device; not started",
            record.pubkey
        );
        return Err(OTHER_DEVICE_REFUSAL.to_owned());
    }
    Ok(())
}

/// `kuzz-acp`'s exit code when the relay says the agent is bound to another
/// device (`403 wrong_host` on `/agents/{pubkey}/status`). Never restarted.
pub(crate) const WRONG_HOST_EXIT: i32 = 77;

/// What `kuzz-acp` logs in that case (older builds exit 0 after it).
const WRONG_HOST_LOG: &str = "this agent is bound to another device";

/// What the desktop appends to the log once it has taken that note.
const WRONG_HOST_NOTED: &str = "kuzz-desktop: noted: this agent runs on another device";

/// Whether the last run (exit code `exit`, log tail `log`) stopped on
/// `wrong_host` and the desktop has not noted it yet. Pure.
fn unnoted_wrong_host(exit: Option<i32>, log: &str) -> bool {
    let last_run = log.rsplit_once("=== starting ").map_or(log, |(_, run)| run);
    (exit == Some(WRONG_HOST_EXIT) || last_run.contains(WRONG_HOST_LOG))
        && !last_run.contains(WRONG_HOST_NOTED)
}

/// After `kuzz-acp` stayed down because the relay bound the agent to another
/// device (exit [`WRONG_HOST_EXIT`], or exit 0 with the log line from older
/// builds), record it as running there: the list shows "Runs on another
/// device" and starts are refused until the relay says it moved back. The
/// note is taken once per run, so a later move back is not undone.
pub(crate) fn note_wrong_host_exit<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    relay_url: &str,
) {
    if record.bot_origin.is_some() || !matches!(record.last_exit_code, Some(0 | WRONG_HOST_EXIT)) {
        return;
    }
    let Some(dir) = data_dir(app) else {
        return;
    };
    let Ok(key) =
        crate::managed_agents::ManagedAgentRuntimeKey::new(record.pubkey.clone(), relay_url)
    else {
        return;
    };
    let Ok(path) = crate::managed_agents::managed_agent_runtime_log_path(app, &key) else {
        return;
    };
    let Ok(tail) = crate::managed_agents::read_log_tail(&path, 200) else {
        return;
    };
    if !unnoted_wrong_host(record.last_exit_code, &tail) {
        return;
    }
    let origin = super::origin_for(relay_url);
    if set_hosting_in(&dir, &origin, &record.pubkey, Some(Hosting::OtherDevice)).is_ok() {
        eprintln!(
            "kuzz-desktop: agents: {} is bound to another device; not restarting it",
            record.pubkey
        );
        let _ = crate::managed_agents::append_log_marker(&path, WRONG_HOST_NOTED);
    }
}

/// Register `record` to this device in the background, once per run (again
/// when `recheck`, for an agent last seen on another device).
pub(crate) fn spawn_register<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    origin: &str,
    owner_hex: &str,
    recheck: bool,
) {
    if crate::device_key::current().is_none() || record.private_key_nsec.is_empty() {
        return;
    }
    let key = entry_key(origin, &record.pubkey);
    {
        let mut attempted = ATTEMPTED.lock().unwrap_or_else(PoisonError::into_inner);
        let attempted = attempted.get_or_insert_with(HashSet::new);
        if !attempted.insert(key.clone()) && !recheck {
            return;
        }
    }
    let app = app.clone();
    let record = record.clone();
    let origin = origin.to_owned();
    let owner_hex = owner_hex.to_owned();
    tauri::async_runtime::spawn(async move {
        if register(&app, &record, &origin, &owner_hex).await == Registration::NoAnswer {
            // Not answered: try again on the next start.
            ATTEMPTED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert_with(HashSet::new)
                .remove(&key);
        }
    });
}

/// Register one agent to this device (which also stores the relay's copy
/// of its key for this device).
async fn register<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    origin: &str,
    owner_hex: &str,
) -> Registration {
    let Some(device) = crate::device_key::current() else {
        return Registration::NoAnswer;
    };
    // Only an agent attested to this owner (NIP-OA) can be registered.
    let Some(auth_tag) = record.auth_tag.clone() else {
        return Registration::Refused;
    };
    if crate::managed_agents::attested_owner_hex(record).as_deref() != Some(owner_hex) {
        return Registration::Refused;
    }
    let Ok(keys) = Keys::parse(&record.private_key_nsec) else {
        return Registration::Refused;
    };
    let Ok(owner) = PublicKey::from_hex(owner_hex) else {
        return Registration::Refused;
    };
    let Some(access) = owner_access(app, origin, &owner).await else {
        return Registration::NoAnswer;
    };
    let state = app.state::<AppState>();
    let client = &state.http_client;
    let outcome = async {
        let challenge = api::agent_challenge(client, origin, &access).await?;
        let body = register_body(
            &keys,
            &challenge,
            owner_hex,
            &device.pubkey().to_hex(),
            &auth_tag,
        );
        api::register_agent(client, origin, &access, &body).await
    }
    .await;
    let hosting = match &outcome {
        Ok(_) => Some(Hosting::Here),
        Err(error) if error.code.as_deref() == Some("agent_on_other_host") => {
            Some(Hosting::OtherDevice)
        }
        Err(error) => {
            // Never the body: it carries the secret key.
            eprintln!(
                "kuzz-desktop: agents: {} not registered to this device: {}",
                record.pubkey,
                error.code.as_deref().unwrap_or("no answer")
            );
            None
        }
    };
    let Some(hosting) = hosting else {
        let refused = matches!(&outcome, Err(error) if error.status.is_some_and(|s| s < 500));
        return if refused {
            Registration::Refused
        } else {
            Registration::NoAnswer
        };
    };
    if let Some(dir) = data_dir(app) {
        if let Err(error) = set_hosting_in(&dir, origin, &record.pubkey, Some(hosting)) {
            eprintln!("kuzz-desktop: agents: hosting not recorded: {error}");
        }
    }
    match hosting {
        Hosting::Here => Registration::Here,
        Hosting::OtherDevice => Registration::OtherDevice,
    }
}

/// The `/auth/agents/register` body: the agent signs
/// `kuzz-agent-register/v1\n<challenge>\n<owner>\n<device>` (SHA-256,
/// BIP-340) and hands over its secret key (hex) and NIP-OA tag. Pure.
pub(crate) fn register_body(
    keys: &Keys,
    challenge: &str,
    owner_hex: &str,
    device_hex: &str,
    auth_tag: &str,
) -> serde_json::Value {
    use nostr::hashes::{sha256::Hash as Sha256Hash, Hash as _};
    let message =
        kuzz_auth_pkg::key_backup::agent_register_message(challenge, owner_hex, device_hex);
    let digest = Sha256Hash::hash(message.as_bytes());
    let signature = keys.sign_schnorr(&nostr::secp256k1::Message::from_digest(
        digest.to_byte_array(),
    ));
    let secret = Zeroizing::new(keys.secret_key().to_secret_hex());
    serde_json::json!({
        "agent_pubkey": keys.public_key().to_hex(),
        "secret_key": secret.as_str(),
        "challenge": challenge,
        "signature": signature.to_string(),
        "auth_tag": auth_tag,
    })
}

/// A current access token of `owner` on `origin`: the active session
/// (refreshed when near expiry), else the owner's background session.
async fn owner_access<R: tauri::Runtime>(
    app: &AppHandle<R>,
    origin: &str,
    owner: &PublicKey,
) -> Option<Zeroizing<String>> {
    let state = app.state::<AppState>();
    if state.token_auth.signing_pubkey(origin) == Some(*owner) {
        return super::commands::fresh_session(&state, origin)
            .await
            .ok()
            .map(|session| session.access);
    }
    let now = chrono::Utc::now().timestamp();
    state
        .token_auth
        .inactive
        .on_origin(origin)
        .into_iter()
        .find(|(account, _)| account == owner)
        .and_then(|(_, auth)| match auth.get(origin) {
            Some((_, super::OriginAuth::Active(session))) if session.access_expires_at > now => {
                Some(session.access)
            }
            _ => None,
        })
}

/// What one restore attempt ended with.
enum Restore {
    /// Answered (the pubkeys filled, maybe none): no retry.
    Done(Vec<String>),
    /// No answer yet: worth another try while the sign-in is fresh.
    Retry,
}

/// Whether a failed key listing is worth another try (no answer, a server
/// error, rate limiting, or a device proof the relay could not accept yet).
/// Pure.
fn restore_retryable(error: &api::ApiError) -> bool {
    match error.status {
        None => true,
        Some(status) if status >= 500 || status == 429 => true,
        Some(_) => matches!(
            error.code.as_deref(),
            Some("device_proof_required" | "device_proof_stale")
        ),
    }
}

/// One attempt to put back the keys of `owner`'s agents of this device
/// that left at a sign-out (only agents still listed here with no key).
async fn restore_once<R: tauri::Runtime>(
    app: &AppHandle<R>,
    origin: &str,
    owner: &PublicKey,
) -> Restore {
    if crate::device_key::ensure_within(RESTORE_DEVICE_WAIT)
        .await
        .is_none()
    {
        return Restore::Retry;
    }
    let Some(access) = owner_access(app, origin, owner).await else {
        return Restore::Retry;
    };
    let state = app.state::<AppState>();
    let copies = match api::agent_keys(&state.http_client, origin, &access).await {
        Ok(copies) => copies,
        // Not offered by this server.
        Err(error) if error.status == Some(404) => return Restore::Done(Vec::new()),
        Err(error) => {
            eprintln!(
                "kuzz-desktop: agents: agents not brought back yet: {}",
                error.code.as_deref().unwrap_or("no answer")
            );
            return if restore_retryable(&error) {
                Restore::Retry
            } else {
                Restore::Done(Vec::new())
            };
        }
    };
    Restore::Done(store_copies(app, origin, &copies))
}

/// Save the matching copies into keyless local agents. Returns the filled.
fn store_copies<R: tauri::Runtime>(
    app: &AppHandle<R>,
    origin: &str,
    copies: &[api::AgentKeyCopy],
) -> Vec<String> {
    if copies.is_empty() {
        return Vec::new();
    }
    let state = app.state::<AppState>();
    let restored = {
        let Ok(_guard) = state.managed_agents_store_lock.lock() else {
            return Vec::new();
        };
        let mut records = match crate::managed_agents::load_managed_agents(app) {
            Ok(records) => records,
            Err(error) => {
                eprintln!("kuzz-desktop: agents: agents not brought back: {error}");
                return Vec::new();
            }
        };
        let restored = fill_missing_keys(&mut records, copies);
        if restored.is_empty() {
            return restored;
        }
        if let Err(error) = crate::managed_agents::save_managed_agents(app, &records) {
            eprintln!("kuzz-desktop: agents: brought-back agents not saved: {error}");
            return Vec::new();
        }
        restored
    };
    if let Some(dir) = data_dir(app) {
        for pubkey in &restored {
            let _ = set_hosting_in(&dir, origin, pubkey, Some(Hosting::Here));
        }
    }
    eprintln!(
        "kuzz-desktop: agents: {} agent key(s) restored to this device",
        restored.len()
    );
    restored
}

/// Agents restored after the sign-in had already gone on (to start them).
pub(crate) type OnLateRestore = Box<dyn FnOnce(Vec<String>) + Send + 'static>;

/// After a sign-in of `owner`: bring back this device's agent keys that left
/// at the last sign-out, before the owner's agents start. Waits at most
/// [`RESTORE_FIRST_LIMIT`]; without an answer it retries in the background
/// (backing off) while the sign-in is fresh enough for the relay, and hands
/// late restores to `on_late`.
pub(crate) async fn restore_after_sign_in(
    app: &AppHandle,
    origin: &str,
    owner: PublicKey,
    on_late: OnLateRestore,
) {
    let deadline = Instant::now() + RESTORE_WINDOW;
    if let Ok(Restore::Done(_)) =
        tokio::time::timeout(RESTORE_FIRST_LIMIT, restore_once(app, origin, &owner)).await
    {
        return;
    }
    let key = format!("{origin} {}", owner.to_hex());
    {
        let mut restoring = RESTORING.lock().unwrap_or_else(PoisonError::into_inner);
        if !restoring
            .get_or_insert_with(HashSet::new)
            .insert(key.clone())
        {
            return;
        }
    }
    let app = app.clone();
    let origin = origin.to_owned();
    tauri::async_runtime::spawn(async move {
        let mut on_late = Some(on_late);
        let mut delay = Duration::from_secs(5);
        while Instant::now() + delay < deadline {
            tokio::time::sleep(delay).await;
            if let Ok(Restore::Done(restored)) =
                tokio::time::timeout(RESTORE_TRY_LIMIT, restore_once(&app, &origin, &owner)).await
            {
                if let (false, Some(on_late)) = (restored.is_empty(), on_late.take()) {
                    on_late(restored);
                }
                break;
            }
            delay = (delay * 2).min(Duration::from_secs(60));
        }
        RESTORING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashSet::new)
            .remove(&key);
    });
}

/// Outside a sign-in (app start, an agent start, an account coming back):
/// try once to bring back keys of this device's agents that are listed here
/// with no key. The relay answers only within 5 minutes of a sign-in, so
/// this mostly helps right after one. At most every [`PENDING_RETRY_GAP`].
pub(crate) fn spawn_restore_pending<R: tauri::Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(dir) = data_dir(&app) else {
            return;
        };
        let Ok(records) = crate::managed_agents::load_managed_agents(&app) else {
            return;
        };
        let hosted_here: Vec<(String, String)> = {
            let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
            read_map(&dir)
                .into_iter()
                .filter(|(_, hosting)| *hosting == Hosting::Here)
                .filter_map(|(key, _)| {
                    key.split_once(' ')
                        .map(|(origin, pubkey)| (origin.to_owned(), pubkey.to_owned()))
                })
                .collect()
        };
        let mut wanted: Vec<(String, PublicKey)> = Vec::new();
        for (origin, pubkey) in hosted_here {
            let Some(record) = records.iter().find(|record| {
                record.bot_origin.is_none()
                    && record.private_key_nsec.is_empty()
                    && record.pubkey.eq_ignore_ascii_case(&pubkey)
            }) else {
                continue;
            };
            let Some(owner) = crate::managed_agents::agent_owner_hex(&app, record)
                .and_then(|hex| PublicKey::from_hex(&hex).ok())
            else {
                continue;
            };
            if !wanted.contains(&(origin.clone(), owner)) {
                wanted.push((origin, owner));
            }
        }
        for (origin, owner) in wanted {
            let key = format!("{origin} {}", owner.to_hex());
            if RESTORING
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert_with(HashSet::new)
                .contains(&key)
            {
                continue;
            }
            {
                let mut tried = PENDING_TRIED.lock().unwrap_or_else(PoisonError::into_inner);
                let tried = tried.get_or_insert_with(HashMap::new);
                if tried
                    .get(&key)
                    .is_some_and(|at| at.elapsed() < PENDING_RETRY_GAP)
                {
                    continue;
                }
                tried.insert(key, Instant::now());
            }
            let _ =
                tokio::time::timeout(RESTORE_TRY_LIMIT, restore_once(&app, &origin, &owner)).await;
        }
    });
}

/// Refuse to start an agent of this device whose key is away (it left at a
/// sign-out), with a plain message, and try to bring it back.
pub(crate) fn key_away_gate<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    relay_url: &str,
) -> Result<(), String> {
    if record.bot_origin.is_some() || !record.private_key_nsec.is_empty() {
        return Ok(());
    }
    let origin = super::origin_for(relay_url);
    let here = data_dir(app)
        .is_some_and(|dir| hosting_in(&dir, &origin, &record.pubkey) == Some(Hosting::Here));
    if !here {
        return Ok(());
    }
    spawn_restore_pending(app);
    Err(KEY_AWAY_REFUSAL.to_owned())
}

/// Give listed agents without a key their copy (when it matches the agent's
/// public key). Returns the pubkeys filled. Pure.
pub(crate) fn fill_missing_keys(
    records: &mut [ManagedAgentRecord],
    copies: &[api::AgentKeyCopy],
) -> Vec<String> {
    let mut filled = Vec::new();
    for copy in copies {
        let Some(record) = records.iter_mut().find(|record| {
            record.bot_origin.is_none()
                && record.private_key_nsec.is_empty()
                && record.pubkey.eq_ignore_ascii_case(&copy.agent_pubkey)
        }) else {
            continue;
        };
        let Ok(keys) = Keys::parse(&copy.secret_key) else {
            continue;
        };
        if keys.public_key().to_hex() != record.pubkey.to_ascii_lowercase() {
            continue;
        }
        let nsec = nostr::ToBech32::to_bech32(keys.secret_key());
        let Ok(nsec) = nsec.map_err(|error| error.to_string()) else {
            continue;
        };
        record.private_key_nsec = nsec;
        filled.push(record.pubkey.clone());
    }
    filled
}

/// Before a sign-out ends the session: ask the relay to (re)confirm, by
/// registering again, that it holds a copy of each of `owners`' agents of
/// this device for this device's key. Returns the confirmed pubkeys
/// (lowercase). Bounded by [`CONFIRM_LIMIT`]; anything unconfirmed keeps
/// its key.
pub(crate) async fn confirm_copies(
    app: &AppHandle,
    origin: &str,
    owners: &[PublicKey],
) -> HashSet<String> {
    let mut confirmed = HashSet::new();
    if crate::device_key::current().is_none() {
        return confirmed;
    }
    let Some(dir) = data_dir(app) else {
        return confirmed;
    };
    let Ok(records) = crate::managed_agents::load_managed_agents(app) else {
        return confirmed;
    };
    let deadline = Instant::now() + CONFIRM_LIMIT;
    for record in &records {
        if record.bot_origin.is_some()
            || record.private_key_nsec.is_empty()
            || hosting_in(&dir, origin, &record.pubkey) != Some(Hosting::Here)
        {
            continue;
        }
        let Some(owner_hex) = crate::managed_agents::agent_owner_hex(app, record) else {
            continue;
        };
        if !owners.iter().any(|owner| owner.to_hex() == owner_hex) {
            continue;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let limit = remaining.min(CONFIRM_ONE_LIMIT);
        if let Ok(Registration::Here) =
            tokio::time::timeout(limit, register(app, record, origin, &owner_hex)).await
        {
            confirmed.insert(record.pubkey.to_ascii_lowercase());
        }
    }
    confirmed
}

/// The keys a sign-out may remove: agents recorded as hosted here whose copy
/// the relay confirmed just now. Pure.
fn keys_to_forget(
    pubkeys: &[String],
    hosting: impl Fn(&str) -> Option<Hosting>,
    confirmed: &HashSet<String>,
) -> Vec<String> {
    pubkeys
        .iter()
        .filter(|pubkey| hosting(pubkey) == Some(Hosting::Here))
        .filter(|pubkey| confirmed.contains(&pubkey.to_ascii_lowercase()))
        .cloned()
        .collect()
}

/// At sign-out, after the agents stopped: remove from this computer the
/// keys of `pubkeys` whose copy the relay confirmed for this device (see
/// [`confirm_copies`]; they come back at the next sign-in). Others stay.
/// Returns how many left.
pub(crate) fn forget_confirmed_keys<R: tauri::Runtime>(
    app: &AppHandle<R>,
    origin: &str,
    pubkeys: &[String],
    confirmed: &HashSet<String>,
) -> usize {
    let Some(dir) = data_dir(app) else {
        return 0;
    };
    let mut removed = 0;
    for pubkey in keys_to_forget(pubkeys, |p| hosting_in(&dir, origin, p), confirmed) {
        match crate::managed_agents::try_delete_agent_key(&pubkey) {
            Ok(()) => removed += 1,
            Err(error) => {
                eprintln!("kuzz-desktop: agents: key of {pubkey} kept at sign-out: {error}")
            }
        }
    }
    // Registered again (and so re-confirmed) at the next start.
    ATTEMPTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashSet::new)
        .retain(|key| !pubkeys.iter().any(|p| *key == entry_key(origin, p)));
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrong_host_stop_is_noted_once_and_only_for_the_last_run() {
        let stopped = "=== starting a (pk) at t ===\nERROR kuzz_acp: not starting: \
                       this agent is bound to another device (wrong_host)\n";
        let quiet = "=== starting a (pk) at t ===\nexited\n";
        assert!(unnoted_wrong_host(Some(0), stopped));
        assert!(unnoted_wrong_host(Some(WRONG_HOST_EXIT), quiet));
        assert!(!unnoted_wrong_host(Some(0), quiet));
        let noted = format!("{stopped}{WRONG_HOST_NOTED}\n");
        assert!(!unnoted_wrong_host(Some(WRONG_HOST_EXIT), &noted));
        assert!(!unnoted_wrong_host(
            Some(0),
            &format!("{stopped}=== starting a (pk) at t2 ===\nrunning\n")
        ));
    }

    #[test]
    fn only_confirmed_keys_hosted_here_are_forgotten() {
        let pubkeys = vec!["AA".to_owned(), "bb".to_owned(), "cc".to_owned()];
        let hosting = |p: &str| match p {
            "AA" | "bb" => Some(Hosting::Here),
            _ => Some(Hosting::OtherDevice),
        };
        let confirmed: HashSet<String> = ["aa".to_owned(), "cc".to_owned()].into();
        assert_eq!(keys_to_forget(&pubkeys, hosting, &confirmed), vec!["AA"]);
        assert!(keys_to_forget(&pubkeys, hosting, &HashSet::new()).is_empty());
    }

    #[test]
    fn restores_retry_only_without_a_definite_answer() {
        let error = |status: Option<u16>, code: Option<&str>| api::ApiError {
            status,
            code: code.map(str::to_owned),
            message: String::new(),
        };
        assert!(restore_retryable(&error(None, None)));
        assert!(restore_retryable(&error(Some(503), None)));
        assert!(restore_retryable(&error(Some(429), None)));
        assert!(restore_retryable(&error(
            Some(401),
            Some("device_proof_stale")
        )));
        assert!(!restore_retryable(&error(
            Some(403),
            Some("reauth_required")
        )));
        assert!(!restore_retryable(&error(
            Some(409),
            Some("device_key_required")
        )));
    }

    #[test]
    fn hosting_is_recorded_per_origin_and_agent() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = ("https://a.example", "https://b.example");
        assert_eq!(hosting_in(dir.path(), a, "AB"), None);
        set_hosting_in(dir.path(), a, "AB", Some(Hosting::OtherDevice)).unwrap();
        assert_eq!(hosting_in(dir.path(), a, "ab"), Some(Hosting::OtherDevice));
        assert_eq!(hosting_in(dir.path(), b, "ab"), None);
        set_hosting_in(dir.path(), a, "ab", Some(Hosting::Here)).unwrap();
        assert_eq!(hosting_in(dir.path(), a, "ab"), Some(Hosting::Here));
        set_hosting_in(dir.path(), a, "ab", None).unwrap();
        assert_eq!(hosting_in(dir.path(), a, "ab"), None);
    }

    #[test]
    fn register_body_verifies_on_the_relay_side() {
        let agent = Keys::generate();
        let owner = Keys::generate().public_key().to_hex();
        let device = "02".to_owned() + &"11".repeat(32);
        let challenge = "ab".repeat(32);
        let body = register_body(&agent, &challenge, &owner, &device, "[\"auth\"]");
        assert_eq!(body["agent_pubkey"], agent.public_key().to_hex());
        assert_eq!(body["secret_key"], agent.secret_key().to_secret_hex());
        kuzz_auth_pkg::key_backup::verify_agent_register(
            &agent.public_key(),
            body["signature"].as_str().unwrap(),
            &challenge,
            &owner,
            &device,
        )
        .unwrap();
        let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "agent_pubkey",
                "auth_tag",
                "challenge",
                "secret_key",
                "signature"
            ]
        );
    }

    #[test]
    fn restored_copies_fill_only_keyless_matching_agents() {
        let agent = Keys::generate();
        let other = Keys::generate();
        let record = |pubkey: String, nsec: &str| -> ManagedAgentRecord {
            serde_json::from_value(serde_json::json!({
                "pubkey": pubkey, "name": "a", "private_key_nsec": nsec,
                "relay_url": "wss://relay.test", "acp_command": "x", "agent_command": "x",
                "agent_args": [], "mcp_command": "x", "turn_timeout_seconds": 1,
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
            }))
            .unwrap()
        };
        let mut records = vec![
            record(agent.public_key().to_hex(), ""),
            record(other.public_key().to_hex(), "kept"),
        ];
        let copy = |pubkey: String, secret: String| api::AgentKeyCopy {
            agent_pubkey: pubkey,
            secret_key: api::SecretField(Zeroizing::new(secret)),
        };
        let copies = vec![
            copy(
                agent.public_key().to_hex(),
                agent.secret_key().to_secret_hex(),
            ),
            copy(
                other.public_key().to_hex(),
                other.secret_key().to_secret_hex(),
            ),
            // A copy whose secret does not match its agent is ignored.
            copy(
                Keys::generate().public_key().to_hex(),
                other.secret_key().to_secret_hex(),
            ),
        ];
        let filled = fill_missing_keys(&mut records, &copies);
        assert_eq!(filled, vec![agent.public_key().to_hex()]);
        assert_eq!(
            Keys::parse(&records[0].private_key_nsec)
                .unwrap()
                .public_key(),
            agent.public_key()
        );
        assert_eq!(records[1].private_key_nsec, "kept");
    }
}
