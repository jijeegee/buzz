//! Which route Smart routing calls: an API-key provider over HTTP, or a
//! signed-in harness CLI run one-shot.
//!
//! Each provider's subscription policy decides its path:
//! - **API keys** (Anthropic, OpenAI, OpenRouter) are called directly with
//!   the key the Providers tab saved.
//! - **Codex (ChatGPT subscription)** runs the official `codex exec`
//!   one-shot. The Codex CLI owns its sign-in; Buzz never reads its tokens.
//! - **Claude Code (Claude subscription)** runs the official `claude -p`
//!   one-shot. Anthropic does not allow third-party apps to reuse Claude
//!   subscription OAuth tokens for direct API calls, so the CLI is the only
//!   subscription path and Buzz never touches those tokens.
//!
//! A CLI call takes ~5 s (process start + one agent turn) against well
//! under 1 s for an API key, so Automatic prefers API keys. The per-route
//! defaults are the cheapest fast model each route offers; they are router
//! facts, not harness capability facts, so they live here rather than in
//! `KnownAcpRuntime`.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::managed_agents::task_models::TaskModelSetting;

/// One route Smart routing can call.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum RouterProvider {
    Anthropic,
    Openai,
    Openrouter,
    /// `codex exec` on the Codex CLI's own (ChatGPT) sign-in.
    Codex,
    /// `claude -p` on the Claude Code CLI's own (Claude) sign-in.
    ClaudeCode,
}

/// How a route authenticates.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum RouterAuthKind {
    ApiKey,
    Subscription,
}

/// Automatic order, fastest first: API keys (sub-second HTTP), then Codex
/// (4.6–6.0 s measured), then Claude Code (5.7–6.0 s measured). The global default
/// provider, when it is an API-key route with a key, is tried before all.
pub const ROUTER_PROVIDERS: [RouterProvider; 5] = [
    RouterProvider::Anthropic,
    RouterProvider::Openai,
    RouterProvider::Openrouter,
    RouterProvider::Codex,
    RouterProvider::ClaudeCode,
];

/// Hard limit for one API-key routing call, retries included.
pub const ROUTER_API_DEADLINE: Duration = Duration::from_millis(2_000);
/// Hard limit for one CLI routing call: process start plus one agent turn
/// measured 4.6–6.0 s, so 12 s leaves room for a cold start.
pub const ROUTER_CLI_DEADLINE: Duration = Duration::from_millis(12_000);
/// How long Enter waits on an API-key route before sending unassigned.
pub const SEND_WAIT_API_MS: u64 = 1_200;
/// How long Enter waits on a CLI route. The preview call started while
/// typing is usually most of the way there by then.
pub const SEND_WAIT_CLI_MS: u64 = 8_000;

