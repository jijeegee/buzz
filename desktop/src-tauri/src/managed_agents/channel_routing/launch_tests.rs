use super::*;
use crate::managed_agents::channel_routing::{
    observed_routing_state, plan_routing_transition, ChannelRoutingMode,
};

fn record(pubkey: &str, starred: bool, deployed: bool) -> ManagedAgentRecord {
    let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": pubkey,
        "name": pubkey,
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
    record.backend = BackendKind::Provider {
        id: "cloud".into(),
        config: serde_json::Value::Null,
    };
    record.backend_agent_id = deployed.then(|| "remote-1".to_string());
    record
}

#[test]
fn a_provider_deployment_is_observed_through_its_stamp_or_the_legacy_star() {
    use RoutingRole::{Dispatcher, None as Plain};
    // Not deployed: nothing runs, whatever a stale stamp says.
    assert_eq!(
        observe_remote(&record("a", true, false), Some(Dispatcher)),
        ObservedProcess::Stopped
    );
    // Stamped: the stamp wins over the current star.
    assert_eq!(
        observe_remote(&record("a", false, true), Some(Dispatcher)),
        ObservedProcess::Tracked(Dispatcher)
    );
    assert_eq!(
        observe_remote(&record("a", true, true), Some(Plain)),
        ObservedProcess::Tracked(Plain)
    );
    // Deployed before stamps existed: the star decided it (Host only).
    assert_eq!(
        observe_remote(&record("a", true, true), None),
        ObservedProcess::Tracked(Dispatcher)
    );
    assert_eq!(
        observe_remote(&record("a", false, true), None),
        ObservedProcess::Tracked(Plain)
    );
}

#[test]
fn a_remote_host_losing_the_star_is_stale_and_holds_the_local_gainer() {
    // Honey (remote) was deployed as host; the star moved to Fizz (local).
    let honey = record("honey", false, true);
    let honey_state = observed_routing_state(
        &honey,
        RoutingRole::None,
        observe_remote(&honey, Some(RoutingRole::Dispatcher)),
    );
    let fizz = AgentRoutingState {
        pubkey: "fizz".into(),
        running: true,
        local: true,
        running_role: RoutingRole::None,
        desired_role: RoutingRole::Dispatcher,
    };
    let plan = plan_routing_transition(ChannelRoutingMode::Host, &[honey_state, fizz]);

    let honey = &plan.agents[0];
    assert!(honey.stale && !honey.local, "the remote host must redeploy");
    assert_eq!(honey.running_role, RoutingRole::Dispatcher);
    assert!(
        plan.agents[1].hold,
        "the local gainer waits for the redeploy"
    );
}

#[test]
fn a_launch_while_another_agent_still_runs_its_old_role_is_stamped_plain() {
    let old_host = AgentRoutingState {
        pubkey: "honey".into(),
        running: true,
        local: false,
        running_role: RoutingRole::Dispatcher,
        desired_role: RoutingRole::None,
    };
    // Held: Fizz launches as an ordinary agent, no error.
    assert_eq!(
        launch_role_among(
            "fizz",
            RoutingRole::Dispatcher,
            std::slice::from_ref(&old_host)
        ),
        RoutingRole::None
    );
    // Once Honey has restarted plain, Fizz gets the role.
    let restarted = AgentRoutingState {
        running_role: RoutingRole::None,
        ..old_host.clone()
    };
    assert_eq!(
        launch_role_among("fizz", RoutingRole::Dispatcher, &[restarted]),
        RoutingRole::Dispatcher
    );
    // An agent never holds itself (Host → Lead on one process is one restart).
    assert_eq!(
        launch_role_among("honey", RoutingRole::Lead, &[old_host]),
        RoutingRole::Lead
    );
    // A plain launch is never affected.
    assert_eq!(
        launch_role_among("fizz", RoutingRole::None, &[]),
        RoutingRole::None
    );
}

#[test]
fn the_deployed_roles_sidecar_parses_and_ignores_damage() {
    assert!(parse_deployed_roles(None).is_empty());
    assert!(parse_deployed_roles(Some("not json")).is_empty());
    let roles = parse_deployed_roles(Some(r#"{"honey":"dispatcher","fizz":"none"}"#));
    assert_eq!(roles.get("honey"), Some(&RoutingRole::Dispatcher));
    assert_eq!(roles.get("fizz"), Some(&RoutingRole::None));
}
