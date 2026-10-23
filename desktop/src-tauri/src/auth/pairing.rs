//! External agent pairing from the app (device-bound keys plan §7 and
//! §14.7, PR C2; no Approve step since 10-23).
//!
//! - "Connect external agent": [`start_agent_pairing`] asks the relay for a
//!   one-use, ten-minute `kuzz pair <code>` command; the screen polls
//!   [`list_agent_pairings`] until the first valid claim connects.
//! - Hermes on this computer: [`connect_hermes`] pairs in one click. It makes
//!   a Hermes agent key, starts a pairing and claims it with that key and
//!   this computer's device key through the same `POST /pair/claim` an
//!   external computer uses.
//!
//! Codes, commands and keys are never logged.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use nostr::Keys;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use zeroize::Zeroizing;

use super::api;
use crate::app_state::AppState;

/// Hermes agents this app paired, with their keys (owner-only file).
const HERMES_FILE: &str = "hermes-agents.json";
/// Longest a local pairing waits for the device key to open.
const DEVICE_WAIT: Duration = Duration::from_secs(3);
/// Longest the plugin hook's `--help` probe and install may take.
const HOOK_PROBE_LIMIT: Duration = Duration::from_secs(5);
const HOOK_RUN_LIMIT: Duration = Duration::from_secs(60);

static FILE_LOCK: Mutex<()> = Mutex::new(());

/// What the connect screen shows for a new code.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStartView {
    pairing_id: String,
    /// `kuzz pair <code>`, shown so the owner can copy it.
    command: String,
    expires_at: String,
    expires_in: i64,
}

/// Whether Hermes is on this computer, and the Hermes agents this app paired
/// for the active account.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesStatus {
    found: bool,
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

/// Whether Hermes is installed here, and its agents this app paired.
#[tauri::command]
pub async fn get_hermes_status(app: AppHandle) -> Result<HermesStatus, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let owner = state.token_auth.signing_pubkey(&origin);
    let found = tokio::task::spawn_blocking(hermes_home)
        .await
        .ok()
        .flatten()
        .is_some();
    let agents = match (owner, data_dir(&app)) {
        (Some(owner), Some(dir)) => hermes_agents_in(&dir, &origin, &owner.to_hex()),
        _ => Vec::new(),
    };
    Ok(HermesStatus { found, agents })
}

