use super::{move_presets_in_dir, presets_moved, MoveReport};
use crate::managed_agents::{load_personas_from_path, AgentDefinition, PRESETS_FILE};
use crate::migration::test_support::{read_agents_json, write_agents_json, write_personas_json};
use std::path::{Path, PathBuf};

fn base(dir: &Path) -> PathBuf {
    dir.join("agents")
}

/// A preset as the agent store held it: a key-less row.
fn preset_row(slug: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "display_name": format!("{name} (display)"),
        "slug": slug,
        "pubkey": "",
        "system_prompt": format!("You are {name}."),
        "runtime": "claude",
        "model": "sonnet",
        "env_vars": { "PRESET_KEY": "1" },
        "definition_respond_to": "mentions",
        "definition_parallelism": 3,
        "is_active": false,
        "acp_command": "kuzz-acp",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-02T00:00:00Z"
    })
}

fn agent_row(name: &str, pubkey: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "pubkey": pubkey,
        "relay_url": "",
        "acp_command": "kuzz-acp",
        "agent_command": "claude-agent-acp",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 320,
        "parallelism": 2,
        "system_prompt": "own prompt",
        "source_preset_id": "custom:reviewer",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    })
}

fn presets(dir: &Path) -> Vec<AgentDefinition> {
    load_personas_from_path(&base(dir).join(PRESETS_FILE)).unwrap()
}

fn agents_bytes(dir: &Path) -> Vec<u8> {
    std::fs::read(base(dir).join("managed-agents.json")).unwrap()
}

#[test]
fn copies_preset_rows_and_leaves_the_agent_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    write_agents_json(
        dir.path(),
        &serde_json::json!([
            preset_row("custom:reviewer", "Reviewer"),
            agent_row("Rev", &"a".repeat(64)),
        ]),
    );
    let before = agents_bytes(dir.path());

    let report = move_presets_in_dir(&base(dir.path())).unwrap();
    assert_eq!(report.moved, vec!["custom:reviewer".to_string()]);
    assert!(presets_moved(&base(dir.path())));
    assert_eq!(
        agents_bytes(dir.path()),
        before,
        "the agent file is never rewritten by the move"
    );

    let moved = presets(dir.path());
    assert_eq!(moved.len(), 1);
    let preset = &moved[0];
    assert_eq!(preset.id, "custom:reviewer");
    assert_eq!(preset.display_name, "Reviewer (display)");
    assert_eq!(preset.system_prompt, "You are Reviewer.");
    assert_eq!(preset.runtime.as_deref(), Some("claude"));
    assert_eq!(preset.model.as_deref(), Some("sonnet"));
    assert_eq!(
        preset.env_vars.get("PRESET_KEY").map(String::as_str),
        Some("1")
    );
    assert_eq!(preset.respond_to.as_deref(), Some("mentions"));
    assert_eq!(preset.parallelism, Some(3));
    assert!(!preset.is_active);
    assert_eq!(
        preset.acp_command, None,
        "the default command is not stored"
    );
    assert_eq!(preset.updated_at, "2026-01-02T00:00:00Z");
}

#[test]
fn runs_once_so_a_deleted_preset_stays_deleted() {
    let dir = tempfile::tempdir().unwrap();
    write_agents_json(
        dir.path(),
        &serde_json::json!([preset_row("custom:reviewer", "Reviewer")]),
    );
    move_presets_in_dir(&base(dir.path())).unwrap();

    // The user deletes the preset; the old row is still in the agent file.
    std::fs::write(base(dir.path()).join(PRESETS_FILE), b"[]").unwrap();
    let report = move_presets_in_dir(&base(dir.path())).unwrap();
    assert_eq!(report, MoveReport::default());
    assert!(presets(dir.path()).is_empty());
    assert_eq!(read_agents_json(dir.path()).len(), 1, "leftover row kept");
}

