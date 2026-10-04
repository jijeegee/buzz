//! Live centralized-identity tests (plan §5.1): real Postgres + Redis, the
//! production `build_router` served on a loopback port, a fake OIDC provider
//! injected through the provider registry, and real WebSocket clients.
//!
//! `#[ignore]`d like every other infrastructure test. Run with:
//! `BUZZ_TEST_DATABASE_URL=… BUZZ_TEST_REDIS_URL=… cargo test -p buzz-relay
//! --lib api::auth::live_tests -- --ignored --test-threads=1`
//! (`--test-threads=1` because the bootstrap test owns the global roster).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use buzz_auth::oidc::{OidcError, OidcIdentity, OidcProvider};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::Message;

use crate::identity::AuthTokenConfig;
use crate::state::AppState;

mod audio;
mod invites;
mod operators;
mod profile;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const REDIRECT_URI: &str = "http://127.0.0.1:9999/cb";
const GRACE_SECS: u64 = 2;

/// Fake IdP: the callback `code` is `subject|email|verified`.
struct TestProvider;

impl OidcProvider for TestProvider {
    fn name(&self) -> &str {
        "google"
    }

    fn authorization_url(&self, _callback_url: &str, state: &str, nonce: &str) -> String {
        format!("https://idp.test/authorize?state={state}&nonce={nonce}")
    }

    fn exchange_code<'a>(
        &'a self,
        code: &'a str,
        _callback_url: &'a str,
        _expected_nonce: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<OidcIdentity, OidcError>> {
        Box::pin(async move {
            let mut parts = code.split('|');
            let subject = parts.next().unwrap_or_default().to_owned();
            let email = parts.next().filter(|e| !e.is_empty()).map(str::to_owned);
            let email_verified = parts.next() == Some("true");
            Ok(OidcIdentity {
                subject,
                email,
                email_verified,
                name: Some("Test User".into()),
                picture: None,
            })
        })
    }
}

struct Instance {
    state: Arc<AppState>,
    addr: SocketAddr,
    host: String,
    http: reqwest::Client,
}

fn redis_url() -> String {
    std::env::var("BUZZ_TEST_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_owned())
}

async fn community(pool: &sqlx::PgPool) -> String {
    let host = format!("{}.auth.test", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(uuid::Uuid::new_v4())
        .bind(&host)
        .execute(pool)
        .await
        .expect("create community");
    host
}

async fn instance_with(host: &str, enabled: bool, bootstrap_email: Option<&str>) -> Instance {
    instance_configured(host, enabled, bootstrap_email, |_| {}).await
}

async fn instance_configured(
    host: &str,
    enabled: bool,
    bootstrap_email: Option<&str>,
    configure: impl FnOnce(&mut crate::config::Config),
) -> Instance {
    let mut config = crate::config::Config::for_test();
    config.database_url = crate::test_support::database_url();
    config.read_database_url = None;
    config.redis_url = redis_url();
    config.require_relay_membership = false;
    let mut auth = AuthTokenConfig::disabled("ws://127.0.0.1:3000");
    auth.enabled = enabled;
    auth.exchange_grace = Duration::from_secs(GRACE_SECS);
    auth.operator_bootstrap_email = bootstrap_email.map(str::to_owned);
    config.auth_token = auth.clone();
    configure(&mut config);

    let pool = sqlx::PgPool::connect(&config.database_url)
        .await
        .expect("connect Postgres");
    let db = buzz_db::Db::from_pool(pool.clone());
    let redis_pool = deadpool_redis::Config::from_url(&config.redis_url)
        .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        .expect("redis pool");
    let pubsub = Arc::new(
        buzz_pubsub::PubSubManager::new(&config.redis_url, redis_pool.clone())
            .await
            .expect("pubsub"),
    );
    let audit = buzz_audit::AuditService::new(pool.clone());
    let auth_service = buzz_auth::AuthService::new(config.auth.clone());
    let search = buzz_search::SearchService::new(pool.clone());
    let workflow_engine = Arc::new(buzz_workflow::WorkflowEngine::new(
        db.clone(),
        buzz_workflow::WorkflowConfig::default(),
    ));
    let media_storage = buzz_media::MediaStorage::new(&config.media).expect("media storage");
    let (state, _audit_shutdown) = AppState::new(
        config,
        db,
        redis_pool,
        audit,
        pubsub,
        auth_service,
        search,
        workflow_engine,
        nostr::Keys::generate(),
        media_storage,
    );
    let state = Arc::new(state);
    assert!(state.identity.enabled() == enabled);
    if enabled {
        // Same startup sequence as main.rs.
        crate::identity::init_relay_principal(&state)
            .await
            .expect("relay principal");
        state
            .identity
            .register_provider("google", Arc::new(TestProvider));
        crate::identity::spawn_revocation_consumer(Arc::clone(&state));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = crate::router::build_router(Arc::clone(&state));
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("http client");
    // Let the revocation subscriber connect before tests publish.
    tokio::time::sleep(Duration::from_millis(200)).await;
    Instance {
        state,
        addr,
        host: host.to_owned(),
        http,
    }
}

async fn instance(host: &str) -> Instance {
    instance_with(host, true, None).await
}

impl Instance {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(self.url(path)).header("host", &self.host)
    }

    /// GET with an encoded query (reqwest's `query` feature is not enabled).
    fn get_with(&self, path: &str, params: &[(&str, &str)]) -> reqwest::RequestBuilder {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in params {
            query.append_pair(key, value);
        }
        self.get(&format!("{path}?{}", query.finish()))
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.post(self.url(path)).header("host", &self.host)
    }

    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.delete(self.url(path)).header("host", &self.host)
    }

    async fn ws(&self) -> Ws {
        let mut request = format!("ws://{}/", self.addr)
            .into_client_request()
            .expect("ws request");
        request
            .headers_mut()
            .insert("host", self.host.parse().expect("host header"));
        let (mut ws, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("ws connect");
        let challenge = next_json(&mut ws).await;
        assert_eq!(challenge[0], "AUTH");
        ws
    }
}

async fn next_json(ws: &mut Ws) -> Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("frame within 10s")
            .expect("stream open")
            .expect("frame ok");
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).expect("json frame");
        }
    }
}

