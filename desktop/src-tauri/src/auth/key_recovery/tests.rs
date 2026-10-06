use super::*;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[test]
fn lost_key_recovery_rejects_different_owner_and_unprovable_existing_agents() {
    let owner = Keys::generate();
    let other = Keys::generate();
    let agent = Keys::generate().public_key();
    let tag = buzz_sdk_pkg::nip_oa::compute_auth_tag(&owner, &agent, "kind=9").unwrap();
    let agent_hex = agent.to_hex();
    assert!(verify_recovered_owner(other.public_key(), Some(owner.public_key()), []).is_err());
    assert!(verify_recovered_owner(
        other.public_key(),
        None,
        [(agent_hex.as_str(), Some(tag.as_str()))]
    )
    .is_err());
    assert!(verify_recovered_owner(
        owner.public_key(),
        None,
        [(agent_hex.as_str(), Some(tag.as_str()))]
    )
    .is_ok());
    assert!(verify_recovered_owner(
        owner.public_key(),
        None,
        [(agent_hex.as_str(), Some("bad tag"))]
    )
    .is_err());
    let error =
        verify_recovered_owner(owner.public_key(), None, [(agent_hex.as_str(), None)]).unwrap_err();
    assert!(error.contains("original key"));
    assert!(verify_recovered_owner(
        owner.public_key(),
        Some(owner.public_key()),
        [(agent_hex.as_str(), None)]
    )
    .is_ok());
    assert!(verify_recovered_owner(owner.public_key(), None, []).is_ok());
}

#[derive(Clone, Default)]
struct Relay {
    key: Arc<Mutex<Option<Keys>>>,
    origin: Arc<Mutex<String>>,
    fail_restore: bool,
    fail_status: bool,
    uploads: Arc<std::sync::atomic::AtomicUsize>,
}

async fn mock_status(State(relay): State<Relay>) -> (StatusCode, Json<Value>) {
    if relay.fail_status {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":"backup_unavailable"})),
        );
    }
    let key = relay.key.lock().unwrap();
    (
        StatusCode::OK,
        Json(
            json!({"state":if key.is_some() {"ready"} else {"absent"}, "account_id":"account-a", "pubkey":key.as_ref().map(|k| k.public_key().to_hex()), "version":1}),
        ),
    )
}

async fn mock_challenge(State(relay): State<Relay>) -> Json<Value> {
    Json(
        json!({"challenge":"ab".repeat(32), "account_id":"account-a", "url":format!("{}/auth/key-backup", relay.origin.lock().unwrap()), "expires_in":120}),
    )
}

async fn mock_initialize(
    State(relay): State<Relay>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    relay
        .uploads
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let event: Event = serde_json::from_value(body["proof"].clone()).unwrap();
    event.verify().unwrap();
    assert_eq!(event.kind, Kind::HttpAuth);
    let key = Keys::parse(body["secret_key"].as_str().unwrap()).unwrap();
    assert_eq!(event.pubkey, key.public_key());
    let mut stored = relay.key.lock().unwrap();
    if stored.is_some() {
        return (StatusCode::CONFLICT, Json(json!({"code":"backup_exists"})));
    }
    let pubkey = key.public_key().to_hex();
    *stored = Some(key);
    (StatusCode::OK, Json(json!({"pubkey":pubkey,"version":1})))
}

async fn mock_restore(State(relay): State<Relay>) -> (StatusCode, Json<Value>) {
    if relay.fail_restore {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"code":"backup_unavailable"})),
        );
    }
    let key = relay.key.lock().unwrap();
    let key = key.as_ref().unwrap();
    (
        StatusCode::OK,
        Json(
            json!({"secret_key":key.secret_key().to_secret_hex(),"pubkey":key.public_key().to_hex(),"version":1}),
        ),
    )
}

async fn server(relay: Relay) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    *relay.origin.lock().unwrap() = origin.clone();
    let app = Router::new()
        .route("/auth/key-backup", get(mock_status).post(mock_initialize))
        .route("/auth/key-backup/challenge", post(mock_challenge))
        .route("/auth/key-backup/restore", post(mock_restore))
        .with_state(relay);
    (
        origin,
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
    )
}

