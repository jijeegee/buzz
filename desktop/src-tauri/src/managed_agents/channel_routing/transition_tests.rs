//! Exclusivity property for [`plan_routing_transition`]: across every mode
//! switch, star move, and set of running agents, no reachable state ever runs
//! two routing paths at once, and every switch can finish.

use std::collections::{HashSet, VecDeque};

use super::*;
use crate::managed_agents::channel_routing::{routing_role_for, ChannelRoutingMode};
use crate::managed_agents::{BackendKind, ManagedAgentRecord};

const MODES: [ChannelRoutingMode; 4] = [
    ChannelRoutingMode::Off,
    ChannelRoutingMode::Host,
    ChannelRoutingMode::Lead,
    ChannelRoutingMode::DesktopRouter,
];
const OWNER: &str = "owner-hex";

fn agent(running: bool, running_role: RoutingRole, desired_role: RoutingRole) -> AgentRoutingState {
    AgentRoutingState {
        pubkey: String::new(),
        running,
        local: true,
        running_role,
        desired_role,
    }
}

fn named(mut state: AgentRoutingState, pubkey: &str) -> AgentRoutingState {
    state.pubkey = pubkey.to_string();
    state
}

#[test]
fn a_stale_agent_losing_a_role_holds_the_agent_gaining_one() {
    // Host moves from A to B while both run: A must restart first.
    let plan = plan_routing_transition(
        ChannelRoutingMode::Host,
        &[
            named(agent(true, RoutingRole::Dispatcher, RoutingRole::None), "a"),
            named(agent(true, RoutingRole::None, RoutingRole::Dispatcher), "b"),
        ],
    );
    assert_eq!(plan.applied, AppliedRouting::Switching);
    assert!(plan.agents[0].stale && !plan.agents[0].hold);
    assert!(plan.agents[1].stale && plan.agents[1].hold);
    assert!(!plan.router_active);
}

#[test]
fn one_process_switching_roles_is_never_held_by_itself() {
    let plan = plan_routing_transition(
        ChannelRoutingMode::Lead,
        &[named(
            agent(true, RoutingRole::Dispatcher, RoutingRole::Lead),
            "a",
        )],
    );
    assert!(plan.agents[0].stale);
    assert!(
        !plan.agents[0].hold,
        "Host → Lead on the same agent is one restart"
    );
}

#[test]
fn the_router_waits_for_every_running_role_to_clear() {
    let switching = plan_routing_transition(
        ChannelRoutingMode::DesktopRouter,
        &[named(
            agent(true, RoutingRole::Dispatcher, RoutingRole::None),
            "a",
        )],
    );
    assert!(!switching.router_active);
    assert_eq!(switching.applied, AppliedRouting::Switching);

    let done = plan_routing_transition(
        ChannelRoutingMode::DesktopRouter,
        &[named(
            agent(true, RoutingRole::None, RoutingRole::None),
            "a",
        )],
    );
    assert!(done.router_active);
    assert_eq!(done.applied, AppliedRouting::SmartRouting);
}

#[test]
fn applied_names_the_running_host_and_ignores_a_stopped_one() {
    let hosting = plan_routing_transition(
        ChannelRoutingMode::Host,
        &[named(
            agent(true, RoutingRole::Dispatcher, RoutingRole::Dispatcher),
            "a",
        )],
    );
    assert_eq!(
        hosting.applied,
        AppliedRouting::Hosting { pubkey: "a".into() }
    );
    // A stopped routing agent: nothing hosts, nothing is stale.
    let stopped = plan_routing_transition(
        ChannelRoutingMode::Host,
        &[named(
            agent(false, RoutingRole::Dispatcher, RoutingRole::Dispatcher),
            "a",
        )],
    );
    assert_eq!(stopped.applied, AppliedRouting::Off);
    assert_eq!(stopped.agents[0].running_role, RoutingRole::None);
    assert!(!stopped.agents[0].stale);
}

// ── Exhaustive exploration ──────────────────────────────────────────────────

/// One agent in the simulated world: its record facts and its process.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Sim {
    local: bool,
    running: bool,
    running_role: RoutingRole,
}

fn record(index: usize, starred: bool, local: bool) -> ManagedAgentRecord {
    let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": format!("{index:064x}"),
        "name": format!("agent-{index}"),
        "private_key_nsec": "",
        "relay_url": "",
        "acp_command": "buzz-acp",
        "agent_command": "goose",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 300,
        "created_at": "",
        "updated_at": "",
    }))
    .expect("minimal record deserializes");
    record.is_default_ai = starred;
    if !local {
        record.backend = BackendKind::Provider {
            id: "cloud".into(),
            config: serde_json::Value::Null,
        };
    }
    record
}

