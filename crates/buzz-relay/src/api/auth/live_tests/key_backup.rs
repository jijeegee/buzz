//! Real HTTP/Redis/Postgres regression coverage. Only the Google provider is
//! mocked; no production OAuth or native secure-storage claim is made here.

use super::*;
use buzz_core::principal::PrincipalId;

async fn custody_instance() -> Instance {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let host = community(&pool).await;
    instance_configured(&host, true, None, |config| {
        config.auth_token.key_backup_master = Some(
            buzz_auth::key_backup::BackupMasterKey::from_hex("live-test", &"42".repeat(32))
                .unwrap(),
        );
    })
    .await
}

async fn key_login(inst: &Instance, subject: &str) -> Login {
    reset_login_limits(inst).await;
    let verifier = verifier();
    let state = &verifier[..24];
    let challenge = buzz_auth::token::pkce_s256_challenge(&verifier);
    let response = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state),
                ("code_challenge", &challenge),
                ("client", "desktop"),
                ("redirect_uri", REDIRECT_URI),
                ("identity_mode", "key_backup"),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 302);
    let response = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("state", state), ("code", subject)],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 302);
    let location = response.headers()["location"].to_str().unwrap();
    let code =
        query_param(location, "code").unwrap_or_else(|| panic!("callback failed: {location}"));
    let response = inst
        .post("/auth/oidc/complete")
        .json(&json!({
            "login_code": code, "code_verifier": verifier,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["identity_mode"], "key_backup");
    Login {
        principal: body["principal_id"].as_str().unwrap().into(),
        access: body["access"].as_str().unwrap().into(),
        refresh: body["refresh"].as_str().unwrap().into(),
    }
}

async fn proof(inst: &Instance, login: &Login, key: &nostr::Keys) -> nostr::Event {
    let response = inst
        .post("/auth/key-backup/challenge")
        .bearer_auth(&login.access)
        .json(&json!({"pubkey": key.public_key().to_hex()}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let challenge: Value = response.json().await.unwrap();
    nostr::EventBuilder::new(nostr::Kind::Custom(27235), "")
        .tags([
            nostr::Tag::parse(["u", challenge["url"].as_str().unwrap()]).unwrap(),
            nostr::Tag::parse(["method", "POST"]).unwrap(),
            nostr::Tag::parse(["challenge", challenge["challenge"].as_str().unwrap()]).unwrap(),
            nostr::Tag::parse(["account", login.principal.as_str()]).unwrap(),
            nostr::Tag::parse(["action", "initialize"]).unwrap(),
        ])
        .sign_with_keys(key)
        .unwrap()
}

async fn initialize(
    inst: &Instance,
    login: &Login,
    key: &nostr::Keys,
    proof: &nostr::Event,
) -> reqwest::Response {
    inst.post("/auth/key-backup")
        .bearer_auth(&login.access)
        .json(&json!({
            "secret_key": key.secret_key().to_secret_hex(), "proof": proof,
        }))
        .send()
        .await
        .unwrap()
}

async fn restore(inst: &Instance, login: &Login) -> reqwest::Response {
    inst.post("/auth/key-backup/restore")
        .bearer_auth(&login.access)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
}

async fn signed_post(
    inst: &Instance,
    key: &nostr::Keys,
    path: &str,
    body: Value,
    owner_auth: Option<&str>,
) -> reqwest::Response {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&body).unwrap();
    let scheme = if inst.state.config.relay_url.starts_with("wss://") {
        "https"
    } else {
        "http"
    };
    let url = format!("{scheme}://{}{path}", inst.host);
    let payload = hex::encode(Sha256::digest(&bytes));
    let event = nostr::EventBuilder::new(nostr::Kind::HttpAuth, "")
        .tags([
            nostr::Tag::parse(["u", &url]).unwrap(),
            nostr::Tag::parse(["method", "POST"]).unwrap(),
            nostr::Tag::parse(["payload", &payload]).unwrap(),
            nostr::Tag::parse(["nonce", &uuid::Uuid::new_v4().to_string()]).unwrap(),
        ])
        .sign_with_keys(key)
        .unwrap();
    let authorization = format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap())
    );
    let mut request = inst
        .post(path)
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes);
    if let Some(tag) = owner_auth {
        request = request.header("x-auth-tag", tag);
    }
    request.send().await.unwrap()
}

async fn assert_signed_submit(
    inst: &Instance,
    key: &nostr::Keys,
    event: &nostr::Event,
    owner_auth: Option<&str>,
) {
    let response = signed_post(inst, key, "/events", json!(event), owner_auth).await;
    let status = response.status();
    let result: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["accepted"], true, "{result}");
}

