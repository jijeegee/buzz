//! Lead mode's generated `buzz-acp` Config-mode rules.
//!
//! Lead needs no harness code: the lead agent spawns with
//! `BUZZ_ACP_SUBSCRIBE=config` and a rules file the desktop writes to
//! `<app-data>/agents/routing/lead-<pubkey prefix>.toml` on every lead spawn.
//! The harness matches rules first-match-wins, so the mention rule comes first
//! and a direct @mention is handled exactly as in Mentions mode; the second
//! rule (`lead-listen`) hears every other kind-9 message from the owner. That
//! includes a body `@Name`: in a routed channel the owner's agent mentions are
//! soft (no `p` tag), so the lead decides who acts on them. The harness reads the file once at startup, so deleting a stale one
//! never disturbs a running process — the sweep below runs on every routing
//! save and at app start, and each non-lead spawn drops its own file.
//!
//! `crates/buzz-acp/tests/fixtures/lead_rules.toml` is the byte-for-byte
//! rendering for a fixed owner; the harness tests match events against it and
//! the tests here render against it, so either side drifting fails a test.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use tauri::AppHandle;

use super::{generated_routing_dir, RoutingRole};
use crate::managed_agents::{
    apply_lead_env, storage::atomic_write_json, BackendKind, ManagedAgentPairRuntime,
    ManagedAgentRecord, ManagedAgentRuntimeKey,
};

/// The harness's default mention kinds (`buzz-acp` `default_mention_kinds`):
/// message, message edit, workflow approval request, reminder. Dropping the
/// edit kind would stop "an edit newly mentions the agent" from waking it.
pub(crate) const MENTION_KINDS: [u32; 4] = [9, 40003, 46010, 40007];

/// Queue header type of a turn the listener rule admitted.
pub(crate) const LEAD_LISTEN_TAG: &str = "lead-listen";

/// Appended to the lead's system prompt so a `lead-listen` turn knows it may
/// end silently; the base prompt otherwise asks every turn to post a result.
pub(crate) const LEAD_LISTEN_ADDENDUM: &str = "## Listening as channel lead
Turns of type `lead-listen` carry a message your owner posted without addressing you directly. You hear every such message, so many are not for you.
You are this channel's coordinator first and a worker second: your owner made you lead so each request reaches the member best suited to it, not so you do everything yourself.
Your final reply text is discarded: nobody ever sees it. Anything meant for the channel, including a clarifying question to your owner, must be posted with `buzz messages send`.
Decide in this order and stop at the first match:
1. Stay silent: small talk, thanks, an FYI or status update, a message addressed to a person by name, a message whose `Parsed:` lists mentions of someone else (they were addressed directly and already have it), or a follow-up in a thread another agent is handling. Post nothing; silence is the correct result here, not a failure.
2. Delegate: the request's core skill (coding, design, research, ...) matches another agent member's name or description better than your own. Humans are never assignees. Post one short message per assignee: `@<Exact Name>` plus a self-contained restatement, sent with `--mention <pubkey>`. If the request splits into independent parts, give each part to its best member. Do not start their part yourself, and do not @mention members who are not assignees.
3. Do it yourself: only when the request matches your own description best, or no member fits. Do the work and post the result, or post one clarifying question.
An `@Name` in your owner's text is not an assignment: the name may be the object, not the assignee ('ask @A to review @B's change' is for A). Decide who should do it.
When torn between delegating and doing it yourself, delegate.
Delegate only to channel members. The `<channel-roster>` block in your context lists them with their pubkeys, descriptions, and whether each is a human or an agent. Only if that block is missing, run `buzz channels members --channel <uuid>` and look up names and descriptions with `buzz users get --pubkey <hex> ...`. Every `@` name and `--mention` pubkey you post must come from that list. Your own subagents, tools, skills, and agents you know from anywhere else are not channel members: never @mention them.";

/// Why `record` cannot lead, or `None` when it can. Mirrors the Lead arm of
/// `routing_role_for` so `set_channel_routing` refuses a Lead that would
/// silently resolve to no role.
pub(crate) fn lead_unavailable_reason(
    record: &ManagedAgentRecord,
    owner_hex: Option<&str>,
) -> Option<&'static str> {
    if record.backend != BackendKind::Local {
        return Some("Lead needs an agent on this computer.");
    }
    if !owner_hex.is_some_and(is_hex_pubkey) {
        return Some("Lead needs your identity to know whose messages to listen to.");
    }
    None
}

const FILE_PREFIX: &str = "lead-";
const FILE_SUFFIX: &str = ".toml";
const PUBKEY_PREFIX_LEN: usize = 12;

fn is_hex_pubkey(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The rules file for a lead listening to `owner_hex`. The owner is checked
/// to be a 64-char hex key because it is spliced into a filter expression.
pub(crate) fn render_lead_rules(owner_hex: &str) -> Result<String, String> {
    if !is_hex_pubkey(owner_hex) {
        return Err("Lead needs the workspace owner's public key.".to_string());
    }
    let owner = owner_hex.to_ascii_lowercase();
    let kinds = MENTION_KINDS
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        r#"# Generated by Buzz (Agents › Channel routing = Lead). Rewritten on every launch.

[[rules]]
name = "mentions"
channels = "all"
kinds = [{kinds}]
require_mention = true
prompt_tag = "@mention"

[[rules]]
name = "{LEAD_LISTEN_TAG}"
channels = "all"
kinds = [9]
require_mention = false
filter = 'author == "{owner}"'
prompt_tag = "{LEAD_LISTEN_TAG}"
"#
    ))
}

