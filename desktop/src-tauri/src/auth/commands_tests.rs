//! Tests for [`super`] (the auth commands), kept beside it so the module
//! stays under the desktop file-size ratchet.

use super::*;

#[test]
fn only_ended_sessions_sign_out_remotely() {
    let error = |status: Option<u16>, code: Option<&str>| api::ApiError {
        status,
        code: code.map(str::to_owned),
        message: String::new(),
    };
    for code in ["token_revoked", "principal_disabled", "invalid_token"] {
        assert!(
            session_ended_remotely(&error(Some(401), Some(code))),
            "{code}"
        );
    }
    assert!(!session_ended_remotely(&error(
        Some(401),
        Some("token_expired")
    )));
    assert!(!session_ended_remotely(&error(
        Some(403),
        Some("token_revoked")
    )));
    assert!(!session_ended_remotely(&error(Some(401), None)));
    assert!(!session_ended_remotely(&error(None, None)));
}

#[tokio::test(start_paused = true)]
async fn a_sign_out_does_not_wait_forever_for_its_agents() {
    // The stop that never finishes (a harness that would not exit): the
    // sign-out goes on after the limit and reports every agent.
    let limit = std::time::Duration::from_secs(10);
    let started = tokio::time::Instant::now();
    let not_stopped = await_agent_stops(
        std::future::pending::<Result<Vec<String>, String>>(),
        2,
        limit,
    )
    .await;
    assert_eq!(not_stopped, 2);
    assert_eq!(started.elapsed(), limit);
}

#[tokio::test]
async fn a_sign_out_reports_only_the_agents_that_did_not_stop() {
    let limit = std::time::Duration::from_secs(10);
    assert_eq!(await_agent_stops(async { Ok(vec![]) }, 3, limit).await, 0);
    assert_eq!(
        await_agent_stops(async { Ok(vec!["a: stuck".to_string()]) }, 3, limit).await,
        1
    );
    assert_eq!(
        await_agent_stops(async { Err("join failed".to_string()) }, 3, limit).await,
        3
    );
}

