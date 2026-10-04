// ── Per-agent Hermes profile ──────────────────────────────────────────────────
//
// Every Hermes entry point (`hermes`, `hermes-acp`) reads exactly one config
// root: `HERMES_HOME`. Left alone, every Buzz-managed Hermes agent would share
// the user's default root — one SOUL.md, one memory store, one `.env`. Hermes
// loads `<HERMES_HOME>/.env` with override semantics, so a `BUZZ_PRIVATE_KEY`
// written there by a separately-run Hermes gateway would replace the per-agent
// key the Desktop passes and the agent would sign as someone else.
//
// So each Buzz-managed Hermes agent gets its own Hermes profile, named
// `buzz-<first 16 hex chars of its pubkey>`, created lazily at spawn through
// Hermes's own CLI and passed to the child as `HERMES_HOME`. The profile lives
// in Hermes's layout (`<root>/profiles/<name>`) rather than a Buzz-owned
// directory: Hermes only falls back to the root `auth.json` (provider OAuth)
// when `HERMES_HOME`'s parent directory is named `profiles`, which is what
// keeps an existing login working without a second sign-in.
//
// Like `harness_max_parallelism`, this is command-keyed execution policy for a
// preset harness, not a `KnownAcpRuntime` capability fact.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use super::ManagedAgentRecord;

/// Env var every Hermes entry point reads its config root from.
pub(crate) const HERMES_HOME_ENV_VAR: &str = "HERMES_HOME";

/// Prefix of every Buzz-owned Hermes profile name. It keeps the names clear of
/// Hermes's reserved profile names (`default`, `hermes`, `root`, …) and makes
/// the deletion guard able to recognise a Buzz-owned directory by name alone.
const PROFILE_PREFIX: &str = "buzz-";

/// Number of pubkey hex characters carried in the profile name (64 bits).
const PROFILE_PUBKEY_CHARS: usize = 16;

/// Hermes's own subdirectory for named profiles under its root.
const PROFILES_DIR: &str = "profiles";

/// Upper bound for one `hermes profile create|delete` run. Creation seeds the
/// bundled skills under Hermes's own 60s timeout; this leaves room for CLI
/// startup on top, so a slow-but-healthy run is not killed half-seeded.
const PROFILE_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Upper bound on the Hermes output quoted back in an error message.
const ERROR_EXCERPT_CHARS: usize = 400;

/// Serializes profile creation and deletion across concurrent spawns, so two
/// relays starting the same agent cannot race `hermes profile create` and see
/// the loser's "already exists" as a failure.
static PROFILE_LOCK: Mutex<()> = Mutex::new(());

/// A Buzz-managed agent's Hermes profile: where Hermes keeps it and what it is
/// called. Construction does not touch the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HermesProfile {
    /// Profile name as Hermes knows it (`buzz-<16 hex>`).
    pub name: String,
    /// The Hermes root the profile belongs to; passed to the Hermes CLI as
    /// `HERMES_HOME` so Hermes and Buzz agree on where profiles live.
    pub root: PathBuf,
    /// `<root>/profiles/<name>`; the agent's `HERMES_HOME`.
    pub dir: PathBuf,
    /// The home a Hermes agent used before it had a profile (the Desktop's
    /// own `HERMES_HOME`, else the root). A new profile's `.env` is seeded
    /// from this home's `.env`, minus Buzz-owned keys.
    pub source_home: PathBuf,
}

impl HermesProfile {
    /// The profile for `pubkey` under the Hermes root this Desktop process
    /// resolves (see [`resolve_hermes_root`]).
    pub(crate) fn for_pubkey(pubkey: &str) -> Result<Self, String> {
        let desktop_home = std::env::var_os(HERMES_HOME_ENV_VAR).filter(|home| !home.is_empty());
        let root = resolve_hermes_root(desktop_home.as_deref(), native_hermes_home())
            .ok_or_else(|| "could not determine the Hermes home directory".to_string())?;
        let mut profile = Self::under_root(pubkey, root)?;
        if let Some(home) = desktop_home {
            profile.source_home = PathBuf::from(home);
        }
        Ok(profile)
    }

    fn under_root(pubkey: &str, root: PathBuf) -> Result<Self, String> {
        let name = profile_name(pubkey)
            .ok_or_else(|| format!("agent pubkey {pubkey:?} is not a hex public key"))?;
        let dir = profile_dir(&root, &name);
        Ok(Self {
            name,
            source_home: root.clone(),
            root,
            dir,
        })
    }
}

