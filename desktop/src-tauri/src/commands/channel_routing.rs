//! Agents page › "Channel routing" commands.
//!
//! `set_channel_routing` is the single write path for both the routing mode
//! and the routing agent (the default-AI star); `get_channel_routing` reports
//! the saved mode together with the transition plan, so the card and the
//! auto-restart loop read the same applied state.

use std::collections::HashMap;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::{
    app_state::AppState,
    managed_agents::{
        channel_routing::{
            load_channel_routing, observed_routing_state, plan_routing_transition,
            routing_owner_hex, routing_role_for, save_channel_routing, AgentTransition,
            AppliedRouting, ChannelRoutingMode, ObservedProcess,
        },
        current_instance_id, find_managed_agent_mut, load_managed_agents, process_is_running,
        save_managed_agents, set_default_ai, sync_managed_agent_processes, workspace_pair_key,
        BackendKind, ManagedAgentPairRuntime, ManagedAgentRecord, ManagedAgentRuntimeKey,
    },
    util::now_iso,
};

/// What the card renders: the saved mode and agent plus the applied state.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRoutingStatus {
    mode: ChannelRoutingMode,
    /// The starred routing agent, if any.
    routing_agent: Option<String>,
    applied: AppliedRouting,
    router_active: bool,
    agents: Vec<AgentTransition>,
}

/// What this desktop can observe of `record`'s process in the active
/// workspace: the tracked pair's spawn stamp, an untracked live pid, or a
/// provider deployment.
fn observe_process<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    runtimes: &HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>,
) -> ObservedProcess {
    if record.backend != BackendKind::Local {
        return if record.backend_agent_id.is_some() {
            ObservedProcess::Unstamped
        } else {
            ObservedProcess::Stopped
        };
    }
    if let Some(runtime) = workspace_pair_key(app, record).and_then(|key| runtimes.get(&key)) {
        return ObservedProcess::Tracked(runtime.spawn_config.routing_role);
    }
    if record.runtime_pid.is_some_and(process_is_running) {
        return ObservedProcess::Unstamped;
    }
    ObservedProcess::Stopped
}

fn routing_status<R: tauri::Runtime>(
    app: &AppHandle<R>,
    records: &[ManagedAgentRecord],
    runtimes: &HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>,
    mode: ChannelRoutingMode,
) -> ChannelRoutingStatus {
    let owner = routing_owner_hex(app);
    let keyed = records.iter().filter(|record| !record.pubkey.is_empty());
    let states: Vec<_> = keyed
        .clone()
        .map(|record| {
            observed_routing_state(
                record,
                routing_role_for(record, mode, owner.as_deref()),
                observe_process(app, record, runtimes),
            )
        })
        .collect();
    let plan = plan_routing_transition(mode, &states);
    ChannelRoutingStatus {
        mode,
        routing_agent: keyed
            .filter(|record| record.is_default_ai)
            .map(|record| record.pubkey.clone())
            .next(),
        applied: plan.applied,
        router_active: plan.router_active,
        agents: plan.agents,
    }
}

/// Lock the store, sync process state, run `body`, and report the status —
/// the same boundary the other managed-agent record commands use.
fn with_routing_store(
    app: &AppHandle,
    body: impl FnOnce(&mut Vec<ManagedAgentRecord>) -> Result<ChannelRoutingMode, String>,
) -> Result<ChannelRoutingStatus, String> {
    let state = app.state::<AppState>();
    let _store_guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let mut records = load_managed_agents(app)?;
    let mut runtimes = state
        .managed_agent_processes
        .lock()
        .map_err(|error| error.to_string())?;

    let (sync_changed, exited_pubkeys) =
        sync_managed_agent_processes(&mut records, &mut runtimes, &current_instance_id(app));
    if sync_changed {
        save_managed_agents(app, &records)?;
    }
    for pubkey in &exited_pubkeys {
        state.clear_agent_session_caches(pubkey);
    }

    let mode = body(&mut records)?;
    Ok(routing_status(app, &records, &runtimes, mode))
}

/// The saved channel routing mode, routing agent, and transition plan.
#[tauri::command]
pub async fn get_channel_routing(app: AppHandle) -> Result<ChannelRoutingStatus, String> {
    tokio::task::spawn_blocking(move || with_routing_store(&app, |_| load_channel_routing(&app)))
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

/// Save the routing mode and, for a mode that needs one, the routing agent —
/// one user action. The star is written first, then the mode: a failure
/// between the two leaves the star moved under the old mode, which is itself
/// a valid state (rule 5: every prefix is consistent). Off keeps the star so
/// switching back remembers the agent.
#[tauri::command]
pub async fn set_channel_routing(
    mode: ChannelRoutingMode,
    agent_pubkey: Option<String>,
    app: AppHandle,
) -> Result<ChannelRoutingStatus, String> {
    if !mode.is_selectable() {
        return Err("That channel routing mode isn't available yet.".to_string());
    }
    tokio::task::spawn_blocking(move || {
        with_routing_store(&app, |records| {
            if mode == ChannelRoutingMode::Host {
                let pubkey = agent_pubkey
                    .as_deref()
                    .map(str::trim)
                    .filter(|pubkey| !pubkey.is_empty())
                    .ok_or_else(|| "Choose a host agent to turn Host on.".to_string())?;
                let changed = set_default_ai(records, Some(pubkey))?;
                if !changed.is_empty() {
                    let now = now_iso();
                    for changed_pubkey in &changed {
                        find_managed_agent_mut(records, changed_pubkey)?.updated_at = now.clone();
                    }
                    save_managed_agents(&app, records)?;
                }
            }
            save_channel_routing(&app, mode)?;
            Ok(mode)
        })
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}
