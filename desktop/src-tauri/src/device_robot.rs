//! Which device each of the owner's agents runs on.
//!
//! Every relay device (one Google sign-in on one computer) gets a robot the
//! frontend (`shared/lib/deviceRobot.ts`) and mobile (`device_robot.dart`)
//! derive from FNV-1a of its device id; `test-fixtures/device-robots.json`
//! pins the hash for all three.
//!
//! This desktop tells the owner which agents it hosts by publishing one
//! author-only `kind:30180` event per device (`d` = device id, content =
//! `{"v":1,"agents":[...]}`), signed with the owner's credential. With several
//! accounts signed in, each account has its own device here and lists only
//! its own agents. The relay serves it only to its author, so nobody else
//! learns where an agent runs.
//! Agent kind:0 profiles no longer carry a device; [`RETIRED_HOST_DEVICE_FIELD`]
//! is only read so reconciliation can republish profiles without it.

use std::collections::HashMap;
use std::sync::Mutex;

use nostr::{EventBuilder, Kind, Tag};
use tauri::Manager;

use crate::app_state::AppState;
use crate::auth::credential::UserCredential;
use crate::auth::{OriginAuth, TokenAuthState};
use crate::managed_agents::{BackendKind, ManagedAgentRecord};

/// Retired kind:0 field that once published an agent's host device tag.
pub(crate) const RETIRED_HOST_DEVICE_FIELD: &str = "kuzz_host_device";

/// Author-only agent host-devices kind (see `kuzz_core::kind`).
pub(crate) const KIND_AGENT_HOST_DEVICES: u16 = 30180;

/// Author-only device robot kind (see `kuzz_core::kind`).
const KIND_DEVICE_ROBOT: u16 = 30181;

/// Content of the last accepted publish per `(origin, d tag)`, so repeated
/// triggers (token refreshes, agent restarts) do not republish an unchanged list.
static LAST_PUBLISHED: Mutex<Option<HashMap<(String, String), String>>> = Mutex::new(None);

/// Serializes publishes end to end (read list, compare, sign, submit), so a
/// burst of triggers cannot let an older list land after a newer one.
static PUBLISH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `created_at` of the last publish: replaceable events tie-break same-second
/// writes by id, so each publish must be strictly newer than the previous one.
static LAST_CREATED_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `now`, or one second after `previous` when that is not earlier.
pub(crate) fn next_created_at(now: u64, previous: u64) -> u64 {
    now.max(previous.saturating_add(1))
}

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
#[cfg(test)]
fn local_device_id(state: &AppState) -> Option<(String, String)> {
    let origin = state.current_auth_origin();
    session_device_id(&state.token_auth, &origin).map(|id| (origin, id))
}

/// The device id of `origin`'s live session in `auth`, normalized.
fn session_device_id(auth: &TokenAuthState, origin: &str) -> Option<String> {
    match auth.get(origin) {
        Some((_, OriginAuth::Active(session))) => {
            let id = session.device_id?.trim().to_lowercase();
            (!id.is_empty()).then_some(id)
        }
        _ => None,
    }
}

