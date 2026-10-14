//! Goal layers through the real `run_prompt_task` seam: `<goal-context>`
//! appears only where the channel has a goal tree and only until the session
//! holds it, and a layer 0 goal edit reaches a live session exactly once as
//! `<goal-update>`.

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

    // A channel with a tree sends the tree and the rules once; unchanged
    // goals add nothing to later turns.
    assert!(prompts[0].contains("<goal-context>"), "{}", prompts[0]);
    assert!(prompts[0].contains("L1 [open] Launch the beta"));
    assert!(prompts[0].contains("Rules:"));
    for prompt in &prompts[1..3] {
        assert!(!prompt.contains("<goal-context>"), "{prompt}");
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
    let channel_id = Uuid::new_v4();
    let mut ctx = make_prompt_context_no_owner();
    ctx.rest_client.base_url = spawn_relay(channel_id).await;
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
    // The conversation's goals ride in the system prompt, not in the turn.
    assert!(requests[0].contains("<goal-context>"), "{}", requests[0]);
    assert!(requests[0].contains("L1 [open] Launch the beta"));
    assert!(!requests[1].contains("<goal-context>"), "{}", requests[1]);
    let delivery = &result.agent.state.deliveries[&SessionScope::Conversation { channel_id }];
    assert_eq!(delivery.layer0_goals_seen, Some(ctx.layer0_goals.current()));
    assert!(
        delivery.goal_context_seen.is_some(),
        "a new session records the goals it starts with"
    );
}

/// A scripted claude-agent-acp: opens `sess-new`, advertises `session/resume`, fails the first
/// `fail_resumes` resumes, and ends every prompt turn. Requests land in the
/// returned capture file.
async fn spawn_claude(fail_resumes: u32) -> (OwnedAgent, std::path::PathBuf) {
    let capture =
        std::env::temp_dir().join(format!("buzz-acp-claude-goals-{}.ndjson", Uuid::new_v4()));
    let quoted_capture = tests::shell_quoted_path(&capture);
    let script = format!(
        r#"resumes=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{quoted_capture}'
  id=$(printf '%s' "$line" | grep -o '"id":[0-9]*' | head -n 1 | cut -d: -f2)
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"protocolVersion\":1,\"agentCapabilities\":{{\"sessionCapabilities\":{{\"resume\":{{}}}}}}}}}}" ;;
    *'"method":"session/new"'*)
      printf '%s
' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"sessionId\":\"sess-new\"}}}}" ;;
    *'"method":"session/resume"'*)
      resumes=$((resumes + 1))
      if [ "$resumes" -le {fail_resumes} ]; then
        printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{{\"code\":-32603,\"message\":\"resume failed\"}}}}"
      else
        printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{}}}}"
      fi ;;
    *)
      printf '%s\n' "{{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{{\"stopReason\":\"end_turn\"}}}}" ;;
  esac
done"#
    );
    let mut acp = crate::acp::AcpClient::spawn(bash(), &["-c".to_string(), script], &[], false)
        .await
        .unwrap();
    acp.initialize().await.unwrap();
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
        agent_name: CLAUDE_AGENT_ACP_NAME.into(),
        goose_system_prompt_supported: None,
        protocol_version: 1,
    };
    (agent, capture)
}

/// A live Claude session that started without goals.
fn claude_session_without_goals(agent: &mut OwnedAgent, channel_id: Uuid) -> SessionScope {
    claude_session(
        agent,
        channel_id,
        PromptGoals {
            base: Some("BUZZ BASE".into()),
            mcp_servers: vec![],
            title: Some("Agent · #general".into()),
            held: None,
            sent: Some("BUZZ BASE".into()),
            in_messages: false,
            in_messages_known: true,
        },
    )
}

/// A live Claude session whose system prompt is described by `prompt`.
fn claude_session(agent: &mut OwnedAgent, channel_id: Uuid, prompt: PromptGoals) -> SessionScope {
    let scope = SessionScope::Conversation { channel_id };
    agent
        .state
        .sessions
        .insert(scope.clone(), "sess-claude".into());
    agent.state.deliveries.insert(
        scope.clone(),
        ChannelDeliveryState {
            prompt_goals: Some(prompt),
            ..Default::default()
        },
    );
    scope
}

