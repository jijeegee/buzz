//! The persona edit command surface: `update_persona` (best-effort enqueue)
//! and the `update_persona_with` seam that `update_persona_and_publish` reuses
//! to await relay acceptance for the same save.

use tauri::AppHandle;

use crate::{
    app_state::AppState,
    managed_agents::{
        apply_persona_behavior, apply_persona_effort_level, load_personas, save_personas,
        try_regenerate_nest, validate_agent_definition_text, AgentDefinition, ManagedAgentRecord,
        UpdatePersonaRequest,
    },
    util::now_iso,
};

use super::{normalize_description, pending, retain_persona_pending, trim_optional, trim_required};

/// Return value of the `update_persona` command. Uses flatten so all
/// `AgentDefinition` fields appear at the top level of the JSON response —
/// backward-compatible with callers that already destructure a raw persona object.
#[derive(Debug, serde::Serialize)]
pub struct UpdatePersonaResult {
    #[serde(flatten)]
    persona: AgentDefinition,
}

/// How an agent's profile reaches the relay.
pub(crate) enum ProfileSyncTarget {
    /// Key agent: publish kind:0 signed with its key.
    Keys(nostr::Keys),
    /// Server bot (token mode): `PATCH /auth/bots/{id}/profile`.
    Bot(String),
}

/// The sync target for `record`; `None` when it has neither (unreadable key).
pub(crate) fn profile_sync_target(record: &ManagedAgentRecord) -> Option<ProfileSyncTarget> {
    if record.bot_origin.is_some() {
        return Some(ProfileSyncTarget::Bot(record.pubkey.clone()));
    }
    nostr::Keys::parse(&record.private_key_nsec)
        .ok()
        .map(ProfileSyncTarget::Keys)
}

/// Profile sync params collected under the store lock for async relay publish:
/// (sync target, relay url, display name, avatar url, kind:0 about, auth tag).
pub(super) type ProfileSyncParams = Vec<(
    ProfileSyncTarget,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
)>;

#[tauri::command]
pub async fn update_persona(
    input: UpdatePersonaRequest,
    app: AppHandle,
) -> Result<UpdatePersonaResult, String> {
    let (persona, ()) = update_persona_with(input, app, |app, state, persona| {
        retain_persona_pending(app, state, persona);
        // F2: immediately refresh any shared 30178 heads that include this
        // persona as a member. Best-effort inside retain so a hiccup cannot
        // fail the persona edit itself.
        crate::commands::refresh_team_catalog_heads_for_persona(app, state, &persona.id);
        Ok(())
    })
    .await?;
    Ok(UpdatePersonaResult { persona })
}

