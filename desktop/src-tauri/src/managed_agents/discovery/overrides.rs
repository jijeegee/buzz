//! The `agent_command_override` decision family: divergence comparison,
//! create-time and update-time resolution, and the record apply. Child module
//! of `discovery` (split under the desktop file-size cap) so the override
//! decisions stay next to the resolution ladder they feed.

use super::known_acp_runtime;

/// Decide whether a user-picked harness command is an explicit per-instance
/// pin or merely `base_command` (what the agent runs without a pin) restated.
/// Returns the override to persist: `Some(picked)` when it diverges, `None`
/// when it does not.
///
/// Comparison is by RUNTIME IDENTITY, not raw string: the `claude` runtime
/// resolves to `claude-agent-acp`, but a client with only the
/// `claude-code-acp` adapter installed sends that command instead. Both map to
/// the same `claude` runtime, so neither is a real divergence — string equality
/// would wrongly bake a pin. An unknown/custom command (no matching runtime)
/// only matches when it exactly equals the base command.
pub fn divergent_agent_command_override(
    base_command: &str,
    picked_command: Option<&str>,
) -> Option<String> {
    let picked = picked_command
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let same_runtime = match (known_acp_runtime(picked), known_acp_runtime(base_command)) {
        (Some(a), Some(b)) => std::ptr::eq(a, b),
        _ => picked == base_command,
    };
    if same_runtime {
        None
    } else {
        Some(picked.to_string())
    }
}

/// Apply an explicit `agent_command` edit to `record`: persist a pin only when
/// the picked command diverges from what the agent's own runtime runs
/// ([`super::record_runtime_command`]). An empty/whitespace command clears the
/// pin, so the agent runs its own runtime. The agent's runtime, effort and env
/// are untouched — an agent has no preset to fall back to.
pub fn apply_agent_command_update(
    record: &mut crate::managed_agents::types::ManagedAgentRecord,
    agent_command: &str,
) {
    let base = super::record_runtime_command(record);
    record.agent_command_override = divergent_agent_command_override(&base, Some(agent_command));
}

/// Strip every record-level thinking-effort env alias — all known native keys
/// plus the legacy `KUZZ_AGENT_THINKING_EFFORT` alias — from `env_vars`.
pub fn remove_record_effort_aliases(env_vars: &mut std::collections::BTreeMap<String, String>) {
    let suppress = crate::managed_agents::config_bridge::effort::effort_suppress_keys();
    env_vars.retain(|k, _| {
        !suppress
            .iter()
            .any(|suppressed| k.eq_ignore_ascii_case(suppressed))
    });
}

/// Decide the `agent_command_override` to persist at AGENT CREATE time, after
/// the preset (if any) was copied into the record (so `base_command` is the
/// command the copied runtime runs).
///
/// A preset-made create receives its harness command from
/// `resolvePersonaRuntime` (frontend), which produces a divergent command in two
/// distinct cases that the backend MUST tell apart:
///
/// - DELIBERATE OVERRIDE (`harness_override` true): the user explicitly picked a
///   runtime command in UI that exposes a runtime selector. This is a real pin
///   and is preserved when it differs from the command the copied runtime
///   runs, including installed aliases such as `claude-code-acp`.
/// - MISSING-RUNTIME FALLBACK (`harness_override` false): the preset's runtime
///   isn't installed locally, so `resolvePersonaRuntime` substitutes a fallback
///   default. This is NOT a pin — baking it would freeze the agent on the
///   fallback harness even after the preset's runtime is installed.
///
/// Preset-less creates (the standalone CreateAgentDialog) preserve the picked
/// command as a real pin via [`divergent_agent_command_override`].
pub fn create_time_agent_command_override(
    from_preset: bool,
    base_command: &str,
    picked_command: Option<&str>,
    harness_override: bool,
) -> Option<String> {
    if from_preset {
        if !harness_override {
            return None;
        }
        let picked = picked_command
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        return (picked != base_command).then(|| picked.to_string());
    }

    divergent_agent_command_override(base_command, picked_command)
}
