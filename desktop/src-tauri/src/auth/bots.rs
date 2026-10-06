//! Managed agents as server bots (plan §4.14, token mode only).
//!
//! Only a legacy token-mode Google account runs each managed agent as a
//! server-registered bot hosted by this device. Google key-backup accounts
//! remain [`CredentialMode::Keys`] and preserve agent keys and NIP-OA tags;
//! none of the adoption or token-renewal paths below apply to them.
//!
//! 1. The agent *is* its bot: created in token mode it is registered at once
//!    ([`register_new_agent`]); an existing key agent is moved onto a new bot
//!    the first time it starts or its pubkey reaches the relay ([`adopt`]),
//!    rewriting the record's `pubkey` to the bot id and dropping its key.
//!    [`prepare_for_start`] (before any runtime lock, ahead of the Hermes
//!    profile preparation) then issues a fresh 1 h bot token
//!    (`POST /auth/bots/{id}/token`).
//! 2. The synchronous spawn takes that token ([`spawn_auth`]) and injects
//!    `BUZZ_BOT_TOKEN` (+ `BUZZ_BOT_TOKEN_EXPIRES_AT`); it never injects the
//!    agent's private key or NIP-OA tag in token mode.
//! 3. `buzz-acp` exits 78 when its token can no longer be refreshed. The exit
//!    is queued ([`note_exit`]); the auth watchdog reissues and restarts, at
//!    most [`RESTART_CAP`] times per [`RESTART_WINDOW`] ([`RestartLimiter`]).
//!    At the cap the agent is parked as auth-failed until the user presses
//!    Restart (which resets the counter) or signs in again (Rule 4, Rule 6).
//!
//! Network calls run on a private thread with its own runtime
//! ([`block_on_isolated`]) so they are safe from both sync and async callers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use tauri::Manager;
use zeroize::Zeroizing;

use super::api::{self, ApiError};
use super::{origin_for, CredentialMode, UserSession};
use crate::app_state::AppState;
use crate::managed_agents::ManagedAgentRecord;

/// `buzz-acp` exit code: the bot token is unusable (plan §4.11).
pub(crate) const EXIT_AUTH_TERMINAL: i32 = 78;
/// Auto-restarts allowed per agent within [`RESTART_WINDOW`].
pub(crate) const RESTART_CAP: usize = 3;
/// Window for [`RESTART_CAP`].
pub(crate) const RESTART_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Token-carrying env vars Desktop owns for every managed-agent spawn: always
/// stripped first, then (token mode) only `BUZZ_BOT_TOKEN*` re-injected.
pub(crate) const TOKEN_ENV_KEYS: [&str; 5] = [
    "BUZZ_BOT_TOKEN",
    "BUZZ_ACCESS_TOKEN",
    "BUZZ_TOKEN_BROKER_URL",
    "BUZZ_TOKEN_BROKER_SECRET",
    "BUZZ_BOT_TOKEN_EXPIRES_AT",
];

/// Outcome of recording an auth-terminal exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartDecision {
    /// Reissue the token and restart.
    Restart,
    /// Too many auth failures in the window: stop auto-restarting.
    Capped,
}

/// Sliding-window restart accounting per agent. Pure (time injected).
#[derive(Default)]
pub(crate) struct RestartLimiter {
    history: HashMap<String, VecDeque<Instant>>,
}

impl RestartLimiter {
    /// Record an auth-terminal exit of `agent` at `now`.
    pub(crate) fn record(&mut self, agent: &str, now: Instant) -> RestartDecision {
        let history = self.history.entry(agent.to_owned()).or_default();
        while history
            .front()
            .is_some_and(|at| now.duration_since(*at) >= RESTART_WINDOW)
        {
            history.pop_front();
        }
        if history.len() >= RESTART_CAP {
            return RestartDecision::Capped;
        }
        history.push_back(now);
        RestartDecision::Restart
    }

    /// Forget `agent`'s history (manual restart / new sign-in).
    pub(crate) fn reset(&mut self, agent: &str) {
        self.history.remove(agent);
    }
}

/// A bot token issued for the next spawn of one agent.
pub(crate) struct IssuedBotToken {
    origin: String,
    /// The bot the token belongs to (kept for diagnostics).
    #[allow(dead_code)]
    pub(crate) bot_id: String,
    pub(crate) token: Zeroizing<String>,
    pub(crate) expires_at: Option<i64>,
}

/// Bot-related runtime state on [`super::TokenAuthState`].
#[derive(Default)]
pub(crate) struct BotRuntime {
    issued: Mutex<HashMap<String, IssuedBotToken>>,
    limiter: Mutex<RestartLimiter>,
    auth_failed: Mutex<HashMap<String, String>>,
    notices: Mutex<Vec<String>>,
}

impl BotRuntime {
    fn put(&self, agent: &str, token: IssuedBotToken) {
        self.issued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(agent.to_owned(), token);
    }

    /// Test seam: stage an issued token for `agent`'s next spawn on `origin`.
    #[cfg(test)]
    pub(crate) fn stage_for_test(&self, agent: &str, origin: &str, token: &str) {
        self.put(
            agent,
            IssuedBotToken {
                origin: origin.to_owned(),
                bot_id: agent.to_owned(),
                token: Zeroizing::new(token.to_owned()),
                expires_at: Some(1_900_000_000),
            },
        );
    }

    fn take(&self, agent: &str, origin: &str) -> Option<IssuedBotToken> {
        let mut issued = self.issued.lock().unwrap_or_else(PoisonError::into_inner);
        match issued.get(agent) {
            Some(token) if token.origin == origin => issued.remove(agent),
            _ => None,
        }
    }