fn desired(
    world: &[Sim],
    star: Option<usize>,
    mode: ChannelRoutingMode,
    owner: Option<&str>,
) -> Vec<RoutingRole> {
    world
        .iter()
        .enumerate()
        .map(|(index, sim)| {
            routing_role_for(&record(index, star == Some(index), sim.local), mode, owner)
        })
        .collect()
}

fn states(world: &[Sim], desired: &[RoutingRole]) -> Vec<AgentRoutingState> {
    world
        .iter()
        .zip(desired)
        .enumerate()
        .map(|(index, (sim, desired))| AgentRoutingState {
            pubkey: format!("{index:064x}"),
            running: sim.running,
            local: sim.local,
            running_role: sim.running_role,
            desired_role: *desired,
        })
        .collect()
}

/// At most one routing path at any moment: running role-holders plus the
/// router, counted together.
fn routing_paths(world: &[Sim], plan: &TransitionPlan) -> usize {
    world
        .iter()
        .filter(|sim| sim.running && sim.running_role != RoutingRole::None)
        .count()
        + usize::from(plan.router_active)
}

/// Explore every interleaving of start/restart/stop after a switch. A start
/// or restart obeys `hold` unless `ignore_hold` (the broken variant). Returns
/// the first violating world, or `None` when the invariant and liveness hold.
fn explore(
    initial: Vec<Sim>,
    desired: &[RoutingRole],
    mode: ChannelRoutingMode,
    ignore_hold: bool,
) -> Option<String> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([initial]);
    while let Some(world) = queue.pop_front() {
        if !seen.insert(world.clone()) {
            continue;
        }
        let plan = plan_routing_transition(mode, &states(&world, desired));
        if routing_paths(&world, &plan) > 1 {
            return Some(format!("two routing paths in {world:?} (mode {mode:?})"));
        }
        // Liveness: someone can always make progress while anything is stale.
        let stale = plan.agents.iter().any(|agent| agent.stale);
        let can_progress = plan.agents.iter().any(|agent| agent.stale && !agent.hold);
        if stale && !can_progress {
            return Some(format!("stuck switching in {world:?} (mode {mode:?})"));
        }
        for (index, transition) in plan.agents.iter().enumerate() {
            let allowed = ignore_hold || !transition.hold;
            if allowed {
                // (Re)start: the new process gets its desired role.
                let mut next = world.clone();
                next[index].running = true;
                next[index].running_role = desired[index];
                queue.push_back(next);
            }
            if world[index].running {
                let mut next = world.clone();
                next[index].running = false;
                next[index].running_role = RoutingRole::None;
                queue.push_back(next);
            }
        }
    }
    None
}

/// Every starting world: each agent local/remote × stopped/running, with the
/// running ones applied for the previous mode and star.
fn initial_worlds(
    count: usize,
    from_mode: ChannelRoutingMode,
    from_star: Option<usize>,
    owner: Option<&str>,
) -> Vec<Vec<Sim>> {
    let mut worlds = Vec::new();
    for bits in 0..(1u32 << (2 * count)) {
        let mut world: Vec<Sim> = (0..count)
            .map(|index| Sim {
                local: bits & (1 << (2 * index)) != 0,
                running: bits & (1 << (2 * index + 1)) != 0,
                running_role: RoutingRole::None,
            })
            .collect();
        let applied = desired(&world, from_star, from_mode, owner);
        for (sim, role) in world.iter_mut().zip(applied) {
            if sim.running {
                sim.running_role = role;
            }
        }
        worlds.push(world);
    }
    worlds
}

fn for_every_switch(mut check: impl FnMut(Vec<Sim>, Vec<RoutingRole>, ChannelRoutingMode)) {
    const AGENTS: usize = 3;
    let stars: Vec<Option<usize>> = std::iter::once(None).chain((0..AGENTS).map(Some)).collect();
    for owner in [Some(OWNER), None] {
        for from_mode in MODES {
            for to_mode in MODES {
                for &from_star in &stars {
                    for &to_star in &stars {
                        for world in initial_worlds(AGENTS, from_mode, from_star, owner) {
                            let want = desired(&world, to_star, to_mode, owner);
                            check(world, want, to_mode);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn no_switch_ever_runs_two_routing_paths_and_every_switch_finishes() {
    for_every_switch(|world, want, mode| {
        if let Some(violation) = explore(world, &want, mode, false) {
            panic!("{violation}");
        }
    });
}

#[test]
fn ignoring_hold_is_caught_by_the_same_exploration() {
    // Falsifiability: the property must fail when restarts skip the hold.
    let mut violations = 0;
    for_every_switch(|world, want, mode| {
        if explore(world, &want, mode, true).is_some() {
            violations += 1;
        }
    });
    assert!(
        violations > 0,
        "a hold-ignoring variant must overlap somewhere"
    );
}
