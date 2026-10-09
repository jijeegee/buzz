//! Agent settings that are only text, applied without restarting the agent:
//! the agent's instructions (system prompt), its team's instructions, the
//! self-opened task thread rules (`task-threads.json`), and the new-session
//! history budget (`context-history.json`).
//!
//! Each local agent gets its own `live-settings/<agent pubkey>.json` (see
//! [`LiveSettingsFile`]), written at spawn and rewritten whenever one of
//! these settings changes. Only the file's path rides in the environment
//! (`BUZZ_ACP_LIVE_SETTINGS_FILE`); the harness re-reads the file every turn,
//! tells live sessions about a change once, and gives new sessions the
//! current values. Because a change needs no restart, these settings are not
//! part of the restart-required spawn snapshot.
//!
//! What applies to an agent still depends on how its process was launched:
//! task thread rules only under the thread session policy, no history budget
//! for a dispatcher, the lead addendum on a lead's prompt. A running agent's
//! file follows the policy and routing role it was launched with; changing
//! those still needs a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::channel_routing::RoutingRole;
use super::types::{AgentDefinition, ManagedAgentRecord, TeamRecord};
use super::{AcpSessionPolicy, GlobalAgentConfig};

/// The harness flag naming the live settings file (`buzz-acp
/// --live-settings-file`).
pub(crate) const LIVE_SETTINGS_ENV_VAR: &str = "BUZZ_ACP_LIVE_SETTINGS_FILE";
const SYSTEM_PROMPT_FILE_ENV_VAR: &str = "BUZZ_ACP_SYSTEM_PROMPT_FILE";
const TEAM_INSTRUCTIONS_ENV_VAR: &str = "BUZZ_ACP_TEAM_INSTRUCTIONS";

/// One agent's live settings as its harness reads them (`buzz-acp`'s
/// `live_settings::LiveSettingsFile`). Values use the spelling of their
/// launch variables; an absent field means "not set".
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct LiveSettingsFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_threads: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_history: Option<String>,
}

/// The already-resolved values the file is built from.
pub(crate) struct LiveSettingsInputs<'a> {
    /// The effective system prompt (`resolve_effective_config`), before any
    /// routing addendum.
    pub system_prompt: Option<&'a str>,
    /// The layered user env the launch applies (`descriptor.env`). It is
    /// written after the Buzz-set team instructions, so an explicit
    /// `BUZZ_ACP_TEAM_INSTRUCTIONS` wins; a lead launch inlines a
    /// `BUZZ_ACP_SYSTEM_PROMPT_FILE`.
    pub user_env: &'a BTreeMap<String, String>,
    /// The bound team's instructions (`effective_team_instructions`).
    pub team_instructions: Option<&'a str>,
    /// The desktop-wide task thread trigger list (`task-threads.json`).
    pub task_threads: &'a str,
    /// The desktop-wide new-session history budget (`context-history.json`).
    pub context_history: &'a str,
    pub session_policy: AcpSessionPolicy,
    pub routing_role: RoutingRole,
}

impl LiveSettingsFile {
    /// The values a process launched with `inputs.session_policy` and
    /// `inputs.routing_role` receives. Spawn and the live sync both build the
    /// file here, so an unchanged setting never looks changed to the harness.
    pub(crate) fn from_inputs(inputs: LiveSettingsInputs<'_>) -> Self {
        let LiveSettingsInputs {
            system_prompt,
            user_env,
            team_instructions,
            task_threads,
            context_history,
            session_policy,
            routing_role,
        } = inputs;
        let system_prompt = if routing_role == RoutingRole::Lead {
            // Mirror `apply_lead_env`: the inline prompt, else the prompt
            // file's contents, then the lead addendum.
            let base = match system_prompt {
                Some(prompt) => prompt.to_string(),
                None => user_env
                    .get(SYSTEM_PROMPT_FILE_ENV_VAR)
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .unwrap_or_default(),
            };
            Some(super::routing_env::lead_system_prompt(
                &base,
                super::channel_routing::lead_rules::LEAD_LISTEN_ADDENDUM,
            ))
        } else {
            system_prompt.map(str::to_string)
        };
        let team_instructions = match user_env.get(TEAM_INSTRUCTIONS_ENV_VAR) {
            Some(value) => Some(value.as_str()),
            None => team_instructions,
        }
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
        Self {
            system_prompt,
            team_instructions,
            task_threads: super::task_threads::task_threads_env_for(session_policy, task_threads)
                .map(str::to_string),
            context_history: super::context_history::context_history_env_for(
                routing_role,
                context_history,
            )
            .map(str::to_string),
        }
    }
}

fn live_settings_path(base_dir: &Path, agent_pubkey: &str) -> PathBuf {
    base_dir
        .join("live-settings")
        .join(format!("{}.json", agent_pubkey.to_ascii_lowercase()))
}

