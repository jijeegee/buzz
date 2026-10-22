//! One-time copy of presets into their own store (akak 2026-10-20). Presets
//! started out as the agents themselves; an earlier fold then kept them as
//! key-less rows inside `managed-agents.json`. A preset is a template and an
//! agent is a running identity, so each now has its own file:
//! `presets.json` (presets) and `managed-agents.json` (agents).
//!
//! The old rows stay in `managed-agents.json`, unread and carried through
//! every agent save (`storage::write_agent_store`), so an older desktop
//! started on these files still finds its presets instead of publishing
//! their deletion. A marker file records that the copy finished; until it
//! exists, preset deletions are not published (`event_sync`).
//!
//! Also takes over the older fold: a `personas.json` left by a very old
//! desktop is copied into `presets.json` the same way.
//!
//! Remove the call, `LegacyDefinitionRow` and the leftover rows in the first
//! release after both of akak's PCs have started a version that ran it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::managed_agents::{
    AcpSessionPolicy, AgentDefinition, CatalogSource, TeamMemberCatalogSource, DEFAULT_ACP_COMMAND,
};

/// Written next to `presets.json` once the copy finished.
const MOVED_MARKER: &str = "presets.json.moved";

/// Whether the preset copy finished in `base_dir` (or there was nothing to
/// copy). Until then `presets.json` may be missing presets.
pub(crate) fn presets_moved(base_dir: &Path) -> bool {
    base_dir.join(MOVED_MARKER).exists()
}

/// What one run copied, for the boot log.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct MoveReport {
    /// Preset ids copied into `presets.json`.
    pub moved: Vec<String>,
    /// Preset ids already in `presets.json` (kept as they are there).
    pub already_there: Vec<String>,
    /// Rows that could not be read as a preset; left where they are.
    pub unreadable: Vec<String>,
    /// Agents whose own display name differed from their name; the agent
    /// keeps `name` (its display name was a preset leftover).
    pub display_names_dropped: Vec<String>,
}

/// Boot entry. Runs right after the JSON-level `personas.json` fix-ups and
/// before anything loads presets.
pub fn move_presets_to_own_store(app: &tauri::AppHandle) {
    let Ok(base_dir) = crate::managed_agents::managed_agents_base_dir(app) else {
        return;
    };
    match move_presets_in_dir(&base_dir) {
        Ok(report) if report == MoveReport::default() => {}
        Ok(report) => eprintln!("kuzz-desktop: preset-store: {report:?}"),
        Err(e) => eprintln!("kuzz-desktop: preset-store: {e}; retrying next boot"),
    }
}

/// Core logic, decoupled from the Tauri `AppHandle` for testing. Runs once:
/// after the marker exists it does nothing, so a preset deleted later is
/// never copied back from the leftover rows.
pub(super) fn move_presets_in_dir(base_dir: &Path) -> Result<MoveReport, String> {
    if presets_moved(base_dir) {
        return Ok(MoveReport::default());
    }
    let presets_path = base_dir.join(crate::managed_agents::PRESETS_FILE);
    let agents_path = base_dir.join("managed-agents.json");
    let personas_path = base_dir.join("personas.json");

    let mut presets = crate::managed_agents::load_personas_from_path(&presets_path)?;
    let mut report = MoveReport::default();
    let mut add = |preset: AgentDefinition, report: &mut MoveReport| {
        if presets.iter().any(|existing| existing.id == preset.id) {
            report.already_there.push(preset.id);
        } else {
            report.moved.push(preset.id.clone());
            presets.push(preset);
        }
    };

    // A very old desktop's own preset file. Skipped once folded (`.bak`): a
    // legacy app-data copy can bring the file back on every boot.
    if personas_path.exists() && !base_dir.join("personas.json.bak").exists() {
        for persona in crate::managed_agents::load_personas_from_path(&personas_path)? {
            // Built-ins regenerate from code (`merge_personas`).
            if !persona.is_builtin {
                add(persona, &mut report);
            }
        }
    }

    for row in read_rows(&agents_path)? {
        if crate::managed_agents::is_agent_row(&row) {
            if let (Some(name), Some(display)) = (
                row.get("name").and_then(|v| v.as_str()),
                row.get("display_name").and_then(|v| v.as_str()),
            ) {
                if !display.trim().is_empty() && display != name {
                    report.display_names_dropped.push(name.to_string());
                }
            }
            continue;
        }
        let label = row
            .get("slug")
            .or_else(|| row.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        match serde_json::from_value::<LegacyDefinitionRow>(row) {
            Ok(legacy) => match legacy.into_preset() {
                Some(preset) => add(preset, &mut report),
                None => report.unreadable.push(label),
            },
            Err(_) => report.unreadable.push(label),
        }
    }

    let payload = serde_json::to_vec_pretty(&presets)
        .map_err(|e| format!("failed to serialize presets: {e}"))?;
    crate::managed_agents::atomic_write_json_restricted(&presets_path, &payload)?;
    std::fs::write(base_dir.join(MOVED_MARKER), b"")
        .map_err(|e| format!("failed to mark the preset copy done: {e}"))?;
    Ok(report)
}

fn read_rows(path: &Path) -> Result<Vec<serde_json::Value>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read managed-agents.json: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("failed to parse managed-agents.json: {e}"))
}

