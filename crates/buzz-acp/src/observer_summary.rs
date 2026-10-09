//! Tier-dependent summarising and folding of observer telemetry.
//!
//! Premium telemetry is sent untouched. Free and standard keep the existing
//! `acp_read`/`acp_write` JSON-RPC shapes but trim them to tool names, short
//! argument previews and (standard only) short results, and mark every
//! summarised event with `"detail": "free" | "standard"`. Lifecycle events
//! (turn boundaries, usage, mode, permission requests, ...) are never changed.
//!
//! When a backlog cannot be sent in time, the oldest completed tool calls and
//! text chunks fold into one `observer_gap` event per turn. A tool folds only
//! as a completed pair — start and terminal update both still queued — so a
//! viewer never loses the start line of a running tool, and never sees the
//! completion of a tool whose start it already showed vanish.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::observer::ObserverEvent;
use crate::observer_policy::ObserverTier;

/// Observer event kind replacing folded events.
pub(crate) const OBSERVER_GAP_KIND: &str = "observer_gap";

const TITLE_CHARS: usize = 80;
const PLAN_ENTRY_CHARS: usize = 80;
const ERROR_CHARS: usize = 200;
const TOP_TOOL_NAMES: usize = 5;
/// Bound on per-message character counters kept for the standard tier.
const MAX_TRACKED_MESSAGES: usize = 4_096;

/// Character budgets of one summarised tier.
#[derive(Debug, Clone, Copy)]
struct SummaryBudget {
    /// Tool argument preview and prompt text.
    args_chars: usize,
    /// Tool result text; `None` drops results.
    result_chars: Option<usize>,
    /// Cumulative assistant text per message; `None` drops assistant text.
    message_chars: Option<usize>,
}

impl SummaryBudget {
    fn for_tier(tier: ObserverTier) -> Option<Self> {
        match tier {
            ObserverTier::Free => Some(Self {
                args_chars: 40,
                result_chars: None,
                message_chars: None,
            }),
            ObserverTier::Standard => Some(Self {
                args_chars: 200,
                result_chars: Some(200),
                message_chars: Some(200),
            }),
            ObserverTier::Premium => None,
        }
    }
}

/// Truncate to at most `max` chars on a char boundary, appending `…` when
/// anything was cut.
pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        None => text.to_string(),
        Some((cut, _)) => format!("{}…", &text[..cut]),
    }
}

/// Key of one assistant message for the standard tier's cumulative cap.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MessageKey {
    channel_id: Option<String>,
    agent_index: Option<usize>,
    turn_id: Option<String>,
    message_id: Option<String>,
}

/// Applies the summary rules of the current tier to observer events.
#[derive(Default)]
pub(crate) struct ObserverSummarizer {
    /// Assistant characters already emitted per message (standard tier).
    message_chars: HashMap<MessageKey, usize>,
}

impl ObserverSummarizer {
    /// Summarise `event` for `tier`; `None` when the tier does not send it.
    pub(crate) fn apply(
        &mut self,
        tier: ObserverTier,
        mut event: ObserverEvent,
    ) -> Option<ObserverEvent> {
        if matches!(event.kind.as_str(), "turn_completed" | "turn_error") {
            self.forget_turn(event.turn_id.as_deref());
        }
        let Some(budget) = SummaryBudget::for_tier(tier) else {
            return Some(event);
        };
        match event.kind.as_str() {
            "acp_read" | "acp_write" => {
                let payload = std::mem::take(&mut event.payload);
                event.payload = self.summarise_rpc(&event, payload, budget)?;
            }
            "acp_parse_error" => {
                summarise_parse_error(&mut event.payload, budget);
            }
            _ => return Some(event),
        }
        event.detail = tier.detail().map(str::to_string);
        Some(event)
    }

    fn forget_turn(&mut self, turn_id: Option<&str>) {
        if let Some(turn_id) = turn_id {
            self.message_chars
                .retain(|key, _| key.turn_id.as_deref() != Some(turn_id));
        }
    }

