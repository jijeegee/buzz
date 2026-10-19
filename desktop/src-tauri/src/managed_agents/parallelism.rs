// ── Per-harness parallelism cap ───────────────────────────────────────────────
//
// Contract: stored = requested; effective = min(requested, harness cap).
//
// `ManagedAgentRecord.parallelism` stores the user's requested value verbatim,
// never clamped at persistence. The cap is applied only where the value
// becomes a running worker-pool size:
//
//   * local spawn  — `KUZZ_ACP_AGENTS` in the child environment
//   * remote deploy — `launch.policy_env["KUZZ_ACP_AGENTS"]` + legacy field
//   * restart hash  — `SpawnConfigSnapshot` stores the effective value
//   * display copy  — the UI derives effective for explanatory hints only
//
// `AgentDefinition.parallelism` is the portable requested value, unchanged
// at every boundary so it travels across devices and harness switches intact.

/// Maximum parallelism for the OpenClaw harness.
///
/// Each kuzz-acp worker spawned by the Desktop is a client of the single
/// shared OpenClaw Gateway daemon — running more than this number of workers
/// is both resource-expensive and architecturally wrong per the OpenClaw
/// design. Tyler's ruling: "try 5 and lower if needed."
pub const OPENCLAW_MAX_PARALLELISM: u32 = 5;

/// Return the maximum allowed parallelism for the given harness command, or
/// `None` when the harness has no cap.
///
/// Keyed on [`super::discovery::normalize_command_identity`] so path prefixes,
/// the `.exe` suffix on Windows, and other cosmetic differences are ignored.
pub fn harness_max_parallelism(command: &str) -> Option<u32> {
    match super::discovery::normalize_command_identity(command).as_str() {
        "openclaw" => Some(OPENCLAW_MAX_PARALLELISM),
        _ => None,
    }
}

/// Return the effective parallelism for the given harness command and
/// requested value: `min(value, harness_max_parallelism(command))`.
///
/// For harnesses without a cap this is the identity function.
pub fn effective_parallelism(command: &str, value: u32) -> u32 {
    match harness_max_parallelism(command) {
        Some(cap) => value.min(cap),
        None => value,
    }
}

/// Return the value to emit as `KUZZ_ACP_AGENTS` for a spawn command.
///
/// Pure helper extracted from `spawn_agent_child` so both the production path
/// and tests can call it without spawning a process. The result is
/// `effective_parallelism(effective_command, record_parallelism)` formatted as
/// a decimal string ready for `command.env("KUZZ_ACP_AGENTS", …)`.
///
/// `effective_command` must be the already-resolved harness command (override →
/// runtime → persona runtime → default).
pub fn acp_agents_value(effective_command: &str, record_parallelism: u32) -> String {
    effective_parallelism(effective_command, record_parallelism).to_string()
}

#[cfg(test)]
mod tests {
    use crate::managed_agents::types::ManagedAgentRecord;

    fn record_with(runtime: Option<&str>, parallelism: u32) -> ManagedAgentRecord {
        ManagedAgentRecord {
            session_policy: Default::default(),
            description: None,
            pubkey: String::new(),
            name: "r".to_string(),
            source_preset_id: None,
            legacy_persona_id: None,
            team_instructions: None,
            private_key_nsec: String::new(),
            auth_tag: None,
            relay_url: String::new(),
            avatar_url: None,
            acp_command: String::new(),
            agent_command: String::new(),
            agent_command_override: None,
            agent_args: vec![],
            mcp_command: String::new(),
            turn_timeout_seconds: 0,
            idle_timeout_seconds: None,
            max_turn_duration_seconds: None,
            parallelism,
            system_prompt: None,
            model: None,
            provider: None,
            start_on_app_launch: false,
            is_default_ai: false,
            auto_restart_on_config_change: true,
            runtime_pid: None,
            backend: Default::default(),
            backend_agent_id: None,
            provider_policy_pending: false,
            provider_binary_path: None,
            team_id: None,
            env_vars: std::collections::BTreeMap::new(),
            created_at: String::new(),
            updated_at: String::new(),
            last_started_at: None,
            last_stopped_at: None,
            last_exit_code: None,
            last_error: None,
            last_error_code: None,
            respond_to: Default::default(),
            respond_to_allowlist: vec![],
            runtime: runtime.map(str::to_string),
            relay_mesh: None,
            effort_level: None,
            bot_origin: None,
        }
    }

