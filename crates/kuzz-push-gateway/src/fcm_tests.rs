//! FCM transport against a local mock of the Google token endpoint and the
//! FCM send API. The RSA key is generated per test run; no Google credential
//! is used or stored.
use super::*;
use aws_lc_rs::{
    encoding::AsDer,
    rsa::{KeyPair, KeySize},
    signature::KeyPair as _,
};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode as AxumStatus},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicI64, Ordering},
        Mutex,
    },
};

const PROJECT: &str = "kuzz-test-project";
const CLIENT_EMAIL: &str = "gateway@kuzz-test-project.iam.gserviceaccount.com";
const KEY_ID: &str = "0123456789abcdef";
const DEVICE_TOKEN: &str = "fcm:APA91b-test_token";

struct Key {
    pem: String,
    public_pkcs1: Vec<u8>,
}

fn generate_key() -> Key {
    let pair = KeyPair::generate(KeySize::Rsa2048).expect("generate RSA test key");
    let pkcs8 = AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&pair).expect("pkcs8");
    let encoded = STANDARD.encode(pkcs8.as_ref());
    let lines = encoded
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    Key {
        pem: format!("-----BEGIN PRIVATE KEY-----\n{lines}\n-----END PRIVATE KEY-----\n"),
        public_pkcs1: pair.public_key().as_ref().to_vec(),
    }
}

fn service_account_json(key: &Key) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "type": "service_account",
        "project_id": PROJECT,
        "private_key_id": KEY_ID,
        "private_key": key.pem,
        "client_email": CLIENT_EMAIL,
        "client_id": "1",
        "token_uri": "https://attacker.example/token",
    }))
    .unwrap()
}