#[tokio::test]
async fn signup_then_new_device_restores_identical_signer_and_existing_link_keeps_it() {
    let relay = Relay::default();
    let (origin, task) = server(relay.clone()).await;
    let client = crate::app_state::build_media_fetch_client().unwrap();
    let original = Keys::generate();
    let pending = std::cell::Cell::new(false);
    let created = resolve_backup(
        &client,
        &origin,
        "test-session",
        "account-a",
        &original,
        true,
        |_| {
            pending.set(true);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert!(pending.get());
    let restored = resolve_backup(
        &client,
        &origin,
        "test-session",
        "account-a",
        &Keys::generate(),
        true,
        |_| panic!("restore must not persist a replacement key"),
    )
    .await
    .unwrap();
    assert_eq!(created.public_key(), restored.public_key());
    let signed = EventBuilder::new(Kind::Custom(40002), "same signer")
        .sign_with_keys(&restored)
        .unwrap();
    signed.verify().unwrap();
    assert_eq!(signed.pubkey, original.public_key());
    let linked = resolve_backup(
        &client,
        &origin,
        "test-session",
        "account-a",
        &original,
        false,
        |_| panic!("existing backup must not be replaced"),
    )
    .await
    .unwrap();
    assert_eq!(linked.public_key(), original.public_key());
    assert_eq!(relay.uploads.load(std::sync::atomic::Ordering::SeqCst), 1);
    task.abort();
    let _ = task.await;
}

// Public secp256k1 test vector, never a real account. Flutter exercises its
// production upload and restore against this same wire representation.
#[tokio::test]
async fn mobile_first_and_desktop_first_share_the_exact_backup_wire_identity() {
    let fixture: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mobile/test/fixtures/google_key_backup_interop.json"
    )))
    .unwrap();
    let expected = Keys::parse(fixture["secret_key"].as_str().unwrap()).unwrap();
    for mobile_first in [true, false] {
        let relay = Relay::default();
        if mobile_first {
            *relay.key.lock().unwrap() = Some(expected.clone());
        }
        let (origin, task) = server(relay.clone()).await;
        let client = crate::app_state::build_media_fetch_client().unwrap();
        let local = if mobile_first {
            Keys::generate()
        } else {
            expected.clone()
        };
        let restored = resolve_backup(
            &client,
            &origin,
            "test-session",
            "account-a",
            &local,
            true,
            |_| {
                assert!(!mobile_first);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(restored.public_key().to_hex(), fixture["pubkey"]);
        assert_eq!(restored.secret_key().to_secret_hex(), fixture["secret_key"]);
        let (_, Json(wire)) = mock_restore(State(relay.clone())).await;
        assert_eq!(wire, fixture);
        assert_eq!(
            relay.uploads.load(std::sync::atomic::Ordering::SeqCst),
            usize::from(!mobile_first)
        );
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn restore_or_status_failure_and_established_key_conflict_never_upload() {
    for (fail_restore, fail_status) in [(true, false), (false, true), (false, false)] {
        let relay = Relay {
            fail_restore,
            fail_status,
            ..Relay::default()
        };
        *relay.key.lock().unwrap() = Some(Keys::generate());
        let (origin, task) = server(relay.clone()).await;
        let client = crate::app_state::build_media_fetch_client().unwrap();
        let local = Keys::generate();
        let result = resolve_backup(
            &client,
            &origin,
            "test-session",
            "account-a",
            &local,
            fail_restore || fail_status,
            |_| panic!("failure must not initialize"),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(relay.uploads.load(std::sync::atomic::Ordering::SeqCst), 0);
        task.abort();
        let _ = task.await;
    }
}

#[test]
fn backup_session_refresh_and_expiry_leave_signed_identity_and_mentions_unchanged() {
    let state = crate::app_state::build_app_state();
    let origin = state.current_auth_origin();
    let pubkey = state.keys.lock().unwrap().public_key();
    state.token_auth.set_key_backup(&origin, pubkey);
    for value in [
        super::super::OriginAuth::Active(super::super::UserSession {
            principal: Keys::generate().public_key(),
            device_id: None,
            access: Zeroizing::new("test-session".into()),
            refresh: Zeroizing::new("test-refresh".into()),
            access_issued_at: 0,
            access_expires_at: i64::MAX,
        }),
        super::super::OriginAuth::Restoring,
        super::super::OriginAuth::NeedsLogin("expired".into()),
        super::super::OriginAuth::SignedOut,
    ] {
        state.token_auth.set(&origin, value);
        let signer = state.user_credential().unwrap();
        assert!(!signer.is_token());
        assert_eq!(signer.public_key(), pubkey);
        let mentioned = Keys::generate().public_key();
        let event = signer
            .sign(
                EventBuilder::new(Kind::Custom(40002), "hello").tags([Tag::public_key(mentioned)]),
            )
            .unwrap();
        event.verify().unwrap();
        assert_eq!(event.pubkey, pubkey);
        assert_eq!(event.tags.public_keys().next(), Some(&mentioned));
    }
}
