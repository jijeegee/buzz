// ── Providers the user set up in Hermes ──────────────────────────────────────
//
// `hermes-acp` lists models for every provider Hermes considers configured,
// which includes credentials it borrows from outside Hermes: API keys in the OS
// environment, the Claude Code and GitHub CLI logins, and keyless free
// catalogs. A Kuzz Hermes agent may only pick from providers the user set up
// in Hermes itself, so it never runs (and bills) on a credential Hermes merely
// found on the machine.
//
// "Set up in Hermes" is any of:
// - `model.provider` in the home's `config.yaml`, or `active_provider` in
//   `auth.json` (always kept, so the agent can always run);
// - a credential-pool entry from a Hermes flow: `manual`, `manual:*`,
//   `device_code`, `loopback_pkce`, `hermes_pkce`, `oauth`, `config:*`,
//   `model_config`;
// - a credential-pool entry `env:<KEY>`, or an API-key variable, whose KEY is
//   in Hermes's own `.env` (where `hermes setup` saves keys) or in the agent's
//   own Kuzz env vars — not one that only lives in the OS environment;
// - an entry in `auth.json` `providers` (Hermes's OAuth token store);
// - Hermes's own Anthropic login (`.anthropic_oauth.json`);
// - a user-defined endpoint from `config.yaml` (`custom:*` model ids).
//
// Only provider ids, entry sources and `.env` key NAMES are read; no secret
// value is ever parsed out of these files.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::hermes_profile::{native_hermes_home, resolve_hermes_root, HERMES_HOME_ENV_VAR};
use super::{AgentModelInfo, AgentModelsResponse, HarnessProviderInfo};

const CONFIG_FILE: &str = "config.yaml";
const AUTH_FILE: &str = "auth.json";
const ENV_FILE: &str = ".env";
const ANTHROPIC_OAUTH_FILE: &str = ".anthropic_oauth.json";

/// Credential-pool sources Hermes writes for a sign-in or key the user made in
/// Hermes (mirrors Hermes's own `_EXPLICIT_POOL_SOURCES`, plus config rows).
const EXPLICIT_SOURCES: &[&str] = &[
    "manual",
    "device_code",
    "loopback_pkce",
    "hermes_pkce",
    "oauth",
    "model_config",
];

/// Description suffix Hermes puts on the row of the model it runs by default.
const CURRENT_MARKER: &str = "• current";
const PROVIDER_LABEL_PREFIX: &str = "Provider:";

/// Shown on a saved model Hermes no longer offers.
pub(crate) const UNAVAILABLE_MODEL_DESCRIPTION: &str =
    "Not available in Hermes — the agent runs the Hermes default";

/// Shown on a saved model Hermes still offers, but only through a credential
/// it borrowed from outside Hermes (the agent would run on it).
pub(crate) const NOT_SET_UP_MODEL_DESCRIPTION: &str =
    "Its provider is not set up in Hermes — pick another model";

/// The parts of `config.yaml` Kuzz reads. Every other field (custom endpoint
/// keys included) is skipped by the parser.
#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    model: Option<serde_yaml::Value>,
}

/// The parts of `auth.json` Kuzz reads: provider ids, the active provider and
/// each credential's `source`. Token and key fields are skipped by the parser.
#[derive(Debug, Default, Deserialize)]
struct AuthFile {
    #[serde(default)]
    active_provider: Option<String>,
    #[serde(default)]
    providers: BTreeMap<String, IgnoredAny>,
    #[serde(default)]
    credential_pool: BTreeMap<String, PoolSources>,
}

/// The `source` of each row of one provider's credential pool (a list of rows,
/// or a single row).
#[derive(Debug, Default)]
struct PoolSources(Vec<String>);

#[derive(Deserialize)]
struct PoolRow {
    #[serde(default)]
    source: Option<String>,
}

