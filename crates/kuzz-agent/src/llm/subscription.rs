//! Codex subscription Responses transport inside the existing LLM module.
use super::*;

const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

fn subscription_body(body: &Value) -> Value {
    let mut body = body.clone();
    if let Some(fields) = body.as_object_mut() {
        fields.remove("max_output_tokens");
        fields.insert("store".into(), json!(false));
        fields.insert("stream".into(), json!(true));
    }
    // Summary requests use a string on the API-key route. The subscription
    // route requires explicit input items, with the same client-owned history.
    if let Some(text) = body["input"].as_str() {
        body["input"] = json!([{"role":"user","content":text}]);
    }
    body
}

impl Llm {
    pub(super) async fn post_subscription(
        &self,
        cfg: &Config,
        body: &Value,
    ) -> Result<Value, AgentError> {
        // Never use OPENAI_COMPAT_BASE_URL for subscription credentials.
        tokio::time::timeout(
            cfg.llm_timeout,
            self.subscription_request(cfg, &subscription_body(body), RESPONSES_URL),
        )
        .await
        .map_err(|_| AgentError::Llm("ChatGPT subscription request deadline exceeded".into()))?
    }

    async fn subscription_request(
        &self,
        cfg: &Config,
        body: &Value,
        url: &str,
    ) -> Result<Value, AgentError> {
        let mut bearer = self.auth.bearer().await?;
        // The subscription backend routes on the `session_id` header, not the
        // body's `prompt_cache_key`: measured 2026-10-14 on gpt-6-sol, a
        // repeated 4.5k-token prefix hit the cache ~85% of the time with the
        // header and ~15% without it, body key or not. Codex sends the same.
        let session = body["prompt_cache_key"].as_str();
        for attempt in 0..2 {
            let account = crate::auth::codex::account_id(&bearer)?;
            let mut request = self
                .http
                .post(url)
                .bearer_auth(&bearer)
                .header("ChatGPT-Account-Id", account)
                .header("Accept", "text/event-stream");
            if let Some(session) = session {
                request = request.header("session_id", session);
            }
            let response = tokio::time::timeout(cfg.llm_timeout, request.json(body).send())
                .await
                .map_err(|_| AgentError::Llm("ChatGPT subscription request timed out".into()))?
                .map_err(|_| AgentError::Llm("ChatGPT subscription connection failed".into()))?;
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                bearer = self.auth.refresh_now(&bearer).await?;
                continue;
            }
            if !status.is_success() {
                // No automatic API-key fallback or replay on a stream failure.
                let detail = read_error_body(response).await;
                if status.as_u16() == 400 || status.as_u16() == 404 {
                    return Err(classify_failure(&detail, status.as_u16() == 404));
                }
                return Err(match status.as_u16() {
                    401 | 403 => AgentError::LlmAuth(
                        "ChatGPT subscription rejected the request; check sign-in and model access"
                            .into(),
                    ),
                    429 => AgentError::Llm(
                        "ChatGPT subscription usage limit reached; retry later".into(),
                    ),
                    _ => AgentError::Llm(format!("ChatGPT subscription HTTP {}", status.as_u16())),
                });
            }
            return read_stream(response, cfg.llm_timeout).await;
        }
        Err(AgentError::LlmAuth(
            "ChatGPT subscription sign-in required".into(),
        ))
    }
}

struct Events {
    pending: Vec<u8>,
    data: Vec<u8>,
    bytes: usize,
    completed_items: std::collections::BTreeMap<u64, Value>,
}

