//! Host follows the app (akak 2026-10-20). Agents own their settings and keep
//! what they were made with, except an agent made from the built-in Host:
//! Host is the app's routing agent, so its prompt is re-copied from the
//! current app version at every boot when it differs.

use std::path::Path;

use crate::managed_agents::{built_in_persona_records, HOST_PERSONA_ID};

/// Boot entry. Runs after the preset unlink, so every Host agent already
/// carries its own prompt.
pub fn refresh_host_agent_prompts(app: &tauri::AppHandle) {
    let Ok(base_dir) = crate::managed_agents::managed_agents_base_dir(app) else {
        return;
    };
    let Some(host) = built_in_persona_records(&crate::util::now_iso())
        .into_iter()
        .find(|preset| preset.id == HOST_PERSONA_ID)
    else {
        return;
    };
    match refresh_host_prompts_in_dir(&base_dir, &host.system_prompt) {
        Ok(refreshed) if refreshed.is_empty() => {}
        Ok(refreshed) => eprintln!("kuzz-desktop: host-prompt: refreshed {refreshed:?}"),
        Err(e) => eprintln!("kuzz-desktop: host-prompt: {e}"),
    }
}

/// Core logic, decoupled from the Tauri `AppHandle` for testing. Edits only
/// `system_prompt` of Host agents, as raw JSON; every other row and field is
/// written back as read. Returns the names of the agents it changed; writes
/// nothing when that is empty.
pub(super) fn refresh_host_prompts_in_dir(
    base_dir: &Path,
    host_prompt: &str,
) -> Result<Vec<String>, String> {
    let agents_path = base_dir.join("managed-agents.json");
    if !agents_path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&agents_path)
        .map_err(|e| format!("failed to read managed-agents.json: {e}"))?;
    let mut rows: Vec<serde_json::Value> = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse managed-agents.json: {e}"))?;

    let mut refreshed = Vec::new();
    for row in &mut rows {
        if !crate::managed_agents::is_agent_row(row)
            || row.get("source_preset_id").and_then(|v| v.as_str()) != Some(HOST_PERSONA_ID)
            || row.get("system_prompt").and_then(|v| v.as_str()) == Some(host_prompt)
        {
            continue;
        }
        let Some(object) = row.as_object_mut() else {
            continue;
        };
        object.insert("system_prompt".into(), host_prompt.into());
        refreshed.push(
            object
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string(),
        );
    }
    if refreshed.is_empty() {
        return Ok(refreshed);
    }
    let payload = serde_json::to_vec_pretty(&rows)
        .map_err(|e| format!("failed to serialize managed-agents.json: {e}"))?;
    crate::managed_agents::atomic_write_json_restricted(&agents_path, &payload)?;
    Ok(refreshed)
}

#[cfg(test)]
#[path = "host_prompt_tests.rs"]
mod tests;
