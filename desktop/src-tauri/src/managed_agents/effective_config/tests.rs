use super::*;
use std::collections::BTreeMap;

fn record(
    source_preset_id: Option<&str>,
    model: Option<&str>,
    provider: Option<&str>,
    prompt: Option<&str>,
) -> ManagedAgentRecord {
    use crate::managed_agents::{BackendKind, RespondTo};
    ManagedAgentRecord {
        session_policy: Default::default(),
        description: None,
        pubkey: "agent-pk".to_string(),
        name: "Agent".to_string(),
        source_preset_id: source_preset_id.map(str::to_string),
        legacy_persona_id: None,
        team_instructions: None,
        private_key_nsec: "".to_string(),
        auth_tag: None,
        relay_url: "ws://localhost:3000".to_string(),
        avatar_url: None,
        acp_command: "kuzz-acp".to_string(),
        agent_command: "goose".to_string(),
        agent_command_override: None,
        agent_args: vec![],
        mcp_command: "".to_string(),
        turn_timeout_seconds: 300,
        idle_timeout_seconds: None,
        max_turn_duration_seconds: None,
        parallelism: 1,
        system_prompt: prompt.map(str::to_string),
        model: model.map(str::to_string),
        provider: provider.map(str::to_string),
        env_vars: BTreeMap::new(),
        start_on_app_launch: false,
        is_default_ai: false,
        runtime_pid: None,
        backend: BackendKind::Local,
        backend_agent_id: None,
        provider_policy_pending: false,
        provider_binary_path: None,
        team_id: None,
        created_at: "".to_string(),
        updated_at: "".to_string(),
        last_started_at: None,
        last_stopped_at: None,
        last_exit_code: None,
        last_error: None,
        last_error_code: None,
        respond_to: RespondTo::OwnerOnly,
        respond_to_allowlist: vec![],
        runtime: None,
        relay_mesh: None,
        effort_level: None,
        auto_restart_on_config_change: false,
        bot_origin: None,
    }
}

fn global(model: Option<&str>, provider: Option<&str>) -> GlobalAgentConfig {
    GlobalAgentConfig {
        model: model.map(str::to_string),
        provider: provider.map(str::to_string),
        ..Default::default()
    }
}

// ── Agent's own fields → global ──

#[test]
fn agent_uses_own_fields() {
    let rec = record(None, Some("my-model"), Some("my-prov"), Some("my prompt"));
    let g = global(Some("global-model"), Some("global-prov"));

    let cfg = resolve_effective_config(&rec, &g);

    assert_eq!(cfg.model.value.as_deref(), Some("my-model"));
    assert_eq!(cfg.model.source, ConfigSource::InstanceLegacy);
    assert_eq!(cfg.provider.value.as_deref(), Some("my-prov"));
    assert_eq!(cfg.provider.source, ConfigSource::InstanceLegacy);
    assert_eq!(cfg.system_prompt.value.as_deref(), Some("my prompt"));
    assert_eq!(cfg.system_prompt.source, ConfigSource::InstanceLegacy);
}

#[test]
fn preset_made_agent_uses_own_fields() {
    // `source_preset_id` is only a label: the agent's own copied values win.
    let rec = record(
        Some("d1"),
        Some("my-model"),
        Some("my-prov"),
        Some("my prompt"),
    );
    let g = global(Some("global-model"), Some("global-prov"));

    let cfg = resolve_effective_config(&rec, &g);

    assert_eq!(cfg.model.value.as_deref(), Some("my-model"));
    assert_eq!(cfg.provider.value.as_deref(), Some("my-prov"));
    assert_eq!(cfg.system_prompt.value.as_deref(), Some("my prompt"));
}

#[test]
fn agent_falls_back_to_global() {
    let rec = record(None, None, None, None);
    let g = global(Some("global-model"), Some("global-prov"));

    let cfg = resolve_effective_config(&rec, &g);

    assert_eq!(cfg.model.value.as_deref(), Some("global-model"));
    assert_eq!(cfg.model.source, ConfigSource::Global);
    assert_eq!(cfg.provider.value.as_deref(), Some("global-prov"));
    assert_eq!(cfg.provider.source, ConfigSource::Global);
}

