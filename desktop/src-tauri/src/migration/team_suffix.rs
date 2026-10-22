//! Strip the legacy baked team-instructions suffix from stored prompts.
//!
//! Records written before the runtime team framing landed have their team
//! instructions BAKED into `system_prompt` by the now-removed
//! `compose_prompt()` in kuzz-persona:
//!
//! ```text
//! {persona_prompt}\n\n---\n# Team Instructions\n{instructions}
//! ```
//!
//! `with_team()` in `kuzz-acp/src/pool.rs` now appends the LIVE
//! `[Team Instructions]` section on top of that stored value, so an affected
//! agent receives two team-instruction blocks per turn — the frozen copy first,
//! the live one second — and the observer feed renders two Team Instructions
//! cards. The frozen copy is not merely redundant: it carries whatever the team
//! roster said the day it was written, so the agent is fed a stale roster ahead
//! of the current one.
//!
//! The fix has to happen at rest. Suppressing the duplicate in the observer
//! parser would hide the symptom while the agent kept receiving the stale bytes.

use std::path::Path;

/// The exact producer boundary emitted by the removed `compose_prompt()`.
/// Matched byte-for-byte: a bare `---`, a `# Team Instructions` heading at a
/// different position, or a single preceding newline are author content, not a
/// producer boundary, and are left alone.
const TEAM_DELIMITER: &str = "\n\n---\n# Team Instructions\n";

/// Strip the baked team-instructions suffix from every stored `system_prompt`.
///
/// Ordering (see `run_boot_migrations`): runs BEFORE the preset move, so the
/// preset rows still in the agent store are cleaned before they are copied
/// to `presets.json`, and before the preset-unlink copy so an agent never
/// copies a suffix this migration is about to remove. Works on raw JSON so
/// every row keeps every other field as written.
pub fn strip_baked_team_instructions(app: &tauri::AppHandle) {
    let Ok(base_dir) = crate::managed_agents::managed_agents_base_dir(app) else {
        return;
    };
    match strip_baked_team_instructions_in_dir(&base_dir) {
        Ok(0) => {}
        Ok(stripped) => eprintln!(
            "kuzz-desktop: team-suffix-strip: removed the baked team-instructions suffix from \
             {stripped} record(s)"
        ),
        Err(e) => eprintln!("kuzz-desktop: team-suffix-strip: {e}"),
    }
}

/// Core logic, decoupled from the Tauri `AppHandle` for testing.
///
/// `base_dir` is the managed-agents base directory (`<AppDataDir>/agents/`).
/// Returns the number of records changed; `Ok(0)` means nothing to do and
/// nothing was written, so a second boot is a clean no-op.
pub(super) fn strip_baked_team_instructions_in_dir(base_dir: &Path) -> Result<usize, String> {
    let agents_path = base_dir.join("managed-agents.json");
    if !agents_path.exists() {
        return Ok(0);
    }
    let content = std::fs::read_to_string(&agents_path)
        .map_err(|e| format!("failed to read managed-agents.json: {e}"))?;
    let mut all: Vec<serde_json::Value> = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse managed-agents.json: {e}"))?;

    // Applies to every record: the preset rows carry the suffix as surely as
    // the agents made from them, and neither `team_id` nor `persona_id` nor a
    // key is evidence either way.
    let mut stripped = 0usize;
    for record in all.iter_mut() {
        let Some(prompt) = record.get("system_prompt").and_then(|v| v.as_str()) else {
            continue;
        };
        // Last occurrence only, mirroring the observer parser's own
        // `lastIndexOf` guard (`agentSessionTranscriptHelpers.ts`): a persona
        // body may legitimately quote a delimiter-shaped passage, and only the
        // final one is the boundary `compose_prompt()` appended.
        let Some(at) = prompt.rfind(TEAM_DELIMITER) else {
            continue;
        };
        let head = prompt[..at].to_string();
        // An empty head means the record held nothing but the baked suffix.
        // Store null rather than "", so no phantom empty prompt is left behind.
        record["system_prompt"] = if head.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(head)
        };
        stripped += 1;
    }

    if stripped == 0 {
        return Ok(0);
    }

    // Pre-migration backup, taken ONCE (same contract as the B5 backfill): a
    // re-run after a partial failure must not replace the pristine backup with
    // a half-migrated snapshot. Owner-only from the initial open, and sited next
    // to the resolved store — see `create_restricted_backup_once` and
    // `resolved_backup_path`.
    let bak_path = crate::util::resolved_backup_path(
        &agents_path,
        "managed-agents.json.pre-team-suffix-strip.bak",
    );
    crate::util::create_restricted_backup_once(&bak_path, content.as_bytes())
        .map_err(|e| format!("failed to write pre-strip backup: {e}"))?;

    let payload = serde_json::to_vec_pretty(&all)
        .map_err(|e| format!("failed to serialize managed-agents.json: {e}"))?;
    crate::managed_agents::atomic_write_json_restricted(&agents_path, &payload)?;
    Ok(stripped)
}

#[cfg(test)]
#[path = "team_suffix_tests.rs"]
mod tests;
