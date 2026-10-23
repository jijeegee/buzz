//! External agent pairing from the app (device-bound keys plan §7 and
//! §14.7, PR C2; no Approve step since 10-23).
//!
//! - "Connect external agent": [`start_agent_pairing`] asks the relay for a
//!   one-use, ten-minute `kuzz pair --relay <url> <code>` command; the screen
//!   polls [`list_agent_pairings`] until the first valid claim connects.
//! - Hermes on this computer: [`connect_hermes`] makes a code the same way
//!   and runs the bundled `kuzz pair --hermes --relay <url> <code>`. The CLI
//!   makes and keeps the agent key, signs with this computer's device key,
//!   saves the pairing and installs the Hermes plugin; the app keeps no
//!   agent key and then reads the pairing list like the connect screen.
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

/// Whether Hermes is on this computer and whether this app can connect it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesStatus {
    found: bool,
    /// The bundled `kuzz` offers `pair --hermes` and the plugin files are
    /// where it looks for them.
    can_connect: bool,
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
        let found = hermes_home().is_some();
        HermesStatus {
            found,
            can_connect: found && hermes_installer().is_some(),
        }
    })
    .await
    .unwrap_or(HermesStatus {
        found: false,
        can_connect: false,
    })
}

/// Pair Hermes on this computer: a new code, then the bundled
/// `kuzz pair --hermes --relay <url> <code>`, which claims it and installs
/// the plugin. Returns once the CLI is done; the caller reads the pairing
/// list for the result.
#[tauri::command]
pub async fn connect_hermes(app: AppHandle) -> Result<HermesPairingRun, String> {
    let kuzz = tokio::task::spawn_blocking(|| hermes_home().and_then(|_| hermes_installer()))
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
        pair.args(hermes_pair_args(&relay, &code));
        for name in CLEARED_ENV {
            pair.env_remove(name);
        }
        crate::managed_agents::output_with_timeout(pair, HERMES_PAIR_LIMIT)
            .is_some_and(|output| output.status.success())
    })
    .await
    .unwrap_or(false);
    eprintln!("kuzz-desktop: hermes: kuzz pair finished (ok: {finished})");
    Ok(HermesPairingRun {
        pairing_id: started.pairing_id,
        finished,
    })
}

/// The `kuzz` argv for a Hermes pairing (no shell). Pure.
fn hermes_pair_args(relay: &str, code: &str) -> Vec<OsString> {
    ["pair", "--hermes", "--relay", relay, code]
        .into_iter()
        .map(OsString::from)
        .collect()
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

/// Whether `kuzz pair --help` output offers `--hermes`. Pure.
fn lists_hermes_flag(help: &str) -> bool {
    help.split(|c: char| c.is_whitespace() || c == ',' || c == '=' || c == '[' || c == ']')
        .any(|word| word == "--hermes")
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