#[test]
fn agent_model_set_provider_inherits_global() {
    let rec = record(Some("d1"), Some("own-model"), None, Some("prompt"));
    let g = global(None, Some("global-prov"));

    let cfg = resolve_effective_config(&rec, &g);

    assert_eq!(cfg.model.value.as_deref(), Some("own-model"));
    assert_eq!(cfg.model.source, ConfigSource::InstanceLegacy);
    assert_eq!(cfg.provider.value.as_deref(), Some("global-prov"));
    assert_eq!(cfg.provider.source, ConfigSource::Global);
}

#[test]
fn blank_agent_fields_fall_through() {
    let rec = record(None, Some("  "), Some(""), Some("  "));
    let g = global(Some("g-model"), None);

    let cfg = resolve_effective_config(&rec, &g);

    assert_eq!(cfg.model.value.as_deref(), Some("g-model"));
    assert_eq!(cfg.model.source, ConfigSource::Global);
    assert_eq!(cfg.provider.value, None);
    assert_eq!(cfg.provider.source, ConfigSource::Global);
    assert_eq!(cfg.system_prompt.value, None);
}

// ── Convenience helper ──

#[test]
fn model_provider_pair_returns_resolved_values() {
    let rec = record(Some("d1"), Some("m"), Some("p"), None);
    let g = global(None, None);

    let pair = resolve_effective_model_provider_pair(&rec, &g);
    assert_eq!(pair, (Some("m".to_string()), Some("p".to_string())));
}

// ── Morgan's exact regression sequence ──

#[test]
fn morgans_sequence_inherit_explicit_inherit() {
    let g = global(Some("claude-opus-4-6"), Some("anthropic"));

    // Step 1: fresh agent with inherited model → resolves global
    let rec_step1 = record(Some("d1"), None, None, Some("agent prompt"));
    let cfg1 = resolve_effective_config(&rec_step1, &g);
    assert_eq!(cfg1.model.value.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(cfg1.model.source, ConfigSource::Global);

    // Step 2: set an explicit model on the agent
    let rec_explicit = record(
        Some("d1"),
        Some("goose-gpt-5-6-sol"),
        Some("databricks"),
        Some("agent prompt"),
    );
    let cfg2 = resolve_effective_config(&rec_explicit, &g);
    assert_eq!(cfg2.model.value.as_deref(), Some("goose-gpt-5-6-sol"));
    assert_eq!(cfg2.model.source, ConfigSource::InstanceLegacy);

    // Step 3: switch back to inherit (fields cleared) → global again
    let rec_inherit = record(Some("d1"), None, None, Some("agent prompt"));
    let cfg3 = resolve_effective_config(&rec_inherit, &g);
    assert_eq!(cfg3.model.value.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(cfg3.model.source, ConfigSource::Global);
    assert_eq!(cfg3.provider.value.as_deref(), Some("anthropic"));
    assert_eq!(cfg3.provider.source, ConfigSource::Global);
}

// ── relay-mesh preflight resolution (Wes review 2 on #1968) ──
//
// The preflight must key off `resolve_effective_config`'s resolution — the
// same one spawn's mesh env consults.

#[test]
fn relay_mesh_model_id_none_for_non_mesh_config() {
    let rec = record(Some("d1"), Some("m"), Some("anthropic"), None);
    let g = global(None, None);

    assert_eq!(resolve_effective_relay_mesh_model_id(&rec, &g), None);
}

#[test]
fn relay_mesh_model_id_defaults_to_auto_when_model_blank() {
    let rec = record(Some("d1"), None, Some(RELAY_MESH_PROVIDER_ID), None);
    let g = global(None, None);

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &g).as_deref(),
        Some(RELAY_MESH_AUTO_MODEL_ID)
    );
}

/// Global-inheritance regression (Wes finding 2): a preset-made agent with a
/// blank provider falling through to a relay-mesh GLOBAL default must trigger
/// the mesh preflight.
#[test]
fn global_relay_mesh_default_triggers_preflight_for_preset_made_agent() {
    let rec = record(Some("d1"), None, None, None);
    let g = global(Some("Qwen3"), Some(RELAY_MESH_PROVIDER_ID));

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &g).as_deref(),
        Some("Qwen3"),
        "global relay-mesh default must trigger the mesh preflight for a blank agent"
    );
}

