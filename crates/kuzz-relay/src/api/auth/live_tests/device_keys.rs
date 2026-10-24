//! Device keys (device-bound keys plan §2.1, PR A1b): a login presents the
//! install's P-256 key with a proof over its login code, the device row
//! records it, and under `KUZZ_DEVICE_ADMISSION` every `/auth/*` use of a
//! user token (and every refresh) carries `X-Kuzz-Device-Proof`.

use p256::ecdsa::signature::Signer as _;
use p256::ecdsa::{Signature, SigningKey};
use sha2::{Digest as _, Sha256};

use super::*;
use crate::identity::config::DeviceAdmissionMode;
use crate::identity::device::{Surface, PROOF_HEADER, SERVER_TIME_HEADER};

/// Redis hash of per-surface overrides (`identity::device::MODES_KEY`).
const MODES_KEY: &str = "kuzz:device_admission";

fn device_key() -> SigningKey {
    loop {
        if let Ok(key) = SigningKey::from_slice(&rand::random::<[u8; 32]>()) {
            return key;
        }
    }
}

fn pubkey_hex(key: &SigningKey) -> String {
    hex::encode(key.verifying_key().to_encoded_point(true).as_bytes())
}

fn pubkey_bytes(key: &SigningKey) -> Vec<u8> {
    key.verifying_key()
        .to_encoded_point(true)
        .as_bytes()
        .to_vec()
}

/// `<ts>.<nonce>.<sig>` over the proof message, with a fresh nonce.
fn proof(
    key: &SigningKey,
    purpose: &str,
    binding: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
) -> String {
    proof_at(
        chrono::Utc::now().timestamp(),
        key,
        purpose,
        binding,
        token,
        body,
    )
}

/// [`proof`] made at `ts`.
fn proof_at(
    ts: i64,
    key: &SigningKey,
    purpose: &str,
    binding: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
) -> String {
    let nonce = hex::encode(rand::random::<[u8; 12]>());
    let token_hash = token.map(kuzz_auth::hash_token);
    let body_hash: Option<[u8; 32]> = body.map(|body| Sha256::digest(body).into());
    let scope = kuzz_auth::device_proof::ProofScope {
        purpose,
        binding,
        token_hash: token_hash.as_ref(),
        body_hash: body_hash.as_ref(),
    };
    let message = kuzz_auth::device_proof::proof_message(&scope, ts, &nonce);
    let signature: Signature = key.sign(message.as_bytes());
    format!("{ts}.{nonce}.{}", hex::encode(signature.to_bytes()))
}

async fn mode_instance(mode: DeviceAdmissionMode, origins: &[&str]) -> (Instance, sqlx::PgPool) {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let origins: Vec<String> = origins.iter().map(|o| (*o).to_owned()).collect();
    let inst = instance_configured(&host, true, None, |config| {
        config.auth_token.device_admission = mode;
        config.auth_token.device_proof_origins = origins;
    })
    .await;
    (inst, pool)
}

struct KeyLogin {
    login: Login,
    device_id: uuid::Uuid,
}

/// How a login presents its device key.
#[derive(Clone, Copy)]
struct Presented<'a> {
    /// Key sent at `start` (`device_pubkey` query value).
    start: Option<&'a SigningKey>,
    /// Key sent and proven at `complete`.
    complete: Option<&'a SigningKey>,
    /// Sign the bind proof over another login code.
    wrong_binding: bool,
}

const KEYLESS: Presented<'static> = Presented {
    start: None,
    complete: None,
    wrong_binding: false,
};

fn keyed(key: &SigningKey) -> Presented<'_> {
    Presented {
        start: Some(key),
        complete: Some(key),
        wrong_binding: false,
    }
}

