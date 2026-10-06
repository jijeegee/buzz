//! Channel routing: how agents pick up channel messages nobody @mentioned.
//!
//! One desktop-wide enum ([`ChannelRoutingMode`]) is stored in
//! `<app-data>/agents/channel-routing.json` (`{ "mode": "host" }`). A missing
//! file reads as [`ChannelRoutingMode::Host`] — the behavior from before the
//! setting existed, where the starred record always spawned as a dispatcher.
//! It is deliberately **not** a `GlobalAgentConfig` field: saving that config
//! restarts every local agent, while a mode switch only concerns the agents
//! whose role changes.
//!
//! Every agent's routing job comes from the one pure function
//! [`routing_role_for`]; local spawn, the restart snapshot, and provider
//! deploys all call it, so the env a process gets and the role the UI reports
//! cannot disagree. Moving between modes is planned by
//! [`plan_routing_transition`], which keeps two routing paths from running at
//! once while processes restart.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::{managed_agents_base_dir, storage::atomic_write_json, BackendKind, ManagedAgentRecord};

mod launch;
pub(crate) mod lead_rules;
mod transition;
pub(crate) use launch::{
    launch_role, live_local_roles, load_deployed_roles, observe_remote, record_deployed_role,
};
pub(crate) use transition::{
    plan_routing_transition, AgentRoutingState, AgentTransition, AppliedRouting,
};

const CHANNEL_ROUTING_FILE: &str = "channel-routing.json";
/// Directory (under `<app-data>/agents/`) for routing files the desktop
/// generates. Harness config pointing inside it is ours to scrub
/// (`routing_env`); anything else the user configured is left alone.
const GENERATED_ROUTING_DIR: &str = "routing";

/// The saved channel routing mode. Exactly one is active per desktop.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelRoutingMode {
    /// Only @mentioned agents answer.
    Off,
    /// The routing agent runs as a channel dispatcher (`BUZZ_ACP_DISPATCHER`).
    #[default]
    Host,
    /// The routing agent listens to its owner's unmentioned messages through
    /// generated Config-mode rules (`lead_rules`).
    Lead,
    /// Smart routing: the desktop assigns each of the user's unmentioned
    /// sends itself (`message_routing`); no agent gets a routing role.
    DesktopRouter,
}

impl ChannelRoutingMode {
    /// Whether this build lets the user select the mode. Every mode is
    /// selectable; `set_channel_routing` still refuses one that is not ready
    /// (no routing agent, a Lead it cannot run, or no Smart routing key).
    pub(crate) fn is_selectable(self) -> bool {
        matches!(
            self,
            Self::Off | Self::Host | Self::Lead | Self::DesktopRouter
        )
    }
}

/// The routing job one agent process has (or would get) at spawn.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingRole {
    /// An ordinary agent: answers only when addressed.
    #[default]
    None,
    /// Channel dispatcher (Host mode).
    Dispatcher,
    /// Channel lead (Lead mode).
    Lead,
}

/// The role `record` takes under `mode`. The only source of a record's
/// routing env — spawn, the restart snapshot, and provider deploys all call
/// this, so they cannot disagree.
///
/// - Host: the starred record dispatches, on any backend.
/// - Lead: the starred record leads only when it runs on this computer (the
///   generated rules file is local) and the workspace owner is known (the
///   rules listen to that owner).
/// - Off and Smart routing: nobody has a role; the star is just remembered.
pub fn routing_role_for(
    record: &ManagedAgentRecord,
    mode: ChannelRoutingMode,
    owner_hex: Option<&str>,
) -> RoutingRole {
    if !record.is_default_ai {
        return RoutingRole::None;
    }
    match mode {
        ChannelRoutingMode::Host => RoutingRole::Dispatcher,
        ChannelRoutingMode::Lead
            if record.backend == BackendKind::Local
                && owner_hex.is_some_and(|owner| !owner.trim().is_empty()) =>
        {
            RoutingRole::Lead
        }
        ChannelRoutingMode::Lead | ChannelRoutingMode::Off | ChannelRoutingMode::DesktopRouter => {
            RoutingRole::None
        }
    }
}

