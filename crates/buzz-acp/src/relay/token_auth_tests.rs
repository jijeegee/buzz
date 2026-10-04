//! Token-mode WebSocket auth (centralized identity, Phase 1): connect-time
//! token AUTH, same-connection re-AUTH, and the terminal/transient split.

use super::tests::{next_test_frame, test_ws_pair};
use super::*;

fn token_identity(token: &str) -> AgentIdentity {
    AgentIdentity::token(
        Keys::generate().public_key(),
        BotToken::new(token.to_owned(), i64::MAX),
    )
}

/// `do_connect` in token mode sends `["AUTH", {"token"}]` (never a signed
/// NIP-42 event), accepts `OK auth true`, and drops the relay's challenge from
/// the replay buffer.
#[tokio::test]
async fn connect_authenticates_with_the_token_frame() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(Message::Text(
            json!(["AUTH", "challenge-1"]).to_string().into(),
        ))
        .await
        .unwrap();
        let frame = next_test_frame(&mut ws).await;
        ws.send(Message::Text(
            json!(["OK", "auth", true, ""]).to_string().into(),
        ))
        .await
        .unwrap();
        (frame, ws)
    });
    let (_ws, buffer) = do_connect(&url, &token_identity("bzb_connect"), None)
        .await
        .expect("token connect");
    let (frame, _server_ws) = server.await.unwrap();
    assert_eq!(frame, json!(["AUTH", {"token": "bzb_connect"}]));
    assert!(
        !buffer
            .iter()
            .any(|m| matches!(m, RelayMessage::Auth { .. })),
        "NIP-42 challenge must not be replayed on a token connection"
    );
}

/// A token AUTH rejection surfaces as `AuthFailed` with the relay's message;
/// a terminal code also raises the auth-terminal exit.
#[tokio::test]
async fn connect_rejection_with_token_code_is_terminal() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = next_test_frame(&mut ws).await;
        ws.send(Message::Text(
            json!(["OK", "auth", false, "auth-required: token_revoked"])
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        // Keep the socket open until the client is done.
        let _ = ws.next().await;
    });
    let Err(err) = do_connect(&url, &token_identity("bzb_revoked"), None).await else {
        panic!("token AUTH rejection must fail the connect");
    };
    assert!(
        matches!(&err, RelayError::AuthFailed(m) if m.contains("token_revoked")),
        "{err}"
    );
    assert!(crate::token_refresh::auth_terminal_requested());
}

/// Same-connection re-AUTH: the command sends the *current* token on the live
/// socket, and the relay's `OK auth true` resolves the waiter.
#[tokio::test]
async fn reauth_sends_current_token_and_resolves_on_ok() {
    let (mut client, mut server) = test_ws_pair().await;
    let (event_tx, _event_rx) = mpsc::channel(4);
    let (observer_tx, _observer_rx) = mpsc::channel(4);
    let identity = token_identity("bzb_one");
    let token = identity.bot_token().cloned().unwrap();
    let mut state = BgState::new();
    state.bot_token = Some(token.clone());
    token.adopt(0, "bzb_two".into(), i64::MAX).unwrap();

    let (ack, mut ack_rx) = oneshot::channel();
    assert!(
        execute_connected_command(
            &mut client,
            &mut state,
            "agent",
            RelayCommand::Reauth { ack }
        )
        .await
    );
    assert_eq!(
        next_test_frame(&mut server).await,
        json!(["AUTH", {"token": "bzb_two"}])
    );
    assert!(
        ack_rx.try_recv().is_err(),
        "pending until the relay answers"
    );

    let ok = json!(["OK", "auth", true, ""]).to_string();
    let keep = handle_ws_message(
        Message::Text(ok.into()),
        &mut client,
        &event_tx,
        &observer_tx,
        &mut state,
        &identity,
        "ws://relay",
        "agent",
        None,
    )
    .await;
    assert!(keep);
    assert_eq!(ack_rx.await.unwrap(), Ok(()));
}

/// A non-terminal re-AUTH rejection keeps the connection (the relay keeps the
/// previous binding) and reports the message to the waiter.
#[tokio::test]
async fn non_terminal_reauth_rejection_keeps_the_connection() {
    let (mut client, _server) = test_ws_pair().await;
    let (event_tx, _event_rx) = mpsc::channel(4);
    let (observer_tx, _observer_rx) = mpsc::channel(4);
    let identity = token_identity("bzb_one");
    let mut state = BgState::new();
    state.bot_token = identity.bot_token().cloned();
    let (ack, ack_rx) = oneshot::channel();
    state.pending_reauth = Some(ack);
    let frame = json!(["OK", "auth", false, "auth-required: unavailable"]).to_string();
    let keep = handle_ws_message(
        Message::Text(frame.into()),
        &mut client,
        &event_tx,
        &observer_tx,
        &mut state,
        &identity,
        "ws://relay",
        "agent",
        None,
    )
    .await;
    assert!(keep, "a transient re-AUTH failure must not drop the socket");
    assert_eq!(
        ack_rx.await.unwrap(),
        Err("auth-required: unavailable".into())
    );
}