/// start → callback → complete. `Err((status, code))` when complete refuses.
async fn login_with(
    inst: &Instance,
    subject: &str,
    install_id: Option<&str>,
    presented: Presented<'_>,
) -> Result<KeyLogin, (u16, String)> {
    reset_login_limits(inst).await;
    let verifier = verifier();
    let state = verifier[..24].to_owned();
    let challenge = kuzz_auth::token::pkce_s256_challenge(&verifier);
    let start_hex = presented.start.map(pubkey_hex);
    let mut params = vec![
        ("state", state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("client", "cli"),
        ("identity_mode", "token"),
        ("redirect_uri", REDIRECT_URI),
        ("device_name", "test laptop"),
    ];
    if let Some(install_id) = install_id {
        params.push(("install_id", install_id));
    }
    if let Some(start_hex) = &start_hex {
        params.push(("device_pubkey", start_hex.as_str()));
    }
    let start = inst
        .get_with("/auth/oidc/google/start", &params)
        .send()
        .await
        .expect("start");
    assert_eq!(start.status(), 302, "start must redirect to the IdP");
    let code = format!("{subject}||false");
    let callback = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("code", code.as_str()), ("state", state.as_str())],
        )
        .send()
        .await
        .expect("callback");
    assert_eq!(callback.status(), 302);
    let location = callback.headers()["location"].to_str().unwrap().to_owned();
    let login_code = query_param(&location, "code").expect("login code");
    let mut body = json!({"login_code": login_code, "code_verifier": verifier});
    if let Some(key) = presented.complete {
        let signed_code = if presented.wrong_binding {
            "some-other-login-code"
        } else {
            login_code.as_str()
        };
        let binding = format!("login:{}", hex::encode(kuzz_auth::hash_token(signed_code)));
        body["device_pubkey"] = json!(pubkey_hex(key));
        body["device_key_kind"] = json!("hardware");
        body["device_proof"] = json!(proof(key, "bind", &binding, None, None));
    }
    let response = inst
        .post("/auth/oidc/complete")
        .json(&body)
        .send()
        .await
        .expect("complete");
    let status = response.status().as_u16();
    let body: Value = response.json().await.expect("json");
    if status != 200 {
        return Err((status, body["code"].as_str().unwrap_or_default().to_owned()));
    }
    Ok(KeyLogin {
        login: Login {
            principal: body["principal_id"].as_str().unwrap().to_owned(),
            access: body["access"].as_str().unwrap().to_owned(),
            refresh: body["refresh"].as_str().unwrap().to_owned(),
        },
        device_id: body["device_id"].as_str().unwrap().parse().expect("uuid"),
    })
}

/// A device row: (key, key kind, install id, revoked).
async fn device_row(
    pool: &sqlx::PgPool,
    device_id: uuid::Uuid,
) -> (Option<Vec<u8>>, Option<String>, Option<String>, bool) {
    sqlx::query_as(
        "SELECT device_pubkey, device_key_kind, install_id, revoked_at IS NOT NULL \
         FROM devices WHERE id = $1",
    )
    .bind(device_id)
    .fetch_one(pool)
    .await
    .expect("device row")
}

/// Live sessions of a device.
async fn live_sessions(pool: &sqlx::PgPool, device_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM sessions WHERE device_id = $1 AND revoked_at IS NULL")
        .bind(device_id)
        .fetch_one(pool)
        .await
        .expect("sessions")
}

fn public_url(inst: &Instance) -> String {
    inst.state.identity.config().public_url.clone()
}

/// `GET /auth/me` with an optional proof header; (status, error code).
async fn me_with(inst: &Instance, token: &str, proof: Option<&str>) -> (u16, String) {
    let mut request = inst.get("/auth/me").bearer_auth(token);
    if let Some(proof) = proof {
        request = request.header(PROOF_HEADER, proof);
    }
    let response = request.send().await.expect("me");
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    (status, body["code"].as_str().unwrap_or_default().to_owned())
}

/// A proof for `GET /auth/me` bound to `origin`.
fn me_proof(key: &SigningKey, origin: &str, token: &str) -> String {
    proof(
        key,
        "auth",
        &format!("GET {origin}/auth/me"),
        Some(token),
        None,
    )
}

/// The exact body `refresh_with` sends.
fn refresh_body(refresh: &str) -> String {
    json!({ "refresh": refresh }).to_string()
}

/// A proof for `POST /auth/refresh` of `refresh`.
fn refresh_proof(inst: &Instance, key: &SigningKey, refresh: &str) -> String {
    proof(
        key,
        "refresh",
        &format!("POST {}/auth/refresh", public_url(inst)),
        Some(refresh),
        Some(refresh_body(refresh).as_bytes()),
    )
}

