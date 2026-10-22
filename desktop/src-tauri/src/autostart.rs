//! Open Kuzz when the computer starts (Windows and macOS).
//!
//! On by default: the first start of an installed build turns it on once and
//! leaves a marker, so turning it off in Settings stays off. A start made by
//! the OS carries [`AUTOSTART_ARG`] and stays in the tray (menu bar on macOS)
//! without opening the window; agents start as usual.

use std::path::Path;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::ManagerExt;

/// Passed by the OS login entry, so the window stays hidden.
pub(crate) const AUTOSTART_ARG: &str = "--autostart";

/// Written once the default has been applied; never removed.
const DEFAULT_APPLIED_MARKER: &str = "autostart-default-applied";

/// True when the OS started this process at login.
pub(crate) fn launched_at_login() -> bool {
    cfg!(any(target_os = "macos", windows)) && std::env::args().any(|arg| arg == AUTOSTART_ARG)
}

/// Turns the login entry on the first time an installed build starts.
/// Development builds never register one: it would start the dev app at
/// login next to the installed one.
pub(crate) fn apply_default<R: Runtime>(app: &AppHandle<R>, data_dir: &Path) {
    if cfg!(debug_assertions) || !cfg!(any(target_os = "macos", windows)) {
        return;
    }
    let is_dev = data_dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(crate::migration::is_dev_data_dir_name);
    if is_dev {
        return;
    }
    let marker = data_dir.join(DEFAULT_APPLIED_MARKER);
    if marker.exists() {
        return;
    }
    if let Err(error) = app.autolaunch().enable() {
        eprintln!("kuzz-desktop: could not turn on open at login: {error}");
        return;
    }
    write_marker(&marker);
}

fn write_marker(marker: &Path) {
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(marker, b"1") {
        eprintln!("kuzz-desktop: could not save open-at-login default: {error}");
    }
}

#[tauri::command]
pub fn get_open_at_login<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_open_at_login<R: Runtime>(app: AppHandle<R>, enabled: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    }
    .map_err(|error| error.to_string())?;
    // A choice made here is final: the default never turns it back on.
    if let Ok(data_dir) = app.path().app_data_dir() {
        write_marker(&data_dir.join(DEFAULT_APPLIED_MARKER));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_is_written_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("nested").join(DEFAULT_APPLIED_MARKER);
        write_marker(&marker);
        assert!(marker.exists());
        write_marker(&marker);
        assert_eq!(std::fs::read(&marker).unwrap(), b"1");
    }
}