async fn run_turns(
    mut agent: OwnedAgent,
    ctx: &Arc<PromptContext>,
    channel_id: Uuid,
    turns: usize,
) -> OwnedAgent {
    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    for turn in 0..turns {
        run_prompt_task(
            agent,
            Some(batch(channel_id, &format!("message {turn}"))),
            None,
            Arc::clone(ctx),
            result_tx.clone(),
            None,
            format!("turn-{turn}"),
        )
        .await;
        let result = result_rx.recv().await.unwrap();
        assert!(
            matches!(result.outcome, PromptOutcome::Ok(StopReason::EndTurn)),
            "turn {turn}"
        );
        agent = result.agent;
    }
    agent
}

fn captured(capture: &std::path::Path) -> Vec<serde_json::Value> {
    let requests = std::fs::read_to_string(capture)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    std::fs::remove_file(capture).unwrap();
    requests
}

fn method(request: &serde_json::Value) -> &str {
    request["method"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn claude_session_gets_changed_goals_in_its_system_prompt_not_its_turns() {
    let channel_id = Uuid::new_v4();
    let mut ctx = make_prompt_context_no_owner();
    ctx.rest_client.base_url = spawn_relay(channel_id).await;
    let ctx = Arc::new(ctx);
    let (mut agent, capture) = spawn_claude(0).await;
    let scope = claude_session_without_goals(&mut agent, channel_id);

    let mut agent = run_turns(agent, &ctx, channel_id, 2).await;
    agent.acp.shutdown().await;
    let requests = captured(&capture);
    let methods: Vec<&str> = requests.iter().map(method).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "session/resume",
            "session/prompt",
            "session/prompt"
        ],
        "goals are swapped in once, before the first turn"
    );

    let resume = &requests[1]["params"];
    assert_eq!(resume["sessionId"], "sess-claude");
    let system_prompt = &resume["_meta"]["systemPrompt"];
    let append = system_prompt["append"].as_str().unwrap();
    assert!(
        append.starts_with("BUZZ BASE\n\n<goal-context>"),
        "{append}"
    );
    assert!(append.contains("L1 [open] Launch the beta"), "{append}");
    assert!(
        !append.contains(crate::goal_context::GOALS_RESTATED),
        "no goals in messages to replace: {append}"
    );
    assert_eq!(system_prompt["snapshot"], false);
    assert_eq!(resume["_meta"]["sessionTitle"], "Agent · #general");

    for prompt in &requests[2..] {
        let text = prompt["params"]["prompt"].to_string();
        assert!(!text.contains("goal-context"), "{text}");
    }
    let delivery = &agent.state.deliveries[&scope];
    let prompt = delivery.prompt_goals.as_ref().unwrap();
    assert!(prompt.held.as_deref().unwrap().contains("Launch the beta"));
    assert!(!prompt.in_messages);
    assert!(delivery.goal_context_seen.is_some());
}

#[tokio::test]
async fn failed_goal_swap_sends_goals_in_the_turn_and_retries_next_turn() {
    let channel_id = Uuid::new_v4();
    let mut ctx = make_prompt_context_no_owner();
    ctx.rest_client.base_url = spawn_relay(channel_id).await;
    let ctx = Arc::new(ctx);
    // The swap fails; reopening with the old prompt works.
    let (mut agent, capture) = spawn_claude(1).await;
    let scope = claude_session_without_goals(&mut agent, channel_id);

    let mut agent = run_turns(agent, &ctx, channel_id, 2).await;
    agent.acp.shutdown().await;
    let requests = captured(&capture);
    let methods: Vec<&str> = requests.iter().map(method).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "session/resume",
            "session/resume",
            "session/prompt",
            "session/resume",
            "session/prompt"
        ]
    );
    // Reopened as it was: no goals in the system prompt.
    let restored = requests[2]["params"]["_meta"]["systemPrompt"]["append"]
        .as_str()
        .unwrap();
    assert_eq!(restored, "BUZZ BASE");
    // The first turn carries the goals itself.
    let first = requests[3]["params"]["prompt"].to_string();
    assert!(first.contains("<goal-context>") && first.contains("Launch the beta"));
    // The next turn swaps them in, marked as replacing the message copy.
    let retried = requests[4]["params"]["_meta"]["systemPrompt"]["append"]
        .as_str()
        .unwrap();
    assert!(
        retried.contains(crate::goal_context::GOALS_RESTATED),
        "{retried}"
    );
    assert!(retried.contains("Launch the beta"), "{retried}");
    let second = requests[5]["params"]["prompt"].to_string();
    assert!(!second.contains("goal-context"), "{second}");

    let prompt = agent.state.deliveries[&scope]
        .prompt_goals
        .as_ref()
        .unwrap();
    assert!(prompt.held.is_some());
    assert!(prompt.in_messages);
}

