//! What routing role a process actually runs with, and the gate every launch
//! passes through.
//!
//! The transition plan needs each live process's *running* role. Local
//! processes this desktop spawned carry it in their spawn stamp; provider
//! deployments have no process here, so the role each deploy shipped is kept
//! in a sidecar, `<app-data>/agents/routing/deployed-roles.json`
//! (`{ "<pubkey>": "dispatcher" }`), written after the provider accepts the
//! payload. `managed-agents.json` is untouched.
//!
//! [`launch_role`] applies the plan's hold rule ([`super::transition::is_held`])
//! at spawn and deploy time: an agent that would take a role while another
//! agent still runs one it is losing launches as an ordinary agent instead.
//! It is never an error — the process is stamped `None`, the plan reports it
//! stale, and the next restart once the hold releases promotes it. Spawn
//! callers already hold the runtimes lock, so they pass the live local roles
//! in ([`live_local_roles`]) instead of this module taking that lock again.

use std::collections::{BTreeMap, HashMap};

use tauri::AppHandle;

use super::{
    generated_routing_dir, load_channel_routing, routing_owner_hex, routing_role_for,
    transition::is_held, AgentRoutingState, ObservedProcess, RoutingRole,
};
use crate::managed_agents::{
    load_managed_agents, storage::atomic_write_json, BackendKind, ManagedAgentPairRuntime,
    ManagedAgentRecord, ManagedAgentRuntimeKey,
};

const DEPLOYED_ROLES_FILE: &str = "deployed-roles.json";

/// The running role of every tracked local process, by pubkey, read from
/// each pair's spawn stamp.
pub(crate) fn live_local_roles(
    runtimes: &HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>,
) -> Vec<(String, RoutingRole)> {
    runtimes
        .iter()
        .map(|(key, runtime)| (key.pubkey.clone(), runtime.spawn_config.routing_role))
        .collect()
}

/// Parse the deployed-roles sidecar. Missing or unreadable reads as empty
/// (every deployment then falls back to the legacy assumption below).
pub(crate) fn parse_deployed_roles(content: Option<&str>) -> HashMap<String, RoutingRole> {
    let Some(content) = content else {
        return HashMap::new();
    };
    serde_json::from_str(content).unwrap_or_else(|error| {
        eprintln!("buzz-desktop: unreadable {DEPLOYED_ROLES_FILE} ({error}); ignoring it");
        HashMap::new()
    })
}

fn deployed_roles_path<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<std::path::PathBuf, String> {
    Ok(generated_routing_dir(app)?.join(DEPLOYED_ROLES_FILE))
}

/// The role each provider deployment shipped with, by pubkey.
pub(crate) fn load_deployed_roles<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> HashMap<String, RoutingRole> {
    let Ok(path) = deployed_roles_path(app) else {
        return HashMap::new();
    };
    parse_deployed_roles(std::fs::read_to_string(path).ok().as_deref())
}

/// Stamp the role a successful provider deployment carried. Callers hold the
/// managed-agents store lock, which also serializes this read-modify-write.
pub(crate) fn record_deployed_role<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pubkey: &str,
    role: RoutingRole,
) -> Result<(), String> {
    let path = deployed_roles_path(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("failed to create {}: {error}", dir.display()))?;
    }
    let mut roles = parse_deployed_roles(std::fs::read_to_string(&path).ok().as_deref());
    roles.insert(pubkey.to_string(), role);
    let sorted: BTreeMap<_, _> = roles.into_iter().collect();
    let payload = serde_json::to_vec_pretty(&sorted)
        .map_err(|error| format!("failed to serialize deployed roles: {error}"))?;
    atomic_write_json(&path, &payload)
}

/// What is observable of a provider-backed record: not deployed → stopped;
/// deployed → the stamped role, or for a deployment made before stamps
/// existed, the role the star gave it then (Host was the only behavior).
pub(crate) fn observe_remote(
    record: &ManagedAgentRecord,
    stamp: Option<RoutingRole>,
) -> ObservedProcess {
    if record.backend_agent_id.is_none() {
        return ObservedProcess::Stopped;
    }
    ObservedProcess::Tracked(stamp.unwrap_or(if record.is_default_ai {
        RoutingRole::Dispatcher
    } else {
        RoutingRole::None
    }))
}

/// Pure core of [`launch_role`]: the desired role, unless the hold rule
/// says another agent must give up its role first.
pub(crate) fn launch_role_among(
    pubkey: &str,
    desired_role: RoutingRole,
    others: &[AgentRoutingState],
) -> RoutingRole {
    if is_held(pubkey, desired_role, others) {
        RoutingRole::None
    } else {
        desired_role
    }
}

/// The role a spawn or deploy of `record` launches with: `desired_role`
/// (from `routing_role_for`) gated by the hold rule over every other live
/// local process (`live_local`, from [`live_local_roles`]) and provider
/// deployment. A store that cannot be read skips
/// the gate (the launch then fails or succeeds on its own terms).
pub(crate) fn launch_role<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    desired_role: RoutingRole,
    live_local: &[(String, RoutingRole)],
) -> RoutingRole {
    if desired_role == RoutingRole::None {
        return RoutingRole::None;
    }
    let records = match load_managed_agents(app) {
        Ok(records) => records,
        Err(error) => {
            eprintln!("buzz-desktop: routing hold check skipped: {error}");
            return desired_role;
        }
    };
    let mode = load_channel_routing(app).unwrap_or_default();
    let owner = routing_owner_hex(app);
    let desired_of = |pubkey: &str| {
        records
            .iter()
            .find(|candidate| candidate.pubkey == pubkey)
            .map(|candidate| routing_role_for(candidate, mode, owner.as_deref()))
            .unwrap_or_default()
    };

    let mut others: Vec<AgentRoutingState> = live_local
        .iter()
        .filter(|(pubkey, _)| *pubkey != record.pubkey)
        .map(|(pubkey, running_role)| AgentRoutingState {
            desired_role: desired_of(pubkey),
            pubkey: pubkey.clone(),
            running: true,
            local: true,
            running_role: *running_role,
        })
        .collect();
    let stamps = load_deployed_roles(app);
    for other in records.iter().filter(|other| {
        other.pubkey != record.pubkey && matches!(other.backend, BackendKind::Provider { .. })
    }) {
        if let ObservedProcess::Tracked(running_role) =
            observe_remote(other, stamps.get(&other.pubkey).copied())
        {
            others.push(AgentRoutingState {
                pubkey: other.pubkey.clone(),
                running: true,
                local: false,
                running_role,
                desired_role: desired_of(&other.pubkey),
            });
        }
    }
    launch_role_among(&record.pubkey, desired_role, &others)
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;
