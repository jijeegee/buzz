//! Start Kuzz when the user signs in to the computer (Windows `Run` key,
//! macOS Launch Agent). A launch started this way passes
//! [`LAUNCHED_AT_LOGIN_ARG`] and stays in the tray instead of opening the
//! window.

use tauri_plugin_autostart::ManagerExt;

/// Argument the login item passes, so the app knows the user did not open it.
pub(crate) const LAUNCHED_AT_LOGIN_ARG: &str = "--launched-at-login";

/// Whether this process was started by the login item.
pub(crate) fn launched_at_login() -> bool {
    std::env::args().any(|arg| arg == LAUNCHED_AT_LOGIN_ARG)
}

/// The login item plugin, registering the app with [`LAUNCHED_AT_LOGIN_ARG`].
pub(crate) fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        Some(vec![LAUNCHED_AT_LOGIN_ARG]),
    )
}

#[tauri::command]
pub fn get_launch_at_login(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_launch_at_login(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    let current = manager.is_enabled().map_err(|error| error.to_string())?;
    // Disabling an absent Windows `Run` entry is an error, so only change
    // what differs.
    if current != enabled {
        if enabled {
            manager.enable()
        } else {
            manager.disable()
        }
        .map_err(|error| error.to_string())?;
    }
    manager.is_enabled().map_err(|error| error.to_string())
}