impl RouterProvider {
    /// The id saved in `task-models.json`. The API-key ids are shared with
    /// `GlobalAgentConfig.provider` and the Providers tab.
    pub fn id(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic API key",
            Self::Openai => "OpenAI API key",
            Self::Openrouter => "OpenRouter API key",
            Self::Codex => "Codex (ChatGPT subscription)",
            Self::ClaudeCode => "Claude Code (Claude subscription, slower)",
        }
    }

    /// Short name for messages: the vendor for API keys, the CLI otherwise.
    pub fn short_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::Openai => "OpenAI",
            Self::Openrouter => "OpenRouter",
            Self::Codex => "Codex",
            Self::ClaudeCode => "Claude Code",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        ROUTER_PROVIDERS
            .into_iter()
            .find(|provider| provider.id() == id.trim())
    }

    pub fn auth_kind(self) -> RouterAuthKind {
        match self {
            Self::Anthropic | Self::Openai | Self::Openrouter => RouterAuthKind::ApiKey,
            Self::Codex | Self::ClaudeCode => RouterAuthKind::Subscription,
        }
    }

    /// The API-key env var the Providers tab writes; `None` for CLI routes.
    pub fn key_env(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            Self::Openai => Some("OPENAI_COMPAT_API_KEY"),
            Self::Openrouter => Some("OPENROUTER_API_KEY"),
            Self::Codex | Self::ClaudeCode => None,
        }
    }

    /// Base-URL override honored the same way `buzz-agent` honors it, so a
    /// proxy configured for agents also carries routing calls.
    fn base_url_env(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("ANTHROPIC_BASE_URL"),
            Self::Openai => Some("OPENAI_COMPAT_BASE_URL"),
            Self::Openrouter => Some("OPENROUTER_BASE_URL"),
            Self::Codex | Self::ClaudeCode => None,
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::Openai => "https://api.openai.com/v1",
            Self::Openrouter => "https://openrouter.ai/api/v1",
            Self::Codex | Self::ClaudeCode => "",
        }
    }

    /// Cheapest fast model: routing needs ~10 output tokens and no thinking,
    /// and a reasoning model could spend the 40-token cap on reasoning and
    /// return nothing. Codex offers only reasoning models, so its route runs
    /// the fast one at low effort (see `cli::cli_args`).
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-haiku-4-5",
            Self::Openai => "gpt-4.1-nano",
            Self::Openrouter => "anthropic/claude-haiku-4.5",
            Self::Codex => "gpt-6-luna",
            Self::ClaudeCode => "haiku",
        }
    }

    /// Wall-clock limit for one call on this route.
    pub fn deadline(self) -> Duration {
        match self.auth_kind() {
            RouterAuthKind::ApiKey => ROUTER_API_DEADLINE,
            RouterAuthKind::Subscription => ROUTER_CLI_DEADLINE,
        }
    }

    /// How long the composer waits on Enter for this route.
    pub fn send_wait_ms(self) -> u64 {
        match self.auth_kind() {
            RouterAuthKind::ApiKey => SEND_WAIT_API_MS,
            RouterAuthKind::Subscription => SEND_WAIT_CLI_MS,
        }
    }

    /// The `buzz-agent` provider of an API-key route.
    pub(crate) fn agent_provider(self) -> Option<buzz_agent_pkg::Provider> {
        match self {
            Self::Anthropic => Some(buzz_agent_pkg::Provider::Anthropic),
            Self::Openai => Some(buzz_agent_pkg::Provider::OpenAi),
            Self::Openrouter => Some(buzz_agent_pkg::Provider::OpenRouter),
            Self::Codex | Self::ClaudeCode => None,
        }
    }
}

/// Human label for a model id, e.g. "Claude Haiku 4.5". Unknown ids are
/// shown as-is.
pub fn model_label(model: &str) -> String {
    match model {
        "claude-haiku-4-5" | "anthropic/claude-haiku-4.5" => "Claude Haiku 4.5".to_string(),
        "gpt-4.1-nano" => "GPT-4.1 nano".to_string(),
        "haiku" => "Claude Haiku".to_string(),
        "gpt-6-luna" => "GPT-6-Luna".to_string(),
        other => other.to_string(),
    }
}

/// USD per million (input, output) tokens for the default API-key models,
/// used only for the comparison log's estimate. Unknown models (and every
/// subscription call, logged as `codex:…` / `claude-code:…`) have none.
pub fn model_price_per_mtok(model: &str) -> Option<(f64, f64)> {
    match model {
        "claude-haiku-4-5" | "anthropic/claude-haiku-4.5" => Some((1.0, 5.0)),
        "gpt-4.1-nano" => Some((0.10, 0.40)),
        _ => None,
    }
}

/// How one resolved route is called. `api_key` never leaves Rust.
#[derive(Clone, PartialEq, Eq)]
pub enum RouterBackend {
    Api {
        api_key: String,
        base_url: String,
    },
    /// The resolved CLI binary (`codex` / `claude`), signed in.
    Cli {
        program: PathBuf,
    },
}

/// Everything one routing call needs.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedRouterModel {
    pub provider: RouterProvider,
    pub model: String,
    pub model_is_default: bool,
    pub backend: RouterBackend,
}

impl std::fmt::Debug for ResolvedRouterModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("ResolvedRouterModel");
        debug
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("model_is_default", &self.model_is_default);
        match &self.backend {
            RouterBackend::Api { base_url, .. } => debug.field("base_url", base_url),
            RouterBackend::Cli { program } => debug.field("program", program),
        };
        debug.finish_non_exhaustive()
    }
}