#[tokio::test]
async fn claude_session_that_cannot_be_reopened_is_dropped_and_the_turn_requeued() {
    let channel_id = Uuid::new_v4();
    let mut ctx = make_prompt_context_no_owner();
    ctx.rest_client.base_url = spawn_relay(channel_id).await;
    ctx.dedup_mode = DedupMode::Queue;
    let ctx = Arc::new(ctx);
    let (mut agent, capture) = spawn_claude(2).await;
    let scope = claude_session_without_goals(&mut agent, channel_id);

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
    assert!(matches!(result.outcome, PromptOutcome::Error(_)));
    assert!(result.batch.is_some(), "the turn is retried");
    result.agent.acp.shutdown().await;
    let methods: Vec<String> = captured(&capture)
        .iter()
        .map(|request| method(request).to_string())
        .collect();
    assert_eq!(
        methods,
        ["initialize", "session/resume", "session/resume"],
        "nothing is sent to the dropped session"
    );
    assert!(
        !result.agent.state.sessions.contains_key(&scope),
        "the retry starts a new session"
    );
    assert!(!result.agent.state.deliveries.contains_key(&scope));
}

#[tokio::test]
async fn claude_session_serving_several_threads_is_not_rebuilt_per_thread() {
    let channel_id = Uuid::new_v4();
    let mut ctx = make_prompt_context_no_owner();
    ctx.rest_client.base_url = spawn_relay(channel_id).await;
    let ctx = Arc::new(ctx);
    let (mut agent, capture) = spawn_claude(0).await;
    let scope = claude_session_without_goals(&mut agent, channel_id);
    // The session's system prompt already holds the channel-wide view.
    let tree = GoalTree::parse(&goal_tree_content()).unwrap();
    let channel_view = crate::goal_context::GoalsNow::new(&tree, channel_id, None);
    {
        let delivery = agent.state.deliveries.get_mut(&scope).unwrap();
        delivery.prompt_goals.as_mut().unwrap().held = channel_view.rendered().map(str::to_string);
        delivery.goal_context_seen =
            Some(crate::goal_context::GoalsSeen::after(None, &channel_view));
    }
    // A reply inside a thread of this conversation.
    let root = EventBuilder::new(Kind::Custom(9), "thread root")
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let reply = EventBuilder::new(Kind::Custom(9), "in the thread")
        .tags([nostr::Tag::parse(["e", &root.id.to_hex(), "", "reply"]).unwrap()])
        .sign_with_keys(&Keys::generate())
        .unwrap();
    let mut thread_batch = batch(channel_id, "unused");
    thread_batch.events[0].event = reply;

    let (result_tx, mut result_rx) = mpsc::unbounded_channel();
    run_prompt_task(
        agent,
        Some(thread_batch),
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
    let requests = captured(&capture);
    let methods: Vec<&str> = requests.iter().map(method).collect();
    assert_eq!(methods, ["initialize", "session/prompt"], "no rebuild");
    // The thread's own view rides in the message, as before.
    let turn = requests[1]["params"]["prompt"].to_string();
    assert!(turn.contains("goal-context"), "{turn}");
}

#[test]
fn goals_removed_from_a_claude_prompt_are_announced_only_when_messages_had_them() {
    use crate::goal_context::{goal_removal_notice, GoalsNow};
    let off = GoalsNow::off(None);
    assert!(goal_removal_notice(false, true, &off).is_none());
    assert!(goal_removal_notice(true, true, &off)
        .unwrap()
        .contains("turned off"));
    // After a restart goals may be in the messages: turning them off is said.
    assert!(goal_removal_notice(true, false, &off)
        .unwrap()
        .contains("turned off"));
    let none = GoalsNow::new(&GoalTree::empty(), Uuid::new_v4(), None);
    assert!(goal_removal_notice(true, true, &none)
        .unwrap()
        .contains("no longer has goals"));
    // ...but a room without goals is not told its goals are gone.
    assert!(goal_removal_notice(true, false, &none).is_none());
    let tree = GoalTree::parse(&goal_tree_content()).unwrap();
    let some = GoalsNow::new(&tree, Uuid::new_v4(), None);
    assert!(goal_removal_notice(true, true, &some).is_none());
}

#[tokio::test]
async fn reattached_claude_session_in_a_room_without_goals_is_not_told_they_are_gone() {
    let channel_id = Uuid::new_v4();
    let mut ctx = make_prompt_context_no_owner();
    // The relay has goals only for some other room.
    ctx.rest_client.base_url = spawn_relay(Uuid::new_v4()).await;
    let ctx = Arc::new(ctx);
    let (mut agent, capture) = spawn_claude(0).await;
    // As reattached after a restart: goals may sit in its messages.
    let scope = claude_session(
        &mut agent,
        channel_id,
        PromptGoals {
            base: Some("BUZZ BASE".into()),
            mcp_servers: vec![],
            title: None,
            held: None,
            sent: Some("BUZZ BASE".into()),
            in_messages: true,
            in_messages_known: false,
        },
    );

    let mut agent = run_turns(agent, &ctx, channel_id, 1).await;
    agent.acp.shutdown().await;
    let requests = captured(&capture);
    let methods: Vec<&str> = requests.iter().map(method).collect();
    assert_eq!(methods, ["initialize", "session/prompt"]);
    let turn = requests[1]["params"]["prompt"].to_string();
    assert!(!turn.contains("goal-context"), "{turn}");
    assert!(agent.state.deliveries[&scope].prompt_goals.is_some());
}

#[tokio::test]
async fn removed_goals_are_announced_once_and_dropped_from_an_otherwise_empty_prompt() {
    let channel_id = Uuid::new_v4();
    let mut ctx = make_prompt_context_no_owner();
    // The room's goals are gone.
    ctx.rest_client.base_url = spawn_relay(Uuid::new_v4()).await;
    let ctx = Arc::new(ctx);
    // The first swap fails and the session is reopened as it was; the next
    // turn's swap works.
    let (mut agent, capture) = spawn_claude(1).await;
    let old = "L1 [open] Old goal".to_string();
    let old_sent = with_goals(None, Some(&old));
    let scope = claude_session(
        &mut agent,
        channel_id,
        PromptGoals {
            base: None,
            mcp_servers: vec![],
            title: None,
            held: Some(old),
            sent: old_sent.clone(),
            in_messages: true,
            in_messages_known: true,
        },
    );

    let mut agent = run_turns(agent, &ctx, channel_id, 2).await;
    agent.acp.shutdown().await;
    let requests = captured(&capture);
    let methods: Vec<&str> = requests.iter().map(method).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "session/resume",
            "session/resume",
            "session/prompt",
            "session/resume",
            "session/prompt"
        ]
    );
    // With nothing else in it, the swap still sends an (empty) append so the
    // old goals do not stay.
    for swap in [&requests[1], &requests[4]] {
        assert_eq!(swap["params"]["_meta"]["systemPrompt"]["append"], "");
    }
    assert_eq!(
        requests[2]["params"]["_meta"]["systemPrompt"]["append"].as_str(),
        old_sent.as_deref()
    );
    let notices = [&requests[3], &requests[5]]
        .iter()
        .filter(|prompt| {
            prompt["params"]["prompt"]
                .to_string()
                .contains("no longer has goals")
        })
        .count();
    assert_eq!(notices, 1, "the removal is announced once");
    let prompt = agent.state.deliveries[&scope]
        .prompt_goals
        .as_ref()
        .unwrap();
    assert!(prompt.held.is_none());
    assert!(!prompt.in_messages);
}

