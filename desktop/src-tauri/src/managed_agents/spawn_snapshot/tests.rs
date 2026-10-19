use super::*;
use crate::managed_agents::types::RespondTo;
use std::collections::BTreeMap;

/// Canonical projection of a prospective snapshot — the exact value the drift
/// comparison reads, so these tests assert on drift itself rather than on a
/// proxy for it.
fn snapshot_with_policy(
    record: &ManagedAgentRecord,
    workspace_relay: &str,
    global: &GlobalAgentConfig,
    enforced_owner_only: bool,
) -> serde_json::Value {
    prospective_spawn_config_snapshot(
        record,
        workspace_relay,
        global,
        enforced_owner_only,
        crate::managed_agents::channel_routing::RoutingRole::None,
    )
    .canonical()
}

fn snapshot(
    record: &ManagedAgentRecord,
    workspace_relay: &str,
    global: &GlobalAgentConfig,
) -> serde_json::Value {
    snapshot_with_policy(record, workspace_relay, global, false)
}

/// `snapshot` with the fixed default-global shape the effort tests share, so
/// their call sites read as `snap(&record)` instead of wrapping.
fn snap(record: &ManagedAgentRecord) -> serde_json::Value {
    snapshot(record, "wss://ws.example", &Default::default())
}

fn record() -> ManagedAgentRecord {
    ManagedAgentRecord {
        session_policy: Default::default(),
        description: None,
        pubkey: "p".repeat(64),
        name: "agent".into(),
        source_preset_id: None,
        legacy_persona_id: None,
        team_instructions: None,
        private_key_nsec: "nsec1fake".into(),
        auth_tag: None,
        relay_url: "ws://localhost:3000".into(),
        avatar_url: None,
        acp_command: "kuzz-acp".into(),
        agent_command: "goose".into(),
        agent_command_override: None,
        agent_args: vec![],
        mcp_command: String::new(),
        turn_timeout_seconds: 320,
        idle_timeout_seconds: None,
        max_turn_duration_seconds: None,
        parallelism: 1,
        system_prompt: Some("You are a test agent.".into()),
        model: None,
        provider: None,
        env_vars: BTreeMap::new(),
        start_on_app_launch: false,
        is_default_ai: false,
        auto_restart_on_config_change: true,
        runtime_pid: None,
        backend: Default::default(),
        backend_agent_id: None,
        provider_policy_pending: false,
        provider_binary_path: None,
        team_id: None,
        created_at: "now".into(),
        updated_at: "now".into(),
        last_started_at: None,
        last_stopped_at: None,
        last_exit_code: None,
        last_error: None,
        last_error_code: None,
        respond_to: Default::default(),
        respond_to_allowlist: vec![],
        runtime: None,
        relay_mesh: None,
        effort_level: None,
        bot_origin: None,
    }
}

#[test]
fn snapshot_is_deterministic() {
    let rec = record();
    assert_eq!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&rec, "wss://ws.example", &Default::default())
    );
}

#[test]
fn record_env_var_edit_changes_snapshot() {
    let rec = record();
    let mut edited = record();
    edited
        .env_vars
        .insert("SOME_KEY".into(), "some-value".into());
    assert_ne!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default())
    );
}

#[test]
fn record_prompt_edit_does_not_change_snapshot_but_model_and_provider_do() {
    // The prompt reaches running agents through their live settings file,
    // so editing it must not badge. Model and provider still need a new
    // process.
    let rec = record();
    let base = snapshot(&rec, "wss://ws.example", &Default::default());
    let mut edited = record();
    edited.system_prompt = Some("Edited prompt.".into());
    assert_eq!(
        base,
        snapshot(&edited, "wss://ws.example", &Default::default()),
        "a prompt edit applies live"
    );
    let mut model = record();
    model.model = Some("other-model".into());
    assert_ne!(
        base,
        snapshot(&model, "wss://ws.example", &Default::default()),
        "a model edit still requires a restart"
    );
    let mut provider = record();
    provider.provider = Some("other-provider".into());
    assert_ne!(
        base,
        snapshot(&provider, "wss://ws.example", &Default::default()),
        "a provider edit still requires a restart"
    );
}

