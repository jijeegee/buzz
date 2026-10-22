//! Live key admission on the other entrances (device-bound keys plan §3, PR
//! A2): the HTTP bridge (NIP-98), Blossom media reads, git NIP-98, NIP-OA
//! owners on WS AUTH, and the closed token message paths, over the
//! production router with accounts, devices and registry rows in Postgres.

use super::device_ws::{device_instance, seed_account, seed_device, seed_registry};
use super::*;
use crate::identity::admission::test_support::{device, signed_event};
use crate::identity::config::DeviceAdmissionMode;
use crate::identity::device::Surface;
use base64::Engine as _;
use sha2::{Digest, Sha256};

fn tags(raw: &[&[&str]]) -> Vec<Vec<String>> {
    raw.iter()
        .map(|tag| tag.iter().map(|part| (*part).to_owned()).collect())
        .collect()
}

fn nostr_authorization(event: &nostr::Event) -> String {
    format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(event).expect("json"))
    )
}

/// The NIP-98 `u` scheme for this relay (`ws://` config: `http`).
fn http_url(inst: &Instance, path: &str) -> String {
    let scheme = if inst.state.config.relay_url.starts_with("wss://") {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{}{path}", inst.host)
}

/// POST /query signed by `keys` from `device_key` (`None`: no device tag);
/// returns (status, body).
async fn bridge_query(
    inst: &Instance,
    keys: &nostr::Keys,
    device_key: Option<&p256::ecdsa::SigningKey>,
    owner_tag: Option<&str>,
) -> (u16, Value) {
    let body = serde_json::to_vec(&json!([{"kinds": [1], "limit": 1}])).expect("body");
    let payload = hex::encode(Sha256::digest(&body));
    let nonce = uuid::Uuid::new_v4().to_string();
    let event = signed_event(
        keys,
        nostr::Kind::HttpAuth,
        tags(&[
            &["u", &http_url(inst, "/query")],
            &["method", "POST"],
            &["payload", &payload],
            &["nonce", &nonce],
        ]),
        "",
        device_key,
    );
    let mut request = inst
        .post("/query")
        .header("authorization", nostr_authorization(&event))
        .header("content-type", "application/json")
        .body(body);
    if let Some(tag) = owner_tag {
        request = request.header("x-auth-tag", tag);
    }
    let response = request.send().await.expect("query");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// GET /media/<hash> with a kind-24242 read token; returns (status, body).
async fn media_read(
    inst: &Instance,
    keys: &nostr::Keys,
    device_key: Option<&p256::ecdsa::SigningKey>,
) -> (u16, String) {
    let sha = hex::encode(Sha256::digest(uuid::Uuid::new_v4().as_bytes()));
    let expiration = (nostr::Timestamp::now().as_secs() + 50).to_string();
    let event = signed_event(
        keys,
        nostr::Kind::Custom(24242),
        tags(&[
            &["t", "get"],
            &["x", &sha],
            &["server", &inst.host],
            &["expiration", &expiration],
        ]),
        "read",
        device_key,
    );
    let response = inst
        .get(&format!("/media/{sha}"))
        .header("authorization", nostr_authorization(&event))
        .send()
        .await
        .expect("media read");
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

/// GET /git/<owner>/<repo>/info/refs signed by `keys`; returns (status, body).
async fn git_refs(
    inst: &Instance,
    keys: &nostr::Keys,
    device_key: Option<&p256::ecdsa::SigningKey>,
) -> (u16, String) {
    let repo = format!("/git/{}/missing", keys.public_key().to_hex());
    let event = signed_event(
        keys,
        nostr::Kind::HttpAuth,
        tags(&[&["u", &http_url(inst, &repo)], &["method", "GET"]]),
        "",
        device_key,
    );
    let response = inst
        .get(&format!("{repo}/info/refs?service=git-upload-pack"))
        .header("authorization", nostr_authorization(&event))
        .send()
        .await
        .expect("git refs");
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

/// NIP-42 AUTH with extra tags (a NIP-OA `auth` tag); retried like
/// `device_ws::key_auth` because a refusal closes the socket.
async fn ws_auth_with(
    inst: &Instance,
    keys: &nostr::Keys,
    extra: Vec<Vec<String>>,
    device_key: Option<&p256::ecdsa::SigningKey>,
) -> (bool, String) {
    for _ in 0..20 {
        let mut ws = inst.ws_raw().await;
        let challenge = next_json(&mut ws).await;
        let mut event_tags = tags(&[
            &["relay", &format!("ws://{}", inst.host)],
            &["challenge", challenge[1].as_str().expect("challenge")],
        ]);
        event_tags.extend(extra.iter().cloned());
        let event = signed_event(
            keys,
            nostr::Kind::Authentication,
            event_tags,
            "",
            device_key,
        );
        send(&mut ws, json!(["AUTH", event])).await;
        if let Some(ok) = ok_frame(&mut ws).await {
            return ok;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no OK frame in 20 tries");
}

/// Token AUTH on a fresh socket, retried like [`ws_auth_with`].
async fn ws_token_auth(inst: &Instance, token: &str) -> (bool, String) {
    for _ in 0..20 {
        let mut ws = inst.ws_raw().await;
        let _challenge = next_json(&mut ws).await;
        send(&mut ws, json!(["AUTH", {"token": token}])).await;
        if let Some(ok) = ok_frame(&mut ws).await {
            return ok;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no OK frame in 20 tries");
}

async fn ok_frame(ws: &mut Ws) -> Option<(bool, String)> {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("frame within 10s");
        let text = match frame {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
            Some(Ok(_)) => continue,
        };
        let ok: Value = serde_json::from_str(&text).expect("json frame");
        if ok[0] == "OK" {
            return Some((
                ok[2].as_bool().expect("accepted"),
                ok[3].as_str().unwrap_or_default().to_owned(),
            ));
        }
    }
}

impl Instance {
    /// A socket before its AUTH challenge is read.
    async fn ws_raw(&self) -> Ws {
        let mut request = format!("ws://{}/", self.addr)
            .into_client_request()
            .expect("ws request");
        request
            .headers_mut()
            .insert("host", self.host.parse().expect("host header"));
        tokio_tungstenite::connect_async(request)
            .await
            .expect("ws connect")
            .0
    }

    fn override_modes(&self, modes: &[(Surface, DeviceAdmissionMode)]) {
        self.state
            .identity
            .devices
            .set_overrides(modes.iter().copied().collect());
    }
}

/// A person key with a signed-in device; returns (person, own device, a
/// stranger device).
async fn signed_in_person(
    inst: &Instance,
    seed: u8,
) -> (
    nostr::Keys,
    p256::ecdsa::SigningKey,
    p256::ecdsa::SigningKey,
) {
    let person = nostr::Keys::generate();
    let account = seed_account(inst, &person).await;
    let (own, own_key) = device(seed);
    let (stranger, _) = device(seed + 1);
    seed_device(inst, &account, &own_key, true).await;
    (person, own, stranger)
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn bridge_nip98_key_needs_its_signed_in_device() {
    for mode in [
        DeviceAdmissionMode::Off,
        DeviceAdmissionMode::Log,
        DeviceAdmissionMode::Enforce,
    ] {
        let inst = device_instance(mode).await;
        let (person, own, stranger) = signed_in_person(&inst, 71).await;
        let (status, body) = bridge_query(&inst, &person, Some(&own), None).await;
        assert_eq!(status, 200, "{mode:?} own device: {body}");
        for device_key in [None, Some(&stranger)] {
            let (status, body) = bridge_query(&inst, &person, device_key, None).await;
            if mode == DeviceAdmissionMode::Enforce {
                assert_eq!(status, 403, "{body}");
                assert_eq!(body["error"], "restricted: device", "{body}");
            } else {
                assert_eq!(status, 200, "{mode:?}: {body}");
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn media_read_key_needs_its_signed_in_device() {
    for mode in [
        DeviceAdmissionMode::Off,
        DeviceAdmissionMode::Log,
        DeviceAdmissionMode::Enforce,
    ] {
        let inst = device_instance(mode).await;
        let (person, own, stranger) = signed_in_person(&inst, 81).await;
        // Admitted readers get the missing blob's 404.
        let (status, body) = media_read(&inst, &person, Some(&own)).await;
        assert_eq!(status, 404, "{mode:?} own device: {body}");
        for device_key in [None, Some(&stranger)] {
            let (status, body) = media_read(&inst, &person, device_key).await;
            if mode == DeviceAdmissionMode::Enforce {
                assert_eq!(status, 403, "{body}");
                assert!(body.contains("restricted: device"), "{body}");
            } else {
                assert_eq!(status, 404, "{mode:?}: {body}");
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn git_key_needs_its_signed_in_device() {
    for mode in [
        DeviceAdmissionMode::Off,
        DeviceAdmissionMode::Log,
        DeviceAdmissionMode::Enforce,
    ] {
        let inst = device_instance(mode).await;
        let (person, own, stranger) = signed_in_person(&inst, 91).await;
        // Admitted: past the device check (the repo does not exist).
        let (status, body) = git_refs(&inst, &person, Some(&own)).await;
        assert!(
            status != 403 && !body.contains("restricted"),
            "{mode:?} own device: {status} {body}"
        );
        for device_key in [None, Some(&stranger)] {
            let (refused_status, refused_body) = git_refs(&inst, &person, device_key).await;
            if mode == DeviceAdmissionMode::Enforce {
                assert_eq!(refused_status, 403, "{refused_body}");
                assert_eq!(refused_body, "restricted: device");
            } else {
                assert_eq!(
                    (refused_status, refused_body.as_str()),
                    (status, body.as_str()),
                    "{mode:?}"
                );
            }
        }
    }
}

/// A registered agent key's NIP-OA tag must name its registry owner.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn nip_oa_tag_must_name_the_registry_owner() {
    for mode in [DeviceAdmissionMode::Log, DeviceAdmissionMode::Enforce] {
        let inst = device_instance(mode).await;
        let owner = nostr::Keys::generate();
        let account = seed_account(&inst, &owner).await;
        let (host, host_key) = device(101);
        let host_id = seed_device(&inst, &account, &host_key, true).await;
        let agent = nostr::Keys::generate();
        seed_registry(&inst, &agent, &account, "agent", host_id).await;
        let tag_by = |signer: &nostr::Keys| -> String {
            kuzz_sdk::nip_oa::compute_auth_tag(signer, &agent.public_key(), "").expect("auth tag")
        };
        let as_tag = |json: &str| -> Vec<Vec<String>> {
            vec![serde_json::from_str::<Vec<String>>(json).expect("tag json")]
        };
        let owner_tag = tag_by(&owner);
        let stranger_tag = tag_by(&nostr::Keys::generate());

        assert_eq!(
            ws_auth_with(&inst, &agent, as_tag(&owner_tag), Some(&host)).await,
            (true, String::new()),
            "{mode:?}"
        );
        let foreign = ws_auth_with(&inst, &agent, as_tag(&stranger_tag), Some(&host)).await;
        let (status, body) = bridge_query(&inst, &agent, Some(&host), Some(&stranger_tag)).await;
        if mode == DeviceAdmissionMode::Enforce {
            assert_eq!(foreign, (false, "restricted: owner".to_owned()));
            assert_eq!(status, 403, "{body}");
            assert_eq!(body["error"], "restricted: owner", "{body}");
        } else {
            // Device admission lets it through; the existing first-write-wins
            // owner link (already recorded for the real owner) still refuses
            // the WS AUTH, the open-relay bridge does not look at it.
            assert_eq!(
                foreign,
                (false, crate::handlers::auth::OWNER_LINK_ERROR.to_owned())
            );
            assert_eq!(status, 200, "{body}");
        }
    }
}

/// Token message paths close under `enforce` (surface `token`), stay open
/// under `log`, and account calls keep working either way.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn token_message_paths_close_only_under_enforce() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    let user = new_user(&inst).await;
    let filter = json!([{"kinds": [1], "limit": 1}]);

    for mode in [
        DeviceAdmissionMode::Off,
        DeviceAdmissionMode::Log,
        DeviceAdmissionMode::Enforce,
    ] {
        inst.override_modes(&[(Surface::Token, mode)]);
        let closed = mode == DeviceAdmissionMode::Enforce;

        let response = inst
            .post("/query")
            .bearer_auth(&user.access)
            .json(&filter)
            .send()
            .await
            .expect("bearer query");
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if closed {
            assert_eq!(status, 403, "{body}");
            assert_eq!(body["code"], "token_message_path_closed", "{body}");
        } else {
            assert_eq!(status, 200, "{mode:?}: {body}");
        }

        let (accepted, reason) = ws_token_auth(&inst, &user.access).await;
        if closed {
            assert_eq!(
                (accepted, reason.as_str()),
                (false, "auth-required: token_message_path_closed")
            );
        } else {
            assert_eq!((accepted, reason.as_str()), (true, ""), "{mode:?}");
        }

        // The account surface is not a message path.
        let (status, me) = me_status(&inst, &user.access).await;
        assert_eq!(status, 200, "{mode:?}: {me}");
    }
}