/// Replace the file atomically, so the harness never reads a half-written
/// one. Unchanged content is left alone.
fn write_live_settings_file(path: &Path, settings: &LiveSettingsFile) -> Result<(), String> {
    let raw = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    if std::fs::read(path).is_ok_and(|current| current == raw) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create the live settings folder: {e}"))?;
    }
    super::storage::atomic_write_json(path, &raw)
}

/// Write `record`'s live settings for a launch and return the file's path for
/// [`LIVE_SETTINGS_ENV_VAR`]. A failed write still returns the path: the
/// harness keeps its launch values until the file can be read, and the next
/// settings change rewrites it.
pub(crate) fn write_spawn_live_settings<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    settings: &LiveSettingsFile,
) -> Result<PathBuf, String> {
    let path = live_settings_path(&super::managed_agents_base_dir(app)?, &record.pubkey);
    if let Err(error) = write_live_settings_file(&path, settings) {
        eprintln!(
            "buzz-desktop: live settings for agent {}: {error}",
            record.pubkey
        );
    }
    Ok(path)
}

/// Delete a removed agent's live settings file. Best effort: the agent is gone.
pub(crate) fn remove_live_settings_file<R: tauri::Runtime>(app: &AppHandle<R>, agent_pubkey: &str) {
    if let Ok(base_dir) = super::managed_agents_base_dir(app) {
        let _ = std::fs::remove_file(live_settings_path(&base_dir, agent_pubkey));
    }
}

/// The live settings `record` would get now from the current personas,
/// teams, and settings, for a process launched with `session_policy` and
/// `routing_role`. Resolves the prompt exactly as the restart snapshot does.
#[allow(clippy::too_many_arguments)]
pub(crate) fn current_live_settings(
    record: &ManagedAgentRecord,
    personas: &[AgentDefinition],
    teams: &[TeamRecord],
    global: &GlobalAgentConfig,
    task_threads: &str,
    context_history: &str,
    session_policy: AcpSessionPolicy,
    routing_role: RoutingRole,
) -> LiveSettingsFile {
    let record = super::persona_events::preview_prospective_persona_snapshot(record, personas);
    let prompt = match super::effective_config::resolve_effective_config(&record, personas, global)
    {
        super::effective_config::EffectiveConfigResult::Resolved(cfg) => cfg.system_prompt.value,
        super::effective_config::EffectiveConfigResult::OrphanedInstance { .. } => None,
    };
    let user_env = super::resolve_effective_harness_descriptor(&record, personas, global)
        .map(|descriptor| descriptor.env)
        .unwrap_or_default();
    LiveSettingsFile::from_inputs(LiveSettingsInputs {
        system_prompt: prompt.as_deref(),
        user_env: &user_env,
        team_instructions: super::spawn_snapshot::effective_team_instructions(&record, teams)
            .as_deref(),
        task_threads,
        context_history,
        session_policy,
        routing_role,
    })
}

/// Set while a sync is queued but has not started reading settings yet.
static SYNC_QUEUED: AtomicBool = AtomicBool::new(false);
/// Serializes syncs, so the last one to run always read the newest state.
static SYNC_LOCK: Mutex<()> = Mutex::new(());

/// Refresh the live settings file of every running local agent, on a
/// background thread. Called after every save of a store these settings come
/// from; saves made while a sync is queued are picked up by that sync, which
/// reads the stores only when it starts.
pub(crate) fn schedule_live_settings_sync<R: tauri::Runtime>(app: &AppHandle<R>) {
    if app.try_state::<crate::app_state::AppState>().is_none()
        || SYNC_QUEUED.swap(true, Ordering::SeqCst)
    {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let _serial = SYNC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        SYNC_QUEUED.store(false, Ordering::SeqCst);
        if let Err(error) = sync_running_agents(&app) {
            eprintln!("buzz-desktop: live settings sync: {error}");
        }
    });
}

