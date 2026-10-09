//! Which device each of the owner's agents runs on.
//!
//! Every relay device (one Google sign-in on one computer) gets a robot the
//! frontend (`shared/lib/deviceRobot.ts`) and mobile (`device_robot.dart`)
//! derive from FNV-1a of its device id; `test-fixtures/device-robots.json`
//! pins the hash for all three.
//!
//! This desktop tells the owner which agents it hosts by publishing one
//! author-only `kind:30180` event per device (`d` = device id, content =
//! `{"v":1,"agents":[...]}`), signed with the owner's credential. The relay
//! serves it only to its author, so nobody else learns where an agent runs.
//! Agent kind:0 profiles no longer carry a device; [`RETIRED_HOST_DEVICE_FIELD`]
//! is only read so reconciliation can republish profiles without it.

use std::sync::Mutex;

use nostr::{EventBuilder, Kind, Tag};
use tauri::Manager;

use crate::app_state::AppState;
use crate::auth::OriginAuth;
use crate::managed_agents::{BackendKind, ManagedAgentRecord};

/// Retired kind:0 field that once published an agent's host device tag.
pub(crate) const RETIRED_HOST_DEVICE_FIELD: &str = "buzz_host_device";

/// Author-only agent host-devices kind (see `buzz_core::kind`).
pub(crate) const KIND_AGENT_HOST_DEVICES: u16 = 30180;

/// `(origin, d tag, content)` of the last accepted publish, so repeated
/// triggers (token refreshes, agent restarts) do not republish an unchanged list.
static LAST_PUBLISHED: Mutex<Option<(String, String, String)>> = Mutex::new(None);

/// FNV-1a (32-bit) of the trimmed, lowercased device id as 8 hex digits;
/// `None` for an empty id.
#[cfg(test)]
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

/// This desktop's relay device id for the current workspace relay, when it is
/// signed in there.
fn local_device_id(state: &AppState) -> Option<(String, String)> {
    let origin = state.current_auth_origin();
    match state.token_auth.get(&origin) {
        Some((_, OriginAuth::Active(session))) => {
            let id = session.device_id?.trim().to_lowercase();
            (!id.is_empty()).then_some((origin, id))
        }
        _ => None,
    }
}

/// Pubkeys of the agents this desktop runs itself, sorted and deduplicated.
/// Provider-backend agents run elsewhere and are left out.
pub(crate) fn hosted_agent_pubkeys(records: &[ManagedAgentRecord]) -> Vec<String> {
    let mut pubkeys: Vec<String> = records
        .iter()
        .filter(|record| record.backend == BackendKind::Local)
        .map(|record| record.pubkey.trim().to_lowercase())
        .filter(|pubkey| pubkey.len() == 64 && pubkey.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    pubkeys.sort();
    pubkeys.dedup();
    pubkeys
}

/// Content of a `kind:30180` event listing `agents`.
pub(crate) fn host_devices_content(agents: &[String]) -> String {
    serde_json::json!({ "v": 1, "agents": agents }).to_string()
}

/// Publish this device's hosted-agent list for the owner, unless it is
/// unchanged since the last publish. No-op when this desktop is not signed in
/// to the current relay with a known device.
pub(crate) async fn publish_agent_host_devices(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let Some((origin, device_id)) = local_device_id(&state) else {
        return Ok(());
    };
    let records = crate::managed_agents::load_managed_agents_without_keys(app)?;
    let content = host_devices_content(&hosted_agent_pubkeys(&records));
    let key = (origin, device_id.clone(), content.clone());
    if LAST_PUBLISHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        == Some(&key)
    {
        return Ok(());
    }
    let builder = EventBuilder::new(Kind::Custom(KIND_AGENT_HOST_DEVICES), content)
        .tags([Tag::identifier(device_id)]);
    crate::relay::submit_event(builder, &state).await?;
    *LAST_PUBLISHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(key);
    Ok(())
}

/// Fire-and-forget [`publish_agent_host_devices`].
pub(crate) fn spawn_publish_agent_host_devices<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(app) = app
        .try_state::<AppState>()
        .and_then(|state| state.app_handle.lock().ok().and_then(|guard| guard.clone()))
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        if let Err(error) = publish_agent_host_devices(&app).await {
            eprintln!("buzz-desktop: could not publish agent host devices: {error}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

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
    fn local_device_id_needs_an_active_session_on_the_current_relay() {
        let state = crate::app_state::build_app_state();
        assert_eq!(local_device_id(&state), None);
        let origin = state.current_auth_origin();
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
            local_device_id(&state),
            Some((origin.clone(), "3f2504e0-4f89-41d3-9a0c-0305e82c3301".into()))
        );
        state.token_auth.set(&origin, OriginAuth::Restoring);
        assert_eq!(local_device_id(&state), None);
    }

    #[test]
    fn hosted_agents_are_local_sorted_and_unique() {
        let record = |pubkey: &str, backend: BackendKind| {
            let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
                "pubkey": pubkey,
                "name": "Scout",
                "private_key_nsec": "",
                "relay_url": "",
                "acp_command": "buzz-acp",
                "agent_command": "goose",
                "agent_args": [],
                "mcp_command": "",
                "turn_timeout_seconds": 320,
                "system_prompt": null,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "last_started_at": null,
                "last_stopped_at": null,
                "last_exit_code": null,
                "last_error": null
            }))
            .expect("minimal record");
            record.backend = backend;
            record
        };
        let a = "b".repeat(64);
        let b = "A".repeat(64);
        let records = vec![
            record(&a, BackendKind::Local),
            record(&b, BackendKind::Local),
            record(&a, BackendKind::Local),
            record("", BackendKind::Local),
            record(
                &"c".repeat(64),
                BackendKind::Provider {
                    id: "p".into(),
                    config: serde_json::Value::Null,
                },
            ),
        ];
        assert_eq!(
            hosted_agent_pubkeys(&records),
            vec!["a".repeat(64), "b".repeat(64)]
        );
        let content: Value =
            serde_json::from_str(&host_devices_content(&hosted_agent_pubkeys(&records)))
                .expect("content is JSON");
        assert_eq!(
            content,
            serde_json::json!({ "v": 1, "agents": ["a".repeat(64), "b".repeat(64)] })
        );
    }
}