/// Whether a CLI route's binary is installed and signed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliState {
    Ready(PathBuf),
    NotInstalled,
    SignedOut,
}

/// Why Smart routing cannot call a model right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouterNotReady {
    /// No route is ready: no API key and no signed-in CLI.
    NoRoute,
    /// The user picked an API-key route whose key is missing.
    ProviderKeyMissing(RouterProvider),
    /// The user picked a CLI route whose CLI is not installed.
    CliNotInstalled(RouterProvider),
    /// The user picked a CLI route whose CLI is not signed in.
    CliSignedOut(RouterProvider),
    /// The saved provider id is not one Smart routing can call.
    UnsupportedProvider(String),
}

impl RouterNotReady {
    pub fn message(&self) -> String {
        match self {
            Self::NoRoute => "Sign in to Codex or Claude Code, or add an API key".to_string(),
            Self::ProviderKeyMissing(provider) => {
                format!("Needs an {} API key", provider.short_name())
            }
            Self::CliNotInstalled(provider) => {
                format!("{} isn't installed", provider.short_name())
            }
            Self::CliSignedOut(provider) => format!("Sign in to {}", provider.short_name()),
            Self::UnsupportedProvider(id) => format!("{id} can't be used for routing"),
        }
    }
}

/// Whether one route can be called right now, and how. `cli` is consulted
/// only for CLI routes; `lookup` only for API-key routes.
pub fn route_backend(
    provider: RouterProvider,
    lookup: &impl Fn(&str) -> Option<String>,
    cli: &impl Fn(RouterProvider) -> CliState,
) -> Result<RouterBackend, RouterNotReady> {
    let Some(key_env) = provider.key_env() else {
        return match cli(provider) {
            CliState::Ready(program) => Ok(RouterBackend::Cli { program }),
            CliState::NotInstalled => Err(RouterNotReady::CliNotInstalled(provider)),
            CliState::SignedOut => Err(RouterNotReady::CliSignedOut(provider)),
        };
    };
    let api_key = lookup(key_env)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or(RouterNotReady::ProviderKeyMissing(provider))?;
    let base_url = provider
        .base_url_env()
        .and_then(lookup)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| provider.default_base_url().to_string());
    Ok(RouterBackend::Api { api_key, base_url })
}

/// Resolve the routing model from the saved task setting, the global default
/// provider, an env lookup (`GlobalAgentConfig.env_vars` first, then the
/// process environment; blank values count as absent), and the CLI routes'
/// install/sign-in state.
///
/// Provider: the saved one (never silently swapped for another route) → the
/// global default provider when it is an API-key route with a key → the
/// first ready route in [`ROUTER_PROVIDERS`]. `cli` runs only for CLI routes
/// the walk reaches. Model: the saved one, else the route's default.
pub fn resolve_router_model(
    setting: Option<&TaskModelSetting>,
    global_provider: Option<&str>,
    lookup: impl Fn(&str) -> Option<String>,
    cli: impl Fn(RouterProvider) -> CliState,
) -> Result<ResolvedRouterModel, RouterNotReady> {
    let saved_provider = setting.and_then(|setting| setting.provider.as_deref());
    let (provider, backend) = match saved_provider {
        Some(id) => {
            let provider = RouterProvider::from_id(id)
                .ok_or_else(|| RouterNotReady::UnsupportedProvider(id.to_string()))?;
            (provider, route_backend(provider, &lookup, &cli)?)
        }
        None => global_provider
            .and_then(RouterProvider::from_id)
            .filter(|provider| provider.auth_kind() == RouterAuthKind::ApiKey)
            .into_iter()
            .chain(ROUTER_PROVIDERS)
            .find_map(|provider| {
                route_backend(provider, &lookup, &cli)
                    .ok()
                    .map(|backend| (provider, backend))
            })
            .ok_or(RouterNotReady::NoRoute)?,
    };
    let saved_model = setting.and_then(|setting| setting.model.clone());
    let model_is_default = saved_model.is_none();
    let model = saved_model.unwrap_or_else(|| provider.default_model().to_string());
    Ok(ResolvedRouterModel {
        provider,
        model,
        model_is_default,
        backend,
    })
}