#[test]
fn an_unfinished_earlier_run_keeps_what_presets_json_has() {
    let dir = tempfile::tempdir().unwrap();
    write_agents_json(
        dir.path(),
        &serde_json::json!([
            preset_row("custom:reviewer", "Reviewer"),
            preset_row("custom:writer", "Writer"),
        ]),
    );
    // Crash after presets.json was written but before the marker; an edit in
    // presets.json proves which copy wins on the retry.
    move_presets_in_dir(&base(dir.path())).unwrap();
    std::fs::remove_file(base(dir.path()).join("presets.json.moved")).unwrap();
    let mut edited = presets(dir.path());
    edited[0].system_prompt = "edited".to_string();
    std::fs::write(
        base(dir.path()).join(PRESETS_FILE),
        serde_json::to_vec(&edited).unwrap(),
    )
    .unwrap();

    let report = move_presets_in_dir(&base(dir.path())).unwrap();
    assert!(report.moved.is_empty());
    assert_eq!(report.already_there.len(), 2);
    let after = presets(dir.path());
    assert_eq!(after.len(), 2, "no duplicates");
    assert_eq!(after[0].system_prompt, "edited");
    assert!(presets_moved(&base(dir.path())));
}

#[test]
fn rows_without_a_slug_are_reported_not_moved() {
    let dir = tempfile::tempdir().unwrap();
    let mut row = preset_row("x", "Nameless");
    row.as_object_mut().unwrap().remove("slug");
    write_agents_json(dir.path(), &serde_json::json!([row]));

    let report = move_presets_in_dir(&base(dir.path())).unwrap();
    assert_eq!(report.unreadable, vec!["Nameless".to_string()]);
    assert!(presets(dir.path()).is_empty());
}

#[test]
fn reports_agents_whose_display_name_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = agent_row("Rev", &"a".repeat(64));
    agent["display_name"] = "Reviewer Bot".into();
    let mut same = agent_row("Same", &"b".repeat(64));
    same["display_name"] = "Same".into();
    write_agents_json(dir.path(), &serde_json::json!([agent, same]));

    let report = move_presets_in_dir(&base(dir.path())).unwrap();
    assert_eq!(report.display_names_dropped, vec!["Rev".to_string()]);
}

fn first_built_in() -> AgentDefinition {
    crate::managed_agents::built_in_persona_records("now")
        .into_iter()
        .next()
        .unwrap()
}

fn custom_persona_json(id: &str) -> serde_json::Value {
    let mut preset = first_built_in();
    preset.id = id.to_string();
    preset.is_builtin = false;
    preset.display_name = "Old".to_string();
    serde_json::to_value(preset).unwrap()
}

#[test]
fn a_very_old_personas_json_is_copied_without_built_ins() {
    let dir = tempfile::tempdir().unwrap();
    let builtin = serde_json::to_value(first_built_in()).unwrap();
    write_personas_json(
        dir.path(),
        &serde_json::json!([custom_persona_json("custom:old"), builtin]),
    );

    let report = move_presets_in_dir(&base(dir.path())).unwrap();
    assert_eq!(report.moved, vec!["custom:old".to_string()]);
    assert_eq!(presets(dir.path()).len(), 1);
}

#[test]
fn a_folded_personas_json_is_not_copied_again() {
    let dir = tempfile::tempdir().unwrap();
    write_personas_json(
        dir.path(),
        &serde_json::json!([custom_persona_json("custom:old")]),
    );
    std::fs::write(base(dir.path()).join("personas.json.bak"), b"[]").unwrap();

    let report = move_presets_in_dir(&base(dir.path())).unwrap();
    assert!(report.moved.is_empty());
    assert!(presets(dir.path()).is_empty());
}

#[test]
fn nothing_to_move_still_marks_done() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(base(dir.path())).unwrap();
    let report = move_presets_in_dir(&base(dir.path())).unwrap();
    assert_eq!(report, MoveReport::default());
    assert!(presets_moved(&base(dir.path())));
}
