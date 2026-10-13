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
        for attempt in 0..2 {
            let account = crate::auth::codex::account_id(&bearer)?;
            let response = tokio::time::timeout(
                cfg.llm_timeout,
                self.http
                    .post(url)
                    .bearer_auth(&bearer)
                    .header("ChatGPT-Account-Id", account)
                    .header("Accept", "text/event-stream")
                    .json(body)
                    .send(),
            )
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
}

impl Events {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            data: Vec::new(),
            bytes: 0,
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
                    Some("response.completed") if event["response"]["status"] == "completed" => {
                        let parsed = parse_responses(event["response"].clone())?;
                        if parsed.text.trim().is_empty() && parsed.tool_calls.is_empty() {
                            return Err(AgentError::Llm("ChatGPT subscription returned no text or tool call; retry the request".into()));
                        }
                        return Ok(Some(event["response"].clone()));
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
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn empty_completion_is_visible_error_and_truncated_tools_are_discarded() {
        let empty = format!(
            "data: {}\n\n",
            json!({"type":"response.completed","response":{"status":"completed","output":[]}})
        );
        assert!(Events::new().push(empty.as_bytes()).is_err());
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
}