/// Whether `command` launches Hermes, so its child must get a per-agent
/// `HERMES_HOME`.
///
/// Matches the normalized basename (path, case, and `.exe`/`.cmd`/`.bat`
/// ignored) against every Hermes entry point — `hermes-acp`, and also `hermes`
/// and `hermes-agent`, the same identity set `buzz-acp` keys its Hermes
/// defaults on. All of them read `HERMES_HOME`, so a custom harness that drives
/// `hermes` directly needs the same isolation as the preset.
pub(crate) fn is_hermes_command(command: &str) -> bool {
    let identity = super::discovery::normalize_command_identity(command);
    let identity = [".cmd", ".bat"]
        .iter()
        .find_map(|suffix| identity.strip_suffix(suffix))
        .unwrap_or(&identity);
    matches!(identity, "hermes-acp" | "hermes" | "hermes-agent")
}

/// `buzz-<first 16 hex chars of pubkey>`, or `None` when `pubkey` is not at
/// least 16 hex characters. Lowercased, so the result always satisfies Hermes's
/// profile-name rule `^[a-z0-9][a-z0-9_-]{0,63}$`.
pub(crate) fn profile_name(pubkey: &str) -> Option<String> {
    let prefix = pubkey.get(..PROFILE_PUBKEY_CHARS)?;
    if !prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!("{PROFILE_PREFIX}{}", prefix.to_ascii_lowercase()))
}

/// `<root>/profiles/<name>` — Hermes's own layout for named profiles.
pub(crate) fn profile_dir(root: &Path, name: &str) -> PathBuf {
    root.join(PROFILES_DIR).join(name)
}

/// The Hermes root directory, mirroring Hermes's `get_default_hermes_root` so
/// Buzz and the Hermes CLI always agree on where `profiles/` lives:
///
/// - no (or empty) `HERMES_HOME` -> the native home;
/// - `HERMES_HOME` anywhere under the native home (the home itself, one of its
///   profiles, or any other subdirectory) -> the native home;
/// - otherwise (a custom root) -> `<root>` for `<root>/profiles/<name>`, else
///   `HERMES_HOME` itself.
pub(crate) fn resolve_hermes_root(
    hermes_home: Option<&OsStr>,
    native_home: Option<PathBuf>,
) -> Option<PathBuf> {
    let Some(home) = hermes_home.filter(|value| !value.is_empty()) else {
        return native_home;
    };
    let home = PathBuf::from(home);
    if let Some(native) = native_home {
        if is_under(&home, &native) {
            return Some(native);
        }
    }
    let profiles_parent = home
        .parent()
        .filter(|parent| parent.file_name() == Some(OsStr::new(PROFILES_DIR)))
        .and_then(Path::parent);
    Some(profiles_parent.map(Path::to_path_buf).unwrap_or(home))
}

/// `path` is `base` or inside it: compared resolved when both exist (as
/// Hermes's `Path.resolve()` does), lexically otherwise.
fn is_under(path: &Path, base: &Path) -> bool {
    match (path.canonicalize(), base.canonicalize()) {
        (Ok(path), Ok(base)) => path.starts_with(base),
        _ => path.starts_with(base),
    }
}

/// Hermes's native home: `%LOCALAPPDATA%\hermes` on Windows, `~/.hermes`
/// elsewhere.
fn native_hermes_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::data_local_dir)
            .map(|dir| dir.join("hermes"))
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir().map(|home| home.join(".hermes"))
    }
}

/// Whether Hermes finished creating the profile. A profile cloned from a root
/// with a `config.yaml` gets one; a profile under a root without one (Hermes
/// never set up) still gets `SOUL.md`. Either means `hermes profile create`
/// ran to completion and must not be re-run (it would fail "already exists").
fn profile_is_created(dir: &Path) -> bool {
    dir.join("config.yaml").is_file() || dir.join("SOUL.md").is_file()
}

/// The per-profile secrets file Hermes loads with override semantics.
const PROFILE_ENV_FILE: &str = ".env";

/// Whether the profile is safe to launch: created, and carrying its own `.env`.
///
/// The `.env` is load-bearing. `hermes update` backfills a profile that lacks
/// one by copying the ROOT `.env` (the very file whose `BUZZ_PRIVATE_KEY` this
/// module isolates agents from), so a profile without its own `.env` is one
/// Hermes upgrade away from signing with the wrong key.
fn profile_is_ready(dir: &Path) -> bool {
    profile_is_created(dir) && dir.join(PROFILE_ENV_FILE).is_file()
}

