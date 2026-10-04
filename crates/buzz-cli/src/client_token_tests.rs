//! Token-mode (centralized identity, Phase 1) tests for [`BuzzClient`] and
//! [`crate::build_client`]: credential precedence, Bearer vs NIP-98 headers,
//! server-stamped drafts, and the auth exit-code contract.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
    Router,
};
use buzz_core::principal::PrincipalId;
use buzz_token_broker::{Secret, TokenSource};
use nostr::{EventBuilder, Keys, Kind};
use tokio::net::TcpListener;

use super::BuzzClient;
use crate::error::{exit_code, CliError};

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    x_sha256: Option<String>,
    body: Vec<u8>,
}

#[derive(Clone)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    me: Arc<(StatusCode, String)>,
    bridge: Arc<(StatusCode, String)>,
}

/// A relay stand-in: `GET /auth/me` answers `me`, every other request answers
/// `bridge` (`/query`, `/events`). Every request is recorded.
async fn fake_relay(
    me: (StatusCode, String),
    bridge: (StatusCode, String),
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let state = Fake {
        seen: Arc::clone(&seen),
        me: Arc::new(me),
        bridge: Arc::new(bridge),
    };
    let app =
        Router::new()
            .fallback(
                |State(fake): State<Fake>,
                 method: Method,
                 uri: Uri,
                 headers: HeaderMap,
                 body: Body| async move {
                    let body = to_bytes(body, 1 << 20).await.unwrap().to_vec();
                    fake.seen.lock().unwrap().push(Seen {
                        method: method.to_string(),
                        path: uri.path().to_owned(),
                        authorization: headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        x_sha256: headers
                            .get("x-sha-256")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned),
                        body,
                    });
                    let (status, body) = if uri.path() == "/auth/me" {
                        (*fake.me).clone()
                    } else {
                        (*fake.bridge).clone()
                    };
                    Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap()
                },
            )
            .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

fn me_ok(principal: &PrincipalId) -> (StatusCode, String) {
    (
        StatusCode::OK,
        serde_json::json!({
            "principal_id": principal.to_hex(),
            "kind": "bot",
            "bot": {"owner": "ab".repeat(32), "host": null},
        })
        .to_string(),
    )
}

fn accepted() -> (StatusCode, String) {
    (
        StatusCode::OK,
        r#"{"event_id":"00","accepted":true,"message":""}"#.into(),
    )
}

fn lookup(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |name| {
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| (*v).to_owned())
    }
}

fn bridge_requests(seen: &Arc<Mutex<Vec<Seen>>>) -> Vec<Seen> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|s| s.path != "/auth/me")
        .cloned()
        .collect()
}

#[tokio::test]
async fn bot_token_wins_and_requests_carry_bearer_not_nip98() {
    let principal = PrincipalId::generate();
    let (url, seen) = fake_relay(me_ok(&principal), (StatusCode::OK, "[]".into())).await;
    // Every source is configured: the bot token must win, even over a key.
    let source = TokenSource::resolve(lookup(&[
        ("BUZZ_BOT_TOKEN", "bzb_bot"),
        ("BUZZ_ACCESS_TOKEN", "bzs_user"),
        ("BUZZ_TOKEN_BROKER_URL", "http://127.0.0.1:9"),
        ("BUZZ_TOKEN_BROKER_SECRET", "s"),
    ]));
    let key = Keys::generate().secret_key().to_secret_hex();
    let client = crate::build_client(url, source, Some(key), None)
        .await
        .unwrap();

    assert!(client.is_token_mode());
    assert_eq!(client.pubkey(), principal.as_public_key());
    assert_eq!(client.auth_tag_owner_hex(), Some("ab".repeat(32)));
    assert!(
        client.keys("nip44").is_err(),
        "token mode has no secret key"
    );

    client
        .query(&serde_json::json!({"kinds": [1]}))
        .await
        .unwrap();
    let all = seen.lock().unwrap().clone();
    assert_eq!(all[0].path, "/auth/me");
    assert_eq!(all[0].authorization.as_deref(), Some("Bearer bzb_bot"));
    let bridge = bridge_requests(&seen);
    assert_eq!(bridge.len(), 1);
    assert_eq!(bridge[0].path, "/query");
    assert_eq!(bridge[0].authorization.as_deref(), Some("Bearer bzb_bot"));
}

