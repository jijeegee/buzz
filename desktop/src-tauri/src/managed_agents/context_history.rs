//! New-session history: how much earlier conversation an agent reads when it
//! starts a fresh session for a channel main timeline or a thread.
//!
//! One desktop-wide setting stored in `<app-data>/agents/context-history.json`
//! (`{ "mode": "budget", "budget": "medium" }`). Like task threads it is
//! deliberately not a `GlobalAgentConfig` field, so changing it only raises the
//! restart badge instead of restarting every agent. The harness receives the
//! budget as `BUZZ_ACP_CONTEXT_HISTORY`; `Recent` (the default) sends nothing,
//! and channel dispatchers never receive it because a router keeps its own
//! small window.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::{channel_routing::RoutingRole, managed_agents_base_dir, storage::atomic_write_json};

const CONTEXT_HISTORY_FILE: &str = "context-history.json";
pub(crate) const CONTEXT_HISTORY_ENV_VAR: &str = "BUZZ_ACP_CONTEXT_HISTORY";

/// What a fresh session reads.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextHistoryMode {
    /// The most recent messages (the harness's `--context-message-limit`).
    #[default]
    Recent,
    /// As much of the conversation as fits in `budget`.
    Budget,
}

/// Size budget for [`ContextHistoryMode::Budget`]. The serialized names are
/// the harness's `BUZZ_ACP_CONTEXT_HISTORY` values (about 8k / 20k / 50k
/// tokens).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextHistoryBudget {
    Small,
    #[default]
    Medium,
    Large,
}

impl ContextHistoryBudget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContextHistorySetting {
    pub mode: ContextHistoryMode,
    /// Only read for `Budget`; kept for `Recent` so switching back restores it.
    #[serde(default)]
    pub budget: ContextHistoryBudget,
}

impl ContextHistorySetting {
    /// The harness value (empty for `Recent`).
    pub fn env_value(&self) -> String {
        match self.mode {
            ContextHistoryMode::Recent => String::new(),
            ContextHistoryMode::Budget => self.budget.as_str().to_string(),
        }
    }
}

/// What a launch with `role` receives: the budget for every agent except a
/// channel dispatcher. Spawn, the restart snapshot, and provider deploys all
/// go through this, so they cannot disagree.
pub(crate) fn context_history_env_for(role: RoutingRole, env_value: &str) -> Option<&str> {
    (role != RoutingRole::Dispatcher && !env_value.is_empty()).then_some(env_value)
}

/// Parse the stored file. Absent, unreadable, or unknown content reads as the
/// default (`Recent`) with a warning; the next save rewrites it.
pub(crate) fn parse_context_history(content: Option<&str>) -> ContextHistorySetting {
    let Some(content) = content else {
        return ContextHistorySetting::default();
    };
    serde_json::from_str(content).unwrap_or_else(|error| {
        eprintln!("buzz-desktop: unreadable {CONTEXT_HISTORY_FILE} ({error}); using the default");
        ContextHistorySetting::default()
    })
}

fn context_history_path<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(managed_agents_base_dir(app)?.join(CONTEXT_HISTORY_FILE))
}

pub fn load_context_history<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<ContextHistorySetting, String> {
    load_context_history_at(&context_history_path(app)?)
}

pub(crate) fn load_context_history_at(path: &Path) -> Result<ContextHistorySetting, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(parse_context_history(Some(&content))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(parse_context_history(None))
        }
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

pub fn save_context_history<R: tauri::Runtime>(
    app: &AppHandle<R>,
    setting: &ContextHistorySetting,
) -> Result<(), String> {
    save_context_history_at(&context_history_path(app)?, setting)
}

pub(crate) fn save_context_history_at(
    path: &Path,
    setting: &ContextHistorySetting,
) -> Result<(), String> {
    let payload = serde_json::to_vec_pretty(setting)
        .map_err(|error| format!("failed to serialize context history setting: {error}"))?;
    atomic_write_json(path, &payload)
}

/// The env value spawn paths apply, read once per launch. A read failure
/// launches with the default rather than blocking the agent.
pub(crate) fn current_context_history_env<R: tauri::Runtime>(app: &AppHandle<R>) -> String {
    load_context_history(app)
        .unwrap_or_else(|error| {
            eprintln!("buzz-desktop: {error}; using the default context history setting");
            ContextHistorySetting::default()
        })
        .env_value()
}

pub(crate) fn apply_context_history_env(
    command: &mut std::process::Command,
    role: RoutingRole,
    env_value: &str,
) {
    match context_history_env_for(role, env_value) {
        Some(value) => command.env(CONTEXT_HISTORY_ENV_VAR, value),
        None => command.env_remove(CONTEXT_HISTORY_ENV_VAR),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_defaults_to_recent_messages() {
        let setting = parse_context_history(None);
        assert_eq!(setting.mode, ContextHistoryMode::Recent);
        assert_eq!(setting.budget, ContextHistoryBudget::Medium);
        assert_eq!(setting.env_value(), "");
    }

    #[test]
    fn budget_mode_sends_the_budget_and_recent_ignores_it() {
        let large = ContextHistorySetting {
            mode: ContextHistoryMode::Budget,
            budget: ContextHistoryBudget::Large,
        };
        assert_eq!(large.env_value(), "large");
        assert_eq!(
            ContextHistorySetting {
                mode: ContextHistoryMode::Recent,
                budget: ContextHistoryBudget::Large,
            }
            .env_value(),
            ""
        );
    }

    #[test]
    fn unreadable_file_falls_back_to_default() {
        assert_eq!(
            parse_context_history(Some("{\"mode\":\"everything\"}")),
            ContextHistorySetting::default()
        );
    }

    #[test]
    fn roundtrips_through_storage_shape() {
        let setting = ContextHistorySetting {
            mode: ContextHistoryMode::Budget,
            budget: ContextHistoryBudget::Small,
        };
        let json = serde_json::to_string(&setting).unwrap_or_default();
        assert_eq!(json, r#"{"mode":"budget","budget":"small"}"#);
        assert_eq!(parse_context_history(Some(&json)), setting);
        assert_eq!(
            parse_context_history(Some(r#"{"mode":"budget"}"#)).budget,
            ContextHistoryBudget::Medium,
            "a missing budget reads as the default"
        );
    }

    #[test]
    fn dispatchers_never_get_the_budget() {
        assert_eq!(
            context_history_env_for(RoutingRole::None, "medium"),
            Some("medium")
        );
        assert_eq!(
            context_history_env_for(RoutingRole::Lead, "small"),
            Some("small")
        );
        assert_eq!(
            context_history_env_for(RoutingRole::Dispatcher, "large"),
            None
        );
        assert_eq!(context_history_env_for(RoutingRole::None, ""), None);

        let mut command = std::process::Command::new("true");
        command.env(CONTEXT_HISTORY_ENV_VAR, "stale");
        apply_context_history_env(&mut command, RoutingRole::Dispatcher, "large");
        assert!(command
            .get_envs()
            .any(|(key, value)| key == CONTEXT_HISTORY_ENV_VAR && value.is_none()));
    }

    #[test]
    fn saves_and_loads_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(CONTEXT_HISTORY_FILE);
        assert_eq!(
            load_context_history_at(&path),
            Ok(ContextHistorySetting::default())
        );
        let setting = ContextHistorySetting {
            mode: ContextHistoryMode::Budget,
            budget: ContextHistoryBudget::Large,
        };
        save_context_history_at(&path, &setting).expect("save");
        assert_eq!(load_context_history_at(&path), Ok(setting));
    }
}