/// Header of a `.env` Buzz writes into a profile.
const PROFILE_ENV_HEADER: &str =
    "# Hermes secrets for this Buzz agent. Buzz-owned keys (BUZZ_*, HERMES_HOME)\n\
     # are set by Buzz Desktop at launch and must not be added here.\n";

/// Owner-only (on Unix) open options for a new profile `.env`.
fn private_file_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
}

/// Give an existing profile a placeholder `.env` unless it already has one.
/// Never overwrites: a user may keep the agent's own provider keys there.
fn ensure_profile_env(dir: &Path) -> Result<(), String> {
    let path = dir.join(PROFILE_ENV_FILE);
    let opened = private_file_options().create_new(true).open(&path);
    match opened {
        Ok(mut file) => std::io::Write::write_all(&mut file, PROFILE_ENV_HEADER.as_bytes())
            .map_err(|error| format!("failed to write {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("failed to create {}: {error}", path.display())),
    }
}

/// Seed a freshly created profile's `.env` from the agent's previous home,
/// minus every Buzz-owned key, so provider keys keep working while the shared
/// `BUZZ_PRIVATE_KEY` (the bug this module fixes) stays behind.
///
/// Replaces whatever placeholder `hermes profile create` wrote; only called
/// for a profile this start just created. Written to a temp file and renamed,
/// so a crash never leaves a half-written `.env`. Values are never logged.
fn seed_profile_env(profile: &HermesProfile) -> Result<(), String> {
    let source = profile.source_home.join(PROFILE_ENV_FILE);
    let seeded = match std::fs::read_to_string(&source) {
        Ok(contents) => strip_buzz_owned_env(&contents),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "buzz-desktop: not seeding Hermes profile {} from {}: {error}",
                    profile.name,
                    source.display()
                );
            }
            String::new()
        }
    };
    let path = profile.dir.join(PROFILE_ENV_FILE);
    let temp = profile.dir.join(".env.buzz-seed");
    let write = || -> std::io::Result<()> {
        let mut file = private_file_options()
            .create(true)
            .truncate(true)
            .open(&temp)?;
        std::io::Write::write_all(&mut file, PROFILE_ENV_HEADER.as_bytes())?;
        std::io::Write::write_all(&mut file, seeded.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, &path)
    };
    write().map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        format!("failed to write {}: {error}", path.display())
    })
}

/// Whether a `.env` key belongs to Buzz rather than to the agent's Hermes:
/// any `BUZZ_*` key, every reserved launch key, and `HERMES_HOME`.
fn is_buzz_owned_env_key(key: &str) -> bool {
    key.to_ascii_uppercase().starts_with("BUZZ_")
        || key.eq_ignore_ascii_case(HERMES_HOME_ENV_VAR)
        || super::is_reserved_env_key(key)
}

/// The key of a dotenv assignment line (`KEY=value` or `export KEY=value`),
/// or `None` for blanks, comments, and lines without `=`.
fn env_line_key(line: &str) -> Option<&str> {
    let line = line.trim_start();
    if line.starts_with('#') {
        return None;
    }
    let line = line
        .strip_prefix("export")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .unwrap_or(line);
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    (!key.is_empty()).then_some(key)
}

/// The quote a dotenv value opens on `line` without closing, if any: such a
/// value continues on the following lines up to the closing quote.
fn unclosed_quote(line: &str) -> Option<char> {
    let (_, value) = line.split_once('=')?;
    let value = value.trim_start();
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let mut escaped = false;
    for c in value[1..].chars() {
        match c {
            '\\' if quote == '"' && !escaped => escaped = true,
            c if c == quote && !escaped => return None,
            _ => escaped = false,
        }
    }
    Some(quote)
}

/// `.env` contents with every Buzz-owned assignment removed. Kept assignments
/// are copied verbatim (an `export` prefix, quoting, and multi-line quoted
/// values included); comments, blanks, and malformed lines are dropped.
pub(crate) fn strip_buzz_owned_env(contents: &str) -> String {
    let mut kept = String::new();
    let mut lines = contents.lines();
    while let Some(line) = lines.next() {
        let mut entry = line.to_string();
        if let Some(quote) = env_line_key(line).and_then(|_| unclosed_quote(line)) {
            for next in lines.by_ref() {
                entry.push('\n');
                entry.push_str(next);
                if next.contains(quote) {
                    break;
                }
            }
        }
        match env_line_key(line) {
            Some(key) if !is_buzz_owned_env_key(key) => {
                kept.push_str(&entry);
                kept.push('\n');
            }
            _ => {}
        }
    }
    kept
}