/// Symmetric case: an agent never made from a preset inheriting the global
/// relay-mesh default must also trigger the preflight.
#[test]
fn global_relay_mesh_default_triggers_preflight_for_presetless_agent() {
    let rec = record(None, None, None, None);
    let g = global(None, Some(RELAY_MESH_PROVIDER_ID));

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &g).as_deref(),
        Some(RELAY_MESH_AUTO_MODEL_ID)
    );
}

// ── Whitespace-provider parity (trim-mismatch regression, T1) ──
//
// The old spawn gate was `effective_provider.as_deref() == Some(RELAY_MESH_PROVIDER_ID)`
// — an exact compare — while `relay_mesh_model_id()` trims before matching.
// After the fix spawn derives its gate from `relay_mesh_model_id()` too, so the
// trim semantics are identical.

/// Agent provider with leading/trailing whitespace — the resolver preserves the
/// raw string, but `relay_mesh_model_id()` trims before matching.
#[test]
fn whitespace_provider_on_agent_triggers_mesh_decision() {
    let rec = record(Some("d1"), Some("qwen3"), Some(" relay-mesh "), None);
    let g = global(None, None);

    let mesh_id = resolve_effective_relay_mesh_model_id(&rec, &g);
    assert_eq!(
        mesh_id.as_deref(),
        Some("qwen3"),
        "padded agent provider must be treated as relay-mesh by the shared helper"
    );
}

/// Global provider with leading/trailing whitespace — same assertion for the
/// global-inherit path.
#[test]
fn whitespace_provider_in_global_triggers_mesh_decision() {
    let rec = record(Some("d1"), None, None, None);
    let g = global(Some("qwen3"), Some("\trelay-mesh\t"));

    let mesh_id = resolve_effective_relay_mesh_model_id(&rec, &g);
    assert_eq!(
        mesh_id.as_deref(),
        Some("qwen3"),
        "padded global provider must be treated as relay-mesh by the shared helper"
    );
}

// ── Legacy relay-mesh record compatibility (Wes review 3 on #1968) ──
//
// Two record generations shipped before `provider: "relay-mesh"` existed and
// are never rewritten on load, so they are still on disk:
//
//   Class A (#798) — the four mesh preset env vars in `env_vars`, no typed
//     field, no `provider` (the record had no `provider` field yet).
//   Class B (#879) — the typed `relay_mesh` marker, still no `provider` field.
//
// Without the fallback both classes resolve to a non-mesh provider while their
// own stale env bytes still reach the child: a silent misroute, not a clean
// failure. The fallback applies only to a record never made from a preset —
// a preset-made record's blank provider means "global default".

/// Class A: env-only legacy record resolves mesh via the four-env preset.
#[test]
fn legacy_record_falls_back_to_env_sniff() {
    let mut rec = record(None, None, None, None);
    rec.env_vars = BTreeMap::from([
        ("KUZZ_AGENT_PROVIDER".to_string(), "openai".to_string()),
        (
            "OPENAI_COMPAT_BASE_URL".to_string(),
            "http://127.0.0.1:9337/v1/".to_string(),
        ),
        ("OPENAI_COMPAT_MODEL".to_string(), "Qwen3".to_string()),
        (
            "OPENAI_COMPAT_API_KEY".to_string(),
            RELAY_MESH_API_KEY_PLACEHOLDER.to_string(),
        ),
    ]);

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &global(None, None)).as_deref(),
        Some("Qwen3"),
        "env-only legacy mesh record must still resolve to its served model"
    );
}

/// Class A, oldest shipped record: both renamed sentinels in their original
/// spelling. The preset first wrote `SPROUT_AGENT_PROVIDER` (#798) and the
/// api-key value `sprout-mesh-local`; #971 and #960 renamed each in the same
/// Jun-11 window without migrating persisted `env_vars`, so this exact shape is
/// still on disk and must resolve mesh.
#[test]
fn legacy_record_falls_back_to_env_sniff_with_pre_rename_sentinels() {
    let mut rec = record(None, None, None, None);
    rec.env_vars = BTreeMap::from([
        ("SPROUT_AGENT_PROVIDER".to_string(), "openai".to_string()),
        (
            "OPENAI_COMPAT_BASE_URL".to_string(),
            "http://127.0.0.1:9337/v1".to_string(),
        ),
        ("OPENAI_COMPAT_MODEL".to_string(), "Qwen3".to_string()),
        (
            "OPENAI_COMPAT_API_KEY".to_string(),
            "sprout-mesh-local".to_string(),
        ),
    ]);

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &global(None, None)).as_deref(),
        Some("Qwen3"),
        "the oldest shipped mesh record carries both pre-rename sentinels and was never migrated"
    );
}