/// Next `OK` frame, skipping live EVENT/EOSE traffic from open subscriptions.
async fn next_ok(ws: &mut Ws) -> Value {
    loop {
        let frame = next_json(ws).await;
        if frame[0] == "OK" {
            return frame;
        }
    }
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .expect("send");
}

/// Wait until the server closes the socket; returns the NOTICEs seen first.
async fn wait_closed(ws: &mut Ws, within: Duration) -> Vec<String> {
    let mut notices = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Err(_) => panic!("socket still open after {within:?}; notices: {notices:?}"),
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return notices,
            Ok(Some(Ok(Message::Text(text)))) => {
                let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                if value[0] == "NOTICE" {
                    notices.push(value[1].as_str().unwrap_or_default().to_owned());
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

async fn auth_ws(ws: &mut Ws, token: &str) -> Value {
    send(ws, json!(["AUTH", {"token": token}])).await;
    next_json(ws).await
}

/// REQ round trip: the connection is open and authenticated.
async fn req_eose(ws: &mut Ws, sub: &str, filter: Value) -> Vec<Value> {
    send(ws, json!(["REQ", sub, filter])).await;
    let mut events = Vec::new();
    loop {
        let frame = next_json(ws).await;
        match frame[0].as_str() {
            Some("EVENT") if frame[1] == sub => events.push(frame[2].clone()),
            Some("EOSE") if frame[1] == sub => return events,
            Some("CLOSED") if frame[1] == sub => panic!("subscription closed: {frame}"),
            _ => {}
        }
    }
}

struct Login {
    principal: String,
    access: String,
    refresh: String,
}

fn verifier() -> String {
    let bytes: [u8; 32] = rand::random();
    hex::encode(bytes)[..50].to_owned()
}

fn query_param(location: &str, name: &str) -> Option<String> {
    let url = url::Url::parse(location).ok()?;
    url.query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

/// start → (provider) → callback; returns the login code.
async fn login_code(inst: &Instance, code: &str, verifier: &str) -> String {
    let state = verifier[..24].to_owned();
    let challenge = buzz_auth::token::pkce_s256_challenge(verifier);
    let start = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state.as_str()),
                ("code_challenge", challenge.as_str()),
                ("client", "cli"),
                ("redirect_uri", REDIRECT_URI),
                ("device_name", "test laptop"),
            ],
        )
        .send()
        .await
        .expect("start");
    assert_eq!(start.status(), 302, "start must redirect to the IdP");
    let location = start.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.starts_with("https://idp.test/authorize"));
    assert_eq!(
        query_param(&location, "state").as_deref(),
        Some(state.as_str())
    );

    let callback = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("code", code), ("state", state.as_str())],
        )
        .send()
        .await
        .expect("callback");
    assert_eq!(callback.status(), 302);
    let location = callback.headers()["location"].to_str().unwrap().to_owned();
    assert!(location.starts_with(REDIRECT_URI), "{location}");
    assert_eq!(
        query_param(&location, "state").as_deref(),
        Some(state.as_str())
    );
    query_param(&location, "code").expect("login code in redirect")
}

async fn complete(inst: &Instance, login_code: &str, verifier: &str) -> Login {
    let response = inst
        .post("/auth/oidc/complete")
        .json(&json!({"login_code": login_code, "code_verifier": verifier}))
        .send()
        .await
        .expect("complete");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json");
    Login {
        principal: body["principal_id"].as_str().unwrap().to_owned(),
        access: body["access"].as_str().unwrap().to_owned(),
        refresh: body["refresh"].as_str().unwrap().to_owned(),
    }
}

/// The per-IP login limits (30 starts / 10 completes per minute) are shared
/// by every test on 127.0.0.1; reset them so the suite measures behavior,
/// not its own login volume.
async fn reset_login_limits(inst: &Instance) {
    let mut redis = inst.state.redis_pool.get().await.expect("redis");
    let _: () = redis::cmd("DEL")
        .arg("buzz:ratelimit:auth:login:start:127.0.0.1")
        .arg("buzz:ratelimit:auth:login:complete:127.0.0.1")
        .query_async(&mut redis)
        .await
        .expect("reset login limits");
}

async fn login(inst: &Instance, subject: &str, email: &str, verified: bool) -> Login {
    reset_login_limits(inst).await;
    let verifier = verifier();
    let code = format!("{subject}|{email}|{verified}");
    let login_code = login_code(inst, &code, &verifier).await;
    complete(inst, &login_code, &verifier).await
}

async fn new_user(inst: &Instance) -> Login {
    let subject = uuid::Uuid::new_v4().to_string();
    login(inst, &subject, "", false).await
}

async fn create_bot(inst: &Instance, owner: &Login, host: &str) -> String {
    let response = inst
        .post("/auth/bots")
        .bearer_auth(&owner.access)
        .json(&json!({"display_name": "helper", "host": host}))
        .send()
        .await
        .expect("create bot");
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.expect("json");
    body["bot_id"].as_str().unwrap().to_owned()
}

async fn bot_token(inst: &Instance, owner: &Login, bot: &str) -> String {
    let response = inst
        .post(&format!("/auth/bots/{bot}/token"))
        .bearer_auth(&owner.access)
        .send()
        .await
        .expect("bot token");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json");
    body["token"].as_str().unwrap().to_owned()
}