fn sync_running_agents<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    // The policy and role each running process was launched with.
    let running: Vec<(String, AcpSessionPolicy, RoutingRole)> = {
        let state = app.state::<crate::app_state::AppState>();
        let runtimes = state
            .managed_agent_processes
            .lock()
            .map_err(|e| e.to_string())?;
        runtimes
            .iter()
            .map(|(key, runtime)| {
                let policy =
                    if runtime.spawn_config.session_policy == AcpSessionPolicy::Thread.as_str() {
                        AcpSessionPolicy::Thread
                    } else {
                        AcpSessionPolicy::Channel
                    };
                (
                    key.pubkey.clone(),
                    policy,
                    runtime.spawn_config.routing_role,
                )
            })
            .collect()
    };
    if running.is_empty() {
        return Ok(());
    }
    let base_dir = super::managed_agents_base_dir(app)?;
    let records = super::storage::load_managed_agents_without_keys(app)?;
    let personas = super::personas::load_personas_readonly(app)?;
    let teams = super::teams::load_teams_readonly(&super::teams::teams_store_path(app)?)?;
    let global = super::global_config::load_global_agent_config(app)?;
    let task_threads = super::task_threads::current_task_threads_env(app);
    let context_history = super::context_history::current_context_history_env(app);
    let mut failures = Vec::new();
    for (pubkey, policy, role) in running {
        let Some(record) = records.iter().find(|record| record.pubkey == pubkey) else {
            continue;
        };
        let settings = current_live_settings(
            record,
            &personas,
            &teams,
            &global,
            &task_threads,
            &context_history,
            policy,
            role,
        );
        if let Err(error) =
            write_live_settings_file(&live_settings_path(&base_dir, &pubkey), &settings)
        {
            failures.push(format!("{pubkey}: {error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(
        env: &'a BTreeMap<String, String>,
        policy: AcpSessionPolicy,
        role: RoutingRole,
    ) -> LiveSettingsInputs<'a> {
        LiveSettingsInputs {
            system_prompt: Some("You are Eva."),
            user_env: env,
            team_instructions: Some("Ship small."),
            task_threads: "long_running,multi_step",
            context_history: "medium",
            session_policy: policy,
            routing_role: role,
        }
    }

    #[test]
    fn values_follow_the_launch_policy_and_role() {
        let env = BTreeMap::new();
        let thread = LiveSettingsFile::from_inputs(inputs(
            &env,
            AcpSessionPolicy::Thread,
            RoutingRole::None,
        ));
        assert_eq!(
            thread,
            LiveSettingsFile {
                system_prompt: Some("You are Eva.".into()),
                team_instructions: Some("Ship small.".into()),
                task_threads: Some("long_running,multi_step".into()),
                context_history: Some("medium".into()),
            }
        );

        let channel_dispatcher = LiveSettingsFile::from_inputs(inputs(
            &env,
            AcpSessionPolicy::Channel,
            RoutingRole::Dispatcher,
        ));
        assert_eq!(channel_dispatcher.task_threads, None);
        let env = BTreeMap::from([(
            TEAM_INSTRUCTIONS_ENV_VAR.to_string(),
            " From env. ".to_string(),
        )]);
        assert_eq!(
            LiveSettingsFile::from_inputs(inputs(
                &env,
                AcpSessionPolicy::Channel,
                RoutingRole::None
            ))
            .team_instructions
            .as_deref(),
            Some("From env."),
            "user env is written after the team instructions and wins"
        );
        assert_eq!(channel_dispatcher.context_history, None);
        assert_eq!(
            channel_dispatcher.system_prompt.as_deref(),
            Some("You are Eva.")
        );
    }

    #[test]
    fn a_lead_prompt_carries_the_lead_addendum_like_its_launch() {
        let env = BTreeMap::new();
        let lead = LiveSettingsFile::from_inputs(inputs(
            &env,
            AcpSessionPolicy::Channel,
            RoutingRole::Lead,
        ));
        let mut command = std::process::Command::new("buzz-acp");
        command.env("BUZZ_ACP_SYSTEM_PROMPT", "You are Eva.");
        super::super::routing_env::apply_lead_env(
            &mut command,
            Path::new("rules.toml"),
            super::super::channel_routing::lead_rules::LEAD_LISTEN_ADDENDUM,
        )
        .unwrap();
        let launched = command
            .get_envs()
            .find(|(key, _)| *key == "BUZZ_ACP_SYSTEM_PROMPT")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(lead.system_prompt, launched);

        let dir = tempfile::tempdir().unwrap();
        let prompt_file = dir.path().join("prompt.md");
        std::fs::write(&prompt_file, "From a file.").unwrap();
        let env = BTreeMap::from([(
            SYSTEM_PROMPT_FILE_ENV_VAR.to_string(),
            prompt_file.display().to_string(),
        )]);
        let from_file = LiveSettingsFile::from_inputs(LiveSettingsInputs {
            system_prompt: None,
            ..inputs(&env, AcpSessionPolicy::Channel, RoutingRole::Lead)
        });
        assert!(from_file
            .system_prompt
            .unwrap()
            .starts_with("From a file.\n\n## Listening as channel lead"));
    }

    #[test]
    fn file_is_written_atomically_and_only_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = live_settings_path(dir.path(), "ABCDEF");
        assert!(path.ends_with("live-settings/abcdef.json"));
        let settings = LiveSettingsFile {
            system_prompt: Some("You are Eva.".into()),
            ..Default::default()
        };
        write_live_settings_file(&path, &settings).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            written,
            serde_json::json!({ "system_prompt": "You are Eva." })
        );

        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_live_settings_file(&path, &settings).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified,
            "unchanged settings are not rewritten"
        );
        write_live_settings_file(&path, &LiveSettingsFile::default()).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }
}
