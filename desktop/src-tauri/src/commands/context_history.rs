//! Settings › Experiments › "New session history" commands.
//!
//! The saved setting is read by every launch; running agents pick a change up
//! on restart, which the restart badge surfaces through the spawn snapshot.

use tauri::AppHandle;

use crate::managed_agents::context_history::{
    load_context_history, save_context_history, ContextHistorySetting,
};

#[tauri::command]
pub async fn get_context_history(app: AppHandle) -> Result<ContextHistorySetting, String> {
    tokio::task::spawn_blocking(move || load_context_history(&app))
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

#[tauri::command]
pub async fn set_context_history(
    setting: ContextHistorySetting,
    app: AppHandle,
) -> Result<ContextHistorySetting, String> {
    tokio::task::spawn_blocking(move || {
        save_context_history(&app, &setting)?;
        Ok(setting)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}