async fn me_status(inst: &Instance, token: &str) -> (u16, Value) {
    let response = inst
        .get("/auth/me")
        .bearer_auth(token)
        .send()
        .await
        .expect("me");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// OIDC (fake provider) start→callback→complete → WS token AUTH → draft →
/// OK with the server-computed id → REQ `authors`=principal returns it → the
/// bridge `/query` with Bearer returns it too. Plus the OIDC and draft
/// rejection rules on the same production seams.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn oidc_login_ws_draft_and_reads_round_trip() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;

    // OIDC: missing state → 400; callback with an unknown state → 400.
    let challenge = buzz_auth::token::pkce_s256_challenge(&verifier());
    let missing_state = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("code_challenge", challenge.as_str()),
                ("client", "cli"),
                ("redirect_uri", REDIRECT_URI),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(missing_state.status(), 400);
    let bad_redirect = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", "abcdefghijklmnopqrstuvwx"),
                ("code_challenge", challenge.as_str()),
                ("client", "cli"),
                ("redirect_uri", "https://evil.example/cb"),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(bad_redirect.status(), 400, "open redirect refused");
    let unknown_state = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("code", "x|y|true"), ("state", "never-started-state-0001")],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(unknown_state.status(), 400);

    // PKCE: a wrong verifier burns the one-time code.
    reset_login_limits(&inst).await;
    let good_verifier = verifier();
    let code = login_code(
        &inst,
        &format!("{}||false", uuid::Uuid::new_v4()),
        &good_verifier,
    )
    .await;
    let wrong = inst
        .post("/auth/oidc/complete")
        .json(&json!({"login_code": code, "code_verifier": verifier()}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 400);
    let replay = inst
        .post("/auth/oidc/complete")
        .json(&json!({"login_code": code, "code_verifier": good_verifier}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 400, "login codes are one-time");

    let user = new_user(&inst).await;
    let mut ws = inst.ws().await;
    assert_eq!(
        auth_ws(&mut ws, &user.access).await,
        json!(["OK", "auth", true, ""])
    );

    // Draft (no pubkey, no id, no sig) → stamped and stored.
    send(
        &mut ws,
        json!(["EVENT", {"kind": 1, "content": "hello token", "tags": []}]),
    )
    .await;
    let ok = next_json(&mut ws).await;
    assert_eq!(ok[0], "OK");
    assert_eq!(ok[2], true, "{ok}");
    let event_id = ok[1].as_str().unwrap().to_owned();
    assert_eq!(event_id.len(), 64);

    let events = req_eose(
        &mut ws,
        "mine",
        json!({"kinds": [1], "authors": [user.principal]}),
    )
    .await;
    let stored = events
        .iter()
        .find(|e| e["id"] == event_id)
        .expect("stamped event returned by REQ (not dropped by the row mapper)");
    assert_eq!(stored["pubkey"], user.principal);
    assert_eq!(stored["content"], "hello token");

    let queried: Value = inst
        .post("/query")
        .bearer_auth(&user.access)
        .json(&json!([{"kinds": [1], "authors": [user.principal]}]))
        .send()
        .await
        .expect("query")
        .json()
        .await
        .expect("json");
    assert!(
        queried
            .as_array()
            .is_some_and(|events| events.iter().any(|e| e["id"] == event_id)),
        "POST /query with Bearer returns the stamped event: {queried}"
    );

    // Bridge Bearer draft submit.
    let submitted: Value = inst
        .post("/events")
        .bearer_auth(&user.access)
        .json(&json!({"kind": 1, "content": "via http"}))
        .send()
        .await
        .expect("events")
        .json()
        .await
        .expect("json");
    assert_eq!(submitted["accepted"], true, "{submitted}");

    // Foreign pubkey → unchanged rejection text, even for the owner's own bot.
    let bot = create_bot(&inst, &user, "headless").await;
    for foreign in [
        bot.as_str(),
        &buzz_core::principal::PrincipalId::generate().to_hex(),
    ] {
        send(
            &mut ws,
            json!(["EVENT", {"kind": 1, "content": "x", "pubkey": foreign}]),
        )
        .await;
        let ok = next_ok(&mut ws).await;
        assert_eq!(ok[2], false);
        assert_eq!(
            ok[3],
            "invalid: event pubkey does not match authenticated identity"
        );
    }
    // Client kind 0 is blocked on token connections.
    send(&mut ws, json!(["EVENT", {"kind": 0, "content": "{}"}])).await;
    let ok = next_ok(&mut ws).await;
    assert_eq!(ok[3], "blocked: profile is managed via /auth/profile");
    // B7 regression guard: a 40002 without an h tag keeps today's rejection.
    send(&mut ws, json!(["EVENT", {"kind": 40002, "content": "dm"}])).await;
    let ok = next_ok(&mut ws).await;
    assert_eq!(ok[2], false);
    assert_eq!(
        ok[3],
        "invalid: channel-scoped events must include an h tag"
    );
}

/// B2 through the production `POST /auth/refresh` handler.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn refresh_rotation_replay_and_reuse_detection() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    let user = new_user(&inst).await;

    let refresh = |token: String| {
        let request = inst.post("/auth/refresh").json(&json!({"refresh": token}));
        async move {
            let response = request.send().await.expect("refresh");
            let status = response.status().as_u16();
            (
                status,
                response.json::<Value>().await.unwrap_or(Value::Null),
            )
        }
    };

    // Unknown token: invalid_token, nothing changes.
    let (status, body) = refresh(format!("bzr_{}", "A".repeat(43))).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("invalid_token"))
    );

    let (status, r2) = refresh(user.refresh.clone()).await;
    assert_eq!(status, 200);
    let r2_refresh = r2["refresh"].as_str().unwrap().to_owned();
    assert_ne!(r2_refresh, user.refresh);

    // Retry of R1 inside the replay window: same R2, session intact.
    let (status, retry) = refresh(user.refresh.clone()).await;
    assert_eq!(status, 200);
    assert_eq!(
        retry["refresh"], r2["refresh"],
        "replay cache returns the same rotation"
    );
    assert_eq!(
        me_status(&inst, r2["access"].as_str().unwrap()).await.0,
        200
    );

    // Past the replay window: R1 again is theft.
    let r1_hash = buzz_auth::hash_token(&user.refresh);
    let mut redis = inst.state.redis_pool.get().await.expect("redis");
    let _: () = redis::cmd("DEL")
        .arg(format!("buzz:auth:refresh:replay:{}", hex::encode(r1_hash)))
        .query_async(&mut redis)
        .await
        .expect("drop replay cache");
    sqlx::query(
        "UPDATE refresh_tokens SET used_at = now() - interval '11 seconds' WHERE token_hash = $1",
    )
    .bind(r1_hash.as_slice())
    .execute(&pool)
    .await
    .expect("age R1");
    let (status, body) = refresh(user.refresh.clone()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("refresh_reused"))
    );
    let session_revoked: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT s.revoked_at FROM refresh_tokens rt JOIN sessions s ON s.id = rt.session_id \
         WHERE rt.token_hash = $1",
    )
    .bind(r1_hash.as_slice())
    .fetch_one(&pool)
    .await
    .expect("session");
    assert!(session_revoked.is_some(), "reuse must revoke the session");
    assert_eq!(
        me_status(&inst, r2["access"].as_str().unwrap()).await.1["code"],
        "token_revoked"
    );
    let (status, body) = refresh(r2_refresh).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("token_revoked"))
    );
}

