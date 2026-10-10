//! Durable "signed out on this computer" markers.
//!
//! Signing out of a Google-held account keeps the local key, agents and data
//! (nothing is deleted), but the app must stop reading or sending as that
//! account until the user proves the same Google account again. The marker is
//! a small JSON file in the app data directory, deliberately outside the OS
//! keyring: it holds no secret, and a signed-out device must stay signed out
//! even when the keyring is unavailable.
//!
//! Entries are keyed by relay origin and remember the identity (signing pubkey
//! for key-backup accounts, principal for legacy token accounts) that a later
//! Google sign-in must resume. A different identity is refused and the marker
//! stays. An entry also lists the local agents the user chose to stop when
//! signing out; launch restore and auto-start fan-out skip them until the
//! marker is cleared by signing in again (their configs are untouched).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use nostr::PublicKey;
use serde::{Deserialize, Serialize};

/// File name of the marker inside the app data directory.
pub(crate) const FILE_NAME: &str = "signed-out.json";

/// Why user-identity work is refused while signed out.
pub(crate) const SIGNED_OUT_MESSAGE: &str =
    "You're signed out on this computer. Sign in with Google to continue.";

/// Shown when a Google sign-in would resume a different account.
pub(crate) const ACCOUNT_MISMATCH_MESSAGE: &str =
    "This Google account is linked to a different Buzz account. Try a different Google account.";

const VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
struct MarkerFile {
    version: u8,
    origins: Vec<MarkerEntry>,
}

#[derive(Serialize, Deserialize)]
struct MarkerEntry {
    origin: String,
    /// Hex pubkey of the identity to resume; `None` when it was unknown at
    /// sign-out (a legacy token session that had already ended).
    identity: Option<String>,
    /// Agent pubkeys stopped by this sign-out; kept from auto-starting.
    #[serde(default)]
    stopped_agents: Vec<String>,
}

/// One signed-out origin.
#[derive(Clone, Default)]
struct Marker {
    identity: Option<PublicKey>,
    stopped_agents: Vec<String>,
}

#[derive(Default)]
struct Markers {
    path: Option<PathBuf>,
    origins: HashMap<String, Marker>,
    /// The file exists but could not be read. Fail closed: every origin counts
    /// as signed out (identity unknown) until a Google sign-in rewrites it.
    unreadable: bool,
}

/// In-memory mirror of the marker file; every write goes to disk first.
#[derive(Default)]
pub(crate) struct SignedOutMarkers {
    inner: Mutex<Markers>,
}

impl SignedOutMarkers {
    /// Load the markers from `data_dir`. Missing file: nobody is signed out.
    pub(crate) fn load(&self, data_dir: &Path) {
        let path = data_dir.join(FILE_NAME);
        let (origins, unreadable) = match read(&path) {
            Ok(origins) => (origins, false),
            Err(error) => {
                eprintln!(
                    "buzz-desktop: auth: sign-out marker unreadable, staying signed out: {error}"
                );
                (HashMap::new(), true)
            }
        };
        *self.lock() = Markers {
            path: Some(path),
            origins,
            unreadable,
        };
    }

    /// `Some(identity)` while `origin` is signed out on this device (`identity`
    /// is `None` when unknown), `None` when it is not.
    pub(crate) fn get(&self, origin: &str) -> Option<Option<PublicKey>> {
        let markers = self.lock();
        match markers.origins.get(origin) {
            Some(marker) => Some(marker.identity),
            None => markers.unreadable.then_some(None),
        }
    }

    /// Whether a sign-out stopped `agent_pubkey`, so it must not auto-start.
    pub(crate) fn suppresses_agent(&self, agent_pubkey: &str) -> bool {
        self.lock().origins.values().any(|marker| {
            marker
                .stopped_agents
                .iter()
                .any(|stopped| stopped.eq_ignore_ascii_case(agent_pubkey))
        })
    }

    /// Whether `origin` is signed out on this device.
    pub(crate) fn is_signed_out(&self, origin: &str) -> bool {
        self.get(origin).is_some()
    }

    /// Durably mark `origin` signed out, remembering the identity to resume
    /// and (added to any already recorded) the agents this sign-out stops.
    pub(crate) fn mark(
        &self,
        origin: &str,
        identity: Option<PublicKey>,
        stopped_agents: &[String],
    ) -> Result<(), String> {
        let mut markers = self.lock();
        let mut next = markers.origins.clone();
        let mut stopped = next
            .remove(origin)
            .map(|marker| marker.stopped_agents)
            .unwrap_or_default();
        for agent in stopped_agents {
            let agent = agent.to_ascii_lowercase();
            if !stopped.contains(&agent) {
                stopped.push(agent);
            }
        }
        next.insert(
            origin.to_owned(),
            Marker {
                identity,
                stopped_agents: stopped,
            },
        );
        write(markers.path.as_deref(), &next)?;
        markers.origins = next;
        Ok(())
    }