/// `POST /auth/refresh`; (status, body).
async fn refresh_with(inst: &Instance, refresh: &str, proof: Option<&str>) -> (u16, Value) {
    let mut request = inst
        .post("/auth/refresh")
        .header("content-type", "application/json")
        .body(refresh_body(refresh));
    if let Some(proof) = proof {
        request = request.header(PROOF_HEADER, proof);
    }
    let response = request.send().await.expect("refresh");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// Ending a replaced sign-in ends only that session: the device, its key and
/// the session that replaced it stay (a second sign-in on a Mac used
/// `/auth/logout` for this and signed the whole device out).
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn ending_a_session_keeps_the_device() {
    let (inst, pool) = mode_instance(DeviceAdmissionMode::Log, &[]).await;
    let subject = uuid::Uuid::new_v4().to_string();
    let install = uuid::Uuid::new_v4().to_string();
    let pc = device_key();
    let old = login_with(&inst, &subject, Some(&install), keyed(&pc))
        .await
        .expect("first sign-in");
    let new = login_with(&inst, &subject, Some(&install), keyed(&pc))
        .await
        .expect("second sign-in");
    assert_eq!(new.device_id, old.device_id);
    assert_eq!(live_sessions(&pool, old.device_id).await, 2);

    let ended = inst
        .post("/auth/sessions/end")
        .bearer_auth(&old.login.access)
        .send()
        .await
        .unwrap();
    assert_eq!(ended.status(), 204);
    assert_eq!(
        me_with(&inst, &old.login.access, None).await.1,
        "token_revoked"
    );
    assert_eq!(me_with(&inst, &new.login.access, None).await.0, 200);
    assert_eq!(live_sessions(&pool, old.device_id).await, 1);
    let (key, _, _, revoked) = device_row(&pool, old.device_id).await;
    assert_eq!(key, Some(pubkey_bytes(&pc)), "the device keeps its key");
    assert!(!revoked, "the device stays signed in");

    let again = inst
        .post("/auth/sessions/end")
        .bearer_auth(&old.login.access)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 401, "an ended session cannot end anything");
}

/// First login stores the key, the same key is the same device, a keyless
/// install row takes the key, and another key on that install is a new row
/// without the install id. Nothing is revoked on the way.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_device_is_found_by_its_key() {
    let (inst, pool) = mode_instance(DeviceAdmissionMode::Log, &[]).await;
    let subject = uuid::Uuid::new_v4().to_string();
    let install = uuid::Uuid::new_v4().to_string();
    let pc = device_key();

    let first = login_with(&inst, &subject, Some(&install), keyed(&pc))
        .await
        .expect("first login");
    let (key, kind, install_id, revoked) = device_row(&pool, first.device_id).await;
    assert_eq!(key, Some(pubkey_bytes(&pc)), "the key is stored compressed");
    assert_eq!(kind.as_deref(), Some("hardware"));
    assert_eq!(install_id.as_deref(), Some(install.as_str()));
    assert!(!revoked);

    let again = login_with(&inst, &subject, Some(&install), keyed(&pc))
        .await
        .expect("same key again");
    assert_eq!(again.device_id, first.device_id, "same key = same device");
    let no_install = login_with(&inst, &subject, None, keyed(&pc))
        .await
        .expect("same key without an install id");
    assert_eq!(
        no_install.device_id, first.device_id,
        "the key alone finds it"
    );
    assert_eq!(live_sessions(&pool, first.device_id).await, 3);

    // Upgrade: an older app's keyless row of this install takes the key.
    let subject = uuid::Uuid::new_v4().to_string();
    let install = uuid::Uuid::new_v4().to_string();
    let old = login_with(&inst, &subject, Some(&install), KEYLESS)
        .await
        .expect("keyless login (old app)");
    assert_eq!(device_row(&pool, old.device_id).await.0, None);
    let phone = device_key();
    let upgraded = login_with(&inst, &subject, Some(&install), keyed(&phone))
        .await
        .expect("keyed login (new app)");
    assert_eq!(
        upgraded.device_id, old.device_id,
        "the install row is reused"
    );
    let (key, kind, install_id, revoked) = device_row(&pool, old.device_id).await;
    assert_eq!(key, Some(pubkey_bytes(&phone)));
    assert_eq!(kind.as_deref(), Some("hardware"));
    assert_eq!(install_id.as_deref(), Some(install.as_str()));
    assert!(!revoked);
    assert_eq!(
        live_sessions(&pool, old.device_id).await,
        2,
        "the keyless session survives the upgrade"
    );

    // The install's key was lost: another key on it is a new row that takes
    // the install id over; the old device stays signed in.
    let replacement = device_key();
    let other = login_with(&inst, &subject, Some(&install), keyed(&replacement))
        .await
        .expect("another key on the same install");
    assert_ne!(other.device_id, old.device_id);
    let (key, _, install_id, revoked) = device_row(&pool, other.device_id).await;
    assert_eq!(key, Some(pubkey_bytes(&replacement)));
    assert_eq!(install_id.as_deref(), Some(install.as_str()));
    assert!(!revoked);
    let (key, _, install_id, revoked) = device_row(&pool, old.device_id).await;
    assert_eq!(key, Some(pubkey_bytes(&phone)), "the old row keeps its key");
    assert_eq!(install_id, None, "the install id moved");
    assert!(!revoked, "nothing is revoked");
    assert_eq!(live_sessions(&pool, old.device_id).await, 2);
    let old_access = me_proof(&phone, &public_url(&inst), &upgraded.login.access);
    assert_eq!(
        me_with(&inst, &upgraded.login.access, Some(&old_access))
            .await
            .0,
        200
    );

    // And a keyless login of that install lands on the install's current row.
    let keyless_again = login_with(&inst, &subject, Some(&install), KEYLESS)
        .await
        .expect("keyless login again");
    assert_eq!(keyless_again.device_id, other.device_id);

    // The old key logging in again takes the install id back.
    let back = login_with(&inst, &subject, Some(&install), keyed(&phone))
        .await
        .expect("old key again");
    assert_eq!(back.device_id, old.device_id);
    assert_eq!(
        device_row(&pool, old.device_id).await.2.as_deref(),
        Some(install.as_str())
    );
    assert_eq!(device_row(&pool, other.device_id).await.2, None);
}

