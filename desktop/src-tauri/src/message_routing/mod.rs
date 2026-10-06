//! Smart routing (channel routing mode `desktop-router`): this desktop picks
//! the agents for the user's own channel sends with one cheap model call per
//! batch (the sends of a few seconds, see the composer's batcher), after
//! they post. The model groups the batch, names each group's agents, and
//! relates it to earlier deliveries (`new`, `continue`, `amend`, `cancel`).
//! Each message is delivered with a same-body edit that newly `p`-tags its
//! agents (+ an `auto-route` mention and, for a follow-up, a `buzz:route`
//! note the agent sees). No extra message is posted.
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
use prompt::{
    build_user_prompt, capped_prior, capped_roster, parse_router_reply, ParsedGroup, MAX_BATCH,
    ROUTER_SYSTEM_PROMPT,
};

/// A reply is about 25 tokens a group; this covers a full batch of
/// separate groups and still bounds cost.
pub const ROUTER_MAX_OUTPUT_TOKENS: u32 = 400;

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

/// One routing call: a batch of the owner's sends in one channel.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RouteMessageInput {
    /// The batch, oldest first. The prompt keeps the first
    /// [`prompt::MAX_BATCH`].
    pub messages: Vec<RouterNewMessage>,
    pub roster: Vec<RouterRosterEntry>,
    /// Display names of the humans in the channel, so "messages for HUMANS"
    /// can return no agent.
    #[serde(default)]
    pub humans: Vec<String>,
    pub phase: RoutePhase,
    #[serde(default)]
    pub channel_id: Option<String>,
    /// Recent channel (or thread) messages before the batch, from the
    /// desktop's cache: the frontend keeps the last 3 hours, and the prompt
    /// re-caps count and length.
    #[serde(default)]
    pub recent: Vec<RouterRecentMessage>,
    /// This desktop's deliveries in the channel over the last 30 minutes,
    /// oldest first: what a follow-up may continue, amend, or cancel.
    #[serde(default)]
    pub prior: Vec<RouterPriorDelivery>,
    /// Agents mid-turn right now.
    #[serde(default)]
    pub working: Vec<String>,
}

/// One message of the batch.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RouterNewMessage {
    /// Event id; returned in [`RouteGroup::message_ids`], never prompted.
    pub id: String,
    pub text: String,
    /// The thread root's text when the send is a reply.
    #[serde(default)]
    pub thread_root: Option<String>,
    /// Agents `@mentioned` in the message. Their mention went out without a
    /// `p` tag, so the router judges whether each is the assignee.
    #[serde(default)]
    pub mentioned: Vec<String>,
}

/// One earlier delivery a new message may relate to.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RouterPriorDelivery {
    /// Event id; returned as [`RouteGroup::of`], never prompted.
    pub id: String,
    /// The agents it was delivered to.
    pub agents: Vec<String>,
    pub text: String,
}

/// One earlier message shown to the router as conversation context.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RouterRecentMessage {
    /// Author pubkey; a roster agent is shown by its alias, never this.
    pub pubkey: String,
    /// Written by this desktop's owner.
    #[serde(default)]
    pub is_owner: bool,
    pub content: String,
    pub created_at: u64,
}

/// Why a call produced no decision. Distinct from a group with no agents,
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

/// How a group relates to an earlier delivery.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RouteRelation {
    /// A fresh request.
    New,
    /// More of the same request: supplement, don't redo.
    Continue,
    /// A change or correction to it.
    Amend,
    /// The owner retracts or replaces it.
    Cancel,
}

impl RouteRelation {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "new" => Self::New,
            "continue" => Self::Continue,
            "amend" => Self::Amend,
            "cancel" => Self::Cancel,
            _ => return None,
        })
    }
}

/// Messages of the batch that go to the same agents together.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RouteGroup {
    pub message_ids: Vec<String>,
    /// Empty: no agent acts (small talk, a human, nothing fits).
    pub pubkeys: Vec<String>,
    pub relation: RouteRelation,
    /// The earlier delivery's event id, for any relation but `new`.
    pub of: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum RouteMessageResult {
    /// Every routed message is in exactly one group. Messages past the
    /// batch cap, or every message when the roster is empty, are in none.
    Routed {
        groups: Vec<RouteGroup>,
    },
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

/// Route one batch: cap and alias the roster, call `complete(system,
/// user)` under `deadline` (the route's own, see
/// [`model::RouterProvider::deadline`]), and map the strict reply back to
/// event ids and pubkeys. An empty roster or batch short-circuits to no
/// groups with no call.
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
    let messages = &input.messages[..input.messages.len().min(MAX_BATCH)];
    if roster.is_empty() || messages.is_empty() {
        return RouteOutcome {
            result: RouteMessageResult::Routed { groups: Vec::new() },
            called: false,
            latency_ms: 0,
            est_input_tokens: 0,
            est_output_tokens: 0,
        };
    }
    let prior = capped_prior(&input.prior);
    let user = build_user_prompt(input, &roster);
    let est_input_tokens = estimate_tokens(ROUTER_SYSTEM_PROMPT) + estimate_tokens(&user);
    let started = Instant::now();
    let reply =
        tokio::time::timeout(deadline, complete(ROUTER_SYSTEM_PROMPT.to_string(), user)).await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let skipped = |reason| RouteMessageResult::Skipped { reason };
    let (result, est_output_tokens) = match reply {
        Err(_) => (skipped(RouterSkip::Timeout), 0),
        Ok(Err(error)) => {
            eprintln!("buzz-desktop: smart routing provider error: {error}");
            (skipped(RouterSkip::ProviderError), 0)
        }
        Ok(Ok(text)) => {
            let tokens = estimate_tokens(&text);
            let result = match parse_router_reply(&text, messages.len(), roster.len(), prior.len())
            {
                None => skipped(RouterSkip::BadOutput),
                Some(groups) => RouteMessageResult::Routed {
                    groups: groups
                        .into_iter()
                        .map(|group| resolve_group(group, messages, &roster, &prior))
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

/// Map a parsed group's aliases back to ids. A follow-up the model gave no
/// agents goes to whoever got the earlier delivery: that agent is the one
/// to supplement, fix, or stop.
fn resolve_group(
    group: ParsedGroup,
    messages: &[RouterNewMessage],
    roster: &[RouterRosterEntry],
    prior: &[RouterPriorDelivery],
) -> RouteGroup {
    let earlier = group.of.map(|index| &prior[index]);
    let mut pubkeys: Vec<String> = group
        .targets
        .iter()
        .map(|&index| roster[index].pubkey.clone())
        .collect();
    if pubkeys.is_empty() {
        if let Some(earlier) = earlier {
            pubkeys = earlier.agents.clone();
        }
    }
    RouteGroup {
        message_ids: group
            .messages
            .iter()
            .map(|&index| messages[index].id.clone())
            .collect(),
        pubkeys,
        relation: group.relation,
        of: earlier.map(|delivery| delivery.id.clone()),
    }
}
