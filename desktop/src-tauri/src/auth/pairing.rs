//! External agent pairing from the app (device-bound keys plan §7 and
//! §14.7, PR C2; no Approve step since 10-23).
//!
//! - "Connect external agent": [`start_agent_pairing`] asks the relay for a
//!   one-use, ten-minute `kuzz pair --relay <url> <code>` command; the screen
//!   polls [`list_agent_pairings`] until the first valid claim connects.
//! - Hermes on this computer: [`get_hermes_status`] lists its profiles;
//!   [`connect_hermes`] makes a code the same way for one of them and runs
//!   the bundled `kuzz pair --hermes --relay <url> <code>`. The CLI
//!   makes and keeps the agent key, signs with this computer's device key,
//!   saves the pairing and installs the Hermes plugin; the app keeps no
//!   agent key and then reads the pairing list like the connect screen.
//!   The app then restarts the Hermes gateway itself
//!   ([`restart_hermes_gateway`]): one started inside the pair command
//!   would die with that command's job.
//!   Offered only when Hermes, a `kuzz` with `--hermes` and the plugin files
//!   are all on this computer ([`hermes_installer`]).
//! - [`disconnect_external_device`] signs a paired computer out.
//!
//! Codes, commands and keys are never logged.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use zeroize::Zeroizing;

use super::api;
use crate::app_state::AppState;

/// The Hermes agent keys older builds kept in the app data folder; removed
/// at start (the `kuzz` CLI keeps Hermes keys now).
const LEGACY_HERMES_FILE: &str = "hermes-agents.json";
/// Longest a local pairing waits for the device key to open.
const DEVICE_WAIT: Duration = Duration::from_secs(3);
/// Longest the `kuzz pair --help` probe may take.
const HELP_PROBE_LIMIT: Duration = Duration::from_secs(5);
/// Longest `kuzz pair --hermes` (claim and plugin install) may take.
const HERMES_PAIR_LIMIT: Duration = Duration::from_secs(180);
/// Where `kuzz pair --hermes` finds the plugin installer under each of its
/// install roots (`crates/kuzz-cli/src/pairing.rs`, `HERMES_INSTALLER`).
const HERMES_INSTALLER: &str = "plugins/platforms/kuzz/install.py";
/// Inherited variables that would change what `kuzz pair` does: another
/// agent key to bind, or pairing switched off.
const CLEARED_ENV: [&str; 2] = ["KUZZ_PRIVATE_KEY", "KUZZ_DISABLE_STORED_SESSION"];
/// Hermes's name for the profile in its home folder.
const DEFAULT_PROFILE: &str = "default";

/// What the connect screen shows for a new code.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingStartView {
    pairing_id: String,
    /// `kuzz pair --relay <url> <code>` as the relay gave it, shown so the
    /// owner can copy it.
    command: String,
    expires_at: String,
    expires_in: i64,
}

/// One Hermes profile a person can connect: the default one and those they
/// made (the agents this app manages have their own profiles, not listed).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesProfileView {
    /// `default`, or the profile's folder name.
    name: String,
    is_default: bool,
    /// The agent this profile acts as (`KUZZ_PAIRED_AGENT` in its `.env`).
    paired_agent: Option<String>,
    /// The Kuzz account it was connected for (`KUZZ_OWNER_PUBKEY`).
    owner: Option<String>,
    /// Why Hermes would not run this profile, in Hermes's words.
    blocked_reason: Option<String>,
}

/// Whether Hermes is on this computer and whether this app can connect it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesStatus {
    found: bool,
    /// The bundled `kuzz` offers `pair --hermes` and the plugin files are
    /// where it looks for them.
    can_connect: bool,
    /// Default first; empty when Hermes isn't found.
    profiles: Vec<HermesProfileView>,
}

/// A Hermes pairing run by this app: its code's pairing id, and whether
/// `kuzz pair --hermes` finished without an error. The pairing list tells
/// whether it connected.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesPairingRun {
    pairing_id: String,
    finished: bool,
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

/// `kuzz pair --relay <url> <code>` and its expiry, for the connect screen.
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

/// Sign a paired computer out (`DELETE /auth/devices/{id}`).
#[tauri::command]
pub async fn disconnect_external_device(app: AppHandle, device_id: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = super::commands::fresh_session(&state, &origin).await?;
    api::revoke_device(&state.http_client, &origin, &session.access, &device_id)
        .await
        .map_err(api_error)
}