/// The key a login proves must be the one its start bound: `log` signs in
/// keyless, `enforce` refuses with `device_proof_invalid`.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_login_key_must_match_its_start() {
    let (log, pool) = mode_instance(DeviceAdmissionMode::Log, &[]).await;
    let (a, b) = (device_key(), device_key());
    let mismatches = [
        Presented {
            start: Some(&a),
            complete: Some(&b),
            wrong_binding: false,
        },
        Presented {
            start: Some(&a),
            complete: None,
            wrong_binding: false,
        },
        Presented {
            start: None,
            complete: Some(&a),
            wrong_binding: false,
        },
        Presented {
            start: Some(&a),
            complete: Some(&a),
            wrong_binding: true,
        },
    ];
    for (i, presented) in mismatches.iter().enumerate() {
        let subject = uuid::Uuid::new_v4().to_string();
        let login = login_with(&log, &subject, None, *presented)
            .await
            .unwrap_or_else(|error| panic!("log mode case {i} signs in: {error:?}"));
        assert_eq!(
            device_row(&pool, login.device_id).await.0,
            None,
            "case {i}: an unproven key is never recorded"
        );
    }
    // A malformed start key is ignored under log.
    reset_login_limits(&log).await;
    let challenge = kuzz_auth::token::pkce_s256_challenge(&verifier());
    let start = log
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", &verifier()[..24]),
                ("code_challenge", challenge.as_str()),
                ("client", "cli"),
                ("redirect_uri", REDIRECT_URI),
                ("device_pubkey", "02ff"),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), 302);

    let (enforce, pool) = mode_instance(DeviceAdmissionMode::Enforce, &[]).await;
    for (i, presented) in mismatches.iter().enumerate() {
        let subject = uuid::Uuid::new_v4().to_string();
        let refused = login_with(&enforce, &subject, None, *presented).await;
        assert_eq!(
            refused.err(),
            Some((401, "device_proof_invalid".to_owned())),
            "enforce case {i}"
        );
    }
    let start = enforce
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", &verifier()[..24]),
                ("code_challenge", challenge.as_str()),
                ("client", "cli"),
                ("redirect_uri", REDIRECT_URI),
                ("device_pubkey", "02ff"),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), 401, "a malformed start key is refused");
    // The matching key signs in with its key recorded; an older app without
    // any key is refused.
    let ok = login_with(&enforce, &uuid::Uuid::new_v4().to_string(), None, keyed(&a))
        .await
        .expect("matching key");
    assert_eq!(
        device_row(&pool, ok.device_id).await.0,
        Some(pubkey_bytes(&a))
    );
    assert_eq!(
        login_with(&enforce, &uuid::Uuid::new_v4().to_string(), None, KEYLESS)
            .await
            .err(),
        Some((401, "device_proof_required".to_owned())),
        "keyless login under enforce"
    );
}

