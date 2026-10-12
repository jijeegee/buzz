use tauri::{AppHandle, Manager};

/// Hand-edited tuning knobs for trying performance trade-offs on an installed
/// build without a settings screen: `<app data dir>/dev-tuning.json`.
const DEV_TUNING_FILE: &str = "dev-tuning.json";
/// The file is a few knobs; anything larger is not ours to parse.
const MAX_DEV_TUNING_BYTES: u64 = 16 * 1024;

/// Raw `dev-tuning.json` contents, or `None` when the file is absent or
/// unreadable. Parsing and defaults live in the frontend.
#[tauri::command]
pub fn get_dev_tuning(app: AppHandle) -> Option<String> {
    let path = app.path().app_data_dir().ok()?.join(DEV_TUNING_FILE);
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > MAX_DEV_TUNING_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}
