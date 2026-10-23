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

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

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
        if !register(&app, &record, &origin, &owner_hex).await {
            // Not answered: try again on the next start.
            ATTEMPTED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_or_insert_with(HashSet::new)
                .remove(&key);
        }
    });
}

/// Register one agent. Returns whether the relay gave a definite answer.
async fn register<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    origin: &str,
    owner_hex: &str,
) -> bool {
    let Some(device) = crate::device_key::current() else {
        return false;
    };
    // Only an agent attested to this owner (NIP-OA) can be registered.
    let Some(auth_tag) = record.auth_tag.clone() else {
        return true;
    };
    if crate::managed_agents::attested_owner_hex(record).as_deref() != Some(owner_hex) {
        return true;
    }
    let Ok(keys) = Keys::parse(&record.private_key_nsec) else {
        return true;
    };
    let Ok(owner) = PublicKey::from_hex(owner_hex) else {
        return true;
    };
    let Some(access) = owner_access(app, origin, &owner).await else {
        return false;
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
        return matches!(&outcome, Err(error) if error.status.is_some_and(|s| s < 500));
    };
    if let Some(dir) = data_dir(app) {
        if let Err(error) = set_hosting_in(&dir, origin, &record.pubkey, Some(hosting)) {
            eprintln!("kuzz-desktop: agents: hosting not recorded: {error}");
        }
    }
    true
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

/// After a sign-in of `owner`: put back the keys of this device's agents
/// that left at the last sign-out (only agents still listed here with no
/// key). Before the owner's agents start.
pub(crate) async fn restore_keys(app: &AppHandle, origin: &str, access: &str) {
    if crate::device_key::current().is_none() {
        return;
    }
    let state = app.state::<AppState>();
    let copies = match api::agent_keys(&state.http_client, origin, access).await {
        Ok(copies) => copies,
        Err(error) => {
            if error.status != Some(404) {
                eprintln!(
                    "kuzz-desktop: agents: keys not restored: {}",
                    error.code.as_deref().unwrap_or("no answer")
                );
            }
            return;
        }
    };
    if copies.is_empty() {
        return;
    }
    let restored = {
        let Ok(_guard) = state.managed_agents_store_lock.lock() else {
            return;
        };
        let mut records = match crate::managed_agents::load_managed_agents(app) {
            Ok(records) => records,
            Err(error) => {
                eprintln!("kuzz-desktop: agents: keys not restored: {error}");
                return;
            }
        };
        let restored = fill_missing_keys(&mut records, &copies);
        if restored.is_empty() {
            return;
        }
        if let Err(error) = crate::managed_agents::save_managed_agents(app, &records) {
            eprintln!("kuzz-desktop: agents: restored keys not saved: {error}");
            return;
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

/// At sign-out, after the agents stopped: remove from this computer the
/// keys of `pubkeys` whose copy the relay confirmed for this device (they
/// come back at the next sign-in). Others stay. Returns how many left.
pub(crate) fn forget_confirmed_keys<R: tauri::Runtime>(
    app: &AppHandle<R>,
    origin: &str,
    pubkeys: &[String],
) -> usize {
    let Some(dir) = data_dir(app) else {
        return 0;
    };
    let mut removed = 0;
    for pubkey in pubkeys {
        if hosting_in(&dir, origin, pubkey) != Some(Hosting::Here) {
            continue;
        }
        match crate::managed_agents::try_delete_agent_key(pubkey) {
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
