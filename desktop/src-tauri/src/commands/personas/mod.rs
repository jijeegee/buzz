use tauri::AppHandle;

use crate::{
    app_state::AppState,
    managed_agents::{
        load_personas, load_teams, save_personas, try_regenerate_nest,
        validate_persona_activation_change, validate_persona_deletion, AgentDefinition,
    },
    util::now_iso,
};

fn trim_required(value: &str, label: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} is required"));
    }
    Ok(trimmed.to_string())
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value.and_then(|candidate| {
        let trimmed = candidate.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

/// Validate the raw authored bytes before applying storage normalization.
/// This ordering is security-relevant: prohibited edge characters must be
/// rejected, never made invisible by trimming.
fn normalize_description(value: Option<String>) -> Result<Option<String>, String> {
    crate::managed_agents::validate_agent_description_text(value.as_deref())?;
    Ok(trim_optional(value))
}

#[cfg(test)]
mod description_normalization_tests {
    use super::normalize_description;

    #[test]
    fn trims_visible_whitespace_and_collapses_blank_to_none() {
        assert_eq!(
            normalize_description(Some("  A careful agent.  ".to_string())).unwrap(),
            Some("A careful agent.".to_string())
        );
        assert_eq!(
            normalize_description(Some("   ".to_string())).unwrap(),
            None
        );
    }

    #[test]
    fn rejects_prohibited_characters_at_the_edges_before_trimming() {
        for value in ["\nA careful agent.", "A careful agent.\n", "\u{feff}Agent"] {
            assert!(normalize_description(Some(value.to_string())).is_err());
        }
    }
}

mod pending;
pub(in crate::commands) use pending::retain_persona_pending;
pub(in crate::commands) use pending::retain_persona_pending_at;
pub(crate) use pending::tombstone_persona_at;
pub(super) use pending::tombstone_persona_pending;
mod create;
pub use create::create_persona;
mod sharing;
pub use sharing::set_persona_shared;
pub use sharing::update_persona_and_publish;
mod update;
pub use update::update_persona;
mod inbound;
pub use inbound::reconcile_inbound_persona_event;
#[cfg(test)]
pub(crate) use inbound::retain_inbound_catalog_witness;

#[tauri::command]
pub async fn list_personas(app: AppHandle) -> Result<Vec<AgentDefinition>, String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut personas = load_personas(&app)?;
        // Each account sees only its own personas (plus the built-in ones).
        let owners = crate::managed_agents::ensure_definition_owners(&app, &personas);
        let me = crate::managed_agents::active_owner_hex(&app);
        personas.retain(|persona| {
            crate::managed_agents::definition_visible(persona, &owners, me.as_deref())
        });
        crate::managed_agents::apply_account_activation(&app, &mut personas);
        pending::project_active_persona_sharing(&app, &state, &mut personas);
        Ok(personas)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

/// Delete a preset. Agents made from it are not touched: an agent owns a copy
/// of everything it was made with, so deleting the template never stops,
/// deletes or changes an agent (akak 2026-10-19).
#[tauri::command]
pub async fn delete_persona(id: String, app: AppHandle) -> Result<(), String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let _store_guard = state
                .managed_agents_store_lock
                .lock()
                .map_err(|error| error.to_string())?;

            // Load and validate the persona before any write.
            let mut personas = load_personas(&app)?;
            let persona = personas
                .iter()
                .find(|record| record.id == id)
                .ok_or_else(|| format!("persona {id} not found"))?
                .clone();
            let referenced_by_team = load_teams(&app)?.iter().any(|team| {
                team.persona_ids
                    .iter()
                    .any(|persona_id| persona_id == id.as_str())
            });
            validate_persona_deletion(&persona, referenced_by_team)?;
            // Only reached for non-builtin, non-team personas (both rejected
            // above), so every deleted persona here is one this owner published.
            let d_tag = crate::managed_agents::persona_events::persona_d_tag(&persona);

            personas.retain(|record| record.id != id);
            save_personas(&app, &personas)?;

            // Side effects strictly after the record is off disk.
            tombstone_persona_pending(&app, &state, &d_tag);
            // _store_guard drops here, before try_regenerate_nest.
        }
        try_regenerate_nest(&app);
        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

#[tauri::command]
pub async fn set_persona_active(
    id: String,
    active: bool,
    app: AppHandle,
) -> Result<AgentDefinition, String> {
    use tauri::Manager;
    tokio::task::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _store_guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let mut personas = load_personas(&app)?;
        let stored_active = personas.iter().any(|p| p.id == id && p.is_active);
        crate::managed_agents::apply_account_activation(&app, &mut personas);
        let me = crate::managed_agents::active_owner_hex(&app);
        let persona = personas
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| format!("agent {id} not found"))?;

        let referenced_by_team = !active
            && load_teams(&app)?.iter().any(|team| {
                team.persona_ids
                    .iter()
                    .any(|persona_id| persona_id == id.as_str())
            });

        validate_persona_activation_change(persona, active, referenced_by_team)?;

        if persona.is_active == active {
            return Ok(persona.clone());
        }

        // A built-in is one record for every account: this account's choice
        // is kept for it alone. Reactivating one that was removed before
        // accounts kept their own choice also brings back the shared record.
        if let Some(me) = me.as_deref() {
            crate::managed_agents::set_builtin_inactive(&app, &id, me, !active)?;
            if !active || stored_active {
                persona.is_active = active;
                let updated = persona.clone();
                try_regenerate_nest(&app);
                return Ok(updated);
            }
        }

        // Save the stored records, not this account's view of them.
        let mut stored = load_personas(&app)?;
        let record = stored
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| format!("agent {id} not found"))?;
        record.is_active = active;
        record.updated_at = now_iso();

        let updated = record.clone();
        save_personas(&app, &stored)?;
        try_regenerate_nest(&app);
        Ok(updated)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

pub(crate) const PNG_MAGIC: [u8; 4] = [0x89, 0x50, 0x4E, 0x47];
mod card;
mod snapshot;
pub use card::*;
#[cfg(test)]
pub(crate) use snapshot::import::decode_snapshot_from_bytes;
pub(crate) use snapshot::import::{
    parse_snapshot_payload_from_bytes, resolve_snapshot_import_behavior, MAX_SNAPSHOT_JSON_BYTES,
    MAX_SNAPSHOT_PNG_BYTES,
};
pub use snapshot::{confirm_agent_snapshot_import, preview_agent_snapshot_import};
pub use snapshot::{encode_agent_snapshot_for_send, export_agent_snapshot};
