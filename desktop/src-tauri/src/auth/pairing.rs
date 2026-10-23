//! External agent pairing from the app (device-bound keys plan §7 and
//! §14.7, PR C2; no Approve step since 10-23).
//!
//! - "Connect external agent": [`start_agent_pairing`] asks the relay for a
//!   one-use, ten-minute `kuzz pair --relay <url> <code>` command; the screen polls
//!   [`list_agent_pairings`] until the first valid claim connects.
//! - Hermes on this computer: [`connect_hermes`] pairs in one click. It
//!   claims a new pairing with the account's Hermes agent key (made once,
//!   then reused) and this computer's device key, through the same
//!   `POST /pair/claim` an external computer uses. Offered only when the
//!   `kuzz pair --hermes` installer is on this computer (PR E).
//! - [`disconnect_external_device`] signs a paired computer out and forgets
//!   any Hermes key bound to it. Signing an account out here forgets its
//!   Hermes keys too ([`watch_accounts`]).
//!
//! Codes, commands and keys are never logged.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use nostr::Keys;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Listener as _, Manager};
use zeroize::Zeroizing;

use super::accounts::{AccountState, Registry, ANY_ORIGIN};
use super::api;
use crate::app_state::AppState;

/// Hermes agents this app paired, with their keys. Only the current user can
/// open it ([`super::private_file`]).
const HERMES_FILE: &str = "hermes-agents.json";
/// Longest a local pairing waits for the device key to open.
const DEVICE_WAIT: Duration = Duration::from_secs(3);
/// Longest the installer's `--help` probe and install may take.
const HOOK_PROBE_LIMIT: Duration = Duration::from_secs(5);
const HOOK_RUN_LIMIT: Duration = Duration::from_secs(60);
/// The relay's refusal for an agent key bound already: the key stays.
const KEY_IN_USE: &str = "key_in_use";

static FILE_LOCK: Mutex<()> = Mutex::new(());

/// What the connect screen shows for a new code.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStartView {
    pairing_id: String,
    /// `kuzz pair --relay <url> <code>` as the relay gave it, shown so the
    /// owner can copy it.
    command: String,
    expires_at: String,
    expires_in: i64,
}

/// Whether Hermes is on this computer, whether this app can connect it, and
/// the Hermes agents this app paired for the active account.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesStatus {
    found: bool,
    /// The `kuzz pair --hermes` installer is here too.
    can_connect: bool,
    agents: Vec<HermesAgentView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesAgentView {
    agent_pubkey: String,
    device_id: Option<String>,
}

/// A Hermes agent paired on this computer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesConnected {
    agent_pubkey: String,
    device_id: String,
    device_name: Option<String>,
}

/// One stored Hermes agent. `device_id` is set once the claim connected.
#[derive(Clone, Serialize, Deserialize)]
struct HermesRecord {
    origin: String,
    owner: String,
    agent_pubkey: String,
    secret_key: String,
    #[serde(default)]
    device_id: Option<String>,
}

fn api_error(error: api::ApiError) -> String {
    match error.code.as_deref() {
        Some("code_used") => "This code was already used. Make a new code.".into(),
        Some("invalid_code") => "This code expired. Make a new code.".into(),
        Some("device_limit") => {
            "Your account already has the most connected computers it can.".into()
        }
        Some("too_many_pairings") => "Too many unused codes. Use one or wait a few minutes.".into(),
        _ => error.to_string(),
    }
}

/// Whether a failed claim was a definite refusal that bound nothing, so a
/// key made for it may go: a 4xx answer other than "key already in use".
/// 5xx and lost answers may have connected. Pure.
fn refused_for_good(error: &api::ApiError) -> bool {
    matches!(error.status, Some(400..=499)) && error.code.as_deref() != Some(KEY_IN_USE)
}

/// `kuzz pair <code>` and its expiry, for the connect screen.
#[tauri::command]
pub async fn start_agent_pairing(app: AppHandle) -> Result<PairingStartView, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = super::commands::fresh_session(&state, &origin).await?;
    // The relay checks this device's proof; open the key if it is not yet.
    let _ = crate::device_key::ensure_within(DEVICE_WAIT).await;
    let started = api::pairing_start(&state.http_client, &origin, &session.access)
        .await
        .map_err(api_error)?;
    Ok(PairingStartView {
        pairing_id: started.pairing_id,
        command: started.command.to_string(),
        expires_at: started.expires_at,
        expires_in: started.expires_in,
    })
}