/// A NIP-42 challenge on a token connection is ignored: no AUTH event is sent.
#[tokio::test]
async fn nip42_challenge_is_ignored_in_token_mode() {
    let (mut client, mut server) = test_ws_pair().await;
    let (event_tx, _event_rx) = mpsc::channel(4);
    let (observer_tx, _observer_rx) = mpsc::channel(4);
    let identity = token_identity("bzb_one");
    let mut state = BgState::new();
    let frame = json!(["AUTH", "challenge-2"]).to_string();
    assert!(
        handle_ws_message(
            Message::Text(frame.into()),
            &mut client,
            &event_tx,
            &observer_tx,
            &mut state,
            &identity,
            "ws://relay",
            "agent",
            None,
        )
        .await
    );
    assert!(
        timeout(Duration::from_millis(200), server.next())
            .await
            .is_err(),
        "no frame may be sent in reply to a NIP-42 challenge"
    );
}

/// While disconnected a re-AUTH resolves at once: the reconnect authenticates
/// with the current token.
#[test]
fn reauth_while_disconnected_resolves_ok() {
    let mut state = BgState::new();
    let (ack, mut rx) = oneshot::channel();
    apply_command_to_state(&mut state, RelayCommand::Reauth { ack });
    assert_eq!(rx.try_recv().unwrap(), Ok(()));
}

/// B5: the relay drops the socket after receiving a re-AUTH, so its `OK auth`
/// never arrives. The production reconnect path must settle the waiter (the
/// new connection authenticated with the current token) instead of leaving
/// the refresh task blocked forever.
#[tokio::test]
async fn socket_drop_after_reauth_settles_the_ack_on_reconnect() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (reauth_seen_tx, reauth_seen_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        // Connection 1: authenticate, receive the re-AUTH, then drop.
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = next_test_frame(&mut ws).await;
        ws.send(Message::Text(
            json!(["OK", "auth", true, ""]).to_string().into(),
        ))
        .await
        .unwrap();
        let reauth = next_test_frame(&mut ws).await;
        drop(ws);
        let _ = reauth_seen_tx.send(reauth);
        // Connection 2: the reconnect authenticates with the current token.
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let auth = next_test_frame(&mut ws).await;
        ws.send(Message::Text(
            json!(["OK", "auth", true, ""]).to_string().into(),
        ))
        .await
        .unwrap();
        let _ = ws.next().await;
        auth
    });

    let identity = token_identity("bzb_one");
    let token = identity.bot_token().cloned().unwrap();
    let (mut ws, _buffer) = do_connect(&url, &identity, None).await.expect("connect");
    let mut state = BgState::new();
    state.bot_token = Some(token.clone());
    token.adopt(0, "bzb_two".into(), i64::MAX).unwrap();

    let (ack, mut ack_rx) = oneshot::channel();
    assert!(
        execute_connected_command(&mut ws, &mut state, "agent", RelayCommand::Reauth { ack }).await
    );
    assert_eq!(
        reauth_seen_rx.await.unwrap(),
        json!(["AUTH", {"token": "bzb_two"}])
    );
    assert!(ack_rx.try_recv().is_err(), "pending while the socket is up");

    let (_cmd_tx, mut cmd_rx) = mpsc::channel(4);
    let (event_tx, _event_rx) = mpsc::channel(4);
    let (observer_tx, _observer_rx) = mpsc::channel(4);
    let generation = state.connection_generation;
    let _ = timeout(
        Duration::from_secs(10),
        try_autonomous_reconnect(
            &mut ws,
            &mut cmd_rx,
            &mut state,
            &identity,
            &url,
            "agent",
            &event_tx,
            &observer_tx,
            None,
        ),
    )
    .await
    .expect("reconnect completes");
    assert_eq!(state.connection_generation, generation + 1);
    assert_eq!(
        ack_rx.try_recv(),
        Ok(Ok(())),
        "the reconnect settles the dropped socket's re-AUTH"
    );
    assert!(state.pending_reauth.is_none());
    drop(ws);
    assert_eq!(
        server.await.unwrap(),
        json!(["AUTH", {"token": "bzb_two"}]),
        "the reconnect authenticated with the exchanged token"
    );
}

/// B5: a re-AUTH whose answer never comes resolves after `AUTH_TIMEOUT`
/// instead of blocking the refresh task forever.
#[tokio::test(start_paused = true)]
async fn reauth_without_an_answer_times_out() {
    let (handle, mut requests) = RelayReauthHandle::test_pair();
    let held = tokio::spawn(async move {
        // Hold the ack without answering, as a dead socket would.
        let ack = requests.recv().await;
        tokio::time::sleep(AUTH_TIMEOUT * 10).await;
        drop(ack);
    });
    let started = tokio::time::Instant::now();
    let outcome = handle.reauth().await;
    assert_eq!(
        outcome,
        Err("re-AUTH timed out waiting for OK auth".to_string())
    );
    assert!(started.elapsed() >= AUTH_TIMEOUT);
    assert!(started.elapsed() < AUTH_TIMEOUT * 2);
    held.abort();
}