#[derive(Default)]
struct Mock {
    public_pkcs1: Vec<u8>,
    token_responses: VecDeque<(u16, String)>,
    send_responses: VecDeque<(u16, Option<&'static str>, String)>,
    token_calls: usize,
    sends: Vec<(String, serde_json::Value)>,
    bad_assertions: usize,
}

type Shared = Arc<Mutex<Mock>>;

#[derive(Debug, Deserialize)]
struct Claims {
    iss: String,
    scope: String,
    aud: String,
    iat: i64,
    exp: i64,
}

async fn token_endpoint(State(mock): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let mut m = mock.lock().unwrap();
    m.token_calls += 1;
    let form = std::str::from_utf8(&body).unwrap();
    let assertion = form
        .strip_prefix("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion=")
        .unwrap_or("");
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_audience(&[GOOGLE_TOKEN_URL]);
    validation.set_issuer(&[CLIENT_EMAIL]);
    // The transport runs on the test clock; iat/exp are checked below instead.
    validation.validate_exp = false;
    let verified = jsonwebtoken::decode::<Claims>(
        assertion,
        &jsonwebtoken::DecodingKey::from_rsa_der(&m.public_pkcs1),
        &validation,
    );
    let header_ok =
        jsonwebtoken::decode_header(assertion).is_ok_and(|h| h.kid.as_deref() == Some(KEY_ID));
    let form_ok = headers
        .get("content-type")
        .is_some_and(|v| v == "application/x-www-form-urlencoded");
    match verified {
        Ok(data)
            if header_ok
                && form_ok
                && data.claims.scope == FCM_SCOPE
                && data.claims.iss == CLIENT_EMAIL
                && data.claims.aud == GOOGLE_TOKEN_URL
                && data.claims.iat >= T0
                && data.claims.exp - data.claims.iat == 3600 => {}
        _ => {
            m.bad_assertions += 1;
            return (AxumStatus::BAD_REQUEST, r#"{"error":"invalid_grant"}"#).into_response();
        }
    }
    let n = m.token_calls;
    let (status, body) = m.token_responses.pop_front().unwrap_or((
        200,
        format!(r#"{{"access_token":"access-{n}","expires_in":3599,"token_type":"Bearer"}}"#),
    ));
    (AxumStatus::from_u16(status).unwrap(), body).into_response()
}

async fn send_endpoint(State(mock): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let mut m = mock.lock().unwrap();
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    m.sends
        .push((auth, serde_json::from_slice(&body).unwrap_or_default()));
    let (status, retry_after, body) = m.send_responses.pop_front().unwrap_or((
        200,
        None,
        r#"{"name":"projects/p/messages/1"}"#.to_owned(),
    ));
    let mut response = (AxumStatus::from_u16(status).unwrap(), body).into_response();
    if let Some(value) = retry_after {
        response
            .headers_mut()
            .insert("retry-after", value.parse().unwrap());
    }
    response
}

struct Harness {
    transport: FcmTransport,
    mock: Shared,
    clock: Arc<AtomicI64>,
}

const T0: i64 = 1_750_000_000;

async fn harness() -> Harness {
    let key = generate_key();
    let mock: Shared = Arc::new(Mutex::new(Mock {
        public_pkcs1: key.public_pkcs1.clone(),
        ..Mock::default()
    }));
    let app = Router::new()
        .route("/token", post(token_endpoint))
        .route(
            &format!("/v1/projects/{PROJECT}/messages:send"),
            post(send_endpoint),
        )
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let clock = Arc::new(AtomicI64::new(T0));
    let now = {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst)) as Clock
    };
    let account = ServiceAccount::from_json(&service_account_json(&key)).unwrap();
    let transport = FcmTransport::with_urls(account, format!("{base}/token"), &base, now).unwrap();
    Harness {
        transport,
        mock,
        clock,
    }
}

fn endpoint() -> String {
    hex::encode(DEVICE_TOKEN)
}

fn attempt(expires_at: i64) -> DeliveryAttempt {
    DeliveryAttempt {
        request_id: uuid::Uuid::new_v4(),
        expires_at,
    }
}

fn fcm_error(status: &str, code: Option<&str>, token_field: bool) -> String {
    let mut details = Vec::new();
    if let Some(code) = code {
        details.push(serde_json::json!({"@type": FCM_ERROR_TYPE, "errorCode": code}));
    }
    if token_field {
        details.push(serde_json::json!({
            "@type": BAD_REQUEST_TYPE,
            "fieldViolations": [{"field": "message.token", "description": "Invalid registration token"}]
        }));
    }
    serde_json::json!({"error": {"code": 400, "message": "m", "status": status, "details": details}})
        .to_string()
}

#[tokio::test]
async fn sends_one_data_only_wake_with_a_cached_service_account_token() {
    let h = harness().await;
    assert_eq!(
        h.transport.send(attempt(T0 + 90), &endpoint()).await,
        DeliveryOutcome::Accepted
    );
    h.clock.store(T0 + 600, Ordering::SeqCst);
    assert_eq!(
        h.transport.send(attempt(T0 + 600), &endpoint()).await,
        DeliveryOutcome::Accepted
    );
    let m = h.mock.lock().unwrap();
    assert_eq!(m.bad_assertions, 0);
    assert_eq!(m.token_calls, 1, "the access token is cached");
    assert_eq!(m.sends.len(), 2);
    assert_eq!(m.sends[0].0, "Bearer access-1");
    assert_eq!(
        m.sends[0].1,
        serde_json::json!({"message": {
            "token": DEVICE_TOKEN,
            "data": {"k": "wake"},
            "android": {"priority": "HIGH", "ttl": "90s", "collapse_key": "kuzz-wake"}
        }})
    );
    assert_eq!(m.sends[1].1["message"]["android"]["ttl"], "0s");
}

#[tokio::test]
async fn token_is_refreshed_about_fifty_minutes_after_issue() {
    let h = harness().await;
    h.transport.send(attempt(T0 + 60), &endpoint()).await;
    h.clock.store(T0 + 2_998, Ordering::SeqCst);
    h.transport.send(attempt(T0 + 3_000), &endpoint()).await;
    assert_eq!(h.mock.lock().unwrap().token_calls, 1);
    h.clock.store(T0 + 2_999, Ordering::SeqCst);
    h.transport.send(attempt(T0 + 3_000), &endpoint()).await;
    let m = h.mock.lock().unwrap();
    assert_eq!(m.token_calls, 2);
    assert_eq!(m.sends.last().unwrap().0, "Bearer access-2");
}

#[tokio::test]
async fn unauthorized_send_refreshes_the_token_once() {
    let h = harness().await;
    h.mock
        .lock()
        .unwrap()
        .send_responses
        .push_back((401, None, String::new()));
    assert_eq!(
        h.transport.send(attempt(T0 + 60), &endpoint()).await,
        DeliveryOutcome::Accepted
    );
    {
        let m = h.mock.lock().unwrap();
        assert_eq!(m.token_calls, 2);
        assert_eq!(m.sends[0].0, "Bearer access-1");
        assert_eq!(m.sends[1].0, "Bearer access-2");
    }

    for _ in 0..2 {
        h.mock.lock().unwrap().send_responses.push_back((
            401,
            None,
            fcm_error("UNAUTHENTICATED", Some("THIRD_PARTY_AUTH_ERROR"), false),
        ));
    }
    assert_eq!(
        h.transport.send(attempt(T0 + 60), &endpoint()).await,
        DeliveryOutcome::ConfigurationFault
    );
    let m = h.mock.lock().unwrap();
    assert_eq!(m.token_calls, 3, "exactly one refresh per delivery");
    assert_eq!(m.sends.len(), 4);
}

#[tokio::test]
async fn provider_errors_map_to_bounded_delivery_outcomes() {
    let invalid = DeliveryOutcome::InvalidEndpoint {
        unregistered_at: None,
    };
    let cases: Vec<(u16, Option<&'static str>, String, DeliveryOutcome)> = vec![
        (
            404,
            None,
            fcm_error("NOT_FOUND", Some("UNREGISTERED"), false),
            invalid,
        ),
        (404, None, String::new(), invalid),
        (
            400,
            None,
            fcm_error("INVALID_ARGUMENT", Some("INVALID_ARGUMENT"), true),
            invalid,
        ),
        (
            400,
            None,
            fcm_error("INVALID_ARGUMENT", Some("INVALID_ARGUMENT"), false),
            DeliveryOutcome::PermanentRequestFault,
        ),
        (
            403,
            None,
            fcm_error("PERMISSION_DENIED", Some("SENDER_ID_MISMATCH"), false),
            DeliveryOutcome::ConfigurationFault,
        ),
        (
            429,
            Some("120"),
            fcm_error("RESOURCE_EXHAUSTED", Some("QUOTA_EXCEEDED"), false),
            DeliveryOutcome::Retry {
                retry_after_seconds: Some(120),
            },
        ),
        (
            503,
            Some("999999"),
            fcm_error("UNAVAILABLE", Some("UNAVAILABLE"), false),
            DeliveryOutcome::Retry {
                retry_after_seconds: Some(3600),
            },
        ),
        (
            500,
            None,
            "not json".to_owned(),
            DeliveryOutcome::Retry {
                retry_after_seconds: None,
            },
        ),
        (
            400,
            None,
            "x".repeat(1024 * 1024),
            DeliveryOutcome::PermanentRequestFault,
        ),
    ];
    let h = harness().await;
    for (status, retry_after, body, expected) in cases {
        h.mock
            .lock()
            .unwrap()
            .send_responses
            .push_back((status, retry_after, body.clone()));
        assert_eq!(
            h.transport.send(attempt(T0 + 60), &endpoint()).await,
            expected,
            "status {status}, body {}",
            &body[..body.len().min(120)]
        );
    }
}

#[test]
fn invalid_argument_without_a_token_violation_never_invalidates_the_endpoint() {
    let body = fcm_error("INVALID_ARGUMENT", Some("INVALID_ARGUMENT"), false);
    assert_eq!(
        classify(400, body.as_bytes(), None),
        DeliveryOutcome::PermanentRequestFault
    );
    let other_field = serde_json::json!({"error": {"status": "INVALID_ARGUMENT", "details": [
        {"@type": BAD_REQUEST_TYPE, "fieldViolations": [{"field": "message.android.ttl"}]}
    ]}})
    .to_string();
    assert_eq!(
        classify(400, other_field.as_bytes(), None),
        DeliveryOutcome::PermanentRequestFault
    );
}

#[tokio::test]
async fn token_endpoint_failures_back_off_and_classify() {
    let h = harness().await;
    h.mock
        .lock()
        .unwrap()
        .token_responses
        .push_back((503, String::new()));
    assert_eq!(
        h.transport.send(attempt(T0 + 60), &endpoint()).await,
        DeliveryOutcome::Retry {
            retry_after_seconds: Some(30)
        }
    );
    // Inside the back-off window no second token request is made.
    h.clock.store(T0 + 29, Ordering::SeqCst);
    assert!(matches!(
        h.transport.send(attempt(T0 + 60), &endpoint()).await,
        DeliveryOutcome::Retry { .. }
    ));
    assert_eq!(h.mock.lock().unwrap().token_calls, 1);

    h.clock.store(T0 + 30, Ordering::SeqCst);
    h.mock.lock().unwrap().token_responses.push_back((
        400,
        r#"{"error":"invalid_grant","error_description":"Invalid JWT Signature."}"#.to_owned(),
    ));
    assert_eq!(
        h.transport.send(attempt(T0 + 60), &endpoint()).await,
        DeliveryOutcome::ConfigurationFault
    );
    h.clock.store(T0 + 60, Ordering::SeqCst);
    assert_eq!(
        h.transport.send(attempt(T0 + 90), &endpoint()).await,
        DeliveryOutcome::Accepted
    );
    let m = h.mock.lock().unwrap();
    assert_eq!(m.token_calls, 3);
    assert_eq!(m.sends.len(), 1, "no send without an access token");
}

#[tokio::test]
async fn concurrent_deliveries_share_one_token_fetch() {
    let h = Arc::new(harness().await);
    let sends = (0..8).map(|_| {
        let h = h.clone();
        tokio::spawn(async move { h.transport.send(attempt(T0 + 60), &endpoint()).await })
    });
    for send in sends {
        assert_eq!(send.await.unwrap(), DeliveryOutcome::Accepted);
    }
    let m = h.mock.lock().unwrap();
    assert_eq!(m.token_calls, 1);
    assert_eq!(m.sends.len(), 8);
}

#[tokio::test]
async fn malformed_endpoints_are_refused_before_any_request() {
    let h = harness().await;
    for endpoint in [
        "zz".to_owned(),
        hex::encode("has space"),
        hex::encode("a".repeat(MAX_FCM_TOKEN_BYTES + 1)),
        String::new(),
    ] {
        assert_eq!(
            h.transport.send(attempt(T0 + 60), &endpoint).await,
            DeliveryOutcome::PermanentRequestFault
        );
    }
    let m = h.mock.lock().unwrap();
    assert_eq!((m.token_calls, m.sends.len()), (0, 0));
}

#[test]
fn ttl_is_clamped_to_fcm_bounds() {
    assert_eq!(ttl_seconds(T0 - 5, T0), 0);
    assert_eq!(ttl_seconds(T0 + 75, T0), 75);
    assert_eq!(ttl_seconds(i64::MAX, T0), MAX_FCM_TTL_SECONDS);
}

#[test]
fn service_account_is_validated_and_never_printed() {
    let key = generate_key();
    let good: serde_json::Value = serde_json::from_slice(&service_account_json(&key)).unwrap();
    let account = ServiceAccount::from_json(&serde_json::to_vec(&good).unwrap()).unwrap();
    assert_eq!(account.project_id(), PROJECT);
    let printed = format!("{account:?}");
    assert!(!printed.contains("PRIVATE KEY") && !printed.contains(KEY_ID));

    for (field, value) in [
        ("type", serde_json::json!("authorized_user")),
        ("project_id", serde_json::json!("Bad/Project")),
        ("private_key_id", serde_json::json!("")),
        ("client_email", serde_json::json!("no-at-sign")),
        (
            "private_key",
            serde_json::json!("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"),
        ),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert!(
            ServiceAccount::from_json(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "accepted bad {field}"
        );
    }
    let mut missing = good.clone();
    missing.as_object_mut().unwrap().remove("client_email");
    assert!(ServiceAccount::from_json(&serde_json::to_vec(&missing).unwrap()).is_err());
}
