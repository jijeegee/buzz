//! Smart routing comparison log: `<app-data>/agents/routing-log/desktop.jsonl`,
//! one line per routing call (one batch) while Smart routing is the saved
//! mode.
//!
//! The log is diagnostic: a failed append is reported on stderr and never
//! fails or delays the routing result. It stays local to this machine and is
//! bounded — past [`MAX_LOG_BYTES`] the file rotates to `desktop.jsonl.1`,
//! replacing the previous rotation.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::model::model_price_per_mtok;
use super::{RouteMessageResult, RouteOutcome, RoutePhase, RouterNewMessage};

const LOG_DIR: &str = "routing-log";
const LOG_FILE: &str = "desktop.jsonl";
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const EXCERPT_CHARS: usize = 80;

#[derive(Serialize, Debug, PartialEq)]
pub struct RoutingLogLine {
    pub ts: String,
    pub mode: &'static str,
    pub phase: RoutePhase,
    pub channel: Option<String>,
    pub decision: &'static str,
    /// Messages in the batch and the groups the model made of them.
    pub messages: usize,
    pub groups: usize,
    pub targets: Vec<String>,
    pub reason: Option<super::RouterSkip>,
    pub latency_ms: u64,
    pub model: String,
    pub est_input_tokens: u64,
    pub est_output_tokens: u64,
    pub est_cost_usd: Option<f64>,
    pub excerpt: String,
}

impl RoutingLogLine {
    pub fn new(
        ts: String,
        phase: RoutePhase,
        channel: Option<String>,
        model: &str,
        messages: &[RouterNewMessage],
        outcome: &RouteOutcome,
    ) -> Self {
        let (groups, reason) = match &outcome.result {
            RouteMessageResult::Routed { groups } => (groups.as_slice(), None),
            RouteMessageResult::Skipped { reason } => (&[][..], Some(*reason)),
        };
        let mut targets: Vec<String> = Vec::new();
        for pubkey in groups.iter().flat_map(|group| &group.pubkeys) {
            if !targets.contains(pubkey) {
                targets.push(pubkey.clone());
            }
        }
        let decision = match (reason, targets.is_empty()) {
            (Some(_), _) => "skipped",
            (None, true) => "none",
            (None, false) => "assigned",
        };
        let est_cost_usd = model_price_per_mtok(model).map(|(input, output)| {
            (outcome.est_input_tokens as f64 * input + outcome.est_output_tokens as f64 * output)
                / 1_000_000.0
        });
        Self {
            ts,
            mode: "desktop-router",
            phase,
            channel,
            decision,
            messages: messages.len(),
            groups: groups.len(),
            targets,
            reason,
            latency_ms: outcome.latency_ms,
            model: model.to_string(),
            est_input_tokens: outcome.est_input_tokens,
            est_output_tokens: outcome.est_output_tokens,
            est_cost_usd,
            excerpt: messages
                .first()
                .map(|message| message.text.trim().chars().take(EXCERPT_CHARS).collect())
                .unwrap_or_default(),
        }
    }
}

pub fn routing_log_path(agents_dir: &Path) -> PathBuf {
    agents_dir.join(LOG_DIR).join(LOG_FILE)
}

/// Append one line, rotating first when the file is over the cap.
pub fn append_routing_log(path: &Path, line: &RoutingLogLine) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() >= MAX_LOG_BYTES) {
        let rotated = path.with_extension("jsonl.1");
        std::fs::rename(path, &rotated)
            .map_err(|error| format!("rotate {}: {error}", path.display()))?;
    }
    let mut json =
        serde_json::to_string(line).map_err(|error| format!("serialize log line: {error}"))?;
    json.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    file.write_all(json.as_bytes())
        .map_err(|error| format!("append {}: {error}", path.display()))
}