/// Pubkeys of the agents `owner` runs on this desktop, sorted and
/// deduplicated. Provider-backend agents run elsewhere and are left out, and
/// so is every agent of another account signed in here (or of no known owner).
pub(crate) fn hosted_agent_pubkeys(
    records: &[ManagedAgentRecord],
    owner: &str,
    owner_of: impl Fn(&ManagedAgentRecord) -> Option<String>,
) -> Vec<String> {
    let mut pubkeys: Vec<String> = records
        .iter()
        .filter(|record| record.backend == BackendKind::Local)
        .filter(|record| owner_of(record).is_some_and(|hex| hex.eq_ignore_ascii_case(owner)))
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

/// One account signed in here on the current relay: its own device there and
/// the credential its list is signed with (that account is the owner listed).
struct AccountDevice {
    device_id: String,
    credential: UserCredential,
    /// A background account: its robot is assigned here, not by the UI.
    background: bool,
}

/// Every account signed in on this desktop for `origin` with a known device:
/// the active one, then each background one, signed with its own key.
fn signed_in_devices(state: &AppState, origin: &str) -> Vec<AccountDevice> {
    let mut devices = Vec::new();
    let active = match (
        session_device_id(&state.token_auth, origin),
        state.user_credential(),
    ) {
        (Some(device_id), Ok(credential)) => {
            let owner = credential.public_key();
            devices.push(AccountDevice {
                device_id,
                credential,
                background: false,
            });
            Some(owner)
        }
        _ => None,
    };
    for (account, auth) in state.token_auth.inactive.on_origin(origin) {
        if Some(account) == active {
            continue;
        }
        let Some(device_id) = session_device_id(&auth, origin) else {
            continue;
        };
        match account_credential(&account) {
            Ok(credential) => devices.push(AccountDevice {
                device_id,
                credential,
                background: true,
            }),
            Err(error) => eprintln!(
                "kuzz-desktop: agent host devices of {}: {error}",
                &account.to_hex()[..8]
            ),
        }
    }
    devices
}

/// A background account's own signing credential.
fn account_credential(account: &nostr::PublicKey) -> Result<UserCredential, String> {
    crate::auth::accounts::account_keys(crate::auth::accounts::shared_secrets(), account)
        .map(UserCredential::Keys)
}

/// Sign and submit `account`'s list for `device_id`, strictly newer than any
/// earlier publish, and remember it as published.
async fn submit_host_devices(
    state: &AppState,
    origin: &str,
    device_id: &str,
    credential: &UserCredential,
    content: String,
) -> Result<(), String> {
    let created_at = next_created_at(
        nostr::Timestamp::now().as_u64(),
        LAST_CREATED_AT.load(std::sync::atomic::Ordering::Acquire),
    );
    let builder = EventBuilder::new(Kind::Custom(KIND_AGENT_HOST_DEVICES), content.clone())
        .tags([Tag::identifier(device_id)])
        .custom_created_at(nostr::Timestamp::from(created_at));
    let api_base_url = crate::relay::relay_api_base_url_with_override(state);
    crate::relay::submit_event_at_with_keys(builder, state, &api_base_url, credential).await?;
    LAST_CREATED_AT.store(created_at, std::sync::atomic::Ordering::Release);
    LAST_PUBLISHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert((origin.to_owned(), device_id.to_owned()), content);
    Ok(())
}

/// Publish each signed-in account's hosted-agent list for its own device,
/// signed as that account, unless unchanged since the last publish. No-op for
/// an account without a known device on the current relay.
pub(crate) async fn publish_agent_host_devices(app: &tauri::AppHandle) -> Result<(), String> {
    let _publishing = PUBLISH_LOCK.lock().await;
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let devices = signed_in_devices(&state, &origin);
    if devices.is_empty() {
        return Ok(());
    }
    let records = crate::managed_agents::load_managed_agents_without_keys(app)?;
    let mut first_error = None;
    for device in devices {
        if device.background {
            assign_background_robot(&state, &device).await;
        }
        let owner = device.credential.public_key().to_hex();
        let content = host_devices_content(&hosted_agent_pubkeys(&records, &owner, |record| {
            crate::managed_agents::visible_owner_hex(app, record)
        }));
        let unchanged = LAST_PUBLISHED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|published| published.get(&(origin.clone(), device.device_id.clone())))
            == Some(&content);
        if unchanged {
            continue;
        }
        if let Err(error) = submit_host_devices(
            &state,
            &origin,
            &device.device_id,
            &device.credential,
            content,
        )
        .await
        {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Robot palette size and shapes, in assignment order (desktop
/// `shared/lib/deviceRobot.ts`, mobile `device_robot.dart`; pinned by
/// `test-fixtures/device-robots.json`).
const ROBOT_PAINT_COUNT: usize = 9;
const ROBOT_SHAPES: [&str; 6] = ["classic", "dome", "boxy", "visor", "tall", "hex"];

/// The paint index a `kind:30181` content stores, or `None` when malformed.
fn stored_robot_paint(content: &str) -> Option<usize> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    if value.get("v")?.as_u64()? != 2 {
        return None;
    }
    let shape = value.get("shape")?.as_str()?;
    if !ROBOT_SHAPES.contains(&shape) {
        return None;
    }
    let color = usize::try_from(value.get("color")?.as_u64()?).ok()?;
    (color < ROBOT_PAINT_COUNT).then_some(color)
}

/// Each device's stored paint: the newest well-formed `kind:30181` event of
/// `owner` per device wins (a malformed head never hides an older valid one).
fn stored_robot_paints(
    events: &[nostr::Event],
    owner: &nostr::PublicKey,
) -> HashMap<String, usize> {
    let mut newest: HashMap<String, (u64, usize)> = HashMap::new();
    for event in events {
        if event.kind != Kind::Custom(KIND_DEVICE_ROBOT) || event.pubkey != *owner {
            continue;
        }
        let Some(device_id) = event
            .tags
            .identifier()
            .map(|id| id.trim().to_lowercase())
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let Some(paint) = stored_robot_paint(&event.content) else {
            continue;
        };
        let created_at = event.created_at.as_secs();
        if newest
            .get(&device_id)
            .is_none_or(|(current, _)| created_at > *current)
        {
            newest.insert(device_id, (created_at, paint));
        }
    }
    newest
        .into_iter()
        .map(|(device_id, (_, paint))| (device_id, paint))
        .collect()
}

/// The `kind:30181` content for `device_id` when it has no stored robot yet:
/// the first slot whose paint no stored robot uses, else the next slot past
/// the palette (`nextDeviceRobotAssignments` for one device).
fn next_robot_content(device_id: &str, stored: &HashMap<String, usize>) -> Option<String> {
    if stored.contains_key(device_id) {
        return None;
    }
    let slot = (0..ROBOT_PAINT_COUNT)
        .find(|paint| !stored.values().any(|used| used == paint))
        .unwrap_or(stored.len());
    Some(
        serde_json::json!({
            "v": 2,
            "shape": ROBOT_SHAPES[slot % ROBOT_SHAPES.len()],
            "color": slot % ROBOT_PAINT_COUNT,
        })
        .to_string(),
    )
}

/// `(account, device)` pairs whose robot was already tried in this app run:
/// assignment is cosmetic and never retried in a loop.
static ROBOT_ATTEMPTED: Mutex<Option<std::collections::HashSet<(String, String)>>> =
    Mutex::new(None);

/// Give a background account's device here its robot when it has none yet,
/// once per app run, signed as that account. The active account's UI does
/// the same for its own device (`useAutoAssignDeviceRobots`), so a device's
/// look is fixed from its first sign-in, not from its first time active.
async fn assign_background_robot(state: &AppState, device: &AccountDevice) {
    let owner = device.credential.public_key();
    let first_try = ROBOT_ATTEMPTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(Default::default)
        .insert((owner.to_hex(), device.device_id.clone()));
    if !first_try {
        return;
    }
    let api_base_url = crate::relay::relay_api_base_url_with_override(state);
    let filter = serde_json::json!({
        "kinds": [KIND_DEVICE_ROBOT],
        "authors": [owner.to_hex()],
        "limit": 400,
    });
    let events = match crate::relay::query_relay_at_with_keys(
        state,
        &api_base_url,
        &[filter],
        &device.credential,
        None,
    )
    .await
    {
        Ok(events) => events,
        Err(error) => {
            eprintln!("kuzz-desktop: could not read device robots: {error}");
            return;
        }
    };
    let Some(content) =
        next_robot_content(&device.device_id, &stored_robot_paints(&events, &owner))
    else {
        return;
    };
    let builder = EventBuilder::new(Kind::Custom(KIND_DEVICE_ROBOT), content)
        .tags([Tag::identifier(device.device_id.clone())]);
    if let Err(error) =
        crate::relay::submit_event_at_with_keys(builder, state, &api_base_url, &device.credential)
            .await
    {
        eprintln!("kuzz-desktop: could not store a device robot: {error}");
    }
}

/// Before `account` signs out on `origin`: publish an empty list for its
/// device here, so its other devices stop showing its agents on this one.
/// Best-effort (the sign-out goes ahead either way).
pub(crate) async fn clear_agent_host_devices(
    state: &AppState,
    origin: &str,
    account: Option<nostr::PublicKey>,
) {
    let _publishing = PUBLISH_LOCK.lock().await;
    let active = state.current_identity_pubkey().ok();
    let target = match account {
        Some(account) if Some(account) != active => state
            .token_auth
            .inactive
            .on_origin(origin)
            .into_iter()
            .find(|(pubkey, _)| *pubkey == account)
            .and_then(|(_, auth)| session_device_id(&auth, origin))
            .map(|device_id| (device_id, account_credential(&account))),
        _ => session_device_id(&state.token_auth, origin)
            .map(|device_id| (device_id, state.user_credential())),
    };
    let Some((device_id, Ok(credential))) = target else {
        return;
    };
    let content = host_devices_content(&[]);
    if let Err(error) = submit_host_devices(state, origin, &device_id, &credential, content).await {
        eprintln!("kuzz-desktop: could not clear agent host devices before sign-out: {error}");
    }
}

/// Waits before each retry of a failed publish (e.g. right after sign-in,
/// before the relay connection is up). Each attempt re-reads the current list.
const PUBLISH_RETRY_DELAYS_SECS: [u64; 5] = [2, 5, 15, 30, 60];

/// Fire-and-forget [`publish_agent_host_devices`], retried on failure so a
/// device's agents are never left listed under an older sign-in.
pub(crate) fn spawn_publish_agent_host_devices<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let Some(app) = app
        .try_state::<AppState>()
        .and_then(|state| state.app_handle.lock().ok().and_then(|guard| guard.clone()))
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let mut delays = PUBLISH_RETRY_DELAYS_SECS.iter();
        loop {
            let Err(error) = publish_agent_host_devices(&app).await else {
                return;
            };
            eprintln!("kuzz-desktop: could not publish agent host devices: {error}");
            let Some(&delay) = delays.next() else {
                return;
            };
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
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
    fn robot_slots_match_the_cross_platform_fixture() {
        let fixture = fixture();
        assert_eq!(
            fixture["paints"].as_array().expect("paints").len(),
            ROBOT_PAINT_COUNT
        );
        let shapes: Vec<&str> = fixture["shapes"]
            .as_array()
            .expect("shapes")
            .iter()
            .map(|shape| shape.as_str().expect("shape"))
            .collect();
        assert_eq!(shapes, ROBOT_SHAPES);
        // The k-th device of an account, each earlier one holding its slot.
        for vector in fixture["slots"].as_array().expect("slots") {
            let slot = vector["slot"].as_u64().expect("slot") as usize;
            let stored: HashMap<String, usize> = (0..slot)
                .map(|k| (format!("device-{k}"), k % ROBOT_PAINT_COUNT))
                .collect();
            let content: Value =
                serde_json::from_str(&next_robot_content("new-device", &stored).expect("assigned"))
                    .expect("content is JSON");
            assert_eq!(content["v"], 2);
            assert_eq!(content["color"], vector["colorIndex"], "slot {slot}");
            assert_eq!(content["shape"], vector["shape"], "slot {slot}");
        }
    }

    #[test]
    fn a_device_with_a_robot_keeps_it_and_freed_paints_come_first() {
        let stored: HashMap<String, usize> = [("a".to_owned(), 0), ("b".to_owned(), 2)]
            .into_iter()
            .collect();
        assert_eq!(next_robot_content("a", &stored), None);
        let content: Value =
            serde_json::from_str(&next_robot_content("c", &stored).expect("assigned"))
                .expect("content is JSON");
        assert_eq!(
            content,
            serde_json::json!({ "v": 2, "shape": "dome", "color": 1 })
        );
    }

    #[test]
    fn stored_robots_take_each_devices_newest_valid_choice() {
        let keys = nostr::Keys::generate();
        let event = |device: &str, content: &str, at: u64| {
            EventBuilder::new(Kind::Custom(KIND_DEVICE_ROBOT), content)
                .tags([Tag::identifier(device)])
                .custom_created_at(nostr::Timestamp::from(at))
                .sign_with_keys(&keys)
                .expect("sign")
        };
        let stranger = EventBuilder::new(
            Kind::Custom(KIND_DEVICE_ROBOT),
            r#"{"v":2,"shape":"dome","color":4}"#,
        )
        .tags([Tag::identifier("C")])
        .sign_with_keys(&nostr::Keys::generate())
        .expect("sign");
        let events = vec![
            event("A", r#"{"v":2,"shape":"dome","color":3}"#, 10),
            event("a", r#"{"v":2,"shape":"boxy","color":5}"#, 20),
            event("B", r#"{"v":2,"shape":"dome","color":1}"#, 10),
            event("B", r#"{"v":1,"shape":"dome","color":2}"#, 30),
            event("B", r#"{"v":2,"shape":"nope","color":2}"#, 31),
            event("B", r#"{"v":2,"shape":"dome","color":9}"#, 32),
            stranger,
        ];
        let stored = stored_robot_paints(&events, &keys.public_key());
        assert_eq!(
            stored,
            [("a".to_owned(), 5), ("b".to_owned(), 1)]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn each_publish_is_strictly_newer_than_the_last() {
        assert_eq!(next_created_at(100, 0), 100);
        assert_eq!(next_created_at(100, 100), 101);
        assert_eq!(next_created_at(100, 150), 151);
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
            Some((
                origin.clone(),
                "3f2504e0-4f89-41d3-9a0c-0305e82c3301".into()
            ))
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
                "acp_command": "kuzz-acp",
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
        let mine = |_: &ManagedAgentRecord| Some("0".repeat(64));
        assert_eq!(
            hosted_agent_pubkeys(&records, &"0".repeat(64), mine),
            vec!["a".repeat(64), "b".repeat(64)]
        );
        let content: Value = serde_json::from_str(&host_devices_content(&hosted_agent_pubkeys(
            &records,
            &"0".repeat(64),
            mine,
        )))
        .expect("content is JSON");
        assert_eq!(
            content,
            serde_json::json!({ "v": 1, "agents": ["a".repeat(64), "b".repeat(64)] })
        );
    }

    #[test]
    fn each_account_lists_only_its_own_agents() {
        let record = |pubkey: &str| {
            let record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
                "pubkey": pubkey,
                "name": "Scout",
                "private_key_nsec": "",
                "relay_url": "",
                "acp_command": "kuzz-acp",
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
            record
        };
        let (owner_a, owner_b) = ("a".repeat(64), "B".repeat(64));
        let (agent_a, agent_b, orphan) = ("1".repeat(64), "2".repeat(64), "3".repeat(64));
        let records = vec![record(&agent_a), record(&agent_b), record(&orphan)];
        let owner_of = |record: &ManagedAgentRecord| {
            if record.pubkey == agent_a {
                Some(owner_a.clone())
            } else if record.pubkey == agent_b {
                Some(owner_b.clone())
            } else {
                None
            }
        };
        assert_eq!(
            hosted_agent_pubkeys(&records, &owner_a, owner_of),
            vec![agent_a.clone()]
        );
        // Owners compare case-insensitively; an agent with no known owner is
        // listed for nobody.
        assert_eq!(
            hosted_agent_pubkeys(&records, &"b".repeat(64), owner_of),
            vec![agent_b.clone()]
        );
        assert!(hosted_agent_pubkeys(&records, &"c".repeat(64), owner_of).is_empty());
    }
}
