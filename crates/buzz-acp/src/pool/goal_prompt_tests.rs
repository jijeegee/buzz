//! Goal layers through the real `run_prompt_task` seam: `<goal-context>`
//! appears only where the channel has a goal tree, and a layer 0 goal edit
//! reaches a live session exactly once as `<goal-update>`.

use super::tests::make_prompt_context_no_owner;
use super::*;
use buzz_core::goal_tree::{GoalOp, GoalTree};
use nostr::{EventBuilder, Keys, Kind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

fn goal_tree_content() -> String {
    let mut tree = GoalTree::empty();
    tree.apply(
        &GoalOp::SetRoot {
            id: "root".into(),
            title: "Launch the beta".into(),
            note: None,
        },
        &"a".repeat(64),
        1,
    )
    .unwrap();
    tree.to_content().unwrap()
}

/// A relay that answers goal tree queries for `with_tree` only; every other
/// query gets an empty result.
async fn spawn_relay(with_tree: Uuid) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let tree_body = serde_json::json!([{ "content": goal_tree_content() }]).to_string();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = vec![0; 16 * 1024];
            let read = socket.read(&mut request).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]);
            let body = if request.contains("40110") && request.contains(&with_tree.to_string()) {
                tree_body.as_str()
            } else {
                "[]"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    base_url
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

fn write_goals(path: &std::path::Path, agent_goal: &str) {
    std::fs::write(
        path,
        serde_json::json!({ "agent_goal": agent_goal }).to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn goal_context_follows_the_tree_and_layer0_edits_reach_live_sessions_once() {
    let goals_dir = tempfile::tempdir().unwrap();
    let goals_file = goals_dir.path().join("agent.json");
    write_goals(&goals_file, "Ship weekly");

    let with_tree = Uuid::new_v4();
    let without_tree = Uuid::new_v4();
    let base_url = spawn_relay(with_tree).await;

    let capture =
        std::env::temp_dir().join(format!("buzz-acp-goal-prompt-{}.ndjson", Uuid::new_v4()));
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
    let mut agent = OwnedAgent {
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
    };
    let layer0 = crate::layer0_goals::Layer0Goals::file(goals_file.clone(), true);
    // Both sessions started while the goal was "Ship weekly".
    for channel_id in [with_tree, without_tree] {
        let scope = SessionScope::Conversation { channel_id };
        agent
            .state
            .sessions
            .insert(scope.clone(), format!("session-{channel_id}"));
        agent.state.deliveries.insert(
            scope,
            ChannelDeliveryState {
                layer0_goals_seen: Some(layer0.current()),
                ..Default::default()
            },
        );
    }

    let mut ctx = make_prompt_context_no_owner();
    ctx.rest_client.base_url = base_url;
    ctx.layer0_goals = layer0;
    let ctx = Arc::new(ctx);
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();

    let turns = [
        (with_tree, None),
        (with_tree, Some("Ship daily")),
        (with_tree, None),
        (without_tree, None),
    ];
    for (turn, (channel_id, new_goal)) in turns.into_iter().enumerate() {
        if let Some(goal) = new_goal {
            write_goals(&goals_file, goal);
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

    let prompts: Vec<String> = std::fs::read_to_string(&capture)
        .unwrap()
        .lines()
        .map(|line| {
            let request: serde_json::Value = serde_json::from_str(line).unwrap();
            request["params"]["prompt"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    std::fs::remove_file(&capture).unwrap();
    assert_eq!(prompts.len(), 4);

    // A channel with a tree gets the tree and the rules every turn.
    for prompt in &prompts[..3] {
        assert!(prompt.contains("<goal-context>"), "{prompt}");
        assert!(prompt.contains("L1 [open] Launch the beta"));
        assert!(prompt.contains("Rules:"));
    }
    // The edit reaches the live session once, with the new value.
    assert!(
        !prompts[0].contains("<goal-update>"),
        "unchanged goals: {}",
        prompts[0]
    );
    assert!(prompts[1].contains("<goal-update>"));
    assert!(prompts[1].contains("Ship daily") && !prompts[1].contains("Ship weekly"));
    assert!(
        !prompts[2].contains("<goal-update>"),
        "sent once: {}",
        prompts[2]
    );
    // No tree: no goal context and no rules. The other live session still
    // learns about the edit on its next turn.
    assert!(!prompts[3].contains("<goal-context>"));
    assert!(!prompts[3].contains("buzz goals"));
    assert!(prompts[3].contains("<goal-update>") && prompts[3].contains("Ship daily"));
}

#[tokio::test]
async fn new_session_takes_current_layer0_goals_into_its_system_prompt() {
    let goals_dir = tempfile::tempdir().unwrap();
    let goals_file = goals_dir.path().join("agent.json");
    write_goals(&goals_file, "Grow the team");
    let mut ctx = make_prompt_context_no_owner();
    ctx.layer0_goals = crate::layer0_goals::Layer0Goals::file(goals_file.clone(), true);
    // Edited after the harness started: a new session still gets the edit.
    write_goals(&goals_file, "Hire two engineers");
    let ctx = Arc::new(ctx);

    let capture =
        std::env::temp_dir().join(format!("buzz-acp-goal-session-{}.ndjson", Uuid::new_v4()));
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
    let agent = OwnedAgent {
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
    };
    let channel_id = Uuid::new_v4();
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    run_prompt_task(
        agent,
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

    let requests: Vec<String> = std::fs::read_to_string(&capture)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    std::fs::remove_file(&capture).unwrap();
    assert!(requests[0].contains("session/new"));
    assert!(
        requests[0].contains("Hire two engineers"),
        "{}",
        requests[0]
    );
    assert!(!requests[0].contains("Grow the team"));
    assert!(
        !requests[1].contains("goal-update"),
        "a new session already holds the current goals"
    );
    let delivery = &result.agent.state.deliveries[&SessionScope::Conversation { channel_id }];
    assert_eq!(delivery.layer0_goals_seen, Some(ctx.layer0_goals.current()));
}
