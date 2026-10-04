//! Which provider, model, key, and base URL Smart routing calls.
//!
//! Only API-key providers are eligible: a harness's CLI sign-in (Claude
//! Code, Codex) is a subscription Buzz may not reuse over HTTP, so a
//! subscription-only desktop is honestly "not ready" rather than routed
//! through a token it does not own. The per-provider defaults are the
//! cheapest non-reasoning model each provider offers; they are router facts,
//! not harness capability facts, so they live here rather than in
//! `KnownAcpRuntime`.

use serde::{Deserialize, Serialize};

use crate::managed_agents::task_models::TaskModelSetting;

/// A provider Smart routing can call with an API key.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum RouterProvider {
    Anthropic,
    Openai,
    Openrouter,
}

/// Automatic provider order when the user picked none and the global
/// default provider is not a router provider with a key.
pub const ROUTER_PROVIDERS: [RouterProvider; 3] = [
    RouterProvider::Anthropic,
    RouterProvider::Openai,
    RouterProvider::Openrouter,
];

impl RouterProvider {
    /// The provider id shared with `GlobalAgentConfig.provider` and the
    /// Providers tab.
    pub fn id(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::Openai => "OpenAI",
            Self::Openrouter => "OpenRouter",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        ROUTER_PROVIDERS
            .into_iter()
            .find(|provider| provider.id() == id.trim())
    }

    /// The API-key env var the Providers tab writes for this provider.
    pub fn key_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Openai => "OPENAI_COMPAT_API_KEY",
            Self::Openrouter => "OPENROUTER_API_KEY",
        }
    }

    /// Base-URL override honored the same way `buzz-agent` honors it, so a
    /// proxy configured for agents also carries routing calls.
    fn base_url_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_BASE_URL",
            Self::Openai => "OPENAI_COMPAT_BASE_URL",
            Self::Openrouter => "OPENROUTER_BASE_URL",
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::Openai => "https://api.openai.com/v1",
            Self::Openrouter => "https://openrouter.ai/api/v1",
        }
    }

    /// Cheapest non-reasoning model: routing needs ~10 output tokens and no
    /// thinking, and a reasoning model could spend the 40-token cap on
    /// reasoning and return nothing.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Anthropic => "claude-haiku-4-5",
            Self::Openai => "gpt-4.1-nano",
            Self::Openrouter => "anthropic/claude-haiku-4.5",
        }
    }

    pub(crate) fn agent_provider(self) -> buzz_agent_pkg::Provider {
        match self {
            Self::Anthropic => buzz_agent_pkg::Provider::Anthropic,
            Self::Openai => buzz_agent_pkg::Provider::OpenAi,
            Self::Openrouter => buzz_agent_pkg::Provider::OpenRouter,
        }
    }
}

/// Human label for a model id, e.g. "Claude Haiku 4.5". Unknown ids are
/// shown as-is.
pub fn model_label(model: &str) -> String {
    match model {
        "claude-haiku-4-5" | "anthropic/claude-haiku-4.5" => "Claude Haiku 4.5".to_string(),
        "gpt-4.1-nano" => "GPT-4.1 nano".to_string(),
        other => other.to_string(),
    }
}

/// USD per million (input, output) tokens for the default models, used only
/// for the comparison log's estimate. Unknown models have no estimate.
pub fn model_price_per_mtok(model: &str) -> Option<(f64, f64)> {
    match model {
        "claude-haiku-4-5" | "anthropic/claude-haiku-4.5" => Some((1.0, 5.0)),
        "gpt-4.1-nano" => Some((0.10, 0.40)),
        _ => None,
    }
}

/// Everything one routing call needs. `api_key` never leaves Rust.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedRouterModel {
    pub provider: RouterProvider,
    pub model: String,
    pub model_is_default: bool,
    pub api_key: String,
    pub base_url: String,
}

impl std::fmt::Debug for ResolvedRouterModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedRouterModel")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("model_is_default", &self.model_is_default)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

/// Why Smart routing cannot call a model right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouterNotReady {
    /// No router provider has an API key (e.g. subscription sign-in only).
    NoApiKey,
    /// The user picked a provider whose key is missing.
    ProviderKeyMissing(RouterProvider),
    /// The saved provider id is not one Smart routing can call.
    UnsupportedProvider(String),
}

impl RouterNotReady {
    pub fn message(&self) -> String {
        match self {
            Self::NoApiKey => "Needs an API key".to_string(),
            Self::ProviderKeyMissing(provider) => {
                format!("Needs an {} API key", provider.label())
            }
            Self::UnsupportedProvider(id) => format!("{id} can't be used for routing"),
        }
    }
}

/// Resolve the routing model from the saved task setting, the global default
/// provider, and an env lookup (`GlobalAgentConfig.env_vars` first, then the
/// process environment; blank values count as absent).
///
/// Provider: the saved one → the global default provider when it is a
/// router provider with a key → the first provider in [`ROUTER_PROVIDERS`]
/// with a key. Model: the saved one, else the provider's default.
pub fn resolve_router_model(
    setting: Option<&TaskModelSetting>,
    global_provider: Option<&str>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<ResolvedRouterModel, RouterNotReady> {
    let key_for = |provider: RouterProvider| {
        lookup(provider.key_env()).filter(|value| !value.trim().is_empty())
    };
    let saved_provider = setting.and_then(|setting| setting.provider.as_deref());
    let (provider, api_key) = match saved_provider {
        Some(id) => {
            let provider = RouterProvider::from_id(id)
                .ok_or_else(|| RouterNotReady::UnsupportedProvider(id.to_string()))?;
            let key = key_for(provider).ok_or(RouterNotReady::ProviderKeyMissing(provider))?;
            (provider, key)
        }
        None => global_provider
            .and_then(RouterProvider::from_id)
            .into_iter()
            .chain(ROUTER_PROVIDERS)
            .find_map(|provider| key_for(provider).map(|key| (provider, key)))
            .ok_or(RouterNotReady::NoApiKey)?,
    };
    let saved_model = setting.and_then(|setting| setting.model.clone());
    let model_is_default = saved_model.is_none();
    let model = saved_model.unwrap_or_else(|| provider.default_model().to_string());
    let base_url = lookup(provider.base_url_env())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| provider.default_base_url().to_string());
    Ok(ResolvedRouterModel {
        provider,
        model,
        model_is_default,
        api_key: api_key.trim().to_string(),
        base_url,
    })
}