#[tokio::test]
async fn new_claude_session_holds_its_goals_in_a_rebuilt_system_prompt() {
    let channel_id = Uuid::new_v4();
    let mut ctx = make_prompt_context_no_owner();
    ctx.rest_client.base_url = spawn_relay(channel_id).await;
    let ctx = Arc::new(ctx);
    let (agent, capture) = spawn_claude(0).await;

    let mut agent = run_turns(agent, &ctx, channel_id, 2).await;
    agent.acp.shutdown().await;
    let requests = captured(&capture);
    let methods: Vec<&str> = requests.iter().map(method).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "session/new",
            "session/prompt",
            "session/prompt"
        ]
    );
    let system_prompt = &requests[1]["params"]["_meta"]["systemPrompt"];
    assert_eq!(system_prompt["snapshot"], false);
    let append = system_prompt["append"].as_str().unwrap();
    assert!(append.ends_with("</goal-context>"), "{append}");
    assert!(append.contains("Launch the beta"), "{append}");

    let prompt = agent.state.deliveries[&SessionScope::Conversation { channel_id }]
        .prompt_goals
        .as_ref()
        .unwrap();
    assert!(prompt.held.as_deref().unwrap().contains("Launch the beta"));
    // Only the goals are kept apart; this test context has no base prompt.
    match prompt.base.as_deref() {
        Some(base) => {
            assert!(!base.contains("goal-context"), "{base}");
            assert!(append.starts_with(base));
        }
        None => assert!(append.starts_with("<goal-context>"), "{append}"),
    }
    assert!(!prompt.in_messages);
}