/// Class A straddling the rename window: old provider key with the new api-key
/// value. Proves the two either/ors are independent rather than one either/or on
/// a bundled old/new pair.
#[test]
fn legacy_record_falls_back_to_env_sniff_with_mixed_rename_sentinels() {
    let mut rec = record(None, None, None, None);
    rec.env_vars = BTreeMap::from([
        ("SPROUT_AGENT_PROVIDER".to_string(), "openai".to_string()),
        (
            "OPENAI_COMPAT_BASE_URL".to_string(),
            "http://127.0.0.1:9337/v1".to_string(),
        ),
        ("OPENAI_COMPAT_MODEL".to_string(), "Qwen3".to_string()),
        (
            "OPENAI_COMPAT_API_KEY".to_string(),
            RELAY_MESH_API_KEY_PLACEHOLDER.to_string(),
        ),
    ]);

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &global(None, None)).as_deref(),
        Some("Qwen3"),
        "old provider key with new api key must resolve — the sentinels renamed independently"
    );
}

/// The mirrored straddle: new provider key with the old api-key value.
#[test]
fn legacy_record_falls_back_to_env_sniff_with_new_provider_and_old_api_key() {
    let mut rec = record(None, None, None, None);
    rec.env_vars = BTreeMap::from([
        ("KUZZ_AGENT_PROVIDER".to_string(), "openai".to_string()),
        (
            "OPENAI_COMPAT_BASE_URL".to_string(),
            "http://127.0.0.1:9337/v1".to_string(),
        ),
        ("OPENAI_COMPAT_MODEL".to_string(), "Qwen3".to_string()),
        (
            "OPENAI_COMPAT_API_KEY".to_string(),
            "sprout-mesh-local".to_string(),
        ),
    ]);

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &global(None, None)).as_deref(),
        Some("Qwen3"),
        "new provider key with old api key must resolve too"
    );
}

/// Both spellings present and disagreeing: the renamed key is authoritative,
/// so a record whose current provider env is non-mesh is not resurrected as
/// mesh by the stale `SPROUT_` leftover.
#[test]
fn legacy_env_sniff_prefers_renamed_provider_key_over_stale_old_key() {
    let mut rec = record(None, None, None, None);
    rec.env_vars = BTreeMap::from([
        ("KUZZ_AGENT_PROVIDER".to_string(), "anthropic".to_string()),
        ("SPROUT_AGENT_PROVIDER".to_string(), "openai".to_string()),
        (
            "OPENAI_COMPAT_BASE_URL".to_string(),
            "http://127.0.0.1:9337/v1".to_string(),
        ),
        ("OPENAI_COMPAT_MODEL".to_string(), "Qwen3".to_string()),
        (
            "OPENAI_COMPAT_API_KEY".to_string(),
            RELAY_MESH_API_KEY_PLACEHOLDER.to_string(),
        ),
    ]);

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &global(None, None)),
        None,
        "the renamed key states current intent — a stale SPROUT_ key must not override it"
    );
}

/// Class A negative: a user's own OpenAI-compatible provider on the same local
/// port is not Kuzz's preset — the placeholder API key is the discriminator.
#[test]
fn legacy_env_sniff_ignores_user_openai_on_same_local_port() {
    let mut rec = record(None, None, None, None);
    rec.env_vars = BTreeMap::from([
        ("KUZZ_AGENT_PROVIDER".to_string(), "openai".to_string()),
        (
            "OPENAI_COMPAT_BASE_URL".to_string(),
            "http://127.0.0.1:9337/v1".to_string(),
        ),
        ("OPENAI_COMPAT_MODEL".to_string(), "Qwen3".to_string()),
        ("OPENAI_COMPAT_API_KEY".to_string(), "real-key".to_string()),
    ]);

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &global(None, None)),
        None,
        "a real user key on the mesh port must not be mistaken for Kuzz's preset"
    );
}