    /// Durably clear `origin`'s marker after a matching Google sign-in. An
    /// unreadable file is replaced by the remaining known markers: the same
    /// Google account just proved this device's owner.
    pub(crate) fn clear(&self, origin: &str) -> Result<(), String> {
        let mut markers = self.lock();
        if !markers.unreadable && !markers.origins.contains_key(origin) {
            return Ok(());
        }
        let mut next = markers.origins.clone();
        next.remove(origin);
        write(markers.path.as_deref(), &next)?;
        markers.origins = next;
        markers.unreadable = false;
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Markers> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Require a resumed identity to be the one that signed out.
pub(crate) fn verify_resumed_identity(
    expected: Option<PublicKey>,
    actual: PublicKey,
) -> Result<(), String> {
    if expected.is_some_and(|expected| expected != actual) {
        return Err(ACCOUNT_MISMATCH_MESSAGE.into());
    }
    Ok(())
}

fn read(path: &Path) -> Result<HashMap<String, Marker>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(format!("read: {error}")),
    };
    let file: MarkerFile = serde_json::from_slice(&bytes).map_err(|e| format!("parse: {e}"))?;
    if file.version != VERSION {
        return Err(format!("unsupported version {}", file.version));
    }
    file.origins
        .into_iter()
        .map(|entry| {
            let identity = entry
                .identity
                .map(|hex| PublicKey::from_hex(&hex).map_err(|_| "invalid identity".to_owned()))
                .transpose()?;
            if entry
                .stopped_agents
                .iter()
                .any(|agent| agent.len() != 64 || !agent.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err("invalid stopped agent".to_owned());
            }
            Ok((
                entry.origin,
                Marker {
                    identity,
                    stopped_agents: entry.stopped_agents,
                },
            ))
        })
        .collect()
}

fn write(path: Option<&Path>, origins: &HashMap<String, Marker>) -> Result<(), String> {
    use atomic_write_file::AtomicWriteFile;

    const FAILED: &str = "Sign-out could not be saved on this computer. Try again.";
    let path = path.ok_or(FAILED)?;
    let mut entries: Vec<MarkerEntry> = origins
        .iter()
        .map(|(origin, marker)| MarkerEntry {
            origin: origin.clone(),
            identity: marker.identity.map(|key| key.to_hex()),
            stopped_agents: marker.stopped_agents.clone(),
        })
        .collect();
    entries.sort_by(|a, b| a.origin.cmp(&b.origin));
    let json = serde_json::to_vec_pretty(&MarkerFile {
        version: VERSION,
        origins: entries,
    })
    .map_err(|_| FAILED)?;
    let result = (|| {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = AtomicWriteFile::open(path)?;
        file.write_all(&json)?;
        file.commit()
    })();
    result.map_err(|error| {
        eprintln!("buzz-desktop: auth: could not write sign-out marker: {error}");
        FAILED.to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_survive_reload_and_clear_only_their_origin() {
        let dir = tempfile::tempdir().unwrap();
        let identity = nostr::Keys::generate().public_key();
        let markers = SignedOutMarkers::default();
        markers.load(dir.path());
        assert!(!markers.is_signed_out("https://a.test"));
        markers.mark("https://a.test", Some(identity), &[]).unwrap();
        markers.mark("https://b.test", None, &[]).unwrap();

        let restarted = SignedOutMarkers::default();
        restarted.load(dir.path());
        assert_eq!(restarted.get("https://a.test"), Some(Some(identity)));
        assert_eq!(restarted.get("https://b.test"), Some(None));
        assert_eq!(restarted.get("https://c.test"), None);

        restarted.clear("https://a.test").unwrap();
        let again = SignedOutMarkers::default();
        again.load(dir.path());
        assert!(!again.is_signed_out("https://a.test"));
        assert!(again.is_signed_out("https://b.test"));
    }

    #[test]
    fn an_unreadable_marker_fails_closed_until_a_sign_in_rewrites_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), b"{not json").unwrap();
        let markers = SignedOutMarkers::default();
        markers.load(dir.path());
        assert_eq!(markers.get("https://any.test"), Some(None));
        markers.clear("https://any.test").unwrap();
        assert!(!markers.is_signed_out("https://any.test"));
        let restarted = SignedOutMarkers::default();
        restarted.load(dir.path());
        assert!(!restarted.is_signed_out("https://other.test"));
    }

    #[test]
    fn marking_without_a_data_dir_fails_instead_of_pretending() {
        let markers = SignedOutMarkers::default();
        assert!(markers.mark("https://a.test", None, &[]).is_err());
        assert!(!markers.is_signed_out("https://a.test"));
    }

    #[test]
    fn stopped_agents_stay_suppressed_until_their_origin_signs_in() {
        let dir = tempfile::tempdir().unwrap();
        let agent = "AB".repeat(32);
        let markers = SignedOutMarkers::default();
        markers.load(dir.path());
        assert!(!markers.suppresses_agent(&agent));
        markers
            .mark("https://a.test", None, std::slice::from_ref(&agent))
            .unwrap();
        // A repeated sign-out without stopping keeps the earlier choice.
        markers.mark("https://a.test", None, &[]).unwrap();
        let restarted = SignedOutMarkers::default();
        restarted.load(dir.path());
        assert!(restarted.suppresses_agent(&agent.to_ascii_lowercase()));
        restarted.clear("https://a.test").unwrap();
        assert!(!restarted.suppresses_agent(&agent));
    }

    #[test]
    fn resumed_identity_must_match_when_known() {
        let a = nostr::Keys::generate().public_key();
        let b = nostr::Keys::generate().public_key();
        assert!(verify_resumed_identity(Some(a), a).is_ok());
        assert_eq!(
            verify_resumed_identity(Some(a), b).unwrap_err(),
            ACCOUNT_MISMATCH_MESSAGE
        );
        assert!(verify_resumed_identity(None, b).is_ok());
    }
}