#[tokio::test]
#[ignore = "requires isolated Postgres + Redis with current migrations"]
async fn client_signup_backup_new_device_restore_preserves_signatures_and_existing_link() {
    let inst = custody_instance().await;
    let subject = uuid::Uuid::new_v4().to_string();
    let first = key_login(&inst, &subject).await;
    let status: Value = inst
        .get("/auth/key-backup")
        .bearer_auth(&first.access)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["state"], "absent");
    // The client generates this key. The relay never creates a signing key.
    let key = nostr::Keys::generate();
    let agent_one = nostr::Keys::generate();
    let agent_two = nostr::Keys::generate();
    let agent_one_auth =
        buzz_sdk::nip_oa::compute_auth_tag(&key, &agent_one.public_key(), "").unwrap();
    let agent_two_auth =
        buzz_sdk::nip_oa::compute_auth_tag(&key, &agent_two.public_key(), "").unwrap();
    let before = nostr::EventBuilder::text_note("existing signed identity")
        .tags([nostr::Tag::public_key(agent_one.public_key())])
        .sign_with_keys(&key)
        .unwrap();
    assert_signed_submit(&inst, &key, &before, None).await;
    let signed = proof(&inst, &first, &key).await;
    assert_eq!(initialize(&inst, &first, &key, &signed).await.status(), 200);
    let second = key_login(&inst, &subject).await;
    assert_eq!(first.principal, second.principal);
    let response = restore(&inst, &second).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: Value = response.json().await.unwrap();
    let restored = nostr::Keys::parse(body["secret_key"].as_str().unwrap()).unwrap();
    assert_eq!(restored.public_key(), before.pubkey);
    assert_eq!(body["pubkey"], key.public_key().to_hex());
    before.verify().unwrap();
    let after = nostr::EventBuilder::text_note("restored signed identity")
        .sign_with_keys(&restored)
        .unwrap();
    after.verify().unwrap();
    assert_eq!(before.pubkey, after.pubkey);
    assert_signed_submit(&inst, &restored, &after, None).await;
    let queried = signed_post(
        &inst,
        &restored,
        "/query",
        json!([{
            "kinds": [1], "authors": [restored.public_key().to_hex()],
        }]),
        None,
    )
    .await;
    let status = queried.status();
    let events: Value = queried.json().await.unwrap();
    assert_eq!(status, 200, "{events}");
    for original in [&before, &after] {
        let stored = events
            .as_array()
            .unwrap()
            .iter()
            .find(|value| value["id"] == original.id.to_hex())
            .unwrap();
        let verified: nostr::Event = serde_json::from_value(stored.clone()).unwrap();
        verified.verify().unwrap();
        assert_eq!(verified.pubkey, restored.public_key());
        assert_eq!(
            verified.tags, original.tags,
            "mention tags survive recovery unchanged"
        );
    }
    // Existing agent keys and owner attestations remain usable after recovery;
    // both siblings materialize under the same recovered owner, not account ID.
    let community = inst
        .state
        .db
        .lookup_community_by_host(&inst.host)
        .await
        .unwrap()
        .unwrap()
        .id;
    for (agent, auth, sibling) in [
        (&agent_one, &agent_one_auth, &agent_two),
        (&agent_two, &agent_two_auth, &agent_one),
    ] {
        assert_eq!(
            buzz_sdk::nip_oa::verify_auth_tag(auth, &agent.public_key()).unwrap(),
            restored.public_key()
        );
        let note = nostr::EventBuilder::text_note("existing agent speaking to sibling")
            .tags([nostr::Tag::public_key(sibling.public_key())])
            .sign_with_keys(agent)
            .unwrap();
        assert_signed_submit(&inst, agent, &note, Some(auth)).await;
        assert!(inst
            .state
            .db
            .is_agent_owner(
                community,
                agent.public_key().as_bytes(),
                restored.public_key().as_bytes()
            )
            .await
            .unwrap());
    }
    // A separately established local key links through the same proof seam.
    let existing_key = nostr::Keys::generate();
    let existing_event = nostr::EventBuilder::text_note("old history")
        .sign_with_keys(&existing_key)
        .unwrap();
    let owner = key_login(&inst, &uuid::Uuid::new_v4().to_string()).await;
    let signed = proof(&inst, &owner, &existing_key).await;
    assert_eq!(
        initialize(&inst, &owner, &existing_key, &signed)
            .await
            .status(),
        200
    );
    let restored: Value = restore(&inst, &owner).await.json().await.unwrap();
    assert_eq!(restored["pubkey"], existing_event.pubkey.to_hex());
}

