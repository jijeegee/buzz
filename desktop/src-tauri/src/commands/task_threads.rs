//! Settings › Experiments › "Agents open task threads" commands.
//!
//! The saved setting is read by every launch; running agents pick a change up
//! on restart, which the restart badge surfaces through the spawn snapshot.

use tauri::AppHandle;

use crate::managed_agents::task_threads::{load_task_threads, save_task_threads, TaskThreadsSetting};

#[tauri::command]
pub async fn get_task_threads(app: AppHandle) -> Result<TaskThreadsSetting, String> {
    tokio::task::spawn_blocking(move || load_task_threads(&app))
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

#[tauri::command]
pub async fn set_task_threads(
    setting: TaskThreadsSetting,
    app: AppHandle,
) -> Result<TaskThreadsSetting, String> {
    tokio::task::spawn_blocking(move || {
        save_task_threads(&app, &setting)?;
        Ok(setting)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}