#[test]
fn team_instruction_edit_reaches_live_settings() {
    // Team instructions are not a snapshot input at all: the agent's own copy
    // reaches the running process through the live settings file.
    let mut rec = record();
    rec.team_id = Some("team-1".into());
    let live = |instructions: &str| {
        let mut rec = rec.clone();
        rec.team_instructions = Some(instructions.into());
        crate::managed_agents::live_settings::current_live_settings(
            &rec,
            &Default::default(),
            "",
            "",
            AcpSessionPolicy::Channel,
            crate::managed_agents::channel_routing::RoutingRole::None,
        )
        .team_instructions
    };
    assert_eq!(live("Ship small.").as_deref(), Some("Ship small."));
    assert_eq!(live("Ship fast.").as_deref(), Some("Ship fast."));
}

#[test]
fn record_runtime_edit_changes_snapshot() {
    // The harness command resolves from the agent's own runtime at spawn, so a
    // runtime change means a restart WOULD change what runs → badge trips.
    let mut before = record();
    before.runtime = Some("goose".into());
    let mut after = record();
    after.runtime = Some("claude".into());
    assert_ne!(
        snapshot(&before, "wss://ws.example", &Default::default()),
        snapshot(&after, "wss://ws.example", &Default::default())
    );
}

#[test]
fn record_prompt_edit_reaches_live_settings_not_the_snapshot() {
    // A prompt edit reaches the running agent through its live settings file
    // on its next message, so the badge must not trip.
    let mut before = record();
    before.system_prompt = Some("old prompt".into());
    let mut after = record();
    after.system_prompt = Some("new prompt".into());
    assert_eq!(
        snapshot(&before, "wss://ws.example", &Default::default()),
        snapshot(&after, "wss://ws.example", &Default::default())
    );
    let live = |rec: &ManagedAgentRecord| {
        crate::managed_agents::live_settings::current_live_settings(
            rec,
            &Default::default(),
            "",
            "",
            AcpSessionPolicy::Channel,
            crate::managed_agents::channel_routing::RoutingRole::None,
        )
        .system_prompt
    };
    assert_eq!(live(&before).as_deref(), Some("old prompt"));
    assert_eq!(live(&after).as_deref(), Some("new prompt"));
}

#[test]
fn workspace_relay_change_trips_snapshot_even_for_stored_record_relay() {
    // The legacy per-record relay pin is ignored (#2122): every record spawns
    // against the active workspace relay, so a workspace relay change means a
    // restart would change what runs — pinned records included.
    let rec = record();
    assert!(
        !rec.relay_url.is_empty(),
        "fixture should carry a legacy pin"
    );
    assert_ne!(
        snapshot(&rec, "wss://relay-a.example", &Default::default()),
        snapshot(&rec, "wss://relay-b.example", &Default::default())
    );
}

#[test]
fn stored_record_relay_does_not_affect_snapshot() {
    // Editing the (ignored) stored pin must not badge a restart: what a
    // restart would run is identical either way.
    let mut a = record();
    let mut b = record();
    a.relay_url = String::new();
    b.relay_url = "wss://legacy-pin.example".into();
    assert_eq!(
        snapshot(&a, "wss://ws.example", &Default::default()),
        snapshot(&b, "wss://ws.example", &Default::default())
    );
}

#[test]
fn owner_only_mode_and_allowlist_edits_do_not_change_effective_snapshot() {
    let mut before = record();
    before.respond_to = RespondTo::Allowlist;
    before.respond_to_allowlist = vec!["a".repeat(64)];

    let mut mode_edited = before.clone();
    mode_edited.respond_to = RespondTo::Anyone;

    let mut allowlist_edited = before.clone();
    allowlist_edited.respond_to_allowlist = vec!["b".repeat(64)];

    let effective_before =
        snapshot_with_policy(&before, "wss://ws.example", &Default::default(), true);
    for (label, edited) in [
        ("respond-to mode", mode_edited),
        ("respond-to allowlist", allowlist_edited),
    ] {
        assert_eq!(
            effective_before,
            snapshot_with_policy(
                &edited,
                "wss://ws.example",
                &Default::default(),
                true,
            ),
            "portable {label} edit must not create restart drift when both spawns enforce owner-only",
        );
    }
}