/// `lead-<first 12 hex chars>.toml`.
pub(crate) fn lead_rules_file_name(pubkey: &str) -> String {
    let prefix: String = pubkey.chars().take(PUBKEY_PREFIX_LEN).collect();
    format!("{FILE_PREFIX}{}{FILE_SUFFIX}", prefix.to_ascii_lowercase())
}

/// Write `pubkey`'s rules into `dir` (created if missing) and return the path.
pub(crate) fn write_lead_rules_in(
    dir: &Path,
    pubkey: &str,
    owner_hex: &str,
) -> Result<PathBuf, String> {
    if !is_hex_pubkey(pubkey) {
        return Err("Lead needs an agent with a public key.".to_string());
    }
    let rules = render_lead_rules(owner_hex)?;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("failed to create {}: {error}", dir.display()))?;
    let path = dir.join(lead_rules_file_name(pubkey));
    atomic_write_json(&path, rules.as_bytes())?;
    Ok(path)
}

/// The spawn's Lead stamp, called right after `apply_routing_env`. A lead
/// gets its freshly written rules and the listening env; a failure fails the
/// spawn, because falling back to Mentions mode would look like Lead while not
/// listening. Any other role drops a leftover file of its own (best effort: a
/// leftover is inert and the next sweep retries).
pub(crate) fn apply_lead_spawn(
    command: &mut Command,
    role: RoutingRole,
    pubkey: &str,
    owner_hex: Option<&str>,
    routing_dir: Result<PathBuf, String>,
) -> Result<(), String> {
    if role == RoutingRole::Lead {
        let owner = owner_hex.ok_or("Lead needs the workspace owner's public key.")?;
        let rules = write_lead_rules_in(&routing_dir?, pubkey, owner)?;
        return apply_lead_env(command, &rules, LEAD_LISTEN_ADDENDUM);
    }
    if let Ok(dir) = routing_dir {
        let path = dir.join(lead_rules_file_name(pubkey));
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "buzz-desktop: failed to remove stale lead rules {}: {error}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

/// Delete every `lead-*.toml` in `dir` that does not belong to a pubkey in
/// `keep`. A missing directory is already clean.
pub(crate) fn sweep_lead_rules_in(dir: &Path, keep: &[&str]) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("failed to read {}: {error}", dir.display())),
    };
    let kept: Vec<String> = keep
        .iter()
        .map(|pubkey| lead_rules_file_name(pubkey))
        .collect();
    let mut failures = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read {}: {error}", dir.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(FILE_PREFIX)
            || !name.ends_with(FILE_SUFFIX)
            || kept.iter().any(|kept| kept == name)
        {
            continue;
        }
        if let Err(error) = std::fs::remove_file(entry.path()) {
            if error.kind() != std::io::ErrorKind::NotFound {
                failures.push(format!("{name}: {error}"));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "failed to remove stale lead rules in {}: {}",
            dir.display(),
            failures.join("; ")
        ))
    }
}

/// Pubkeys whose tracked process was spawned as the lead. Their file stays
/// (it is what that process runs, and what the user inspects); everything
/// else in the routing directory is residue.
fn running_leads(runtimes: &HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>) -> Vec<&str> {
    runtimes
        .iter()
        .filter(|(_, runtime)| runtime.spawn_config.routing_role == RoutingRole::Lead)
        .map(|(key, _)| key.pubkey.as_str())
        .collect()
}

/// Remove lead rules no running lead uses. Call with the process map locked
/// so a lead spawning concurrently is either already tracked (kept) or writes
/// its file after the sweep.
pub(crate) fn sweep_stale_lead_rules<R: tauri::Runtime>(
    app: &AppHandle<R>,
    runtimes: &HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>,
) -> Result<(), String> {
    sweep_lead_rules_in(&generated_routing_dir(app)?, &running_leads(runtimes))
}

/// App start: nothing this desktop spawned is tracked yet, so every lead file
/// is residue (an adopted process already read its rules at startup). Logged,
/// not fatal — a leftover file is inert (`routing_env` scrubs any env that
/// points at it) and the next sweep retries.
pub(crate) fn sweep_lead_rules_at_launch<R: tauri::Runtime>(app: &AppHandle<R>) {
    use tauri::Manager;
    let state = app.state::<crate::app_state::AppState>();
    let result = match state.managed_agent_processes.lock() {
        Ok(runtimes) => sweep_stale_lead_rules(app, &runtimes),
        Err(error) => Err(error.to_string()),
    };
    if let Err(error) = result {
        eprintln!("buzz-desktop: lead rules sweep at launch: {error}");
    }
}

#[cfg(test)]
#[path = "lead_rules_tests.rs"]
mod tests;
