//! Dispatcher-mode launch env for the default AI.
//!
//! The one managed agent starred as the default AI (`ManagedAgentRecord::
//! is_default_ai`) is launched as a channel dispatcher: `buzz-acp` reads
//! `BUZZ_ACP_DISPATCHER` (a clap `bool`, so the accepted spellings are
//! `true`/`false`) and, when set, subscribes to every non-DM channel without a
//! mention filter and swaps in its compiled-in dispatcher base prompt. The
//! desktop therefore only ever has to set that single flag.
//!
//! `BUZZ_ACP_DISPATCHER_CONFIG` (the per-channel human/AI allowlist JSON) is
//! deliberately **never** set by the desktop in this phase: an absent config
//! means the harness applies its default gate (channel owner/admin plus the
//! agent's own owner; no AI authors). Both keys are reserved env keys so a
//! saved or ambient value cannot turn an ordinary agent into a router, or hand
//! a custom gate to a harness the UI shows as running with the default one.
//!
//! Local spawns go through [`apply_dispatcher_env`]; provider deployments
//! carry the same decision in `launch.policy_env` via
//! [`insert_dispatcher_env`]. Both are driven solely by the record's star so
//! the two launch paths cannot disagree.

use std::collections::BTreeMap;
use std::process::Command;

/// Harness flag that turns on dispatcher mode (`buzz-acp --dispatcher`).
pub(crate) const DISPATCHER_ENV_VAR: &str = "BUZZ_ACP_DISPATCHER";
/// Harness per-channel gate policy (`buzz-acp --dispatcher-config`). The
/// desktop never sets it; it is reserved and scrubbed so the harness default
/// gate is what actually runs.
pub(crate) const DISPATCHER_CONFIG_ENV_VAR: &str = "BUZZ_ACP_DISPATCHER_CONFIG";
/// The spelling clap's `bool` env parser accepts, matching the
/// `BUZZ_ACP_LAZY_POOL=true` convention the spawn path already uses.
const DISPATCHER_ENABLED: &str = "true";

/// Stamp dispatcher mode onto a local spawn `Command` from the record's star.
///
/// `true` sets `BUZZ_ACP_DISPATCHER=true`; `false` removes the flag so a
/// value inherited from the desktop's own environment can never promote an
/// unstarred agent. The config key is always removed for the same reason.
pub(crate) fn apply_dispatcher_env(command: &mut Command, dispatcher: bool) {
    if dispatcher {
        command.env(DISPATCHER_ENV_VAR, DISPATCHER_ENABLED);
    } else {
        command.env_remove(DISPATCHER_ENV_VAR);
    }
    command.env_remove(DISPATCHER_CONFIG_ENV_VAR);
}

/// The `launch.policy_env` twin of [`apply_dispatcher_env`] for provider
/// deployments: insert the flag when starred, otherwise make sure neither key
/// survives in the desktop-owned tier.
pub(crate) fn insert_dispatcher_env(policy_env: &mut BTreeMap<String, String>, dispatcher: bool) {
    if dispatcher {
        policy_env.insert(
            DISPATCHER_ENV_VAR.to_string(),
            DISPATCHER_ENABLED.to_string(),
        );
    } else {
        policy_env.remove(DISPATCHER_ENV_VAR);
    }
    policy_env.remove(DISPATCHER_CONFIG_ENV_VAR);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Some(Some(value))` set, `Some(None)` explicitly removed, `None` untouched.
    fn env_state(command: &Command, key: &str) -> Option<Option<String>> {
        command
            .get_envs()
            .find(|(k, _)| *k == key)
            .map(|(_, value)| value.and_then(|v| v.to_str()).map(str::to_owned))
    }

    fn ambient_command() -> Command {
        let mut command = Command::new("true");
        // Simulate a desktop process whose own environment carries both keys.
        command.env(DISPATCHER_ENV_VAR, "true");
        command.env(DISPATCHER_CONFIG_ENV_VAR, "{\"deadbeef\":{}}");
        command
    }

    #[test]
    fn default_ai_spawns_with_the_dispatcher_flag_and_no_custom_gate() {
        let mut command = ambient_command();

        apply_dispatcher_env(&mut command, true);

        assert_eq!(
            env_state(&command, DISPATCHER_ENV_VAR),
            Some(Some("true".to_string()))
        );
        assert_eq!(
            env_state(&command, DISPATCHER_CONFIG_ENV_VAR),
            Some(None),
            "phase 1 never hands the harness a per-channel gate"
        );
    }

    #[test]
    fn ordinary_agent_spawns_with_both_dispatcher_keys_removed() {
        let mut command = ambient_command();

        apply_dispatcher_env(&mut command, false);

        assert_eq!(env_state(&command, DISPATCHER_ENV_VAR), Some(None));
        assert_eq!(env_state(&command, DISPATCHER_CONFIG_ENV_VAR), Some(None));
    }

    #[test]
    fn policy_env_carries_the_flag_only_for_the_default_ai() {
        let mut starred = BTreeMap::from([(
            DISPATCHER_CONFIG_ENV_VAR.to_string(),
            "{\"deadbeef\":{}}".to_string(),
        )]);
        insert_dispatcher_env(&mut starred, true);
        assert_eq!(
            starred.get(DISPATCHER_ENV_VAR).map(String::as_str),
            Some("true")
        );
        assert!(!starred.contains_key(DISPATCHER_CONFIG_ENV_VAR));

        let mut unstarred = BTreeMap::from([
            (DISPATCHER_ENV_VAR.to_string(), "true".to_string()),
            (DISPATCHER_CONFIG_ENV_VAR.to_string(), "{}".to_string()),
            ("KEEP_ME".to_string(), "yes".to_string()),
        ]);
        insert_dispatcher_env(&mut unstarred, false);
        assert_eq!(
            unstarred,
            BTreeMap::from([("KEEP_ME".to_string(), "yes".to_string())])
        );
    }
}
