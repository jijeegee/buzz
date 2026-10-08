//! Self-opened task threads: the situations in which an agent may open a task
//! thread without being asked.
//!
//! One desktop-wide setting stored in `<app-data>/agents/task-threads.json`
//! (`{ "level": "standard", "triggers": [...] }`). Like channel routing it is
//! deliberately not a `GlobalAgentConfig` field, so changing it only raises the
//! restart badge instead of restarting every agent. The harness receives the
//! enabled triggers as `BUZZ_ACP_TASK_THREADS`, and only under the thread
//! session policy, where task threads get their own session and report back.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::{managed_agents_base_dir, storage::atomic_write_json, AcpSessionPolicy};

const TASK_THREADS_FILE: &str = "task-threads.json";
pub(crate) const TASK_THREADS_ENV_VAR: &str = "BUZZ_ACP_TASK_THREADS";

/// A situation in which the agent may open a task thread unasked. The
/// serialized names are the harness's `BUZZ_ACP_TASK_THREADS` entries.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TaskThreadTrigger {
    LongRunning,
    MultiStep,
    Parallel,
    SideDiscussion,
    Delegation,
}

impl TaskThreadTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::LongRunning => "long_running",
            Self::MultiStep => "multi_step",
            Self::Parallel => "parallel",
            Self::SideDiscussion => "side_discussion",
            Self::Delegation => "delegation",
        }
    }
}

/// How readily agents open task threads. Every level except `Custom` is a
/// preset trigger set; `Custom` keeps the user's own selection.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskThreadLevel {
    /// Only when a human asks (the base prompt's rule).
    Off,
    /// Only clearly long-running work.
    #[default]
    Minimal,
    Standard,
    Proactive,
    Custom,
}