/// Pair Hermes on this computer in one step: a new Hermes agent key, a new
/// code, and the claim with this computer's device key.
#[tauri::command]
pub async fn connect_hermes(app: AppHandle) -> Result<HermesConnected, String> {
    let home = tokio::task::spawn_blocking(hermes_home)
        .await
        .ok()
        .flatten()
        .ok_or("Hermes was not found on this computer.")?;
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let owner = state
        .token_auth
        .signing_pubkey(&origin)
        .ok_or("Sign in with Google to connect Hermes.")?
        .to_hex();
    let dir = data_dir(&app).ok_or("This computer has no app data folder.")?;
    let session = super::commands::fresh_session(&state, &origin).await?;
    let device = crate::device_key::ensure_within(DEVICE_WAIT)
        .await
        .ok_or("This computer is not ready to connect agents yet. Try again in a moment.")?;
    let device_name = api::list_devices(&state.http_client, &origin, &session.access)
        .await
        .ok()
        .and_then(|devices| devices.into_iter().find(|device| device.current))
        .map(|device| device.name);
    let started = api::pairing_start(&state.http_client, &origin, &session.access)
        .await
        .map_err(api_error)?;

    let agent = Keys::generate();
    let agent_pubkey = agent.public_key().to_hex();
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
    // left without its key here.
    remember_hermes_in(
        &dir,
        HermesRecord {
            origin: origin.clone(),
            owner,
            agent_pubkey: agent_pubkey.clone(),
            secret_key: agent.secret_key().to_secret_hex(),
            device_id: None,
        },
    )?;
    match api::pair_claim(&state.http_client, &origin, &body).await {
        Ok(device_id) => {
            if let Err(error) = set_hermes_device_in(&dir, &origin, &agent_pubkey, &device_id) {
                eprintln!("kuzz-desktop: hermes: connection not recorded: {error}");
            }
            let relay = crate::relay::relay_ws_url_with_override(&state);
            let secret = Zeroizing::new(agent.secret_key().to_secret_hex());
            tauri::async_runtime::spawn(hermes_plugin_hook(home, relay, secret));
            Ok(HermesConnected {
                agent_pubkey,
                device_id,
                device_name,
            })
        }
        Err(error) => {
            // A refusal binds nothing; a lost answer may have connected, so
            // that key is kept (the owner can disconnect the computer).
            if error.status.is_some() {
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

/// TODO(E): the Hermes Kuzz plugin and its credentials are installed by PR E
/// (`kuzz pair --hermes`). Until a `kuzz` whose `pair --help` lists
/// `--hermes` is found this does nothing; then it runs that command with
/// the agent key in the environment (never as an argument) and logs only
/// whether it worked.
async fn hermes_plugin_hook(home: PathBuf, relay: String, secret: Zeroizing<String>) {
    let ran = tokio::task::spawn_blocking(move || {
        let kuzz = crate::managed_agents::resolve_command("kuzz")?;
        let mut probe = std::process::Command::new(&kuzz);
        probe.args(["pair", "--help"]);
        let help = crate::managed_agents::output_with_timeout(probe, HOOK_PROBE_LIMIT)?;
        if !String::from_utf8_lossy(&help.stdout).contains("--hermes") {
            return None;
        }
        let mut install = std::process::Command::new(&kuzz);
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
    if let Some(ok) = ran {
        eprintln!("kuzz-desktop: hermes: plugin install finished (ok: {ok})");
    }
}

fn data_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
}

fn hermes_path(dir: &Path) -> PathBuf {
    dir.join(HERMES_FILE)
}

fn read_hermes(dir: &Path) -> Vec<HermesRecord> {
    std::fs::read(hermes_path(dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_hermes(dir: &Path, records: &[HermesRecord]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create app data folder: {e}"))?;
    let json = Zeroizing::new(
        serde_json::to_vec_pretty(records).map_err(|e| format!("encode Hermes agents: {e}"))?,
    );
    crate::managed_agents::atomic_write_json_restricted(&hermes_path(dir), &json)
}

fn update_hermes(dir: &Path, change: impl FnOnce(&mut Vec<HermesRecord>)) -> Result<(), String> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut records = read_hermes(dir);
    change(&mut records);
    write_hermes(dir, &records)
}

fn remember_hermes_in(dir: &Path, record: HermesRecord) -> Result<(), String> {
    update_hermes(dir, |records| records.push(record))
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

/// `owner`'s Hermes agents on `origin` (no keys).
fn hermes_agents_in(dir: &Path, origin: &str, owner: &str) -> Vec<HermesAgentView> {
    read_hermes(dir)
        .into_iter()
        .filter(|record| record.origin == origin && record.owner == owner)
        .map(|record| HermesAgentView {
            agent_pubkey: record.agent_pubkey,
            device_id: record.device_id,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[cfg(windows)]
    const HOME: &str = r"C:\Users\u\hermes-home";
    #[cfg(not(windows))]
    const HOME: &str = "/home/u/hermes-home";
    #[cfg(windows)]
    const LOCAL: &str = r"C:\Users\u\AppData\Local";
    #[cfg(not(windows))]
    const LOCAL: &str = "/home/u/local";

    #[test]
    fn hermes_is_found_by_hermes_home_first() {
        let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
        assert_eq!(hermes_home_from(&var, |_| true), Some(PathBuf::from(HOME)));
    }

    #[test]
    fn hermes_is_found_in_local_app_data() {
        let var = vars(&[("LOCALAPPDATA", LOCAL)]);
        let expected = PathBuf::from(LOCAL).join("hermes");
        let found = hermes_home_from(&var, |path| path == expected.as_path());
        assert_eq!(found, Some(PathBuf::from(LOCAL).join("hermes")));
    }

    #[test]
    fn hermes_home_that_does_not_exist_falls_back() {
        let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
        let local = PathBuf::from(LOCAL).join("hermes");
        let found = hermes_home_from(&var, |path| path == local.as_path());
        assert_eq!(found, Some(local));
    }

    #[test]
    fn no_hermes_without_folders_or_with_relative_paths() {
        let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
        assert_eq!(hermes_home_from(&var, |_| false), None);
        assert_eq!(hermes_home_from(vars(&[]), |_| true), None);
        let relative = vars(&[("HERMES_HOME", "hermes"), ("LOCALAPPDATA", "")]);
        assert_eq!(hermes_home_from(relative, |_| true), None);
    }

    #[test]
    fn claim_body_verifies_with_the_relay_check() {
        let dir = tempfile::tempdir().unwrap();
        let device =
            crate::device_key::DeviceKey::open_or_create_file(&dir.path().join("device.key"))
                .unwrap();
        let agent = Keys::generate();
        let code = "AAAAAAAAAAAAAAAAAAAAAA";
        let body = claim_body(code, &agent, &device, Some("akak-PC")).unwrap();
        assert_eq!(body["code"], code);
        assert_eq!(body["device_name"], "akak-PC");
        assert_eq!(body["device_key_kind"], "software");
        let device_pubkey = kuzz_auth_pkg::device_proof::DevicePubkey::from_hex(
            body["device_pubkey"].as_str().unwrap(),
        )
        .unwrap();
        kuzz_auth_pkg::pairing::verify_pair_claim(
            code,
            &agent.public_key(),
            body["agent_signature"].as_str().unwrap(),
            &device_pubkey,
            body["device_signature"].as_str().unwrap(),
        )
        .expect("the relay accepts the claim signatures");
        // Another code does not verify with these signatures.
        assert!(kuzz_auth_pkg::pairing::verify_pair_claim(
            "BBBBBBBBBBBBBBBBBBBBBB",
            &agent.public_key(),
            body["agent_signature"].as_str().unwrap(),
            &device_pubkey,
            body["device_signature"].as_str().unwrap(),
        )
        .is_err());
        let without_name = claim_body(code, &agent, &device, None).unwrap();
        assert!(without_name.get("device_name").is_none());
    }

    #[test]
    fn hermes_records_are_kept_per_account_and_connected_by_device() {
        let dir = tempfile::tempdir().unwrap();
        let record = |owner: &str, agent: &str| HermesRecord {
            origin: "https://relay.example".into(),
            owner: owner.into(),
            agent_pubkey: agent.into(),
            secret_key: "00".repeat(32),
            device_id: None,
        };
        remember_hermes_in(dir.path(), record("o1", "a1")).unwrap();
        remember_hermes_in(dir.path(), record("o2", "a2")).unwrap();
        set_hermes_device_in(dir.path(), "https://relay.example", "a1", "d1").unwrap();
        let mine = hermes_agents_in(dir.path(), "https://relay.example", "o1");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].agent_pubkey, "a1");
        assert_eq!(mine[0].device_id.as_deref(), Some("d1"));
        assert!(hermes_agents_in(dir.path(), "https://other.example", "o1").is_empty());
        forget_hermes_in(dir.path(), "https://relay.example", "a2").unwrap();
        assert!(hermes_agents_in(dir.path(), "https://relay.example", "o2").is_empty());
        // The view never carries the key.
        let view = serde_json::to_string(&mine).unwrap();
        assert!(!view.contains(&"00".repeat(32)));
    }
}