    fn summarise_rpc(
        &mut self,
        event: &ObserverEvent,
        payload: Value,
        budget: SummaryBudget,
    ) -> Option<Value> {
        let Value::Object(mut rpc) = payload else {
            return Some(payload);
        };
        let method = rpc
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut out = Map::new();
        for key in ["jsonrpc", "id", "method"] {
            if let Some(value) = rpc.remove(key) {
                out.insert(key.to_string(), value);
            }
        }
        match method.as_deref() {
            Some("session/update") => {
                let mut params = match rpc.remove("params") {
                    Some(Value::Object(params)) => params,
                    _ => return Some(Value::Object(out)),
                };
                let update = params.remove("update").unwrap_or(Value::Null);
                let update = self.summarise_update(event, update, budget)?;
                let mut summarised = Map::new();
                if let Some(session_id) = params.remove("sessionId") {
                    summarised.insert("sessionId".into(), session_id);
                }
                summarised.insert("update".into(), update);
                out.insert("params".into(), Value::Object(summarised));
            }
            // Permission requests are lifecycle: always sent whole.
            Some("session/request_permission") => {
                out.extend(rpc);
            }
            Some("session/prompt") | Some("_goose/unstable/session/steer") => {
                if let Some(Value::Object(mut params)) = rpc.remove("params") {
                    let mut summarised = Map::new();
                    if let Some(session_id) = params.remove("sessionId") {
                        summarised.insert("sessionId".into(), session_id);
                    }
                    if let Some(Value::Array(blocks)) = params.remove("prompt") {
                        summarised.insert(
                            "prompt".into(),
                            truncate_text_blocks(blocks, budget.args_chars),
                        );
                    }
                    out.insert("params".into(), Value::Object(summarised));
                }
            }
            // Other requests and notifications keep only their envelope.
            Some(_) => {}
            // Responses keep only their outcome.
            None => {
                if let Some(Value::Object(result)) = rpc.remove("result") {
                    let status: Map<String, Value> = result
                        .into_iter()
                        .filter(|(key, _)| key == "outcome" || key == "stopReason")
                        .collect();
                    out.insert("result".into(), Value::Object(status));
                }
                if let Some(Value::Object(error)) = rpc.remove("error") {
                    let mut summarised = Map::new();
                    if let Some(code) = error.get("code") {
                        summarised.insert("code".into(), code.clone());
                    }
                    if let Some(message) = error.get("message").and_then(Value::as_str) {
                        summarised.insert(
                            "message".into(),
                            truncate_chars(message, ERROR_CHARS).into(),
                        );
                    }
                    out.insert("error".into(), Value::Object(summarised));
                }
            }
        }
        Some(Value::Object(out))
    }

    fn summarise_update(
        &mut self,
        event: &ObserverEvent,
        update: Value,
        budget: SummaryBudget,
    ) -> Option<Value> {
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match kind.as_str() {
            "tool_call" | "tool_call_update" => Some(summarise_tool(&update, budget)),
            "agent_thought_chunk" => None,
            "agent_message_chunk" => self.summarise_message_chunk(event, update, budget),
            "user_message_chunk" => Some(truncate_chunk(update, budget.args_chars)),
            "plan" => Some(summarise_plan(update)),
            _ => Some(update),
        }
    }