/// B3: exchange + same-connection re-AUTH keeps the socket; a socket that
/// does not re-AUTH closes when the superseded token's grace ends; a
/// different principal on re-AUTH is refused and closed. Plus the exchange
/// replay cache, 409 `token_superseded`, and the 2/min exchange limit.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn exchange_reauth_binding_swap_and_grace_expiry() {
    // Several real DB/HTTP round trips precede the cache-miss retry. Keep
    // them inside grace on slower hosts; expiry is still explicitly tested.
    const GRACE_SECS: u64 = 10;
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance_configured(&host, true, None, |config| {
        config.auth_token.exchange_grace = Duration::from_secs(GRACE_SECS);
    })
    .await;
    let owner = new_user(&inst).await;

    let exchange = |token: String| {
        let request = inst.post("/auth/token/exchange").bearer_auth(token);
        async move {
            let response = request.send().await.expect("exchange");
            let status = response.status().as_u16();
            (
                status,
                response.json::<Value>().await.unwrap_or(Value::Null),
            )
        }
    };

    // Bot 1: connection A re-AUTHs after the exchange and survives the grace.
    let bot1 = create_bot(&inst, &owner, "this_device").await;
    let t1 = bot_token(&inst, &owner, &bot1).await;
    let mut a = inst.ws().await;
    assert_eq!(auth_ws(&mut a, &t1).await[2], true);
    let (status, ex) = exchange(t1.clone()).await;
    assert_eq!(status, 200, "{ex}");
    let t2 = ex["token"].as_str().unwrap().to_owned();
    let (status, again) = exchange(t1.clone()).await;
    assert_eq!(status, 200);
    assert_eq!(
        again["token"], ex["token"],
        "replay cache returns the same token"
    );
    assert_eq!(
        auth_ws(&mut a, &t2).await,
        json!(["OK", "auth", true, ""]),
        "same-principal re-AUTH swaps the binding"
    );
    // A retry that misses the replay cache (it raced the winner's commit and
    // cache write) inside the 10 s window waits for the cache instead of
    // failing with 409: drop the entry, retry, and let the "winner" write it
    // back while the retry is polling.
    let replay_key = format!(
        "buzz:auth:exchange:replay:{}",
        hex::encode(buzz_auth::hash_token(&t1))
    );
    let mut redis = inst.state.redis_pool.get().await.expect("redis");
    let cached: String = redis::cmd("GETDEL")
        .arg(&replay_key)
        .query_async(&mut redis)
        .await
        .expect("take exchange replay");
    let retry = tokio::spawn(exchange(t1.clone()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _: () = redis::cmd("SET")
        .arg(&replay_key)
        .arg(&cached)
        .arg("EX")
        .arg(10)
        .query_async(&mut redis)
        .await
        .expect("restore exchange replay");
    let (status, body) = retry.await.expect("retry task");
    assert_eq!(
        (status, &body["token"]),
        (200, &ex["token"]),
        "a retry inside the replay window recovers the same token, not 409: {body}"
    );
    let (status, body) = exchange(t2.clone()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (429, Some("rate_limited")),
        "{body}"
    );

    // Outside the replay window the superseded token conflicts. Age the
    // supersede past the window (keeping the token itself unexpired) and
    // clear the cache and this bot's exchange budget.
    let _: () = redis::cmd("DEL")
        .arg(&replay_key)
        .arg(format!("buzz:ratelimit:auth:exchange:{bot1}"))
        .query_async(&mut redis)
        .await
        .expect("drop exchange replay and budget");
    sqlx::query(
        "UPDATE access_tokens SET superseded_at = now() - interval '11 seconds',                 expires_at = now() + interval '30 seconds'          WHERE token_hash = $1",
    )
    .bind(buzz_auth::hash_token(&t1).as_slice())
    .execute(&pool)
    .await
    .expect("age supersede");
    let (status, body) = exchange(t1.clone()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("token_superseded")),
        "{body}"
    );

    tokio::time::sleep(Duration::from_secs(GRACE_SECS + 2)).await;
    assert!(
        req_eose(&mut a, "alive", json!({"kinds": [1], "limit": 1}))
            .await
            .len()
            <= 1,
        "A answers REQ after the old token's grace: it is bound to the new token"
    );

    // Bot 2: connection B does not re-AUTH and is closed at grace end.
    let bot2 = create_bot(&inst, &owner, "this_device").await;
    let u1 = bot_token(&inst, &owner, &bot2).await;
    let mut b = inst.ws().await;
    assert_eq!(auth_ws(&mut b, &u1).await[2], true);
    let (status, _) = exchange(u1.clone()).await;
    assert_eq!(status, 200);
    let notices = wait_closed(&mut b, Duration::from_secs(GRACE_SECS + 5)).await;
    assert!(
        notices.iter().any(|n| n.starts_with("auth-expired")),
        "{notices:?}"
    );

    // A different principal's token on re-AUTH → refused and closed.
    let mut c = inst.ws().await;
    let u2 = bot_token(&inst, &owner, &bot2).await;
    assert_eq!(auth_ws(&mut c, &u2).await[2], true);
    assert_eq!(
        auth_ws(&mut c, &owner.access).await,
        json!(["OK", "auth", false, "auth-required: principal mismatch"])
    );
    wait_closed(&mut c, Duration::from_secs(5)).await;
}

