//! One-time unlink of agents from the presets they were made from (akak
//! 2026-10-19). An agent owns everything it needs: this copies each linked
//! agent's preset (and its team's instructions) into the agent and keeps the
//! preset id only as a label. After it, editing or deleting a preset never
//! changes an agent.
//!
//! Remove the call in the first release after both of akak's PCs have started
//! a version that ran it; `copy_preset_into_agent` stays (create uses it).

use std::path::Path;

use crate::managed_agents::{
    persona_events::copy_preset_into_agent, AgentDefinition, ManagedAgentRecord, TeamRecord,
};

/// What one run changed, for the boot log.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct UnlinkReport {
    /// Agents now holding their own copy of their preset.
    pub copied: Vec<String>,
    /// Agents whose preset was already gone: they keep their own fields (the
    /// last start-time copy of prompt/model/provider/runtime); the preset's
    /// env, effort and description were never stored on them and are lost.
    pub orphans_unlinked: Vec<String>,
    /// Agents whose preset is gone and that hold no prompt of their own: left
    /// linked (nothing to keep), retried on the next boot.
    pub kept_linked: Vec<String>,
}

/// Boot entry: copy every linked agent's preset into it and drop the link.
/// Runs last in `run_boot_migrations`, after every migration that still reads
/// the legacy link.
pub fn unlink_agents_from_presets(app: &tauri::AppHandle) {
    let Ok(base_dir) = crate::managed_agents::managed_agents_base_dir(app) else {
        return;
    };
    // Built-ins merged, so `builtin:*` links are found, not treated as gone.
    let presets = match crate::managed_agents::load_personas(app) {
        Ok(presets) => presets,
        Err(e) => {
            eprintln!("kuzz-desktop: preset-unlink: presets unreadable, retrying next boot: {e}");
            return;
        }
    };
    let teams = crate::managed_agents::load_teams(app).unwrap_or_default();
    match unlink_agents_from_presets_in_dir(&base_dir, &presets, &teams) {
        Ok(report) if report == UnlinkReport::default() => {}
        Ok(report) => eprintln!(
            "kuzz-desktop: preset-unlink: copied {} agent(s) {:?}; preset already gone, kept \
             their own settings: {:?}; preset gone and no prompt, left as is: {:?}",
            report.copied.len(),
            report.copied,
            report.orphans_unlinked,
            report.kept_linked
        ),
        Err(e) => eprintln!("kuzz-desktop: preset-unlink: {e}"),
    }
}

/// Core logic, decoupled from the Tauri `AppHandle` for testing. Nothing to
/// do writes nothing, so a second boot is a clean no-op.
pub(super) fn unlink_agents_from_presets_in_dir(
    base_dir: &Path,
    presets: &[AgentDefinition],
    teams: &[TeamRecord],
) -> Result<UnlinkReport, String> {
    let agents_path = base_dir.join("managed-agents.json");
    if !agents_path.exists() {
        return Ok(UnlinkReport::default());
    }
    let content = std::fs::read_to_string(&agents_path)
        .map_err(|e| format!("failed to read managed-agents.json: {e}"))?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse managed-agents.json: {e}"))?;
    // Leftover preset rows (see `preset_store`) are carried through as written.
    let (agent_rows, leftovers): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(crate::managed_agents::is_agent_row);
    let mut all: Vec<ManagedAgentRecord> = agent_rows
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("failed to parse managed-agents.json: {e}"))?;

    let linked = |record: &ManagedAgentRecord| record.legacy_persona_id.is_some();
    if !all.iter().any(linked) {
        return Ok(UnlinkReport::default());
    }

    // Pre-migration backup, taken ONCE: a re-run after a partial failure must
    // not replace the pristine backup with a half-migrated snapshot.
    let bak_path =
        crate::util::resolved_backup_path(&agents_path, "managed-agents.json.before-preset-unlink");
    crate::util::create_restricted_backup_once(&bak_path, content.as_bytes())
        .map_err(|e| format!("failed to write pre-unlink backup: {e}"))?;

    let mut report = UnlinkReport::default();
    for record in all.iter_mut().filter(|record| linked(record)) {
        let preset_id = record.legacy_persona_id.clone().unwrap_or_default();
        match presets.iter().find(|preset| preset.id == preset_id) {
            Some(preset) => {
                copy_preset_into_agent(record, preset);
                report.copied.push(record.name.clone());
            }
            None if has_own_prompt(record) => report.orphans_unlinked.push(record.name.clone()),
            None => {
                report.kept_linked.push(record.name.clone());
                continue;
            }
        }
        if record.team_instructions.is_none() {
            record.team_instructions = team_instructions(record, teams);
        }
        // The label never names the agent itself: an older desktop made a
        // preset per agent at the agent's own pubkey.
        record.source_preset_id = Some(preset_id).filter(|id| *id != record.pubkey);
        record.legacy_persona_id = None;
    }

    if report.copied.is_empty() && report.orphans_unlinked.is_empty() {
        return Ok(report);
    }
    let mut rows = leftovers;
    for record in &all {
        rows.push(
            serde_json::to_value(record)
                .map_err(|e| format!("failed to serialize managed-agents.json: {e}"))?,
        );
    }
    let payload = serde_json::to_vec_pretty(&rows)
        .map_err(|e| format!("failed to serialize managed-agents.json: {e}"))?;
    crate::managed_agents::atomic_write_json_restricted(&agents_path, &payload)?;
    Ok(report)
}

fn has_own_prompt(record: &ManagedAgentRecord) -> bool {
    record
        .system_prompt
        .as_deref()
        .is_some_and(|prompt| !prompt.trim().is_empty())
}

/// The instructions of the agent's team, as launch read them until now.
fn team_instructions(record: &ManagedAgentRecord, teams: &[TeamRecord]) -> Option<String> {
    teams
        .iter()
        .find(|team| Some(team.id.as_str()) == record.team_id.as_deref())
        .and_then(|team| team.instructions.as_deref())
        .map(str::trim)
        .filter(|instructions| !instructions.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "preset_unlink_tests.rs"]
mod tests;