    /// Record an auth-terminal exit; at the cap the agent is parked.
    pub(crate) fn record_auth_exit(&self, agent: &str, now: Instant) -> RestartDecision {
        let decision = self
            .limiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .record(agent, now);
        if decision == RestartDecision::Capped {
            self.auth_failed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    agent.to_owned(),
                    "The agent's sign-in token was rejected repeatedly. Press Restart to try \
                     again, or sign in with Google again if your session ended."
                        .to_owned(),
                );
        }
        decision
    }

    /// Park `agent` as auth-failed with `reason` (shown as its last error until
    /// a manual Start resets it).
    pub(crate) fn park(&self, agent: &str, reason: String) {
        self.auth_failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(agent.to_owned(), reason);
    }

    /// Manual restart: reset the counter and un-park the agent.
    pub(crate) fn reset(&self, agent: &str) {
        self.limiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reset(agent);
        self.auth_failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(agent);
    }

    /// `Some(reason)` when `agent` is parked as auth-failed.
    pub(crate) fn auth_failed(&self, agent: &str) -> Option<String> {
        self.auth_failed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(agent)
            .cloned()
    }

    fn push_notice(&self, name: String) {
        self.notices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(name);
    }

    /// Drain one-time "agent re-registered" notices.
    pub(crate) fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Auth-terminal exits waiting for the watchdog: `(agent pubkey, relay url)`.
fn exit_queue() -> &'static Mutex<Vec<(String, String)>> {
    static QUEUE: OnceLock<Mutex<Vec<(String, String)>>> = OnceLock::new();
    QUEUE.get_or_init(Mutex::default)
}

/// Called from the process-exit sync for every exited harness.
pub(crate) fn note_exit(agent: &str, relay_url: &str, code: Option<i32>) {
    if code == Some(EXIT_AUTH_TERMINAL) {
        exit_queue()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((agent.to_owned(), relay_url.to_owned()));
    }
}

/// Drain queued auth-terminal exits (deduplicated).
pub(crate) fn take_exits() -> Vec<(String, String)> {
    take_exits_matching(|_| true)
}

/// Drain the queued exits `keep` selects (deduplicated); others stay queued.
pub(crate) fn take_exits_matching(
    keep: impl Fn(&(String, String)) -> bool,
) -> Vec<(String, String)> {
    let mut queue = exit_queue().lock().unwrap_or_else(PoisonError::into_inner);
    let (mut drained, rest): (Vec<_>, Vec<_>) = queue.drain(..).partition(|entry| keep(entry));
    *queue = rest;
    let mut seen = HashSet::new();
    drained.retain(|entry| seen.insert(entry.clone()));
    drained
}

// ── adoption: a managed agent becomes its server bot ───────────────────────
//
// In a Google-session community the relay knows a managed agent only as its
// server bot id. Rather than aliasing the local record key to that id, the
// record itself is rewritten once (plan §4.14, history break accepted in Q7):
// `pubkey` becomes the bot id, the local key and NIP-OA tag are dropped, and
// `bot_origin` records the community. Every other path (DMs, membership,
// deletion, profile) then sees the one identity the agent connects as.
//
// The server registration and the record rewrite are two durable writes, so a
// journal (`auth-bot-adoptions.json`, `{origin: {from: bot id}}`) is written
// between them: a crash after registering resumes with the same bot instead of
// registering a second one (Rules 1 and 5). A per-`(origin, agent)` lock makes
// concurrent callers share one registration.

/// Resume journal for an in-flight adoption (see the section comment).
const ADOPTION_JOURNAL_FILE: &str = "auth-bot-adoptions.json";

type AdoptionJournal = HashMap<String, HashMap<String, String>>;

fn journal_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(Mutex::default)
}