#[test]
fn oss_mode_and_allowlist_edits_change_effective_snapshot() {
    let mut before = record();
    before.respond_to = RespondTo::Allowlist;
    before.respond_to_allowlist = vec!["a".repeat(64)];

    let mut mode_edited = before.clone();
    mode_edited.respond_to = RespondTo::Anyone;

    let mut allowlist_edited = before.clone();
    allowlist_edited.respond_to_allowlist = vec!["b".repeat(64)];

    let effective_before =
        snapshot_with_policy(&before, "wss://ws.example", &Default::default(), false);
    for (label, edited) in [
        ("respond-to mode", mode_edited),
        ("respond-to allowlist", allowlist_edited),
    ] {
        assert_ne!(
            effective_before,
            snapshot_with_policy(&edited, "wss://ws.example", &Default::default(), false,),
            "OSS spawn must retain restart drift for effective {label} edits",
        );
    }
}

#[test]
fn respond_to_allowlist_edit_changes_snapshot() {
    let rec = record();
    let mut edited = record();
    edited.respond_to = RespondTo::Allowlist;
    edited.respond_to_allowlist = vec!["a".repeat(64)];
    assert_ne!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default())
    );
}

#[test]
fn allowlist_ignored_when_mode_is_not_allowlist() {
    // Spawn only sets KUZZ_ACP_RESPOND_TO_ALLOWLIST in allowlist mode, so
    // editing the (dormant) list under owner-only must not badge.
    let rec = record();
    let mut edited = record();
    edited.respond_to_allowlist = vec!["a".repeat(64)];
    assert_eq!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default())
    );
}

#[test]
fn allowlist_normalization_equivalent_edits_do_not_change_snapshot() {
    // The env receives the normalized list (trim/lowercase/dedup), so edits
    // that normalize to the same value must not badge.
    let mut rec = record();
    rec.respond_to = RespondTo::Allowlist;
    rec.respond_to_allowlist = vec!["a".repeat(64)];
    let mut edited = rec.clone();
    edited.respond_to_allowlist = vec![
        format!(" {} ", "A".repeat(64)), // whitespace + case
        "a".repeat(64),                  // duplicate
    ];
    assert_eq!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default())
    );
}

#[test]
fn allowlist_content_edit_still_changes_snapshot() {
    let mut rec = record();
    rec.respond_to = RespondTo::Allowlist;
    rec.respond_to_allowlist = vec!["a".repeat(64)];
    let mut edited = rec.clone();
    edited.respond_to_allowlist = vec!["b".repeat(64)];
    assert_ne!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default())
    );
}

#[test]
fn explicit_max_turn_duration_changes_snapshot_from_none() {
    let rec = record();
    let mut edited = record();
    edited.max_turn_duration_seconds = Some(7200);
    assert_ne!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default())
    );
}

#[test]
fn non_default_max_turn_duration_changes_snapshot() {
    let rec = record();
    let mut edited = record();
    edited.max_turn_duration_seconds = Some(42);
    assert_ne!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default())
    );
}

#[test]
fn non_spawn_bookkeeping_fields_do_not_change_snapshot() {
    // updated_at / runtime_pid / last_* are lifecycle bookkeeping, not spawn
    // inputs — routine record saves must not trip the badge.
    let rec = record();
    let mut edited = record();
    edited.updated_at = "later".into();
    edited.runtime_pid = Some(12345);
    edited.last_started_at = Some("later".into());
    edited.last_exit_code = Some(0);
    assert_eq!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default())
    );
}