/// Class B: typed marker with no `provider` resolves mesh, and its `model_ref`
/// wins over an unrelated global model so preflight and spawn agree.
#[test]
fn legacy_record_falls_back_to_typed_marker_without_provider() {
    let mut rec = record(None, None, None, None);
    rec.relay_mesh = Some(crate::managed_agents::RelayMeshConfig {
        model_ref: "Qwen3".to_string(),
    });

    let cfg = resolve_effective_config(&rec, &global(Some("gpt-5"), Some("openai")));

    assert_eq!(cfg.provider.value.as_deref(), Some(RELAY_MESH_PROVIDER_ID));
    assert_eq!(
        cfg.relay_mesh_model_id().as_deref(),
        Some("Qwen3"),
        "typed marker must resolve mesh even with no provider on the record"
    );
}

/// Class B with a blank `model_ref`: the marker is still the mesh signal, and
/// the model resolves to auto — same rule `apply_relay_mesh_env` applies.
#[test]
fn legacy_typed_marker_with_blank_model_resolves_auto() {
    let mut rec = record(None, None, None, None);
    rec.relay_mesh = Some(crate::managed_agents::RelayMeshConfig {
        model_ref: "  ".to_string(),
    });

    assert_eq!(
        resolve_effective_relay_mesh_model_id(&rec, &global(None, None)).as_deref(),
        Some(RELAY_MESH_AUTO_MODEL_ID)
    );
}

/// A legacy record that was switched AWAY from mesh keeps its stale
/// marker and env bytes on disk. Its explicit `provider` states current intent,
/// so the legacy fallback must not resurrect mesh.
#[test]
fn explicit_provider_wins_over_stale_legacy_mesh_bytes() {
    let mut rec = record(None, Some("claude-opus-4-6"), Some("anthropic"), None);
    rec.relay_mesh = Some(crate::managed_agents::RelayMeshConfig {
        model_ref: "Qwen3".to_string(),
    });
    rec.env_vars = BTreeMap::from([
        ("KUZZ_AGENT_PROVIDER".to_string(), "openai".to_string()),
        (
            "OPENAI_COMPAT_BASE_URL".to_string(),
            "http://127.0.0.1:9337/v1".to_string(),
        ),
        ("OPENAI_COMPAT_MODEL".to_string(), "Qwen3".to_string()),
        (
            "OPENAI_COMPAT_API_KEY".to_string(),
            RELAY_MESH_API_KEY_PLACEHOLDER.to_string(),
        ),
    ]);

    let cfg = resolve_effective_config(&rec, &global(None, None));

    assert_eq!(cfg.provider.value.as_deref(), Some("anthropic"));
    assert_eq!(cfg.model.value.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(
        cfg.relay_mesh_model_id(),
        None,
        "a switched-away legacy record must not be dragged back onto mesh"
    );
}

/// A preset-made record never consults its legacy mesh bytes: a blank
/// provider means the global default, so neither the typed marker nor the env
/// preset may trip mesh.
#[test]
fn preset_made_record_with_legacy_bytes_inherits_global_not_mesh() {
    let mut rec = record(Some("d1"), None, None, None);
    rec.relay_mesh = Some(crate::managed_agents::RelayMeshConfig {
        model_ref: "Qwen3".to_string(),
    });
    rec.env_vars = BTreeMap::from([
        ("KUZZ_AGENT_PROVIDER".to_string(), "openai".to_string()),
        (
            "OPENAI_COMPAT_BASE_URL".to_string(),
            "http://127.0.0.1:9337/v1".to_string(),
        ),
        ("OPENAI_COMPAT_MODEL".to_string(), "Qwen3".to_string()),
        (
            "OPENAI_COMPAT_API_KEY".to_string(),
            RELAY_MESH_API_KEY_PLACEHOLDER.to_string(),
        ),
    ]);

    let cfg = resolve_effective_config(&rec, &global(Some("gpt-5"), Some("openai")));

    assert_eq!(cfg.provider.value.as_deref(), Some("openai"));
    assert_eq!(cfg.model.value.as_deref(), Some("gpt-5"));
    assert_eq!(cfg.relay_mesh_model_id(), None);
}
