//! Backend tests for `agent_command` edits and the record effort-alias strip.
//! An agent owns its runtime and effort, so a harness edit only sets or clears
//! the pin — it never touches the agent's runtime, effort column or env.
//! Split out of `discovery/tests.rs` to hold that file under the desktop
//! file-size ratchet.
//!
//! `use super::*` pulls the parent test module's helpers (`record_with`,
//! `record_agent_command`) and its imported command surface
//! (`apply_agent_command_update`, `remove_record_effort_aliases`).

use super::*;

#[test]
fn apply_agent_command_update_empty_command_clears_pin_keeps_runtime_and_column() {
    // An empty command clears the pin; the agent then runs its own runtime.
    // Its runtime and effort column are left untouched.
    let mut record = record_with(Some("claude"), Some("p1"), Some("codex-acp"));
    record.effort_level = Some("high".into());

    apply_agent_command_update(&mut record, "");

    assert_eq!(record.agent_command_override, None);
    assert_eq!(record.runtime.as_deref(), Some("claude"));
    assert_eq!(
        record.effort_level.as_deref(),
        Some("high"),
        "clearing the pin must preserve the effort column"
    );
    assert_eq!(record_agent_command(&record), "claude-agent-acp");
}

#[test]
fn apply_agent_command_update_concrete_pin_keeps_runtime_and_column() {
    // A concrete pick of a different runtime only sets the pin; the runtime
    // and the effort column are left intact. The pin shadows the runtime.
    let mut record = record_with(Some("claude"), Some("p1"), None);
    record.effort_level = Some("high".into());

    apply_agent_command_update(&mut record, "codex-acp");

    assert_eq!(record.agent_command_override.as_deref(), Some("codex-acp"));
    assert_eq!(record.runtime.as_deref(), Some("claude"));
    assert_eq!(
        record.effort_level.as_deref(),
        Some("high"),
        "a concrete pin must preserve the effort column"
    );
    assert_eq!(record_agent_command(&record), "codex-acp");
}

#[test]
fn apply_agent_command_update_same_runtime_pick_is_not_a_pin() {
    // Restating the agent's own runtime (even through an alias command of the
    // same runtime) does not pin.
    let mut record = record_with(Some("claude"), None, Some("codex-acp"));

    apply_agent_command_update(&mut record, "claude-code-acp");

    assert_eq!(record.agent_command_override, None);
    assert_eq!(record_agent_command(&record), "claude-agent-acp");
}

#[test]
fn remove_record_effort_aliases_strips_all_known_and_legacy_keys() {
    // Every known native effort key and the legacy alias must be removed,
    // while unrelated env survives.
    let mut env: std::collections::BTreeMap<String, String> = [
        ("GOOSE_THINKING_EFFORT", "high"),
        ("KUZZ_AGENT_THINKING_EFFORT", "high"),
        ("KUZZ_ACP_EFFORT_LEVEL", "high"),
        ("UNRELATED_KEY", "keep"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();

    remove_record_effort_aliases(&mut env);

    assert!(!env.contains_key("GOOSE_THINKING_EFFORT"));
    assert!(!env.contains_key("KUZZ_AGENT_THINKING_EFFORT"));
    assert!(!env.contains_key("KUZZ_ACP_EFFORT_LEVEL"));
    assert_eq!(
        env.get("UNRELATED_KEY").map(String::as_str),
        Some("keep"),
        "unrelated env must survive the effort-alias strip"
    );
}
