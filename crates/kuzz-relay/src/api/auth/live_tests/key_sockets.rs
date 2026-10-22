//! Open key sockets on sign-out and revoke, and the new sign-in notice
//! (device-bound keys plan §5, PR A4b), over the production router.

use p256::ecdsa::SigningKey;

use super::agent_keys::{agents_instance, call, device_key, other_device, owner, sign_in};
use super::*;
use crate::identity::admission::test_support::auth_event;
use crate::identity::config::DeviceAdmissionMode;
use kuzz_core::kind::KIND_NEW_SIGNIN_NOTICE;

/// A socket NIP-42-authenticated as `keys` from `device`.
async fn key_socket(inst: &Instance, keys: &nostr::Keys, device: &SigningKey) -> Ws {
    let mut request = format!("ws://{}/", inst.addr)
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("host", inst.host.parse().expect("host header"));
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.expect("ws");
    let challenge = next_json(&mut ws).await;
    let event = auth_event(
        keys,
        challenge[1].as_str().expect("challenge"),
        &format!("ws://{}", inst.host),
        Some(device),
    );
    send(&mut ws, json!(["AUTH", event])).await;
    let ok = next_ok(&mut ws).await;
    assert_eq!(ok[2], true, "admitted: {ok}");
    ws
}

/// A fresh AUTH as `keys` from `device`: (accepted, reason). A refusal
/// closes the socket right after its OK, and on Windows the close can reset
/// the connection before the frame is read; the answer is deterministic, so
/// such a try is repeated (as in `device_ws`).
async fn auth_answer(inst: &Instance, keys: &nostr::Keys, device: &SigningKey) -> (bool, String) {
    for _ in 0..20 {
        let mut request = format!("ws://{}/", inst.addr)
            .into_client_request()
            .expect("ws request");
        request
            .headers_mut()
            .insert("host", inst.host.parse().expect("host header"));
        let (mut ws, _) = tokio_tungstenite::connect_async(request).await.expect("ws");
        let challenge = next_json(&mut ws).await;
        let event = auth_event(
            keys,
            challenge[1].as_str().expect("challenge"),
            &format!("ws://{}", inst.host),
            Some(device),
        );
        send(&mut ws, json!(["AUTH", event])).await;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), ws.next())
                .await
                .expect("frame within 10s")
            {
                Some(Ok(Message::Text(text))) => {
                    let ok: Value = serde_json::from_str(&text).expect("json frame");
                    if ok[0] == "OK" {
                        return (
                            ok[2].as_bool().expect("accepted"),
                            ok[3].as_str().unwrap_or_default().to_owned(),
                        );
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no OK frame in 20 tries");
}

/// The socket still answers a REQ (EOSE or CLOSED): it is open.
async fn assert_open(ws: &mut Ws, sub: &str) {
    send(ws, json!(["REQ", sub, {"kinds": [1], "limit": 1}])).await;
    loop {
        let frame = next_json(ws).await;
        if (frame[0] == "EOSE" || frame[0] == "CLOSED") && frame[1] == sub {
            return;
        }
    }
}

/// Logout and remote device revoke close the key sockets admitted on that
/// device under `enforce` (and a new AUTH there is refused as signed out);
/// sockets on other devices stay. Under `log` every socket stays open.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn key_sockets_close_on_logout_and_device_revoke_under_enforce() {
    for mode in [DeviceAdmissionMode::Enforce, DeviceAdmissionMode::Log] {
        let inst = agents_instance(mode).await;
        let (pc, subject) = owner(&inst).await;
        let laptop = other_device(&inst, &pc, &subject).await;
        let phone = other_device(&inst, &pc, &subject).await;
        let mut on_pc = key_socket(&inst, &pc.keys, &pc.device).await;
        let mut on_laptop = key_socket(&inst, &pc.keys, &laptop.device).await;
        let mut on_phone = key_socket(&inst, &pc.keys, &phone.device).await;

        // The phone signs the laptop out remotely.
        let path = format!("/auth/devices/{}", laptop.device_id);
        let (status, body) = call(&inst, &phone, "DELETE", &path, None).await;
        assert_eq!(status, 204, "{body}");
        // The pc signs itself out.
        let (status, body) = call(&inst, &pc, "POST", "/auth/logout", None).await;
        assert_eq!(status, 204, "{body}");

        if mode == DeviceAdmissionMode::Enforce {
            for ws in [&mut on_laptop, &mut on_pc] {
                let notices = wait_closed(ws, Duration::from_secs(5)).await;
                // On Windows a reset can swallow the NOTICE; when it arrives
                // it carries the refusal.
                assert!(
                    notices
                        .iter()
                        .all(|notice| notice == "restricted: signed_out"),
                    "{notices:?}"
                );
            }
            assert_eq!(
                auth_answer(&inst, &pc.keys, &pc.device).await,
                (false, "restricted: signed_out".to_owned())
            );
            assert_eq!(
                auth_answer(&inst, &pc.keys, &laptop.device).await,
                (false, "restricted: signed_out".to_owned())
            );
        } else {
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_open(&mut on_pc, "pc").await;
            assert_open(&mut on_laptop, "laptop").await;
        }
        assert_open(&mut on_phone, "phone").await;
        assert_eq!(
            auth_answer(&inst, &pc.keys, &phone.device).await,
            (true, String::new()),
            "{mode:?}"
        );
    }
}

/// A login that creates a device row sends the account one relay-signed
/// "New sign-in: <device name>" notice; signing in again on that device
/// sends none.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_new_device_login_sends_one_sign_in_notice() {
    let inst = agents_instance(DeviceAdmissionMode::Enforce).await;
    let (pc, subject) = owner(&inst).await;
    let me = pc.keys.public_key().to_hex();
    let mut ws = key_socket(&inst, &pc.keys, &pc.device).await;
    send(
        &mut ws,
        json!(["REQ", "signin", {"kinds": [KIND_NEW_SIGNIN_NOTICE], "#p": [me]}]),
    )
    .await;
    loop {
        let frame = next_json(&mut ws).await;
        assert_ne!(frame[0], "CLOSED", "{frame}");
        if frame[0] == "EOSE" {
            break;
        }
    }

    let laptop = device_key();
    let (_, _, laptop_id, laptop) = sign_in(&inst, &subject, laptop).await;
    let frame = next_json(&mut ws).await;
    assert_eq!(frame[0], "EVENT", "{frame}");
    assert_eq!(frame[1], "signin");
    let event = &frame[2];
    assert_eq!(event["kind"], KIND_NEW_SIGNIN_NOTICE);
    assert_eq!(
        event["pubkey"],
        inst.state.relay_keypair.public_key().to_hex()
    );
    let content = event["content"].as_str().expect("content");
    let name: String = sqlx::query_scalar("SELECT name FROM devices WHERE id = $1")
        .bind(laptop_id)
        .fetch_one(inst.state.db.pool())
        .await
        .expect("device name");
    assert_eq!(content, format!("New sign-in: {name}"));
    let tags = event["tags"].as_array().expect("tags");
    assert!(tags.contains(&json!(["p", me])), "{tags:?}");
    assert!(
        tags.contains(&json!(["device", laptop_id.to_string()])),
        "{tags:?}"
    );

    // The same device again: a known device, no notice.
    let (_, _, again, _) = sign_in(&inst, &subject, laptop).await;
    assert_eq!(again, laptop_id);
    match tokio::time::timeout(Duration::from_millis(1500), next_json(&mut ws)).await {
        Err(_) => {}
        Ok(frame) => panic!("unexpected frame after a re-login: {frame}"),
    }
}
