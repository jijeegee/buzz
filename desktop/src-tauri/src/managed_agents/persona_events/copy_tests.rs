//! `copy_preset_into_agent`: one copy of the preset into the agent at create
//! time, one test per rule in the function's doc comment.

use std::collections::BTreeMap;

use super::copy_preset_into_agent;
use super::tests::{sample_persona, sample_record};
use crate::managed_agents::{AcpSessionPolicy, RelayMeshConfig, DEFAULT_ACP_COMMAND};

const EFFORT_KEY: &str = "GOOSE_THINKING_EFFORT";

fn assert_effort_key_is_known() {
    assert!(
        crate::managed_agents::config_bridge::effort::effort_suppress_keys().contains(&EFFORT_KEY),
        "{EFFORT_KEY} must be an effort env key for these tests to mean anything"
    );
}

#[test]
fn copies_prompt_model_provider_runtime_and_session_policy() {
    let mut record = sample_record();
    record.system_prompt = Some("old prompt".into());
    record.model = Some("old-model".into());
    record.provider = Some("old-provider".into());
    record.runtime = Some("claude".into());
    let mut preset = sample_persona();
    preset.session_policy = AcpSessionPolicy::Thread;

    copy_preset_into_agent(&mut record, &preset);

    assert_eq!(
        record.system_prompt.as_deref(),
        Some("You are a test assistant.")
    );
    assert_eq!(record.model.as_deref(), Some("claude-opus-4"));
    assert_eq!(record.provider.as_deref(), Some("anthropic"));
    assert_eq!(record.runtime.as_deref(), Some("goose"));
    assert_eq!(record.session_policy, AcpSessionPolicy::Thread);
    assert_eq!(record.source_preset_id.as_deref(), Some("test-persona"));
}

/// A blank preset model/provider/runtime stays blank on the agent (global
/// default at launch) — the agent's old values are not kept.
#[test]
fn blank_preset_model_provider_runtime_clear_the_agent_values() {
    let mut record = sample_record();
    record.model = Some("old-model".into());
    record.provider = Some("old-provider".into());
    record.runtime = Some("claude".into());
    let mut preset = sample_persona();
    preset.model = None;
    preset.provider = None;
    preset.runtime = None;

    copy_preset_into_agent(&mut record, &preset);

    assert_eq!(record.model, None);
    assert_eq!(record.provider, None);
    assert_eq!(record.runtime, None);
}

#[test]
fn acp_command_is_copied_and_absent_means_default() {
    let mut record = sample_record();
    record.acp_command = "/opt/old-acp".into();
    let mut preset = sample_persona();

    preset.acp_command = Some("kuzz-janet-acp".into());
    copy_preset_into_agent(&mut record, &preset);
    assert_eq!(record.acp_command, "kuzz-janet-acp");

    preset.acp_command = None;
    copy_preset_into_agent(&mut record, &preset);
    assert_eq!(record.acp_command, DEFAULT_ACP_COMMAND);

    preset.acp_command = Some("/opt/custom-acp".into());
    copy_preset_into_agent(&mut record, &preset);
    assert_eq!(record.acp_command, "/opt/custom-acp");
}

#[test]
fn env_merges_with_the_agent_winning_per_key() {
    let mut record = sample_record();
    record.env_vars = BTreeMap::from([
        ("KEY".to_string(), "agent".to_string()),
        ("AGENT_ONLY".to_string(), "a".to_string()),
    ]);
    let mut preset = sample_persona();
    preset.env_vars = BTreeMap::from([
        ("KEY".to_string(), "preset".to_string()),
        ("PRESET_ONLY".to_string(), "p".to_string()),
    ]);

    copy_preset_into_agent(&mut record, &preset);

    assert_eq!(
        record.env_vars,
        BTreeMap::from([
            ("AGENT_ONLY".to_string(), "a".to_string()),
            ("KEY".to_string(), "agent".to_string()),
            ("PRESET_ONLY".to_string(), "p".to_string()),
        ])
    );
}

/// Agent without its own effort: the preset's column and effort env keys
/// are copied.
#[test]
fn effort_is_copied_when_the_agent_has_none() {
    assert_effort_key_is_known();
    let mut record = sample_record();
    let mut preset = sample_persona();
    preset.effort_level = Some("high".into());
    preset
        .env_vars
        .insert(EFFORT_KEY.to_string(), "medium".to_string());

    copy_preset_into_agent(&mut record, &preset);

    assert_eq!(record.effort_level.as_deref(), Some("high"));
    assert_eq!(
        record.env_vars.get(EFFORT_KEY).map(String::as_str),
        Some("medium")
    );
}