    fn summarise_message_chunk(
        &mut self,
        event: &ObserverEvent,
        mut update: Value,
        budget: SummaryBudget,
    ) -> Option<Value> {
        let cap = budget.message_chars?;
        let text = update.pointer("/content/text").and_then(Value::as_str)?;
        let key = MessageKey {
            channel_id: event.channel_id.clone(),
            agent_index: event.agent_index,
            turn_id: event.turn_id.clone(),
            message_id: update
                .get("messageId")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        if self.message_chars.len() >= MAX_TRACKED_MESSAGES
            && !self.message_chars.contains_key(&key)
        {
            self.message_chars.clear();
        }
        let used = self.message_chars.entry(key).or_default();
        let remaining = cap.saturating_sub(*used);
        if remaining == 0 {
            return None;
        }
        let chars = text.chars().count();
        let text = if chars > remaining {
            *used = cap;
            truncate_chars(text, remaining)
        } else {
            *used += chars;
            text.to_string()
        };
        update["content"] = json!({ "type": "text", "text": text });
        Some(update)
    }
}

/// Keep a tool update's identity, an argument preview, and (standard) a short
/// result; everything else (raw output, diffs, locations) is dropped.
fn summarise_tool(update: &Value, budget: SummaryBudget) -> Value {
    let mut out = Map::new();
    for key in ["sessionUpdate", "toolCallId", "status", "kind"] {
        if let Some(value) = update.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    for key in ["title", "toolName", "tool_name", "name"] {
        if let Some(value) = update.get(key).and_then(Value::as_str) {
            out.insert(key.into(), truncate_chars(value, TITLE_CHARS).into());
        }
    }
    let args = ["rawInput", "input", "arguments", "args"]
        .into_iter()
        .find_map(|key| update.get(key).filter(|value| !value.is_null()));
    if let Some(args) = args {
        let compact = serde_json::to_string(args).unwrap_or_default();
        let mut preview = Map::new();
        preview.insert(
            "preview".into(),
            truncate_chars(&compact, budget.args_chars).into(),
        );
        // Name-like argument fields identify MCP tools in the viewers.
        for key in ["toolName", "tool_name", "name"] {
            if let Some(value) = args.get(key).and_then(Value::as_str) {
                preview.insert(key.into(), truncate_chars(value, TITLE_CHARS).into());
            }
        }
        out.insert("rawInput".into(), Value::Object(preview));
    }
    if let Some(max) = budget.result_chars {
        let text = tool_result_text(update);
        if !text.is_empty() {
            out.insert(
                "content".into(),
                json!([{
                    "type": "content",
                    "content": { "type": "text", "text": truncate_chars(&text, max) },
                }]),
            );
        }
    }
    Value::Object(out)
}

/// Result text of a tool update: its text content blocks, else `rawOutput`.
fn tool_result_text(update: &Value) -> String {
    let mut text = String::new();
    if let Some(blocks) = update.get("content").and_then(Value::as_array) {
        for block in blocks {
            let inner = block.get("content").unwrap_or(block);
            if let Some(part) = inner.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(part);
            }
        }
    }
    if text.is_empty() {
        match update.get("rawOutput") {
            Some(Value::String(output)) => text.push_str(output),
            Some(Value::Null) | None => {}
            Some(output) => text = serde_json::to_string(output).unwrap_or_default(),
        }
    }
    text
}

fn summarise_plan(mut update: Value) -> Value {
    if let Some(entries) = update.get_mut("entries").and_then(Value::as_array_mut) {
        for entry in entries.iter_mut() {
            let mut out = Map::new();
            for key in ["content", "title"] {
                if let Some(text) = entry.get(key).and_then(Value::as_str) {
                    out.insert(key.into(), truncate_chars(text, PLAN_ENTRY_CHARS).into());
                }
            }
            for key in ["status", "priority"] {
                if let Some(text) = entry.get(key).and_then(Value::as_str) {
                    out.insert(key.into(), truncate_chars(text, 20).into());
                }
            }
            *entry = Value::Object(out);
        }
    }
    update
}

fn truncate_chunk(mut update: Value, max: usize) -> Value {
    if let Some(text) = update.pointer("/content/text").and_then(Value::as_str) {
        let text = truncate_chars(text, max);
        update["content"] = json!({ "type": "text", "text": text });
    }
    update
}

/// Keep prompt text blocks, truncated to `max` chars in total; drop the rest.
fn truncate_text_blocks(blocks: Vec<Value>, max: usize) -> Value {
    let mut remaining = max;
    let mut out = Vec::new();
    for block in blocks {
        if remaining == 0 {
            break;
        }
        let Some(text) = block.get("text").and_then(Value::as_str) else {
            continue;
        };
        let chars = text.chars().count();
        let text = truncate_chars(text, remaining);
        remaining = remaining.saturating_sub(chars);
        out.push(json!({ "type": "text", "text": text }));
    }
    Value::Array(out)
}

fn summarise_parse_error(payload: &mut Value, budget: SummaryBudget) {
    for (key, max) in [("line", budget.args_chars), ("error", ERROR_CHARS)] {
        if let Some(text) = payload.get(key).and_then(Value::as_str) {
            payload[key] = truncate_chars(text, max).into();
        }
    }
}

/// Tool call identity of an `acp_read` `session/update` tool event.
struct ToolEvent<'a> {
    id: &'a str,
    start: bool,
    terminal: bool,
}