/// Whether `dir` is exactly a Buzz-owned profile directory under `root`. The
/// deletion guard: a mis-resolved root or name can never widen a recursive
/// delete beyond `<root>/profiles/buzz-<16 lowercase hex>`.
pub(crate) fn is_buzz_profile_dir(root: &Path, dir: &Path) -> bool {
    let Some(name) = dir.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let Some(hex) = name.strip_prefix(PROFILE_PREFIX) else {
        return false;
    };
    hex.len() == PROFILE_PUBKEY_CHARS
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && dir == profile_dir(root, name)
}

/// The agent name shown in Hermes profile errors and descriptions.
pub(crate) fn agent_label(record: &ManagedAgentRecord) -> &str {
    record.display_name.as_deref().unwrap_or(&record.name)
}

fn refusal(agent_label: &str, profile: &str, reason: &str) -> String {
    format!(
        "cannot start Hermes agent {agent_label}: failed to create Hermes profile {profile}: {reason}"
    )
}

fn profile_for(pubkey: &str, agent_label: &str) -> Result<HermesProfile, String> {
    HermesProfile::for_pubkey(pubkey).map_err(|reason| {
        refusal(
            agent_label,
            &profile_name(pubkey).unwrap_or_default(),
            &reason,
        )
    })
}

/// Create (or repair) the agent's Hermes profile ahead of a start.
///
/// Every start entry point calls this BEFORE taking the runtime-transition,
/// store, or process locks: the Hermes CLI can take seconds, and nothing else
/// may wait on it. `spawn_agent_child`, which runs under those locks, only
/// checks the result via [`require_for_spawn`].
///
/// The harness is resolved both from the record as stored and with the linked
/// persona re-snapshotted onto it, because some callers re-snapshot before
/// spawning and some do not, and `record.runtime` outranks the persona's
/// runtime. Either one resolving to Hermes prepares the profile, so the spawn
/// check passes whichever the caller's spawn sees. A record whose harness
/// cannot be resolved at all is left to the spawn path, which refuses it with
/// its own error.
pub(crate) fn prepare_for_start<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    record: &ManagedAgentRecord,
) -> Result<(), String> {
    let personas = super::load_personas(app).unwrap_or_default();
    let global = super::load_global_agent_config(app).unwrap_or_default();
    let Some(hermes_command) = hermes_command_for_start(record, &personas, &global) else {
        return Ok(());
    };
    let label = agent_label(record);
    let profile = profile_for(&record.pubkey, label)?;
    ensure_profile(
        &profile,
        || find_hermes_cli(&hermes_command),
        &format!("Buzz agent: {label}"),
    )
    .map_err(|reason| refusal(label, &profile.name, &reason))
}

/// The Hermes agent command a start of `record` may spawn, if any: resolved
/// from the record as stored and from the record with its persona
/// re-snapshotted (see [`prepare_for_start`]).
pub(crate) fn hermes_command_for_start(
    record: &ManagedAgentRecord,
    personas: &[super::AgentDefinition],
    global: &super::GlobalAgentConfig,
) -> Option<String> {
    let resnapshotted = record.persona_id.as_deref().and_then(|persona_id| {
        let persona = personas.iter().find(|persona| persona.id == persona_id)?;
        let mut record = record.clone();
        super::persona_events::apply_persona_snapshot(&mut record, persona);
        Some(record)
    });
    let command = [Some(record), resnapshotted.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|candidate| {
            super::resolve_effective_harness_descriptor(candidate, personas, global).ok()
        })
        .map(|descriptor| descriptor.command)
        .find(|command| is_hermes_command(command));
    command
}

/// The profile a spawn of `effective_command` must launch with: `Ok(None)` for
/// non-Hermes harnesses, otherwise the agent's profile, which
/// [`prepare_for_start`] must already have made ready.
///
/// Check-only (no CLI, no writes) because it runs under the spawn locks.
/// Fail-closed: a missing or incomplete profile refuses the spawn rather than
/// falling back to the shared root this module exists to keep agents out of.
pub(crate) fn require_for_spawn(
    effective_command: &str,
    pubkey: &str,
    agent_label: &str,
) -> Result<Option<HermesProfile>, String> {
    if !is_hermes_command(effective_command) {
        return Ok(None);
    }
    let profile = profile_for(pubkey, agent_label)?;
    check_ready(&profile).map_err(|reason| refusal(agent_label, &profile.name, &reason))?;
    Ok(Some(profile))
}