/// Revocation fan-out matches the bound token hash across instances: logout
/// on X closes the logged-out session's socket on Y, while the same
/// principal's socket bound to another session's token stays open.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn revocation_closes_bound_socket_on_other_instance_only() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let x = instance(&host).await;
    let y = instance(&host).await;
    let subject = uuid::Uuid::new_v4().to_string();
    let laptop = login(&x, &subject, "", false).await;
    let phone = login(&x, &subject, "", false).await;
    assert_eq!(laptop.principal, phone.principal);

    let mut doomed = y.ws().await;
    assert_eq!(auth_ws(&mut doomed, &laptop.access).await[2], true);
    let mut survivor = y.ws().await;
    assert_eq!(auth_ws(&mut survivor, &phone.access).await[2], true);

    let logout = x
        .post("/auth/logout")
        .bearer_auth(&laptop.access)
        .send()
        .await
        .expect("logout");
    assert_eq!(logout.status(), 204);
    let notices = wait_closed(&mut doomed, Duration::from_secs(10)).await;
    assert!(
        notices.iter().any(|n| n.starts_with("auth-revoked")),
        "{notices:?}"
    );
    req_eose(
        &mut survivor,
        "still-here",
        json!({"kinds": [1], "limit": 1}),
    )
    .await;
}

/// Rule 2: every revocation path revokes the tokens it covers.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn every_revocation_path_revokes_its_tokens() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    let owner = new_user(&inst).await;

    // Stop.
    let bot = create_bot(&inst, &owner, "this_device").await;
    let token = bot_token(&inst, &owner, &bot).await;
    let stop = inst
        .post(&format!("/auth/bots/{bot}/revoke"))
        .bearer_auth(&owner.access)
        .send()
        .await
        .unwrap();
    assert_eq!(stop.status(), 204);
    assert_eq!(me_status(&inst, &token).await.1["code"], "token_revoked");

    // Reissue revokes the previous bzb_.
    let first = bot_token(&inst, &owner, &bot).await;
    let second = bot_token(&inst, &owner, &bot).await;
    assert_eq!(me_status(&inst, &first).await.1["code"], "token_revoked");
    assert_eq!(me_status(&inst, &second).await.0, 200);

    // Revoke-all covers hosted and headless tokens; bots survive.
    let headless = create_bot(&inst, &owner, "headless").await;
    let issued: Value = inst
        .post(&format!("/auth/bots/{headless}/headless-token"))
        .bearer_auth(&owner.access)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let headless_token = issued["token"].as_str().unwrap().to_owned();
    assert_eq!(me_status(&inst, &headless_token).await.0, 200);
    let revoke_all = inst
        .post("/auth/bots/revoke-all")
        .bearer_auth(&owner.access)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke_all.status(), 204);
    assert_eq!(me_status(&inst, &second).await.1["code"], "token_revoked");
    assert_eq!(
        me_status(&inst, &headless_token).await.1["code"],
        "token_revoked"
    );

    // Delete.
    let token = bot_token(&inst, &owner, &bot).await;
    let deleted = inst
        .delete(&format!("/auth/bots/{bot}"))
        .bearer_auth(&owner.access)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 204);
    // The deleted bot's principal is disabled, which is checked first.
    assert_eq!(
        me_status(&inst, &token).await.1["code"],
        "principal_disabled"
    );
    let reason: Option<String> =
        sqlx::query_scalar("SELECT revoked_reason FROM access_tokens WHERE token_hash = $1")
            .bind(buzz_auth::hash_token(&token).as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        reason.as_deref(),
        Some("bot_deleted"),
        "the token row itself is revoked"
    );

    // Remote device logout revokes that session and the bots it hosts.
    let other = login(&inst, &uuid::Uuid::new_v4().to_string(), "", false).await;
    let hosted = create_bot(&inst, &other, "this_device").await;
    let hosted_token = bot_token(&inst, &other, &hosted).await;
    let devices: Value = inst
        .get("/auth/devices")
        .bearer_auth(&other.access)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device = devices.as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let revoked = inst
        .delete(&format!("/auth/devices/{device}"))
        .bearer_auth(&other.access)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 204);
    assert_eq!(
        me_status(&inst, &other.access).await.1["code"],
        "token_revoked"
    );
    assert_eq!(
        me_status(&inst, &hosted_token).await.1["code"],
        "token_revoked"
    );

    // Revoke other sessions keeps the caller's own session and bot tokens.
    let subject = uuid::Uuid::new_v4().to_string();
    let keep = login(&inst, &subject, "", false).await;
    let drop = login(&inst, &subject, "", false).await;
    let keep_bot = create_bot(&inst, &keep, "this_device").await;
    let keep_bot_token = bot_token(&inst, &keep, &keep_bot).await;
    let others = inst
        .post("/auth/sessions/revoke-others")
        .bearer_auth(&keep.access)
        .send()
        .await
        .unwrap();
    assert_eq!(others.status(), 204);
    assert_eq!(
        me_status(&inst, &drop.access).await.1["code"],
        "token_revoked"
    );
    assert_eq!(me_status(&inst, &keep.access).await.0, 200);
    assert_eq!(
        me_status(&inst, &keep_bot_token).await.0,
        200,
        "bots untouched"
    );

    // Account deletion disables the principal and revokes everything.
    let gone = inst
        .delete("/auth/account")
        .bearer_auth(&keep.access)
        .send()
        .await
        .unwrap();
    assert_eq!(gone.status(), 204);
    assert_eq!(
        me_status(&inst, &keep_bot_token).await.1["code"],
        "token_revoked"
    );
}

