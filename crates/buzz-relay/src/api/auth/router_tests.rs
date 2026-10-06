//! Router-level gating of the `/auth/*` surface (plan §5.1 risk (c)), bound to
//! the production `build_router`. Infra-free: the database and Redis are
//! unroutable, so only the routing decision and pre-I/O checks are observed.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

use crate::identity::{AuthTokenConfig, IdentityRuntime};
use crate::state::AppState;

async fn state_with_token_auth(enabled: bool) -> Arc<AppState> {
    let mut state = crate::state::tests::test_state_with_database_url(
        "postgres://buzz:buzz_dev@127.0.0.1:1/buzz",
    ) // sadscan:disable np.postgres.1 -- closed port, never connects
    .await;
    let mut config = AuthTokenConfig::disabled("ws://localhost:3000");
    config.enabled = enabled;
    Arc::get_mut(&mut state).expect("sole reference").identity =
        Arc::new(IdentityRuntime::new(config));
    state
}

async fn status_and_body(
    state: Arc<AppState>,
    request: Request<Body>,
) -> (StatusCode, serde_json::Value) {
    let response = crate::router::build_router(state)
        .oneshot(request)
        .await
        .expect("router response");
    let status = response.status();
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

fn request(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost")
        .body(Body::empty())
        .expect("request")
}

#[tokio::test]
async fn custody_only_mounts_account_routes_without_enabling_token_messaging() {
    let mut state = state_with_token_auth(false).await;
    let mut config = AuthTokenConfig::disabled("https://relay.example");
    config.key_backup_master = Some(
        buzz_auth::key_backup::BackupMasterKey::from_hex("test-v1", &"42".repeat(32)).unwrap(),
    );
    Arc::get_mut(&mut state).unwrap().identity = Arc::new(IdentityRuntime::new(config));
    assert!(!state.identity.enabled());
    for (method, path) in [
        ("GET", "/auth/me"),
        ("GET", "/auth/key-backup"),
        ("POST", "/auth/key-backup/challenge"),
        ("POST", "/auth/key-backup"),
        ("POST", "/auth/key-backup/restore"),
    ] {
        let response = crate::router::build_router(Arc::clone(&state))
            .oneshot(request(method, path))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(response.headers()["cache-control"], "no-store", "{path}");
    }
}

#[tokio::test]
async fn disabled_custody_cannot_return_any_key() {
    let state = state_with_token_auth(true).await;
    let response = crate::router::build_router(state)
        .oneshot(request("POST", "/auth/key-backup/restore"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn auth_body_limit_errors_are_also_uncacheable() {
    let state = state_with_token_auth(true).await;
    let request = Request::builder()
        .method("POST")
        .uri("/auth/key-backup")
        .header("host", "localhost")
        .body(Body::from(vec![b'x'; super::AUTH_BODY_LIMIT + 1]))
        .unwrap();
    let response = crate::router::build_router(state)
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn unknown_auth_routes_are_uncacheable_even_with_sessions_disabled() {
    let state = state_with_token_auth(false).await;
    let response = crate::router::build_router(state)
        .oneshot(request("GET", "/auth/unknown-callback"))
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
}

/// With `AUTH_TOKEN_ENABLED=false` (the default) every `/auth/*` route —
/// OIDC (and with it the operator bootstrap), refresh, exchange, operators —
/// is unrouted: it answers exactly like a path that never existed (the
/// router's existing fallback), never like an auth handler. Removing the flag
/// check in `build_router` mounts the handlers and fails this test.
#[tokio::test]
async fn auth_surface_is_unrouted_when_token_auth_disabled() {
    let state = state_with_token_auth(false).await;
    let (control_status, control_body) = status_and_body(
        Arc::clone(&state),
        request("GET", "/definitely/not/a/route"),
    )
    .await;
    for (method, uri) in [
        ("GET", "/auth/oidc/google/start"),
        ("GET", "/auth/oidc/google/callback?state=x"),
        ("POST", "/auth/oidc/complete"),
        ("POST", "/auth/refresh"),
        ("GET", "/auth/me"),
        ("POST", "/auth/token/exchange"),
        ("GET", "/auth/operators"),
    ] {
        let (status, body) = status_and_body(Arc::clone(&state), request(method, uri)).await;
        assert_eq!(
            (status, &body),
            (control_status, &control_body),
            "{method} {uri} must be unrouted"
        );
        assert!(
            body.get("code").is_none(),
            "{method} {uri} reached an auth handler"
        );
    }
}

/// With the flag on the surface is mounted: an unauthenticated `/auth/me` is
/// a 401 with the stable `invalid_token` code, not a 404.
#[tokio::test]
async fn auth_surface_is_mounted_when_token_auth_enabled() {
    let state = state_with_token_auth(true).await;
    let (status, body) = status_and_body(Arc::clone(&state), request("GET", "/auth/me")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "invalid_token");

    let malformed = Request::builder()
        .method("GET")
        .uri("/auth/me")
        .header("host", "localhost")
        .header("authorization", "Bearer bzs_not-a-real-token")
        .body(Body::empty())
        .expect("request");
    let (status, body) = status_and_body(state, malformed).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body["code"], "invalid_token",
        "malformed tokens never reach the store"
    );
}

/// The bearer scheme name is case-insensitive (RFC 7235 §2.1); anything that
/// is not a bearer credential yields no token.
#[test]
fn bearer_scheme_is_case_insensitive() {
    use axum::http::{header, HeaderMap, HeaderValue};
    let token_of = |value: &str| {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(value).expect("header"),
        );
        super::bearer_token(&headers).map(|token| token.expose().to_owned())
    };
    for scheme in ["Bearer", "bearer", "BEARER", "bEaReR"] {
        assert_eq!(
            token_of(&format!("{scheme} bzs_x")).as_deref(),
            Some("bzs_x")
        );
    }
    assert_eq!(token_of("Basic bzs_x"), None);
    assert_eq!(token_of("Bearer"), None);
    assert_eq!(token_of("Bearer   "), None);
    assert_eq!(token_of("Bearerbzs_x"), None);
}

/// The web client's `/auth/cb` return page is the web bundle's `index.html`
/// when token auth is on, and an unknown path (404) when it is off.
#[tokio::test]
async fn web_auth_callback_serves_the_spa_only_with_token_auth() {
    let dir = std::env::temp_dir().join(format!("buzz-web-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("web dir");
    std::fs::write(dir.join("index.html"), "<!doctype html><title>web</title>").expect("index");
    for enabled in [true, false] {
        let mut state = state_with_token_auth(enabled).await;
        let inner = Arc::get_mut(&mut state).expect("sole reference");
        Arc::make_mut(&mut inner.config).web_dir = Some(dir.clone());
        let response = crate::router::build_router(state)
            .oneshot(request("GET", "/auth/cb?code=bzl_x&state=s"))
            .await
            .expect("router response");
        let status = response.status();
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .expect("body")
            .to_bytes();
        if enabled {
            assert_eq!(status, StatusCode::OK);
            assert!(String::from_utf8_lossy(&bytes).contains("<title>web</title>"));
        } else {
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
