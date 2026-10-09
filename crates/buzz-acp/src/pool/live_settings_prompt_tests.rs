//! Live text settings through the real `run_prompt_task` seam: an edit to the
//! live settings file reaches each live session exactly once as
//! `<settings-update>`, and a new session takes the current values into its
//! system prompt.

use super::tests::make_prompt_context_no_owner;
use super::*;
use crate::live_settings::{LivePins, LiveSettings, LiveValues};
use nostr::{EventBuilder, Keys, Kind};

/// A POSIX shell for the scripted ACP peer. On Windows a bare `bash` resolves
/// to System32's WSL launcher, which cannot see Windows paths; prefer Git Bash.
fn bash() -> &'static str {
    const GIT_BASH: &str = "C:/Program Files/Git/bin/bash.exe";
    if cfg!(windows) && std::path::Path::new(GIT_BASH).exists() {
        GIT_BASH
    } else {
        "bash"
    }
}

fn batch(channel_id: Uuid, text: &str) -> FlushBatch {
    let event = EventBuilder::new(Kind::Custom(9), text)
        .sign_with_keys(&Keys::generate())
        .unwrap();
    FlushBatch {
        channel_id,
        scope: SessionScope::Conversation { channel_id },
        events: vec![crate::queue::BatchEvent {
            edit: None,
            event,
            prompt_tag: "test".into(),
            received_at: std::time::Instant::now(),
        }],
        cancelled_events: vec![],
        cancel_reason: None,
    }
}

fn write_settings(path: &std::path::Path, prompt: &str, task_threads: &str) {
    std::fs::write(
        path,
        serde_json::json!({
            "system_prompt": prompt,
            "team_instructions": "Ship small.",
            "task_threads": task_threads,
        })
        .to_string(),
    )
    .unwrap();
}

fn live_settings(path: &std::path::Path) -> LiveSettings {
    LiveSettings::file(
        path.to_path_buf(),
        LiveValues::default(),
        LivePins::default(),
    )
}

/// A thread-policy context whose sessions get the task thread rules.
fn thread_policy_context(settings: LiveSettings) -> PromptContext {
    let mut ctx = make_prompt_context_no_owner();
    ctx.goals_enabled = false;
    ctx.base_prompt = Some("BASE".into());
    ctx.task_thread_rules_apply = true;
    ctx.live_settings = settings;
    ctx
}

fn modern_agent(acp: crate::acp::AcpClient) -> OwnedAgent {
    OwnedAgent {
        index: 0,
        acp,
        state: SessionState::default(),
        model_capabilities: None,
        desired_model: None,
        model_overridden: false,
        desired_model_request_id: None,
        desired_model_pending_ack: false,
        startup_effort: None,
        agent_name: "modern-test-agent".into(),
        goose_system_prompt_supported: None,
        protocol_version: 2,
    }
}

fn captured_requests(capture: &std::path::Path) -> Vec<String> {
    let requests = std::fs::read_to_string(capture)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    std::fs::remove_file(capture).unwrap();
    requests
}

