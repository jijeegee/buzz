//! The iOS shadow report: body contract (unit) and the route (Postgres + Redis).

use serde_json::{json, Value};

use super::*;

/// The app's counter names and `closed` keys (mobile
/// `user_stream_counters.dart`: `userStreamCounterNames` + `snapshotMs`,
/// `userStreamReportClosedKeys`).
const APP_COUNTERS: [&str; 15] = [
    "eventsCompared",
    "chunkOnly",
    "streamOnlyUnexpected",
    "streamOnlyExpected",
    "predictedSkips",
    "verifiedSkips",
    "falseSkip",
    "liveRoomsChecked",
    "liveMissed",
    "skipAudited",
    "skipAuditNew",
    "skipAuditFailed",
    "seqWriteSkippedOrder",
    "reconnects",
    "snapshotMs",
];
const APP_CLOSED: [&str; 11] = [
    "off",
    "not_enabled",
    "invalid",
    "too_many_rooms",
    "restricted",
    "auth_required",
    "shadow_full",
    "full",
    "lagged",
    "closed",
    "other",
];

/// What `userStreamReportBody` builds, with every number at `n`.
fn app_body(n: u64) -> Value {
    json!({
        "platform": "ios",
        "app_version": "0.6.17",
        "seq": n,
        "counters": APP_COUNTERS.iter().map(|k| (k.to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
        "closed": APP_CLOSED.iter().map(|k| (k.to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
    })
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

#[test]
fn the_app_body_parses_with_every_label() {
    let report = parse(&bytes(&app_body(7))).expect("app body");
    assert_eq!(report.seq, 7);
    assert_eq!(report.counters.events_compared, 7);
    assert_eq!(report.counters.snapshot_ms, 7);
    assert_eq!(report.closed.shadow_full, 7);
    let max = parse(&bytes(&app_body(u64::from(u32::MAX)))).expect("u32 max");
    assert_eq!(max.closed.other, u32::MAX);
    assert!(
        bytes(&app_body(u64::from(u32::MAX))).len() <= MAX_BODY_BYTES,
        "the largest app body fits"
    );
}

#[test]
fn every_label_is_required_and_no_other_is_accepted() {
    for section in ["counters", "closed"] {
        let keys: Vec<String> = app_body(1)[section]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for key in keys {
            let mut body = app_body(1);
            body[section].as_object_mut().unwrap().remove(&key);
            assert!(parse(&bytes(&body)).is_err(), "missing {section}.{key}");
        }
        let mut body = app_body(1);
        body[section]["fallbacks"] = json!(1);
        assert!(parse(&bytes(&body)).is_err(), "unknown {section} field");
    }
    let mut body = app_body(1);
    body["extra"] = json!(1);
    assert!(parse(&bytes(&body)).is_err(), "unknown top-level field");
}

#[test]
fn numbers_are_u32_platform_is_ios_and_version_is_semver() {
    for bad in [
        json!(-1),
        json!(1.5),
        json!(u64::from(u32::MAX) + 1),
        json!("1"),
    ] {
        let mut body = app_body(1);
        body["counters"]["liveMissed"] = bad.clone();
        assert!(parse(&bytes(&body)).is_err(), "{bad}");
    }
    let mut body = app_body(1);
    body["platform"] = json!("android");
    assert!(parse(&bytes(&body)).is_err());
    for version in [
        "0.6",
        "0.6.17-beta",
        "v0.6.17",
        "0.6.17\n",
        "1..2",
        &"9".repeat(40),
    ] {
        let mut body = app_body(1);
        body["app_version"] = json!(version);
        assert!(parse(&bytes(&body)).is_err(), "{version:?}");
    }
}

// ── The route, through the production router ───────────────────────────────

struct Route {
    state: Arc<AppState>,
    host: String,
    url: String,
}

impl Route {
    async fn new() -> Self {
        let redis = std::env::var("REDIS_URL").expect("REDIS_URL");
        let state = crate::state::tests::test_state_with_config(
            &crate::test_support::database_url(),
            |config| {
                config.redis_url = redis;
                config.relay_url = "wss://report-test.local".to_string();
            },
        )
        .await;
        let host = format!("report-{}.local", uuid::Uuid::new_v4().simple());
        state.db.ensure_configured_community(&host).await.unwrap();
        let tenant = crate::tenant::bind_community(&state.db, &host)
            .await
            .unwrap();
        let url = crate::api::bridge::nip98_expected_url(&state.config.relay_url, &tenant, PATH);
        Self { state, host, url }
    }

    fn auth(&self, keys: &nostr::Keys, body: &[u8]) -> String {
        crate::nip98::nip98_header(keys, &self.url, body).unwrap()
    }

    async fn post(&self, authorization: Option<&str>, body: Vec<u8>) -> StatusCode {
        use tower::ServiceExt;
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri(PATH)
            .header("host", &self.host)
            .header("content-type", "application/json");
        if let Some(authorization) = authorization {
            request = request.header("authorization", authorization);
        }
        crate::router::build_router(Arc::clone(&self.state))
            .oneshot(request.body(axum::body::Body::from(body)).unwrap())
            .await
            .unwrap()
            .status()
    }
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn a_signed_report_is_taken_once_per_5_min() {
    let route = Route::new().await;
    let keys = nostr::Keys::generate();
    let body = bytes(&app_body(1));

    assert_eq!(
        route.post(None, body.clone()).await,
        StatusCode::UNAUTHORIZED,
        "no auth"
    );
    let first = route.auth(&keys, &body);
    assert_eq!(
        route.post(Some(&first), body.clone()).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        route.post(Some(&first), body.clone()).await,
        StatusCode::UNAUTHORIZED,
        "the same NIP-98 event again is a replay"
    );
    let second = route.auth(&keys, &body);
    assert_eq!(
        route.post(Some(&second), body.clone()).await,
        StatusCode::TOO_MANY_REQUESTS,
        "one report per account per 5 min"
    );
    // Another account has its own window.
    let other = nostr::Keys::generate();
    let auth = route.auth(&other, &body);
    assert_eq!(route.post(Some(&auth), body).await, StatusCode::NO_CONTENT);
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn bad_bodies_are_refused_without_using_the_window() {
    let route = Route::new().await;
    let keys = nostr::Keys::generate();

    let mut unknown = app_body(1);
    unknown["counters"]["fallbacks"] = json!(1);
    let body = bytes(&unknown);
    let auth = route.auth(&keys, &body);
    assert_eq!(route.post(Some(&auth), body).await, StatusCode::BAD_REQUEST);

    let mut padded = app_body(1);
    padded["app_version"] = json!("1".repeat(MAX_BODY_BYTES));
    let body = bytes(&padded);
    let auth = route.auth(&keys, &body);
    assert_eq!(
        route.post(Some(&auth), body).await,
        StatusCode::PAYLOAD_TOO_LARGE
    );

    let body = bytes(&app_body(2));
    let auth = route.auth(&keys, &body);
    assert_eq!(route.post(Some(&auth), body).await, StatusCode::NO_CONTENT);
}
