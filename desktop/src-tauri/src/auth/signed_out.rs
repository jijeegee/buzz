//! Durable "signed out on this computer" markers.
//!
//! Signing out of a Google-held account keeps the local key, agents and data
//! (nothing is deleted), but the app must stop reading or sending as that
//! account until the user proves the same Google account again. The marker is
//! a small JSON file in the app data directory, deliberately outside the OS
//! keyring: it holds no secret, and a signed-out device must stay signed out
//! even when the keyring is unavailable.
//!
//! Entries are keyed by (relay origin, account): the signing pubkey of a
//! key-backup account, or no account for a legacy token account (which then
//! fences the whole origin). The current community counts as signed out while
//! its active account has an entry. Signing in with any account is allowed; it
//! clears only that account's entry, so another account that signed out here
//! stays signed out. An entry also lists the local agents the user chose to
//! stop when signing out; launch restore and auto-start fan-out skip them until
//! that same account signs in again (their configs are untouched).

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

const VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
struct MarkerFile {
    version: u8,
    origins: Vec<MarkerEntry>,
}

#[derive(Serialize, Deserialize)]
struct MarkerEntry {
    origin: String,
    /// Hex pubkey of the account that signed out; `None` for a legacy token
    /// account (the marker then covers the whole origin).
    identity: Option<String>,
    /// Agent pubkeys stopped by this sign-out; kept from auto-starting.
    #[serde(default)]
    stopped_agents: Vec<String>,
}

/// One signed-out (origin, account).
#[derive(Clone, Default)]
struct Marker {
    stopped_agents: Vec<String>,
}

/// (origin, account) — `None` account covers the whole origin.
type MarkerKey = (String, Option<PublicKey>);

#[derive(Default)]
struct Markers {
    path: Option<PathBuf>,
    origins: HashMap<MarkerKey, Marker>,
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

    /// Whether `account` (the active account of `origin`, if known) is
    /// signed out there. An unreadable file fails closed.
    pub(crate) fn is_signed_out(&self, origin: &str, account: Option<PublicKey>) -> bool {
        let markers = self.lock();
        markers.unreadable
            || markers.origins.contains_key(&(origin.to_owned(), None))
            || account.is_some_and(|account| {
                markers
                    .origins
                    .contains_key(&(origin.to_owned(), Some(account)))
            })
    }

    /// Whether a readable marker names `account` on `origin` (or covers the
    /// whole origin). Unlike [`Self::is_signed_out`], an unreadable file does
    /// not count: the agent start gate then decides from the account list
    /// instead of refusing every agent.
    pub(crate) fn is_marked(&self, origin: &str, account: &PublicKey) -> bool {
        let markers = self.lock();
        markers.origins.contains_key(&(origin.to_owned(), None))
            || markers
                .origins
                .contains_key(&(origin.to_owned(), Some(*account)))
    }

    /// Whether any marker covers `origin`: a legacy token account, whose
    /// version-1 marker may name its principal rather than no account.
    pub(crate) fn is_signed_out_any(&self, origin: &str) -> bool {
        let markers = self.lock();
        markers.unreadable || markers.origins.keys().any(|(o, _)| o == origin)
    }

    /// The agents `account`'s sign-out of `origin` stopped (to start again
    /// when it signs back in).
    pub(crate) fn stopped_agents(&self, origin: &str, account: Option<PublicKey>) -> Vec<String> {
        self.lock()
            .origins
            .get(&(origin.to_owned(), account))
            .map(|marker| marker.stopped_agents.clone())
            .unwrap_or_default()
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

    /// Durably mark `account` signed out of `origin` and (added to any already
    /// recorded) the agents this sign-out stops.
    pub(crate) fn mark(
        &self,
        origin: &str,
        account: Option<PublicKey>,
        stopped_agents: &[String],
    ) -> Result<(), String> {
        let mut markers = self.lock();
        let mut next = markers.origins.clone();
        let key = (origin.to_owned(), account);
        let mut stopped = next
            .remove(&key)
            .map(|marker| marker.stopped_agents)
            .unwrap_or_default();
        for agent in stopped_agents {
            let agent = agent.to_ascii_lowercase();
            if !stopped.contains(&agent) {
                stopped.push(agent);
            }
        }
        next.insert(
            key,
            Marker {
                stopped_agents: stopped,
            },
        );
        write(markers.path.as_deref(), &next)?;
        markers.origins = next;
        Ok(())
    }

    /// Durably clear `account`'s marker on `origin` (and an origin-wide token
    /// marker) after it signs in. Other accounts' markers stay.
    ///
    /// Known limitation: when the file was unreadable (fail-closed), the
    /// rewrite keeps no other entries, so their stopped agents are released.
    pub(crate) fn clear(&self, origin: &str, account: Option<PublicKey>) -> Result<(), String> {
        let mut markers = self.lock();
        let own = (origin.to_owned(), account);
        let wide = (origin.to_owned(), None);
        if !markers.unreadable
            && !markers.origins.contains_key(&own)
            && !markers.origins.contains_key(&wide)
        {
            return Ok(());
        }
        let mut next = markers.origins.clone();
        next.remove(&own);
        next.remove(&wide);
        write(markers.path.as_deref(), &next)?;
        markers.origins = next;
        markers.unreadable = false;
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Markers> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether the marker file in `data_dir` names `account` on `origin` (or
/// covers the whole origin), read straight from disk for the account-list
/// migration. An unreadable file names nobody, like [`SignedOutMarkers::is_marked`].
pub(crate) fn names_on_disk(data_dir: &Path, origin: &str, account: &PublicKey) -> bool {
    read(&data_dir.join(FILE_NAME)).is_ok_and(|origins| {
        origins.contains_key(&(origin.to_owned(), None))
            || origins.contains_key(&(origin.to_owned(), Some(*account)))
    })
}

fn read(path: &Path) -> Result<HashMap<MarkerKey, Marker>, String> {
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
                (entry.origin, identity),
                Marker {
                    stopped_agents: entry.stopped_agents,
                },
            ))
        })
        .collect()
}

