//! Mode transitions without overlap.
//!
//! A routing role is spawn-time env, so it changes only when the process
//! restarts. Between the save and the restarts the old and the new role could
//! both be live (an old host still dispatching while a new one starts), and
//! the same message would be handled twice. [`plan_routing_transition`] is the
//! one place that ordering is decided: agents **losing** a role restart first,
//! agents **gaining** one are held until no other process still runs a role,
//! and the desktop router turns on only once every running agent is plain.

use serde::Serialize;

use super::{ChannelRoutingMode, RoutingRole};

/// What the plan needs to know about one agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRoutingState {
    /// The agent's pubkey.
    pub pubkey: String,
    /// A process (or provider deployment) for this agent is live.
    pub running: bool,
    /// Runs on this computer (its restart is the desktop's to make).
    pub local: bool,
    /// The role the live process was spawned with; `None` when not running.
    pub running_role: RoutingRole,
    /// The role [`super::routing_role_for`] gives it under the saved mode.
    pub desired_role: RoutingRole,
}

/// One agent's place in the plan, as the card and auto-restart read it.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentTransition {
    /// The agent's pubkey.
    pub pubkey: String,
    /// Mirrors [`AgentRoutingState::running`].
    pub running: bool,
    /// Mirrors [`AgentRoutingState::local`].
    pub local: bool,
    /// Mirrors [`AgentRoutingState::running_role`].
    pub running_role: RoutingRole,
    /// Mirrors [`AgentRoutingState::desired_role`].
    pub desired_role: RoutingRole,
    /// Running with a role other than the desired one: a restart applies it.
    pub stale: bool,
    /// Must not (re)start yet: another running agent still holds a role it
    /// is losing. Gates auto-restart and the card's "Restart now".
    pub hold: bool,
}

/// What actually routes right now (not what was saved).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum AppliedRouting {
    /// Nothing routes unmentioned messages.
    Off,
    /// This agent is running as the channel dispatcher.
    Hosting {
        /// The hosting agent.
        pubkey: String,
    },
    /// This agent is running as the channel lead.
    Leading {
        /// The leading agent.
        pubkey: String,
    },
    /// The desktop router assigns sends.
    SmartRouting,
    /// At least one running agent still has its old role.
    Switching,
}

/// The whole plan for the current mode and processes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionPlan {
    /// Per-agent stale/hold, in input order.
    pub agents: Vec<AgentTransition>,
    /// What runs right now.
    pub applied: AppliedRouting,
    /// The desktop router may run: Smart routing is saved and no running
    /// agent still holds a role.
    pub router_active: bool,
}

fn is_stale(agent: &AgentRoutingState) -> bool {
    agent.running && agent.running_role != agent.desired_role
}

/// Running with a role the saved mode takes away (or changes): this process
/// must restart before anyone else may take a role.
fn holds_role_it_is_losing(agent: &AgentRoutingState) -> bool {
    is_stale(agent) && agent.running_role != RoutingRole::None
}

/// The hold rule, shared by the plan and by every launch path: an agent that
/// wants a role waits while any **other** agent still runs a role it is
/// losing. Spawn and deploy call this through `launch_role` so a manual
/// Start/Restart/redeploy cannot bypass what auto-restart already respects.
pub(crate) fn is_held(
    pubkey: &str,
    desired_role: RoutingRole,
    agents: &[AgentRoutingState],
) -> bool {
    desired_role != RoutingRole::None
        && agents
            .iter()
            .any(|other| other.pubkey != pubkey && holds_role_it_is_losing(other))
}

/// Plan the transition from what runs to what `mode` wants.
///
/// - `stale`: running and `running_role != desired_role`.
/// - `hold`: wants a role while some **other** running agent is stale and
///   still holds one — that agent must restart (lose it) first.
/// - `router_active`: Smart routing is saved and every running agent is plain.
/// - `applied`: `Switching` while anything is stale, otherwise the role a
///   running agent holds, the router, or `Off`.
pub fn plan_routing_transition(
    mode: ChannelRoutingMode,
    agents: &[AgentRoutingState],
) -> TransitionPlan {
    let transitions: Vec<AgentTransition> = agents
        .iter()
        .map(|agent| AgentTransition {
            pubkey: agent.pubkey.clone(),
            running: agent.running,
            local: agent.local,
            running_role: if agent.running {
                agent.running_role
            } else {
                RoutingRole::None
            },
            desired_role: agent.desired_role,
            stale: is_stale(agent),
            hold: is_held(&agent.pubkey, agent.desired_role, agents),
        })
        .collect();

    let router_active = mode == ChannelRoutingMode::DesktopRouter
        && agents
            .iter()
            .all(|agent| !agent.running || agent.running_role == RoutingRole::None);

    let applied = if transitions.iter().any(|agent| agent.stale) {
        AppliedRouting::Switching
    } else if router_active {
        AppliedRouting::SmartRouting
    } else {
        transitions
            .iter()
            .filter(|agent| agent.running)
            .find_map(|agent| match agent.running_role {
                RoutingRole::Dispatcher => Some(AppliedRouting::Hosting {
                    pubkey: agent.pubkey.clone(),
                }),
                RoutingRole::Lead => Some(AppliedRouting::Leading {
                    pubkey: agent.pubkey.clone(),
                }),
                RoutingRole::None => None,
            })
            .unwrap_or(AppliedRouting::Off)
    };

    TransitionPlan {
        agents: transitions,
        applied,
        router_active,
    }
}

#[cfg(test)]
#[path = "transition_tests.rs"]
mod tests;