fn session_update(event: &ObserverEvent) -> Option<&Value> {
    if event.kind != "acp_read"
        || event.payload.get("method").and_then(Value::as_str) != Some("session/update")
    {
        return None;
    }
    event.payload.pointer("/params/update")
}

fn tool_event(event: &ObserverEvent) -> Option<ToolEvent<'_>> {
    let update = session_update(event)?;
    let kind = update.get("sessionUpdate")?.as_str()?;
    if kind != "tool_call" && kind != "tool_call_update" {
        return None;
    }
    let status = update.get("status").and_then(Value::as_str);
    Some(ToolEvent {
        id: update.get("toolCallId")?.as_str()?,
        start: kind == "tool_call",
        terminal: matches!(status, Some("completed" | "failed")),
    })
}

fn is_text_chunk(event: &ObserverEvent) -> bool {
    session_update(event)
        .and_then(|update| update.get("sessionUpdate"))
        .and_then(Value::as_str)
        .is_some_and(|kind| matches!(kind, "agent_message_chunk" | "agent_thought_chunk"))
}

/// Foldable units among `events` (one channel's backlog, oldest first), each
/// a sorted list of indices, ordered oldest first. A unit is a text chunk, or
/// every queued event of a tool whose start AND terminal update are both
/// queued. Everything else — running tools, updates of tools whose start was
/// already sent, lifecycle events — is never folded.
pub(crate) fn foldable_units(events: &[&ObserverEvent]) -> Vec<Vec<usize>> {
    #[derive(Default)]
    struct Tool {
        start: bool,
        terminal: bool,
        members: Vec<usize>,
    }
    let mut units = Vec::new();
    let mut tools: HashMap<(Option<&str>, &str), Tool> = HashMap::new();
    for (index, event) in events.iter().enumerate() {
        if let Some(tool) = tool_event(event) {
            let entry = tools
                .entry((event.session_id.as_deref(), tool.id))
                .or_default();
            entry.start |= tool.start;
            entry.terminal |= tool.terminal;
            entry.members.push(index);
        } else if is_text_chunk(event) {
            units.push(vec![index]);
        }
    }
    units.extend(
        tools
            .into_values()
            .filter(|tool| tool.start && tool.terminal)
            .map(|tool| tool.members),
    );
    units.sort_by_key(|unit| unit[0]);
    units
}

fn tool_name(event: &ObserverEvent) -> Option<String> {
    let update = session_update(event)?;
    let meta_name = update
        .get("_meta")
        .and_then(Value::as_object)
        .and_then(|meta| {
            meta.values()
                .find_map(|value| value.get("toolName").and_then(Value::as_str))
        });
    ["toolName", "tool_name", "name"]
        .into_iter()
        .find_map(|key| update.get(key).and_then(Value::as_str))
        .or(meta_name)
        .or_else(|| update.get("title").and_then(Value::as_str))
        .or_else(|| update.get("kind").and_then(Value::as_str))
        .map(|name| truncate_chars(name, TITLE_CHARS))
}