fn check_ready(profile: &HermesProfile) -> Result<(), String> {
    if profile_is_ready(&profile.dir) {
        return Ok(());
    }
    Err(format!(
        "{} is not ready (it needs config.yaml or SOUL.md, and .env); start the agent again to repair it",
        profile.dir.display()
    ))
}

/// Write the user-env layer (`descriptor.env`) onto the child, then point it at
/// its Hermes profile.
///
/// One function on purpose: the order is the guarantee. `HERMES_HOME` goes on
/// AFTER every user value so nothing saved can redirect a Hermes agent back to
/// the shared root. (It is also a reserved key, so user env should never carry
/// it; this is the second line of defence.)
pub(crate) fn apply_user_env_then_hermes_home<'a>(
    command: &mut Command,
    user_env: impl IntoIterator<Item = (&'a String, &'a String)>,
    profile: Option<&HermesProfile>,
) {
    for (key, value) in user_env {
        command.env(key, value);
    }
    if let Some(profile) = profile {
        command.env(HERMES_HOME_ENV_VAR, &profile.dir);
    }
}

/// Create `profile` with the Hermes CLI unless it already exists, verify Hermes
/// actually produced it, and make sure it has its own `.env`: seeded from the
/// previous home for a new profile, a placeholder only if an existing one
/// lost it. `.env` is part of readiness, so it is in place before the profile
/// counts as ready.
///
/// `hermes` is only resolved when the CLI is actually needed, so a start of an
/// existing profile costs no command lookup.
pub(crate) fn ensure_profile(
    profile: &HermesProfile,
    hermes: impl FnOnce() -> Option<PathBuf>,
    description: &str,
) -> Result<(), String> {
    let _guard = PROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if profile_is_created(&profile.dir) {
        ensure_profile_env(&profile.dir)?;
    } else {
        create_profile(profile, hermes, description)?;
        seed_profile_env(profile)?;
    }
    check_ready(profile)
}

fn create_profile(
    profile: &HermesProfile,
    hermes: impl FnOnce() -> Option<PathBuf>,
    description: &str,
) -> Result<(), String> {
    let hermes = hermes().ok_or_else(|| {
        "the `hermes` command was not found next to `hermes-acp` or on PATH".to_string()
    })?;
    let mut command = hermes_cli_command(&hermes, &profile.root);
    command
        .args([
            "profile",
            "create",
            &profile.name,
            "--no-alias",
            "--description",
        ])
        .arg(sanitize_description(description));
    let result = run_hermes_cli(command, "hermes profile create");
    // A failed run is still a success if the profile is there afterwards (for
    // example, created by a concurrent Desktop instance).
    if profile_is_created(&profile.dir) {
        return Ok(());
    }
    result?;
    Err(format!(
        "Hermes reported success but {} was not created",
        profile.dir.display()
    ))
}

/// Best-effort removal of `profile` when its agent is deleted. Never fails the
/// caller: a profile left behind is reported on stderr and is harmless (it is
/// re-used if an agent with the same key is ever recreated).
pub(crate) fn delete_profile(profile: &HermesProfile, hermes: impl FnOnce() -> Option<PathBuf>) {
    let _guard = PROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !profile.dir.exists() {
        return;
    }
    if !is_buzz_profile_dir(&profile.root, &profile.dir) {
        eprintln!(
            "buzz-desktop: refusing to delete {}: not a Buzz-owned Hermes profile directory",
            profile.dir.display()
        );
        return;
    }
    if let Some(hermes) = hermes() {
        let mut command = hermes_cli_command(&hermes, &profile.root);
        command.args(["profile", "delete", &profile.name, "-y"]);
        if let Err(error) = run_hermes_cli(command, "hermes profile delete") {
            eprintln!(
                "buzz-desktop: {error}; removing Hermes profile {} directly",
                profile.name
            );
        }
    }
    if profile.dir.exists() {
        if let Err(error) = std::fs::remove_dir_all(&profile.dir) {
            eprintln!(
                "buzz-desktop: failed to remove Hermes profile directory {}: {error}",
                profile.dir.display()
            );
        }
    }
}