/// Agent with its own effort column: it keeps only that; the preset's column
/// and effort env keys are not copied. Non-effort env still merges.
#[test]
fn agent_effort_column_blocks_all_preset_effort() {
    assert_effort_key_is_known();
    let mut record = sample_record();
    record.effort_level = Some("low".into());
    let mut preset = sample_persona();
    preset.effort_level = Some("high".into());
    preset
        .env_vars
        .insert(EFFORT_KEY.to_string(), "medium".to_string());

    copy_preset_into_agent(&mut record, &preset);

    assert_eq!(record.effort_level.as_deref(), Some("low"));
    assert!(!record.env_vars.contains_key(EFFORT_KEY));
    assert_eq!(
        record.env_vars.get("KEY").map(String::as_str),
        Some("value")
    );
}

/// An effort env key on the agent counts as "agent has effort" too, matched
/// case-insensitively.
#[test]
fn agent_effort_env_key_blocks_preset_effort() {
    assert_effort_key_is_known();
    let mut record = sample_record();
    record
        .env_vars
        .insert(EFFORT_KEY.to_ascii_lowercase(), "low".to_string());
    let mut preset = sample_persona();
    preset.effort_level = Some("high".into());
    preset
        .env_vars
        .insert(EFFORT_KEY.to_string(), "medium".to_string());

    copy_preset_into_agent(&mut record, &preset);

    assert_eq!(record.effort_level, None);
    assert!(!record.env_vars.contains_key(EFFORT_KEY));
    assert_eq!(
        record
            .env_vars
            .get(&EFFORT_KEY.to_ascii_lowercase())
            .map(String::as_str),
        Some("low")
    );
}

#[test]
fn description_is_copied_avatar_only_when_agent_has_none() {
    let mut record = sample_record();
    record.description = Some("old description".into());
    let mut preset = sample_persona();
    preset.description = Some("preset description".into());

    copy_preset_into_agent(&mut record, &preset);
    assert_eq!(record.description.as_deref(), Some("preset description"));
    assert_eq!(
        record.avatar_url.as_deref(),
        Some("https://example.com/avatar.png"),
        "agent without an avatar takes the preset's"
    );

    let mut record = sample_record();
    record.avatar_url = Some("https://example.com/own.png".into());
    copy_preset_into_agent(&mut record, &preset);
    assert_eq!(
        record.avatar_url.as_deref(),
        Some("https://example.com/own.png"),
        "agent's own avatar is kept"
    );

    preset.description = None;
    copy_preset_into_agent(&mut record, &preset);
    assert_eq!(record.description, None);
}

#[test]
fn display_name_is_never_copied() {
    let mut record = sample_record();
    record.name = "Own Name".into();
    let preset = sample_persona();
    assert_ne!(preset.display_name, record.name);

    copy_preset_into_agent(&mut record, &preset);

    assert_eq!(record.name, "Own Name");
}

#[test]
fn relay_mesh_marker_is_cleared() {
    let mut record = sample_record();
    record.relay_mesh = Some(RelayMeshConfig {
        model_ref: "some-model".into(),
    });

    copy_preset_into_agent(&mut record, &sample_persona());

    assert_eq!(record.relay_mesh, None);
}

#[test]
fn updated_at_is_untouched() {
    let mut record = sample_record();
    record.updated_at = "2020-01-01T00:00:00Z".into();

    copy_preset_into_agent(&mut record, &sample_persona());

    assert_eq!(record.updated_at, "2020-01-01T00:00:00Z");
}

/// A harness pin naming a different known runtime than the preset's is
/// dropped; a pin for the same runtime is kept. (More cases in
/// `stale_pin_tests`.)
#[test]
fn stale_harness_pin_is_cleared_when_runtime_changes() {
    let mut record = sample_record();
    record.agent_command_override = Some("claude-agent-acp".into());
    copy_preset_into_agent(&mut record, &sample_persona()); // runtime goose
    assert_eq!(record.agent_command_override, None);

    let mut record = sample_record();
    record.agent_command_override = Some("goose".into());
    copy_preset_into_agent(&mut record, &sample_persona());
    assert_eq!(record.agent_command_override.as_deref(), Some("goose"));
}
