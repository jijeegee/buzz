//! Settings › Experiments › "New session history" commands.
//!
//! The saved setting is read by every launch, and running agents pick a change
//! up for their next new session through their live settings file.

use tauri::AppHandle;

use crate::managed_agents::account_settings::require_settings_owner;
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
    owner: String,
    app: AppHandle,
) -> Result<ContextHistorySetting, String> {
    tokio::task::spawn_blocking(move || {
        require_settings_owner(&app, &owner)?;
        save_context_history(&app, &owner, &setting)?;
        Ok(setting)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}
