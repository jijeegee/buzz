//! Who owns each local agent.
//!
//! Several accounts can be signed in on this computer and every account's
//! agents keep running whichever account is active, so an agent's owner must
//! come from the agent itself, never from the active account. It is the owner
//! signed into the agent's NIP-OA `auth_tag`. Older agents created without one
//! get their owner written once into `agent-owners.json` (next to the agent
//! store) by [`migrate_owners`], from the account that was active when this
//! computer moved to multi-account.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nostr::PublicKey;
use tauri::AppHandle;

use super::ManagedAgentRecord;

/// File (next to the agent store) holding recorded owners.
pub(crate) const FILE_NAME: &str = "agent-owners.json";

fn owners_path<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    super::managed_agents_base_dir(app)
        .ok()
        .map(|dir| dir.join(FILE_NAME))
}

fn read_owners(path: &Path) -> BTreeMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The owner signed into `record`'s attestation, when it verifies.
pub(crate) fn attested_owner_hex(record: &ManagedAgentRecord) -> Option<String> {
    let tag = record.auth_tag.as_deref()?;
    let agent = PublicKey::from_hex(&record.pubkey).ok()?;
    buzz_sdk_pkg::nip_oa::verify_auth_tag(tag, &agent)
        .ok()
        .map(|owner| owner.to_hex())
}

/// The owner of `record`: its attestation, else the owner recorded for it at
/// migration. `None` for an agent with neither (callers fall back to their
/// legacy behaviour).
pub(crate) fn agent_owner_hex<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
) -> Option<String> {
    attested_owner_hex(record).or_else(|| {
        let path = owners_path(app)?;
        read_owners(&path).remove(&record.pubkey.to_ascii_lowercase())
    })
}

/// Agents (by pubkey) with no attested owner and no recorded owner.
fn unowned<'a>(
    records: &'a [ManagedAgentRecord],
    recorded: &BTreeMap<String, String>,
) -> impl Iterator<Item = String> + 'a {
    let recorded: Vec<String> = recorded.keys().cloned().collect();
    records
        .iter()
        .filter(|record| !record.pubkey.is_empty() && record.bot_origin.is_none())
        .filter(|record| attested_owner_hex(record).is_none())
        .map(|record| record.pubkey.to_ascii_lowercase())
        .filter(move |pubkey| !recorded.contains(pubkey))
}

/// Record `owner` for every agent that has no attested or recorded owner yet.
/// Returns how many were recorded.
pub(crate) fn record_missing_owners(
    path: &Path,
    records: &[ManagedAgentRecord],
    owner: &PublicKey,
) -> Result<usize, String> {
    let mut recorded = read_owners(path);
    let missing: Vec<String> = unowned(records, &recorded).collect();
    if missing.is_empty() {
        return Ok(0);
    }
    for pubkey in &missing {
        recorded.insert(pubkey.clone(), owner.to_hex());
    }
    let json = serde_json::to_vec_pretty(&recorded).map_err(|e| e.to_string())?;
    super::storage::atomic_write_json(path, &json)?;
    Ok(missing.len())
}

/// One-time launch migration: older agents without an attestation get the
/// account active now as their owner, unless owners were already recorded
/// when the account list was created.
pub(crate) fn migrate_owners<R: tauri::Runtime>(app: &AppHandle<R>) {
    use tauri::Manager;
    if let Ok(data_dir) = app.path().app_data_dir() {
        crate::auth::accounts::record_owners_on_launch(&data_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(pubkey: &str, auth_tag: Option<String>) -> ManagedAgentRecord {
        let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
            "pubkey": pubkey,
            "name": "a",
            "relay_url": "wss://relay.test",
            "acp_command": "x",
            "agent_command": "x",
            "agent_args": [],
            "mcp_command": "x",
            "turn_timeout_seconds": 1,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        record.auth_tag = auth_tag;
        record
    }

    #[test]
    fn owners_come_from_the_attestation_else_the_migration_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let owner_a = nostr::Keys::generate();
        let owner_b = nostr::Keys::generate().public_key();
        let tagged = nostr::Keys::generate().public_key();
        let tag = buzz_sdk_pkg::nip_oa::compute_auth_tag(&owner_a, &tagged, "kind=9").unwrap();
        let legacy = nostr::Keys::generate().public_key().to_hex();
        let records = vec![record(&tagged.to_hex(), Some(tag)), record(&legacy, None)];
        assert_eq!(
            attested_owner_hex(&records[0]),
            Some(owner_a.public_key().to_hex())
        );
        assert_eq!(attested_owner_hex(&records[1]), None);
        // Migration records only the unattested agent, under the given owner.
        assert_eq!(record_missing_owners(&path, &records, &owner_b).unwrap(), 1);
        let recorded = read_owners(&path);
        assert_eq!(recorded.get(&legacy), Some(&owner_b.to_hex()));
        assert!(!recorded.contains_key(&tagged.to_hex()));
        // Running again with another (now active) account changes nothing.
        let later = nostr::Keys::generate().public_key();
        assert_eq!(record_missing_owners(&path, &records, &later).unwrap(), 0);
        assert_eq!(read_owners(&path).get(&legacy), Some(&owner_b.to_hex()));
    }
}