#[test]
fn empty_prompt_snapshots_like_absent_prompt() {
    // B5 drift row 2 foundation: Some("") and None spawn identically (env var
    // absent either way), so they must snapshot equal — a backfilled prompt-less
    // record re-snapshots to Some("") and must not trip the badge.
    let mut absent = record();
    absent.system_prompt = None;
    let mut empty = record();
    empty.system_prompt = Some(String::new());
    assert_eq!(
        snapshot(&absent, "wss://ws.example", &Default::default()),
        snapshot(&empty, "wss://ws.example", &Default::default()),
    );
}

/// A command override (pin) wins over the agent's runtime, so a runtime
/// edit under a pin changes nothing that runs — the badge must NOT fire.
#[test]
fn command_override_beats_record_runtime_change() {
    let mut before = record();
    before.runtime = Some("goose".into());
    before.agent_command_override = Some("/opt/custom/my-agent".into());
    let mut after = before.clone();
    after.runtime = Some("claude".into());
    assert_eq!(
        snapshot(&before, "wss://ws.example", &Default::default()),
        snapshot(&after, "wss://ws.example", &Default::default()),
        "a command override must win regardless of a runtime change"
    );
}

// ── Global default vs the agent's own model ────────────────────────────────────────────

#[test]
fn global_model_change_trips_snapshot_for_agent_without_own_model() {
    let mut rec = record();
    rec.runtime = Some("goose".into());

    let global_a = GlobalAgentConfig {
        model: Some("model-a".to_string()),
        provider: Some("prov-a".to_string()),
        ..Default::default()
    };
    let global_b = GlobalAgentConfig {
        model: Some("model-b".to_string()),
        provider: Some("prov-b".to_string()),
        ..Default::default()
    };

    let snapshot_a = snapshot(&rec, "wss://ws.example", &global_a);
    let snapshot_b = snapshot(&rec, "wss://ws.example", &global_b);

    assert_ne!(
        snapshot_a, snapshot_b,
        "changing the global default must drift an agent that inherits it"
    );
}

#[test]
fn agent_model_wins_over_global_model_change() {
    let mut rec = record();
    rec.runtime = Some("goose".into());
    rec.model = Some("agent-model".into());
    rec.provider = Some("agent-prov".into());

    let global_a = GlobalAgentConfig {
        model: Some("model-a".to_string()),
        provider: Some("prov-a".to_string()),
        ..Default::default()
    };
    let global_b = GlobalAgentConfig {
        model: Some("model-b".to_string()),
        provider: Some("prov-b".to_string()),
        ..Default::default()
    };

    assert_eq!(
        snapshot(&rec, "wss://ws.example", &global_a),
        snapshot(&rec, "wss://ws.example", &global_b),
        "an agent's own model/provider must shadow the global default"
    );
}

#[test]
fn global_model_change_trips_snapshot_without_model_env_var() {
    let mut rec = record();
    rec.agent_command = "some-harness-without-model-env".into();

    let global_a = GlobalAgentConfig {
        model: Some("model-a".to_string()),
        ..Default::default()
    };
    let global_b = GlobalAgentConfig {
        model: Some("model-b".to_string()),
        ..Default::default()
    };

    let snapshot_a = snapshot(&rec, "wss://ws.example", &global_a);
    let snapshot_b = snapshot(&rec, "wss://ws.example", &global_b);

    assert_ne!(
        snapshot_a, snapshot_b,
        "global model change must drift even without a model_env_var runtime"
    );
}

#[test]
fn name_edit_changes_snapshot() {
    // The spawn writes KUZZ_ACP_SESSION_TITLE from the agent's name, so a
    // rename must trip the badge: the running process keeps the old title
    // until it restarts, and the operator has to be told that.
    let rec = record();
    let mut renamed = record();
    renamed.name = "agent-2".into();
    assert_ne!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&renamed, "wss://ws.example", &Default::default()),
        "a rename changes the spawned session title and must badge"
    );
}