/// Under `log` every use goes through, proven or not.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn log_mode_lets_unproven_uses_through() {
    let (inst, _pool) = mode_instance(DeviceAdmissionMode::Log, &[]).await;
    let key = device_key();
    let pc = login_with(&inst, &uuid::Uuid::new_v4().to_string(), None, keyed(&key))
        .await
        .expect("login");
    let origin = public_url(&inst);

    let response = inst
        .get("/auth/me")
        .bearer_auth(&pc.login.access)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "no proof");
    assert!(
        response.headers().contains_key(SERVER_TIME_HEADER),
        "the server clock is sent while the surface is on"
    );
    let proven = me_proof(&key, &origin, &pc.login.access);
    assert_eq!(me_with(&inst, &pc.login.access, Some(&proven)).await.0, 200);
    let wrong_key = me_proof(&device_key(), &origin, &pc.login.access);
    assert_eq!(
        me_with(&inst, &pc.login.access, Some(&wrong_key)).await.0,
        200
    );
    assert_eq!(
        me_with(&inst, &pc.login.access, Some(&proven)).await.0,
        200,
        "a replay is only logged"
    );

    let (status, body) = refresh_with(&inst, &pc.login.refresh, None).await;
    assert_eq!(status, 200, "refresh without proof: {body}");
    let refresh = body["refresh"].as_str().unwrap().to_owned();
    let proven = refresh_proof(&inst, &key, &refresh);
    let (status, body) = refresh_with(&inst, &refresh, Some(&proven)).await;
    assert_eq!(status, 200, "refresh with proof: {body}");

    let keyless = login_with(&inst, &uuid::Uuid::new_v4().to_string(), None, KEYLESS)
        .await
        .expect("keyless login");
    assert_eq!(me_with(&inst, &keyless.login.access, None).await.0, 200);
    let (status, _) = refresh_with(&inst, &keyless.login.refresh, None).await;
    assert_eq!(status, 200);
}