#[tokio::test]
#[ignore = "requires isolated Postgres + Redis with current migrations"]
async fn custody_rejects_cross_account_restore_bad_proof_replay_and_token_bot_adoption() {
    let inst = custody_instance().await;
    let owner = key_login(&inst, &uuid::Uuid::new_v4().to_string()).await;
    let other = key_login(&inst, &uuid::Uuid::new_v4().to_string()).await;
    let key = nostr::Keys::generate();
    assert_eq!(
        inst.post("/auth/key-backup/restore")
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let valid = proof(&inst, &owner, &key).await;
    let mut bad = valid.clone();
    bad.content = "tampered".into();
    assert_eq!(initialize(&inst, &owner, &key, &bad).await.status(), 400);
    assert_eq!(
        initialize(&inst, &owner, &key, &valid).await.status(),
        400,
        "bad attempt consumes challenge"
    );
    let valid = proof(&inst, &owner, &key).await;
    assert_eq!(
        initialize(&inst, &other, &key, &valid).await.status(),
        400,
        "session/account-bound challenge"
    );
    let valid = proof(&inst, &owner, &key).await;
    assert_eq!(initialize(&inst, &owner, &key, &valid).await.status(), 200);
    assert_eq!(
        initialize(&inst, &owner, &key, &valid).await.status(),
        400,
        "one-use proof"
    );
    assert_eq!(restore(&inst, &other).await.status(), 404);
    assert_eq!(
        inst.post("/auth/key-backup/restore")
            .bearer_auth(&other.access)
            .json(&json!({"account_id": owner.principal}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let token = buzz_auth::TokenSecret::new(owner.access.clone());
    assert!(crate::identity::verify_access_token(&inst.state, &token)
        .await
        .is_err());
    assert!(crate::identity::verify_session_token(&inst.state, &token)
        .await
        .is_ok());
    assert_eq!(
        inst.post("/auth/bots")
            .bearer_auth(&owner.access)
            .json(&json!({"display_name":"no adoption"}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        inst.delete("/auth/account")
            .bearer_auth(&owner.access)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
}

#[tokio::test]
#[ignore = "requires isolated Postgres + Redis with current migrations"]
async fn concurrent_initialization_is_immutable_and_refresh_never_reauthenticates_exports() {
    let inst = custody_instance().await;
    let subject = uuid::Uuid::new_v4().to_string();
    let one = key_login(&inst, &subject).await;
    let two = key_login(&inst, &subject).await;
    let key_one = nostr::Keys::generate();
    let key_two = nostr::Keys::generate();
    let proof_one = proof(&inst, &one, &key_one).await;
    let proof_two = proof(&inst, &two, &key_two).await;
    let (one_result, two_result) = tokio::join!(
        initialize(&inst, &one, &key_one, &proof_one),
        initialize(&inst, &two, &key_two, &proof_two),
    );
    let mut statuses = [one_result.status().as_u16(), two_result.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let winner = if one_result.status() == 200 {
        &key_one
    } else {
        &key_two
    };
    let restored: Value = restore(&inst, &two).await.json().await.unwrap();
    assert_eq!(restored["pubkey"], winner.public_key().to_hex());
    let refresh: Value = inst
        .post("/auth/refresh")
        .json(&json!({"refresh":two.refresh}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let access = refresh["access"].as_str().unwrap();
    let me: Value = inst
        .get("/auth/me")
        .bearer_auth(access)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["principal_id"], two.principal);
    assert_eq!(me["identity_mode"], "key_backup");
    assert_eq!(me["signing_pubkey"], winner.public_key().to_hex());
    let account = PrincipalId::from_hex(&two.principal).unwrap();
    sqlx::query(
        "UPDATE sessions SET created_at = now() - interval '10 minutes' WHERE device_id IN (SELECT id FROM devices WHERE principal_id = $1)",
    )
    .bind(account.as_bytes().as_slice())
    .execute(inst.state.db.pool())
    .await
    .unwrap();
    let response = inst
        .post("/auth/key-backup/restore")
        .bearer_auth(access)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let error: Value = response.json().await.unwrap();
    assert_eq!(error["code"], "reauth_required");
    let fresh = key_login(&inst, &subject).await;
    assert_eq!(restore(&inst, &fresh).await.status(), 200);
}

#[tokio::test]
#[ignore = "requires isolated Postgres + Redis with current migrations"]
async fn custody_corrupt_ciphertext_and_existing_token_account_fail_closed() {
    let inst = custody_instance().await;
    let subject = uuid::Uuid::new_v4().to_string();
    let owner = key_login(&inst, &subject).await;
    let key = nostr::Keys::generate();
    let signed = proof(&inst, &owner, &key).await;
    assert_eq!(initialize(&inst, &owner, &key, &signed).await.status(), 200);
    let account = PrincipalId::from_hex(&owner.principal).unwrap();
    let encrypted = inst
        .state
        .db
        .get_key_backup(&account)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE account_key_backups SET ciphertext = set_byte(ciphertext, 0, get_byte(ciphertext, 0) # 1) WHERE account_id = $1")
        .bind(account.as_bytes().as_slice()).execute(inst.state.db.pool()).await.unwrap();
    let response = restore(&inst, &owner).await;
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "backup_unavailable");
    assert!(body.get("secret_key").is_none());
    let wrong_pubkey = nostr::Keys::generate().public_key().to_bytes();
    sqlx::query(
        "UPDATE account_key_backups SET ciphertext = $2, pubkey = $3 WHERE account_id = $1",
    )
    .bind(account.as_bytes().as_slice())
    .bind(&encrypted.ciphertext)
    .bind(wrong_pubkey.as_slice())
    .execute(inst.state.db.pool())
    .await
    .unwrap();
    let response = restore(&inst, &owner).await;
    assert_eq!(
        response.status(),
        503,
        "valid ciphertext with wrong public-key AAD must fail closed"
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "backup_unavailable");
    assert!(body.get("secret_key").is_none());
    let status: Value = inst
        .get("/auth/key-backup")
        .bearer_auth(&owner.access)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status["state"], "ready",
        "corruption must never authorize creation"
    );

    let old_subject = uuid::Uuid::new_v4().to_string();
    let old = inst
        .state
        .db
        .login_identity("google", &old_subject, None, "Existing account", None)
        .await
        .unwrap();
    reset_login_limits(&inst).await;
    let verifier = verifier();
    let state = &verifier[..24];
    let challenge = buzz_auth::token::pkce_s256_challenge(&verifier);
    let start = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state),
                ("code_challenge", &challenge),
                ("client", "desktop"),
                ("redirect_uri", REDIRECT_URI),
                ("identity_mode", "key_backup"),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), 302);
    let callback = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("state", state), ("code", &old_subject)],
        )
        .send()
        .await
        .unwrap();
    let location = callback.headers()["location"].to_str().unwrap();
    assert_eq!(
        query_param(location, "error").as_deref(),
        Some("account_mode_conflict")
    );
    assert!(query_param(location, "code").is_none());
    assert!(!inst
        .state
        .db
        .account_key_mode(&old.principal)
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "requires isolated Postgres + Redis with current migrations"]
async fn custody_web_cli_clients_get_explicit_unsupported_flow_instead_of_new_principal() {
    let inst = custody_instance().await;
    reset_login_limits(&inst).await;
    for client in ["web", "cli"] {
        let verifier = verifier();
        let challenge = buzz_auth::token::pkce_s256_challenge(&verifier);
        let response = inst
            .get_with(
                "/auth/oidc/google/start",
                &[
                    ("state", &verifier[..24]),
                    ("code_challenge", &challenge),
                    ("client", client),
                    ("identity_mode", "key_backup"),
                    ("redirect_uri", REDIRECT_URI),
                ],
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["code"], "unsupported_client");
    }
}

#[tokio::test]
#[ignore = "requires isolated Postgres + Redis with current migrations"]
async fn custody_preserves_existing_token_login_but_refuses_new_token_identity() {
    let inst = custody_instance().await;
    let existing_subject = uuid::Uuid::new_v4().to_string();
    let existing = inst
        .state
        .db
        .login_identity("google", &existing_subject, None, "Existing", None)
        .await
        .unwrap();
    reset_login_limits(&inst).await;
    let verifier = verifier();
    // An unupdated token client omits identity_mode. Dual-mode deployments
    // keep that existing login working, while new clients choose explicitly.
    let state = &verifier[..24];
    let challenge = buzz_auth::token::pkce_s256_challenge(&verifier);
    let start = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state),
                ("code_challenge", &challenge),
                ("client", "cli"),
                ("redirect_uri", REDIRECT_URI),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), 302);
    let callback = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("state", state), ("code", &existing_subject)],
        )
        .send()
        .await
        .unwrap();
    let location = callback.headers()["location"].to_str().unwrap();
    let code = query_param(location, "code").unwrap();
    let login = complete(&inst, &code, &verifier).await;
    assert_eq!(login.principal, existing.principal.to_hex());
    let me: Value = inst
        .get("/auth/me")
        .bearer_auth(&login.access)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["identity_mode"], "token");

    let subject = uuid::Uuid::new_v4().to_string();
    let verifier = super::verifier();
    let state = &verifier[..24];
    let challenge = buzz_auth::token::pkce_s256_challenge(&verifier);
    let start = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state),
                ("code_challenge", &challenge),
                ("client", "cli"),
                ("redirect_uri", REDIRECT_URI),
                ("identity_mode", "token"),
            ],
        )
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), 302);
    let callback = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("state", state), ("code", &subject)],
        )
        .send()
        .await
        .unwrap();
    let location = callback.headers()["location"].to_str().unwrap();
    assert_eq!(
        query_param(location, "error").as_deref(),
        Some("unsupported_client")
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM identities WHERE provider = 'google' AND subject = $1",
    )
    .bind(&subject)
    .fetch_one(inst.state.db.pool())
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "unsupported signup must not create a token principal"
    );
}