    // ── Policy table: harness_max_parallelism / effective_parallelism ─────────

    #[test]
    fn policy_table() {
        let cap = super::OPENCLAW_MAX_PARALLELISM;

        // harness_max_parallelism: openclaw variants → Some(cap); others → None.
        assert_eq!(super::harness_max_parallelism("openclaw"), Some(cap));
        assert_eq!(
            super::harness_max_parallelism("/usr/local/bin/openclaw"),
            Some(cap)
        );
        assert_eq!(super::harness_max_parallelism("openclaw.exe"), Some(cap));
        assert_eq!(
            super::harness_max_parallelism(r"C:\Tools\openclaw.exe"),
            Some(cap)
        );
        assert_eq!(super::harness_max_parallelism("goose"), None);
        assert_eq!(super::harness_max_parallelism("kuzz-agent"), None);
        assert_eq!(super::harness_max_parallelism(""), None);

        // effective_parallelism: openclaw clamps above cap, honors at/below; goose passes through.
        assert_eq!(super::effective_parallelism("openclaw", cap + 5), cap);
        assert_eq!(super::effective_parallelism("openclaw", cap), cap);
        assert_eq!(super::effective_parallelism("openclaw", cap - 2), cap - 2);
        assert_eq!(super::effective_parallelism("goose", 99), 99);
        assert_eq!(super::effective_parallelism("kuzz-agent", 32), 32);
    }

    // ── acp_agents_value: spawn-env seam ──────────────────────────────────────
    //
    // Drives the pure helper extracted from spawn_agent_child.
    // Deleting or changing it breaks this test AND the production spawn env.

    /// Legacy OpenClaw record (parallelism 10, above cap): KUZZ_ACP_AGENTS must be "5".
    #[test]
    fn acp_agents_value_openclaw_above_cap_is_capped() {
        assert_eq!(
            super::acp_agents_value("openclaw", 10),
            "5",
            "KUZZ_ACP_AGENTS for openclaw with parallelism 10 must be \"5\""
        );
        assert_eq!(super::acp_agents_value("goose", 10), "10");
    }

    // ── Override-direction: summary seam agreement ────────────────────────────
    //
    // Tests effective_parallelism and record_agent_command agreement for both
    // override directions. Removing either direction loses the seam test for
    // that cap/uncap path through the summary resolver.

    /// OpenClaw runtime + Goose override: summary resolves goose → uncapped (10).
    #[test]
    fn override_direction_openclaw_runtime_goose_override_is_uncapped() {
        let mut record = record_with(Some("openclaw"), 10);
        record.agent_command_override = Some("goose".to_string());
        let cmd = crate::managed_agents::record_agent_command(&record);
        assert_eq!(cmd, "goose");
        assert_eq!(super::effective_parallelism(&cmd, record.parallelism), 10);
    }

    /// Goose runtime + OpenClaw override: summary resolves openclaw → capped (5).
    #[test]
    fn override_direction_goose_runtime_openclaw_override_is_capped() {
        let mut record = record_with(Some("goose"), 10);
        record.agent_command_override = Some("openclaw".to_string());
        let cmd = crate::managed_agents::record_agent_command(&record);
        assert_eq!(cmd, "openclaw");
        assert_eq!(
            super::effective_parallelism(&cmd, record.parallelism),
            super::OPENCLAW_MAX_PARALLELISM
        );
    }

    // ── Snapshot export: requested-definition / effective-instance contract ───

    fn snapshot_record(runtime: Option<&str>, parallelism: u32) -> ManagedAgentRecord {
        use crate::managed_agents::types::{BackendKind, RespondTo};
        use std::collections::BTreeMap;
        let mut r = record_with(runtime, parallelism);
        r.name = "snap-test".to_string();
        r.backend = BackendKind::Local;
        r.respond_to = RespondTo::OwnerOnly;
        r.env_vars = BTreeMap::new();
        r
    }

    /// Snapshot export carries the agent's requested parallelism verbatim
    /// (not clamped to the harness cap).
    #[test]
    fn snapshot_export_carries_requested_parallelism() {
        use crate::managed_agents::agent_snapshot::{build_snapshot, MemoryLevel};
        let snap = build_snapshot(
            &snapshot_record(Some("openclaw"), 10),
            MemoryLevel::None,
            vec![],
            None,
        );
        assert_eq!(snap.definition.parallelism, Some(10));
    }
}
