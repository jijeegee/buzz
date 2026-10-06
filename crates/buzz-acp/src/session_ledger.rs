//! Durable scope → provider-session ledger for ACP session resume.
//!
//! Provider sessions (the adapter's own transcript, e.g. Claude Code's files
//! under `~/.claude/projects`) survive a harness restart, but the harness's
//! scope → session map lives only in worker memory. This ledger persists that
//! map so the first turn after a restart can reattach to the same provider
//! session with ACP `session/resume` (or `session/load`) instead of starting
//! over from relay context.
//!
//! The ledger is best-effort: relay history stays the source of truth. Any
//! mismatch or failure (different adapter, different cwd, unsupported resume,
//! deleted transcript, unreadable file) falls back to a fresh `session/new`
//! whose context is rebuilt from the relay, exactly as without a ledger.
//!
//! One file per (agent, relay). A harness process serves exactly one agent on
//! one relay, so each file has a single writer and needs no cross-process lock.
//! Writes replace the whole file atomically (temp file + rename).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::scope::SessionScope;

/// On-disk schema version. A file with any other version is set aside, not
/// interpreted.
const LEDGER_VERSION: u32 = 1;
/// Entries unused for this long are dropped: their transcripts are likely
/// gone and their relay context is better rebuilt fresh.
const ENTRY_TTL: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// Hard cap on entries; the least recently used are dropped first.
const MAX_ENTRIES: usize = 256;

/// One persisted scope → provider-session mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Provider session id returned by `session/new`.
    pub session_id: String,
    /// Normalized agent command identity (adapter). A different adapter can
    /// never resume this session.
    pub agent_identity: String,
    /// Working directory the session was created in. Adapters key transcripts
    /// by cwd, so a different cwd cannot resume it.
    pub cwd: String,
    /// Unix seconds when the session was created.
    pub created_at: u64,
    /// Unix seconds of the last successful use (create, resume, or turn).
    pub last_used_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LedgerFile {
    version: u32,
    /// Keyed by [`scope_key`]; a `BTreeMap` keeps the file stable and diffable.
    entries: BTreeMap<String, LedgerEntry>,
}

/// Durable ledger shared by every worker of one harness process.
pub struct SessionLedger {
    path: PathBuf,
    file: Mutex<LedgerFile>,
}

impl std::fmt::Debug for SessionLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLedger")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Stable string key for a scope. DM/channel `Conversation` scopes are keyed
/// too, so a ledger is policy-agnostic even though only the main-and-threads
/// policy enables it today.
pub(crate) fn scope_key(scope: &SessionScope) -> String {
    match scope {
        SessionScope::Conversation { channel_id } => format!("conversation:{channel_id}"),
        SessionScope::Main { channel_id } => format!("main:{channel_id}"),
        SessionScope::Thread {
            channel_id,
            root_event_id,
        } => format!("thread:{channel_id}:{root_event_id}"),
    }
}