impl Events {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            data: Vec::new(),
            bytes: 0,
            completed_items: std::collections::BTreeMap::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Result<Option<Value>, AgentError> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_LLM_RESPONSE_BYTES {
            return Err(AgentError::Llm(
                "ChatGPT subscription response too large".into(),
            ));
        }
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = line.strip_suffix(b"\n").unwrap_or(&line);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                if self.data.is_empty() {
                    continue;
                }
                let event: Value = serde_json::from_slice(&self.data)
                    .map_err(|_| AgentError::Llm("invalid ChatGPT subscription event".into()))?;
                self.data.clear();
                match event["type"].as_str() {
                    Some("response.output_item.done") => {
                        if let (Some(index), Some(item)) =
                            (event["output_index"].as_u64(), event.get("item"))
                        {
                            self.completed_items.insert(index, item.clone());
                        }
                    }
                    Some("response.completed") if event["response"]["status"] == "completed" => {
                        let mut response = event["response"].clone();
                        // Some subscription streams omit output from the final
                        // envelope. Retain completed items, but release them only
                        // after a successful terminal event (never on failure).
                        if response["output"].as_array().is_none_or(Vec::is_empty) {
                            response["output"] = Value::Array(
                                std::mem::take(&mut self.completed_items)
                                    .into_values()
                                    .collect(),
                            );
                        }
                        // A successful tool publish may be followed by an empty
                        // end_turn. It is valid silence, not a transport failure
                        // to replay (which could send the user's reply twice).
                        // The common agent loop owns reply reminders.
                        return Ok(Some(response));
                    }
                    Some("response.incomplete")
                        if event["response"]["status"] == "incomplete"
                            && event["response"]["incomplete_details"]["reason"]
                                == "max_output_tokens" =>
                    {
                        // The existing parser discards incomplete tool calls and
                        // returns MaxTokens for the bounded recovery path.
                        return Ok(Some(event["response"].clone()));
                    }
                    Some(
                        "response.failed" | "response.incomplete" | "error" | "response.completed",
                    ) => {
                        return Err(classify_failure(&event.to_string(), false));
                    }
                    _ => {}
                }
            } else if let Some(data) = line.strip_prefix(b"data:") {
                let data = data.strip_prefix(b" ").unwrap_or(data);
                if data == b"[DONE]" {
                    return Err(AgentError::Llm(
                        "ChatGPT subscription stream ended without a completed response".into(),
                    ));
                }
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(data);
            }
        }
        Ok(None)
    }
}

fn classify_failure(detail: &str, model_not_found: bool) -> AgentError {
    // Inspect provider diagnostics without echoing raw prompt/account data.
    if is_context_length_error(detail) {
        AgentError::LlmContextExceeded("ChatGPT subscription context window exceeded".into())
    } else if is_unsupported_image_input_error(detail) {
        AgentError::UnsupportedImageInput(
            "ChatGPT subscription model does not accept this image input".into(),
        )
    } else if super::is_unsupported_effort_error(detail) {
        // Keep only the provider's supported-level list (no prompt/account
        // data) so `Llm::complete` can retry with a level it accepts.
        let supported = super::parse_supported_efforts(detail, None)
            .into_iter()
            .map(ThinkingEffort::openai_effort_str)
            .collect::<Vec<_>>();
        AgentError::LlmUnsupportedEffort(format!(
            "ChatGPT subscription rejected the reasoning effort; supported values: {}",
            supported.join(", ")
        ))
    } else if model_not_found {
        AgentError::LlmModelNotFound("ChatGPT subscription model not found".into())
    } else {
        AgentError::Llm("ChatGPT subscription request failed or did not complete".into())
    }
}