/// The active account's pairings of the last day, newest first.
#[tauri::command]
pub async fn list_agent_pairings(app: AppHandle) -> Result<Vec<api::PairingInfo>, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = super::commands::fresh_session(&state, &origin).await?;
    api::pairing_list(&state.http_client, &origin, &session.access)
        .await
        .map_err(api_error)
}

/// Sign a paired computer out (`DELETE /auth/devices/{id}`) and forget the
/// Hermes key bound to it, if any.
#[tauri::command]
pub async fn disconnect_external_device(app: AppHandle, device_id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = super::commands::fresh_session(&state, &origin).await?;
    api::revoke_device(&state.http_client, &origin, &session.access, &device_id)
        .await
        .map_err(api_error)?;
    if let Some(dir) = data_dir(&app) {
        update_hermes(&dir, |records| {
            records.retain(|record| {
                !(record.origin == origin && record.device_id.as_deref() == Some(&device_id))
            });
        })?;
    }
    Ok(())
}

/// Whether Hermes is installed here, whether it can be connected, and its
/// agents this app paired. A pairing whose answer was lost is checked with
/// the relay first, so a connected one is not offered again.
#[tauri::command]
pub async fn get_hermes_status(app: AppHandle) -> Result<HermesStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let owner = state.token_auth.signing_pubkey(&origin);
    let (found, can_connect) = tokio::task::spawn_blocking(|| {
        let found = hermes_home().is_some();
        (found, found && hermes_installer().is_some())
    })
    .await
    .unwrap_or((false, false));
    let (Some(owner), Some(dir)) = (owner, data_dir(&app)) else {
        return Ok(HermesStatus {
            found,
            can_connect,
            agents: Vec::new(),
        });
    };
    let owner = owner.to_hex();
    prune_signed_out(&dir);
    if record_for(&dir, &origin, &owner)?.is_some_and(|record| record.device_id.is_none()) {
        if let Ok(session) = super::commands::fresh_session(&state, &origin).await {
            if let Ok(pairings) =
                api::pairing_list(&state.http_client, &origin, &session.access).await
            {
                settle_lost_claim(&dir, &origin, &owner, &pairings)?;
            }
        }
    }
    Ok(HermesStatus {
        found,
        can_connect,
        agents: hermes_agents_in(&dir, &origin, &owner)?,
    })
}