/// Build the `observer_gap` event standing in for `folded` (oldest first),
/// each paired with the number of source events it represents.
pub(crate) fn gap_event(folded: &[(u64, &ObserverEvent)], detail: Option<&str>) -> ObserverEvent {
    let first = folded.first().map(|(_, event)| *event);
    let folded_events: u64 = folded.iter().map(|(count, _)| count).sum();
    let mut names: Vec<(String, usize, usize)> = Vec::new();
    let mut folded_tools = 0u64;
    for (_, event) in folded {
        if !tool_event(event).is_some_and(|tool| tool.start) {
            continue;
        }
        folded_tools += 1;
        let name = tool_name(event).unwrap_or_else(|| "tool".to_string());
        match names.iter_mut().find(|(known, _, _)| *known == name) {
            Some(entry) => entry.1 += 1,
            None => {
                let order = names.len();
                names.push((name, 1, order));
            }
        }
    }
    names.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    let tool_names: Vec<String> = names
        .into_iter()
        .take(TOP_TOOL_NAMES)
        .map(|(name, _, _)| name)
        .collect();
    let from_seq = folded.iter().map(|(_, event)| event.seq).min().unwrap_or(0);
    let to_seq = folded.iter().map(|(_, event)| event.seq).max().unwrap_or(0);
    ObserverEvent {
        seq: from_seq,
        timestamp: first
            .map(|event| event.timestamp.clone())
            .unwrap_or_default(),
        kind: OBSERVER_GAP_KIND.to_string(),
        agent_index: first.and_then(|event| event.agent_index),
        channel_id: first.and_then(|event| event.channel_id.clone()),
        session_id: first.and_then(|event| event.session_id.clone()),
        turn_id: first.and_then(|event| event.turn_id.clone()),
        started_at: first.and_then(|event| event.started_at.clone()),
        detail: detail.map(str::to_string),
        payload: json!({
            "folded_events": folded_events,
            "folded_tools": folded_tools,
            "tool_names": tool_names,
            "from_seq": from_seq,
            "to_seq": to_seq,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(seq: u64, kind: &str, payload: Value) -> ObserverEvent {
        ObserverEvent {
            seq,
            timestamp: format!("2026-10-10T00:00:{:02}Z", seq % 60),
            kind: kind.to_string(),
            agent_index: Some(0),
            channel_id: Some("chan".to_string()),
            session_id: Some("session".to_string()),
            turn_id: Some("turn".to_string()),
            started_at: None,
            detail: None,
            payload,
        }
    }

    fn update(seq: u64, update: Value) -> ObserverEvent {
        event(
            seq,
            "acp_read",
            json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": { "sessionId": "session", "update": update, "_meta": { "x": 1 } },
            }),
        )
    }

    fn big_tool_call() -> Value {
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call-1",
            "status": "pending",
            "kind": "execute",
            "title": "t".repeat(120),
            "rawInput": { "command": "x".repeat(20_000) },
            "content": [{ "type": "content", "content": { "type": "text", "text": "y".repeat(5_000) } }],
            "locations": [{ "path": "/tmp/a" }],
        })
    }

    fn tool_result(text: &str) -> Value {
        json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call-1",
            "status": "completed",
            "content": [{ "type": "content", "content": { "type": "text", "text": text } }],
            "rawOutput": { "stdout": text },
        })
    }

    fn summarise(tier: ObserverTier, event: ObserverEvent) -> Option<ObserverEvent> {
        ObserverSummarizer::default().apply(tier, event)
    }

    #[test]
    fn truncation_is_char_safe_and_marks_cuts() {
        assert_eq!(truncate_chars("hello", 5), "hello");
        assert_eq!(truncate_chars("hello", 4), "hell…");
        assert_eq!(truncate_chars("한국어텍스트", 3), "한국어…");
        assert_eq!(truncate_chars("a😀b", 2), "a😀…");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn premium_is_untouched_and_carries_no_detail() {
        let original = update(1, big_tool_call());
        let before = serde_json::to_string(&original).unwrap();
        let after = summarise(ObserverTier::Premium, original).unwrap();
        assert_eq!(serde_json::to_string(&after).unwrap(), before);
        assert!(!before.contains("\"detail\""));
        let thought = update(
            2,
            json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hm" } }),
        );
        assert!(summarise(ObserverTier::Premium, thought).is_some());
    }

    #[test]
    fn free_tool_call_keeps_identity_and_a_40_char_preview() {
        let summarised = summarise(ObserverTier::Free, update(1, big_tool_call())).unwrap();
        assert_eq!(summarised.detail.as_deref(), Some("free"));
        let payload = &summarised.payload;
        assert_eq!(payload["jsonrpc"], "2.0");
        assert_eq!(payload["method"], "session/update");
        assert_eq!(payload["params"]["sessionId"], "session");
        assert!(payload["params"].get("_meta").is_none());
        let update = &payload["params"]["update"];
        assert_eq!(update["sessionUpdate"], "tool_call");
        assert_eq!(update["toolCallId"], "call-1");
        assert_eq!(update["status"], "pending");
        assert_eq!(update["kind"], "execute");
        assert_eq!(update["title"].as_str().unwrap().chars().count(), 81);
        let preview = update["rawInput"]["preview"].as_str().unwrap();
        assert!(preview.starts_with("{\"command\":\"xxx"));
        assert_eq!(preview.chars().count(), 41, "40 chars + ellipsis");
        for dropped in ["content", "rawOutput", "locations"] {
            assert!(update.get(dropped).is_none(), "{dropped} dropped on free");
        }
        let json = serde_json::to_string(&summarised).unwrap();
        assert!(json.contains("\"detail\":\"free\""));
        assert!(json.len() < 600, "summary line is small: {}", json.len());
    }

    #[test]
    fn standard_tool_update_keeps_a_200_char_result() {
        let mut summarizer = ObserverSummarizer::default();
        let start = summarizer
            .apply(ObserverTier::Standard, update(1, big_tool_call()))
            .unwrap();
        let preview = start.payload["params"]["update"]["rawInput"]["preview"]
            .as_str()
            .unwrap();
        assert_eq!(preview.chars().count(), 201);

        let done = summarizer
            .apply(
                ObserverTier::Standard,
                update(2, tool_result(&"r".repeat(500))),
            )
            .unwrap();
        assert_eq!(done.detail.as_deref(), Some("standard"));
        let update_json = &done.payload["params"]["update"];
        assert_eq!(update_json["status"], "completed");
        assert!(update_json.get("rawOutput").is_none());
        assert_eq!(update_json["content"][0]["type"], "content");
        let text = update_json["content"][0]["content"]["text"]
            .as_str()
            .unwrap();
        assert_eq!(text.chars().count(), 201);

        // With no content, the result text comes from rawOutput.
        let raw_only = json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c", "status": "completed",
            "rawOutput": "plain output",
        });
        let done = summarizer
            .apply(ObserverTier::Standard, update(3, raw_only))
            .unwrap();
        assert_eq!(
            done.payload["params"]["update"]["content"][0]["content"]["text"],
            "plain output"
        );

        // Free drops results entirely.
        let free = summarise(ObserverTier::Free, update(4, tool_result("ok"))).unwrap();
        assert!(free.payload["params"]["update"].get("content").is_none());
    }

    #[test]
    fn message_chunks_free_none_standard_capped_per_message() {
        let chunk = |seq, message: &str, text: &str| {
            update(
                seq,
                json!({ "sessionUpdate": "agent_message_chunk", "messageId": message,
                        "content": { "type": "text", "text": text } }),
            )
        };
        assert!(summarise(ObserverTier::Free, chunk(1, "m1", "hi")).is_none());

        let mut summarizer = ObserverSummarizer::default();
        let mut sent = String::new();
        let mut events = 0;
        for seq in 0..5 {
            let chunk = chunk(seq, "m1", &"가".repeat(70));
            if let Some(event) = summarizer.apply(ObserverTier::Standard, chunk) {
                events += 1;
                sent.push_str(
                    event.payload["params"]["update"]["content"]["text"]
                        .as_str()
                        .unwrap(),
                );
            }
        }
        assert_eq!(events, 3, "chunks past the cap are not sent");
        assert_eq!(sent.chars().count(), 201, "200 chars + ellipsis");
        assert!(sent.ends_with('…'));
        // A different message has its own budget.
        assert!(summarizer
            .apply(ObserverTier::Standard, chunk(9, "m2", "next"))
            .is_some());
        // Turn end forgets the turn's counters.
        summarizer.apply(
            ObserverTier::Standard,
            event(10, "turn_completed", json!({})),
        );
        assert!(summarizer.message_chars.is_empty());
    }

    #[test]
    fn thoughts_are_not_sent_on_summarised_tiers() {
        let thought = update(
            1,
            json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hm" } }),
        );
        assert!(summarise(ObserverTier::Free, thought.clone()).is_none());
        assert!(summarise(ObserverTier::Standard, thought).is_none());
    }

    #[test]
    fn plan_entries_keep_short_titles() {
        let plan = update(
            1,
            json!({ "sessionUpdate": "plan", "entries": [
                { "content": "c".repeat(100), "status": "pending", "priority": "high",
                  "extra": "x".repeat(1000) },
            ]}),
        );
        let summarised = summarise(ObserverTier::Free, plan).unwrap();
        let entry = &summarised.payload["params"]["update"]["entries"][0];
        assert_eq!(entry["content"].as_str().unwrap().chars().count(), 81);
        assert_eq!(entry["status"], "pending");
        assert_eq!(entry["priority"], "high");
        assert!(entry.get("extra").is_none());
    }

    #[test]
    fn prompts_and_other_rpc_are_trimmed_to_envelope_and_status() {
        let prompt = event(
            1,
            "acp_write",
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "session/prompt",
                "params": { "sessionId": "s", "prompt": [
                    { "type": "text", "text": "a".repeat(30) },
                    { "type": "resource", "resource": { "text": "z".repeat(1000) } },
                    { "type": "text", "text": "b".repeat(30) },
                ]},
            }),
        );
        let summarised = summarise(ObserverTier::Free, prompt).unwrap();
        let blocks = summarised.payload["params"]["prompt"].as_array().unwrap();
        let total: usize = blocks
            .iter()
            .map(|block| {
                block["text"]
                    .as_str()
                    .unwrap()
                    .trim_end_matches('…')
                    .chars()
                    .count()
            })
            .sum();
        assert_eq!(total, 40);
        assert_eq!(summarised.payload["id"], 7);

        let new_session = event(
            2,
            "acp_write",
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "session/new",
                "params": { "systemPrompt": "p".repeat(10_000) },
            }),
        );
        let summarised = summarise(ObserverTier::Standard, new_session).unwrap();
        assert_eq!(
            summarised.payload,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "session/new" })
        );

        let permission_reply = event(
            3,
            "acp_write",
            json!({
                "jsonrpc": "2.0", "id": 4,
                "result": { "outcome": { "outcome": "selected", "optionId": "allow" },
                            "big": "x".repeat(100) },
            }),
        );
        let summarised = summarise(ObserverTier::Free, permission_reply).unwrap();
        assert_eq!(
            summarised.payload["result"],
            json!({ "outcome": { "outcome": "selected", "optionId": "allow" } })
        );

        let prompt_done = event(
            4,
            "acp_read",
            json!({
                "jsonrpc": "2.0", "id": 7,
                "result": { "stopReason": "end_turn", "_meta": { "x": 1 } },
            }),
        );
        let summarised = summarise(ObserverTier::Free, prompt_done).unwrap();
        assert_eq!(
            summarised.payload["result"],
            json!({ "stopReason": "end_turn" })
        );
    }

    #[test]
    fn lifecycle_and_permission_requests_are_unchanged_on_every_tier() {
        let permission = event(
            1,
            "acp_read",
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "session/request_permission",
                "params": { "toolCall": { "rawInput": { "command": "rm -rf /tmp/x" } },
                            "options": [] },
            }),
        );
        for tier in [ObserverTier::Free, ObserverTier::Standard] {
            let summarised = summarise(tier, permission.clone()).unwrap();
            assert_eq!(summarised.payload, permission.payload);
            for kind in [
                "turn_started",
                "turn_completed",
                "turn_error",
                "turn_liveness",
                "context_usage",
            ] {
                let lifecycle = event(2, kind, json!({ "big": "x".repeat(300) }));
                let summarised = summarise(tier, lifecycle.clone()).unwrap();
                assert_eq!(summarised.payload, lifecycle.payload);
                assert!(summarised.detail.is_none(), "{kind} is not summarised");
            }
            let usage = update(
                3,
                json!({ "sessionUpdate": "usage_update", "used": 5, "size": 10 }),
            );
            let summarised = summarise(tier, usage).unwrap();
            assert_eq!(summarised.payload["params"]["update"]["used"], 5);
        }
    }

    fn tool(seq: u64, id: &str, start: bool, status: &str) -> ObserverEvent {
        let kind = if start {
            "tool_call"
        } else {
            "tool_call_update"
        };
        update(
            seq,
            json!({
                "sessionUpdate": kind, "toolCallId": id, "status": status,
                "title": format!("tool {id}"),
            }),
        )
    }

    #[test]
    fn only_completed_pairs_and_text_chunks_fold() {
        let events = [
            event(1, "turn_started", json!({})),
            tool(2, "done", true, "pending"),
            tool(3, "running", true, "in_progress"),
            tool(4, "sent-earlier", false, "completed"),
            update(
                5,
                json!({ "sessionUpdate": "agent_message_chunk",
                        "content": { "type": "text", "text": "x" } }),
            ),
            tool(6, "done", false, "in_progress"),
            tool(7, "done", false, "failed"),
            tool(8, "running", false, "in_progress"),
            event(9, "turn_completed", json!({})),
        ];
        let refs: Vec<&ObserverEvent> = events.iter().collect();
        // The pair (start, progress, failure) and the text chunk fold; the
        // running tool, the completion whose start was sent, and the turn
        // boundaries never do.
        assert_eq!(foldable_units(&refs), vec![vec![1, 5, 6], vec![4]]);
    }

    #[test]
    fn gap_counts_tools_and_ranks_top_five_names() {
        let mut events = Vec::new();
        let mut seq = 10;
        for (name, count) in [("a", 1), ("b", 3), ("c", 2), ("d", 1), ("e", 1), ("f", 4)] {
            for i in 0..count {
                let id = format!("{name}{i}");
                let mut start = tool(seq, &id, true, "pending");
                start.payload["params"]["update"]["title"] = json!(name);
                events.push(start);
                events.push(tool(seq + 1, &id, false, "completed"));
                seq += 2;
            }
        }
        let folded: Vec<(u64, &ObserverEvent)> = events.iter().map(|e| (1, e)).collect();
        let gap = gap_event(&folded, Some("free"));
        assert_eq!(gap.kind, OBSERVER_GAP_KIND);
        assert_eq!(gap.detail.as_deref(), Some("free"));
        assert_eq!(gap.turn_id.as_deref(), Some("turn"));
        assert_eq!(gap.channel_id.as_deref(), Some("chan"));
        assert_eq!(gap.payload["folded_events"], 24);
        assert_eq!(gap.payload["folded_tools"], 12);
        assert_eq!(gap.payload["tool_names"], json!(["f", "b", "c", "a", "d"]));
        assert_eq!(gap.payload["from_seq"], 10);
        assert_eq!(gap.payload["to_seq"], 33);
        assert_eq!(gap.seq, 10);
        let premium_gap = serde_json::to_string(&gap_event(&folded, None)).unwrap();
        assert!(!premium_gap.contains("\"detail\""));
    }
}