#[tokio::test]
async fn access_token_is_used_when_no_bot_token() {
    let principal = PrincipalId::generate();
    let (url, seen) = fake_relay(me_ok(&principal), (StatusCode::OK, "[]".into())).await;
    let source = TokenSource::resolve(lookup(&[("BUZZ_ACCESS_TOKEN", "bzs_user")]));
    let client = crate::build_client(url, source, None, None).await.unwrap();
    client
        .query(&serde_json::json!({"kinds": [1]}))
        .await
        .unwrap();
    assert_eq!(
        bridge_requests(&seen)[0].authorization.as_deref(),
        Some("Bearer bzs_user")
    );
}

#[tokio::test]
async fn key_mode_is_the_fallback_without_a_token_source() {
    let principal = PrincipalId::generate();
    let (url, seen) = fake_relay(me_ok(&principal), (StatusCode::OK, "[]".into())).await;
    let keys = Keys::generate();
    let source = TokenSource::resolve(lookup(&[]));
    let client = crate::build_client(url, source, Some(keys.secret_key().to_secret_hex()), None)
        .await
        .unwrap();
    assert!(!client.is_token_mode());
    assert_eq!(client.pubkey(), keys.public_key());

    client
        .query(&serde_json::json!({"kinds": [1]}))
        .await
        .unwrap();
    let all = seen.lock().unwrap().clone();
    assert!(
        all.iter().all(|s| s.path != "/auth/me"),
        "key mode never calls /auth/me"
    );
    assert!(all[0]
        .authorization
        .as_deref()
        .is_some_and(|a| a.starts_with("Nostr ")));
}

#[tokio::test]
async fn no_credential_at_all_is_an_auth_error() {
    let err = crate::build_client("http://127.0.0.1:9".into(), None, None, None)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, CliError::Auth(_)));
    assert_eq!(exit_code(&err), 3);
}

#[tokio::test]
async fn broker_failure_after_retry_exits_3() {
    // Bind then drop: nothing listens on this loopback port any more.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let source = TokenSource::Broker {
        url: format!("http://127.0.0.1:{port}"),
        secret: Secret::new("s".into()),
    };
    let err = crate::build_client("http://127.0.0.1:9".into(), Some(source), None, None)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, CliError::Auth(ref m) if m.contains("cannot obtain access token")));
    assert_eq!(exit_code(&err), 3);
}

#[tokio::test]
async fn revoked_token_at_auth_me_exits_3() {
    let revoked = (
        StatusCode::UNAUTHORIZED,
        r#"{"error":"authentication failed","code":"token_revoked"}"#.to_owned(),
    );
    let (url, _seen) = fake_relay(revoked, accepted()).await;
    let err = BuzzClient::with_token(url, Secret::new("bzb_dead".into()))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, CliError::Auth(ref m) if m.contains("token_revoked")),
        "{err}"
    );
    assert_eq!(exit_code(&err), 3);
}

#[tokio::test]
async fn revoked_token_on_a_bridge_call_exits_3() {
    let principal = PrincipalId::generate();
    let revoked = (
        StatusCode::UNAUTHORIZED,
        r#"{"error":"authentication failed","code":"token_revoked"}"#.to_owned(),
    );
    let (url, _seen) = fake_relay(me_ok(&principal), revoked).await;
    let client = BuzzClient::with_token(url, Secret::new("bzb_x".into()))
        .await
        .unwrap();
    let err = client
        .query(&serde_json::json!({"kinds": [1]}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, CliError::Auth(ref m) if m.starts_with("token_revoked")),
        "{err}"
    );
    assert_eq!(exit_code(&err), 3);
}