/// Pair Hermes on this computer in one step: the account's Hermes agent key
/// (made the first time), a new code, and the claim with this computer's
/// device key. Already connected: nothing new is made.
#[tauri::command]
pub async fn connect_hermes(app: AppHandle) -> Result<HermesConnected, String> {
    let (home, installer) = tokio::task::spawn_blocking(|| {
        let home = hermes_home()?;
        Some((home, hermes_installer()?))
    })
    .await
    .ok()
    .flatten()
    .ok_or("Hermes can't be connected from this app yet.")?;
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let owner = state
        .token_auth
        .signing_pubkey(&origin)
        .ok_or("Sign in with Google to connect Hermes.")?
        .to_hex();
    let dir = data_dir(&app).ok_or("This computer has no app data folder.")?;
    let session = super::commands::fresh_session(&state, &origin).await?;
    let devices = api::list_devices(&state.http_client, &origin, &session.access)
        .await
        .map_err(api_error)?;
    let device_name = devices
        .iter()
        .find(|device| device.current)
        .map(|device| device.name.clone());

    // The key made before, if any: a lost answer may have connected it.
    let mut existing = record_for(&dir, &origin, &owner)?;
    if existing
        .as_ref()
        .is_some_and(|record| record.device_id.is_none())
    {
        let pairings = api::pairing_list(&state.http_client, &origin, &session.access)
            .await
            .map_err(api_error)?;
        settle_lost_claim(&dir, &origin, &owner, &pairings)?;
        existing = record_for(&dir, &origin, &owner)?;
    }
    if let Some(record) = &existing {
        if let Some(device_id) = record
            .device_id
            .as_ref()
            .filter(|id| devices.iter().any(|device| &device.id == *id))
        {
            return Ok(HermesConnected {
                agent_pubkey: record.agent_pubkey.clone(),
                device_id: device_id.clone(),
                device_name,
            });
        }
    }

    let device = crate::device_key::ensure_within(DEVICE_WAIT)
        .await
        .ok_or("This computer is not ready to connect agents yet. Try again in a moment.")?;
    let agent = match &existing {
        Some(record) => Keys::parse(&record.secret_key)
            .map_err(|_| "The saved Hermes key is damaged. Disconnect Hermes and try again.")?,
        None => Keys::generate(),
    };
    let agent_pubkey = agent.public_key().to_hex();
    let started = api::pairing_start(&state.http_client, &origin, &session.access)
        .await
        .map_err(api_error)?;
    let body = {
        let code = started.code.0.clone();
        let agent = agent.clone();
        let device_name = device_name.clone();
        tokio::time::timeout(
            crate::device_key::USER_WAIT,
            tokio::task::spawn_blocking(move || {
                claim_body(&code, &agent, &device, device_name.as_deref())
            }),
        )
        .await
        .map_err(|_| "This computer took too long to sign. Try again.".to_string())?
        .map_err(|_| "This computer could not sign. Try again.".to_string())??
    };
    // Keep the key before the claim binds it, so a connected agent is never
    // left without its key here. A reused key drops its old computer.
    let record = HermesRecord {
        origin: origin.clone(),
        owner: owner.clone(),
        agent_pubkey: agent_pubkey.clone(),
        secret_key: agent.secret_key().to_secret_hex(),
        device_id: None,
    };
    update_hermes(&dir, |records| {
        records.retain(|old| !(old.origin == origin && old.owner == owner));
        records.push(record);
    })?;
    match api::pair_claim(&state.http_client, &origin, &body).await {
        Ok(device_id) => {
            if let Err(error) = set_hermes_device_in(&dir, &origin, &agent_pubkey, &device_id) {
                eprintln!("kuzz-desktop: hermes: connection not recorded: {error}");
            }
            let relay = crate::relay::relay_ws_url_with_override(&state);
            let secret = Zeroizing::new(agent.secret_key().to_secret_hex());
            tauri::async_runtime::spawn(hermes_plugin_hook(installer, home, relay, secret));
            Ok(HermesConnected {
                agent_pubkey,
                device_id,
                device_name,
            })
        }
        Err(error) => {
            // Only a definite refusal bound nothing; after a 5xx or a lost
            // answer the key stays, and the next check asks the relay.
            if refused_for_good(&error) {
                let _ = forget_hermes_in(&dir, &origin, &agent_pubkey);
            }
            Err(api_error(error))
        }
    }
}

/// The `/pair/claim` body: the agent (BIP-340 over SHA-256) and the device
/// (ECDSA P-256) both sign [`kuzz_auth_pkg::pairing::pair_claim_message`].
/// Blocks on the device key (TPM).
fn claim_body(
    code: &str,
    agent: &Keys,
    device: &crate::device_key::DeviceKey,
    device_name: Option<&str>,
) -> Result<serde_json::Value, String> {
    use kuzz_auth_pkg::pairing::{pair_claim_message, pair_code_hash};
    use nostr::hashes::{sha256::Hash as Sha256Hash, Hash as _};

    let agent_hex = agent.public_key().to_hex();
    let device_hex = device.pubkey().to_hex();
    let message = pair_claim_message(&pair_code_hash(code), &agent_hex, &device_hex);
    let digest = Sha256Hash::hash(message.as_bytes());
    let agent_signature = agent.sign_schnorr(&nostr::secp256k1::Message::from_digest(
        digest.to_byte_array(),
    ));
    let device_signature = device
        .sign(message.as_bytes())
        .map_err(|_| "This computer could not sign. Try again.".to_string())?;
    let mut body = serde_json::json!({
        "code": code,
        "agent_pubkey": agent_hex,
        "agent_signature": agent_signature.to_string(),
        "device_pubkey": device_hex,
        "device_signature": hex::encode(device_signature),
        "device_key_kind": device.kind().as_wire(),
    });
    if let (Some(name), Some(object)) = (device_name, body.as_object_mut()) {
        object.insert("device_name".into(), name.into());
    }
    Ok(body)
}