/// Under `enforce` a keyed device's token works only with its own fresh
/// proof, on `/auth/me` and on refresh, and a keyless device is refused.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn enforce_mode_requires_the_device_proof() {
    let (inst, pool) = mode_instance(DeviceAdmissionMode::Enforce, &[]).await;
    let key = device_key();
    let pc = login_with(&inst, &uuid::Uuid::new_v4().to_string(), None, keyed(&key))
        .await
        .expect("login");
    let origin = public_url(&inst);
    let access = pc.login.access.clone();

    assert_eq!(
        me_with(&inst, &access, None).await,
        (401, "device_proof_required".to_owned()),
        "a bare (copied) token is refused"
    );
    let wrong_key = me_proof(&device_key(), &origin, &access);
    assert_eq!(me_with(&inst, &access, Some(&wrong_key)).await.0, 401);
    let other_token = proof(
        &key,
        "auth",
        &format!("GET {origin}/auth/me"),
        Some(&pc.login.refresh),
        None,
    );
    assert_eq!(me_with(&inst, &access, Some(&other_token)).await.0, 401);
    let other_path = proof(
        &key,
        "auth",
        &format!("GET {origin}/auth/devices"),
        Some(&access),
        None,
    );
    assert_eq!(me_with(&inst, &access, Some(&other_path)).await.0, 401);
    assert_eq!(
        me_with(&inst, &access, Some("1.not-a-proof.00")).await,
        (401, "device_proof_required".to_owned())
    );
    let proven = me_proof(&key, &origin, &access);
    assert_eq!(me_with(&inst, &access, Some(&proven)).await.0, 200);

    // Refresh: refused bare, with another key or over another body.
    let refresh = pc.login.refresh.clone();
    let (status, body) = refresh_with(&inst, &refresh, None).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("device_proof_required"))
    );
    let foreign = refresh_proof(&inst, &device_key(), &refresh);
    assert_eq!(refresh_with(&inst, &refresh, Some(&foreign)).await.0, 401);
    let other_body = proof(
        &key,
        "refresh",
        &format!("POST {origin}/auth/refresh"),
        Some(&refresh),
        Some(b"{}"),
    );
    assert_eq!(
        refresh_with(&inst, &refresh, Some(&other_body)).await.0,
        401
    );
    let auth_purpose = proof(
        &key,
        "auth",
        &format!("POST {origin}/auth/refresh"),
        Some(&refresh),
        Some(refresh_body(&refresh).as_bytes()),
    );
    assert_eq!(
        refresh_with(&inst, &refresh, Some(&auth_purpose)).await.0,
        401
    );
    let proven = refresh_proof(&inst, &key, &refresh);
    let (status, body) = refresh_with(&inst, &refresh, Some(&proven)).await;
    assert_eq!(status, 200, "{body}");
    let rotated_access = body["access"].as_str().unwrap().to_owned();
    let rotated_refresh = body["refresh"].as_str().unwrap().to_owned();
    // The rotation replay cache is behind the proof check too.
    assert_eq!(refresh_with(&inst, &refresh, None).await.0, 401);
    let proven = me_proof(&key, &origin, &rotated_access);
    assert_eq!(me_with(&inst, &rotated_access, Some(&proven)).await.0, 200);
    assert_eq!(me_with(&inst, &rotated_access, None).await.0, 401);
    // A clock two minutes off gets its own answer, to resync and retry.
    let stale = proof_at(
        chrono::Utc::now().timestamp() - 120,
        &key,
        "auth",
        &format!("GET {origin}/auth/me"),
        Some(&rotated_access),
        None,
    );
    assert_eq!(
        me_with(&inst, &rotated_access, Some(&stale)).await,
        (401, "device_proof_stale".to_owned())
    );
    let proven = refresh_proof(&inst, &key, &rotated_refresh);
    assert_eq!(
        refresh_with(&inst, &rotated_refresh, Some(&proven)).await.0,
        200
    );

    // A keyless device (an older app signed in before enforce) cannot prove
    // anything: refused.
    let keyless = login_with(
        &inst,
        &uuid::Uuid::new_v4().to_string(),
        None,
        keyed(&device_key()),
    )
    .await
    .expect("login");
    sqlx::query("UPDATE devices SET device_pubkey = NULL, device_key_kind = NULL WHERE id = $1")
        .bind(keyless.device_id)
        .execute(&pool)
        .await
        .expect("make the device keyless");
    assert_eq!(
        me_with(&inst, &keyless.login.access, None).await,
        (401, "device_proof_required".to_owned())
    );
    let any_key = me_proof(&device_key(), &origin, &keyless.login.access);
    assert_eq!(
        me_with(&inst, &keyless.login.access, Some(&any_key))
            .await
            .0,
        401
    );
    let (status, body) = refresh_with(&inst, &keyless.login.refresh, None).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("device_proof_required"))
    );
}

/// A proof's nonce is burned on first use.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_replayed_proof_is_refused() {
    let (inst, _pool) = mode_instance(DeviceAdmissionMode::Enforce, &[]).await;
    let key = device_key();
    let pc = login_with(&inst, &uuid::Uuid::new_v4().to_string(), None, keyed(&key))
        .await
        .expect("login");
    let once = me_proof(&key, &public_url(&inst), &pc.login.access);
    assert_eq!(me_with(&inst, &pc.login.access, Some(&once)).await.0, 200);
    assert_eq!(
        me_with(&inst, &pc.login.access, Some(&once)).await,
        (401, "device_proof_required".to_owned()),
        "same nonce again"
    );
    // A fresh proof still works.
    let fresh = me_proof(&key, &public_url(&inst), &pc.login.access);
    assert_eq!(me_with(&inst, &pc.login.access, Some(&fresh)).await.0, 200);

    // Same for refresh: a replayed refresh proof cannot read the cached
    // rotation.
    let refresh = pc.login.refresh.clone();
    let once = refresh_proof(&inst, &key, &refresh);
    assert_eq!(refresh_with(&inst, &refresh, Some(&once)).await.0, 200);
    assert_eq!(refresh_with(&inst, &refresh, Some(&once)).await.0, 401);
}

