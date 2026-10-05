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
        GlobalAgentConfig,
    },
    message_routing::{
        cli::{cli_models, cli_state, complete_via_cli, is_safe_cli_model},
        log::{append_routing_log, routing_log_path, RoutingLogLine},
        model::{
            model_label, resolve_router_model, route_backend, ResolvedRouterModel, RouterAuthKind,
            RouterBackend, RouterNotReady, RouterProvider, ROUTER_PROVIDERS, SEND_WAIT_API_MS,
        },
        route_with_deadline, router_agent_config, RouteMessageInput, RouteMessageResult,
        RouterSkip, ROUTER_MAX_OUTPUT_TOKENS,
    },
    util::now_iso,
};

/// `GlobalAgentConfig.env_vars` first, then the process environment;
/// blank = absent.
fn env_lookup(global: &GlobalAgentConfig) -> impl Fn(&str) -> Option<String> + '_ {
    |key| {
        global
            .env_vars
            .get(key)
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .or_else(|| std::env::var(key).ok())
    }
}

/// Resolve the routing model from saved task models, the global config, the
/// process environment, and the CLI routes' sign-in state. Blocking: a CLI
/// sign-in probe may spawn.
pub(crate) fn resolve_router_model_for_app<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task_models: &TaskModels,
) -> Result<Result<ResolvedRouterModel, RouterNotReady>, String> {
    let global = load_global_agent_config(app)?;
    Ok(resolve_router_model(
        task_models.get(MESSAGE_ROUTING_TASK),
        global.provider.as_deref(),
        env_lookup(&global),
        cli_state,
    ))
}

/// Why Smart routing can't call a model right now, or `None` when it can.
pub(crate) fn router_not_ready<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<RouterNotReady>, String> {
    Ok(resolve_router_model_for_app(app, &load_task_models(app)?)?.err())
}

/// Route one batch of composer sends. Returns `Skipped { NotConfigured }` without a
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
    let deadline = resolved.provider.deadline();
    let outcome = match &resolved.backend {
        RouterBackend::Api { .. } => {
            let cfg = router_agent_config(&resolved)
                .ok_or_else(|| "routing provider has no HTTP client".to_string())?;
            let model = resolved.model.clone();
            route_with_deadline(&input, deadline, |system, user| async move {
                buzz_agent_pkg::complete_once(
                    &cfg,
                    &system,
                    &user,
                    ROUTER_MAX_OUTPUT_TOKENS,
                    &model,
                )
                .await
                .map_err(|error| error.to_string())
            })
            .await
        }
        RouterBackend::Cli { program } => {
            let (provider, program, model) =
                (resolved.provider, program.clone(), resolved.model.clone());
            route_with_deadline(&input, deadline, |system, user| async move {
                tokio::task::spawn_blocking(move || {
                    complete_via_cli(provider, &program, &model, &system, &user, deadline)
                })
                .await
                .map_err(|e| format!("spawn_blocking failed: {e}"))?
            })
            .await
        }
    };
    if outcome.called {
        let line = RoutingLogLine::new(
            now_iso(),
            input.phase,
            input.channel_id.clone(),
            &log_model(&resolved),
            &input.messages,
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

/// The comparison log's model: bare for API keys (so the cost estimate
/// applies), `codex:<model>` / `claude-code:<model>` for subscription calls.
fn log_model(resolved: &ResolvedRouterModel) -> String {
    match resolved.backend {
        RouterBackend::Api { .. } => resolved.model.clone(),
        RouterBackend::Cli { .. } => format!("{}:{}", resolved.provider.id(), resolved.model),
    }
}

/// One route choice in a Task models row.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskModelProviderOption {
    id: &'static str,
    label: &'static str,
    kind: RouterAuthKind,
    /// Callable right now (key saved, or CLI installed and signed in).
    ready: bool,
    /// Why not, e.g. "Sign in to Codex" (absent when ready).
    unavailable_reason: Option<String>,
    default_model: &'static str,
    /// Suggested model ids for a subscription route; empty for API keys (the
    /// row discovers those from the provider).
    models: Vec<String>,
    /// How long Enter waits for a routing answer on this route.
    send_wait_ms: u64,
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
    /// Enter's wait budget on the effective route.
    send_wait_ms: u64,
    providers: Vec<TaskModelProviderOption>,
}

fn task_model_statuses<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task_models: &TaskModels,
) -> Result<Vec<TaskModelStatus>, String> {
    let global = load_global_agent_config(app)?;
    let lookup = env_lookup(&global);
    let saved = task_models.get(MESSAGE_ROUTING_TASK);
    let resolved = resolve_router_model(saved, global.provider.as_deref(), &lookup, cli_state);
    let providers = ROUTER_PROVIDERS
        .into_iter()
        .map(|provider| {
            let backend = route_backend(provider, &lookup, &cli_state);
            TaskModelProviderOption {
                id: provider.id(),
                label: provider.label(),
                kind: provider.auth_kind(),
                ready: backend.is_ok(),
                unavailable_reason: backend.err().map(|reason| reason.message()),
                default_model: provider.default_model(),
                models: cli_models(provider),
                send_wait_ms: provider.send_wait_ms(),
            }
        })
        .collect();
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
        send_wait_ms: resolved.as_ref().map_or(SEND_WAIT_API_MS, |resolved| {
            resolved.provider.send_wait_ms()
        }),
        providers,
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
    let route = match provider.as_deref() {
        Some(id) => Some(
            RouterProvider::from_id(id)
                .ok_or_else(|| format!("{id} can't be used for this task."))?,
        ),
        None => None,
    };
    let model = normalize_choice(model, "Model")?;
    // A subscription route passes the model id on the CLI's command line.
    if let (Some(route), Some(model)) = (route, model.as_deref()) {
        if route.auth_kind() == RouterAuthKind::Subscription && !is_safe_cli_model(model) {
            return Err(format!(
                "{model} can't be used with {}: model ids are letters, digits, and . - _ : /",
                route.short_name()
            ));
        }
    }
    let setting = TaskModelSetting { provider, model };
    tokio::task::spawn_blocking(move || {
        let saved = save_task_model(&app, task_id, setting)?;
        task_model_statuses(&app, &saved)
    })
    .await
    .map_err(|e| format!("spawn_blocking failed: {e}"))?
}