fn load_journal(dir: &Path) -> Result<AdoptionJournal, String> {
    match std::fs::read(dir.join(ADOPTION_JOURNAL_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("{ADOPTION_JOURNAL_FILE} is unreadable: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(error) => Err(format!("read {ADOPTION_JOURNAL_FILE}: {error}")),
    }
}

fn save_journal(dir: &Path, journal: &AdoptionJournal) -> Result<(), String> {
    use std::io::Write;
    let bytes = serde_json::to_vec_pretty(journal).map_err(|e| e.to_string())?;
    let mut file = atomic_write_file::AtomicWriteFile::open(dir.join(ADOPTION_JOURNAL_FILE))
        .map_err(|e| format!("open {ADOPTION_JOURNAL_FILE}: {e}"))?;
    file.write_all(&bytes)
        .map_err(|e| format!("write {ADOPTION_JOURNAL_FILE}: {e}"))?;
    file.commit()
        .map_err(|e| format!("commit {ADOPTION_JOURNAL_FILE}: {e}"))
}

fn journal_get(dir: &Path, origin: &str, from: &str) -> Result<Option<String>, String> {
    let _guard = journal_lock()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    Ok(load_journal(dir)?
        .get(origin)
        .and_then(|entries| entries.get(from))
        .cloned())
}

fn journal_set(dir: &Path, origin: &str, from: &str, bot_id: Option<&str>) -> Result<(), String> {
    let _guard = journal_lock()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut journal = load_journal(dir)?;
    match bot_id {
        Some(id) => {
            journal
                .entry(origin.to_owned())
                .or_default()
                .insert(from.to_owned(), id.to_owned());
        }
        None => {
            if let Some(entries) = journal.get_mut(origin) {
                entries.remove(from);
            }
            journal.retain(|_, entries| !entries.is_empty());
        }
    }
    save_journal(dir, &journal)
}

/// The record rewrite of an adoption: the record *is* the bot from now on.
pub(crate) fn adopt_record_identity(record: &mut ManagedAgentRecord, bot_id: &str, origin: &str) {
    record.pubkey = bot_id.to_owned();
    record.private_key_nsec.clear();
    record.auth_tag = None;
    record.bot_origin = Some(origin.to_owned());
    record.updated_at = crate::util::now_iso();
}

/// Outcome of [`adopt`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Adoption {
    /// The agent's record now carries this server bot id.
    Bot(String),
    /// The agent is running on its local key; it moves on its next start.
    Running,
}

/// Side effects of an adoption. Production talks to the relay and the agent
/// store; tests inject fakes and keep the locking and journal real.
pub(crate) trait AdoptionEffects {
    /// Whether `pubkey`'s record is already a bot on this origin.
    fn is_bot(&self, pubkey: &str) -> Result<bool, String>;
    /// Whether a harness for `pubkey` is alive (its record must not move).
    fn is_running(&self, pubkey: &str) -> bool;
    /// `POST /auth/bots`.
    fn create_bot(&self) -> Result<String, String>;
    /// Best-effort `DELETE /auth/bots/{id}` of a bot that could not be journaled.
    fn delete_bot(&self, bot_id: &str);
    /// Rewrite the record of `from` to `to` (see [`rewrite_record`]).
    /// `Ok(true)` once every local trace of `from` is gone, so the journal
    /// entry may be cleared; `Ok(false)` leaves it for the startup sweep.
    fn rewrite(&self, from: &str, to: &str) -> Result<bool, String>;
}

/// The local steps of moving an agent from `from` to `to`.
pub(crate) trait MoveOps {
    /// Rename the Hermes profile directory (best-effort, never overwrites).
    fn move_profile(&self, from: &str, to: &str);
    /// Persist the record rewrite (see [`adopt_record_identity`]).
    fn save_record(&self, from: &str, to: &str) -> Result<(), String>;
    /// Drop per-agent runtime state kept under `from`.
    fn forget_runtime(&self, from: &str);
    /// Remove `from`'s private key from the keyring.
    fn drop_local_key(&self, from: &str) -> Result<(), String>;
    /// Tell the UI the agent list changed (`agents-data-changed`).
    fn announce(&self, to: &str);
}

/// Move an agent's local state from `from` to `to`, crash-safe in this order:
/// the Hermes profile first (so a saved record never points at a missing
/// profile; a failed save moves it back), then the record, then the old key.
/// A crash in between leaves the adoption journal entry, which the startup
/// sweep ([`sweep_adoption_journal`]) settles. Returns whether the old key is
/// gone (the journal entry may then be cleared).
pub(crate) fn rewrite_record(ops: &impl MoveOps, from: &str, to: &str) -> Result<bool, String> {
    ops.move_profile(from, to);
    if let Err(error) = ops.save_record(from, to) {
        ops.move_profile(to, from);
        return Err(error);
    }
    ops.forget_runtime(from);
    let key_gone = match ops.drop_local_key(from) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("buzz-desktop: auth: could not remove the moved agent's local key: {error}");
            false
        }
    };
    ops.announce(to);
    Ok(key_gone)
}

/// Completed moves `old pubkey → new pubkey` (lowercase), so a UI action
/// still holding an agent's old pubkey reaches its record (N1). Filled when a
/// rewrite completes and, at launch, from journal entries a crash left behind.
fn moves() -> &'static Mutex<HashMap<String, String>> {
    static MOVES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    MOVES.get_or_init(Mutex::default)
}

fn remember_move(from: &str, to: &str) {
    moves()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(from.to_ascii_lowercase(), to.to_owned());
}

/// The pubkey `pubkey`'s agent has now: itself unless the agent moved onto a
/// server bot (possibly more than once), in which case its latest id.
pub(crate) fn current_agent_pubkey(pubkey: &str) -> String {
    let moves = moves().lock().unwrap_or_else(PoisonError::into_inner);
    let mut current = pubkey.to_owned();
    // Bounded: a chain is at most as long as the map.
    for _ in 0..=moves.len() {
        match moves.get(&current.to_ascii_lowercase()) {
            Some(next) => current = next.clone(),
            None => break,
        }
    }
    current
}

/// Settle the adoption journal at launch, before any agent starts. For each
/// entry `from → to`:
/// - the record still has `from` (crash before the record was saved): the
///   Hermes profile goes back to `from`, and the entry stays so the next
///   adoption resumes with the same bot;
/// - otherwise (moved, or deleted since): the profile follows the record to
///   `to`, `from`'s key is removed from the keyring, and the entry is
///   cleared once that succeeded.
pub(crate) fn sweep_journal(
    dir: &Path,
    has_record: impl Fn(&str) -> bool,
    ops: &impl MoveOps,
) -> Result<(), String> {
    let journal = {
        let _guard = journal_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        load_journal(dir)?
    };
    for (origin, entries) in journal {
        for (from, to) in entries {
            if has_record(&from) {
                ops.move_profile(&to, &from);
                continue;
            }
            if has_record(&to) {
                ops.move_profile(&from, &to);
                remember_move(&from, &to);
            }
            match ops.drop_local_key(&from) {
                Ok(()) => journal_set(dir, &origin, &from, None)?,
                Err(error) => {
                    eprintln!("buzz-desktop: auth: moved agent key still in the keyring: {error}");
                }
            }
        }
    }
    Ok(())
}

