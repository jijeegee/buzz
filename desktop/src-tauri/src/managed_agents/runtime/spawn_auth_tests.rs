//! Rule 3: `spawn_agent_child` injects the child's relay credential through
//! `apply_child_relay_auth`; these bind that production seam and read the
//! resulting `Command` env as ground truth.

use std::ffi::OsStr;

use zeroize::Zeroizing;

use super::super::apply_child_relay_auth;
use super::super::test_fixtures::fixture;
use crate::auth::{origin_for, OriginAuth, UserSession};
use crate::managed_agents::types::RespondTo;

fn env_of<'a>(command: &'a std::process::Command, key: &str) -> Option<Option<&'a OsStr>> {
    command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new(key))
        .map(|(_, value)| value)
}

#[test]
fn token_mode_child_gets_the_bot_token_and_never_the_key() {
    let state = crate::app_state::build_app_state();
    let record = fixture(RespondTo::OwnerOnly, vec![], Some("tag".into()));
    let origin = origin_for(&record.relay_url);
    state.token_auth.set(
        &origin,
        OriginAuth::Active(UserSession {
            principal: nostr::Keys::generate().public_key(),
            device_id: None,
            access: Zeroizing::new("bzs_user".into()),
            refresh: Zeroizing::new("bzr_user".into()),
            access_issued_at: 0,
            access_expires_at: i64::MAX,
        }),
    );
    state
        .token_auth
        .bots
        .stage_for_test(&record.pubkey, &origin, "bzb_child");
    let mut command = std::process::Command::new("buzz-acp");
    command.env("BUZZ_PRIVATE_KEY", "nsec1leaked");
    let auth = apply_child_relay_auth(&state, &mut command, &record, &record.relay_url).unwrap();
    assert!(matches!(auth, crate::auth::bots::SpawnAuth::Token(_)));
    assert_eq!(
        env_of(&command, "BUZZ_BOT_TOKEN"),
        Some(Some(OsStr::new("bzb_child")))
    );
    assert_eq!(env_of(&command, "BUZZ_PRIVATE_KEY"), Some(None));
    assert_eq!(env_of(&command, "BUZZ_AUTH_TAG"), Some(None));
}

#[test]
fn key_mode_child_keeps_its_key_and_gets_no_token() {
    let state = crate::app_state::build_app_state();
    let record = fixture(RespondTo::OwnerOnly, vec![], Some("tag".into()));
    let mut command = std::process::Command::new("buzz-acp");
    command.env("BUZZ_BOT_TOKEN", "bzb_stray");
    let auth = apply_child_relay_auth(&state, &mut command, &record, &record.relay_url).unwrap();
    assert!(matches!(auth, crate::auth::bots::SpawnAuth::Keys));
    assert_eq!(
        env_of(&command, "BUZZ_PRIVATE_KEY"),
        Some(Some(OsStr::new("nsec1fake")))
    );
    assert_eq!(
        env_of(&command, "BUZZ_AUTH_TAG"),
        Some(Some(OsStr::new("tag")))
    );
    assert_eq!(env_of(&command, "BUZZ_BOT_TOKEN"), Some(None));
}

#[test]
fn google_backup_session_keeps_agent_key_owner_attestation_and_record_identity() {
    let state = crate::app_state::build_app_state();
    let record = fixture(
        RespondTo::OwnerOnly,
        vec![],
        Some("owner-attestation".into()),
    );
    let origin = origin_for(&record.relay_url);
    state
        .token_auth
        .set_key_backup(&origin, state.keys.lock().unwrap().public_key());
    state
        .token_auth
        .set(&origin, OriginAuth::NeedsLogin("expired".into()));
    let before = serde_json::to_value(&record).unwrap();
    let mut command = std::process::Command::new("buzz-acp");
    let auth = apply_child_relay_auth(&state, &mut command, &record, &record.relay_url).unwrap();
    assert!(matches!(auth, crate::auth::bots::SpawnAuth::Keys));
    assert_eq!(
        env_of(&command, "BUZZ_PRIVATE_KEY"),
        Some(Some(OsStr::new("nsec1fake")))
    );
    assert_eq!(
        env_of(&command, "BUZZ_AUTH_TAG"),
        Some(Some(OsStr::new("owner-attestation")))
    );
    assert_eq!(env_of(&command, "BUZZ_BOT_TOKEN"), Some(None));
    assert_eq!(serde_json::to_value(&record).unwrap(), before);
}

#[test]
fn google_backup_session_reports_existing_token_bot_without_recreating_it() {
    let state = crate::app_state::build_app_state();
    let mut record = fixture(
        RespondTo::OwnerOnly,
        vec![],
        Some("owner-attestation".into()),
    );
    let origin = origin_for(&record.relay_url);
    record.bot_origin = Some(origin.clone());
    state
        .token_auth
        .set_key_backup(&origin, state.keys.lock().unwrap().public_key());
    let before = serde_json::to_value(&record).unwrap();
    let mut command = std::process::Command::new("buzz-acp");
    let result = apply_child_relay_auth(&state, &mut command, &record, &record.relay_url);
    assert!(
        matches!(result, Err(ref message) if message.contains("earlier token account") && message.contains("cannot merge or recreate"))
    );
    assert_eq!(serde_json::to_value(&record).unwrap(), before);
}
