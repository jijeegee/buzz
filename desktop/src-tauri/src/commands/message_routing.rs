//! Smart routing commands: `route_message` (one routing call for the
//! composer) and the Task models reads/writes for its model choice.

use serde::Serialize;
use tauri::AppHandle;

use crate::{
    managed_agents::{
        channel_routing::{load_channel_routing, ChannelRoutingMode},
        load_global_agent_config, managed_agents_base_dir,
        task_models::{
            load_task_models, normalize_choice, save_task_model, TaskModelSetting, TaskModels,
            KNOWN_TASK_IDS, MESSAGE_ROUTING_TASK,
        },
    },
    message_routing::{
        log::{append_routing_log, routing_log_path, RoutingLogLine},
        model::{
            model_label, resolve_router_model, ResolvedRouterModel, RouterNotReady, RouterProvider,
            ROUTER_PROVIDERS,
        },
        route_with, router_agent_config, RouteMessageInput, RouteMessageResult, RouterSkip,
        ROUTER_MAX_OUTPUT_TOKENS,
    },
    util::now_iso,
};

/// Resolve the routing model from saved task models, the global config, and
/// the process environment (global `env_vars` win; blank = absent).
pub(crate) fn resolve_router_model_for_app<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task_models: &TaskModels,
) -> Result<Result<ResolvedRouterModel, RouterNotReady>, String> {
    let global = load_global_agent_config(app)?;
    Ok(resolve_router_model(
        task_models.get(MESSAGE_ROUTING_TASK),
        global.provider.as_deref(),
        |key| {
            global
                .env_vars
                .get(key)
                .filter(|value| !value.trim().is_empty())
                .cloned()
                .or_else(|| std::env::var(key).ok())
        },
    ))
}

/// Whether Smart routing can call a model right now.
pub(crate) fn router_model_ready<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    Ok(resolve_router_model_for_app(app, &load_task_models(app)?)?.is_ok())
}

/// Route one composer send. Returns `Skipped { NotConfigured }` without a
/// model call unless Smart routing is the saved mode and a key is present.
/// Each real call appends one line to the comparison log.
#[tauri::command]
pub async fn route_message(
    input: RouteMessageInput,
    app: AppHandle,
) -> Result<RouteMessageResult, String> {
    let resolve_app = app.clone();
    let resolved = tokio::task::spawn_blocking(move || -> Result<_, String> {
        if load_channel_routing(&resolve_app)? != ChannelRoutingMode::DesktopRouter {
            return Ok(None);
        }
        Ok(resolve_router_model_for_app(&resolve_app, &load_task_models(&resolve_app)?)?.ok())
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))??;
    let Some(resolved) = resolved else {
        return Ok(RouteMessageResult::Skipped {
            reason: RouterSkip::NotConfigured,
        });
    };
    let cfg = router_agent_config(&resolved);
    let model = resolved.model.clone();
    let outcome = route_with(&input, |system, user| async move {
        buzz_agent_pkg::complete_once(&cfg, &system, &user, ROUTER_MAX_OUTPUT_TOKENS, &model)
            .await
            .map_err(|error| error.to_string())
    })
    .await;
    if outcome.called {
        let line = RoutingLogLine::new(
            now_iso(),
            input.phase,
            input.channel_id.clone(),
            &resolved.model,
            &input.message,
            &outcome,
        );
        let written = tokio::task::spawn_blocking(move || {
            managed_agents_base_dir(&app)
                .and_then(|dir| append_routing_log(&routing_log_path(&dir), &line))
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))
        .and_then(|result| result);
        if let Err(error) = written {
            eprintln!("buzz-desktop: smart routing log append failed: {error}");
        }
    }
    Ok(outcome.result)
}

/// One provider choice in a Task models row.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskModelProviderOption {
    id: &'static str,
    label: &'static str,
    has_key: bool,
    default_model: &'static str,
}

/// What Settings › Models › Task models renders for one task.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskModelStatus {
    task_id: &'static str,
    /// Saved choices; `None` = automatic.
    provider: Option<String>,
    model: Option<String>,
    /// What a call would use right now (absent when not ready).
    effective_provider: Option<RouterProvider>,
    effective_model: Option<String>,
    /// e.g. "Claude Haiku 4.5".
    model_label: Option<String>,
    ready: bool,
    not_ready_reason: Option<String>,
    providers: Vec<TaskModelProviderOption>,
}

fn task_model_statuses<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task_models: &TaskModels,
) -> Result<Vec<TaskModelStatus>, String> {
    let global = load_global_agent_config(app)?;
    let has_key = |key: &str| {
        global
            .env_vars
            .get(key)
            .is_some_and(|value| !value.trim().is_empty())
            || std::env::var(key).is_ok_and(|value| !value.trim().is_empty())
    };
    let resolved = resolve_router_model_for_app(app, task_models)?;
    let saved = task_models.get(MESSAGE_ROUTING_TASK);
    Ok(vec![TaskModelStatus {
        task_id: MESSAGE_ROUTING_TASK,
        provider: saved.and_then(|setting| setting.provider.clone()),
        model: saved.and_then(|setting| setting.model.clone()),
        effective_provider: resolved.as_ref().ok().map(|resolved| resolved.provider),
        effective_model: resolved
            .as_ref()
            .ok()
            .map(|resolved| resolved.model.clone()),
        model_label: resolved
            .as_ref()
            .ok()
            .map(|resolved| model_label(&resolved.model)),
        ready: resolved.is_ok(),
        not_ready_reason: resolved.as_ref().err().map(RouterNotReady::message),
        providers: ROUTER_PROVIDERS
            .into_iter()
            .map(|provider| TaskModelProviderOption {
                id: provider.id(),
                label: provider.label(),
                has_key: has_key(provider.key_env()),
                default_model: provider.default_model(),
            })
            .collect(),
    }])
}

#[tauri::command]
pub async fn get_task_models(app: AppHandle) -> Result<Vec<TaskModelStatus>, String> {
    tokio::task::spawn_blocking(move || task_model_statuses(&app, &load_task_models(&app)?))
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))?
}

/// Save one task's provider and model (blank/`None` = automatic) as one
/// write. Never touches `GlobalAgentConfig`, so no agent restarts.
#[tauri::command]
pub async fn set_task_model(
    task_id: String,
    provider: Option<String>,
    model: Option<String>,
    app: AppHandle,
) -> Result<Vec<TaskModelStatus>, String> {
    let task_id = KNOWN_TASK_IDS
        .iter()
        .copied()
        .find(|known| *known == task_id)
        .ok_or_else(|| format!("Unknown app task: {task_id}"))?;
    let provider = normalize_choice(provider, "Provider")?;
    if let Some(id) = provider.as_deref() {
        if RouterProvider::from_id(id).is_none() {
            return Err(format!("{id} can't be used for this task."));
        }
    }
    let setting = TaskModelSetting {
        provider,
        model: normalize_choice(model, "Model")?,
    };
    tokio::task::spawn_blocking(move || {
        let saved = save_task_model(&app, task_id, setting)?;
        task_model_statuses(&app, &saved)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}