/// A proof may be bound to the public URL or an extra accepted origin, and
/// to nothing else.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_proof_binds_to_an_accepted_origin() {
    let (inst, _pool) = mode_instance(DeviceAdmissionMode::Enforce, &["https://old.example"]).await;
    let key = device_key();
    let pc = login_with(&inst, &uuid::Uuid::new_v4().to_string(), None, keyed(&key))
        .await
        .expect("login");
    let access = &pc.login.access;
    let listed = me_proof(&key, "https://old.example", access);
    assert_eq!(me_with(&inst, access, Some(&listed)).await.0, 200);
    let public = me_proof(&key, &public_url(&inst), access);
    assert_eq!(me_with(&inst, access, Some(&public)).await.0, 200);
    let unlisted = me_proof(&key, "https://evil.example", access);
    assert_eq!(me_with(&inst, access, Some(&unlisted)).await.0, 401);
    let real_port = me_proof(&key, &format!("http://{}", inst.addr), access);
    assert_eq!(
        me_with(&inst, access, Some(&real_port)).await.0,
        401,
        "the address the request happened to reach is not an accepted origin"
    );
}

/// `off` changes nothing: keys are ignored, no proof is needed.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn off_mode_changes_nothing() {
    let (inst, pool) = mode_instance(DeviceAdmissionMode::Off, &[]).await;
    let key = device_key();
    let subject = uuid::Uuid::new_v4().to_string();
    let install = uuid::Uuid::new_v4().to_string();
    let pc = login_with(&inst, &subject, Some(&install), keyed(&key))
        .await
        .expect("login");
    assert_eq!(
        device_row(&pool, pc.device_id).await.0,
        None,
        "no key is recorded while off"
    );
    let mismatched = login_with(
        &inst,
        &subject,
        Some(&install),
        Presented {
            start: Some(&key),
            complete: Some(&device_key()),
            wrong_binding: true,
        },
    )
    .await
    .expect("mismatches are not checked");
    assert_eq!(
        mismatched.device_id, pc.device_id,
        "install id still finds it"
    );

    let response = inst
        .get("/auth/me")
        .bearer_auth(&pc.login.access)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key(SERVER_TIME_HEADER));
    let garbage = me_with(&inst, &pc.login.access, Some("garbage")).await;
    assert_eq!(garbage.0, 200, "a proof header is ignored");
    let (status, body) = refresh_with(&inst, &pc.login.refresh, None).await;
    assert_eq!(status, 200, "{body}");
}

async fn set_override(inst: &Instance, value: Option<&str>) {
    let mut redis = inst.state.redis_pool.get().await.expect("redis");
    let mut cmd = match value {
        Some(_) => redis::cmd("HSET"),
        None => redis::cmd("DEL"),
    };
    cmd.arg(MODES_KEY);
    if let Some(value) = value {
        cmd.arg("auth").arg(value);
    }
    let _: i64 = cmd.query_async(&mut redis).await.expect("override");
}