impl<'de> Deserialize<'de> for PoolSources {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourcesVisitor;
        impl<'de> Visitor<'de> for SourcesVisitor {
            type Value = PoolSources;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a credential row or a list of rows")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<PoolSources, A::Error> {
                let mut sources = Vec::new();
                while let Some(row) = seq.next_element::<PoolRow>()? {
                    sources.extend(row.source);
                }
                Ok(PoolSources(sources))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PoolSources, A::Error> {
                let mut sources = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "source" {
                        sources.extend(map.next_value::<Option<String>>()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(PoolSources(sources))
            }
            fn visit_unit<E>(self) -> Result<PoolSources, E> {
                Ok(PoolSources::default())
            }
        }
        deserializer.deserialize_any(SourcesVisitor)
    }
}

/// What Kuzz read from one Hermes home.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct HermesSetup {
    /// `config.yaml` `model.provider`.
    pub default_provider: Option<String>,
    /// `config.yaml` `model.default`.
    pub default_model: Option<String>,
    /// Providers set up in Hermes (default/active provider included).
    pub providers: BTreeSet<String>,
    /// Names of API-key variables the user gave Hermes (Hermes `.env` plus the
    /// agent's own Kuzz env vars).
    pub key_names: BTreeSet<String>,
}

/// The Hermes home a model-discovery child reads: the `HERMES_HOME` it is
/// given (an agent's profile), else the Desktop's own, else the native one.
pub(crate) fn discovery_home(child_env: &BTreeMap<String, String>) -> Option<PathBuf> {
    if let Some(home) = child_env
        .get(HERMES_HOME_ENV_VAR)
        .filter(|home| !home.trim().is_empty())
    {
        return Some(PathBuf::from(home));
    }
    std::env::var_os(HERMES_HOME_ENV_VAR)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(native_hermes_home)
}

/// Read the setup of `home`. `None` when neither `config.yaml` nor `auth.json`
/// can be read (Hermes not set up), or when a file exists that Kuzz cannot
/// parse; the caller then leaves Hermes's list unfiltered rather than hide real
/// choices or lose the default provider.
pub(crate) fn read_setup(home: &Path, agent_env_keys: &BTreeSet<String>) -> Option<HermesSetup> {
    let config = read_file::<ConfigFile>(&home.join(CONFIG_FILE), read_yaml).ok()?;
    let root = resolve_hermes_root(Some(home.as_os_str()), None).filter(|root| root != home);
    let own_auth = read_file::<AuthFile>(&home.join(AUTH_FILE), read_json).ok()?;
    let root_auth = match root.as_ref() {
        Some(root) => read_file::<AuthFile>(&root.join(AUTH_FILE), read_json).ok()?,
        None => None,
    };
    if config.is_none() && own_auth.is_none() && root_auth.is_none() {
        return None;
    }

    let mut key_names = env_key_names(home);
    key_names.extend(agent_env_keys.iter().cloned());

    let model = config.as_ref().and_then(|config| config.model.as_ref());
    let yaml_str = |key: &str| {
        model
            .and_then(|model| model.get(key))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let default_provider = yaml_str("provider");
    let default_model = yaml_str("default");

    let mut providers = BTreeSet::new();
    providers.extend(default_provider.clone());
    // Hermes merges auth per provider: a profile's own entries win, every
    // other provider comes from the root.
    let auths = [own_auth.as_ref(), root_auth.as_ref()];
    for auth in auths.into_iter().flatten() {
        if let Some(active) = auth.active_provider.as_deref().map(str::trim) {
            if !active.is_empty() {
                providers.insert(active.to_string());
            }
        }
        providers.extend(auth.providers.keys().cloned());
    }
    let mut pool: BTreeMap<&str, &PoolSources> = BTreeMap::new();
    for auth in [root_auth.as_ref(), own_auth.as_ref()]
        .into_iter()
        .flatten()
    {
        for (provider, sources) in &auth.credential_pool {
            pool.insert(provider.as_str(), sources);
        }
    }
    for (provider, sources) in pool {
        if sources
            .0
            .iter()
            .any(|source| is_set_up_source(source, &key_names))
        {
            providers.insert(provider.to_string());
        }
    }
    let anthropic_login = std::iter::once(home)
        .chain(root.as_deref())
        .any(|dir| dir.join(ANTHROPIC_OAUTH_FILE).is_file());
    if anthropic_login {
        providers.insert("anthropic".to_string());
    }

    Some(HermesSetup {
        default_provider,
        default_model,
        providers,
        key_names,
    })
}

/// Whether a credential-pool `source` is one the user set up in Hermes.
pub(crate) fn is_set_up_source(source: &str, key_names: &BTreeSet<String>) -> bool {
    let normalized = source.trim().to_ascii_lowercase();
    if EXPLICIT_SOURCES.contains(&normalized.as_str())
        || normalized.starts_with("manual:")
        || normalized.starts_with("config:")
    {
        return true;
    }
    source
        .trim()
        .split_once(':')
        .filter(|(kind, _)| kind.eq_ignore_ascii_case("env"))
        .is_some_and(|(_, key)| has_key(key_names, key))
}

fn has_key(key_names: &BTreeSet<String>, key: &str) -> bool {
    key_names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(key.trim()))
}