/// Ledger file path for one agent on one relay under `state_dir`:
/// `<state_dir>/<agent_pubkey_hex>/sessions-<sha256(relay_url)[..16]>.json`.
pub fn ledger_path(state_dir: &Path, agent_pubkey_hex: &str, relay_url: &str) -> PathBuf {
    let digest = Sha256::digest(relay_url.trim_end_matches('/').as_bytes());
    let relay_tag = hex::encode(&digest[..8]);
    state_dir
        .join(agent_pubkey_hex.to_ascii_lowercase())
        .join(format!("sessions-{relay_tag}.json"))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl SessionLedger {
    /// Open (or start) the ledger at `path`. Never fails: an unreadable,
    /// corrupt, or other-version file is moved aside to `<name>.bak-<unix>` and
    /// the ledger starts empty, so a bad file can only cost a resume, never a
    /// turn.
    pub fn open(path: PathBuf) -> Self {
        let file = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<LedgerFile>(&bytes) {
                Ok(file) if file.version == LEDGER_VERSION => file,
                Ok(file) => {
                    tracing::warn!(
                        target: "acp::ledger",
                        path = %path.display(),
                        version = file.version,
                        "unsupported session ledger version — setting it aside"
                    );
                    set_aside(&path);
                    LedgerFile::default()
                }
                Err(error) => {
                    tracing::warn!(
                        target: "acp::ledger",
                        path = %path.display(),
                        "unreadable session ledger — setting it aside: {error}"
                    );
                    set_aside(&path);
                    LedgerFile::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => LedgerFile::default(),
            Err(error) => {
                tracing::warn!(
                    target: "acp::ledger",
                    path = %path.display(),
                    "cannot read session ledger — starting empty: {error}"
                );
                LedgerFile::default()
            }
        };
        let ledger = Self {
            path,
            file: Mutex::new(LedgerFile {
                version: LEDGER_VERSION,
                entries: file.entries,
            }),
        };
        ledger.prune_and_persist(now_unix());
        ledger
    }

    /// The persisted entry for `scope`, if it can be resumed by an adapter with
    /// `agent_identity` running in `cwd`. A mismatched entry is not returned —
    /// it is left for [`record`](Self::record) to overwrite.
    pub fn lookup(
        &self,
        scope: &SessionScope,
        agent_identity: &str,
        cwd: &str,
    ) -> Option<LedgerEntry> {
        let file = self.file.lock().ok()?;
        file.entries
            .get(&scope_key(scope))
            .filter(|entry| entry.agent_identity == agent_identity && entry.cwd == cwd)
            .cloned()
    }

    /// Record that `scope` is served by provider session `session_id`, replacing
    /// any previous mapping, and persist.
    pub fn record(&self, scope: &SessionScope, session_id: &str, agent_identity: &str, cwd: &str) {
        let now = now_unix();
        let key = scope_key(scope);
        {
            let Ok(mut file) = self.file.lock() else {
                return;
            };
            let created_at = file
                .entries
                .get(&key)
                .filter(|entry| entry.session_id == session_id)
                .map_or(now, |entry| entry.created_at);
            file.entries.insert(
                key,
                LedgerEntry {
                    session_id: session_id.to_string(),
                    agent_identity: agent_identity.to_string(),
                    cwd: cwd.to_string(),
                    created_at,
                    last_used_at: now,
                },
            );
            prune(&mut file, now);
        }
        self.persist();
    }

    /// Forget `scope`'s mapping (resume failed, operator `!rotate`) and persist.
    pub fn remove(&self, scope: &SessionScope) {
        let removed = self
            .file
            .lock()
            .map(|mut file| file.entries.remove(&scope_key(scope)).is_some())
            .unwrap_or(false);
        if removed {
            self.persist();
        }
    }

    /// Forget every mapping in `channel_id` (the agent left the channel).
    pub fn remove_channel(&self, channel_id: Uuid) {
        let channel = channel_id.to_string();
        let removed = self
            .file
            .lock()
            .map(|mut file| {
                let before = file.entries.len();
                file.entries
                    .retain(|key, _| key.split(':').nth(1) != Some(channel.as_str()));
                before != file.entries.len()
            })
            .unwrap_or(false);
        if removed {
            self.persist();
        }
    }

    fn prune_and_persist(&self, now: u64) {
        let pruned = self
            .file
            .lock()
            .map(|mut file| prune(&mut file, now))
            .unwrap_or(false);
        if pruned {
            self.persist();
        }
    }

    /// Write the whole ledger atomically. Failures are logged and leave the
    /// previous file intact; the in-memory map stays authoritative for this
    /// process, and the next successful write catches the file up.
    fn persist(&self) {
        let bytes = match self.file.lock() {
            Ok(file) => match serde_json::to_vec_pretty(&*file) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!(target: "acp::ledger", "cannot encode session ledger: {error}");
                    return;
                }
            },
            Err(_) => return,
        };
        if let Err(error) = write_atomically(&self.path, &bytes) {
            tracing::warn!(
                target: "acp::ledger",
                path = %self.path.display(),
                "cannot persist session ledger: {error}"
            );
        }
    }
}

