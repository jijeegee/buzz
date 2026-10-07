//! Per-device robot tags for agent profiles.
//!
//! A relay device (one Google sign-in on one computer) is identified to other
//! clients only by an 8-hex-digit tag: FNV-1a (32-bit) of its lowercased
//! device id. When this desktop signs a kind:0 for an agent it runs locally,
//! it stamps the tag as `buzz_host_device`, so the agent's owner sees the
//! robot of the computer it runs on and finds the same robot in the device
//! list. The frontend (`shared/lib/deviceRobot.ts`) and mobile
//! (`device_robot.dart`) derive identical robots from the tag;
//! `test-fixtures/device-robots.json` pins all three.

use serde_json::Value;

use crate::app_state::AppState;
use crate::auth::OriginAuth;
use crate::managed_agents::BackendKind;

/// kind:0 content field carrying the host device tag of an agent.
pub(crate) const HOST_DEVICE_FIELD: &str = "buzz_host_device";

/// FNV-1a (32-bit) of the trimmed, lowercased device id as 8 hex digits;
/// `None` for an empty id.
pub(crate) fn device_tag(device_id: &str) -> Option<String> {
    let normalized = device_id.trim().to_lowercase();
    if normalized.is_empty() {
        return None;
    }
    let hash = normalized.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    Some(format!("{hash:08x}"))
}

/// Whether `tag` is a well-formed device tag (8 lowercase hex digits).
pub(crate) fn is_device_tag(tag: &str) -> bool {
    tag.len() == 8
        && tag
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The well-formed `buzz_host_device` of parsed kind:0 content, if any.
pub(crate) fn host_device_from_content(content: &Value) -> Option<String> {
    content
        .get(HOST_DEVICE_FIELD)
        .and_then(Value::as_str)
        .filter(|tag| is_device_tag(tag))
        .map(str::to_string)
}

/// The tag of this desktop's relay device for `relay_url`, when an agent with
/// `agent_pubkey` runs here. `None` when this desktop has no signed-in device
/// on that relay (key-only sign-in, session restoring) or the agent runs on a
/// provider backend elsewhere: such profiles carry no tag and show the
/// default robot.
pub(crate) fn local_host_device_tag(
    state: &AppState,
    relay_url: &str,
    agent_pubkey: &str,
) -> Option<String> {
    let origin = crate::auth::origin_for(relay_url);
    let device_id = match state.token_auth.get(&origin) {
        Some((_, OriginAuth::Active(session))) => session.device_id?,
        _ => return None,
    };
    if runs_on_provider_backend(state, agent_pubkey) {
        return None;
    }
    device_tag(&device_id)
}

/// Whether the stored record for `agent_pubkey` runs on a provider backend
/// (another machine). An agent without a readable record is treated as local:
/// every kind:0 this desktop signs for it comes from a local create flow.
fn runs_on_provider_backend(state: &AppState, agent_pubkey: &str) -> bool {
    let app = state
        .app_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some(app) = app else {
        return false;
    };
    crate::managed_agents::load_managed_agents_without_keys(&app)
        .ok()
        .and_then(|records| {
            records
                .into_iter()
                .find(|record| record.pubkey.eq_ignore_ascii_case(agent_pubkey))
        })
        .is_some_and(|record| matches!(record.backend, BackendKind::Provider { .. }))
}

/// Whether a published profile's host device differs from `expected`. An
/// unknown `expected` (no signed-in device right now) never forces a
/// republish, so a restoring session cannot strip a valid tag.
pub(crate) fn host_device_stale(published: Option<&str>, expected: Option<&str>) -> bool {
    expected.is_some_and(|expected| published != Some(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test-fixtures/device-robots.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).expect("read fixture"))
            .expect("parse fixture")
    }

    #[test]
    fn device_tags_match_the_cross_platform_fixture() {
        let fixture = fixture();
        let devices = fixture["devices"].as_array().expect("devices");
        assert!(!devices.is_empty());
        for vector in devices {
            let id = vector["deviceId"].as_str().expect("deviceId");
            assert_eq!(
                device_tag(id).as_deref(),
                vector["tag"].as_str(),
                "device {id:?}"
            );
        }
        for id in fixture["emptyDeviceIds"].as_array().expect("empty ids") {
            assert_eq!(device_tag(id.as_str().expect("id")), None);
        }
    }

    #[test]
    fn only_well_formed_tags_are_read_from_profiles() {
        let fixture = fixture();
        for tag in fixture["invalidTags"].as_array().expect("invalid tags") {
            let content = serde_json::json!({ HOST_DEVICE_FIELD: tag });
            assert_eq!(host_device_from_content(&content), None, "{tag}");
        }
        let content = serde_json::json!({ HOST_DEVICE_FIELD: "e8c41c31" });
        assert_eq!(
            host_device_from_content(&content).as_deref(),
            Some("e8c41c31")
        );
        assert_eq!(host_device_from_content(&serde_json::json!({})), None);
        assert_eq!(
            host_device_from_content(&serde_json::json!({ HOST_DEVICE_FIELD: 7 })),
            None
        );
    }

    #[test]
    fn unknown_local_device_never_forces_a_republish() {
        assert!(!host_device_stale(Some("e8c41c31"), None));
        assert!(!host_device_stale(None, None));
        assert!(!host_device_stale(Some("e8c41c31"), Some("e8c41c31")));
        assert!(host_device_stale(None, Some("e8c41c31")));
        assert!(host_device_stale(Some("f108e530"), Some("e8c41c31")));
    }

    #[test]
    fn signed_in_device_tags_local_agents_only_for_its_relay() {
        let state = crate::app_state::build_app_state();
        let relay = "wss://relay.example.com";
        let origin = crate::auth::origin_for(relay);
        assert_eq!(local_host_device_tag(&state, relay, "a"), None);
        state.token_auth.set(
            &origin,
            OriginAuth::Active(crate::auth::UserSession {
                principal: nostr::Keys::generate().public_key(),
                device_id: Some("3F2504E0-4F89-41D3-9A0C-0305E82C3301".into()),
                access: String::new().into(),
                refresh: String::new().into(),
                access_issued_at: 0,
                access_expires_at: 0,
            }),
        );
        assert_eq!(
            local_host_device_tag(&state, relay, "a").as_deref(),
            Some("e8c41c31")
        );
        assert_eq!(
            local_host_device_tag(&state, "wss://other.example.com", "a"),
            None
        );
        state.token_auth.set(&origin, OriginAuth::Restoring);
        assert_eq!(local_host_device_tag(&state, relay, "a"), None);
    }
}
