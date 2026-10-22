//! Queue of agent profiles (kind:0) to republish after a local rename.
//!
//! Written by the former Bumble-to-Pollen rename migration; agents keep
//! reading it until every queued profile is reconciled.

use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct ProfileReconcileQueueEntry {
    pub(crate) pubkey: String,
    #[serde(default = "default_profile_reconcile_name")]
    pub(crate) expected_name: String,
    /// Canonical relay identities already repaired for this migrated agent.
    ///
    /// Keep the entry after success: Desktop does not persist its community
    /// list in Rust, so a community that is inactive (or re-added later) must
    /// still get one repair when it is next applied.
    #[serde(default)]
    pub(crate) reconciled_relays: Vec<String>,
}

fn default_profile_reconcile_name() -> String {
    crate::managed_agents::POLLEN_DISPLAY_NAME.to_string()
}

#[derive(serde::Deserialize)]
struct CurrentProfileReconcileQueueEntry {
    pubkey: String,
    #[serde(default = "default_profile_reconcile_name")]
    expected_name: String,
    #[serde(default)]
    reconciled_relays: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredProfileReconcileQueueEntry {
    Current(CurrentProfileReconcileQueueEntry),
    Legacy(String),
}

impl<'de> serde::Deserialize<'de> for ProfileReconcileQueueEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match StoredProfileReconcileQueueEntry::deserialize(deserializer)? {
            StoredProfileReconcileQueueEntry::Current(entry) => Ok(Self {
                pubkey: entry.pubkey,
                expected_name: entry.expected_name,
                reconciled_relays: entry.reconciled_relays,
            }),
            StoredProfileReconcileQueueEntry::Legacy(pubkey) => Ok(Self {
                pubkey,
                expected_name: default_profile_reconcile_name(),
                reconciled_relays: Vec::new(),
            }),
        }
    }
}

pub(crate) fn profile_reconcile_queue_path(agent_store_path: &Path) -> std::path::PathBuf {
    agent_store_path.with_file_name("profile-reconcile-pending.json")
}

pub(crate) const PROFILE_RECONCILE_QUEUE_MAX_BYTES: usize = 1024 * 1024;

pub(crate) fn read_profile_reconcile_queue(
    path: &Path,
) -> Result<Vec<ProfileReconcileQueueEntry>, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("failed to inspect profile reconcile queue: {error}"))?;
    if metadata.len() > PROFILE_RECONCILE_QUEUE_MAX_BYTES as u64 {
        return Err("profile reconcile queue exceeds its size limit".to_string());
    }
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read profile reconcile queue: {error}"))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("failed to parse profile reconcile queue: {error}"))
}

pub(crate) fn write_profile_reconcile_queue(
    path: &Path,
    entries: &[ProfileReconcileQueueEntry],
) -> Result<(), String> {
    if entries.is_empty() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "failed to remove empty profile reconcile queue {}: {error}",
                path.display()
            )),
        };
    }
    let bytes = serde_json::to_vec_pretty(entries)
        .map_err(|error| format!("failed to serialize profile reconcile queue: {error}"))?;
    if bytes.len() > PROFILE_RECONCILE_QUEUE_MAX_BYTES {
        return Err("profile reconcile queue exceeds its size limit".to_string());
    }
    crate::managed_agents::atomic_write_json_restricted(path, &bytes)
}

pub(crate) fn profile_reconcile_relay_key(relay_url: &str) -> Result<String, String> {
    kuzz_core_pkg::relay::normalize_relay_url(relay_url)
        .map_err(|error| format!("invalid profile reconcile relay: {error}"))
}

#[cfg(test)]
pub(crate) fn profile_reconcile_is_pending(
    entries: &[ProfileReconcileQueueEntry],
    pubkey: &str,
    relay_key: &str,
) -> bool {
    entries.iter().any(|entry| {
        entry.pubkey == pubkey
            && !entry
                .reconciled_relays
                .iter()
                .any(|relay| relay == relay_key)
    })
}

pub(crate) fn record_profile_reconciled(
    entries: &mut [ProfileReconcileQueueEntry],
    pubkey: &str,
    relay_key: String,
) {
    if let Some(entry) = entries.iter_mut().find(|entry| entry.pubkey == pubkey) {
        if !entry.reconciled_relays.contains(&relay_key) {
            entry.reconciled_relays.push(relay_key);
            entry.reconciled_relays.sort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_profile_reconcile_queue_remains_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile-reconcile-pending.json");
        std::fs::write(&path, r#"["pollen-pubkey"]"#).unwrap();

        assert_eq!(
            read_profile_reconcile_queue(&path).unwrap(),
            vec![ProfileReconcileQueueEntry {
                expected_name: crate::managed_agents::POLLEN_DISPLAY_NAME.to_string(),
                pubkey: "pollen-pubkey".to_string(),
                reconciled_relays: Vec::new(),
            }]
        );
    }

    #[test]
    fn profile_reconcile_queue_tracks_each_relay_without_dropping_other_communities() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile-reconcile-pending.json");
        let relay_a = profile_reconcile_relay_key("WSS://A.EXAMPLE:443/").unwrap();
        let relay_b = profile_reconcile_relay_key("wss://b.example").unwrap();
        let mut entries = vec![ProfileReconcileQueueEntry {
            pubkey: "pollen-pubkey".to_string(),
            expected_name: crate::managed_agents::POLLEN_DISPLAY_NAME.to_string(),
            reconciled_relays: Vec::new(),
        }];
        assert!(profile_reconcile_is_pending(
            &entries,
            "pollen-pubkey",
            &relay_a
        ));
        record_profile_reconciled(&mut entries, "pollen-pubkey", relay_a.clone());
        assert!(!profile_reconcile_is_pending(
            &entries,
            "pollen-pubkey",
            &relay_a
        ));
        assert!(profile_reconcile_is_pending(
            &entries,
            "pollen-pubkey",
            &relay_b
        ));

        write_profile_reconcile_queue(&path, &entries).unwrap();
        assert_eq!(read_profile_reconcile_queue(&path).unwrap(), entries);
        assert_eq!(
            profile_reconcile_relay_key("wss://a.example").unwrap(),
            profile_reconcile_relay_key("WSS://A.EXAMPLE:443/").unwrap(),
            "equivalent relay spellings must share one completion key"
        );
    }

    #[test]
    fn empty_profile_reconcile_queue_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profile-reconcile-pending.json");
        write_profile_reconcile_queue(
            &path,
            &[ProfileReconcileQueueEntry {
                expected_name: crate::managed_agents::POLLEN_DISPLAY_NAME.to_string(),
                pubkey: "pollen-pubkey".to_string(),
                reconciled_relays: Vec::new(),
            }],
        )
        .unwrap();
        assert!(path.exists());

        write_profile_reconcile_queue(&path, &[]).unwrap();

        assert!(!path.exists());
    }
}
