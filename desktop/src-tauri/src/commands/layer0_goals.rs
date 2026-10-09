//! Layer 0 goals — the goal of a person or an agent, above every
//! conversation's goal tree.
//!
//! Each goal has a private part and an optional public part. The private
//! part never leaves this machine. The public part is off by default; when
//! enabled it is published as the subject's replaceable kind 10110 event,
//! and disabling it publishes empty content.
//!
//! # Where private goals live
//!
//! All under the app data directory's `agents/` folder
//! ([`managed_agents_base_dir`]):
//!
//! - `layer0-goals.json` — the store: every owner's and agent's goal.
//! - `.ctx/<agent pubkey>.json` — one derived file per local agent with only
//!   the goals that agent may see (see [`AgentGoalsFile`]). Desktop rewrites
//!   every agent's file whenever a goal is saved and at each spawn, and
//!   deletes it with the agent. The harness re-reads it every turn, so an
//!   edit reaches running agents on their next message, with no restart.
//!   Only the path is passed at spawn (`BUZZ_ACP_LAYER0_GOALS_FILE`, which
//!   the harness removes from its own environment); goal text never rides
//!   in environment variables.
//!
//! These files are plaintext. Any process running as the same OS user can
//! read them, including an agent's own shell tools if it goes looking. That
//! is acceptable for a single-user desktop, where the goals' owner is that
//! user; the folder name only keeps the files out of casual view and is not
//! a protection.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use buzz_core_pkg::kind::{KIND_PUBLIC_GOAL, MAX_PUBLIC_GOAL_CHARS};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::managed_agents::{load_managed_agents, managed_agents_base_dir, ManagedAgentRecord};

const MAX_PRIVATE_GOAL_CHARS: usize = 4_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer0Goal {
    #[serde(default)]
    pub private: String,
    #[serde(default)]
    pub public: String,
    #[serde(default)]
    pub public_enabled: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Layer0Store {
    /// Each identity's own goal, keyed by its pubkey (lowercase hex), so a
    /// different identity on this device never inherits it.
    #[serde(default)]
    owners: BTreeMap<String, Layer0Goal>,
    #[serde(default)]
    agents: BTreeMap<String, Layer0Goal>,
}

/// One agent's layer 0 goals as its harness reads them
/// (`buzz-acp`'s `layer0_goals::Layer0File`). Absent fields mean no goal.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentGoalsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_goal: Option<String>,
}

fn store_path<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(managed_agents_base_dir(app)?.join("layer0-goals.json"))
}

fn load_store<R: tauri::Runtime>(app: &AppHandle<R>) -> Layer0Store {
    store_path(app)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_store<R: tauri::Runtime>(app: &AppHandle<R>, store: &Layer0Store) -> Result<(), String> {
    let path = store_path(app)?;
    let raw = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| format!("failed to write layer 0 goals: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("failed to save layer 0 goals: {e}"))
}

/// The private goals one agent may see: its own, and its owner's.
///
/// The owner's goal only reaches agents that answer the owner alone: an
/// agent that answers others could be talked into repeating it.
fn agent_goals(
    store: &Layer0Store,
    agent_pubkey: &str,
    owner_pubkey: Option<&str>,
    answers_owner_only: bool,
) -> AgentGoalsFile {
    let non_empty = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    };
    AgentGoalsFile {
        agent_goal: store
            .agents
            .get(&agent_pubkey.to_ascii_lowercase())
            .and_then(|goal| non_empty(&goal.private)),
        owner_goal: owner_pubkey
            .filter(|_| answers_owner_only)
            .and_then(|owner| store.owners.get(&owner.to_ascii_lowercase()))
            .and_then(|goal| non_empty(&goal.private)),
    }
}

/// Whether `record` answers its owner alone, under this build's access policy.
fn answers_owner_only(record: &ManagedAgentRecord) -> bool {
    crate::managed_agents::projected_access_with_policy(record, crate::managed_agents::owner_only())
        .0
        == crate::managed_agents::RespondTo::OwnerOnly
}

fn agent_goals_dir(base_dir: &Path) -> PathBuf {
    base_dir.join(".ctx")
}

fn agent_goals_path(base_dir: &Path, agent_pubkey: &str) -> PathBuf {
    agent_goals_dir(base_dir).join(format!("{}.json", agent_pubkey.to_ascii_lowercase()))
}

/// Replace an agent's goals file atomically, so the harness never reads a
/// half-written file.
fn write_agent_goals_file(path: &Path, goals: &AgentGoalsFile) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create the agent goals folder: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(goals).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| format!("failed to write agent goals: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("failed to save agent goals: {e}"))
}

/// Write `record`'s goals file from the current store and return its path,
/// for `BUZZ_ACP_LAYER0_GOALS_FILE` at spawn. A failed write still returns
/// the path (the harness treats a missing file as "no goals"), so the next
/// goal edit, which rewrites every agent's file, can reach this agent.
pub(crate) fn sync_agent_goals_file<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    owner_pubkey: Option<&str>,
) -> Result<PathBuf, String> {
    let path = agent_goals_path(&managed_agents_base_dir(app)?, &record.pubkey);
    let goals = agent_goals(
        &load_store(app),
        &record.pubkey,
        owner_pubkey,
        answers_owner_only(record),
    );
    if let Err(error) = write_agent_goals_file(&path, &goals) {
        eprintln!(
            "buzz-desktop: layer 0 goals for agent {}: {error}",
            record.pubkey
        );
    }
    Ok(path)
}

