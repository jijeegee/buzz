//! `buzz auth` and stored-session tests (centralized identity, Phase 2):
//! the login round trip through the real loopback listener, refresh rotation
//! persistence, terminal refresh → exit 3, the stored session as the last
//! credential source, logout durability, and the management retry-on-expiry.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::Response,
    Router,
};
use buzz_token_broker::{Secret, TokenSource};
use tokio::net::TcpListener;

use super::*;
use crate::auth_loopback::challenge_for;
use crate::auth_session::{
    relay_origin, session_access_token, IssuedTokens, MemoryStore, RefreshStorage, RefreshStore,
    SessionStore,
};
use crate::error::exit_code;

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

/// `"METHOD /path"` → queued `(status, body)` responses; the last one repeats.
type Routes = HashMap<String, VecDeque<(u16, String)>>;

#[derive(Clone, Default)]
struct FakeRelay {
    seen: Arc<Mutex<Vec<Seen>>>,
    routes: Arc<Mutex<Routes>>,
}

impl FakeRelay {
    fn route(&self, key: &str, status: u16, body: &str) {
        self.routes
            .lock()
            .unwrap()
            .entry(key.to_owned())
            .or_default()
            .push_back((status, body.to_owned()));
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn seen_path(&self, path: &str) -> Vec<Seen> {
        self.seen().into_iter().filter(|s| s.path == path).collect()
    }
}

async fn serve(relay: FakeRelay) -> String {
    let app = Router::new()
        .fallback(
            |State(relay): State<FakeRelay>,
             method: Method,
             uri: Uri,
             headers: HeaderMap,
             body: Body| async move {
                let body = to_bytes(body, 1 << 20).await.unwrap();
                relay.seen.lock().unwrap().push(Seen {
                    method: method.to_string(),
                    path: uri.path().to_owned(),
                    authorization: headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned),
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let key = format!("{method} {}", uri.path());
                let (status, body) = {
                    let mut routes = relay.routes.lock().unwrap();
                    match routes.get_mut(&key) {
                        Some(queue) if queue.len() > 1 => queue.pop_front().unwrap(),
                        Some(queue) => queue.front().cloned().unwrap(),
                        None => (404, r#"{"error":"not found","code":"not_found"}"#.into()),
                    }
                };
                Response::builder()
                    .status(StatusCode::from_u16(status).unwrap())
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap()
            },
        )
        .with_state(relay);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn principal_hex() -> String {
    buzz_core::principal::PrincipalId::generate().to_hex()
}

fn test_store(dir: &tempfile::TempDir) -> (SessionStore, Arc<MemoryStore>) {
    let keyring = Arc::new(MemoryStore::default());
    let store = SessionStore::new(
        dir.path().join("buzz").join("session.json"),
        keyring.clone() as Arc<dyn RefreshStore>,
    );
    (store, keyring)
}

async fn seed(
    store: &SessionStore,
    relay_url: &str,
    principal: &str,
    access: &str,
    refresh: &str,
    expires_in: i64,
) {
    store
        .save_tokens(
            &relay_origin(relay_url).unwrap(),
            principal,
            Some("00000000-0000-0000-0000-000000000001".into()),
            &IssuedTokens {
                access: Secret::new(access.into()),
                refresh: Secret::new(refresh.into()),
                expires_in,
            },
        )
        .await
        .unwrap();
}

fn file_json(store: &SessionStore) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap()
}

// ── Login ────────────────────────────────────────────────────────────────

/// A "browser" that follows the start URL: records it, then calls the
/// loopback redirect with `code` and the given `state` (`None` = echo the
/// login's own state).
fn fake_browser(
    started: Arc<Mutex<Option<url::Url>>>,
    callback_state: Option<&'static str>,
) -> impl Fn(&str) -> bool + Sync {
    move |start: &str| {
        let url = url::Url::parse(start).unwrap();
        *started.lock().unwrap() = Some(url.clone());
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let state = callback_state
            .map(str::to_owned)
            .unwrap_or_else(|| params["state"].clone());
        let target = format!(
            "{}?code=bzl_logincode&state={}",
            params["redirect_uri"], state
        );
        tokio::spawn(async move {
            let _ = reqwest::get(target).await;
        });
        true
    }
}

#[tokio::test]
async fn login_round_trip_verifies_pkce_and_stores_refresh_in_keyring() {
    let relay = FakeRelay::default();
    let principal = principal_hex();
    relay.route(
        "POST /auth/oidc/complete",
        200,
        &serde_json::json!({
            "principal_id": principal,
            "device_id": "11111111-1111-1111-1111-111111111111",
            "access": "bzs_access1",
            "refresh": "bzr_refresh1",
            "expires_in": 3600,
        })
        .to_string(),
    );
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, keyring) = test_store(&dir);
    let started = Arc::new(Mutex::new(None));
    let browser = fake_browser(started.clone(), None);

    let out = login(
        &relay_url,
        &store,
        "google",
        "laptop (CLI)",
        &browser,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(out["principal_id"], principal);
    assert_eq!(out["refresh_storage"], "keyring");

    // The start URL carries the cli client, a loopback redirect and the device.
    let start = started.lock().unwrap().clone().unwrap();
    assert_eq!(start.path(), "/auth/oidc/google/start");
    let params: HashMap<String, String> = start.query_pairs().into_owned().collect();
    assert_eq!(params["client"], "cli");
    assert_eq!(params["device_name"], "laptop (CLI)");
    assert!(params["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert!(params["redirect_uri"].ends_with("/cb"));

    // complete got the loopback code and the verifier matching the challenge.
    let complete = relay.seen_path("/auth/oidc/complete");
    assert_eq!(complete.len(), 1);
    let body: serde_json::Value = serde_json::from_str(&complete[0].body).unwrap();
    assert_eq!(body["login_code"], "bzl_logincode");
    let verifier = body["code_verifier"].as_str().unwrap();
    assert_eq!(challenge_for(verifier), params["code_challenge"]);

    // Refresh in the credential store, never in the file; access cached.
    let origin = relay_origin(&relay_url).unwrap();
    assert_eq!(
        keyring.load(&origin).unwrap().unwrap().expose(),
        "bzr_refresh1"
    );
    let file = file_json(&store);
    let entry = &file["sessions"][&origin];
    assert_eq!(entry["access"], "bzs_access1");
    assert!(
        entry.get("refresh").is_none(),
        "refresh leaked to file: {file}"
    );
}

#[tokio::test]
async fn login_with_foreign_state_fails_and_stores_nothing() {
    let relay = FakeRelay::default();
    relay.route("POST /auth/oidc/complete", 200, "{}");
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _keyring) = test_store(&dir);
    let browser = fake_browser(
        Arc::new(Mutex::new(None)),
        Some("attacker-state-0123456789"),
    );

    let err = login(
        &relay_url,
        &store,
        "google",
        "x",
        &browser,
        Duration::from_secs(10),
    )
    .await
    .unwrap_err();
    assert_eq!(exit_code(&err), 3, "{err}");
    assert!(relay.seen_path("/auth/oidc/complete").is_empty());
    assert!(!store.path().exists());
}

#[tokio::test]
async fn login_falls_back_to_file_when_credential_store_is_unavailable() {
    let relay = FakeRelay::default();
    relay.route(
        "POST /auth/oidc/complete",
        200,
        &serde_json::json!({
            "principal_id": principal_hex(),
            "access": "bzs_a", "refresh": "bzr_r", "expires_in": 3600,
        })
        .to_string(),
    );
    let relay_url = serve(relay).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, keyring) = test_store(&dir);
    keyring.fail_saves.store(true, Ordering::SeqCst);
    let browser = fake_browser(Arc::new(Mutex::new(None)), None);
    let out = login(
        &relay_url,
        &store,
        "google",
        "x",
        &browser,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(out["refresh_storage"], "file");
    let origin = relay_origin(&relay_url).unwrap();
    assert_eq!(file_json(&store)["sessions"][&origin]["refresh"], "bzr_r");
}

// ── Refresh rotation ────────────────────────────────────────────────────

#[tokio::test]
async fn fresh_access_is_used_without_refreshing() {
    let relay = FakeRelay::default();
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(
        &store,
        &relay_url,
        &principal_hex(),
        "bzs_fresh",
        "bzr_r1",
        3600,
    )
    .await;
    let http = reqwest::Client::new();
    let (token, _) = session_access_token(&http, &relay_url, &store, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(token.expose(), "bzs_fresh");
    assert!(relay.seen().is_empty());
}

#[tokio::test]
async fn near_expiry_rotates_and_persists_the_new_refresh() {
    let relay = FakeRelay::default();
    relay.route(
        "POST /auth/refresh",
        200,
        r#"{"access":"bzs_new","refresh":"bzr_r2","expires_in":3600}"#,
    );
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, keyring) = test_store(&dir);
    seed(
        &store,
        &relay_url,
        &principal_hex(),
        "bzs_old",
        "bzr_r1",
        30,
    )
    .await;

    let http = reqwest::Client::new();
    let (token, _) = session_access_token(&http, &relay_url, &store, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(token.expose(), "bzs_new");
    let sent: serde_json::Value =
        serde_json::from_str(&relay.seen_path("/auth/refresh")[0].body).unwrap();
    assert_eq!(sent["refresh"], "bzr_r1");

    // Rule 5: the rotated refresh is durable and the access cache follows.
    let origin = relay_origin(&relay_url).unwrap();
    assert_eq!(keyring.load(&origin).unwrap().unwrap().expose(), "bzr_r2");
    assert_eq!(file_json(&store)["sessions"][&origin]["access"], "bzs_new");

    // A second run uses the cached new access: no second refresh.
    let (again, _) = session_access_token(&http, &relay_url, &store, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.expose(), "bzs_new");
    assert_eq!(relay.seen_path("/auth/refresh").len(), 1);
}

#[tokio::test]
async fn terminal_refresh_forgets_the_session_and_exits_3() {
    for code in ["refresh_reused", "token_revoked", "token_expired"] {
        let relay = FakeRelay::default();
        relay.route(
            "POST /auth/refresh",
            401,
            &format!(r#"{{"error":"x","code":"{code}"}}"#),
        );
        let relay_url = serve(relay).await;
        let dir = tempfile::tempdir().unwrap();
        let (store, keyring) = test_store(&dir);
        seed(&store, &relay_url, &principal_hex(), "bzs_old", "bzr_r1", 0).await;

        let err = session_access_token(&reqwest::Client::new(), &relay_url, &store, false)
            .await
            .unwrap_err();
        assert_eq!(exit_code(&err), 3, "{code}: {err}");
        assert!(err.to_string().contains("buzz auth login"), "{err}");
        let origin = relay_origin(&relay_url).unwrap();
        assert!(store.get(&origin).unwrap().is_none(), "{code}");
        assert!(keyring.load(&origin).unwrap().is_none(), "{code}");
    }
}

#[tokio::test]
async fn transient_refresh_failure_keeps_the_session_and_is_retryable() {
    let relay = FakeRelay::default();
    relay.route(
        "POST /auth/refresh",
        503,
        r#"{"error":"down","code":"unavailable"}"#,
    );
    let relay_url = serve(relay).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, keyring) = test_store(&dir);
    seed(&store, &relay_url, &principal_hex(), "bzs_old", "bzr_r1", 0).await;

    let err = session_access_token(&reqwest::Client::new(), &relay_url, &store, false)
        .await
        .unwrap_err();
    assert_eq!(exit_code(&err), 2, "{err}");
    let origin = relay_origin(&relay_url).unwrap();
    assert!(store.get(&origin).unwrap().is_some());
    assert_eq!(keyring.load(&origin).unwrap().unwrap().expose(), "bzr_r1");
}

// ── Credential order (production seam: build_client_with_session) ────────

fn me_body(principal: &str) -> String {
    serde_json::json!({ "principal_id": principal, "kind": "user", "bot": null }).to_string()
}

#[tokio::test]
async fn stored_session_is_the_last_credential_source() {
    let principal = principal_hex();
    let relay = FakeRelay::default();
    relay.route("GET /auth/me", 200, &me_body(&principal));
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(
        &store,
        &relay_url,
        &principal,
        "bzs_session",
        "bzr_r1",
        3600,
    )
    .await;
    let key = nostr::Keys::generate().secret_key().to_secret_hex();

    // 1. An explicit token wins over the stored session.
    let client = crate::build_client_with_session(
        relay_url.clone(),
        Some(TokenSource::AccessToken(Secret::new("bzs_env".into()))),
        None,
        None,
        Some(&store),
    )
    .await
    .unwrap();
    assert!(client.is_token_mode());
    assert_eq!(
        relay
            .seen_path("/auth/me")
            .last()
            .unwrap()
            .authorization
            .as_deref(),
        Some("Bearer bzs_env")
    );

    // 2. A configured private key wins over the stored session.
    let before = relay.seen().len();
    let client =
        crate::build_client_with_session(relay_url.clone(), None, Some(key), None, Some(&store))
            .await
            .unwrap();
    assert!(!client.is_token_mode());
    assert_eq!(
        relay.seen().len(),
        before,
        "key mode must not touch the session"
    );

    // 3. Nothing else configured: the stored session.
    let client =
        crate::build_client_with_session(relay_url.clone(), None, None, None, Some(&store))
            .await
            .unwrap();
    assert!(client.is_token_mode());
    assert_eq!(client.pubkey().to_hex(), principal);
    assert_eq!(
        relay
            .seen_path("/auth/me")
            .last()
            .unwrap()
            .authorization
            .as_deref(),
        Some("Bearer bzs_session")
    );
}

#[tokio::test]
async fn no_credential_and_no_session_points_at_auth_login() {
    let relay_url = serve(FakeRelay::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    let err = crate::build_client_with_session(relay_url, None, None, None, Some(&store))
        .await
        .err()
        .unwrap();
    assert_eq!(exit_code(&err), 3);
    assert!(err.to_string().contains("buzz auth login"), "{err}");
}

#[tokio::test]
async fn session_for_another_relay_is_not_used() {
    let relay_url = serve(FakeRelay::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(
        &store,
        "https://other.example",
        &principal_hex(),
        "bzs_x",
        "bzr_x",
        3600,
    )
    .await;
    let err = crate::build_client_with_session(relay_url, None, None, None, Some(&store))
        .await
        .err()
        .unwrap();
    assert!(err.to_string().contains("buzz auth login"), "{err}");
}

#[tokio::test]
async fn relay_reported_expiry_forces_one_refresh() {
    let principal = principal_hex();
    let relay = FakeRelay::default();
    relay.route(
        "GET /auth/me",
        401,
        r#"{"error":"x","code":"token_expired"}"#,
    );
    relay.route("GET /auth/me", 200, &me_body(&principal));
    relay.route(
        "POST /auth/refresh",
        200,
        r#"{"access":"bzs_new","refresh":"bzr_r2","expires_in":3600}"#,
    );
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(&store, &relay_url, &principal, "bzs_skewed", "bzr_r1", 3600).await;

    let client = crate::build_client_with_session(relay_url, None, None, None, Some(&store))
        .await
        .unwrap();
    assert!(client.is_token_mode());
    let me = relay.seen_path("/auth/me");
    assert_eq!(me.len(), 2);
    assert_eq!(me[1].authorization.as_deref(), Some("Bearer bzs_new"));
}

#[tokio::test]
async fn first_login_cannot_succeed_without_a_session_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
    std::fs::create_dir(
        store
            .path()
            .with_extension(format!("json.tmp{}", std::process::id())),
    )
    .unwrap();
    let result = store
        .save_tokens(
            "https://relay.test",
            "alice",
            None,
            &IssuedTokens {
                access: Secret::new("access".into()),
                refresh: Secret::new("refresh".into()),
                expires_in: 3600,
            },
        )
        .await;
    assert!(result.is_err(), "no discoverable session was persisted");
}

#[tokio::test]
async fn existing_keyring_pointer_recovers_when_file_is_unwritable() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    let origin = "https://relay.test";
    seed(&store, origin, "alice", "old_access", "old_refresh", 3600).await;
    let old = store.get(origin).unwrap().unwrap();
    std::fs::create_dir(
        store
            .path()
            .with_extension(format!("json.tmp{}", std::process::id())),
    )
    .unwrap();
    let storage = store
        .save_tokens(
            origin,
            "alice",
            old.device_id,
            &IssuedTokens {
                access: Secret::new("new_access".into()),
                refresh: Secret::new("new_refresh".into()),
                expires_in: 3600,
            },
        )
        .await
        .unwrap();
    assert_eq!(storage, RefreshStorage::Keyring);
    let current = store.get(origin).unwrap().unwrap();
    assert_eq!(
        store
            .load_refresh(origin, &current)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        "new_refresh"
    );
}

/// Records whether the session file held a refresh token in plaintext at the
/// moment the keyring was written.
struct PlaintextSpyStore {
    inner: MemoryStore,
    path: std::path::PathBuf,
    plaintext_at_save: Mutex<Vec<bool>>,
}
impl RefreshStore for PlaintextSpyStore {
    fn load(&self, origin: &str) -> Result<Option<Secret>, String> {
        self.inner.load(origin)
    }
    fn save(&self, origin: &str, token: &Secret) -> Result<(), String> {
        let on_disk = std::fs::read_to_string(&self.path).unwrap_or_default();
        self.plaintext_at_save
            .lock()
            .unwrap()
            .push(on_disk.contains(token.expose()));
        self.inner.save(origin, token)
    }
    fn delete(&self, origin: &str) -> Result<(), String> {
        self.inner.delete(origin)
    }
}

#[tokio::test]
async fn working_keyring_leaves_no_plaintext_refresh_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("buzz").join("session.json");
    let spy = Arc::new(PlaintextSpyStore {
        inner: MemoryStore::default(),
        path: path.clone(),
        plaintext_at_save: Mutex::new(Vec::new()),
    });
    let store = SessionStore::new(path.clone(), spy.clone());
    let origin = "https://relay.test";
    let device = Some("00000000-0000-0000-0000-000000000001".to_owned());
    let issue = |access: &str, refresh: &str| IssuedTokens {
        access: Secret::new(access.into()),
        refresh: Secret::new(refresh.into()),
        expires_in: 3600,
    };

    // First login: the crash-safe snapshot holds the refresh only until the
    // keyring has it.
    let first = store
        .save_tokens(origin, "alice", device.clone(), &issue("a1", "bzr_first"))
        .await
        .unwrap();
    assert_eq!(first, RefreshStorage::Keyring);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("bzr_first"), "plaintext copy removed");
    assert!(file_json(&store)["sessions"][origin]
        .get("refresh")
        .is_none());

    // Rotation over the keyring pointer never writes the refresh to the file.
    let rotated = store
        .save_tokens(origin, "alice", device, &issue("a2", "bzr_second"))
        .await
        .unwrap();
    assert_eq!(rotated, RefreshStorage::Keyring);
    assert_eq!(*spy.plaintext_at_save.lock().unwrap(), [true, false]);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("bzr_second") && on_disk.contains("\"a2\""));
    let current = store.get(origin).unwrap().unwrap();
    assert_eq!(
        store
            .load_refresh(origin, &current)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        "bzr_second"
    );
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, [std::ffi::OsString::from("session.json")]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

/// Fail the session-file commit after the keyring accepts a rotated token.
struct FailCommitStore {
    inner: MemoryStore,
    block_path: std::path::PathBuf,
}
impl RefreshStore for FailCommitStore {
    fn load(&self, origin: &str) -> Result<Option<Secret>, String> {
        self.inner.load(origin)
    }
    fn save(&self, origin: &str, token: &Secret) -> Result<(), String> {
        self.inner.save(origin, token)?;
        std::fs::create_dir_all(&self.block_path).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn delete(&self, origin: &str) -> Result<(), String> {
        self.inner.delete(origin)
    }
}

#[tokio::test]
async fn migration_commit_failure_keeps_rotated_refresh_discoverable() {
    let dir = tempfile::tempdir().unwrap();
    let (initial, keyring) = test_store(&dir);
    keyring.fail_saves.store(true, Ordering::SeqCst);
    let origin = "https://relay.test";
    seed(
        &initial,
        origin,
        "alice",
        "old_access",
        "consumed_refresh",
        3600,
    )
    .await;
    let failing = Arc::new(FailCommitStore {
        inner: MemoryStore::default(),
        block_path: initial
            .path()
            .with_extension(format!("json.tmp{}", std::process::id())),
    });
    let store = SessionStore::new(initial.path().to_owned(), failing);
    let result = store
        .save_tokens(
            origin,
            "alice",
            None,
            &IssuedTokens {
                access: Secret::new("new_access".into()),
                refresh: Secret::new("new_refresh".into()),
                expires_in: 3600,
            },
        )
        .await
        .unwrap();
    let reloaded = store.get(origin).unwrap().unwrap();
    assert_eq!(
        store
            .load_refresh(origin, &reloaded)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        "new_refresh"
    );
    assert_eq!(reloaded.access, "new_access");
    assert_eq!(result, RefreshStorage::File);
}

#[tokio::test]
async fn logout_refreshes_expired_access_and_retries_once() {
    let relay = FakeRelay::default();
    relay.route("POST /auth/logout", 401, r#"{"code":"token_expired"}"#);
    relay.route("POST /auth/logout", 204, "");
    relay.route(
        "POST /auth/refresh",
        200,
        r#"{"access":"bzs_new","refresh":"bzr_new","expires_in":3600}"#,
    );
    let url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(&store, &url, &principal_hex(), "bzs_old", "bzr_old", 3600).await;
    logout(&url, &store, false).await.unwrap();
    let calls = relay.seen_path("/auth/logout");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].authorization.as_deref(), Some("Bearer bzs_new"));
    assert!(store.get(&url).unwrap().is_none());
}

#[tokio::test]
async fn logout_keeps_session_when_refresh_401_has_no_terminal_code() {
    let relay = FakeRelay::default();
    relay.route("POST /auth/logout", 401, r#"{"code":"token_expired"}"#);
    relay.route("POST /auth/refresh", 401, "{}");
    let url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(&store, &url, &principal_hex(), "bzs_old", "bzr_old", 3600).await;
    assert!(logout(&url, &store, false).await.is_err());
    assert!(store.get(&url).unwrap().is_some());
    assert_eq!(relay.seen_path("/auth/refresh").len(), 1);
}

#[tokio::test]
async fn logout_does_not_forget_on_unknown_or_repeated_expired_401() {
    for code in ["unknown", "token_expired"] {
        let relay = FakeRelay::default();
        relay.route("POST /auth/logout", 401, &format!(r#"{{"code":"{code}"}}"#));
        relay.route(
            "POST /auth/refresh",
            200,
            r#"{"access":"bzs_new","refresh":"bzr_new","expires_in":3600}"#,
        );
        let url = serve(relay.clone()).await;
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = test_store(&dir);
        seed(&store, &url, &principal_hex(), "bzs_old", "bzr_old", 3600).await;
        assert!(logout(&url, &store, false).await.is_err());
        assert!(store.get(&url).unwrap().is_some());
        assert!(relay.seen_path("/auth/logout").len() <= 2);
    }
}

// ── Management commands and logout ──────────────────────────────────────

#[tokio::test]
async fn management_call_refreshes_once_on_expired_access() {
    let relay = FakeRelay::default();
    relay.route(
        "GET /auth/devices",
        401,
        r#"{"error":"x","code":"token_expired"}"#,
    );
    relay.route("GET /auth/devices", 200, "[]");
    relay.route(
        "POST /auth/refresh",
        200,
        r#"{"access":"bzs_new","refresh":"bzr_r2","expires_in":3600}"#,
    );
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(
        &store,
        &relay_url,
        &principal_hex(),
        "bzs_old",
        "bzr_r1",
        3600,
    )
    .await;

    let api = AuthApi::new(relay_url, Some(store), None).await.unwrap();
    let out = api
        .call(reqwest::Method::GET, "/auth/devices", None)
        .await
        .unwrap();
    assert_eq!(out, serde_json::json!([]));
    let calls = relay.seen_path("/auth/devices");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].authorization.as_deref(), Some("Bearer bzs_old"));
    assert_eq!(calls[1].authorization.as_deref(), Some("Bearer bzs_new"));
}

#[tokio::test]
async fn management_terminal_rejection_exits_3_without_refresh_loop() {
    let relay = FakeRelay::default();
    relay.route(
        "POST /auth/bots/revoke-all",
        401,
        r#"{"error":"x","code":"token_revoked"}"#,
    );
    let relay_url = serve(relay.clone()).await;
    let api = AuthApi::new(
        relay_url,
        None,
        Some(TokenSource::AccessToken(Secret::new("bzs_env".into()))),
    )
    .await
    .unwrap();
    let err = api
        .call(
            reqwest::Method::POST,
            "/auth/bots/revoke-all",
            Some(serde_json::json!({})),
        )
        .await
        .unwrap_err();
    assert_eq!(exit_code(&err), 3);
    assert_eq!(relay.seen().len(), 1);
}

#[tokio::test]
async fn management_without_any_token_points_at_login() {
    let relay_url = serve(FakeRelay::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    let api = AuthApi::new(relay_url, Some(store), None).await.unwrap();
    let err = api
        .call(reqwest::Method::GET, "/auth/devices", None)
        .await
        .unwrap_err();
    assert_eq!(exit_code(&err), 3);
    assert!(err.to_string().contains("buzz auth login"));
}

#[tokio::test]
async fn logout_revokes_then_forgets() {
    let relay = FakeRelay::default();
    relay.route("POST /auth/logout", 204, "");
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, keyring) = test_store(&dir);
    seed(
        &store,
        &relay_url,
        &principal_hex(),
        "bzs_a",
        "bzr_r1",
        3600,
    )
    .await;

    let out = logout(&relay_url, &store, false).await.unwrap();
    assert_eq!(out["relay_session"], "revoked");
    let calls = relay.seen_path("/auth/logout");
    assert_eq!(calls[0].method, "POST");
    assert_eq!(calls[0].authorization.as_deref(), Some("Bearer bzs_a"));
    let origin = relay_origin(&relay_url).unwrap();
    assert!(store.get(&origin).unwrap().is_none());
    assert!(keyring.load(&origin).unwrap().is_none());
}

#[tokio::test]
async fn logout_keeps_the_session_when_the_relay_cannot_revoke_it() {
    let relay = FakeRelay::default();
    relay.route("POST /auth/logout", 503, r#"{"error":"down"}"#);
    let relay_url = serve(relay).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(
        &store,
        &relay_url,
        &principal_hex(),
        "bzs_a",
        "bzr_r1",
        3600,
    )
    .await;

    let err = logout(&relay_url, &store, false).await.unwrap_err();
    assert_eq!(exit_code(&err), 2);
    let origin = relay_origin(&relay_url).unwrap();
    assert!(
        store.get(&origin).unwrap().is_some(),
        "an unrevoked session must stay so logout can be retried"
    );

    // --local-only forgets it anyway.
    logout(&relay_url, &store, true).await.unwrap();
    assert!(store.get(&origin).unwrap().is_none());
}

#[tokio::test]
async fn logout_of_an_already_ended_session_forgets_it() {
    let relay = FakeRelay::default();
    relay.route(
        "POST /auth/refresh",
        401,
        r#"{"error":"x","code":"token_revoked"}"#,
    );
    let relay_url = serve(relay.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    seed(&store, &relay_url, &principal_hex(), "bzs_a", "bzr_r1", 0).await;

    let out = logout(&relay_url, &store, false).await.unwrap();
    assert_eq!(out["relay_session"], "already_ended");
    assert!(relay.seen_path("/auth/logout").is_empty());
}

#[test]
fn start_url_encodes_every_parameter() {
    let url = start_url(
        "https://relay.example",
        "google",
        "st",
        "ch",
        "http://127.0.0.1:5000/cb",
        "my host & co",
    );
    let parsed = url::Url::parse(&url).unwrap();
    let q: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    assert_eq!(parsed.path(), "/auth/oidc/google/start");
    assert_eq!(q["redirect_uri"], "http://127.0.0.1:5000/cb");
    assert_eq!(q["device_name"], "my host & co");
    assert_eq!(q["client"], "cli");
    assert_eq!(q.get("identity_mode").map(String::as_str), Some("token"));
}

#[tokio::test]
async fn custody_session_is_not_adopted_as_cli_messaging_identity() {
    let relay = FakeRelay::default();
    relay.route(
        "POST /auth/oidc/complete",
        200,
        &serde_json::json!({
            "identity_mode":"key_backup", "principal_id":principal_hex(),
            "access":"bzs_account", "refresh":"bzr_account", "expires_in":3600,
        })
        .to_string(),
    );
    let relay_url = serve(relay).await;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = test_store(&dir);
    let browser = fake_browser(Arc::new(Mutex::new(None)), None);
    let error = login(
        &relay_url,
        &store,
        "google",
        "CLI",
        &browser,
        Duration::from_secs(10),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("desktop or mobile"));
    assert!(!store.path().exists());
}

#[test]
fn device_name_is_bounded_and_printable() {
    let name = default_device_name();
    assert!(!name.is_empty() && name.chars().count() <= 64);
    assert!(!name.chars().any(char::is_control));
}

#[test]
fn refresh_storage_names_are_stable() {
    assert_eq!(RefreshStorage::Keyring.as_str(), "keyring");
    assert_eq!(RefreshStorage::File.as_str(), "file");
}

/// Managed agents (acp sets `BUZZ_DISABLE_STORED_SESSION=1`) never fall back
/// to the human's stored login: the CLI's session lookup yields nothing, so a
/// missing agent credential is an auth failure (exit 3), not the human.
#[test]
fn managed_agents_never_get_the_stored_session() {
    let env = |value: &'static str| {
        move |name: &str| (name == "BUZZ_DISABLE_STORED_SESSION").then(|| value.to_owned())
    };
    assert!(crate::stored_session_for(env("1")).is_none());
    assert!(buzz_token_broker::stored_session_disabled(env("true")));
    assert!(!buzz_token_broker::stored_session_disabled(env("0")));
    assert!(!buzz_token_broker::stored_session_disabled(env("")));
    assert!(!buzz_token_broker::stored_session_disabled(|_| None));
}
