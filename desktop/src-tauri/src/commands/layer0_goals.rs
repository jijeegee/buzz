//! Layer 0 goals — the goal of a person or an agent, above every
//! conversation's goal tree.
//!
//! Each goal has a private part and an optional public part. The private
//! part never leaves this machine: it lives in `layer0-goals.json` and is
//! handed to the owner's own agents at spawn (`BUZZ_ACP_AGENT_GOAL`,
//! `BUZZ_ACP_OWNER_GOAL`). The public part is off by default; when enabled it
//! is published as the subject's replaceable kind 10110 event, and disabling
//! it publishes empty content.

use std::collections::BTreeMap;
use std::path::PathBuf;

use buzz_core_pkg::kind::{KIND_PUBLIC_GOAL, MAX_PUBLIC_GOAL_CHARS};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::managed_agents::{load_managed_agents, managed_agents_base_dir};

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

/// Private goals handed to an agent at spawn: `(its own, its owner's)`.
///
/// The owner's goal only reaches agents that answer the owner alone: an
/// agent that answers others could be talked into repeating it.
pub(crate) fn spawn_goals<R: tauri::Runtime>(
    app: &AppHandle<R>,
    agent_pubkey: &str,
    owner_pubkey: Option<&str>,
    answers_owner_only: bool,
) -> (Option<String>, Option<String>) {
    let store = load_store(app);
    let non_empty = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    };
    let agent = store
        .agents
        .get(&agent_pubkey.to_ascii_lowercase())
        .and_then(|goal| non_empty(&goal.private));
    let owner = owner_pubkey
        .filter(|_| answers_owner_only)
        .and_then(|owner| store.owners.get(&owner.to_ascii_lowercase()))
        .and_then(|goal| non_empty(&goal.private));
    (agent, owner)
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

/// Save a layer 0 goal and publish (or clear) its public part. Agents pick
/// up a changed private goal on their next restart.
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
            store.owners.insert(identity, goal);
        }
    }
    save_store(&app, &store)
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
    }
}