/// Whether Hermes is installed here and whether it can be connected.
#[tauri::command]
pub async fn get_hermes_status() -> HermesStatus {
    tokio::task::spawn_blocking(|| {
        let home = hermes_home();
        HermesStatus {
            found: home.is_some(),
            can_connect: home.is_some() && hermes_installer().is_some(),
            profiles: home.as_deref().map(hermes_profiles).unwrap_or_default(),
        }
    })
    .await
    .unwrap_or(HermesStatus {
        found: false,
        can_connect: false,
        profiles: Vec::new(),
    })
}

/// Pair Hermes on this computer: a new code, then the bundled
/// `kuzz pair --hermes --relay <url> <code>`, which claims it and installs
/// the plugin. `profile` is a name from [`get_hermes_status`], read again
/// here (never a path from the screen). Returns once the CLI is done; the
/// caller reads the pairing list for the result.
#[tauri::command]
pub async fn connect_hermes(app: AppHandle, profile: String) -> Result<HermesPairingRun, String> {
    let (kuzz, target) = tokio::task::spawn_blocking(move || {
        let root = hermes_home()?;
        let target = hermes_profile_target(&root, &profile)?;
        Some((hermes_installer()?, target))
    })
    .await
    .ok()
    .flatten()
    .ok_or("Hermes can't be connected from this app yet.")?;
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = super::commands::fresh_session(&state, &origin).await?;
    let _ = crate::device_key::ensure_within(DEVICE_WAIT).await;
    let started = api::pairing_start(&state.http_client, &origin, &session.access)
        .await
        .map_err(api_error)?;
    let relay = started
        .relay_url
        .clone()
        .filter(|relay| !relay.trim().is_empty())
        .unwrap_or_else(|| origin.clone());
    let code = Zeroizing::new(started.code.to_string());
    let finished = tokio::task::spawn_blocking(move || {
        let mut pair = std::process::Command::new(&kuzz);
        pair.args(hermes_pair_args(&relay, &code, target.as_deref()));
        for name in CLEARED_ENV {
            pair.env_remove(name);
        }
        crate::managed_agents::output_with_timeout(pair, HERMES_PAIR_LIMIT)
            .is_some_and(|output| output.status.success())
    })
    .await
    .unwrap_or(false);
    eprintln!("kuzz-desktop: hermes: kuzz pair finished (ok: {finished})");
    if finished {
        restart_hermes_gateway();
    }
    Ok(HermesPairingRun {
        pairing_id: started.pairing_id,
        finished,
    })
}

/// The `kuzz` argv for a Hermes pairing (no shell). Pure. `--no-restart`:
/// the app restarts the gateway after the pair command ends.
/// `profile_home`: a named profile's folder (`None`: the default profile).
fn hermes_pair_args(relay: &str, code: &str, profile_home: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["pair", "--hermes", "--no-restart"]
        .into_iter()
        .map(OsString::from)
        .collect();
    if let Some(home) = profile_home {
        args.push("--hermes-home".into());
        args.push(home.into());
    }
    args.extend(["--relay", relay, code].map(OsString::from));
    args
}

/// The profiles to offer under Hermes's `root` home: default first, then the
/// others by name. Reads two lines of each `.env`, nothing else.
fn hermes_profiles(root: &Path) -> Vec<HermesProfileView> {
    let blocked = serving_blocker(root);
    let mut names: Vec<String> = std::fs::read_dir(root.join("profiles"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| is_served_profile_dir(&entry.path()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| is_person_profile(name))
        .filter(|name| !root.join("profiles/.deleted").join(name).exists())
        .collect();
    names.sort();
    let default = profile_view(DEFAULT_PROFILE, root, true, None);
    std::iter::once(default)
        .chain(names.into_iter().map(|name| {
            let home = root.join("profiles").join(&name);
            profile_view(&name, &home, false, blocked.clone())
        }))
        .collect()
}

fn profile_view(
    name: &str,
    home: &Path,
    is_default: bool,
    blocked_reason: Option<String>,
) -> HermesProfileView {
    let env = std::fs::read(home.join(".env")).unwrap_or_default();
    let env = String::from_utf8_lossy(&env);
    let env = env.trim_start_matches('\u{feff}');
    HermesProfileView {
        name: name.to_owned(),
        is_default,
        paired_agent: env_pubkey(env, "KUZZ_PAIRED_AGENT"),
        owner: env_pubkey(env, "KUZZ_OWNER_PUBKEY"),
        blocked_reason,
    }
}

/// A profile folder the host gateway would serve: it has a config, and it
/// hasn't opted out (`gateway.standalone`) or been stopped (`gateway.parked`).
fn is_served_profile_dir(home: &Path) -> bool {
    let Ok(config) = std::fs::read_to_string(home.join("config.yaml")) else {
        return false;
    };
    let standalone = serde_yaml::from_str::<serde_yaml::Value>(&config)
        .ok()
        .and_then(|config| config.get("gateway")?.get("standalone")?.as_bool())
        .unwrap_or(false);
    !standalone && !home.join("gateway.parked").exists()
}

/// A profile id Hermes accepts that a person made: not one this app (or a
/// Buzz build before it) created for a managed agent. Pure.
fn is_person_profile(name: &str) -> bool {
    let id_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let valid = name.len() <= 64
        && name.chars().next().is_some_and(id_char)
        && name.chars().all(|c| id_char(c) || c == '_' || c == '-');
    let managed = ["kuzz-", "buzz-"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|hex| hex.len() == 16 && hex.chars().all(|c| c.is_ascii_hexdigit()))
    });
    valid && name != DEFAULT_PROFILE && !managed
}