/// Production [`sweep_journal`] at app launch. Best-effort: logged, never fatal.
pub(crate) fn sweep_adoption_journal<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let result = (|| {
        let dir = data_dir(app)?;
        let state = app.state::<AppState>();
        let pubkeys: HashSet<String> = {
            let _store = state
                .managed_agents_store_lock
                .lock()
                .map_err(|e| e.to_string())?;
            crate::managed_agents::load_managed_agents(app)?
                .into_iter()
                .map(|record| record.pubkey.to_ascii_lowercase())
                .collect()
        };
        let ops = StoreMoveOps { app, origin: "" };
        sweep_journal(
            &dir,
            |pubkey| pubkeys.contains(&pubkey.to_ascii_lowercase()),
            &ops,
        )
    })();
    if let Err(error) = result {
        eprintln!("buzz-desktop: auth: adoption journal sweep failed: {error}");
    }
}

type AdoptionSlot = std::sync::Arc<Mutex<Option<String>>>;

/// Per-`(origin, agent)` lock. Its value remembers where the agent moved, so a
/// caller that waited behind the adoption follows it to the new id.
fn adoption_slot(origin: &str, agent: &str) -> AdoptionSlot {
    static SLOTS: OnceLock<Mutex<HashMap<(String, String), AdoptionSlot>>> = OnceLock::new();
    SLOTS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry((origin.to_owned(), agent.to_ascii_lowercase()))
        .or_default()
        .clone()
}

/// Move `agent` onto a server bot on `origin`, once. With `replace`, `agent`
/// is already a bot the relay no longer lets this device host (403/404) and a
/// new bot replaces it. Blocking: call from a blocking context.
pub(crate) fn adopt(
    dir: &Path,
    origin: &str,
    agent: &str,
    replace: bool,
    effects: &impl AdoptionEffects,
) -> Result<Adoption, String> {
    let slot = adoption_slot(origin, agent);
    let mut moved = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(bot_id) = moved.as_ref() {
        return Ok(Adoption::Bot(bot_id.clone()));
    }
    if !replace && effects.is_bot(agent)? {
        return Ok(Adoption::Bot(agent.to_owned()));
    }
    if effects.is_running(agent) {
        return Ok(Adoption::Running);
    }
    let bot_id = match journal_get(dir, origin, agent)? {
        Some(bot_id) => bot_id,
        None => {
            let bot_id = effects.create_bot()?;
            if let Err(error) = journal_set(dir, origin, agent, Some(&bot_id)) {
                effects.delete_bot(&bot_id);
                return Err(error);
            }
            bot_id
        }
    };
    // A failed rewrite keeps the journal entry: the next attempt resumes with
    // this bot rather than registering another.
    let settled = effects.rewrite(agent, &bot_id)?;
    remember_move(agent, &bot_id);
    if settled {
        if let Err(error) = journal_set(dir, origin, agent, None) {
            // Harmless leftover: the startup sweep clears it.
            eprintln!("buzz-desktop: auth: could not clear the adoption journal: {error}");
        }
    }
    *moved = Some(bot_id.clone());
    Ok(Adoption::Bot(bot_id))
}

/// Production [`AdoptionEffects`]: the relay at `origin` and the agent store.
struct StoreEffects<'a, R: tauri::Runtime> {
    app: &'a tauri::AppHandle<R>,
    origin: &'a str,
    access: &'a str,
    display_name: String,
}

impl<R: tauri::Runtime> AdoptionEffects for StoreEffects<'_, R> {
    fn is_bot(&self, pubkey: &str) -> Result<bool, String> {
        let state = self.app.state::<AppState>();
        let _store = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        let records = crate::managed_agents::load_managed_agents(self.app)?;
        let record = records
            .iter()
            .find(|record| record.pubkey.eq_ignore_ascii_case(pubkey))
            .ok_or_else(|| format!("agent {pubkey} not found"))?;
        Ok(record.bot_origin.as_deref() == Some(self.origin))
    }

    fn is_running(&self, pubkey: &str) -> bool {
        let state = self.app.state::<AppState>();
        let Ok(mut runtimes) = state.managed_agent_processes.lock() else {
            // Unknown is not idle: never move a record we cannot check.
            return true;
        };
        runtimes.iter_mut().any(|(key, runtime)| {
            key.pubkey.eq_ignore_ascii_case(pubkey)
                && runtime.child.try_wait().ok().flatten().is_none()
        })
    }

    fn create_bot(&self) -> Result<String, String> {
        let (origin, access, name) = (
            self.origin.to_owned(),
            self.access.to_owned(),
            self.display_name.clone(),
        );
        block_on_isolated(async move {
            api::create_bot(&reqwest::Client::new(), &origin, &access, &name).await
        })?
        .map_err(|e| format!("could not register the agent on this community: {e}"))
    }

    fn delete_bot(&self, bot_id: &str) {
        let (origin, access, bot_id) = (
            self.origin.to_owned(),
            self.access.to_owned(),
            bot_id.to_owned(),
        );
        let result = block_on_isolated(async move {
            api::delete_bot(&reqwest::Client::new(), &origin, &access, &bot_id).await
        });
        if !matches!(result, Ok(Ok(()))) {
            eprintln!("buzz-desktop: auth: could not delete an unjournaled bot");
        }
    }

    fn rewrite(&self, from: &str, to: &str) -> Result<bool, String> {
        rewrite_record(
            &StoreMoveOps {
                app: self.app,
                origin: self.origin,
            },
            from,
            to,
        )
    }
}

