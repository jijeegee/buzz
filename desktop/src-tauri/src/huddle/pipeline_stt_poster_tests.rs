//! STT transcript identity: token mode posts as the Google-session principal
//! with its current bearer; key mode keeps signing with the local key.

use super::{sign_and_guard_stt_body, SttPoster};
use crate::app_state::build_app_state;
use crate::auth::{OriginAuth, UserSession};

fn active(principal: nostr::PublicKey, access: &str) -> OriginAuth {
    OriginAuth::Active(UserSession {
        principal,
        device_id: None,
        access: zeroize::Zeroizing::new(access.into()),
        refresh: zeroize::Zeroizing::new("bzr_test".into()),
        access_issued_at: 0,
        access_expires_at: i64::MAX,
    })
}

/// Token mode posts transcripts as the principal with the session's current
/// bearer, re-read per post, and never falls back to the local key.
#[test]
fn token_mode_transcripts_post_as_the_principal_with_the_current_bearer() {
    let state = build_app_state();
    let origin = state.current_auth_origin();
    let principal = nostr::Keys::generate().public_key();
    state.token_auth.set(&origin, active(principal, "bzs_one"));

    let poster = SttPoster::for_state(&state).expect("poster");
    let signer = poster.credential().expect("credential");
    let body =
        sign_and_guard_stt_body(nostr::EventBuilder::text_note("hi"), &signer).expect("body");
    let event: nostr::Event = serde_json::from_slice(&body).expect("event");
    assert_eq!(event.pubkey, principal, "posted as the principal");
    assert_ne!(
        event.pubkey,
        state.keys.lock().expect("keys").public_key(),
        "never as the local key"
    );
    assert_eq!(
        signer
            .http_auth(&reqwest::Method::POST, "http://relay.test/", &body)
            .expect("auth"),
        "Bearer bzs_one"
    );

    // A rotated access token is picked up by the next post.
    state.token_auth.set(&origin, active(principal, "bzs_two"));
    let signer = poster.credential().expect("credential");
    assert_eq!(
        signer
            .http_auth(&reqwest::Method::POST, "http://relay.test/", b"")
            .expect("auth"),
        "Bearer bzs_two"
    );

    // Signed out mid-huddle: refuse instead of using the local key.
    state.token_auth.set(&origin, OriginAuth::SignedOut);
    assert!(poster.credential().is_err());
}

#[test]
fn key_mode_transcripts_still_sign_with_the_local_key() {
    let state = build_app_state();
    let poster = SttPoster::for_state(&state).expect("poster");
    let signer = poster.credential().expect("credential");
    let body =
        sign_and_guard_stt_body(nostr::EventBuilder::text_note("hi"), &signer).expect("body");
    let event: nostr::Event = serde_json::from_slice(&body).expect("event");
    assert_eq!(event.pubkey, state.keys.lock().expect("keys").public_key());
    assert!(event.verify().is_ok());
}