/// Save an edited persona and hand the saved record to `retain` while the
/// store lock is still held. Agents made from the preset are not touched: they
/// own their settings.
///
/// `retain` is the only difference between the two update commands:
/// [`update_persona`] enqueues best-effort, while
/// [`sharing::update_persona_and_publish`] prepares a strict publication and
/// returns the event so the caller can await relay acceptance.
pub(super) async fn update_persona_with<R: Send + 'static>(
    input: UpdatePersonaRequest,
    app: AppHandle,
    retain: impl FnOnce(&AppHandle, &AppState, &AgentDefinition) -> Result<R, String> + Send + 'static,
) -> Result<(AgentDefinition, R), String> {
    use tauri::Manager;

    let (result, retained) = tokio::task::spawn_blocking({
        let app = app.clone();
        move || -> Result<(AgentDefinition, R), String> {
            let state = app.state::<AppState>();
            let display_name = trim_required(&input.display_name, "Display name")?;
            let system_prompt = input.system_prompt.clone();
            validate_agent_definition_text(&display_name, &system_prompt)?;
            let description = normalize_description(input.description)?;
            let avatar_url = trim_optional(input.avatar_url);
            let acp_command = trim_optional(input.acp_command);
            let runtime = trim_optional(input.runtime);
            let model = trim_optional(input.model);
            let provider = trim_optional(input.provider);

            let _store_guard = state
                .managed_agents_store_lock
                .lock()
                .map_err(|error| error.to_string())?;
            let mut personas = load_personas(&app)?;
            pending::project_active_persona_sharing(&app, &state, &mut personas);
            let mut index = personas
                .iter()
                .position(|record| record.id == input.id)
                .ok_or_else(|| format!("agent {} not found", input.id))?;
            // A built-in is shared by every account on this computer, so an
            // edit goes to this account's own copy instead (see below).
            let me = crate::managed_agents::active_owner_hex(&app);
            let edited_builtin = match me.as_deref() {
                Some(me) if personas[index].is_builtin => {
                    let copy = crate::managed_agents::builtin_copy(&personas[index], &now_iso());
                    personas.push(copy);
                    index = personas.len() - 1;
                    Some((input.id.clone(), me.to_string()))
                }
                _ => None,
            };
            let persona = &mut personas[index];

            persona.display_name = display_name;
            persona.avatar_url = avatar_url;
            persona.description = description;
            persona.system_prompt = system_prompt;
            persona.acp_command = acp_command;
            persona.runtime = runtime;
            persona.model = model;
            persona.provider = provider;
            apply_persona_effort_level(persona, input.effort_level)?;
            persona.name_pool = input
                .name_pool
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if let Some(env_vars) = input.env_vars {
                crate::managed_agents::validate_user_env_keys(&env_vars)?;
                persona.env_vars = env_vars;
            }
            apply_persona_behavior(persona, input.behavior)?;
            persona.updated_at = now_iso();

            let result = persona.clone();
            save_personas(&app, &personas)?;
            if let Some((builtin_id, me)) = &edited_builtin {
                // The copy replaces the built-in in this account's preset list.
                crate::managed_agents::set_builtin_inactive(&app, builtin_id, me, true)?;
            }

            let retained = retain(&app, &state, &result)?;
            try_regenerate_nest(&app);

            Ok((result, retained))
        }
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))??;

    Ok((result, retained))
}

/// The kind:0 profile sync for `record` with its current name, avatar and
/// description, or `None` when it has no sync target (unreadable key).
pub(super) fn agent_profile_sync(
    state: &AppState,
    record: &ManagedAgentRecord,
) -> Option<(
    ProfileSyncTarget,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    let target = profile_sync_target(record)?;
    let workspace_relay = crate::relay::relay_ws_url_with_override(state);
    let avatar = record.avatar_url.clone().or_else(|| {
        crate::managed_agents::managed_agent_avatar_url(&crate::managed_agents::record_agent_command(
            record,
        ))
    });
    Some((
        target,
        crate::relay::effective_agent_relay_url(&record.relay_url, &workspace_relay),
        record.name.clone(),
        avatar,
        crate::managed_agents::record_effective_description(record),
        record.auth_tag.clone(),
    ))
}

/// Await relay profile sync for agents whose name or description (kind:0
/// about) was just updated. Best-effort — failures are logged, not surfaced.
pub(super) async fn publish_agent_profiles(
    app: &AppHandle,
    params: ProfileSyncParams,
    after: &str,
) {
    use tauri::Manager;

    if params.is_empty() {
        return;
    }
    let state = app.state::<AppState>();
    for (target, relay_url, display_name, avatar_url, about, auth_tag) in params {
        let synced = match &target {
            ProfileSyncTarget::Keys(agent_keys) => {
                crate::relay::sync_managed_agent_profile(
                    &state,
                    &relay_url,
                    agent_keys,
                    &display_name,
                    avatar_url.as_deref(),
                    about.as_deref(),
                    auth_tag.as_deref(),
                )
                .await
            }
            ProfileSyncTarget::Bot(bot_id) => crate::auth::bots::sync_bot_profile(
                &state,
                bot_id,
                &relay_url,
                &display_name,
                avatar_url.as_deref(),
            )
            .await
            .map(|_| ()),
        };
        if let Err(e) = synced {
            eprintln!("kuzz-desktop: relay profile sync failed after {after}: {e}");
        }
    }
}