#[test]
fn name_edit_does_not_change_snapshot_under_an_explicit_title_override() {
    // User env is written AFTER the Kuzz-set title (last-wins), so an explicit
    // KUZZ_ACP_SESSION_TITLE is what the child actually runs with. Renaming the
    // record changes nothing about the spawned process, so badging it would be
    // a false restart prompt. The override itself still reaches the snapshot
    // through the effective env.
    let mut rec = record();
    rec.env_vars
        .insert("KUZZ_ACP_SESSION_TITLE".into(), "Pinned Title".into());
    let mut renamed = rec.clone();
    renamed.name = "Fizz".into();
    assert_eq!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&renamed, "wss://ws.example", &Default::default()),
        "a rename shadowed by an explicit title override must not badge"
    );
}

#[test]
fn title_override_edit_changes_snapshot() {
    // Counterpart to the test above: the override is not inert — editing it
    // changes what the child runs with and must badge.
    let mut rec = record();
    rec.env_vars
        .insert("KUZZ_ACP_SESSION_TITLE".into(), "Pinned Title".into());
    let mut edited = record();
    edited
        .env_vars
        .insert("KUZZ_ACP_SESSION_TITLE".into(), "Other Title".into());
    assert_ne!(
        snapshot(&rec, "wss://ws.example", &Default::default()),
        snapshot(&edited, "wss://ws.example", &Default::default()),
        "editing an explicit title override must badge"
    );
}

// ── I2: definition args and env reach the snapshot ───────────────────────────
//
// These tests prove that editing a custom harness definition's args or env
// change the snapshot, which trips the "restart required" badge.
// They would fail if the snapshot used only record.agent_args without
// falling back to definition args, or if resolve_effective_agent_env did not
// include definition env.

/// When a record has no instance args but the definition has default args,
/// changing the definition args changes the snapshot. This would fail if
/// the snapshot used only record.agent_args.
#[test]
fn spawn_snapshot_changes_when_definition_default_args_change() {
    use crate::managed_agents::custom_harnesses::{
        registry_test_lock, warm_harness_registry_from_dir,
    };
    use std::fs;
    use tempfile::tempdir;

    // The loaded-harness registry is process-global: a parallel test re-warming
    // it between the two snapshots makes both resolve to no-definition
    // and s1 == s2 (observed on Windows CI).
    let _lock = registry_test_lock();
    let dir = tempdir().unwrap();

    // Write v1 definition (args: ["--mode", "v1"]).
    fs::write(
        dir.path().join("my-def.json"),
        r#"{"id":"my-def","label":"My Def","command":"my-def-bin","args":["--mode","v1"]}"#,
    )
    .unwrap();
    warm_harness_registry_from_dir(Some(dir.path()));

    let mut r = record();
    r.runtime = Some("my-def".into());
    r.agent_args = vec![]; // no instance args → definition args are used

    let s1 = snapshot(&r, "ws://relay", &Default::default());

    // Update to v2 args and re-warm (simulating save + transactional refresh).
    fs::write(
        dir.path().join("my-def.json"),
        r#"{"id":"my-def","label":"My Def","command":"my-def-bin","args":["--mode","v2"]}"#,
    )
    .unwrap();
    warm_harness_registry_from_dir(Some(dir.path()));

    let s2 = snapshot(&r, "ws://relay", &Default::default());

    assert_ne!(
        s1, s2,
        "changing definition default args must change the snapshot"
    );
}

/// When a definition has env vars, adding them changes the snapshot. This
/// proves resolve_effective_agent_env includes definition env in the layering.
#[test]
fn spawn_snapshot_changes_when_definition_env_changes() {
    use crate::managed_agents::custom_harnesses::{
        registry_test_lock, warm_harness_registry_from_dir,
    };
    use std::fs;
    use tempfile::tempdir;

    // Serialize against parallel registry re-warms (see the args test above).
    let _lock = registry_test_lock();
    let dir = tempdir().unwrap();

    // Write definition without env.
    fs::write(
        dir.path().join("env-def.json"),
        r#"{"id":"env-def","label":"Env Def","command":"env-def-bin"}"#,
    )
    .unwrap();
    warm_harness_registry_from_dir(Some(dir.path()));

    let mut r = record();
    r.runtime = Some("env-def".into());

    let s1 = snapshot(&r, "ws://relay", &Default::default());

    // Update to include env and re-warm.
    fs::write(
        dir.path().join("env-def.json"),
        r#"{"id":"env-def","label":"Env Def","command":"env-def-bin","env":{"MY_FLAG":"1"}}"#,
    )
    .unwrap();
    warm_harness_registry_from_dir(Some(dir.path()));

    let s2 = snapshot(&r, "ws://relay", &Default::default());

    assert_ne!(s1, s2, "adding definition env must change the snapshot");
}

