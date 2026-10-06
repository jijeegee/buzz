//! Production HTTP seams held at admission while the active view changes.

use super::*;
use std::{future::Future, task::Poll, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct Request {
    headers: String,
    body: Vec<u8>,
}

async fn local_relay(response: String) -> (String, tokio::task::JoinHandle<Request>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                headers.push_str(&line);
                assert!(headers.len() < 16_384, "bounded fixture headers");
                if line == "\r\n" {
                    break;
                }
            }
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            assert!(length < 65_536, "bounded fixture body");
            let mut body = vec![0; length];
            reader.read_exact(&mut body).await.unwrap();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(), response
            );
            reader.get_mut().write_all(reply.as_bytes()).await.unwrap();
            Request { headers, body }
        })
        .await
        .expect("fixture request must complete")
    });
    (origin, task)
}

fn auth_header(request: &Request) -> &str {
    request
        .headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim())
        })
        .unwrap()
}

fn nip98(request: &Request) -> nostr::Event {
    let encoded = auth_header(request).strip_prefix("Nostr ").unwrap();
    serde_json::from_slice(&BASE64.decode(encoded).unwrap()).unwrap()
}

async fn parked(future: std::pin::Pin<&mut impl Future>) {
    let mut future = future;
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending(), "request must park");
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn account_scope_query_keeps_original_signer_after_admission_wait() {
    let _serial = crate::relay_admission::TEST_SERIAL.lock().await;
    crate::relay_admission::reset_rate_limit_gate();
    let (origin, server) = local_relay("[]".into()).await;
    let state = crate::app_state::build_app_state();
    let original = nostr::Keys::generate();
    *state.keys.lock().unwrap() = original.clone();
    *state.relay_url_override.lock().unwrap() = Some(origin.clone());
    let filters = [serde_json::json!({"kinds": [40002], "#h": ["room-a"]})];
    crate::relay_admission::activate_rate_limit(Some(1));
    let request = query_relay_at(&state, &origin, &filters);
    tokio::pin!(request);
    parked(request.as_mut()).await;
    *state.keys.lock().unwrap() = nostr::Keys::generate();
    *state.relay_url_override.lock().unwrap() = Some("http://127.0.0.1:1".into());
    assert!(tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap()
        .is_empty());
    let received = server.await.unwrap();
    let auth = nip98(&received);
    auth.verify().unwrap();
    assert_eq!(auth.pubkey, original.public_key());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&received.body).unwrap(),
        serde_json::json!(filters)
    );
    assert!(received.headers.starts_with("POST /query "));
    crate::relay_admission::reset_rate_limit_gate();
}

#[tokio::test]
async fn account_scope_query_keeps_original_token_on_same_origin() {
    let _serial = crate::relay_admission::TEST_SERIAL.lock().await;
    crate::relay_admission::reset_rate_limit_gate();
    let (origin, server) = local_relay("[]".into()).await;
    let state = crate::app_state::build_app_state();
    *state.relay_url_override.lock().unwrap() = Some(origin.clone());
    let session = |token: &str| {
        crate::auth::OriginAuth::Active(crate::auth::UserSession {
            principal: nostr::Keys::generate().public_key(),
            device_id: None,
            access: zeroize::Zeroizing::new(token.into()),
            refresh: zeroize::Zeroizing::new("fixture-refresh".into()),
            access_issued_at: 0,
            access_expires_at: i64::MAX,
        })
    };
    state.token_auth.set(&origin, session("fixture-account-a"));
    let filters = [serde_json::json!({"kinds": [40002], "#h": ["room-a"]})];
    crate::relay_admission::activate_rate_limit(Some(1));
    let request = query_relay_at(&state, &origin, &filters);
    tokio::pin!(request);
    parked(request.as_mut()).await;
    state.token_auth.set(&origin, session("fixture-account-b"));
    assert!(tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap()
        .is_empty());
    assert_eq!(
        auth_header(&server.await.unwrap()),
        "Bearer fixture-account-a"
    );
    crate::relay_admission::reset_rate_limit_gate();
}

#[tokio::test]
async fn account_scope_agent_send_keeps_original_relay_and_room() {
    let _serial = crate::relay_admission::TEST_SERIAL.lock().await;
    crate::relay_admission::reset_rate_limit_gate();
    let agent = nostr::Keys::generate();
    let event = nostr::EventBuilder::new(nostr::Kind::Custom(40002), "A result")
        .tag(nostr::Tag::parse(["h", "room-a"]).unwrap())
        .sign_with_keys(&agent)
        .unwrap();
    let (origin, server) = local_relay(
        serde_json::json!({
            "event_id": event.id.to_hex(), "accepted": true, "message": ""
        })
        .to_string(),
    )
    .await;
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let state = crate::app_state::build_app_state();
    *state.relay_url_override.lock().unwrap() = Some(origin);
    crate::relay_admission::activate_rate_limit(Some(1));
    let request = submit_signed_event_with_keys(&event, &state, &agent, Some("fixture-owner-a"));
    tokio::pin!(request);
    parked(request.as_mut()).await;
    *state.keys.lock().unwrap() = nostr::Keys::generate();
    *state.relay_url_override.lock().unwrap() =
        Some(format!("http://{}", other.local_addr().unwrap()));
    assert!(
        tokio::time::timeout(Duration::from_secs(5), request)
            .await
            .unwrap()
            .unwrap()
            .accepted
    );
    let received = server.await.unwrap();
    let signed: nostr::Event = serde_json::from_slice(&received.body).unwrap();
    signed.verify().unwrap();
    assert_eq!(signed, event);
    assert_eq!(nip98(&received).pubkey, agent.public_key());
    assert!(received
        .headers
        .to_ascii_lowercase()
        .contains("x-auth-tag: fixture-owner-a"));
    assert!(
        tokio::time::timeout(Duration::from_millis(25), other.accept())
            .await
            .is_err(),
        "B must receive no A request"
    );
    crate::relay_admission::reset_rate_limit_gate();
}