/// The hex pubkey in `key=...` of a `.env` text. Pure.
fn env_pubkey(env: &str, key: &str) -> Option<String> {
    let line = env
        .lines()
        .find(|line| crate::managed_agents::hermes_profile::env_line_key(line) == Some(key))?;
    let value = line.split_once('=')?.1.trim().trim_matches(['"', '\'']);
    (value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

/// Why Hermes's gateway won't serve named profiles (Hermes's words, from its
/// state file). "Only one profile" clears on the restart after Connect.
fn serving_blocker(root: &Path) -> Option<String> {
    let state = std::fs::read(root.join("gateway_state.json")).ok()?;
    let state: serde_json::Value = serde_json::from_slice(&state).ok()?;
    blocker_reason(state.get("multiplex_standalone_reason")?.as_str()?)
}

fn blocker_reason(reason: &str) -> Option<String> {
    (!reason.trim().is_empty() && !reason.starts_with("only one profile"))
        .then(|| reason.to_owned())
}

/// Where to connect `profile`: `Some(None)` for the default profile,
/// `Some(Some(home))` for a named one, `None` when it isn't offered.
fn hermes_profile_target(root: &Path, profile: &str) -> Option<Option<PathBuf>> {
    let view = hermes_profiles(root)
        .into_iter()
        .find(|view| view.name == profile)?;
    if view.blocked_reason.is_some() {
        return None;
    }
    Some((!view.is_default).then(|| root.join("profiles").join(&view.name)))
}

/// Start `hermes gateway restart --detach` so the gateway loads the new
/// plugin and home channel, and leave it running: outside any job of this
/// app, no pipes, no window, without Kuzz settings or secrets in its env
/// (Hermes reads them from its own `.env`). Hermes refuses while it has
/// live work; the owner then restarts it later. Only the exit is logged.
fn restart_hermes_gateway() {
    let Some(home) = hermes_home() else {
        return;
    };
    let Some(hermes) = hermes_launcher(&home) else {
        eprintln!("kuzz-desktop: hermes: no hermes launcher; gateway not restarted");
        return;
    };
    let mut restart = std::process::Command::new(hermes);
    restart
        .args(["gateway", "restart", "--detach"])
        .env("HERMES_HOME", &home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for (name, _) in std::env::vars_os() {
        if is_kuzz_setting(&name) {
            restart.env_remove(name);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        restart.creation_flags(CREATE_NO_WINDOW);
    }
    match restart.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let status = child.wait().ok().and_then(|status| status.code());
                eprintln!("kuzz-desktop: hermes: gateway restart exited ({status:?})");
            });
        }
        Err(error) => eprintln!("kuzz-desktop: hermes: gateway restart failed to start: {error}"),
    }
}

/// The `hermes` launcher: on `PATH`, else under the Hermes home. On Windows
/// only `hermes.exe` (a `.cmd`/`.bat` is never run), as the plugin installer.
fn hermes_launcher(home: &Path) -> Option<PathBuf> {
    let name = format!("hermes{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    let in_home = [
        home.join("bin"),
        home.join("hermes-agent").join("venv").join("Scripts"),
        home.join("hermes-agent").join("venv").join("bin"),
    ];
    on_path
        .into_iter()
        .chain(in_home)
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
}

