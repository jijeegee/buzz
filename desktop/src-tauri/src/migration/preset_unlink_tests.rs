use super::{unlink_agents_from_presets_in_dir, UnlinkReport};
use crate::managed_agents::{AgentDefinition, ManagedAgentRecord, TeamRecord};
use crate::migration::test_support::{read_agents_json, write_agents_json};
use std::path::{Path, PathBuf};

fn base(dir: &Path) -> PathBuf {
    dir.join("agents")
}

/// A keyed agent as an older desktop stored it: linked through `persona_id`.
fn linked_agent_json(name: &str, pubkey: &str, persona_id: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "pubkey": pubkey,
        "persona_id": persona_id,
        "persona_source_version": "f".repeat(64),
        "relay_url": "",
        "acp_command": "kuzz-acp",
        "agent_command": "claude-agent-acp",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 320,
        "parallelism": 2,
        "system_prompt": "last started prompt",
        "model": "old-model",
        "env_vars": { "AGENT_ONLY": "1" },
        "team_id": "team-1",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    })
}

fn preset(id: &str) -> AgentDefinition {
    let mut preset = crate::managed_agents::built_in_persona_records("now")
        .into_iter()
        .next()
        .expect("a built-in preset");
    preset.id = id.to_string();
    preset.display_name = "Preset Name".to_string();
    preset.system_prompt = "You review code.".to_string();
    preset.model = Some("preset-model".to_string());
    preset.provider = Some("anthropic".to_string());
    preset.description = Some("Reviews changes.".to_string());
    preset.env_vars = [("PRESET_KEY".to_string(), "secret".to_string())].into();
    preset.effort_level = Some("high".to_string());
    preset
}