/// Whether `provider` is backed by an API-key variable the user gave Hermes,
/// for keys Hermes has not yet recorded in its credential pool. Matches the
/// usual `<PROVIDER>_API_KEY` naming (`OPENAI_API_KEY` -> `openai`,
/// `openai-api`), plus Google's two names for Gemini.
fn has_provider_key(provider: &str, key_names: &BTreeSet<String>) -> bool {
    key_names.iter().any(|name| {
        let upper = name.to_ascii_uppercase();
        let Some(stem) = upper.strip_suffix("_API_KEY") else {
            return false;
        };
        let stem = stem.to_ascii_lowercase().replace('_', "-");
        let stem = match stem.as_str() {
            "google" => "gemini".to_string(),
            _ => stem,
        };
        provider == stem || provider == format!("{stem}-api")
    })
}

/// Whether models of `provider` may be offered.
fn provider_is_set_up(provider: &str, setup: &HermesSetup, default: Option<&str>) -> bool {
    Some(provider) == default
        || setup.providers.contains(provider)
        || has_provider_key(provider, &setup.key_names)
}

/// The provider part of a Hermes `provider:model` id: the longest `:`-ended
/// prefix that is a set-up provider (ids like `custom:<name>:<model>` and
/// models like `llama3:8b` carry more than one colon).
fn set_up_provider_of<'a>(
    model_id: &'a str,
    setup: &HermesSetup,
    default: Option<&str>,
) -> Option<&'a str> {
    // User-defined endpoints are `custom:<name>:<model>`; Hermes only lists
    // them from the user's own config, so they are always set up.
    if let Some(rest) = model_id.strip_prefix("custom:") {
        let name_end = rest.find(':')?;
        return Some(&model_id[.."custom:".len() + name_end]);
    }
    model_id
        .match_indices(':')
        .map(|(index, _)| &model_id[..index])
        .filter(|prefix| provider_is_set_up(prefix, setup, default))
        .last()
}

fn provider_label(model: &AgentModelInfo) -> Option<String> {
    let description = model.description.as_deref()?.trim();
    let label = description.strip_prefix(PROVIDER_LABEL_PREFIX)?;
    // `Provider: <label> • <endpoint description> • current`
    let label = label.split('•').next().unwrap_or_default().trim();
    (!label.is_empty()).then(|| label.to_string())
}