impl TaskThreadLevel {
    fn preset(self) -> Option<&'static [TaskThreadTrigger]> {
        use TaskThreadTrigger::*;
        match self {
            Self::Off => Some(&[]),
            Self::Minimal => Some(&[LongRunning]),
            Self::Standard => Some(&[LongRunning, MultiStep, Parallel]),
            Self::Proactive => Some(&[LongRunning, MultiStep, Parallel, SideDiscussion, Delegation]),
            Self::Custom => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskThreadsSetting {
    pub level: TaskThreadLevel,
    /// Only read for `Custom`; preset levels always resolve to their preset.
    #[serde(default)]
    pub triggers: Vec<TaskThreadTrigger>,
}

impl TaskThreadsSetting {
    /// The triggers this setting enables, sorted and deduplicated.
    pub fn effective_triggers(&self) -> Vec<TaskThreadTrigger> {
        let mut triggers = self
            .level
            .preset()
            .map_or_else(|| self.triggers.clone(), <[_]>::to_vec);
        triggers.sort();
        triggers.dedup();
        triggers
    }

    /// The comma-separated harness list (empty when nothing is enabled).
    pub fn env_value(&self) -> String {
        self.effective_triggers()
            .iter()
            .map(|trigger| trigger.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// What a launch under `policy` receives: the list under the thread policy,
/// nothing otherwise. Spawn, the restart snapshot, and provider deploys all
/// go through this, so they cannot disagree.
pub(crate) fn task_threads_env_for(policy: AcpSessionPolicy, env_value: &str) -> Option<&str> {
    (policy == AcpSessionPolicy::Thread && !env_value.is_empty()).then_some(env_value)
}

/// Parse the stored file. Absent, unreadable, or unknown content reads as the
/// default (`Minimal`) with a warning; the next save rewrites it.
pub(crate) fn parse_task_threads(content: Option<&str>) -> TaskThreadsSetting {
    let Some(content) = content else {
        return TaskThreadsSetting::default();
    };
    serde_json::from_str(content).unwrap_or_else(|error| {
        eprintln!("buzz-desktop: unreadable {TASK_THREADS_FILE} ({error}); using the default");
        TaskThreadsSetting::default()
    })
}

fn task_threads_path<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(managed_agents_base_dir(app)?.join(TASK_THREADS_FILE))
}

pub fn load_task_threads<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<TaskThreadsSetting, String> {
    load_task_threads_at(&task_threads_path(app)?)
}

pub(crate) fn load_task_threads_at(path: &Path) -> Result<TaskThreadsSetting, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(parse_task_threads(Some(&content))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(parse_task_threads(None)),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

pub fn save_task_threads<R: tauri::Runtime>(
    app: &AppHandle<R>,
    setting: &TaskThreadsSetting,
) -> Result<(), String> {
    save_task_threads_at(&task_threads_path(app)?, setting)
}

pub(crate) fn save_task_threads_at(path: &Path, setting: &TaskThreadsSetting) -> Result<(), String> {
    let payload = serde_json::to_vec_pretty(setting)
        .map_err(|error| format!("failed to serialize task threads setting: {error}"))?;
    atomic_write_json(path, &payload)
}

/// The env value spawn paths apply, read once per launch. A read failure
/// launches with the default rather than blocking the agent.
pub(crate) fn current_task_threads_env<R: tauri::Runtime>(app: &AppHandle<R>) -> String {
    load_task_threads(app)
        .unwrap_or_else(|error| {
            eprintln!("buzz-desktop: {error}; using the default task threads setting");
            TaskThreadsSetting::default()
        })
        .env_value()
}

pub(crate) fn apply_task_threads_env(
    command: &mut std::process::Command,
    policy: AcpSessionPolicy,
    env_value: &str,
) {
    match task_threads_env_for(policy, env_value) {
        Some(value) => command.env(TASK_THREADS_ENV_VAR, value),
        None => command.env_remove(TASK_THREADS_ENV_VAR),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_defaults_to_long_running_only() {
        let setting = parse_task_threads(None);
        assert_eq!(setting.level, TaskThreadLevel::Minimal);
        assert_eq!(setting.env_value(), "long_running");
    }

    #[test]
    fn presets_ignore_stored_triggers_and_custom_uses_them() {
        let standard = TaskThreadsSetting {
            level: TaskThreadLevel::Standard,
            triggers: vec![TaskThreadTrigger::Delegation],
        };
        assert_eq!(standard.env_value(), "long_running,multi_step,parallel");

        let custom = TaskThreadsSetting {
            level: TaskThreadLevel::Custom,
            triggers: vec![
                TaskThreadTrigger::Delegation,
                TaskThreadTrigger::SideDiscussion,
                TaskThreadTrigger::Delegation,
            ],
        };
        assert_eq!(custom.env_value(), "side_discussion,delegation");
        assert_eq!(
            TaskThreadsSetting {
                level: TaskThreadLevel::Off,
                triggers: vec![TaskThreadTrigger::LongRunning],
            }
            .env_value(),
            ""
        );
    }

    #[test]
    fn unreadable_file_falls_back_to_default() {
        assert_eq!(
            parse_task_threads(Some("{\"level\":\"sometimes\"}")),
            TaskThreadsSetting::default()
        );
    }

    #[test]
    fn roundtrips_through_storage_shape() {
        let setting = TaskThreadsSetting {
            level: TaskThreadLevel::Custom,
            triggers: vec![TaskThreadTrigger::MultiStep],
        };
        let json = serde_json::to_string(&setting).unwrap_or_default();
        assert_eq!(json, r#"{"level":"custom","triggers":["multi_step"]}"#);
        assert_eq!(parse_task_threads(Some(&json)), setting);
    }

    #[test]
    fn only_thread_policy_launches_get_the_list() {
        assert_eq!(
            task_threads_env_for(AcpSessionPolicy::Thread, "long_running"),
            Some("long_running")
        );
        assert_eq!(task_threads_env_for(AcpSessionPolicy::Channel, "long_running"), None);
        assert_eq!(task_threads_env_for(AcpSessionPolicy::Thread, ""), None);

        let mut command = std::process::Command::new("true");
        command.env(TASK_THREADS_ENV_VAR, "stale");
        apply_task_threads_env(&mut command, AcpSessionPolicy::Channel, "long_running");
        assert!(command
            .get_envs()
            .any(|(key, value)| key == TASK_THREADS_ENV_VAR && value.is_none()));
    }
}