/// B8: operator bootstrap on every login — verified email match into an
/// empty DB roster only, and no lock-out for users who registered before
/// the setting existed.
#[tokio::test]
#[ignore = "requires Postgres + Redis — owns the global relay_operators roster"]
async fn operator_bootstrap_rules() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    sqlx::query("DELETE FROM relay_operators")
        .execute(&pool)
        .await
        .expect("clear roster");
    let host = community(&pool).await;
    let boss = format!("boss-{}@example.test", uuid::Uuid::new_v4().simple());
    let inst = instance_with(&host, true, Some(&boss.to_uppercase())).await;
    let is_operator = |principal: String| {
        let pool = pool.clone();
        async move {
            let bytes = hex::decode(principal).unwrap();
            sqlx::query_scalar::<_, String>("SELECT role FROM relay_operators WHERE pubkey = $1")
                .bind(bytes)
                .fetch_optional(&pool)
                .await
                .unwrap()
                .as_deref()
                == Some("operator")
        }
    };

    let s1 = uuid::Uuid::new_v4().to_string();
    let unverified = login(&inst, &s1, &boss, false).await;
    assert!(
        !is_operator(unverified.principal.clone()).await,
        "unverified email never bootstraps"
    );
    let other = login(
        &inst,
        &uuid::Uuid::new_v4().to_string(),
        "x@example.test",
        true,
    )
    .await;
    assert!(!is_operator(other.principal).await, "email must match");
    let granted = login(&inst, &s1, &boss, true).await;
    assert!(
        is_operator(granted.principal.clone()).await,
        "verified match into empty roster"
    );
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relay_operator_audit WHERE target_pubkey = $1 AND actor_pubkey = $2",
    )
    .bind(hex::decode(&granted.principal).unwrap())
    .bind(
        inst.state
            .identity
            .relay_principal()
            .unwrap()
            .as_bytes()
            .as_slice(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        audited, 1,
        "granted by the relay principal through the audited path"
    );
    let second = login(&inst, &uuid::Uuid::new_v4().to_string(), &boss, true).await;
    assert!(
        !is_operator(second.principal).await,
        "non-empty roster: no grant"
    );
    let me = me_status(&inst, &granted.access).await.1;
    assert_eq!(me["operator"], true);

    // Registered before the setting existed → granted on a later login.
    sqlx::query("DELETE FROM relay_operators")
        .execute(&pool)
        .await
        .unwrap();
    let late_email = format!("late-{}@example.test", uuid::Uuid::new_v4().simple());
    let plain = instance_with(&host, true, None).await;
    let s4 = uuid::Uuid::new_v4().to_string();
    let before = login(&plain, &s4, &late_email, true).await;
    assert!(!is_operator(before.principal.clone()).await);
    let configured = instance_with(&host, true, Some(&late_email)).await;
    let after = login(&configured, &s4, &late_email, true).await;
    assert_eq!(after.principal, before.principal);
    assert!(is_operator(after.principal.clone()).await, "no lock-out");

    // Operator endpoints run on the existing roster store.
    let list = configured
        .get("/auth/operators")
        .bearer_auth(&after.access)
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let last = configured
        .delete(&format!("/auth/operators/{}", after.principal))
        .bearer_auth(&after.access)
        .send()
        .await
        .unwrap();
    assert_eq!(last.status(), 409, "the last operator cannot remove itself");
    let other_session = configured
        .get("/auth/operators")
        .bearer_auth(&before.access)
        .send()
        .await
        .unwrap();
    assert_eq!(
        other_session.status(),
        200,
        "same principal, other session: still operator"
    );
    sqlx::query("DELETE FROM relay_operators")
        .execute(&pool)
        .await
        .unwrap();
}

/// Two simultaneous first logins for one subject converge on one principal.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn concurrent_first_callbacks_create_one_principal() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    let subject = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        login(&inst, &subject, "", false),
        login(&inst, &subject, "", false)
    );
    assert_eq!(a.principal, b.principal);
    let identities: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM identities WHERE provider = 'google' AND subject = $1",
    )
    .bind(&subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(identities, 1);
}

/// The bridge Bearer branch exists only with the flag on: with it off, a
/// Bearer request is exactly today's unauthenticated request.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn bridge_bearer_branch_is_gated_by_flag() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let on = instance(&host).await;
    let off = instance_with(&host, false, None).await;
    let user = new_user(&on).await;
    let filter = json!([{"kinds": [1], "limit": 1}]);

    let enabled = on
        .post("/query")
        .bearer_auth(&user.access)
        .json(&filter)
        .send()
        .await
        .unwrap();
    assert_eq!(enabled.status(), 200);
    let disabled = off
        .post("/query")
        .bearer_auth(&user.access)
        .json(&filter)
        .send()
        .await
        .unwrap();
    assert_eq!(disabled.status(), 401);
    let body: Value = disabled.json().await.unwrap();
    assert_eq!(
        body["error"], "missing Nostr auth",
        "flag off: key-auth path unchanged"
    );
    let bad = on
        .post("/query")
        .bearer_auth(format!("bzs_{}", "B".repeat(43)))
        .json(&filter)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 401);
    assert_eq!(bad.json::<Value>().await.unwrap()["code"], "invalid_token");
}

/// Unauthenticated-endpoint limits key on the peer IP only when the listener
/// supplies one; without it (UDS listener) a single deployment-wide bucket
/// with its own, larger cap applies instead of every client sharing the
/// per-IP cap. Falsifying mutations: key the no-peer case on a constant IP
/// with the per-IP cap → the third global request is refused; drop the IP
/// from the key → the two peers share one bucket.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn unauthenticated_limits_use_peer_ip_or_a_global_bucket() {
    use axum::extract::ConnectInfo;
    use axum::http::Extensions;

    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    let scope = format!("auth:test:{}", uuid::Uuid::new_v4().simple());

    let peer = |ip: [u8; 4]| {
        let mut extensions = Extensions::new();
        extensions.insert(ConnectInfo(SocketAddr::from((ip, 4000))));
        extensions
    };
    let check = |extensions: Extensions| {
        let state = Arc::clone(&inst.state);
        let scope = scope.clone();
        async move {
            super::rate_limit_client(
                &state,
                &extensions,
                &axum::http::HeaderMap::new(),
                &scope,
                1,
                3,
            )
            .await
            .is_ok()
        }
    };

    // Per-IP: one each, independently.
    assert!(check(peer([10, 0, 0, 1])).await);
    assert!(!check(peer([10, 0, 0, 1])).await, "per-IP cap is 1");
    assert!(
        check(peer([10, 0, 0, 2])).await,
        "another peer has its own bucket"
    );

    // No peer IP: the global cap (3), not the per-IP cap (1).
    for attempt in 1..=3 {
        assert!(check(Extensions::new()).await, "global request {attempt}");
    }
    assert!(!check(Extensions::new()).await, "global cap is 3");
}