fn prompt_text(request: &str) -> String {
    let request: serde_json::Value = serde_json::from_str(request).unwrap();
    request["params"]["prompt"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn settings_edits_reach_each_live_session_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("agent.json");
    write_settings(&file, "You are Eva.", "long_running");

    let capture =
        std::env::temp_dir().join(format!("buzz-acp-live-settings-{}.ndjson", Uuid::new_v4()));
    let quoted_capture = tests::shell_quoted_path(&capture);
    let script = format!(
        r#"count=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{quoted_capture}'
  printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$count,\"result\":{{\"stopReason\":\"end_turn\"}}}}"
  count=$((count + 1))
done"#
    );
    let acp = crate::acp::AcpClient::spawn(bash(), &["-c".to_string(), script], &[], false)
        .await
        .unwrap();
    let mut agent = modern_agent(acp);
    let ctx = thread_policy_context(live_settings(&file));
    // Both sessions started with the settings as first written.
    let started_with = ctx.live_now().text;
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    for channel_id in [first, second] {
        let scope = SessionScope::Conversation { channel_id };
        agent
            .state
            .sessions
            .insert(scope.clone(), format!("session-{channel_id}"));
        agent.state.deliveries.insert(
            scope,
            ChannelDeliveryState {
                layer0_goals_seen: Some(None),
                live_text_seen: Some(started_with.clone()),
                ..Default::default()
            },
        );
    }
    let ctx = Arc::new(ctx);
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();

    let turns = [
        (first, None),
        (first, Some(("You are Eva.", "long_running,delegation"))),
        (first, None),
        (second, None),
    ];
    for (turn, (channel_id, edit)) in turns.into_iter().enumerate() {
        if let Some((prompt, task_threads)) = edit {
            write_settings(&file, prompt, task_threads);
        }
        run_prompt_task(
            agent,
            Some(batch(channel_id, &format!("message {turn}"))),
            None,
            Arc::clone(&ctx),
            result_tx.clone(),
            None,
            format!("turn-{turn}"),
        )
        .await;
        let result = result_rx.recv().await.unwrap();
        assert!(matches!(
            result.outcome,
            PromptOutcome::Ok(StopReason::EndTurn)
        ));
        agent = result.agent;
    }
    agent.acp.shutdown().await;

    let prompts: Vec<String> = captured_requests(&capture)
        .iter()
        .map(|request| prompt_text(request))
        .collect();
    assert_eq!(prompts.len(), 4);

    assert!(
        !prompts[0].contains("<settings-update>"),
        "unchanged settings: {}",
        prompts[0]
    );
    // Only the task thread rules changed, so only they are restated.
    assert!(prompts[1].contains("<settings-update>"), "{}", prompts[1]);
    assert!(prompts[1].contains("**Delegating to another agent:**"));
    assert!(
        !prompts[1].contains("<agent-instructions>"),
        "{}",
        prompts[1]
    );
    assert!(
        !prompts[1].contains("<team-instructions>"),
        "{}",
        prompts[1]
    );
    assert!(
        !prompts[2].contains("<settings-update>"),
        "sent once: {}",
        prompts[2]
    );
    // The other live session learns about the edit on its own next turn.
    assert!(prompts[3].contains("<settings-update>"));
    assert!(prompts[3].contains("**Delegating to another agent:**"));
    let second_scope = SessionScope::Conversation { channel_id: second };
    assert_eq!(
        agent.state.deliveries[&second_scope].live_text_seen,
        Some(ctx.live_now().text)
    );
}

#[tokio::test]
async fn new_session_takes_current_settings_into_its_system_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("agent.json");
    write_settings(&file, "You are Eva.", "");
    let settings = live_settings(&file);
    // Edited after the harness started: a new session still gets the edit.
    write_settings(&file, "You are Eva, the release manager.", "multi_step");
    let ctx = Arc::new(thread_policy_context(settings));

    let capture =
        std::env::temp_dir().join(format!("buzz-acp-live-session-{}.ndjson", Uuid::new_v4()));
    let quoted_capture = tests::shell_quoted_path(&capture);
    let script = format!(
        r#"count=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{quoted_capture}'
  if [ "$count" -eq 0 ]; then
    printf '%s\n' '{{"jsonrpc":"2.0","id":0,"result":{{"sessionId":"sess-1"}}}}'
  else
    printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$count,\"result\":{{\"stopReason\":\"end_turn\"}}}}"
  fi
  count=$((count + 1))
done"#
    );
    let acp = crate::acp::AcpClient::spawn(bash(), &["-c".to_string(), script], &[], false)
        .await
        .unwrap();
    let channel_id = Uuid::new_v4();
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    run_prompt_task(
        modern_agent(acp),
        Some(batch(channel_id, "hello")),
        None,
        Arc::clone(&ctx),
        result_tx,
        None,
        "turn-0".into(),
    )
    .await;
    let mut result = result_rx.recv().await.unwrap();
    assert!(matches!(
        result.outcome,
        PromptOutcome::Ok(StopReason::EndTurn)
    ));
    result.agent.acp.shutdown().await;

    let requests = captured_requests(&capture);
    assert!(requests[0].contains("session/new"));
    assert!(
        requests[0].contains("You are Eva, the release manager."),
        "{}",
        requests[0]
    );
    assert!(requests[0].contains("Ship small."));
    assert!(requests[0].contains("**Multi-step work:**"));
    assert!(
        !prompt_text(&requests[1]).contains("settings-update"),
        "a new session already holds the current settings"
    );
    let delivery = &result.agent.state.deliveries[&SessionScope::Conversation { channel_id }];
    assert_eq!(delivery.live_text_seen, Some(ctx.live_now().text));
}

#[test]
fn context_history_follows_the_live_file() {
    use crate::context_history::{ContextBudget, ContextHistory};
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("agent.json");
    let ctx = thread_policy_context(live_settings(&file));
    assert_eq!(ctx.live_now().context_history, ContextHistory::Recent);
    std::fs::write(&file, r#"{"context_history":"large"}"#).unwrap();
    assert_eq!(
        ctx.live_now().context_history,
        ContextHistory::Budget(ContextBudget::Large)
    );
}

#[test]
fn launch_values_apply_without_a_live_file() {
    let mut ctx = make_prompt_context_no_owner();
    ctx.base_prompt = Some("BASE".into());
    ctx.system_prompt = Some("Launch prompt".into());
    ctx.task_threads = vec![crate::task_threads::TaskThreadTrigger::Parallel];
    ctx.task_thread_rules_apply = true;
    let live = ctx.live_now();
    assert_eq!(live.text.system_prompt.as_deref(), Some("Launch prompt"));
    let base = live.base_prompt.unwrap();
    assert!(base.starts_with("BASE\n\n### Opening Task Threads Yourself"));
    assert!(base.contains("**Several independent tasks"));
}