/// Rewrite every local agent's goals file after a goal edit, so running
/// agents see it on their next message.
fn sync_all_agent_goals_files<R: tauri::Runtime>(
    app: &AppHandle<R>,
    store: &Layer0Store,
    owner_pubkey: &str,
) -> Result<(), String> {
    let base_dir = managed_agents_base_dir(app)?;
    for record in load_managed_agents(app)? {
        let goals = agent_goals(
            store,
            &record.pubkey,
            Some(owner_pubkey),
            answers_owner_only(&record),
        );
        write_agent_goals_file(&agent_goals_path(&base_dir, &record.pubkey), &goals)?;
    }
    Ok(())
}

/// Delete a removed agent's goals file. Best effort: the agent is gone.
pub(crate) fn remove_agent_goals_file<R: tauri::Runtime>(app: &AppHandle<R>, agent_pubkey: &str) {
    if let Ok(base_dir) = managed_agents_base_dir(app) {
        let _ = std::fs::remove_file(agent_goals_path(&base_dir, agent_pubkey));
    }
}

fn current_identity(state: &AppState) -> Result<String, String> {
    Ok(state.signing_keys()?.public_key().to_hex())
}

fn validate(goal: &Layer0Goal) -> Result<(), String> {
    if goal.private.chars().count() > MAX_PRIVATE_GOAL_CHARS {
        return Err(format!(
            "Keep the private goal under {MAX_PRIVATE_GOAL_CHARS} characters."
        ));
    }
    if goal.public.chars().count() > MAX_PUBLIC_GOAL_CHARS {
        return Err(format!(
            "Keep the public goal under {MAX_PUBLIC_GOAL_CHARS} characters."
        ));
    }
    Ok(())
}

/// Read a layer 0 goal. `agent_pubkey` selects one of your agents; `None`
/// reads your own.
#[tauri::command]
pub fn get_layer0_goal(
    app: AppHandle,
    agent_pubkey: Option<String>,
    state: State<'_, AppState>,
) -> Result<Layer0Goal, String> {
    let store = load_store(&app);
    let key = match agent_pubkey {
        Some(pubkey) => {
            return Ok(store
                .agents
                .get(&pubkey.to_ascii_lowercase())
                .cloned()
                .unwrap_or_default())
        }
        None => current_identity(&state)?,
    };
    Ok(store.owners.get(&key).cloned().unwrap_or_default())
}