/// Admin API Bearer branch (plan §4.4, B6): the header scheme selects token
/// auth even in `nip98` admin mode; an operator session gets its roster role,
/// a non-operator 403, a bot token 403, a bad token 401, and with the flag off
/// the Bearer header is not token auth at all.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn admin_api_accepts_operator_bearer_only() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let admin_host = format!("admin-{}.localhost", uuid::Uuid::new_v4().simple());
    let admin = crate::config::AdminConfig {
        host: admin_host.clone(),
        auth: crate::config::AdminAuth::Nip98,
        web_dir: None,
    };
    let inst = instance_configured(&host, true, None, {
        let admin = admin.clone();
        move |config| {
            config.relay_operator_pubkeys = Vec::new();
            config.relay_owner_pubkey = None;
            config.admin = Some(admin);
        }
    })
    .await;
    let operator = new_user(&inst).await;
    let member = new_user(&inst).await;
    let relay = inst
        .state
        .identity
        .relay_principal()
        .expect("relay principal");
    sqlx::query("INSERT INTO relay_operators (pubkey, role, added_by) VALUES ($1, 'operator', $2)")
        .bind(hex::decode(&operator.principal).unwrap())
        .bind(relay.as_bytes().to_vec())
        .execute(&pool)
        .await
        .expect("roster operator");

    let probe = |token: Option<String>| {
        let mut request = inst
            .http
            .get(inst.url("/api/admin/v1/probe"))
            .header("host", &admin_host);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        async move {
            let response = request.send().await.expect("probe");
            let status = response.status().as_u16();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            (status, body)
        }
    };

    let (status, body) = probe(Some(operator.access.clone())).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["role"], "operator");
    assert_eq!(body["source"], "db");

    assert_eq!(probe(Some(member.access.clone())).await.0, 403);
    let bot = create_bot(&inst, &operator, "headless").await;
    // Even a rostered bot id is refused: admin Bearer takes user sessions only.
    sqlx::query("INSERT INTO relay_operators (pubkey, role, added_by) VALUES ($1, 'operator', $2)")
        .bind(hex::decode(&bot).unwrap())
        .bind(relay.as_bytes().to_vec())
        .execute(&pool)
        .await
        .expect("roster bot");
    let token = bot_token_or_headless(&inst, &operator, &bot).await;
    assert_eq!(
        probe(Some(token)).await.0,
        403,
        "bot tokens are never staff"
    );
    assert_eq!(probe(Some(format!("bzs_{}", "A".repeat(43)))).await.0, 401);
    assert_eq!(probe(None).await.0, 401, "no credential is still 401");

    // Flag off: the same Bearer is not token auth (NIP-98 mode rejects it).
    let off = instance_configured(&host, false, None, move |config| {
        config.relay_operator_pubkeys = Vec::new();
        config.relay_owner_pubkey = None;
        config.admin = Some(admin);
    })
    .await;
    let response = off
        .http
        .get(off.url("/api/admin/v1/probe"))
        .header("host", &admin_host)
        .bearer_auth(&operator.access)
        .send()
        .await
        .expect("probe off");
    assert_eq!(response.status(), 401);

    sqlx::query("DELETE FROM relay_operators WHERE pubkey = ANY($1)")
        .bind(vec![
            hex::decode(&operator.principal).unwrap(),
            hex::decode(&bot).unwrap(),
        ])
        .execute(&pool)
        .await
        .expect("clean roster");
}

/// `BUZZ_ADMIN_AUTH=disabled` is always read-only (plan §4.4): an operator's
/// Bearer token must not resolve a principal there, so a staffing mutation
/// stays 403 and writes nothing, while reads still pass.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn admin_bearer_is_inert_under_disabled_admin_auth() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let admin_host = format!("admin-{}.localhost", uuid::Uuid::new_v4().simple());
    let admin = crate::config::AdminConfig {
        host: admin_host.clone(),
        auth: crate::config::AdminAuth::Disabled,
        web_dir: None,
    };
    let inst = instance_configured(&host, true, None, move |config| {
        config.relay_operator_pubkeys = Vec::new();
        config.relay_owner_pubkey = None;
        config.admin = Some(admin);
    })
    .await;
    let operator = new_user(&inst).await;
    let member = new_user(&inst).await;
    let relay = inst
        .state
        .identity
        .relay_principal()
        .expect("relay principal");
    sqlx::query("INSERT INTO relay_operators (pubkey, role, added_by) VALUES ($1, 'operator', $2)")
        .bind(hex::decode(&operator.principal).unwrap())
        .bind(relay.as_bytes().to_vec())
        .execute(&pool)
        .await
        .expect("roster operator");

    let response = inst
        .http
        .put(inst.url(&format!("/api/admin/v1/operators/{}", member.principal)))
        .header("host", &admin_host)
        .header("content-type", "application/json")
        .bearer_auth(&operator.access)
        .body(r#"{"role":"moderator"}"#)
        .send()
        .await
        .expect("upsert operator");
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    assert_eq!(
        status, 403,
        "disabled admin auth must refuse mutations: {body}"
    );

    let staffed: Option<(String,)> =
        sqlx::query_as("SELECT role FROM relay_operators WHERE pubkey = $1")
            .bind(hex::decode(&member.principal).unwrap())
            .fetch_optional(&pool)
            .await
            .expect("read roster");
    assert!(staffed.is_none(), "no roster row may be written");

    let probe = inst
        .http
        .get(inst.url("/api/admin/v1/probe"))
        .header("host", &admin_host)
        .bearer_auth(&operator.access)
        .send()
        .await
        .expect("probe");
    assert_eq!(probe.status(), 200, "reads still pass in disabled mode");
    let probe: Value = probe.json().await.expect("probe json");
    assert_eq!(probe["authMode"], "disabled");
    assert_eq!(probe["canAct"], false);

    sqlx::query("DELETE FROM relay_operators WHERE pubkey = ANY($1)")
        .bind(vec![
            hex::decode(&operator.principal).unwrap(),
            hex::decode(&member.principal).unwrap(),
        ])
        .execute(&pool)
        .await
        .expect("clean roster");
}

