use super::*;

#[test]
fn signed_team_transport_survives_adoption_and_agent_creation_copy() {
    let mut definition = persona("m1", "Do the work.");
    definition.acp_command = Some("kuzz-janet-acp".to_string());
    let (event, source) = published(&team_fixture(vec!["m1".to_string()]), &[definition], true);
    let content = verified_head_content(&event, &source, &event.id.to_hex()).unwrap();
    let (personas, _) = plan_add(&[], &[], &source, &content, NOW)
        .unwrap()
        .stores
        .unwrap();
    assert_eq!(personas[0].acp_command.as_deref(), Some("kuzz-janet-acp"));
    let mut agent: crate::managed_agents::ManagedAgentRecord =
        serde_json::from_value(serde_json::json!({
            "pubkey": "a".repeat(64),
            "name": "m1",
            "relay_url": "",
            "acp_command": "kuzz-acp",
            "agent_command": "goose",
            "agent_args": [],
            "mcp_command": "",
            "turn_timeout_seconds": 320,
            "created_at": NOW,
            "updated_at": NOW
        }))
        .unwrap();
    crate::managed_agents::persona_events::copy_preset_into_agent(&mut agent, &personas[0]);
    assert_eq!(agent.acp_command, "kuzz-janet-acp");
}
