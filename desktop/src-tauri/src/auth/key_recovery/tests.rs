use super::*;
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

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
    let asked = std::cell::Cell::new(false);
    let created = resolve_backup(&client, &origin, "test-session", "account-a", || {
        asked.set(true);
        Ok(original.clone())
    })
    .await
    .unwrap();
    assert!(asked.get());
    let restored = resolve_backup(&client, &origin, "test-session", "account-a", || {
        panic!("a backed-up account never asks for a new key")
    })
    .await
    .unwrap();
    assert_eq!(created.public_key(), restored.public_key());
    let signed = EventBuilder::new(Kind::Custom(40002), "same signer")
        .sign_with_keys(&restored)
        .unwrap();
    signed.verify().unwrap();
    assert_eq!(signed.pubkey, original.public_key());
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
        let restored = resolve_backup(&client, &origin, "test-session", "account-a", || {
            assert!(!mobile_first);
            Ok(expected.clone())
        })
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
async fn restore_or_status_failure_never_uploads_or_creates_a_key() {
    for (fail_restore, fail_status) in [(true, false), (false, true)] {
        let relay = Relay {
            fail_restore,
            fail_status,
            ..Relay::default()
        };
        *relay.key.lock().unwrap() = Some(Keys::generate());
        let (origin, task) = server(relay.clone()).await;
        let client = crate::app_state::build_media_fetch_client().unwrap();
        let result = resolve_backup(&client, &origin, "test-session", "account-a", || {
            panic!("failure must not create a key")
        })
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

/// Whatever was on this computer, the chosen Google account decides the
/// account: its backup is restored, and a Google account with no backup gets
/// the new key it is handed, never the key that was active here.
#[tokio::test]
async fn any_google_account_signs_in_to_its_own_account() {
    let backed_up = Keys::generate();
    let relay = Relay::default();
    *relay.key.lock().unwrap() = Some(backed_up.clone());
    let (origin, task) = server(relay.clone()).await;
    let client = crate::app_state::build_media_fetch_client().unwrap();
    let restored = resolve_backup(&client, &origin, "test-session", "account-a", || {
        panic!("a backed-up account is restored, not replaced")
    })
    .await
    .unwrap();
    assert_eq!(restored.public_key(), backed_up.public_key());
    assert_eq!(relay.uploads.load(std::sync::atomic::Ordering::SeqCst), 0);
    task.abort();

    let empty = Relay::default();
    let (origin, task) = server(empty.clone()).await;
    let fresh = Keys::generate();
    let created = resolve_backup(&client, &origin, "test-session", "account-a", || {
        Ok(fresh.clone())
    })
    .await
    .unwrap();
    assert_eq!(created.public_key(), fresh.public_key());
    assert_eq!(
        empty.key.lock().unwrap().as_ref().map(Keys::public_key),
        Some(fresh.public_key())
    );
    task.abort();
}

#[test]
fn connect_is_refused_unless_a_listed_key_only_account_links_its_own_key() {
    use super::super::accounts::{Account, AccountState, Registry};
    let key = Keys::generate().public_key();
    let listed = |state| Registry {
        accounts: vec![Account {
            pubkey: key.to_hex(),
            origin: "https://relay.test".into(),
            state,
        }],
        ..Registry::default()
    };
    // The one allowed case.
    assert!(!connect_refused(
        &listed(AccountState::KeyOnly),
        &key,
        false,
        false
    ));
    // Not on the list (an empty list never counts as "all key-only").
    assert!(connect_refused(&Registry::default(), &key, false, false));
    // Already held by a Google account.
    assert!(connect_refused(
        &listed(AccountState::SignedIn),
        &key,
        false,
        false
    ));
    // Recovering a lost key: the in-memory key is a stand-in.
    assert!(connect_refused(
        &listed(AccountState::KeyOnly),
        &key,
        true,
        false
    ));
    // This login method is already bound to a key.
    assert!(connect_refused(
        &listed(AccountState::KeyOnly),
        &key,
        false,
        true
    ));
}