/// Save a layer 0 goal and publish (or clear) its public part. Running agents
/// get a changed private goal on their next message.
#[tauri::command]
pub async fn set_layer0_goal(
    app: AppHandle,
    agent_pubkey: Option<String>,
    goal: Layer0Goal,
    state: State<'_, AppState>,
) -> Result<(), String> {
    validate(&goal)?;
    let identity = current_identity(&state)?;
    let mut store = load_store(&app);
    let previous = match &agent_pubkey {
        Some(pubkey) => store.agents.get(&pubkey.to_ascii_lowercase()),
        None => store.owners.get(&identity),
    }
    .cloned()
    .unwrap_or_default();

    let public_content = if goal.public_enabled {
        goal.public.trim().to_string()
    } else {
        String::new()
    };
    let previous_public = if previous.public_enabled {
        previous.public.trim().to_string()
    } else {
        String::new()
    };
    if public_content != previous_public {
        let builder =
            nostr::EventBuilder::new(nostr::Kind::Custom(KIND_PUBLIC_GOAL as u16), public_content);
        match &agent_pubkey {
            None => {
                crate::relay::submit_event(builder, &state).await?;
            }
            Some(pubkey) => {
                let records = load_managed_agents(&app)?;
                let record = records
                    .iter()
                    .find(|record| record.pubkey.eq_ignore_ascii_case(pubkey))
                    .ok_or_else(|| "That agent is not one of yours.".to_string())?;
                let keys = nostr::Keys::parse(&record.private_key_nsec)
                    .map_err(|_| "This agent's key is unavailable on this device.".to_string())?;
                crate::relay::submit_event_with_keys(
                    builder,
                    &state,
                    &keys,
                    record.auth_tag.as_deref(),
                )
                .await?;
            }
        }
    }

    match agent_pubkey {
        Some(pubkey) => {
            store.agents.insert(pubkey.to_ascii_lowercase(), goal);
        }
        None => {
            store.owners.insert(identity.clone(), goal);
        }
    }
    save_store(&app, &store)?;
    // The store is the source of truth; agent files are derived from it and
    // rewritten in full at every spawn, so retrying the save repairs a
    // failed rewrite.
    sync_all_agent_goals_files(&app, &store, &identity)
}

/// Someone's public layer 0 goal, if they share one.
#[tauri::command]
pub async fn get_public_goal(
    pubkey: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let filter = serde_json::json!({
        "kinds": [KIND_PUBLIC_GOAL],
        "authors": [pubkey.to_ascii_lowercase()],
        "limit": 1,
    });
    let events = crate::relay::query_relay(&state, &[filter]).await?;
    Ok(events
        .first()
        .map(|event| event.content.trim().to_string())
        .filter(|content| !content.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_limits_lengths() {
        let ok = Layer0Goal {
            private: "Grow the team".into(),
            public: "Ship weekly".into(),
            public_enabled: true,
        };
        assert!(validate(&ok).is_ok());
        let long = Layer0Goal {
            public: "x".repeat(MAX_PUBLIC_GOAL_CHARS + 1),
            ..ok.clone()
        };
        assert!(validate(&long).is_err());
    }

    #[test]
    fn store_defaults_empty() {
        let store: Layer0Store = serde_json::from_str("{}").unwrap();
        assert!(store.owners.is_empty());
        assert!(store.agents.is_empty());
        assert!(!Layer0Goal::default().public_enabled);
        // Stores written while the toggle was mirrored here still load.
        let legacy: Layer0Store = serde_json::from_str(r#"{"feature_enabled":true}"#).unwrap();
        assert!(legacy.owners.is_empty());
    }

    const AGENT: &str = "AAAA";
    const OWNER: &str = "bbbb";

    fn store() -> Layer0Store {
        let goal = |private: &str| Layer0Goal {
            private: private.into(),
            ..Layer0Goal::default()
        };
        Layer0Store {
            owners: BTreeMap::from([(OWNER.into(), goal("Owner plan"))]),
            agents: BTreeMap::from([(AGENT.to_ascii_lowercase(), goal(" Ship weekly "))]),
        }
    }

    #[test]
    fn owner_goal_reaches_only_owner_only_agents() {
        let owner_only = agent_goals(&store(), AGENT, Some(OWNER), true);
        assert_eq!(owner_only.agent_goal.as_deref(), Some("Ship weekly"));
        assert_eq!(owner_only.owner_goal.as_deref(), Some("Owner plan"));
        let open = agent_goals(&store(), AGENT, Some(OWNER), false);
        assert_eq!(open.agent_goal.as_deref(), Some("Ship weekly"));
        assert_eq!(open.owner_goal, None);
        assert_eq!(
            agent_goals(&store(), "cccc", None, true),
            AgentGoalsFile::default()
        );
    }

    #[test]
    fn agent_goals_file_is_rewritten_in_place_in_the_harness_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = agent_goals_path(dir.path(), AGENT);
        assert!(path.ends_with(".ctx/aaaa.json"));

        write_agent_goals_file(&path, &agent_goals(&store(), AGENT, Some(OWNER), true)).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["agent_goal"], "Ship weekly");
        assert_eq!(raw["owner_goal"], "Owner plan");

        // A later edit (here: the agent now answers others) replaces the file.
        write_agent_goals_file(&path, &agent_goals(&store(), AGENT, Some(OWNER), false)).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["agent_goal"], "Ship weekly");
        assert!(raw.get("owner_goal").is_none());
        assert!(!path.with_extension("json.tmp").exists());
    }
}
