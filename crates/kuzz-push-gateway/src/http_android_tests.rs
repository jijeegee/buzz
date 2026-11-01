//! Android enrollment and signed mutations through the public router, with a
//! synthetic Key Attestation chain, an in-memory authority store and no
//! external service.
use super::*;
use crate::{
    android_attest::{
        tests::{
            allowed_digest, chain_for_key, key_id_of, root, Fetcher, Root, Spec, HARDWARE_ROOT_CN,
            PACKAGE,
        },
        AndroidAttestVerifier, RevocationList,
    },
    authority::MemoryAuthorityStore,
    grant::{GrantKey, GrantKeyring},
    token::{TokenKey, TokenKeyring},
};
use axum::{body::Body, http::Request};
use rcgen::{KeyPair, SigningKey, PKCS_ECDSA_P256_SHA256};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const NOW: i64 = 1_800_000_000;
const FCM_TOKEN: &str = "dGVzdA:APA91bH-synthetic_token";
const RELAY: &str = "11111111111111111111111111111111111111111111111111111111111111aa";

struct NeverTransport;
#[async_trait::async_trait]
impl PushTransport for NeverTransport {
    async fn send(&self, _: DeliveryAttempt, _: &str) -> DeliveryOutcome {
        panic!("enrollment tests never deliver")
    }
}

fn now() -> i64 {
    NOW
}

fn state(root: &Root, android: bool) -> (AppState, Arc<MemoryAuthorityStore>) {
    let authority = Arc::new(MemoryAuthorityStore::default());
    let app_attest = AppAttestVerifier::new(
        "TEAMID.app.kuzz.dogfood.mobile".to_owned(),
        include_bytes!("../tests/fixtures/apple-app-attestation-root.pem").to_vec(),
    )
    .unwrap();
    let android = android.then(|| {
        Arc::new(AndroidProfileRuntime {
            attestation: Arc::new(
                AndroidAttestVerifier::new(
                    PACKAGE,
                    vec![allowed_digest()],
                    &root.pem,
                    Arc::new(RevocationList::new(Arc::new(Fetcher::default()))),
                )
                .unwrap(),
            ),
            transport: Arc::new(NeverTransport),
        })
    });
    (
        AppState {
            grant_keyring: Arc::new(
                GrantKeyring::new(vec![GrantKey::new("test", &[1; 32]).unwrap()]).unwrap(),
            ),
            authority: authority.clone(),
            token_keyring: Arc::new(
                TokenKeyring::new(vec![TokenKey::new("test", &[2; 32]).unwrap()]).unwrap(),
            ),
            profile: Arc::new(ProfileRuntime {
                app_attest: Arc::new(app_attest),
                transport: Arc::new(NeverTransport),
            }),
            android,
            max_grant_lifetime_seconds: 86_400,
            max_installation_lifetime_seconds: 86_400,
            endpoint_quota_window_seconds: 60,
            endpoint_quota_max_deliveries: 10,
            now,
            accepting: Arc::new(AtomicBool::new(true)),
        },
        authority,
    )
}