/// Production [`MoveOps`]: the agent store, keyring, Hermes profiles, UI.
struct StoreMoveOps<'a, R: tauri::Runtime> {
    app: &'a tauri::AppHandle<R>,
    origin: &'a str,
}

impl<R: tauri::Runtime> MoveOps for StoreMoveOps<'_, R> {
    fn move_profile(&self, from: &str, to: &str) {
        crate::managed_agents::hermes_profile::move_profile_for_new_pubkey(from, to);
    }

    fn save_record(&self, from: &str, to: &str) -> Result<(), String> {
        let state = self.app.state::<AppState>();
        let _store = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        let mut records = crate::managed_agents::load_managed_agents(self.app)?;
        let record = records
            .iter_mut()
            .find(|record| record.pubkey.eq_ignore_ascii_case(from))
            .ok_or_else(|| format!("agent {from} not found"))?;
        adopt_record_identity(record, to, self.origin);
        crate::managed_agents::save_managed_agents(self.app, &records)
    }

    fn forget_runtime(&self, from: &str) {
        let state = self.app.state::<AppState>();
        state.clear_agent_session_caches(from);
        state.token_auth.bots.reset(from);
    }

    fn drop_local_key(&self, from: &str) -> Result<(), String> {
        crate::managed_agents::storage::try_delete_agent_key(from)
    }

    fn announce(&self, to: &str) {
        use tauri::Emitter;
        self.app
            .state::<AppState>()
            .token_auth
            .bots
            .push_notice(to.to_owned());
        // The agent list does not poll while idle: without this the UI keeps
        // the old pubkey and Start/Stop/Edit/Delete miss the record.
        if let Err(error) = self.app.emit("agents-data-changed", ()) {
            eprintln!("buzz-desktop: auth: could not announce the moved agent: {error}");
        }
    }
}

fn data_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
    Ok(dir)
}

// ── network helpers ────────────────────────────────────────────────────────

/// Run `fut` to completion on a private thread with its own runtime. Safe to
/// call from sync code and from inside an async task (it never nests runtimes).
pub(crate) fn block_on_isolated<F, T>(fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map(|runtime| runtime.block_on(fut))
            .map_err(|e| format!("auth runtime: {e}"))
    })
    .join()
    .map_err(|_| "auth worker thread panicked".to_string())?
}

fn session_for(state: &AppState, origin: &str) -> Result<Option<UserSession>, String> {
    match state.token_auth.mode(origin) {
        CredentialMode::Keys => Ok(None),
        CredentialMode::Token(session) => Ok(Some(session)),
        CredentialMode::Blocked(reason) => Err(reason),
    }
}

fn key_backup_bot_conflict(
    state: &AppState,
    record: &ManagedAgentRecord,
    origin: &str,
) -> Result<(), String> {
    if record.bot_origin.is_some() && state.token_auth.signing_pubkey(origin).is_some() {
        return Err(format!("{} belongs to an earlier token account. Google key recovery cannot merge or recreate this agent. Its existing record and history were preserved.", record_label(record)));
    }
    Ok(())
}

fn record_label(record: &ManagedAgentRecord) -> &str {
    record.display_name.as_deref().unwrap_or(&record.name)
}

fn bot_display_name(record: &ManagedAgentRecord) -> String {
    let name: String = record_label(record).chars().take(64).collect();
    if name.trim().is_empty() {
        "Agent".to_owned()
    } else {
        name
    }
}

/// Why a server-bot record cannot run on `origin` (it runs only on its own
/// community, and only while signed in there). `None` for key agents.
pub(crate) fn bot_home_refusal(
    record: &ManagedAgentRecord,
    origin: &str,
    signed_in: bool,
) -> Option<String> {
    let home = record.bot_origin.as_deref()?;
    let name = record_label(record);
    if home != origin {
        Some(format!(
            "{name} belongs to your Google account on {home}; switch to that community to start it."
        ))
    } else if !signed_in {
        Some(format!(
            "{name} runs with your Google account on this community. Sign in with Google to \
             start it."
        ))
    } else {
        None
    }
}

/// Why `record`, a server bot of another community, must not be named on
/// `origin`'s relay: its bot id means nothing there. `None` otherwise.
pub(crate) fn foreign_bot_refusal(record: &ManagedAgentRecord, origin: &str) -> Option<String> {
    let home = record.bot_origin.as_deref()?;
    (home != origin).then(|| {
        format!(
            "{} belongs to your Google account on {home}; it cannot be added or messaged in this \
             community.",
            record_label(record)
        )
    })
}

/// A key agent still running on its local key in a community now signed in
/// with Google: it moves onto its server bot only when it restarts, so the
/// agent list shows "Restart required" with this entry (never silent).
pub(crate) fn key_agent_move_pending(
    record: &ManagedAgentRecord,
    running_here: bool,
    signed_in_here: bool,
) -> Option<crate::managed_agents::spawn_snapshot::RestartDiffEntry> {
    let movable =
        record.backend == crate::managed_agents::BackendKind::Local && record.bot_origin.is_none();
    (movable && running_here && signed_in_here).then(|| {
        crate::managed_agents::spawn_snapshot::RestartDiffEntry {
            field: "sign_in".to_owned(),
            change: crate::managed_agents::spawn_snapshot::diff::RestartChange::Value {
                before: serde_json::Value::String("local key".to_owned()),
                after: serde_json::Value::String("Google account".to_owned()),
            },
        }
    })
}

// ── deletion fence ─────────────────────────────────────────────────────────

fn deleting() -> &'static Mutex<HashSet<String>> {
    static DELETING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    DELETING.get_or_init(Mutex::default)
}