fn write(path: Option<&Path>, origins: &HashMap<MarkerKey, Marker>) -> Result<(), String> {
    use atomic_write_file::AtomicWriteFile;

    const FAILED: &str = "Sign-out could not be saved on this computer. Try again.";
    let path = path.ok_or(FAILED)?;
    let mut entries: Vec<MarkerEntry> = origins
        .iter()
        .map(|((origin, account), marker)| MarkerEntry {
            origin: origin.clone(),
            identity: account.map(|key| key.to_hex()),
            stopped_agents: marker.stopped_agents.clone(),
        })
        .collect();
    entries.sort_by(|a, b| (&a.origin, &a.identity).cmp(&(&b.origin, &b.identity)));
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
    fn markers_are_per_account_and_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let a = nostr::Keys::generate().public_key();
        let b = nostr::Keys::generate().public_key();
        let markers = SignedOutMarkers::default();
        markers.load(dir.path());
        assert!(!markers.is_signed_out("https://a.test", Some(a)));
        markers.mark("https://a.test", Some(a), &[]).unwrap();
        markers.mark("https://b.test", None, &[]).unwrap();

        let restarted = SignedOutMarkers::default();
        restarted.load(dir.path());
        assert!(restarted.is_signed_out("https://a.test", Some(a)));
        // Another account active on the same origin is not signed out.
        assert!(!restarted.is_signed_out("https://a.test", Some(b)));
        // A token-account marker covers the whole origin.
        assert!(restarted.is_signed_out("https://b.test", Some(b)));
        assert!(!restarted.is_signed_out("https://c.test", Some(a)));

        // B signing in on a.test leaves A signed out.
        restarted.clear("https://a.test", Some(b)).unwrap();
        assert!(restarted.is_signed_out("https://a.test", Some(a)));
        restarted.clear("https://a.test", Some(a)).unwrap();
        let again = SignedOutMarkers::default();
        again.load(dir.path());
        assert!(!again.is_signed_out("https://a.test", Some(a)));
        assert!(again.is_signed_out("https://b.test", None));
    }

    #[test]
    fn version_one_files_with_an_identity_still_read() {
        let dir = tempfile::tempdir().unwrap();
        let a = nostr::Keys::generate().public_key();
        std::fs::write(
            dir.path().join(FILE_NAME),
            format!(
                r#"{{"version":1,"origins":[{{"origin":"https://a.test","identity":"{}","stopped_agents":[]}}]}}"#,
                a.to_hex()
            ),
        )
        .unwrap();
        let markers = SignedOutMarkers::default();
        markers.load(dir.path());
        assert!(markers.is_signed_out("https://a.test", Some(a)));
    }

    #[test]
    fn an_unreadable_marker_fails_closed_until_a_sign_in_rewrites_it() {
        let dir = tempfile::tempdir().unwrap();
        let a = nostr::Keys::generate().public_key();
        std::fs::write(dir.path().join(FILE_NAME), b"{not json").unwrap();
        let markers = SignedOutMarkers::default();
        markers.load(dir.path());
        assert!(markers.is_signed_out("https://any.test", Some(a)));
        markers.clear("https://any.test", Some(a)).unwrap();
        assert!(!markers.is_signed_out("https://any.test", Some(a)));
        let restarted = SignedOutMarkers::default();
        restarted.load(dir.path());
        assert!(!restarted.is_signed_out("https://other.test", Some(a)));
    }

    #[test]
    fn marking_without_a_data_dir_fails_instead_of_pretending() {
        let markers = SignedOutMarkers::default();
        assert!(markers.mark("https://a.test", None, &[]).is_err());
        assert!(!markers.is_signed_out("https://a.test", None));
    }

    #[test]
    fn stopped_agents_stay_suppressed_until_their_own_account_signs_in() {
        let dir = tempfile::tempdir().unwrap();
        let agent = "AB".repeat(32);
        let a = nostr::Keys::generate().public_key();
        let b = nostr::Keys::generate().public_key();
        let markers = SignedOutMarkers::default();
        markers.load(dir.path());
        assert!(!markers.suppresses_agent(&agent));
        markers
            .mark("https://a.test", Some(a), std::slice::from_ref(&agent))
            .unwrap();
        // A repeated sign-out without stopping keeps the earlier choice.
        markers.mark("https://a.test", Some(a), &[]).unwrap();
        let restarted = SignedOutMarkers::default();
        restarted.load(dir.path());
        assert!(restarted.suppresses_agent(&agent.to_ascii_lowercase()));
        // Another account signing in does not release A's stopped agents.
        restarted.clear("https://a.test", Some(b)).unwrap();
        assert!(restarted.suppresses_agent(&agent));
        restarted.clear("https://a.test", Some(a)).unwrap();
        assert!(!restarted.suppresses_agent(&agent));
    }
}