async fn post(
    app: &Router,
    path: &str,
    body: &impl serde::Serialize,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn challenge(app: &Router) -> (uuid::Uuid, String) {
    let (status, body) = post(
        app,
        "/v1/installations/challenges",
        &serde_json::json!({"v": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    (
        body["challenge_id"].as_str().unwrap().parse().unwrap(),
        body["challenge"].as_str().unwrap().to_owned(),
    )
}

/// Build an enrollment whose attestation challenge is SHA-256 of its exact
/// transcript, the way the Android app does.
fn enrollment(
    root: &Root,
    leaf_key: KeyPair,
    challenge: (uuid::Uuid, String),
    endpoint: &str,
) -> (InstallationEnrollRequest, KeyPair) {
    let key_id = key_id_of(&leaf_key);
    let t = EnrollTranscript {
        v: WIRE_VERSION,
        audience: ENROLL_AUDIENCE,
        challenge_id: challenge.0,
        challenge: &challenge.1,
        key_id: &key_id,
        app_profile: AppProfile::KuzzAndroid,
        endpoint,
        endpoint_epoch: 1,
        expires_at: NOW + 3_600,
    };
    let signed = transcript("kuzz.push.enroll.v1", &t).unwrap();
    let spec = Spec {
        challenge: Sha256::digest(signed.as_bytes()).to_vec(),
        ..Spec::default()
    };
    let chain = chain_for_key(root, &spec, leaf_key);
    let request = InstallationEnrollRequest {
        v: WIRE_VERSION,
        challenge_id: challenge.0,
        challenge: challenge.1,
        key_id,
        attestation: chain.attestation,
        app_profile: AppProfile::KuzzAndroid,
        endpoint: endpoint.to_owned(),
        endpoint_epoch: 1,
        expires_at: NOW + 3_600,
    };
    (request, chain.leaf_key)
}

fn delegation(
    signer: &KeyPair,
    installation: uuid::Uuid,
    challenge: (uuid::Uuid, String),
    generation: i64,
) -> DelegationRequest {
    let t = DelegateTranscript {
        v: WIRE_VERSION,
        audience: DELEGATE_AUDIENCE,
        challenge_id: challenge.0,
        challenge: &challenge.1,
        installation_handle: installation,
        endpoint_epoch: 1,
        generation,
        relay_pubkey: RELAY,
        not_before: NOW,
        expires_at: NOW + 3_600,
    };
    let signed = transcript("kuzz.push.delegate.v1", &t).unwrap();
    DelegationRequest {
        v: WIRE_VERSION,
        challenge_id: challenge.0,
        challenge: challenge.1,
        installation_handle: installation,
        endpoint_epoch: 1,
        generation,
        relay_pubkey: RELAY.to_owned(),
        not_before: NOW,
        expires_at: NOW + 3_600,
        assertion: STANDARD.encode(signer.sign(signed.as_bytes()).unwrap()),
    }
}

#[tokio::test]
async fn android_installation_enrolls_and_delegates_with_its_attested_key() {
    let root = root(HARDWARE_ROOT_CN);
    let (state, authority) = state(&root, true);
    let (app, _) = router(state);
    let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let (request, signer) = enrollment(
        &root,
        leaf_key,
        challenge(&app).await,
        &hex::encode(FCM_TOKEN),
    );
    let leaf_spki = rcgen::PublicKeyData::subject_public_key_info(&signer);
    let (status, body) = post(&app, "/v1/installations", &request).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let installation: uuid::Uuid = body["installation_handle"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let stored = authority.installation(installation, NOW).await.unwrap();
    assert_eq!(stored.profile, AppProfile::KuzzAndroid);
    assert_eq!(stored.app_attest_public_key, leaf_spki);
    assert_eq!(
        stored.app_attest_key_id,
        Sha256::digest(&leaf_spki).to_vec()
    );
    assert_eq!(stored.assertion_counter, 0);

    // Repeating the exact enrollment returns the same installation.
    let (status, body) = post(&app, "/v1/installations", &request).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["installation_handle"], installation.to_string());

    let signed = delegation(&signer, installation, challenge(&app).await, 1);
    let (status, body) = post(&app, "/v1/delegations", &signed).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let grant = body["endpoint_grant"].as_str().unwrap();
    let keyring = GrantKeyring::new(vec![GrantKey::new("test", &[1; 32]).unwrap()]).unwrap();
    assert_eq!(
        keyring.open(grant).unwrap().app_profile,
        AppProfile::KuzzAndroid
    );
    assert_eq!(
        authority
            .installation(installation, NOW)
            .await
            .unwrap()
            .assertion_counter,
        1
    );

    // Replaying the signed delegation fails: its challenge was consumed.
    let (status, _) = post(&app, "/v1/delegations", &signed).await;
    assert!(
        !status.is_success(),
        "replayed delegation accepted: {status}"
    );

    // Another key cannot sign for this installation.
    let other = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let forged = delegation(&other, installation, challenge(&app).await, 2);
    let (status, _) = post(&app, "/v1/delegations", &forged).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        authority
            .installation(installation, NOW)
            .await
            .unwrap()
            .assertion_counter,
        1
    );

    let renewed = delegation(&signer, installation, challenge(&app).await, 2);
    let (status, _) = post(&app, "/v1/delegations", &renewed).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        authority
            .installation(installation, NOW)
            .await
            .unwrap()
            .assertion_counter,
        2
    );
}

#[tokio::test]
async fn android_enrollment_refuses_bad_attestation_tokens_and_disabled_profile() {
    let root = root(HARDWARE_ROOT_CN);
    let (state, _) = state(&root, true);
    let (app, _) = router(state);

    // Attestation bound to a different transcript (here: another endpoint).
    let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let (mut request, _) = enrollment(
        &root,
        leaf_key,
        challenge(&app).await,
        &hex::encode(FCM_TOKEN),
    );
    request.endpoint = hex::encode("another-token");
    let (status, _) = post(&app, "/v1/installations", &request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // N11: tokens over 512 bytes, and non-printable tokens, are refused.
    for token in ["a".repeat(513).into_bytes(), b"tab\there".to_vec()] {
        let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let (request, _) = enrollment(&root, leaf_key, challenge(&app).await, &hex::encode(&token));
        let (status, _) = post(&app, "/v1/installations", &request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // Without an Android runtime the profile is not offered.
    let (disabled, _) = self::state(&root, false);
    let (app, _) = router(disabled);
    let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let (request, _) = enrollment(
        &root,
        leaf_key,
        challenge(&app).await,
        &hex::encode(FCM_TOKEN),
    );
    let (status, _) = post(&app, "/v1/installations", &request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
