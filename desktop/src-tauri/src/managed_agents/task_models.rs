//! Settings › Models › Task models: which provider/model each app task (AI
//! work Buzz runs on its own behalf) uses.
//!
//! Stored alone in `<app-data>/agents/task-models.json`, one entry per task:
//! `{ "message-routing": { "provider": "anthropic", "model": "claude-haiku-4-5" } }`.
//! A missing entry or field means "automatic" (see
//! `message_routing::resolve_router_model`). It is deliberately **not** a
//! `GlobalAgentConfig` field: saving that record restarts every running
//! local agent, which an app-task change must never do. No secrets live here
//! — keys stay in the Providers tab's `GlobalAgentConfig.env_vars`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::{managed_agents_base_dir, storage::atomic_write_json};

const TASK_MODELS_FILE: &str = "task-models.json";

/// Smart routing's task: pick the agent for an unmentioned channel send.
pub const MESSAGE_ROUTING_TASK: &str = "message-routing";

/// Every task id this build knows. `set_task_model` rejects anything else so
/// a typo can never create an entry nothing reads.
pub const KNOWN_TASK_IDS: &[&str] = &[MESSAGE_ROUTING_TASK];

/// Longest model id accepted. Provider ids are far shorter in practice.
const MAX_MODEL_ID_CHARS: usize = 128;

/// One task's saved choice. `None` fields mean "automatic".
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskModelSetting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl TaskModelSetting {
    fn is_automatic(&self) -> bool {
        self.provider.is_none() && self.model.is_none()
    }
}

pub type TaskModels = BTreeMap<String, TaskModelSetting>;

fn task_models_path<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(managed_agents_base_dir(app)?.join(TASK_MODELS_FILE))
}

/// Parse the stored file. Damaged JSON reads as "all automatic" with a
/// warning — the next save rewrites it — so a bad file can never wedge the
/// settings tab or the router.
pub(crate) fn parse_task_models(content: &str) -> TaskModels {
    match serde_json::from_str::<TaskModels>(content) {
        Ok(models) => models,
        Err(error) => {
            eprintln!("buzz-desktop: unreadable {TASK_MODELS_FILE} ({error}); using automatic task models");
            TaskModels::new()
        }
    }
}

pub fn load_task_models<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<TaskModels, String> {
    load_task_models_at(&task_models_path(app)?)
}

pub(crate) fn load_task_models_at(path: &Path) -> Result<TaskModels, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(parse_task_models(&content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(TaskModels::new()),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

/// Trim a user-supplied id; blank means "automatic". Rejects control
/// characters and over-long values — the model id is sent verbatim to the
/// provider.
pub(crate) fn normalize_choice(
    value: Option<String>,
    what: &str,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_MODEL_ID_CHARS {
        return Err(format!("{what} is too long."));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(format!("{what} contains a control character."));
    }
    Ok(Some(trimmed.to_string()))
}

/// Replace one task's entry and persist the whole file with one atomic
/// write (one user action = one save). An all-automatic setting removes the
/// entry.
pub fn save_task_model<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task_id: &str,
    setting: TaskModelSetting,
) -> Result<TaskModels, String> {
    save_task_model_at(&task_models_path(app)?, task_id, setting)
}

pub(crate) fn save_task_model_at(
    path: &Path,
    task_id: &str,
    setting: TaskModelSetting,
) -> Result<TaskModels, String> {
    if !KNOWN_TASK_IDS.contains(&task_id) {
        return Err(format!("Unknown app task: {task_id}"));
    }
    let mut models = load_task_models_at(path)?;
    if setting.is_automatic() {
        models.remove(task_id);
    } else {
        models.insert(task_id.to_string(), setting);
    }
    let payload = serde_json::to_vec_pretty(&models)
        .map_err(|error| format!("failed to serialize task models: {error}"))?;
    atomic_write_json(path, &payload)?;
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_reads_as_all_automatic() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_task_models_at(&dir.path().join(TASK_MODELS_FILE))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn damaged_file_reads_as_all_automatic() {
        assert!(parse_task_models("{not json").is_empty());
        assert!(parse_task_models("[1,2]").is_empty());
    }

    #[test]
    fn save_round_trips_and_automatic_removes_the_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TASK_MODELS_FILE);
        let chosen = TaskModelSetting {
            provider: Some("anthropic".into()),
            model: Some("claude-haiku-4-5".into()),
        };
        save_task_model_at(&path, MESSAGE_ROUTING_TASK, chosen.clone()).unwrap();
        let loaded = load_task_models_at(&path).unwrap();
        assert_eq!(loaded.get(MESSAGE_ROUTING_TASK), Some(&chosen));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"message-routing\""), "{raw}");

        save_task_model_at(&path, MESSAGE_ROUTING_TASK, TaskModelSetting::default()).unwrap();
        assert!(load_task_models_at(&path).unwrap().is_empty());
    }

    #[test]
    fn unknown_task_ids_are_rejected_and_write_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TASK_MODELS_FILE);
        let error = save_task_model_at(
            &path,
            "summaries",
            TaskModelSetting {
                provider: Some("anthropic".into()),
                model: None,
            },
        )
        .unwrap_err();
        assert!(error.contains("summaries"), "{error}");
        assert!(!path.exists());
    }

    #[test]
    fn normalize_choice_trims_blanks_and_rejects_unsafe_values() {
        assert_eq!(normalize_choice(None, "Model").unwrap(), None);
        assert_eq!(normalize_choice(Some("  ".into()), "Model").unwrap(), None);
        assert_eq!(
            normalize_choice(Some(" claude-haiku-4-5 ".into()), "Model").unwrap(),
            Some("claude-haiku-4-5".into())
        );
        assert!(normalize_choice(Some("a\nb".into()), "Model").is_err());
        assert!(normalize_choice(Some("x".repeat(MAX_MODEL_ID_CHARS + 1)), "Model").is_err());
    }
}