/// How the desktop can see one agent's process right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservedProcess {
    /// Nothing live for this agent in the active workspace.
    Stopped,
    /// A process this desktop spawned, with the role stamped at spawn.
    Tracked(RoutingRole),
    /// Live, but with no spawn stamp to read (a process adopted from an
    /// earlier desktop run). Its role is assumed to be the desired one, the
    /// same assumption the restart badge makes for unstamped processes.
    Unstamped,
}

/// Turn one record and what is observed of its process into the plan input.
pub(crate) fn observed_routing_state(
    record: &ManagedAgentRecord,
    desired_role: RoutingRole,
    observed: ObservedProcess,
) -> AgentRoutingState {
    let (running, running_role) = match observed {
        ObservedProcess::Stopped => (false, RoutingRole::None),
        ObservedProcess::Tracked(role) => (true, role),
        ObservedProcess::Unstamped => (true, desired_role),
    };
    AgentRoutingState {
        pubkey: record.pubkey.clone(),
        running,
        local: record.backend == BackendKind::Local,
        running_role,
        desired_role,
    }
}

#[derive(Serialize, Deserialize)]
struct ChannelRoutingFile {
    mode: ChannelRoutingMode,
}

/// Parse the stored file. Absent → Host (today's behavior); unreadable JSON
/// or an unknown mode also reads as Host, with a warning, so a damaged file
/// can never silently turn routing off or on in a new way. The next save
/// rewrites it.
pub(crate) fn parse_channel_routing(content: Option<&str>) -> ChannelRoutingMode {
    let Some(content) = content else {
        return ChannelRoutingMode::default();
    };
    match serde_json::from_str::<ChannelRoutingFile>(content) {
        Ok(file) => file.mode,
        Err(error) => {
            eprintln!(
                "buzz-desktop: unreadable {CHANNEL_ROUTING_FILE} ({error}); using the default Host mode"
            );
            ChannelRoutingMode::default()
        }
    }
}

fn channel_routing_path<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(managed_agents_base_dir(app)?.join(CHANNEL_ROUTING_FILE))
}

/// `<app-data>/agents/routing/` — where generated routing files live.
pub(crate) fn generated_routing_dir<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<PathBuf, String> {
    Ok(managed_agents_base_dir(app)?.join(GENERATED_ROUTING_DIR))
}

/// Load the saved mode; see [`parse_channel_routing`] for the fallbacks.
/// Only a failure to read an existing file is an error.
pub fn load_channel_routing<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<ChannelRoutingMode, String> {
    load_channel_routing_at(&channel_routing_path(app)?)
}

pub(crate) fn load_channel_routing_at(path: &Path) -> Result<ChannelRoutingMode, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(parse_channel_routing(Some(&content))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(parse_channel_routing(None))
        }
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

/// Persist the mode with one atomic write.
pub fn save_channel_routing<R: tauri::Runtime>(
    app: &AppHandle<R>,
    mode: ChannelRoutingMode,
) -> Result<(), String> {
    save_channel_routing_at(&channel_routing_path(app)?, mode)
}

pub(crate) fn save_channel_routing_at(path: &Path, mode: ChannelRoutingMode) -> Result<(), String> {
    let payload = serde_json::to_vec_pretty(&ChannelRoutingFile { mode })
        .map_err(|error| format!("failed to serialize channel routing: {error}"))?;
    atomic_write_json(path, &payload)
}

/// The workspace owner's hex pubkey, which Lead's rules listen to. `None`
/// when the identity is unavailable (Lead then resolves to no role).
pub(crate) fn routing_owner_hex<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<String> {
    use tauri::Manager;
    let state = app.state::<crate::app_state::AppState>();
    let keys = state.keys.lock().ok()?;
    Some(keys.public_key().to_hex())
}

#[cfg(test)]
mod tests;