#[tokio::test]
async fn reattached_claude_session_gets_its_system_prompt_and_goals_again() {
    let channel_id = Uuid::new_v4();
    let scope = SessionScope::Conversation { channel_id };
    let mut ctx = make_prompt_context_no_owner();
    ctx.rest_client.base_url = spawn_relay(channel_id).await;
    let ledger_dir = tempfile::tempdir().unwrap();
    let ledger_path = ledger_dir.path().join("sessions.json");
    // The previous run recorded this scope's session.
    crate::session_ledger::SessionLedger::open(ledger_path.clone()).record(
        &scope,
        "sess-old",
        &ctx.harness_name,
        &ctx.cwd,
    );
    ctx.session_ledger = Some(Arc::new(crate::session_ledger::SessionLedger::open(
        ledger_path,
    )));
    let ctx = Arc::new(ctx);
    let (agent, capture) = spawn_claude(0).await;

    let mut agent = run_turns(agent, &ctx, channel_id, 1).await;
    agent.acp.shutdown().await;
    let requests = captured(&capture);
    let methods: Vec<&str> = requests.iter().map(method).collect();
    assert_eq!(methods, ["initialize", "session/resume", "session/prompt"]);
    let resume = &requests[1]["params"];
    assert_eq!(resume["sessionId"], "sess-old");
    let system_prompt = &resume["_meta"]["systemPrompt"];
    assert_eq!(system_prompt["snapshot"], false);
    let append = system_prompt["append"].as_str().unwrap();
    // Its history may hold older goals, which these replace.
    assert!(
        append.contains(crate::goal_context::GOALS_RESTATED),
        "{append}"
    );
    assert!(append.contains("Launch the beta"), "{append}");
    let turn = requests[2]["params"]["prompt"].to_string();
    assert!(!turn.contains("goal-context"), "{turn}");

    let delivery = &agent.state.deliveries[&scope];
    assert!(delivery.session_reattached);
    let prompt = delivery.prompt_goals.as_ref().unwrap();
    assert!(prompt.held.is_some());
    assert!(prompt.in_messages);
}