/// Remove the Hermes profile of a deleted agent, if it has one.
///
/// Keyed on the profile directory rather than the agent's current harness: the
/// name is Buzz-owned and derived from the pubkey, so an agent that ran on
/// Hermes and later switched harness still has its profile cleaned up.
///
/// Both agent-removal paths call this (`delete_managed_agent` and the
/// `delete_persona` cascade) after the store lock is released.
pub(crate) fn delete_profile_for_deleted_agent(pubkey: &str) {
    match HermesProfile::for_pubkey(pubkey) {
        Ok(profile) => delete_profile(&profile, || find_hermes_cli("hermes-acp")),
        Err(error) => {
            eprintln!("buzz-desktop: skipping Hermes profile cleanup for {pubkey}: {error}")
        }
    }
}

/// Keep a Hermes agent's profile (memory, sessions, seeded `.env`) when its
/// pubkey changes because it moved onto its server bot (`auth::bots::adopt`):
/// rename `buzz-<old>` to `buzz-<new>`. Best-effort — on failure the next
/// start creates a fresh profile for the new pubkey and the old one stays.
pub(crate) fn move_profile_for_new_pubkey(from: &str, to: &str) {
    let moved = HermesProfile::for_pubkey(from)
        .and_then(|old| HermesProfile::for_pubkey(to).map(|new| (old, new)))
        .and_then(|(old, new)| move_profile_dir(&old, &new));
    if let Err(error) = moved {
        eprintln!("buzz-desktop: keeping a new Hermes profile for moved agent {to}: {error}");
    }
}

/// Rename `from`'s profile directory to `to`'s. `Ok(false)` when there is
/// nothing to move or `to` already exists (never overwritten).
pub(crate) fn move_profile_dir(from: &HermesProfile, to: &HermesProfile) -> Result<bool, String> {
    let _guard = PROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !from.dir.is_dir() || to.dir.exists() {
        return Ok(false);
    }
    if !is_buzz_profile_dir(&from.root, &from.dir) || !is_buzz_profile_dir(&to.root, &to.dir) {
        return Err("not a Buzz-owned Hermes profile directory".into());
    }
    std::fs::rename(&from.dir, &to.dir)
        .map(|()| true)
        .map_err(|error| format!("rename {}: {error}", from.dir.display()))
}

/// The `hermes` CLI to manage profiles with: the sibling of the resolved Hermes
/// agent command (the installer puts `hermes` and `hermes-acp` side by side,
/// and a custom-path install may not be on PATH), else `hermes` on PATH.
pub(crate) fn find_hermes_cli(agent_command: &str) -> Option<PathBuf> {
    let sibling = super::resolve_command(agent_command)
        .and_then(|agent| agent.parent().map(Path::to_path_buf))
        .and_then(|dir| {
            hermes_cli_file_names()
                .iter()
                .map(|name| dir.join(name))
                .find(|candidate| candidate.is_file())
        });
    sibling.or_else(|| super::resolve_command("hermes"))
}

#[cfg(windows)]
fn hermes_cli_file_names() -> &'static [&'static str] {
    &["hermes.exe", "hermes.cmd", "hermes.bat"]
}

#[cfg(not(windows))]
fn hermes_cli_file_names() -> &'static [&'static str] {
    &["hermes"]
}

/// A Hermes CLI invocation rooted at `root`. Python's stdio falls back to the
/// ANSI code page when piped on Windows, and Hermes prints non-ASCII status
/// glyphs, so UTF-8 is forced to keep a successful run from dying on output.
fn hermes_cli_command(hermes: &Path, root: &Path) -> Command {
    let mut command = Command::new(hermes);
    command
        .env(HERMES_HOME_ENV_VAR, root)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    command
}

/// Run a Hermes CLI command under the bounded runner (null stdin, capped
/// output, `CREATE_NO_WINDOW` and whole-tree teardown on Windows).
fn run_hermes_cli(command: Command, what: &str) -> Result<(), String> {
    let output = super::discovery::output_with_timeout(command, PROFILE_COMMAND_TIMEOUT)
        .ok_or_else(|| {
            format!(
                "`{what}` could not be run or did not finish within {}s",
                PROFILE_COMMAND_TIMEOUT.as_secs()
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let excerpt: String = detail.chars().take(ERROR_EXCERPT_CHARS).collect();
    Err(format!("`{what}` failed ({}): {excerpt}", output.status))
}

/// One line of printable text: a display name with control characters would
/// otherwise reach Hermes's YAML profile metadata verbatim.
fn sanitize_description(description: &str) -> String {
    description
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
#[path = "hermes_profile_tests.rs"]
mod tests;