/// The provider Hermes runs by default, as it appears in model ids. The
/// configured name may be an alias (`codex`), so it is read off the row Hermes
/// marks as current when it can be: that row's id minus `:<default model>`.
fn default_provider_id(response: &AgentModelsResponse, setup: &HermesSetup) -> Option<String> {
    let current_row = response.models.iter().find(|model| {
        model
            .description
            .as_deref()
            .is_some_and(|description| description.trim_end().ends_with(CURRENT_MARKER))
    });
    let current_id = current_row
        .map(|row| row.id.as_str())
        .or(response.agent_default_model.as_deref());
    if let (Some(id), Some(model)) = (current_id, setup.default_model.as_deref()) {
        if let Some(provider) = id.strip_suffix(&format!(":{model}")) {
            return Some(provider.to_string());
        }
    }
    setup.default_provider.clone()
}

/// Keep only the models of providers set up in Hermes, list those providers
/// (default first), and keep a saved model Hermes no longer offers visible.
pub(crate) fn apply_setup(response: &mut AgentModelsResponse, setup: &HermesSetup) {
    // Without the default provider the "default model" row could land under
    // the wrong provider; leave Hermes's list as it is.
    let Some(default) = default_provider_id(response, setup) else {
        return;
    };
    let default = Some(default.as_str());
    let listed_by_hermes: BTreeSet<String> = response
        .models
        .iter()
        .map(|model| model.id.clone())
        .collect();

    let mut providers: Vec<HarnessProviderInfo> = Vec::new();
    response.models.retain(|model| {
        let Some(provider) = set_up_provider_of(&model.id, setup, default) else {
            return false;
        };
        if !providers.iter().any(|known| known.id == provider) {
            providers.push(HarnessProviderInfo {
                id: provider.to_string(),
                label: provider_label(model).unwrap_or_else(|| provider.to_string()),
                is_default: Some(provider) == default,
            });
        }
        true
    });
    providers.sort_by_key(|provider| !provider.is_default);

    if let Some(saved) = response
        .selected_model
        .as_deref()
        .map(str::trim)
        .filter(|saved| !saved.is_empty())
    {
        if !response.models.iter().any(|model| model.id == saved) {
            response.models.push(AgentModelInfo {
                supported_efforts: None,
                default_effort: None,
                id: saved.to_string(),
                name: None,
                description: Some(
                    if listed_by_hermes.contains(saved) {
                        NOT_SET_UP_MODEL_DESCRIPTION
                    } else {
                        UNAVAILABLE_MODEL_DESCRIPTION
                    }
                    .to_string(),
                ),
            });
        }
    }

    response.supports_switching = !response.models.is_empty();
    response.harness_providers = Some(providers);
}

/// `Ok(None)` when the file does not exist, `Err` when it exists but cannot be
/// read or parsed.
fn read_file<T>(
    path: &Path,
    parse: fn(BufReader<std::fs::File>) -> Result<T, String>,
) -> Result<Option<T>, String> {
    match std::fs::File::open(path) {
        Ok(file) => parse(BufReader::new(file)).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(reader: BufReader<std::fs::File>) -> Result<T, String> {
    serde_json::from_reader(reader).map_err(|error| error.to_string())
}

fn read_yaml<T: for<'de> Deserialize<'de>>(reader: BufReader<std::fs::File>) -> Result<T, String> {
    serde_yaml::from_reader(reader).map_err(|error| error.to_string())
}

/// Names (never values) of the keys assigned a non-empty value in `.env`.
/// Read line by line; each line is dropped as soon as its name is taken.
fn env_key_names(home: &Path) -> BTreeSet<String> {
    let Ok(file) = std::fs::File::open(home.join(ENV_FILE)) else {
        return BTreeSet::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| env_line_key_name(&line))
        .collect()
}

#[cfg(test)]
pub(crate) fn parse_env_key_names(text: &str) -> BTreeSet<String> {
    text.lines().filter_map(env_line_key_name).collect()
}

fn env_line_key_name(line: &str) -> Option<String> {
    let line = line.trim();
    if line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    let is_name = !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    let has_value = !value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .is_empty();
    (is_name && has_value).then(|| key.to_string())
}

#[cfg(test)]
#[path = "hermes_providers_tests.rs"]
mod tests;