/// Hermes's home on this computer: `$HERMES_HOME`, else
/// `%LOCALAPPDATA%\hermes`, when that folder exists.
pub(crate) fn hermes_home() -> Option<PathBuf> {
    hermes_home_from(|name| std::env::var_os(name), Path::is_dir)
}

/// [`hermes_home`] reading variables through `var` and folders through
/// `is_dir`. Pure.
fn hermes_home_from(
    var: impl Fn(&str) -> Option<OsString>,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let absolute = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    [
        absolute("HERMES_HOME"),
        absolute("LOCALAPPDATA").map(|base| base.join("hermes")),
    ]
    .into_iter()
    .flatten()
    .find(|path| is_dir(path))
}

/// The `kuzz` that installs the Hermes plugin: one whose `pair --help`
/// lists `--hermes` (PR E). `None` until then, so Hermes is not offered.
/// Blocks for at most [`HOOK_PROBE_LIMIT`].
fn hermes_installer() -> Option<PathBuf> {
    let kuzz = crate::managed_agents::resolve_command("kuzz")?;
    let mut probe = std::process::Command::new(&kuzz);
    probe.args(["pair", "--help"]);
    let help = crate::managed_agents::output_with_timeout(probe, HOOK_PROBE_LIMIT)?;
    lists_hermes_flag(&String::from_utf8_lossy(&help.stdout)).then_some(kuzz)
}

/// Whether `kuzz pair --help` output offers `--hermes`. Pure.
fn lists_hermes_flag(help: &str) -> bool {
    help.split(|c: char| c.is_whitespace() || c == ',' || c == '=' || c == '[' || c == ']')
        .any(|word| word == "--hermes")
}

/// Install the Hermes Kuzz plugin with `kuzz pair --hermes` (PR E), the agent
/// key in the environment (never as an argument). Logs only whether it
/// worked.
async fn hermes_plugin_hook(
    installer: PathBuf,
    home: PathBuf,
    relay: String,
    secret: Zeroizing<String>,
) {
    let ran = tokio::task::spawn_blocking(move || {
        let mut install = std::process::Command::new(&installer);
        install
            .args(["pair", "--hermes"])
            .env("HERMES_HOME", &home)
            .env("KUZZ_RELAY_URL", &relay)
            .env("KUZZ_PRIVATE_KEY", secret.as_str());
        crate::managed_agents::output_with_timeout(install, HOOK_RUN_LIMIT)
            .map(|output| output.status.success())
    })
    .await
    .ok()
    .flatten();
    eprintln!(
        "kuzz-desktop: hermes: plugin install finished (ok: {})",
        ran.unwrap_or(false)
    );
}

/// Forget the Hermes keys of accounts signed out of (or removed from) this
/// computer whenever the account list changes.
pub(crate) fn watch_accounts(app: &AppHandle) {
    let handle = app.clone();
    app.listen_any(super::ACCOUNTS_CHANGED_EVENT, move |_| {
        if let Some(dir) = data_dir(&handle) {
            tauri::async_runtime::spawn_blocking(move || prune_signed_out(&dir));
        }
    });
}

/// Drop the Hermes records whose account is no longer signed in here.
/// Nothing changes when the account list cannot be read.
fn prune_signed_out(dir: &Path) {
    let Ok(Some(registry)) = super::accounts::load_registry(dir) else {
        return;
    };
    let result = update_hermes_if(dir, |records| {
        let before = records.len();
        records.retain(|record| still_signed_in(&registry, &record.origin, &record.owner));
        records.len() != before
    });
    if let Err(error) = result {
        eprintln!("kuzz-desktop: hermes: signed-out keys not removed: {error}");
    }
}