/// Marks an agent as being deleted for its lifetime: no start (and so no
/// watchdog restart, which would register a new bot) is admitted meanwhile.
pub(crate) struct DeleteFence(String);

impl DeleteFence {
    /// Fence `agent` and drop its queued auth-terminal exits.
    pub(crate) fn begin(agent: &str) -> Self {
        let key = agent.to_ascii_lowercase();
        deleting()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone());
        take_exits_matching(|(queued, _)| queued.eq_ignore_ascii_case(agent));
        Self(key)
    }

    /// Whether `agent` is being deleted right now.
    #[cfg(test)]
    pub(crate) fn is_active(agent: &str) -> bool {
        is_being_deleted(agent)
    }
}

impl Drop for DeleteFence {
    fn drop(&mut self) {
        deleting()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

fn is_being_deleted(agent: &str) -> bool {
    deleting()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&agent.to_ascii_lowercase())
}

/// Register (or adopt) the bot when needed and issue a fresh bot token for the
/// next spawn of `record` on `relay_url`. Returns the pubkey the agent spawns
/// as: unchanged in key mode, the bot id in token mode — the caller continues
/// with that pubkey (the record may have just been rewritten). Blocking; call
/// before taking runtime locks.
pub(crate) fn prepare_for_start<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    record: &ManagedAgentRecord,
    relay_url: &str,
) -> Result<String, String> {
    if is_being_deleted(&record.pubkey) {
        return Err(format!("{} is being deleted.", record_label(record)));
    }
    let state = app.state::<AppState>();
    let origin = origin_for(relay_url);
    // A launch restore can run before the UI asked for this community's
    // status: restore a stored session first so a signed-in community never
    // spawns its agents on key auth.
    super::restore::restore_if_stored_blocking(app, &origin);
    key_backup_bot_conflict(&state, record, &origin)?;
    // Refreshed first when about to expire: a reissue after sleep or a long
    // backoff must not go out with an expired access token.
    let Some(session) = super::commands::fresh_session_for_spawn(&state, &origin)? else {
        state.token_auth.bots.take(&record.pubkey, &origin);
        return match bot_home_refusal(record, &origin, false) {
            Some(reason) => Err(reason),
            None => Ok(record.pubkey.clone()),
        };
    };
    if let Some(reason) = bot_home_refusal(record, &origin, true) {
        return Err(reason);
    }
    if record.backend != crate::managed_agents::BackendKind::Local {
        return Ok(record.pubkey.clone());
    }
    let dir = data_dir(app)?;
    let effects = StoreEffects {
        app,
        origin: &origin,
        access: &session.access,
        display_name: bot_display_name(record),
    };
    let adopt_or_refuse =
        |agent: &str, replace: bool| match adopt(&dir, &origin, agent, replace, &effects)? {
            Adoption::Bot(bot_id) => Ok(bot_id),
            Adoption::Running => Err(format!(
                "{} is still running on its local key. Stop it, then start it again to move it to \
             your Google account.",
                record_label(record)
            )),
        };
    let issue = |bot_id: &str| {
        let (origin, access, bot_id) = (origin.clone(), session.access.clone(), bot_id.to_owned());
        block_on_isolated(async move {
            api::issue_bot_token(&reqwest::Client::new(), &origin, &access, &bot_id).await
        })
    };
    let mut bot_id = adopt_or_refuse(&record.pubkey, false)?;
    // Already running as the bot (nothing will spawn): issuing a token now
    // would only revoke the live one.
    if effects.is_running(&bot_id) {
        return Ok(bot_id);
    }
    let token = match issue(&bot_id)? {
        Ok(token) => token,
        // The relay no longer lets this device host the bot (403: another
        // device registered it, e.g. after a sign-out; 404: deleted). Replace
        // it with a new registration — a history break the plan accepts (Q7).
        Err(ApiError {
            status: Some(403 | 404),
            ..
        }) => {
            bot_id = adopt_or_refuse(&bot_id, true)?;
            issue(&bot_id)?.map_err(|e| format!("could not issue the agent's token: {e}"))?
        }
        Err(error) => return Err(format!("could not issue the agent's token: {error}")),
    };
    state.token_auth.bots.put(
        &bot_id,
        IssuedBotToken {
            origin,
            bot_id: bot_id.clone(),
            expires_at: token.expires_at_unix(),
            token: Zeroizing::new(token.token.0.to_string()),
        },
    );
    Ok(bot_id)
}

