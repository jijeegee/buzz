//! Smart routing (channel routing mode `desktop-router`): this desktop picks
//! the agent for each of the user's own unmentioned channel sends with one
//! cheap model call, and the composer carries the pick as an ordinary
//! agent address (`p` tag + `agent-address` mention). Nothing is posted to
//! the channel and no harness changes are involved.
//!
//! - [`model`] resolves the route: an API key (Providers tab) or a
//!   signed-in Codex / Claude Code CLI (Settings › Models › Task models).
//! - [`cli`] runs one call on a subscription route through the official CLI.
//! - [`prompt`] builds the aliased prompt and strictly parses the reply.
//! - [`log`] appends one JSONL line per call for the routing comparison.
//! - [`route_with_deadline`] runs one call under a hard deadline; the completion is
//!   injected so tests never reach a real provider.

pub mod cli;
pub mod log;
pub mod model;
pub mod prompt;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use model::{ResolvedRouterModel, RouterBackend};
use prompt::{build_user_prompt, capped_roster, parse_router_reply, ROUTER_SYSTEM_PROMPT};

/// A reply is `{"to":["a1","a2"]}` — 40 tokens is ample and bounds cost.
pub const ROUTER_MAX_OUTPUT_TOKENS: u32 = 40;

/// One agent the message may be assigned to.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RouterRosterEntry {
    pub pubkey: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// When in the composer's life the call happened, for the comparison log.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RoutePhase {
    /// Debounced while typing; its pick shows as a chip.
    Preview,
    /// On Enter, when no preview result matched the final text.
    Send,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RouteMessageInput {
    pub message: String,
    /// The thread root's text when the send is a reply.
    #[serde(default)]
    pub thread_root: Option<String>,
    pub roster: Vec<RouterRosterEntry>,
    /// Display names of the humans in the channel, so "messages for HUMANS"
    /// can return no agent.
    #[serde(default)]
    pub humans: Vec<String>,
    pub phase: RoutePhase,
    #[serde(default)]
    pub channel_id: Option<String>,
}

/// Why a call produced no decision. Distinct from [`RouteMessageResult::NoFit`]
/// so the composer can tell "the model said nobody" from "routing failed".
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RouterSkip {
    /// Smart routing is not the saved mode, or no API key is configured.
    NotConfigured,
    Timeout,
    ProviderError,
    BadOutput,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum RouteMessageResult {
    Assigned {
        pubkeys: Vec<String>,
    },
    /// The model chose nobody (small talk, a human, no fitting agent).
    #[serde(rename = "none")]
    NoFit,
    Skipped {
        reason: RouterSkip,
    },
}

/// One finished call plus what the comparison log records about it.
#[derive(Debug, Clone)]
pub struct RouteOutcome {
    pub result: RouteMessageResult,
    /// False when nothing was sent to the model (empty roster).
    pub called: bool,
    pub latency_ms: u64,
    pub est_input_tokens: u64,
    pub est_output_tokens: u64,
}

/// ~4 characters per token: an estimate, labelled as such in the log.
fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// Build the `buzz-agent` config for one API-key routing call; `None` for a
/// CLI route.
pub fn router_agent_config(
    resolved: &ResolvedRouterModel,
) -> Option<buzz_agent_pkg::config::Config> {
    let RouterBackend::Api { api_key, base_url } = &resolved.backend else {
        return None;
    };
    let mut cfg = buzz_agent_pkg::config::Config::for_discovery(
        resolved.provider.agent_provider()?,
        api_key.clone(),
        base_url.clone(),
        None,
    );
    cfg.llm_timeout = resolved.provider.deadline();
    Some(cfg)
}

/// Route one message: cap and alias the roster, call `complete(system,
/// user)` under `deadline` (the route's own, see
/// [`model::RouterProvider::deadline`]), and map the strict reply back to
/// pubkeys. An empty roster short-circuits to `NoFit` with no call.
pub async fn route_with_deadline<F, Fut>(
    input: &RouteMessageInput,
    deadline: Duration,
    complete: F,
) -> RouteOutcome
where
    F: FnOnce(String, String) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let roster = capped_roster(&input.roster);
    if roster.is_empty() {
        return RouteOutcome {
            result: RouteMessageResult::NoFit,
            called: false,
            latency_ms: 0,
            est_input_tokens: 0,
            est_output_tokens: 0,
        };
    }
    let user = build_user_prompt(input, &roster);
    let est_input_tokens = estimate_tokens(ROUTER_SYSTEM_PROMPT) + estimate_tokens(&user);
    let started = Instant::now();
    let reply =
        tokio::time::timeout(deadline, complete(ROUTER_SYSTEM_PROMPT.to_string(), user)).await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (result, est_output_tokens) = match reply {
        Err(_) => (
            RouteMessageResult::Skipped {
                reason: RouterSkip::Timeout,
            },
            0,
        ),
        Ok(Err(error)) => {
            eprintln!("buzz-desktop: smart routing provider error: {error}");
            (
                RouteMessageResult::Skipped {
                    reason: RouterSkip::ProviderError,
                },
                0,
            )
        }
        Ok(Ok(text)) => {
            let tokens = estimate_tokens(&text);
            let result = match parse_router_reply(&text, roster.len()) {
                None => RouteMessageResult::Skipped {
                    reason: RouterSkip::BadOutput,
                },
                Some(indexes) if indexes.is_empty() => RouteMessageResult::NoFit,
                Some(indexes) => RouteMessageResult::Assigned {
                    pubkeys: indexes
                        .into_iter()
                        .map(|index| roster[index].pubkey.clone())
                        .collect(),
                },
            };
            (result, tokens)
        }
    };
    RouteOutcome {
        result,
        called: true,
        latency_ms,
        est_input_tokens,
        est_output_tokens,
    }
}
