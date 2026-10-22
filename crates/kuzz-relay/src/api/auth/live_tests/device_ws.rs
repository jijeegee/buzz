//! Live WebSocket key admission (device-bound keys plan §2.2, PR A1c): a
//! NIP-42 AUTH over the production router, with accounts, devices, sessions
//! and registry rows seeded in Postgres.

use super::*;
use crate::identity::admission::test_support::{auth_event, device};
use crate::identity::config::DeviceAdmissionMode;

async fn device_instance(mode: DeviceAdmissionMode) -> Instance {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    instance_configured(&community(&pool).await, false, None, |config| {
        config.auth_token.device_admission = mode;
    })
    .await
}

/// A key-backup account whose own key is `person`.
async fn seed_account(inst: &Instance, person: &nostr::Keys) -> Vec<u8> {
    let principal = nostr::Keys::generate().public_key().to_bytes().to_vec();
    let pool = inst.state.db.pool();
    sqlx::query(
        "INSERT INTO principals (id, kind, identity_mode, display_name) \
         VALUES ($1, 'user', 'key_backup', 'device test')",
    )
    .bind(&principal)
    .execute(pool)
    .await
    .expect("principal");
    sqlx::query(
        "INSERT INTO account_key_backups (account_id, pubkey, version, key_id, nonce, ciphertext) \
         VALUES ($1, $2, 2, 'k1', $3, $4)",
    )
    .bind(&principal)
    .bind(person.public_key().to_bytes().as_slice())
    .bind([0u8; 12].as_slice())
    .bind([0u8; 48].as_slice())
    .execute(pool)
    .await
    .expect("key backup");
    principal
}

/// A device row of `principal` holding `device_key`, signed in (a live
/// session with an unused refresh token) when `signed_in`.
async fn seed_device(
    inst: &Instance,
    principal: &[u8],
    device_key: &kuzz_auth::device_proof::DevicePubkey,
    signed_in: bool,
) -> uuid::Uuid {
    let pool = inst.state.db.pool();
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO devices (principal_id, name, platform, device_pubkey, device_key_kind) \
         VALUES ($1, 'pc', 'desktop', $2, 'software') RETURNING id",
    )
    .bind(principal)
    .bind(device_key.as_bytes().as_slice())
    .fetch_one(pool)
    .await
    .expect("device");
    if signed_in {
        let session: uuid::Uuid =
            sqlx::query_scalar("INSERT INTO sessions (device_id) VALUES ($1) RETURNING id")
                .bind(id)
                .fetch_one(pool)
                .await
                .expect("session");
        sqlx::query(
            "INSERT INTO refresh_tokens (token_hash, session_id, generation, expires_at) \
             VALUES ($1, $2, 1, now() + interval '1 day')",
        )
        .bind(rand::random::<[u8; 32]>().as_slice())
        .bind(session)
        .execute(pool)
        .await
        .expect("refresh token");
    }
    id
}