/// Drop entries past [`ENTRY_TTL`], then the least recently used beyond
/// [`MAX_ENTRIES`]. Returns whether anything was removed.
fn prune(file: &mut LedgerFile, now: u64) -> bool {
    let before = file.entries.len();
    let ttl = ENTRY_TTL.as_secs();
    file.entries
        .retain(|_, entry| now.saturating_sub(entry.last_used_at) <= ttl);
    if file.entries.len() > MAX_ENTRIES {
        let mut by_age: Vec<(u64, String)> = file
            .entries
            .iter()
            .map(|(key, entry)| (entry.last_used_at, key.clone()))
            .collect();
        by_age.sort();
        let excess = file.entries.len() - MAX_ENTRIES;
        for (_, key) in by_age.into_iter().take(excess) {
            file.entries.remove(&key);
        }
    }
    before != file.entries.len()
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("ledger path has no parent directory"))?;
    std::fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn set_aside(path: &Path) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(format!(".bak-{}", now_unix()));
    if let Err(error) = std::fs::rename(path, &aside) {
        tracing::warn!(
            target: "acp::ledger",
            path = %path.display(),
            "cannot set aside session ledger: {error}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(channel_id: Uuid, root: char) -> SessionScope {
        SessionScope::Thread {
            channel_id,
            root_event_id: root.to_string().repeat(64),
        }
    }

    #[test]
    fn records_survive_reopen_and_respect_identity_and_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = ledger_path(dir.path(), "AB".repeat(32).as_str(), "wss://relay.example/");
        let ch = Uuid::new_v4();
        let main = SessionScope::Main { channel_id: ch };
        let t = thread(ch, 'a');

        let ledger = SessionLedger::open(path.clone());
        ledger.record(&main, "sess-main", "claude-agent-acp", "/home/a/.buzz");
        ledger.record(&t, "sess-thread", "claude-agent-acp", "/home/a/.buzz");
        drop(ledger);

        let reopened = SessionLedger::open(path.clone());
        let entry = reopened
            .lookup(&main, "claude-agent-acp", "/home/a/.buzz")
            .expect("main entry");
        assert_eq!(entry.session_id, "sess-main");
        assert_eq!(
            reopened
                .lookup(&t, "claude-agent-acp", "/home/a/.buzz")
                .map(|e| e.session_id),
            Some("sess-thread".into())
        );
        // A different adapter or cwd can never resume the session.
        assert!(reopened
            .lookup(&main, "codex-acp", "/home/a/.buzz")
            .is_none());
        assert!(reopened
            .lookup(&main, "claude-agent-acp", "/elsewhere")
            .is_none());
        // Path is per agent (lowercased) and per relay (trailing slash ignored).
        assert!(path.starts_with(dir.path().join("ab".repeat(32))));
        assert_eq!(
            path,
            ledger_path(dir.path(), &"ab".repeat(32), "wss://relay.example")
        );
        assert_ne!(
            path,
            ledger_path(dir.path(), &"ab".repeat(32), "wss://other.example")
        );
    }

    #[test]
    fn record_replaces_and_remove_forgets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a").join("sessions.json");
        let ch = Uuid::new_v4();
        let main = SessionScope::Main { channel_id: ch };
        let ledger = SessionLedger::open(path.clone());
        ledger.record(&main, "old", "id", "/cwd");
        ledger.record(&main, "new", "id", "/cwd");
        assert_eq!(
            ledger.lookup(&main, "id", "/cwd").map(|e| e.session_id),
            Some("new".into())
        );
        ledger.remove(&main);
        assert!(ledger.lookup(&main, "id", "/cwd").is_none());
        assert!(SessionLedger::open(path)
            .lookup(&main, "id", "/cwd")
            .is_none());
    }

    #[test]
    fn remove_channel_drops_only_that_channel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sessions.json");
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let ledger = SessionLedger::open(path);
        ledger.record(&SessionScope::Main { channel_id: a }, "1", "id", "/c");
        ledger.record(&thread(a, 'a'), "2", "id", "/c");
        ledger.record(&SessionScope::Main { channel_id: b }, "3", "id", "/c");
        ledger.remove_channel(a);
        assert!(ledger
            .lookup(&SessionScope::Main { channel_id: a }, "id", "/c")
            .is_none());
        assert!(ledger.lookup(&thread(a, 'a'), "id", "/c").is_none());
        assert!(ledger
            .lookup(&SessionScope::Main { channel_id: b }, "id", "/c")
            .is_some());
    }

    #[test]
    fn corrupt_or_foreign_version_file_is_set_aside_not_trusted() {
        let dir = tempfile::tempdir().expect("tempdir");
        for contents in [&b"not json"[..], br#"{"version":99,"entries":{}}"#] {
            let path = dir.path().join(format!("s-{}.json", contents.len()));
            std::fs::write(&path, contents).expect("write");
            let ledger = SessionLedger::open(path.clone());
            assert!(ledger
                .lookup(
                    &SessionScope::Main {
                        channel_id: Uuid::nil()
                    },
                    "id",
                    "/c"
                )
                .is_none());
            let set_aside = std::fs::read_dir(dir.path())
                .expect("read dir")
                .filter_map(Result::ok)
                .any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("s-{}.json.bak-", contents.len()))
                });
            assert!(set_aside, "bad ledger must be preserved for inspection");
        }
    }

    #[test]
    fn stale_and_excess_entries_are_pruned_on_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sessions.json");
        let now = now_unix();
        let mut entries = BTreeMap::new();
        let entry = |last_used_at| LedgerEntry {
            session_id: "s".into(),
            agent_identity: "id".into(),
            cwd: "/c".into(),
            created_at: last_used_at,
            last_used_at,
        };
        entries.insert(
            "main:stale".to_string(),
            entry(now - ENTRY_TTL.as_secs() - 60),
        );
        for i in 0..(MAX_ENTRIES + 5) {
            entries.insert(format!("main:{i:04}"), entry(now - 1000 + i as u64));
        }
        let file = LedgerFile {
            version: LEDGER_VERSION,
            entries,
        };
        std::fs::write(&path, serde_json::to_vec(&file).expect("encode")).expect("write");

        let ledger = SessionLedger::open(path.clone());
        let kept = &ledger.file.lock().expect("lock").entries;
        assert_eq!(kept.len(), MAX_ENTRIES);
        assert!(!kept.contains_key("main:stale"));
        // The least recently used fall off first.
        assert!(!kept.contains_key("main:0000"));
        assert!(kept.contains_key(&format!("main:{:04}", MAX_ENTRIES + 4)));
        // And the pruned ledger was written back.
        let on_disk: LedgerFile =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("decode");
        assert_eq!(on_disk.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sessions.json");
        let ledger = SessionLedger::open(path.clone());
        for i in 0..5 {
            ledger.record(
                &SessionScope::Main {
                    channel_id: Uuid::new_v4(),
                },
                &i.to_string(),
                "id",
                "/c",
            );
        }
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["sessions.json"]);
    }
}
