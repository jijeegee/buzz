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

use std::collections::{BTreeMap, HashSet};
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
/// Minimum spacing between persisted `last_used_at` refreshes for one entry.
const TOUCH_INTERVAL: Duration = Duration::from_secs(60 * 60);

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
    /// Unix seconds of the last recorded use (create, reattach, or a turn;
    /// turn refreshes are coarsened to [`TOUCH_INTERVAL`]).
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
    state: Mutex<LedgerState>,
}

struct LedgerState {
    file: LedgerFile,
    /// Keys whose recorded session the next new session for that scope may
    /// reattach: every key loaded from disk at startup, plus a key re-armed by
    /// [`SessionLedger::allow_handoff`] when a busy owner's scope moves to
    /// another worker. Otherwise in-process rotation must create new sessions,
    /// never resume one that was deliberately rotated away.
    resumable: HashSet<String>,
}

impl std::fmt::Debug for SessionLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLedger")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Stable string key for a scope.
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
        let entries = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<LedgerFile>(&bytes) {
                Ok(file) if file.version == LEDGER_VERSION => file.entries,
                Ok(file) => {
                    tracing::warn!(
                        target: "acp::ledger",
                        path = %path.display(),
                        version = file.version,
                        "unsupported session ledger version — setting it aside"
                    );
                    set_aside(&path);
                    BTreeMap::new()
                }
                Err(error) => {
                    tracing::warn!(
                        target: "acp::ledger",
                        path = %path.display(),
                        "unreadable session ledger — setting it aside: {error}"
                    );
                    set_aside(&path);
                    BTreeMap::new()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => {
                tracing::warn!(
                    target: "acp::ledger",
                    path = %path.display(),
                    "cannot read session ledger — starting empty: {error}"
                );
                BTreeMap::new()
            }
        };
        let mut file = LedgerFile {
            version: LEDGER_VERSION,
            entries,
        };
        let pruned = prune(&mut file, now_unix());
        let resumable = file.entries.keys().cloned().collect();
        let ledger = Self {
            path,
            state: Mutex::new(LedgerState { file, resumable }),
        };
        if pruned {
            if let Ok(state) = ledger.state.lock() {
                ledger.persist_locked(&state.file);
            }
        }
        ledger
    }

    /// Claim the recorded provider session for `scope` for reattach. Returns it
    /// only when it was loaded from disk at startup or re-armed by
    /// [`allow_handoff`](Self::allow_handoff), has not been claimed since, and
    /// was created by an adapter with
    /// `agent_identity` running in `cwd`. Claiming consumes the chance even on
    /// a mismatch, so later sessions for the scope are always new ones.
    pub fn take_resumable(
        &self,
        scope: &SessionScope,
        agent_identity: &str,
        cwd: &str,
    ) -> Option<String> {
        let mut state = self.state.lock().ok()?;
        let key = scope_key(scope);
        if !state.resumable.remove(&key) {
            return None;
        }
        state
            .file
            .entries
            .get(&key)
            .filter(|entry| entry.agent_identity == agent_identity && entry.cwd == cwd)
            .map(|entry| entry.session_id.clone())
    }

    /// Let the next new session for `scope` reattach its recorded provider
    /// session. Called when a busy owner's thread is handed to an idle worker:
    /// the owner is mid-turn on a different scope, so this scope's session is
    /// idle and the new worker can continue it instead of starting over. The
    /// returning owner drops its stale copy (`AgentPool::return_agent`), so
    /// only one worker drives the session. No-op when nothing is recorded.
    pub fn allow_handoff(&self, scope: &SessionScope) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let key = scope_key(scope);
        if state.file.entries.contains_key(&key) {
            state.resumable.insert(key);
        }
    }

    /// Record that `scope` is served by provider session `session_id`, replacing
    /// any previous mapping, and persist.
    pub fn record(&self, scope: &SessionScope, session_id: &str, agent_identity: &str, cwd: &str) {
        let now = now_unix();
        let key = scope_key(scope);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.resumable.remove(&key);
        let created_at = state
            .file
            .entries
            .get(&key)
            .filter(|entry| entry.session_id == session_id)
            .map_or(now, |entry| entry.created_at);
        state.file.entries.insert(
            key,
            LedgerEntry {
                session_id: session_id.to_string(),
                agent_identity: agent_identity.to_string(),
                cwd: cwd.to_string(),
                created_at,
                last_used_at: now,
            },
        );
        prune(&mut state.file, now);
        self.persist_locked(&state.file);
    }

    /// Note a successful turn on `session_id` so a session in steady use is
    /// not pruned as stale. Persists at most once per [`TOUCH_INTERVAL`] per
    /// entry to keep per-turn cost negligible.
    pub fn touch(&self, scope: &SessionScope, session_id: &str) {
        let now = now_unix();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(entry) = state.file.entries.get_mut(&scope_key(scope)) else {
            return;
        };
        if entry.session_id != session_id
            || now.saturating_sub(entry.last_used_at) < TOUCH_INTERVAL.as_secs()
        {
            return;
        }
        entry.last_used_at = now;
        self.persist_locked(&state.file);
    }

    /// Forget `scope`'s mapping (operator `!rotate`) and persist.
    pub fn remove(&self, scope: &SessionScope) {
        let key = scope_key(scope);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.resumable.remove(&key);
        if state.file.entries.remove(&key).is_some() {
            self.persist_locked(&state.file);
        }
    }

    /// Forget every mapping in `channel_id` (the agent left the channel).
    pub fn remove_channel(&self, channel_id: Uuid) {
        let channel = channel_id.to_string();
        let in_channel = |key: &String| key.split(':').nth(1) == Some(channel.as_str());
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.resumable.retain(|key| !in_channel(key));
        let before = state.file.entries.len();
        state.file.entries.retain(|key, _| !in_channel(key));
        if before != state.file.entries.len() {
            self.persist_locked(&state.file);
        }
    }

    /// Write the whole ledger atomically while the caller holds the state
    /// lock, so concurrent writers cannot land an older snapshot last.
    /// Failures are logged and leave the previous file intact; the in-memory
    /// map stays authoritative for this process, and the next successful write
    /// catches the file up.
    fn persist_locked(&self, file: &LedgerFile) {
        let bytes = match serde_json::to_vec_pretty(file) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(target: "acp::ledger", "cannot encode session ledger: {error}");
                return;
            }
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

    fn main_scope(channel_id: Uuid) -> SessionScope {
        SessionScope::Main { channel_id }
    }

    #[test]
    fn recorded_sessions_are_resumable_once_after_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = ledger_path(dir.path(), &"AB".repeat(32), "wss://relay.example/");
        let ch = Uuid::new_v4();
        let main = main_scope(ch);
        let t = thread(ch, 'a');
        let cwd = "/home/a/.buzz";

        let ledger = SessionLedger::open(path.clone());
        ledger.record(&main, "sess-main", "claude-agent-acp", cwd);
        ledger.record(&t, "sess-thread", "claude-agent-acp", cwd);
        // Nothing recorded in this process is resumable in this process.
        assert!(ledger
            .take_resumable(&main, "claude-agent-acp", cwd)
            .is_none());
        drop(ledger);

        let reopened = SessionLedger::open(path.clone());
        assert_eq!(
            reopened.take_resumable(&main, "claude-agent-acp", cwd),
            Some("sess-main".into())
        );
        // Claimed once: a later rotation starts fresh.
        assert!(reopened
            .take_resumable(&main, "claude-agent-acp", cwd)
            .is_none());
        assert_eq!(
            reopened.take_resumable(&t, "claude-agent-acp", cwd),
            Some("sess-thread".into())
        );

        // A different adapter or cwd can never resume the session.
        assert!(SessionLedger::open(path.clone())
            .take_resumable(&main, "codex-acp", cwd)
            .is_none());
        assert!(SessionLedger::open(path.clone())
            .take_resumable(&main, "claude-agent-acp", "/elsewhere")
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
    fn handoff_rearms_the_recorded_session_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = ledger_path(dir.path(), &"ab".repeat(32), "wss://relay.example");
        let ch = Uuid::new_v4();
        let t = thread(ch, 'a');
        let other = thread(ch, 'b');
        let cwd = "/home/a/.buzz";

        let ledger = SessionLedger::open(path);
        ledger.record(&t, "sess-thread", "claude-agent-acp", cwd);
        assert!(ledger.take_resumable(&t, "claude-agent-acp", cwd).is_none());

        // A busy owner's thread moves to an idle worker: that worker's new
        // session reattaches the recorded one, exactly once.
        ledger.allow_handoff(&t);
        assert_eq!(
            ledger.take_resumable(&t, "claude-agent-acp", cwd),
            Some("sess-thread".into())
        );
        assert!(ledger.take_resumable(&t, "claude-agent-acp", cwd).is_none());

        // Nothing recorded (or rotated away): the handoff starts fresh.
        ledger.allow_handoff(&other);
        assert!(ledger
            .take_resumable(&other, "claude-agent-acp", cwd)
            .is_none());
        ledger.remove(&t);
        ledger.allow_handoff(&t);
        assert!(ledger.take_resumable(&t, "claude-agent-acp", cwd).is_none());
    }

    #[test]
    fn record_replaces_and_remove_forgets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a").join("sessions.json");
        let main = main_scope(Uuid::new_v4());
        let ledger = SessionLedger::open(path.clone());
        ledger.record(&main, "old", "id", "/cwd");
        ledger.record(&main, "new", "id", "/cwd");
        assert_eq!(
            SessionLedger::open(path.clone()).take_resumable(&main, "id", "/cwd"),
            Some("new".into())
        );
        ledger.remove(&main);
        assert!(SessionLedger::open(path)
            .take_resumable(&main, "id", "/cwd")
            .is_none());
    }

    #[test]
    fn remove_channel_drops_only_that_channel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sessions.json");
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let ledger = SessionLedger::open(path.clone());
        ledger.record(&main_scope(a), "1", "id", "/c");
        ledger.record(&thread(a, 'a'), "2", "id", "/c");
        ledger.record(&main_scope(b), "3", "id", "/c");
        ledger.remove_channel(a);
        let reopened = SessionLedger::open(path);
        assert!(reopened
            .take_resumable(&main_scope(a), "id", "/c")
            .is_none());
        assert!(reopened
            .take_resumable(&thread(a, 'a'), "id", "/c")
            .is_none());
        assert!(reopened
            .take_resumable(&main_scope(b), "id", "/c")
            .is_some());
    }

    #[test]
    fn corrupt_or_foreign_version_file_is_set_aside_not_trusted() {
        let dir = tempfile::tempdir().expect("tempdir");
        for contents in [&b"not json"[..], br#"{"version":99,"entries":{}}"#] {
            let name = format!("s-{}.json", contents.len());
            let path = dir.path().join(&name);
            std::fs::write(&path, contents).expect("write");
            let ledger = SessionLedger::open(path.clone());
            assert!(ledger
                .take_resumable(&main_scope(Uuid::nil()), "id", "/c")
                .is_none());
            let set_aside = std::fs::read_dir(dir.path())
                .expect("read dir")
                .filter_map(Result::ok)
                .any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("{name}.bak-"))
                });
            assert!(set_aside, "bad ledger must be preserved for inspection");
        }
    }

    #[test]
    fn stale_and_excess_entries_are_pruned_on_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sessions.json");
        let now = now_unix();
        let entry = |last_used_at| LedgerEntry {
            session_id: "s".into(),
            agent_identity: "id".into(),
            cwd: "/c".into(),
            created_at: last_used_at,
            last_used_at,
        };
        let mut entries = BTreeMap::new();
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
        {
            let state = ledger.state.lock().expect("lock");
            let kept = &state.file.entries;
            assert_eq!(kept.len(), MAX_ENTRIES);
            assert!(!kept.contains_key("main:stale"));
            // The least recently used fall off first.
            assert!(!kept.contains_key("main:0000"));
            assert!(kept.contains_key(&format!("main:{:04}", MAX_ENTRIES + 4)));
        }
        // And the pruned ledger was written back.
        let on_disk: LedgerFile =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("decode");
        assert_eq!(on_disk.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn touch_refreshes_only_the_current_session_and_is_coarse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let main = main_scope(Uuid::new_v4());
        let ledger = SessionLedger::open(dir.path().join("sessions.json"));
        ledger.record(&main, "s", "id", "/c");
        let key = scope_key(&main);
        let last_used = |ledger: &SessionLedger| {
            ledger.state.lock().expect("lock").file.entries[&key].last_used_at
        };
        let stale = now_unix() - TOUCH_INTERVAL.as_secs() - 1;
        ledger
            .state
            .lock()
            .expect("lock")
            .file
            .entries
            .get_mut(&key)
            .expect("entry")
            .last_used_at = stale;

        ledger.touch(&main, "other-session");
        assert_eq!(
            last_used(&ledger),
            stale,
            "a replaced session never refreshes"
        );
        ledger.touch(&main, "s");
        let refreshed = last_used(&ledger);
        assert!(refreshed > stale);
        ledger.touch(&main, "s");
        assert_eq!(last_used(&ledger), refreshed, "refreshes are coarsened");
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sessions.json");
        let ledger = SessionLedger::open(path);
        for i in 0..5 {
            ledger.record(&main_scope(Uuid::new_v4()), &i.to_string(), "id", "/c");
        }
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["sessions.json"]);
    }
}
