//! `complete_once` against a local HTTP stub: the public facade sends one
//! system + user request with the caller's model and token cap, returns the
//! reply text, and honors `cfg.llm_timeout`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::complete_once;
use crate::config::{Config, Provider};

/// Read one HTTP request (headers + `content-length` body) from `socket`.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, Value)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break index + 4;
        }
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let path = headers.split(' ').nth(1).unwrap_or_default().to_string();
    let body = serde_json::from_slice(&bytes[header_end..header_end + content_length]).ok()?;
    Some((path, body))
}

#[tokio::test]
async fn complete_once_returns_the_reply_text_for_one_anthropic_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (captured_tx, captured_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await.unwrap();
        let body = json!({
            "content": [{ "type": "text", "text": "{\"to\":[\"a1\"]}" }],
            "stop_reason": "end_turn",
        })
        .to_string();
        let wire = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(wire.as_bytes()).await.unwrap();
        let _ = captured_tx.send(request);
    });

    let cfg = Config::for_discovery(Provider::Anthropic, "key".into(), base_url, None);
    let text = complete_once(&cfg, "route it", "MESSAGE\nhi", 40, "claude-haiku-4-5")
        .await
        .unwrap();
    assert_eq!(text, "{\"to\":[\"a1\"]}");

    let (path, body) = captured_rx.await.unwrap();
    assert_eq!(path, "/v1/messages");
    assert_eq!(body["model"], "claude-haiku-4-5");
    assert_eq!(body["max_tokens"], 40);
    assert_eq!(body["system"], "route it");
    assert_eq!(body["messages"][0]["content"][0]["text"], "MESSAGE\nhi");
}

#[tokio::test]
async fn complete_once_gives_up_on_a_silent_provider_within_the_configured_timeout() {
    // Accept connections and never answer. With the discovery default (30 s)
    // the call would outlive this test's bound; the configured 100 ms per
    // attempt (plus the bounded retries) must end it well inside.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });

    let mut cfg = Config::for_discovery(Provider::Anthropic, "key".into(), base_url, None);
    cfg.llm_timeout = Duration::from_millis(100);
    let started = Instant::now();
    let result = complete_once(&cfg, "system", "user", 40, "claude-haiku-4-5").await;
    assert!(
        result.is_err(),
        "a silent provider must fail, got {result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "complete_once ignored cfg.llm_timeout: took {:?}",
        started.elapsed()
    );
}