fn team() -> TeamRecord {
    serde_json::from_value(serde_json::json!({
        "id": "team-1", "name": "Team", "instructions": "  Coordinate.  ",
        "persona_ids": [], "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

fn load_typed(dir: &Path) -> Vec<ManagedAgentRecord> {
    serde_json::from_value(serde_json::Value::Array(read_agents_json(dir))).unwrap()
}

#[test]
fn copies_the_preset_and_keeps_only_a_label() {
    let dir = tempfile::tempdir().unwrap();
    let pubkey = "a".repeat(64);
    write_agents_json(
        dir.path(),
        &serde_json::json!([linked_agent_json("Reviewer", &pubkey, "custom:reviewer")]),
    );

    let report = unlink_agents_from_presets_in_dir(
        &base(dir.path()),
        &[preset("custom:reviewer")],
        &[team()],
    )
    .unwrap();
    assert_eq!(report.copied, vec!["Reviewer".to_string()]);

    let agent = &load_typed(dir.path())[0];
    assert_eq!(agent.name, "Reviewer", "the agent's own name is kept");
    assert_eq!(agent.system_prompt.as_deref(), Some("You review code."));
    assert_eq!(agent.model.as_deref(), Some("preset-model"));
    assert_eq!(agent.provider.as_deref(), Some("anthropic"));
    assert_eq!(agent.description.as_deref(), Some("Reviews changes."));
    assert_eq!(
        agent.env_vars.get("PRESET_KEY").map(String::as_str),
        Some("secret")
    );
    assert_eq!(
        agent.env_vars.get("AGENT_ONLY").map(String::as_str),
        Some("1")
    );
    assert_eq!(agent.effort_level.as_deref(), Some("high"));
    assert_eq!(agent.team_instructions.as_deref(), Some("Coordinate."));
    assert_eq!(agent.source_preset_id.as_deref(), Some("custom:reviewer"));
    assert_eq!(agent.legacy_persona_id, None);

    let stored = &read_agents_json(dir.path())[0];
    assert!(
        stored.get("persona_id").is_none(),
        "the link is gone on disk"
    );
    assert!(stored.get("persona_source_version").is_none());
}

#[test]
fn a_preset_made_at_the_agents_own_pubkey_leaves_no_label() {
    let dir = tempfile::tempdir().unwrap();
    let pubkey = "b".repeat(64);
    write_agents_json(
        dir.path(),
        &serde_json::json!([linked_agent_json("Solo", &pubkey, &pubkey)]),
    );

    unlink_agents_from_presets_in_dir(&base(dir.path()), &[preset(&pubkey)], &[]).unwrap();

    let agent = &load_typed(dir.path())[0];
    assert_eq!(agent.source_preset_id, None);
    assert_eq!(agent.system_prompt.as_deref(), Some("You review code."));
}

#[test]
fn an_agent_whose_preset_is_gone_keeps_its_own_settings() {
    let dir = tempfile::tempdir().unwrap();
    let pubkey = "c".repeat(64);
    write_agents_json(
        dir.path(),
        &serde_json::json!([linked_agent_json("Orphan", &pubkey, "custom:deleted")]),
    );

    let report = unlink_agents_from_presets_in_dir(&base(dir.path()), &[], &[team()]).unwrap();
    assert_eq!(report.orphans_unlinked, vec!["Orphan".to_string()]);

    let agent = &load_typed(dir.path())[0];
    assert_eq!(agent.system_prompt.as_deref(), Some("last started prompt"));
    assert_eq!(agent.model.as_deref(), Some("old-model"));
    assert_eq!(agent.team_instructions.as_deref(), Some("Coordinate."));
    assert_eq!(agent.source_preset_id.as_deref(), Some("custom:deleted"));
    assert_eq!(agent.legacy_persona_id, None);
}

#[test]
fn an_orphan_without_a_prompt_is_left_linked() {
    let dir = tempfile::tempdir().unwrap();
    let pubkey = "d".repeat(64);
    let mut agent = linked_agent_json("Empty", &pubkey, "custom:deleted");
    agent["system_prompt"] = serde_json::Value::Null;
    write_agents_json(dir.path(), &serde_json::json!([agent]));

    let report = unlink_agents_from_presets_in_dir(&base(dir.path()), &[], &[]).unwrap();
    assert_eq!(
        report,
        UnlinkReport {
            kept_linked: vec!["Empty".to_string()],
            ..UnlinkReport::default()
        }
    );
    assert_eq!(
        read_agents_json(dir.path())[0]["persona_id"],
        "custom:deleted"
    );
}

#[test]
fn a_second_run_is_a_no_op_and_the_backup_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let pubkey = "e".repeat(64);
    let original = serde_json::json!([linked_agent_json("Reviewer", &pubkey, "custom:reviewer")]);
    write_agents_json(dir.path(), &original);
    let presets = [preset("custom:reviewer")];

    unlink_agents_from_presets_in_dir(&base(dir.path()), &presets, &[]).unwrap();
    let after_first = read_agents_json(dir.path());
    let report = unlink_agents_from_presets_in_dir(&base(dir.path()), &presets, &[]).unwrap();

    assert_eq!(report, UnlinkReport::default());
    assert_eq!(read_agents_json(dir.path()), after_first);
    let backup =
        std::fs::read_to_string(base(dir.path()).join("managed-agents.json.before-preset-unlink"))
            .unwrap();
    let backup: serde_json::Value = serde_json::from_str(&backup).unwrap();
    assert_eq!(backup[0]["persona_id"], "custom:reviewer");
}

#[test]
fn preset_records_in_the_store_are_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let pubkey = "f".repeat(64);
    let definition = serde_json::json!({
        "name": "Reviewer preset",
        "pubkey": "",
        "slug": "custom:reviewer",
        "relay_url": "",
        "acp_command": "kuzz-acp",
        "agent_command": "",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 320,
        "system_prompt": "You review code.",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    });
    write_agents_json(
        dir.path(),
        &serde_json::json!([
            definition.clone(),
            linked_agent_json("Reviewer", &pubkey, "custom:reviewer")
        ]),
    );

    unlink_agents_from_presets_in_dir(&base(dir.path()), &[preset("custom:reviewer")], &[])
        .unwrap();

    let stored = read_agents_json(dir.path());
    assert_eq!(stored.len(), 2, "no preset is added or removed");
    assert_eq!(stored[0]["slug"], "custom:reviewer");
    assert_eq!(stored[0]["system_prompt"], "You review code.");
}