/// A preset row as the agent store held it. Read only by this migration.
#[derive(Deserialize)]
struct LegacyDefinitionRow {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    acp_command: Option<String>,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    effort_level: Option<String>,
    #[serde(default)]
    name_pool: Vec<String>,
    #[serde(default)]
    is_builtin: bool,
    #[serde(default = "active_by_default")]
    is_active: bool,
    #[serde(default)]
    source_team: Option<String>,
    #[serde(default)]
    source_team_persona_slug: Option<String>,
    #[serde(default)]
    catalog_source: Option<CatalogSource>,
    #[serde(default)]
    team_catalog_source: Option<TeamMemberCatalogSource>,
    #[serde(default)]
    env_vars: BTreeMap<String, String>,
    #[serde(default)]
    definition_respond_to: Option<String>,
    #[serde(default)]
    definition_respond_to_allowlist: Vec<String>,
    #[serde(default)]
    definition_parallelism: Option<u32>,
    #[serde(default)]
    session_policy: AcpSessionPolicy,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
}

fn active_by_default() -> bool {
    true
}

impl LegacyDefinitionRow {
    /// The preset this row stood for (the former `to_definition_view`). A row
    /// without a slug was never addressable as a preset and is dropped.
    fn into_preset(self) -> Option<AgentDefinition> {
        let id = self.slug?;
        Some(AgentDefinition {
            id,
            display_name: self.display_name.unwrap_or(self.name),
            avatar_url: self.avatar_url,
            description: self.description,
            system_prompt: self.system_prompt.unwrap_or_default(),
            acp_command: self
                .acp_command
                .filter(|command| command != DEFAULT_ACP_COMMAND),
            runtime: self.runtime,
            model: self.model,
            provider: self.provider,
            effort_level: self.effort_level,
            name_pool: self.name_pool,
            is_builtin: self.is_builtin,
            is_active: self.is_active,
            shared: false,
            source_team: self.source_team,
            source_team_persona_slug: self.source_team_persona_slug,
            catalog_source: self.catalog_source,
            team_catalog_source: self.team_catalog_source,
            env_vars: self.env_vars,
            respond_to: self.definition_respond_to,
            respond_to_allowlist: self.definition_respond_to_allowlist,
            parallelism: self.definition_parallelism,
            session_policy: self.session_policy,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

/// `preset id → runtime` from the preset store next to `agents_path` (or a
/// very old `personas.json`). Empty when neither can be read; callers then
/// keep the record's own value.
pub(super) fn load_persona_runtimes(
    agents_path: &Path,
) -> std::collections::HashMap<String, String> {
    let Some(dir) = agents_path.parent() else {
        return Default::default();
    };
    let presets_path = dir.join(crate::managed_agents::PRESETS_FILE);
    let path = if presets_path.exists() {
        presets_path
    } else {
        dir.join("personas.json")
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Default::default();
    };
    let Ok(records) = serde_json::from_str::<Vec<serde_json::Value>>(&content) else {
        return Default::default();
    };
    records
        .into_iter()
        .filter_map(|record| {
            let id = record.get("id")?.as_str()?.to_string();
            let runtime = record.get("runtime")?.as_str()?.to_string();
            Some((id, runtime))
        })
        .collect()
}

#[cfg(test)]
#[path = "preset_store_tests.rs"]
mod tests;