/// Register `key` as an agent or external key of `principal` on `host`.
async fn seed_registry(
    inst: &Instance,
    key: &nostr::Keys,
    principal: &[u8],
    role: &str,
    host: uuid::Uuid,
) {
    sqlx::query(
        "INSERT INTO key_registry (pubkey, principal_id, role, host_device_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(key.public_key().to_bytes().as_slice())
    .bind(principal)
    .bind(role)
    .bind(host)
    .execute(inst.state.db.pool())
    .await
    .expect("registry row");
}

/// Sign out every session of `device` (logout revokes the session).
async fn sign_out(inst: &Instance, device: uuid::Uuid) {
    sqlx::query(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = 'logout' WHERE device_id = $1",
    )
    .bind(device)
    .execute(inst.state.db.pool())
    .await
    .expect("sign out");
}

/// NIP-42 AUTH as `keys` from `device_key` (`None`: no device tag) on a
/// fresh socket; returns the OK frame's (accepted, reason). A refusal closes
/// the socket right after its OK, and on Windows the close can reset the
/// connection before the client reads the frame; the answer is
/// deterministic (only admissions are cached), so such a try is repeated.
async fn key_auth(
    inst: &Instance,
    keys: &nostr::Keys,
    device_key: Option<&p256::ecdsa::SigningKey>,
) -> (bool, String) {
    for _ in 0..20 {
        if let Some(ok) = try_key_auth(inst, keys, device_key).await {
            return ok;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no OK frame in 20 tries");
}

async fn try_key_auth(
    inst: &Instance,
    keys: &nostr::Keys,
    device_key: Option<&p256::ecdsa::SigningKey>,
) -> Option<(bool, String)> {
    let mut request = format!("ws://{}/", inst.addr)
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("host", inst.host.parse().expect("host header"));
    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("ws connect");
    let challenge = next_json(&mut ws).await;
    assert_eq!(challenge[0], "AUTH");
    let event = auth_event(
        keys,
        challenge[1].as_str().expect("challenge"),
        &format!("ws://{}", inst.host),
        device_key,
    );
    send(&mut ws, json!(["AUTH", event])).await;
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

fn admitted() -> (bool, String) {
    (true, String::new())
}

fn refused(reason: &str) -> (bool, String) {
    (false, reason.to_owned())
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn person_key_needs_its_signed_in_device() {
    let (pc, pc_key) = device(21);
    let (stranger, _) = device(22);
    for mode in [DeviceAdmissionMode::Log, DeviceAdmissionMode::Enforce] {
        let inst = device_instance(mode).await;
        let person = nostr::Keys::generate();
        let account = seed_account(&inst, &person).await;
        // A revoked older row with the same key must not hide the live one.
        let old = seed_device(&inst, &account, &pc_key, false).await;
        sqlx::query("UPDATE devices SET revoked_at = now() - interval '1 day', last_seen_at = now() + interval '1 hour' WHERE id = $1")
            .bind(old)
            .execute(inst.state.db.pool())
            .await
            .expect("revoke old row");
        seed_device(&inst, &account, &pc_key, true).await;

        assert_eq!(key_auth(&inst, &person, Some(&pc)).await, admitted());
        let enforced = mode == DeviceAdmissionMode::Enforce;
        let expect = |reason: &str| {
            if enforced {
                refused(reason)
            } else {
                admitted()
            }
        };
        // No device tag: would_refuse(no_device) under log, refused under enforce.
        assert_eq!(
            key_auth(&inst, &person, None).await,
            expect("restricted: device"),
            "{mode:?}"
        );
        // Another machine's device key.
        assert_eq!(
            key_auth(&inst, &person, Some(&stranger)).await,
            expect("restricted: device"),
            "{mode:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn agent_key_needs_its_host_and_its_owner_signed_in() {
    let inst = device_instance(DeviceAdmissionMode::Enforce).await;
    let owner = nostr::Keys::generate();
    let account = seed_account(&inst, &owner).await;
    let (host, host_key) = device(31);
    let (laptop, laptop_key) = device(32);
    let host_id = seed_device(&inst, &account, &host_key, true).await;
    // The owner is signed in on the laptop too; the agent still may not move.
    seed_device(&inst, &account, &laptop_key, true).await;
    let agent = nostr::Keys::generate();
    seed_registry(&inst, &agent, &account, "agent", host_id).await;

    assert_eq!(key_auth(&inst, &agent, Some(&host)).await, admitted());
    assert_eq!(
        key_auth(&inst, &agent, Some(&laptop)).await,
        refused("restricted: device")
    );

    // After the owner signs out on the host, a new socket is refused with
    // the signed-out reason (a fresh relay: the first answer is cached 30 s).
    let inst = device_instance(DeviceAdmissionMode::Enforce).await;
    let account = seed_account(&inst, &nostr::Keys::generate()).await;
    let host_id = seed_device(&inst, &account, &host_key, true).await;
    let agent = nostr::Keys::generate();
    seed_registry(&inst, &agent, &account, "agent", host_id).await;
    sign_out(&inst, host_id).await;
    assert_eq!(
        key_auth(&inst, &agent, Some(&host)).await,
        refused("restricted: signed_out")
    );
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn signed_out_person_is_refused_until_signed_in() {
    let inst = device_instance(DeviceAdmissionMode::Enforce).await;
    let person = nostr::Keys::generate();
    let account = seed_account(&inst, &person).await;
    let (pc, pc_key) = device(41);
    let pc_id = seed_device(&inst, &account, &pc_key, true).await;
    sign_out(&inst, pc_id).await;
    assert_eq!(
        key_auth(&inst, &person, Some(&pc)).await,
        refused("restricted: signed_out")
    );
    // A remote revoke of the device reads the same to the client.
    sqlx::query("UPDATE devices SET revoked_at = now() WHERE id = $1")
        .bind(pc_id)
        .execute(inst.state.db.pool())
        .await
        .expect("revoke device");
    assert_eq!(
        key_auth(&inst, &person, Some(&pc)).await,
        refused("restricted: signed_out")
    );
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn external_key_needs_only_its_paired_device() {
    let inst = device_instance(DeviceAdmissionMode::Enforce).await;
    let owner = nostr::Keys::generate();
    let account = seed_account(&inst, &owner).await;
    let (paired, paired_key) = device(51);
    let (other, _) = device(52);
    // The owner has no login on the external machine.
    let paired_id = seed_device(&inst, &account, &paired_key, false).await;
    let external = nostr::Keys::generate();
    seed_registry(&inst, &external, &account, "external", paired_id).await;

    assert_eq!(key_auth(&inst, &external, Some(&paired)).await, admitted());
    assert_eq!(
        key_auth(&inst, &external, Some(&other)).await,
        refused("restricted: device")
    );
    assert_eq!(
        key_auth(&inst, &external, None).await,
        refused("restricted: device")
    );

    // A revoked key is parked by its supervisor, not restarted.
    let removed = nostr::Keys::generate();
    seed_registry(&inst, &removed, &account, "external", paired_id).await;
    sqlx::query("UPDATE key_registry SET revoked_at = now() WHERE pubkey = $1")
        .bind(removed.public_key().to_bytes().as_slice())
        .execute(inst.state.db.pool())
        .await
        .expect("revoke key");
    assert_eq!(
        key_auth(&inst, &removed, Some(&paired)).await,
        refused("restricted: revoked")
    );
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn unregistered_keys_are_admitted_and_off_changes_nothing() {
    let inst = device_instance(DeviceAdmissionMode::Enforce).await;
    // A key registered nowhere (older key, other client) is admitted for now.
    assert_eq!(
        key_auth(&inst, &nostr::Keys::generate(), None).await,
        admitted()
    );

    let inst = device_instance(DeviceAdmissionMode::Off).await;
    let person = nostr::Keys::generate();
    let account = seed_account(&inst, &person).await;
    let (pc, pc_key) = device(61);
    let pc_id = seed_device(&inst, &account, &pc_key, true).await;
    sign_out(&inst, pc_id).await;
    let (stranger, _) = device(62);
    // Everything enforce refuses goes through.
    assert_eq!(key_auth(&inst, &person, Some(&pc)).await, admitted());
    assert_eq!(key_auth(&inst, &person, None).await, admitted());
    assert_eq!(key_auth(&inst, &person, Some(&stranger)).await, admitted());
}