/// Whether an inherited variable is a Kuzz/Buzz setting a restarted gateway
/// must not inherit. Pure.
fn is_kuzz_setting(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        let upper = name.to_ascii_uppercase();
        upper.starts_with("KUZZ_") || upper.starts_with("BUZZ_")
    })
}

/// Hermes's home on this computer, when that folder exists: `$HERMES_HOME`,
/// else `%LOCALAPPDATA%\hermes` on Windows, else `~/.hermes` (where
/// `kuzz pair --hermes` installs).
pub(crate) fn hermes_home() -> Option<PathBuf> {
    hermes_home_from(
        |name| std::env::var_os(name),
        dirs::home_dir().as_deref(),
        Path::is_dir,
    )
}

/// [`hermes_home`] reading variables through `var`, the user's home from
/// `home`, and folders through `is_dir`. Pure.
fn hermes_home_from(
    var: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
    is_dir: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let absolute = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let fallback = if cfg!(windows) {
        absolute("LOCALAPPDATA").map(|base| base.join("hermes"))
    } else {
        home.map(|home| home.join(".hermes"))
    };
    [absolute("HERMES_HOME"), fallback]
        .into_iter()
        .flatten()
        .find(|path| is_dir(path))
}

/// The bundled `kuzz`, when it offers `pair --hermes` and the Hermes plugin
/// installer is in one of its install roots. Blocks for at most
/// [`HELP_PROBE_LIMIT`].
fn hermes_installer() -> Option<PathBuf> {
    let kuzz = bundled_kuzz()?;
    let data = dirs::data_local_dir();
    let has_plugin = hermes_plugin_roots(&kuzz, data.as_deref())
        .into_iter()
        .any(|root| root.join(HERMES_INSTALLER).is_file());
    if !has_plugin {
        return None;
    }
    let mut probe = std::process::Command::new(&kuzz);
    probe.args(["pair", "--help"]);
    let help = crate::managed_agents::output_with_timeout(probe, HELP_PROBE_LIMIT)?;
    lists_hermes_flag(&String::from_utf8_lossy(&help.stdout)).then_some(kuzz)
}

/// The `kuzz` shipped next to this app (in development builds, the one the
/// agents resolve, usually the workspace build).
fn bundled_kuzz() -> Option<PathBuf> {
    let name = format!("kuzz{}", std::env::consts::EXE_SUFFIX);
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .filter(|path| path.is_file());
    if beside.is_some() || !cfg!(debug_assertions) {
        return beside;
    }
    crate::managed_agents::resolve_command("kuzz")
}

/// Where `kuzz` at `kuzz` looks for `plugins/`, as its
/// `hermes_installer_roots`: its own folder, `<prefix>/share/kuzz` beside a
/// `<prefix>/bin`, and `<local data>/kuzz`. Pure.
fn hermes_plugin_roots(kuzz: &Path, local_data: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = kuzz.parent() {
        roots.push(dir.to_path_buf());
        if let Some(prefix) = dir.parent().filter(|_| dir.ends_with("bin")) {
            roots.push(prefix.join("share").join("kuzz"));
        }
    }
    if let Some(data) = local_data {
        roots.push(data.join("kuzz"));
    }
    roots
}

/// Whether `kuzz pair --help` output offers every flag of
/// [`hermes_pair_args`] (an older `kuzz` would refuse them). Pure.
fn lists_hermes_flag(help: &str) -> bool {
    let words: Vec<&str> = help
        .split(|c: char| c.is_whitespace() || c == ',' || c == '=' || c == '[' || c == ']')
        .collect();
    ["--hermes", "--no-restart", "--hermes-home"]
        .iter()
        .all(|flag| words.contains(flag))
}

/// Delete the Hermes agent keys older builds of this app kept, once at
/// start. Their agents stay paired until disconnected.
pub(crate) fn remove_legacy_hermes_keys<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    tauri::async_runtime::spawn_blocking(move || remove_legacy_hermes_file(&dir));
}

fn remove_legacy_hermes_file(dir: &Path) {
    match std::fs::remove_file(dir.join(LEGACY_HERMES_FILE)) {
        Ok(()) => eprintln!("kuzz-desktop: hermes: removed the old Hermes key file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!("kuzz-desktop: hermes: old Hermes key file not removed: {error}"),
    }
}

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;
