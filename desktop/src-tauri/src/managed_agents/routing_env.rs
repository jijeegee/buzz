//! Channel-routing launch env, driven only by the agent's [`RoutingRole`].
//!
//! The role comes from `channel_routing::routing_role_for` (saved mode ×
//! the record's star), and this module turns it into harness env so the role
//! a process runs with and the role the UI reports cannot disagree:
//!
//! - [`RoutingRole::Dispatcher`] (Host): `BUZZ_ACP_DISPATCHER=true`. `buzz-acp`
//!   reads it as a clap `bool` and, when set, subscribes to every non-DM
//!   channel without a mention filter and swaps in its compiled-in dispatcher
//!   base prompt.
//! - [`RoutingRole::Lead`]: `BUZZ_ACP_SUBSCRIBE=config` with `BUZZ_ACP_CONFIG`
//!   at the generated rules (`channel_routing::lead_rules`) and the listening
//!   addendum appended to the system prompt ([`apply_lead_env`]).
//! - Any role but Dispatcher: the dispatcher flag is removed, so a value
//!   inherited from the desktop's own environment can never promote an agent.
//!
//! `BUZZ_ACP_DISPATCHER_CONFIG` (the per-channel human/AI allowlist JSON) is
//! **never** set: an absent config means the harness applies its default gate
//! (channel owner/admin plus the agent's own owner; no AI authors). Both
//! dispatcher keys are reserved env keys so a saved or ambient value cannot
//! turn an ordinary agent into a router, or hand it a custom gate.
//!
//! No residue across modes: `BUZZ_ACP_SUBSCRIBE`/`BUZZ_ACP_CONFIG` are not
//! reserved (users may run their own Config-mode rules), but a `BUZZ_ACP_CONFIG`
//! pointing into the desktop's generated routing directory belongs to the
//! Lead role, so for every other role both keys are scrubbed — a Lead-era
//! value copied into an agent's env cannot outlive the mode. A user's own
//! rules file anywhere else is preserved.
//!
//! Local spawns go through [`apply_routing_env`]; provider deployments carry
//! the same decision in `launch.policy_env` via [`insert_routing_env`].

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use super::channel_routing::RoutingRole;

/// Harness flag that turns on dispatcher mode (`buzz-acp --dispatcher`).
pub(crate) const DISPATCHER_ENV_VAR: &str = "BUZZ_ACP_DISPATCHER";
/// Harness per-channel gate policy (`buzz-acp --dispatcher-config`). The
/// desktop never sets it; it is reserved and scrubbed so the harness default
/// gate is what actually runs.
pub(crate) const DISPATCHER_CONFIG_ENV_VAR: &str = "BUZZ_ACP_DISPATCHER_CONFIG";
/// Harness subscription mode (`buzz-acp --subscribe`); `config` reads rules.
pub(crate) const SUBSCRIBE_ENV_VAR: &str = "BUZZ_ACP_SUBSCRIBE";
/// Harness Config-mode rules file (`buzz-acp --config`).
pub(crate) const CONFIG_ENV_VAR: &str = "BUZZ_ACP_CONFIG";
/// `buzz-acp --subscribe` value that reads rules from `BUZZ_ACP_CONFIG`.
const LEAD_SUBSCRIBE_MODE: &str = "config";
/// Harness system prompt, inline (`buzz-acp --system-prompt`).
const SYSTEM_PROMPT_ENV_VAR: &str = "BUZZ_ACP_SYSTEM_PROMPT";
/// Harness system prompt from a file; conflicts with the inline key.
const SYSTEM_PROMPT_FILE_ENV_VAR: &str = "BUZZ_ACP_SYSTEM_PROMPT_FILE";
/// The spelling clap's `bool` env parser accepts, matching the
/// `BUZZ_ACP_LAZY_POOL=true` convention the spawn path already uses.
const DISPATCHER_ENABLED: &str = "true";

/// Whether a `BUZZ_ACP_CONFIG` value names a file the desktop generated.
fn is_generated_rules_path(value: &OsStr, generated_dir: &Path) -> bool {
    Path::new(value).starts_with(generated_dir)
}