/// In a Google-session community, move every not-yet-moved local managed agent
/// named in `pubkeys` onto its server bot before its pubkey reaches the relay
/// (channel add, DM, mention), and return the pubkeys the relay knows. This is
/// the one-time record rewrite of [`adopt`], not an alias: afterwards the
/// record's own pubkey is the bot id. Key mode, other pubkeys, and agents
/// still running on their local key pass through unchanged.
pub(crate) async fn adopt_named_agents(
    state: &AppState,
    pubkeys: Vec<String>,
) -> Result<Vec<String>, String> {
    let origin = state.current_auth_origin();
    let session = match state.token_auth.mode(&origin) {
        CredentialMode::Token(session) => session,
        _ => return Ok(pubkeys),
    };
    let Some(app) = state.app_handle.lock().ok().and_then(|guard| guard.clone()) else {
        return Ok(pubkeys);
    };
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let records = {
            let _store = state
                .managed_agents_store_lock
                .lock()
                .map_err(|e| e.to_string())?;
            crate::managed_agents::load_managed_agents(&app)?
        };
        let dir = data_dir(&app)?;
        let mut out = Vec::with_capacity(pubkeys.len());
        for pubkey in pubkeys {
            let record = records
                .iter()
                .find(|record| record.pubkey.eq_ignore_ascii_case(&pubkey));
            if let Some(reason) = record.and_then(|record| foreign_bot_refusal(record, &origin)) {
                return Err(reason);
            }
            let movable = record.filter(|record| {
                record.backend == crate::managed_agents::BackendKind::Local
                    && record.bot_origin.is_none()
            });
            let Some(record) = movable else {
                out.push(pubkey);
                continue;
            };
            let effects = StoreEffects {
                app: &app,
                origin: &origin,
                access: &session.access,
                display_name: bot_display_name(record),
            };
            match adopt(&dir, &origin, &record.pubkey, false, &effects)? {
                Adoption::Bot(bot_id) => out.push(bot_id),
                Adoption::Running => out.push(pubkey),
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

/// [`adopt_named_agents`] over the values of `p` tags.
pub(crate) async fn adopt_named_agents_in_p_tags(
    state: &AppState,
    mut tags: Vec<Vec<String>>,
) -> Result<Vec<Vec<String>>, String> {
    let positions: Vec<usize> = tags
        .iter()
        .enumerate()
        .filter(|(_, tag)| tag.first().map(String::as_str) == Some("p") && tag.len() >= 2)
        .map(|(i, _)| i)
        .collect();
    if positions.is_empty() {
        return Ok(tags);
    }
    let values = positions.iter().map(|&i| tags[i][1].clone()).collect();
    let adopted = adopt_named_agents(state, values).await?;
    for (i, value) in positions.into_iter().zip(adopted) {
        tags[i][1] = value;
    }
    Ok(tags)
}

/// The keys Desktop may sign a message *as* `record` with (welcome kickoff).
/// Refused for a server bot and in a Google-session community: there the
/// agent's identity is server-stamped, and only the agent's own harness holds
/// a token for it (issuing one here would revoke the running agent's token).
pub(crate) fn managed_agent_message_keys(
    record: &ManagedAgentRecord,
    token_mode: bool,
) -> Result<nostr::Keys, String> {
    if token_mode || record.bot_origin.is_some() {
        return Err(format!(
            "Desktop cannot post as {} in a community you signed in to with Google; the \
             agent posts for itself once it is running.",
            record_label(record)
        ));
    }
    let keys = nostr::Keys::parse(record.private_key_nsec.trim())
        .map_err(|error| format!("failed to parse managed agent key: {error}"))?;
    if !keys
        .public_key()
        .to_hex()
        .eq_ignore_ascii_case(&record.pubkey)
    {
        return Err(format!(
            "managed agent key does not match stored pubkey {}",
            record.pubkey
        ));
    }
    Ok(keys)
}

/// How the next spawn of an agent authenticates.
pub(crate) enum SpawnAuth {
    /// Key auth: the record's own key (and NIP-OA tag) as before.
    Keys,
    /// Token auth with this freshly issued bot token.
    Token(IssuedBotToken),
}

/// Decide (and consume) the credential for spawning `record` on `relay_url`.
/// In token mode a token must have been prepared by [`prepare_for_start`]; a
/// server-bot record never falls back to key auth.
pub(crate) fn spawn_auth(
    state: &AppState,
    record: &ManagedAgentRecord,
    relay_url: &str,
) -> Result<SpawnAuth, String> {
    let origin = origin_for(relay_url);
    key_backup_bot_conflict(state, record, &origin)?;
    let signed_in = session_for(state, &origin)?.is_some();
    if let Some(reason) = bot_home_refusal(record, &origin, signed_in) {
        return Err(reason);
    }
    if !signed_in {
        return Ok(SpawnAuth::Keys);
    }
    state
        .token_auth
        .bots
        .take(&record.pubkey, &origin)
        .map(SpawnAuth::Token)
        .ok_or_else(|| {
            "the agent's sign-in token was not issued for this community; start the agent again"
                .to_string()
        })
}

/// Apply `auth` to a spawn command. Always strips every Desktop-owned token
/// variable first so neither the parent env nor saved agent env can leak one.
pub(crate) fn apply_spawn_auth(
    command: &mut std::process::Command,
    auth: &SpawnAuth,
    private_key_nsec: &str,
    auth_tag: Option<&str>,
) {
    for key in TOKEN_ENV_KEYS {
        command.env_remove(key);
    }
    match auth {
        SpawnAuth::Keys => {
            command.env("BUZZ_PRIVATE_KEY", private_key_nsec);
            match auth_tag {
                Some(tag) => command.env("BUZZ_AUTH_TAG", tag),
                None => command.env_remove("BUZZ_AUTH_TAG"),
            };
        }
        SpawnAuth::Token(token) => {
            command.env_remove("BUZZ_PRIVATE_KEY");
            command.env_remove("BUZZ_AUTH_TAG");
            command.env("BUZZ_BOT_TOKEN", token.token.as_str());
            if let Some(expires_at) = token.expires_at {
                command.env("BUZZ_BOT_TOKEN_EXPIRES_AT", expires_at.to_string());
            }
        }
    }
}

fn load_record<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    pubkey: &str,
) -> Result<Option<ManagedAgentRecord>, String> {
    let state = app.state::<AppState>();
    let _store = state
        .managed_agents_store_lock
        .lock()
        .map_err(|e| e.to_string())?;
    Ok(crate::managed_agents::load_managed_agents(app)?
        .into_iter()
        .find(|record| record.pubkey.eq_ignore_ascii_case(pubkey)))
}

/// Best-effort `POST /auth/bots/{id}/revoke` for a server-bot `agent` (agent
/// Stop). Key agents and signed-out communities are a no-op.
pub(crate) fn revoke_on_stop<R: tauri::Runtime>(app: &tauri::AppHandle<R>, agent: &str) {
    let Ok(Some(record)) = load_record(app, agent) else {
        return;
    };
    let Some(origin) = record.bot_origin else {
        return;
    };
    let state = app.state::<AppState>();
    let Ok(Some(session)) = session_for(&state, &origin) else {
        return;
    };
    let access = session.access.clone();
    let result = block_on_isolated(async move {
        api::stop_bot(&reqwest::Client::new(), &origin, &access, &record.pubkey)
            .await
            .map_err(|e| e.to_string())
    })
    .and_then(|inner| inner);
    if let Err(error) = result {
        // The token still expires within the hour; Stop never fails on this.
        eprintln!("buzz-desktop: auth: bot token revoke on stop failed: {error}");
    }
}

/// [`delete_server_bot`] for the stored record of `agent`.
pub(crate) async fn delete_server_bots_for<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    agent: &str,
) -> Result<(), String> {
    let record = load_record(app, agent)?;
    match record {
        Some(record) => delete_server_bot(&app.state::<AppState>(), &record).await,
        None => Ok(()),
    }
}

/// Delete `record`'s server bot when it is one. Must succeed before local
/// deletion (Rule 1): an error leaves the agent in place so the user can
/// retry. Signed out of its community, the bot stays server-side (owned by
/// the account) and local deletion proceeds.
pub(crate) async fn delete_server_bot(
    state: &AppState,
    record: &ManagedAgentRecord,
) -> Result<(), String> {
    let Some(origin) = record.bot_origin.as_deref() else {
        return Ok(());
    };
    let session = match state.token_auth.mode(origin) {
        CredentialMode::Token(session) => session,
        CredentialMode::Keys => return Ok(()),
        CredentialMode::Blocked(reason) => return Err(reason),
    };
    match api::delete_bot(&state.http_client, origin, &session.access, &record.pubkey).await {
        Ok(())
        | Err(ApiError {
            status: Some(404), ..
        }) => Ok(()),
        Err(error) => Err(format!("could not delete the agent on the server: {error}")),
    }
}

/// Delete on the server, then locally — never the reverse (Rule 1): a server
/// failure returns before `local` runs, so the agent stays to retry.
pub(crate) async fn server_then_local<T, Local, Fut>(
    server: impl std::future::Future<Output = Result<(), String>>,
    local: Local,
) -> Result<T, String>
where
    Local: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    server.await?;
    local().await
}