/// A token for `bot` usable by the admin/git tests: a headless bot gets a
/// `bzk_` headless token.
async fn bot_token_or_headless(inst: &Instance, owner: &Login, bot: &str) -> String {
    let response = inst
        .post(&format!("/auth/bots/{bot}/headless-token"))
        .bearer_auth(&owner.access)
        .send()
        .await
        .expect("headless token");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json");
    body["token"].as_str().unwrap().to_owned()
}

/// Git smart HTTP token branch (plan §4.4, B6): `Basic token:<t>` and
/// `Bearer <t>` authenticate; a revoked token and a missing credential get a
/// 401 that offers both the `Nostr` and `Basic` challenges.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn git_transport_accepts_basic_and_bearer_tokens() {
    use base64::Engine as _;
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    let owner = new_user(&inst).await;
    let bot = create_bot(&inst, &owner, "this_device").await;
    let token = bot_token(&inst, &owner, &bot).await;
    let path = "/git/no-such-owner/no-such-repo.git/info/refs?service=git-upload-pack";

    let get = |auth: Option<String>| {
        let mut request = inst.get(path);
        if let Some(auth) = auth {
            request = request.header("authorization", auth);
        }
        async move { request.send().await.expect("git request") }
    };

    let basic = |t: &str| {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("token:{t}"))
        )
    };
    // Authenticated: past auth, the repo simply does not exist.
    for auth in [basic(&token), format!("Bearer {token}")] {
        let status = get(Some(auth)).await.status().as_u16();
        assert!(
            status != 401 && status != 403,
            "token must authenticate, got {status}"
        );
    }

    // Wrong username in Basic is not a token credential.
    let wrong_user = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("git:{token}"))
    );
    assert_eq!(get(Some(wrong_user)).await.status(), 401);

    // Missing credential: both challenges.
    let response = get(None).await;
    assert_eq!(response.status(), 401);
    let challenges: Vec<String> = response
        .headers()
        .get_all("www-authenticate")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect();
    assert!(
        challenges.iter().any(|c| c.starts_with("Nostr ")),
        "{challenges:?}"
    );
    assert!(
        challenges.iter().any(|c| c == "Basic realm=\"buzz\""),
        "{challenges:?}"
    );

    // Stop revokes the bot token: git now gets 401.
    let stop = inst
        .post(&format!("/auth/bots/{bot}/revoke"))
        .bearer_auth(&owner.access)
        .send()
        .await
        .expect("stop");
    assert_eq!(stop.status(), 204);
    assert_eq!(get(Some(basic(&token))).await.status(), 401);
}

/// Blossom media through the Bearer door (Phase 2): with the flag on a user
/// token authorizes reads (the request reaches the blob lookup: 404 for an
/// unknown blob) and uploads (past auth into storage); a revoked token is a
/// 401 with its code; with the flag off the Bearer header is ignored and the
/// kind-24242 path answers exactly as before. On a closed relay a non-member
/// token is refused and the same principal is admitted once it is a member.
/// Falsifying mutations: skip `media_token_principal` → token reads/uploads
/// get the Blossom 401; drop its membership call → the non-member read
/// reaches the 404.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn media_accepts_bearer_tokens_behind_the_flag() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let on = instance(&host).await;
    let off = instance_with(&host, false, None).await;
    let user = new_user(&on).await;
    let blob = format!("/media/{}", "ab".repeat(32));
    async fn head(inst: &Instance, path: &str, token: &str) -> u16 {
        inst.http
            .head(inst.url(path))
            .header("host", &inst.host)
            .bearer_auth(token)
            .send()
            .await
            .expect("head")
            .status()
            .as_u16()
    }

    assert_eq!(head(&on, &blob, &user.access).await, 404);
    let get = on
        .get(&blob)
        .bearer_auth(&user.access)
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 404, "token GET reaches the blob lookup");

    let body = b"not stored: storage is unreachable in this test".to_vec();
    let sha256 = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&body));
    let upload = on
        .http
        .put(on.url("/upload"))
        .header("host", &on.host)
        .bearer_auth(&user.access)
        .header("x-sha-256", &sha256)
        .body(body.clone())
        .send()
        .await
        .unwrap();
    let status = upload.status().as_u16();
    assert!(
        status != 401 && status != 403,
        "token upload passes auth (storage then fails), got {status}"
    );
    let missing_hash = on
        .http
        .put(on.url("/upload"))
        .header("host", &on.host)
        .bearer_auth(&user.access)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        missing_hash.status(),
        401,
        "token upload still requires X-SHA-256"
    );

    assert_eq!(
        head(&off, &blob, &user.access).await,
        401,
        "flag off: Bearer is not media auth"
    );

    let logout = on
        .post("/auth/logout")
        .bearer_auth(&user.access)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);
    let revoked = on
        .get(&blob)
        .bearer_auth(&user.access)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 401);
    assert_eq!(
        revoked.json::<Value>().await.unwrap()["code"],
        "token_revoked"
    );

    let closed = instance_configured(&host, true, None, |config| {
        config.require_relay_membership = true;
    })
    .await;
    let outsider = new_user(&closed).await;
    assert_eq!(
        head(&closed, &blob, &outsider.access).await,
        403,
        "non-member token is refused on a closed relay"
    );
    let community = crate::tenant::bind_community(&closed.state.db, &host)
        .await
        .expect("community")
        .community();
    closed
        .state
        .db
        .add_relay_member(community, &outsider.principal, "member", None)
        .await
        .expect("add member");
    assert_eq!(
        head(&closed, &blob, &outsider.access).await,
        404,
        "the same token is admitted once the principal is a member"
    );
}