/// Instance-level args win over definition default args (non-empty instance
/// args must NOT be overridden by the definition). The snapshot must match a record
/// that has the same effective args from either source.
#[test]
fn spawn_snapshot_instance_args_win_over_definition_args() {
    use crate::managed_agents::custom_harnesses::{
        registry_test_lock, warm_harness_registry_from_dir,
    };
    use std::fs;
    use tempfile::tempdir;

    // Serialize against parallel registry re-warms (see the args test above).
    let _lock = registry_test_lock();
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("arg-def.json"),
        r#"{"id":"arg-def","label":"Arg Def","command":"arg-def-bin","args":["--def-arg"]}"#,
    )
    .unwrap();
    warm_harness_registry_from_dir(Some(dir.path()));

    let mut r_instance = record();
    r_instance.runtime = Some("arg-def".into());
    r_instance.agent_args = vec!["--instance-arg".to_string()];

    let mut r_no_instance = record();
    r_no_instance.runtime = Some("arg-def".into());
    r_no_instance.agent_args = vec![];

    let snapshot_instance = snapshot(&r_instance, "ws://relay", &Default::default());
    let snapshot_no_instance = snapshot(&r_no_instance, "ws://relay", &Default::default());

    assert_ne!(
        snapshot_instance, snapshot_no_instance,
        "instance args and definition args must produce different snapshots"
    );
}

// ── Parallelism cap: above-cap equivalence + cap crossing ─────────────────────
//
// The snapshot stores the *effective* parallelism (min(requested, harness cap))
// so that over-cap edits that don't change the running pool size do not raise a
// spurious "restart required" badge, while cap crossings (e.g. 8 → 3, where 3
// is below the cap) still badge because the pool actually changes.

/// Two over-cap parallelism values (10 and 8) produce the same snapshot for
/// OpenClaw: both clamp to OPENCLAW_MAX_PARALLELISM (5).
#[test]
fn openclaw_above_cap_parallelism_snapshots_equal() {
    let mut at_10 = record();
    at_10.runtime = Some("openclaw".into());
    at_10.agent_command = "openclaw".into();
    at_10.parallelism = 10;

    let mut at_8 = record();
    at_8.runtime = Some("openclaw".into());
    at_8.agent_command = "openclaw".into();
    at_8.parallelism = 8;

    assert_eq!(
        snapshot(&at_10, "wss://ws.example", &Default::default()),
        snapshot(&at_8, "wss://ws.example", &Default::default()),
        "parallelism 10 and 8 both clamp to 5 for OpenClaw — snapshots must be equal, no restart badge"
    );
}

/// A cap-crossing edit (8 → 3) produces different snapshots: 8 clamps to 5,
/// but 3 is below the cap and runs as 3 — the pool changes, so the badge fires.
#[test]
fn openclaw_cap_crossing_parallelism_snapshots_differ() {
    let mut at_8 = record();
    at_8.runtime = Some("openclaw".into());
    at_8.agent_command = "openclaw".into();
    at_8.parallelism = 8;

    let mut at_3 = record();
    at_3.runtime = Some("openclaw".into());
    at_3.agent_command = "openclaw".into();
    at_3.parallelism = 3;

    assert_ne!(
        snapshot(&at_8, "wss://ws.example", &Default::default()),
        snapshot(&at_3, "wss://ws.example", &Default::default()),
        "parallelism 8 (clamps to 5) and 3 (runs as 3) must produce different snapshots"
    );
}

#[cfg(test)]
#[path = "tests_ext.rs"]
mod ext;