/// Whether `owner` is still signed in to `origin` here (an ended session
/// waiting for a new sign-in counts; a sign-out or removal does not). Pure.
fn still_signed_in(registry: &Registry, origin: &str, owner: &str) -> bool {
    registry.accounts.iter().any(|account| {
        account.pubkey.eq_ignore_ascii_case(owner)
            && (account.origin == origin || account.origin == ANY_ORIGIN)
            && matches!(
                account.state,
                AccountState::SignedIn | AccountState::NeedsLogin
            )
    })
}

/// A claim whose answer was lost: if the relay lists it connected, record
/// its computer.
fn settle_lost_claim(
    dir: &Path,
    origin: &str,
    owner: &str,
    pairings: &[api::PairingInfo],
) -> Result<(), String> {
    update_hermes_if(dir, |records| {
        let mut changed = false;
        for record in records
            .iter_mut()
            .filter(|r| r.origin == origin && r.owner == owner && r.device_id.is_none())
        {
            let connected = pairings.iter().find(|pairing| {
                pairing.state == "connected"
                    && pairing
                        .agent_pubkey
                        .as_deref()
                        .is_some_and(|agent| agent.eq_ignore_ascii_case(&record.agent_pubkey))
            });
            if let Some(device_id) = connected.and_then(|pairing| pairing.device_id.clone()) {
                record.device_id = Some(device_id);
                changed = true;
            }
        }
        changed
    })
}

fn data_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
}

fn hermes_path(dir: &Path) -> PathBuf {
    dir.join(HERMES_FILE)
}

/// The stored records; none when the file does not exist yet. Any other
/// read or parse failure is an error, so nothing overwrites the file.
fn read_hermes(dir: &Path) -> Result<Vec<HermesRecord>, String> {
    let bytes = match std::fs::read(hermes_path(dir)) {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Hermes agents unreadable: {error}")),
    };
    serde_json::from_slice(&bytes).map_err(|_| "Hermes agents file is damaged.".to_string())
}

fn write_hermes(dir: &Path, records: &[HermesRecord]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create app data folder: {e}"))?;
    let json = Zeroizing::new(
        serde_json::to_vec_pretty(records).map_err(|e| format!("encode Hermes agents: {e}"))?,
    );
    super::private_file::write_owner_only(&hermes_path(dir), &json)
}

/// Read, change and write the records under one lock. `change` returns
/// whether it changed anything; unchanged records are not written.
fn update_hermes_if(
    dir: &Path,
    change: impl FnOnce(&mut Vec<HermesRecord>) -> bool,
) -> Result<(), String> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut records = read_hermes(dir)?;
    if change(&mut records) {
        write_hermes(dir, &records)?;
    }
    Ok(())
}

fn update_hermes(dir: &Path, change: impl FnOnce(&mut Vec<HermesRecord>)) -> Result<(), String> {
    update_hermes_if(dir, |records| {
        change(records);
        true
    })
}

fn set_hermes_device_in(
    dir: &Path,
    origin: &str,
    agent: &str,
    device_id: &str,
) -> Result<(), String> {
    update_hermes(dir, |records| {
        for record in records.iter_mut() {
            if record.origin == origin && record.agent_pubkey == agent {
                record.device_id = Some(device_id.to_owned());
            }
        }
    })
}

fn forget_hermes_in(dir: &Path, origin: &str, agent: &str) -> Result<(), String> {
    update_hermes(dir, |records| {
        records.retain(|record| !(record.origin == origin && record.agent_pubkey == agent));
    })
}

/// `owner`'s Hermes record on `origin`, if one was made.
fn record_for(dir: &Path, origin: &str, owner: &str) -> Result<Option<HermesRecord>, String> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    Ok(read_hermes(dir)?
        .into_iter()
        .rev()
        .find(|record| record.origin == origin && record.owner == owner))
}

/// `owner`'s Hermes agents on `origin` (no keys).
fn hermes_agents_in(dir: &Path, origin: &str, owner: &str) -> Result<Vec<HermesAgentView>, String> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    Ok(read_hermes(dir)?
        .into_iter()
        .filter(|record| record.origin == origin && record.owner == owner)
        .map(|record| HermesAgentView {
            agent_pubkey: record.agent_pubkey,
            device_id: record.device_id,
        })
        .collect())
}

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;