#[tokio::test]
async fn token_mode_submits_a_principal_draft_with_sentinel_sig() {
    let principal = PrincipalId::generate();
    let (url, seen) = fake_relay(me_ok(&principal), accepted()).await;
    let client = BuzzClient::with_token(url, Secret::new("bzb_x".into()))
        .await
        .unwrap();
    let event = client
        .sign_event(EventBuilder::new(Kind::Custom(40002), "hello"))
        .unwrap();
    client.submit_event(event.clone()).await.unwrap();

    let bridge = bridge_requests(&seen);
    assert_eq!(bridge.len(), 1);
    assert_eq!(
        (bridge[0].method.as_str(), bridge[0].path.as_str()),
        ("POST", "/events")
    );
    assert_eq!(bridge[0].authorization.as_deref(), Some("Bearer bzb_x"));
    let body: serde_json::Value = serde_json::from_slice(&bridge[0].body).unwrap();
    assert_eq!(body["pubkey"], principal.to_hex());
    assert_eq!(body["sig"], "0".repeat(128));
    assert_eq!(body["id"], event.id.to_hex());
    assert!(
        event.verify_id(),
        "client id is the NIP-01 id the relay recomputes"
    );
    assert!(
        !body
            .get("tags")
            .and_then(|t| t.as_array())
            .is_some_and(|tags| tags.iter().any(|t| t[0] == "auth")),
        "no NIP-OA auth tag in token mode"
    );
}

#[tokio::test]
async fn token_mode_media_get_sends_bearer_to_the_relay_origin() {
    let principal = PrincipalId::generate();
    let (url, seen) = fake_relay(me_ok(&principal), (StatusCode::OK, "blob".into())).await;
    let client = BuzzClient::with_token(url, Secret::new("bzb_x".into()))
        .await
        .unwrap();
    let hash = "ab".repeat(32);
    let bytes = client.download_media(&hash).await.unwrap();
    assert_eq!(&bytes[..], b"blob");
    let requests = bridge_requests(&seen);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, format!("/media/{hash}"));
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer bzb_x"));
}

#[tokio::test]
async fn token_mode_upload_sends_bearer_and_declared_hash() {
    use sha2::Digest as _;
    let principal = PrincipalId::generate();
    // A minimal PNG signature is enough for the CLI's magic-byte MIME check.
    let png: Vec<u8> = [
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..],
        &[0u8; 32][..],
    ]
    .concat();
    let sha256 = hex::encode(sha2::Sha256::digest(&png));
    let descriptor = serde_json::json!({
        "url": "http://relay/media/x.png",
        "sha256": sha256,
        "size": png.len(),
        "type": "image/png",
        "uploaded": 0,
    })
    .to_string();
    let (url, seen) = fake_relay(me_ok(&principal), (StatusCode::OK, descriptor)).await;
    let client = BuzzClient::with_token(url, Secret::new("bzs_x".into()))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pic.png");
    std::fs::write(&path, &png).unwrap();

    let desc = client.upload_file(path.to_str().unwrap()).await.unwrap();
    assert_eq!(desc.sha256, sha256);
    let requests = bridge_requests(&seen);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "PUT");
    assert_eq!(requests[0].path, "/upload");
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer bzs_x"));
    assert_eq!(requests[0].x_sha256.as_deref(), Some(sha256.as_str()));
    assert_eq!(requests[0].body, png);
}

#[tokio::test]
async fn key_mode_media_keeps_blossom_auth() {
    let keys = Keys::generate();
    let (url, seen) = fake_relay(
        me_ok(&PrincipalId::generate()),
        (StatusCode::OK, "b".into()),
    )
    .await;
    let client = BuzzClient::new(url, keys, None, None).unwrap();
    client.download_media(&"ab".repeat(32)).await.unwrap();
    let auth = bridge_requests(&seen)[0].authorization.clone().unwrap();
    assert!(auth.starts_with("Nostr "), "{auth}");
}

/// A loopback broker stand-in that answers every request with `status_line`.
async fn fake_broker(status_line: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response =
                format!("{status_line}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    format!("http://{addr}")
}

fn broker_source(url: String) -> TokenSource {
    TokenSource::Broker {
        url,
        secret: Secret::new("s".into()),
    }
}

#[tokio::test]
async fn rate_limited_broker_is_a_network_error_not_auth() {
    let broker = fake_broker("HTTP/1.1 429 Too Many Requests").await;
    let err = BuzzClient::from_token_source("http://127.0.0.1:9".into(), broker_source(broker))
        .await
        .err()
        .expect("429 from the broker must fail");
    assert_eq!(exit_code(&err), 2, "{err}");
}

#[tokio::test]
async fn rejected_broker_secret_is_an_auth_error() {
    let broker = fake_broker("HTTP/1.1 401 Unauthorized").await;
    let err = BuzzClient::from_token_source("http://127.0.0.1:9".into(), broker_source(broker))
        .await
        .err()
        .expect("401 from the broker must fail");
    assert_eq!(exit_code(&err), 3, "{err}");
}