async fn read_stream(
    mut response: reqwest::Response,
    idle_timeout: std::time::Duration,
) -> Result<Value, AgentError> {
    let mut events = Events::new();
    loop {
        let chunk = tokio::time::timeout(idle_timeout, response.chunk())
            .await
            .map_err(|_| AgentError::Llm("ChatGPT subscription stream timed out".into()))?
            .map_err(|_| AgentError::Llm("ChatGPT subscription stream interrupted".into()))?;
        match chunk {
            Some(bytes) => {
                if let Some(response) = events.push(&bytes)? {
                    return Ok(response);
                }
            }
            None => {
                return Err(AgentError::Llm(
                    "ChatGPT subscription stream ended before completion".into(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_effort_event_keeps_only_the_supported_levels() {
        let mut events = Events::new();
        let err = events
            .push(
                br#"data: {"type":"error","error":{"type":"invalid_request_error","code":"unsupported_value","param":"reasoning.effort","message":"Unsupported value: 'minimal' is not supported with this model. Supported values are: 'none', 'low', 'medium', 'high', 'xhigh', and 'max'."}}

"#,
            )
            .unwrap_err();
        let AgentError::LlmUnsupportedEffort(detail) = err else {
            panic!("expected an effort rejection, got {err:?}");
        };
        assert_eq!(
            detail,
            "ChatGPT subscription rejected the reasoning effort; supported values: none, low, medium, high, xhigh, max"
        );
    }

    #[test]
    fn streamed_items_survive_empty_terminal_output_without_duplicate_tools() {
        let text = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello."}]});
        let tool = json!({"type":"function_call","call_id":"call1","name":"echo","arguments":"{}"});
        let mut events = Events::new();
        // Ordering is by output index; repeated item completion is idempotent.
        for (index, item) in [(1, &tool), (0, &text), (1, &tool)] {
            let wire = format!(
                "data: {}\n\n",
                json!({"type":"response.output_item.done","output_index":index,"item":item})
            );
            assert!(events.push(wire.as_bytes()).unwrap().is_none());
        }
        let end = b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n";
        let response = events.push(end).unwrap().unwrap();
        assert_eq!(response["output"], json!([text, tool]));
        let parsed = parse_responses(response).unwrap();
        assert_eq!(parsed.text, "Hello.");
        assert_eq!(parsed.tool_calls.len(), 1);
    }

    #[test]
    fn streamed_items_are_not_released_on_failure_or_added_to_terminal_output() {
        let item = b"data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call1\",\"name\":\"echo\",\"arguments\":\"{}\"}}\n\n";
        for terminal in ["response.failed", "response.incomplete", "error"] {
            let mut events = Events::new();
            assert!(events.push(item).unwrap().is_none());
            let end = format!(
                "data: {}\n\n",
                json!({"type":terminal,"response":{"status":"failed"}})
            );
            assert!(events.push(end.as_bytes()).is_err());
        }
        let mut events = Events::new();
        events.push(item).unwrap();
        let output = json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Final"}]}]);
        let end = format!(
            "data: {}\n\n",
            json!({"type":"response.completed","response":{"status":"completed","output":output}})
        );
        assert_eq!(
            events.push(end.as_bytes()).unwrap().unwrap()["output"],
            output
        );
    }

    #[test]
    fn split_utf8_and_crlf_completion_retains_tool_calls() {
        let expected = json!({"status":"completed","output":[{"type":"function_call","call_id":"c1","name":"echo","arguments":"{\"text\":\"한글\"}"}]});
        let wire = format!(
            "event: response.completed\r\ndata: {}\r\n\r\n",
            json!({"type":"response.completed","response":expected})
        );
        let mut events = Events::new();
        let mut result = None;
        for byte in wire.as_bytes() {
            result = events.push(&[*byte]).unwrap().or(result);
        }
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn failed_or_incomplete_events_never_release_tool_calls() {
        for kind in ["response.failed", "response.incomplete", "error"] {
            let wire = format!(
                "data: {}\n\n",
                json!({"type":kind,"response":{"status":"failed","output":[{"type":"function_call"}]}})
            );
            assert!(Events::new().push(wire.as_bytes()).is_err());
        }
        assert!(Events::new().push(b"data: [DONE]\n\n").is_err());
    }

    #[test]
    fn empty_completed_turn_is_valid_and_truncated_tools_are_discarded() {
        let empty = format!(
            "data: {}\n\n",
            json!({"type":"response.completed","response":{"status":"completed","output":[]}})
        );
        let value = Events::new().push(empty.as_bytes()).unwrap().unwrap();
        let parsed = parse_responses(value).unwrap();
        assert!(parsed.text.is_empty());
        assert!(parsed.tool_calls.is_empty());
        assert_eq!(parsed.stop, ProviderStop::EndTurn);
        let wire = format!(
            "data: {}\n\n",
            json!({"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"function_call","arguments":"{"}]}})
        );
        let value = Events::new().push(wire.as_bytes()).unwrap().unwrap();
        let parsed = parse_responses(value).unwrap();
        assert!(parsed.tool_calls.is_empty());
        assert_eq!(parsed.stop, ProviderStop::MaxTokens);
    }

    #[test]
    fn summary_and_completion_use_subscription_wire_contract() {
        let body =
            subscription_body(&json!({"model":"test","max_output_tokens":100,"input":"summary"}));
        assert_eq!(body["input"][0]["content"], "summary");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn subscription_body_keeps_cache_key_for_session_header() {
        let body =
            subscription_body(&json!({"model":"test","input":[],"prompt_cache_key":"ses_1"}));
        assert_eq!(body["prompt_cache_key"].as_str(), Some("ses_1"));
    }
}