/// Wait until the auth surface runs in `expected`.
async fn wait_mode(inst: &Instance, expected: DeviceAdmissionMode) {
    // Each poller reads the hash once right away, then every 10 s.
    crate::identity::device::spawn_mode_poller(Arc::clone(&inst.state));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while crate::identity::device::mode(&inst.state, Surface::Auth) != expected {
        assert!(
            tokio::time::Instant::now() < deadline,
            "mode never became {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The Redis override switches the auth surface without a restart; a bad
/// value falls back to `KUZZ_DEVICE_ADMISSION`.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_redis_override_switches_the_mode() {
    let (inst, _pool) = mode_instance(DeviceAdmissionMode::Off, &[]).await;
    set_override(&inst, None).await;
    let keyless = login_with(&inst, &uuid::Uuid::new_v4().to_string(), None, KEYLESS)
        .await
        .expect("login");
    assert_eq!(me_with(&inst, &keyless.login.access, None).await.0, 200);

    set_override(&inst, Some("enforce")).await;
    wait_mode(&inst, DeviceAdmissionMode::Enforce).await;
    assert_eq!(
        me_with(&inst, &keyless.login.access, None).await,
        (401, "device_proof_required".to_owned())
    );

    set_override(&inst, Some("on")).await;
    wait_mode(&inst, DeviceAdmissionMode::Off).await;
    assert_eq!(me_with(&inst, &keyless.login.access, None).await.0, 200);

    set_override(&inst, Some("LOG")).await;
    wait_mode(&inst, DeviceAdmissionMode::Log).await;
    assert_eq!(me_with(&inst, &keyless.login.access, None).await.0, 200);

    set_override(&inst, None).await;
    wait_mode(&inst, DeviceAdmissionMode::Off).await;
}

/// `POST /auth/device/rotate` with `body`, proven by `old` (when given);
/// (status, error code).
async fn rotate_with(
    inst: &Instance,
    token: &str,
    old: Option<&SigningKey>,
    body: &Value,
) -> (u16, String) {
    let body = body.to_string();
    let mut request = inst
        .post("/auth/device/rotate")
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(body.clone());
    if let Some(old) = old {
        let binding = format!("POST {}/auth/device/rotate", public_url(inst));
        request = request.header(
            PROOF_HEADER,
            proof(old, "auth", &binding, Some(token), Some(body.as_bytes())),
        );
    }
    let response = request.send().await.expect("rotate");
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    (status, body["code"].as_str().unwrap_or_default().to_owned())
}

/// The rotation body moving `device` from `old` to `new`, proven by `signer`.
fn rotate_body(
    device: uuid::Uuid,
    old: &SigningKey,
    new: &SigningKey,
    signer: &SigningKey,
    token: &str,
) -> Value {
    let key = |k: &SigningKey| {
        kuzz_auth::device_proof::DevicePubkey::from_sec1(&pubkey_bytes(k)).expect("key")
    };
    let binding =
        kuzz_auth::device_proof::rotate_binding(&device.to_string(), &key(old), &key(new));
    json!({
        "device_pubkey": pubkey_hex(new),
        "device_key_kind": "hardware",
        "proof": proof(signer, "rotate", &binding, Some(token), None),
    })
}

/// A device moves from its key to a new one proven by both: the session and
/// device row stay, the old key stops working and the new one works.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn a_device_key_rotates_when_both_keys_prove_it() {
    let (inst, pool) = mode_instance(DeviceAdmissionMode::Enforce, &[]).await;
    let old = device_key();
    let new = device_key();
    let pc = login_with(&inst, &uuid::Uuid::new_v4().to_string(), None, keyed(&old))
        .await
        .expect("login");
    let access = pc.login.access.clone();
    let device = pc.device_id;
    let good = rotate_body(device, &old, &new, &new, &access);

    assert_eq!(
        rotate_with(&inst, &access, None, &good).await.0,
        401,
        "a bare (copied) token cannot rotate"
    );
    let stranger = device_key();
    let unproven = rotate_body(device, &old, &new, &stranger, &access);
    assert_eq!(
        rotate_with(&inst, &access, Some(&old), &unproven).await,
        (403, "device_proof_invalid".to_owned()),
        "the new key must prove it"
    );
    let other_device = rotate_body(uuid::Uuid::new_v4(), &old, &new, &new, &access);
    assert_eq!(
        rotate_with(&inst, &access, Some(&old), &other_device).await.0,
        403
    );
    let mut software = good.clone();
    software["device_key_kind"] = json!("software");
    assert_eq!(
        rotate_with(&inst, &access, Some(&old), &software).await.0,
        400
    );
    assert_eq!(device_row(&pool, device).await.0, Some(pubkey_bytes(&old)));

    assert_eq!(rotate_with(&inst, &access, Some(&old), &good).await.0, 204);
    let (key, kind, _, revoked) = device_row(&pool, device).await;
    assert_eq!(key, Some(pubkey_bytes(&new)));
    assert_eq!(kind.as_deref(), Some("hardware"));
    assert!(!revoked);
    assert_eq!(live_sessions(&pool, device).await, 1, "the session stays");

    let origin = public_url(&inst);
    assert_eq!(
        me_with(&inst, &access, Some(&me_proof(&old, &origin, &access)))
            .await
            .0,
        401,
        "the old key no longer proves"
    );
    assert_eq!(
        me_with(&inst, &access, Some(&me_proof(&new, &origin, &access)))
            .await
            .0,
        200
    );
    // The old key cannot rotate the device any more.
    let back = rotate_body(device, &new, &old, &old, &access);
    assert_eq!(rotate_with(&inst, &access, Some(&old), &back).await.0, 401);

    let refresh = pc.login.refresh.clone();
    let proven = refresh_proof(&inst, &new, &refresh);
    assert_eq!(refresh_with(&inst, &refresh, Some(&proven)).await.0, 200);
}