/// Sync a token-mode agent's profile through `PATCH /auth/bots/{id}/profile`.
/// `Ok(false)` when the community is on key auth (caller publishes kind:0).
/// In a Google-session community an agent that has not moved to its bot yet
/// is skipped: it registers with its current name on its next start.
pub(crate) async fn sync_bot_profile(
    state: &AppState,
    agent: &str,
    relay_url: &str,
    display_name: &str,
    avatar_url: Option<&str>,
) -> Result<bool, String> {
    let origin = origin_for(relay_url);
    let session = match state.token_auth.mode(&origin) {
        CredentialMode::Keys => return Ok(false),
        CredentialMode::Token(session) => session,
        CredentialMode::Blocked(reason) => return Err(reason),
    };
    let Some(app) = state.app_handle.lock().ok().and_then(|guard| guard.clone()) else {
        return Ok(true);
    };
    let is_bot_here = load_record(&app, agent)?
        .is_some_and(|record| record.bot_origin.as_deref() == Some(origin.as_str()));
    if !is_bot_here {
        return Ok(true);
    }
    let mut body =
        serde_json::json!({ "display_name": display_name.chars().take(64).collect::<String>() });
    if let Some(url) = avatar_url.filter(|u| u.starts_with("https://") || u.starts_with("http://"))
    {
        body["avatar_url"] = serde_json::Value::String(url.to_owned());
    }
    api::update_bot_profile(&state.http_client, &origin, &session.access, agent, body)
        .await
        .map(|()| true)
        .map_err(|e| format!("Could not sync the agent's profile: {e}"))
}

/// Register a new managed agent directly as a server bot when `relay_url`'s
/// community is signed in with Google (agent creation, plan §4.14). Returns
/// `(bot id, origin)`, or `None` on key auth (create a key agent as before).
pub(crate) async fn register_new_agent(
    state: &AppState,
    relay_url: &str,
    display_name: &str,
) -> Result<Option<(String, String)>, String> {
    let origin = origin_for(relay_url);
    let Some(session) = session_for(state, &origin)? else {
        return Ok(None);
    };
    let name: String = display_name.chars().take(64).collect();
    let name = if name.trim().is_empty() {
        "Agent".to_owned()
    } else {
        name
    };
    let bot_id = api::create_bot(&state.http_client, &origin, &session.access, &name)
        .await
        .map_err(|e| format!("could not register the agent on this community: {e}"))?;
    Ok(Some((bot_id, origin)))
}

/// Best-effort removal of a bot registered by [`register_new_agent`] whose
/// local record was never saved.
pub(crate) async fn discard_new_agent(state: &AppState, origin: &str, bot_id: &str) {
    if let Ok(Some(session)) = session_for(state, origin) {
        if api::delete_bot(&state.http_client, origin, &session.access, bot_id)
            .await
            .is_err()
        {
            eprintln!("buzz-desktop: auth: could not delete an unsaved agent's bot");
        }
    }
}

#[cfg(test)]
mod tests;