/// The value of `key` the child will see: the value set on `command` (user
/// env layer) or, when the command leaves it alone, the inherited one.
fn effective_env_value(command: &Command, key: &str) -> Option<std::ffi::OsString> {
    match command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new(key))
    {
        Some((_, value)) => value.map(OsStr::to_os_string),
        None => std::env::var_os(key),
    }
}

/// Stamp the routing role onto a local spawn `Command`. Call after the
/// `descriptor.env` loop so user env can neither enable nor keep a role.
/// `generated_dir` is `<app-data>/agents/routing/`; `None` (unresolvable app
/// data dir) skips only the generated-rules scrub.
pub(crate) fn apply_routing_env(
    command: &mut Command,
    role: RoutingRole,
    generated_dir: Option<&Path>,
) {
    if role == RoutingRole::Dispatcher {
        command.env(DISPATCHER_ENV_VAR, DISPATCHER_ENABLED);
    } else {
        command.env_remove(DISPATCHER_ENV_VAR);
    }
    command.env_remove(DISPATCHER_CONFIG_ENV_VAR);

    if role != RoutingRole::Lead {
        let generated = generated_dir.is_some_and(|dir| {
            effective_env_value(command, CONFIG_ENV_VAR)
                .is_some_and(|value| is_generated_rules_path(&value, dir))
        });
        if generated {
            command.env_remove(SUBSCRIBE_ENV_VAR);
            command.env_remove(CONFIG_ENV_VAR);
        }
    }
}

/// Stamp the Lead role's subscription and listening instructions. Call after
/// [`apply_routing_env`], and only for [`RoutingRole::Lead`], so these values
/// win over user env: `BUZZ_ACP_SUBSCRIBE=config` with `BUZZ_ACP_CONFIG` at
/// the generated `rules_path`, and `addendum` appended to the system prompt
/// the child would otherwise get. A prompt given as a file is inlined (the
/// harness rejects both prompt keys together) and the file key removed.
pub(crate) fn apply_lead_env(
    command: &mut Command,
    rules_path: &Path,
    addendum: &str,
) -> Result<(), String> {
    command.env(SUBSCRIBE_ENV_VAR, LEAD_SUBSCRIBE_MODE);
    command.env(CONFIG_ENV_VAR, rules_path);

    let base = match effective_env_value(command, SYSTEM_PROMPT_ENV_VAR) {
        Some(prompt) => prompt.to_string_lossy().into_owned(),
        None => match effective_env_value(command, SYSTEM_PROMPT_FILE_ENV_VAR) {
            Some(path) => std::fs::read_to_string(&path).map_err(|error| {
                format!(
                    "failed to read the system prompt file {}: {error}",
                    Path::new(&path).display()
                )
            })?,
            None => String::new(),
        },
    };
    let prompt = if base.trim().is_empty() {
        addendum.to_string()
    } else {
        format!("{}\n\n{addendum}", base.trim_end())
    };
    command.env_remove(SYSTEM_PROMPT_FILE_ENV_VAR);
    command.env(SYSTEM_PROMPT_ENV_VAR, prompt);
    Ok(())
}

/// The `launch.policy_env` twin of [`apply_routing_env`] for provider
/// deployments: insert the flag for a dispatcher, otherwise make sure neither
/// dispatcher key survives in the desktop-owned tier. A provider deployment
/// never leads (Lead needs a local rules file), so nothing else applies.
pub(crate) fn insert_routing_env(policy_env: &mut BTreeMap<String, String>, role: RoutingRole) {
    if role == RoutingRole::Dispatcher {
        policy_env.insert(
            DISPATCHER_ENV_VAR.to_string(),
            DISPATCHER_ENABLED.to_string(),
        );
    } else {
        policy_env.remove(DISPATCHER_ENV_VAR);
    }
    policy_env.remove(DISPATCHER_CONFIG_ENV_VAR);
}

/// The role a built provider payload actually carries, read back from its
/// `launch.policy_env` — what the deployed-role stamp records after the
/// provider accepts it.
pub(crate) fn deployed_routing_role(agent_json: &serde_json::Value) -> RoutingRole {
    if agent_json["launch"]["policy_env"][DISPATCHER_ENV_VAR] == DISPATCHER_ENABLED {
        RoutingRole::Dispatcher
    } else {
        RoutingRole::None
    }
}

#[cfg(test)]
#[path = "routing_env_tests.rs"]
mod tests;