#[test]
fn explicit_token_login_preserves_existing_key_agents_and_allows_token_bots() {
    let origin = "https://relay.test";
    let mut record: crate::managed_agents::ManagedAgentRecord =
        serde_json::from_value(serde_json::json!({
            "pubkey": nostr::Keys::generate().public_key().to_hex(),
            "name": "Scout", "relay_url": "wss://relay.test", "acp_command": "kuzz-acp",
            "agent_command": "goose", "agent_args": [], "mcp_command": "",
            "turn_timeout_seconds": 320, "system_prompt": null,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
    let before = serde_json::to_value(&record).unwrap();
    assert!(
        ensure_existing_token_agents(origin, std::slice::from_ref(&record)).is_err(),
        "the compatibility route must not expose key agents to token adoption"
    );
    assert_eq!(serde_json::to_value(&record).unwrap(), before);
    record.relay_url = "wss://old-community.test".into();
    assert!(
        ensure_existing_token_agents(origin, std::slice::from_ref(&record)).is_err(),
        "legacy relay pins are ignored by production agent fan-out"
    );
    record.bot_origin = Some(origin.into());
    assert!(ensure_existing_token_agents(origin, std::slice::from_ref(&record)).is_ok());
    assert!(ensure_existing_token_agents(origin, &[]).is_ok());
    // A key record created while the browser was open must fail the same
    // guard when the final store-locked login commit rechecks the records.
    record.bot_origin = None;
    assert!(ensure_existing_token_agents(origin, &[record]).is_err());
}

struct LoginStore {
    binding: Option<PublicKey>,
    refresh: Option<String>,
}

impl RefreshStore for LoginStore {
    fn load(&self, _: &str) -> Result<Option<String>, String> {
        Ok(self.refresh.clone())
    }
    fn store(&self, _: &str, _: &str) -> Result<(), String> {
        panic!("selection must not change stored credentials")
    }
    fn delete(&self, _: &str) -> Result<(), String> {
        panic!("selection must preserve stored credentials")
    }
    fn signing_pubkey(&self, _: &str) -> Result<Option<PublicKey>, String> {
        Ok(self.binding)
    }
}

#[test]
fn google_relogin_keeps_known_token_accounts_on_dual_mode_relays() {
    let store = LoginStore {
        binding: None,
        refresh: None,
    };
    for state in [
        OriginAuth::Restoring,
        OriginAuth::NeedsLogin("token_expired".into()),
        OriginAuth::Active(UserSession {
            principal: nostr::Keys::generate().public_key(),
            device_id: None,
            access: Zeroizing::new("test-access".into()),
            refresh: Zeroizing::new("test-refresh".into()),
            access_issued_at: 0,
            access_expires_at: i64::MAX,
        }),
    ] {
        let auth = super::super::TokenAuthState::default();
        auth.record_backup_support("https://relay.test", true);
        auth.set("https://relay.test", state);
        assert!(
            !login_uses_key_backup(&auth, "https://relay.test", &store).unwrap(),
            "known token re-login must not select a different account mode"
        );
    }
}

#[test]
fn google_relogin_backup_marker_wins_even_when_discovery_is_unavailable() {
    let auth = super::super::TokenAuthState::default();
    let pubkey = nostr::Keys::generate().public_key();
    let store = LoginStore {
        binding: Some(pubkey),
        refresh: Some("test-refresh".into()),
    };
    assert!(
        login_uses_key_backup(&auth, "https://relay.test", &store).unwrap(),
        "discovery failure must not choose token login for a backed-up key"
    );
    assert_eq!(store.binding, Some(pubkey));
    assert_eq!(store.refresh.as_deref(), Some("test-refresh"));
}

#[test]
fn google_relogin_fresh_accounts_prefer_backup_and_stored_token_accounts_keep_their_mode() {
    let auth = super::super::TokenAuthState::default();
    auth.record_backup_support("https://relay.test", true);
    let fresh = LoginStore {
        binding: None,
        refresh: None,
    };
    assert!(login_uses_key_backup(&auth, "https://relay.test", &fresh).unwrap());
    let legacy = LoginStore {
        binding: None,
        refresh: Some("test-refresh".into()),
    };
    assert!(!login_uses_key_backup(&auth, "https://relay.test", &legacy).unwrap());
}

#[test]
fn google_relogin_rejects_cross_mode_completion_before_session_storage() {
    assert!(verify_login_mode(false, Some("key_backup")).is_err());
    assert!(verify_login_mode(true, Some("token")).is_err());
    assert!(verify_login_mode(true, None).is_err());
    assert!(verify_login_mode(false, None).is_ok());
    assert!(verify_login_mode(false, Some("token")).is_ok());
    assert!(verify_login_mode(true, Some("key_backup")).is_ok());
}

#[test]
fn google_relogin_explicit_legacy_route_cannot_replace_a_backup_binding() {
    let auth = super::super::TokenAuthState::default();
    let origin = "https://relay.test";
    auth.record_backup_support(origin, true);
    let fresh = LoginStore {
        binding: None,
        refresh: None,
    };
    assert!(!requested_login_mode(&auth, origin, &fresh, true).unwrap());
    assert!(requested_login_mode(&auth, origin, &fresh, false).unwrap());
    let bound = LoginStore {
        binding: Some(nostr::Keys::generate().public_key()),
        refresh: None,
    };
    assert!(requested_login_mode(&auth, origin, &bound, true).is_err());
    assert!(requested_login_mode(&auth, origin, &bound, false).unwrap());
}

#[tokio::test]
async fn google_relogin_after_logout_and_app_restart_keeps_legacy_mode() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };
    struct LogoutStore {
        refresh: Mutex<Option<String>>,
        legacy: AtomicBool,
    }
    impl RefreshStore for LogoutStore {
        fn load(&self, _: &str) -> Result<Option<String>, String> {
            Ok(self.refresh.lock().unwrap().clone())
        }
        fn store(&self, _: &str, _: &str) -> Result<(), String> {
            panic!("logout cannot create a credential")
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            self.refresh.lock().unwrap().take();
            Ok(())
        }
        fn legacy_token_account(&self, _: &str) -> Result<bool, String> {
            Ok(self.legacy.load(Ordering::SeqCst))
        }
        fn remember_legacy_token_account(&self, _: &str) -> Result<(), String> {
            self.legacy.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    let origin = "https://relay.test";
    let store = LogoutStore {
        refresh: Mutex::new(Some("test-refresh".into())),
        legacy: AtomicBool::new(false),
    };
    let auth = super::super::TokenAuthState::default();
    auth.set(origin, OriginAuth::NeedsLogin("token_expired".into()));
    forget_session(&auth, origin, &store).await.unwrap();
    assert!(store.load(origin).unwrap().is_none());
    let restarted = super::super::TokenAuthState::default();
    restarted.record_backup_support(origin, true);
    assert!(
        !login_uses_key_backup(&restarted, origin, &store).unwrap(),
        "logging out deletes credentials, not knowledge of the existing account mode"
    );
}

struct SignOutStore {
    binding: Option<PublicKey>,
    refresh: std::sync::Mutex<Option<String>>,
}

impl RefreshStore for SignOutStore {
    fn load(&self, _: &str) -> Result<Option<String>, String> {
        Ok(self.refresh.lock().unwrap().clone())
    }
    fn store(&self, _: &str, _: &str) -> Result<(), String> {
        panic!("sign-out cannot create a credential")
    }
    fn delete(&self, _: &str) -> Result<(), String> {
        self.refresh.lock().unwrap().take();
        Ok(())
    }
    fn signing_pubkey(&self, _: &str) -> Result<Option<PublicKey>, String> {
        Ok(self.binding)
    }
}

fn loaded_auth(dir: &tempfile::TempDir) -> super::super::TokenAuthState {
    let auth = super::super::TokenAuthState::default();
    auth.signed_out.load(dir.path());
    auth
}

#[tokio::test]
async fn sign_out_marks_the_device_and_keeps_the_key_binding() {
    let dir = tempfile::tempdir().unwrap();
    let origin = "https://relay.test";
    let identity = nostr::Keys::generate().public_key();
    let store = SignOutStore {
        binding: Some(identity),
        refresh: std::sync::Mutex::new(Some("test-refresh".into())),
    };
    let auth = loaded_auth(&dir);
    let agent = "cd".repeat(32);
    sign_out_device(&auth, origin, &store, std::slice::from_ref(&agent))
        .await
        .unwrap();
    assert!(auth.device_signed_out_with(origin, &store));
    assert!(auth.signed_out.suppresses_agent(&agent));
    assert!(store.load(origin).unwrap().is_none(), "session forgotten");
    assert_eq!(store.binding, Some(identity), "key binding kept");
    // Durable: a restarted app is still signed out.
    assert!(loaded_auth(&dir).device_signed_out_with(origin, &store));
    // Another account signing in here leaves this account signed out and
    // its stopped agents stopped; this account signing in clears both.
    let other = nostr::Keys::generate().public_key();
    auth.signed_out.clear(origin, Some(other)).unwrap();
    assert!(auth.signed_out.is_signed_out(origin, Some(identity)));
    assert!(!auth.signed_out.is_signed_out(origin, Some(other)));
    assert!(auth.signed_out.suppresses_agent(&agent));
    auth.signed_out.clear(origin, Some(identity)).unwrap();
    assert!(!loaded_auth(&dir).device_signed_out_with(origin, &store));
    assert!(!loaded_auth(&dir).signed_out.suppresses_agent(&agent));
}

#[tokio::test]
async fn sign_out_of_an_active_token_account_covers_its_origin() {
    let dir = tempfile::tempdir().unwrap();
    let origin = "https://relay.test";
    let principal = nostr::Keys::generate().public_key();
    let auth = loaded_auth(&dir);
    auth.set(
        origin,
        OriginAuth::Active(UserSession {
            principal,
            device_id: None,
            access: Zeroizing::new("test-access".into()),
            refresh: Zeroizing::new("test-refresh".into()),
            access_issued_at: 0,
            access_expires_at: i64::MAX,
        }),
    );
    struct TokenStore(std::sync::atomic::AtomicBool);
    impl RefreshStore for TokenStore {
        fn load(&self, _: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn store(&self, _: &str, _: &str) -> Result<(), String> {
            unreachable!()
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn remember_legacy_token_account(&self, _: &str) -> Result<(), String> {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }
    sign_out_device(
        &auth,
        origin,
        &TokenStore(std::sync::atomic::AtomicBool::new(false)),
        &[],
    )
    .await
    .unwrap();
    // A token account has no local key: the marker covers the origin.
    assert!(auth.signed_out.is_signed_out(origin, None));
    assert!(auth.signed_out.is_signed_out(origin, Some(principal)));
    assert!(matches!(auth.get(origin), Some((_, OriginAuth::SignedOut))));
}

#[tokio::test]
async fn local_only_accounts_get_no_marker_so_they_are_never_locked_out() {
    let dir = tempfile::tempdir().unwrap();
    let origin = "https://relay.test";
    let store = SignOutStore {
        binding: None,
        refresh: std::sync::Mutex::new(None),
    };
    let auth = loaded_auth(&dir);
    sign_out_device(&auth, origin, &store, &[]).await.unwrap();
    assert!(!auth.device_signed_out_with(origin, &store));
    // With running agents to stop: still signs out, still no marker.
    sign_out_device(&auth, origin, &store, &["ab".repeat(32)])
        .await
        .unwrap();
    assert!(!auth.device_signed_out_with(origin, &store));
    assert!(!auth.signed_out.suppresses_agent(&"ab".repeat(32)));
}

#[test]
fn ws_auth_and_user_signing_are_refused_while_signed_out() {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::app_state::build_app_state();
    state.token_auth.signed_out.load(dir.path());
    let origin = state.current_auth_origin();
    assert!(matches!(
        ws_auth_mode(&state.token_auth, &origin),
        Ok(CredentialMode::Keys)
    ));
    assert!(state.user_credential().is_ok());
    let identity = state.keys.lock().unwrap().public_key();
    state.token_auth.set_key_backup(&origin, identity);
    state
        .token_auth
        .signed_out
        .mark(&origin, Some(identity), &[])
        .unwrap();
    let message = super::super::signed_out::SIGNED_OUT_MESSAGE;
    assert_eq!(
        ws_auth_mode(&state.token_auth, &origin).err().as_deref(),
        Some(message)
    );
    assert_eq!(state.user_credential().err().as_deref(), Some(message));
    assert_eq!(state.signing_keys().err().as_deref(), Some(message));
    assert!(state.native_auth().is_err());
    // The retained key itself is untouched.
    assert_eq!(state.keys.lock().unwrap().public_key(), identity);
    state
        .token_auth
        .signed_out
        .clear(&origin, Some(identity))
        .unwrap();
    assert!(ws_auth_mode(&state.token_auth, &origin).is_ok());
    assert!(state.signing_keys().is_ok());
}

#[test]
fn stop_agent_ids_are_validated() {
    assert_eq!(
        normalize_agent_pubkeys(vec!["AB".repeat(32)]).unwrap(),
        vec!["ab".repeat(32)]
    );
    assert!(normalize_agent_pubkeys(vec!["nope".into()]).is_err());
}

#[test]
fn status_shape() {
    let signed_out = status_of("http://o", Some(vec!["google".into()]), None);
    assert!(signed_out.supported);
    assert_eq!(signed_out.state, "signed_out");
    let unsupported = status_of("http://o", None, None);
    assert!(!unsupported.supported);
    let needs = status_of(
        "http://o",
        Some(vec![]),
        Some(OriginAuth::NeedsLogin("refresh_reused".into())),
    );
    assert_eq!(needs.state, "needs_login");
    assert_eq!(needs.reason.as_deref(), Some("refresh_reused"));
    let json = serde_json::to_value(&needs).unwrap();
    assert!(json.get("deviceId").is_some(), "camelCase wire shape");
    assert_eq!(json.get("deviceSignedOut"), Some(&serde_json::json!(false)));
}

#[test]
fn sign_out_only_stops_the_active_accounts_own_agents() {
    let record = |pubkey: &str| -> crate::managed_agents::ManagedAgentRecord {
        serde_json::from_value(serde_json::json!({
            "pubkey": pubkey, "name": "a", "relay_url": "wss://relay.test",
            "acp_command": "x", "agent_command": "x", "agent_args": [],
            "mcp_command": "x", "turn_timeout_seconds": 1,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap()
    };
    let (mine, theirs, unowned) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
    let me = "1".repeat(64);
    let records = [record(&mine), record(&theirs), record(&unowned)];
    let owner_of = |r: &crate::managed_agents::ManagedAgentRecord| match r.pubkey.as_str() {
        p if p == mine => Some(me.to_ascii_uppercase()),
        p if p == theirs => Some("2".repeat(64)),
        _ => None,
    };
    let asked = vec![
        mine.clone(),
        theirs.clone(),
        unowned.clone(),
        "d".repeat(64),
    ];
    assert_eq!(owned_by(asked, &records, &me, owner_of), vec![mine]);
}

#[test]
fn signing_in_starts_that_accounts_auto_start_and_stopped_agents_only() {
    let record = |pubkey: &str, auto: bool| -> crate::managed_agents::ManagedAgentRecord {
        let mut record: crate::managed_agents::ManagedAgentRecord =
            serde_json::from_value(serde_json::json!({
                "pubkey": pubkey, "name": "a", "relay_url": "wss://relay.test",
                "acp_command": "x", "agent_command": "x", "agent_args": [],
                "mcp_command": "x", "turn_timeout_seconds": 1,
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
            }))
            .unwrap();
        record.start_on_app_launch = auto;
        record
    };
    let (auto, stopped, idle, theirs) = (
        "a".repeat(64),
        "b".repeat(64),
        "c".repeat(64),
        "d".repeat(64),
    );
    let me = "1".repeat(64);
    let records = [
        record(&auto, true),
        record(&stopped, false),
        record(&idle, false),
        record(&theirs, true),
    ];
    let owner_of = |r: &crate::managed_agents::ManagedAgentRecord| {
        Some(if r.pubkey == theirs {
            "2".repeat(64)
        } else {
            me.clone()
        })
    };
    assert_eq!(
        owner_agents_to_start(&records, &me, &[stopped.to_ascii_uppercase()], owner_of),
        vec![auto, stopped]
    );
}
