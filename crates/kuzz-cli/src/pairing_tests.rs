//! `kuzz pair` against a relay stand-in that verifies claims with the relay's
//! own [`kuzz_auth::pairing::verify_pair_claim`], the pairing store, and how
//! the paired agent becomes the identity.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use kuzz_auth::device_proof::DevicePubkey;
use kuzz_auth::pairing::verify_pair_claim;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::*;
use crate::auth_session::{IssuedTokens, MemoryStore, RefreshStore};
use crate::device::test_support::file_signer;

const CODE: &str = "AAAAAAAAAAAAAAAAAAAAAA";
const OWNER: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

/// The relay's `ClaimRequest` (`crates/kuzz-relay/src/api/auth/pairing.rs`).
#[allow(dead_code)]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimShape {
    code: String,
    agent_pubkey: String,
    agent_signature: String,
    device_pubkey: String,
    device_signature: String,
    #[serde(default)]
    device_key_kind: Option<String>,
    #[serde(default)]
    device_name: Option<String>,
}

#[derive(Clone)]
struct Fake {
    /// The answer to a claim whose signatures verify.
    answer: Arc<(StatusCode, Value)>,
    claims: Arc<Mutex<Vec<Value>>>,
}

/// A relay whose `POST /pair/claim` verifies like the real one, then answers
/// `answer` (or `invalid_signature`).
async fn fake_relay(answer: (StatusCode, Value)) -> (String, Arc<Mutex<Vec<Value>>>) {
    let claims = Arc::new(Mutex::new(Vec::new()));
    let fake = Fake {
        answer: Arc::new(answer),
        claims: Arc::clone(&claims),
    };
    let app = Router::new()
        .route(
            "/pair/claim",
            post(
                |State(fake): State<Fake>, Json(body): Json<Value>| async move {
                    fake.claims.lock().unwrap().push(body.clone());
                    // The relay's body shape (`deny_unknown_fields`).
                    if serde_json::from_value::<ClaimShape>(body.clone()).is_err() {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "code": "invalid_request", "error": "shape" })),
                        );
                    }
                    let agent = nostr::PublicKey::from_hex(body["agent_pubkey"].as_str().unwrap())
                        .unwrap();
                    let device =
                        DevicePubkey::from_hex(body["device_pubkey"].as_str().unwrap()).unwrap();
                    let verified = verify_pair_claim(
                        body["code"].as_str().unwrap(),
                        &agent,
                        body["agent_signature"].as_str().unwrap(),
                        &device,
                        body["device_signature"].as_str().unwrap(),
                    );
                    if verified.is_err() {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "code": "invalid_signature", "error": "bad" })),
                        );
                    }
                    let (status, answer) = (*fake.answer).clone();
                    (status, Json(answer))
                },
            ),
        )
        // The relay's answer to a paired agent asking about itself.
        .route(
            "/agents/{pubkey}/status",
            get(|Path(pubkey): Path<String>| async move {
                Json(json!({
                    "admission": "log",
                    "keys": [{ "pubkey": pubkey, "role": "external", "owner": OWNER, "live": true }],
                }))
            }),
        )
        .with_state(fake);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), claims)
}

fn connected() -> (StatusCode, Value) {
    (
        StatusCode::OK,
        json!({
            "pairing_id": "11111111-1111-4111-8111-111111111111",
            "state": "connected",
            "device_id": "22222222-2222-4222-8222-222222222222",
            "version": 1,
        }),
    )
}

fn refused(status: StatusCode, code: &str) -> (StatusCode, Value) {
    (status, json!({ "code": code, "error": "refused" }))
}

fn request<'a>(relay_url: &'a str, keys: Option<Keys>) -> PairRequest<'a> {
    PairRequest {
        relay_url,
        code: CODE,
        agent_name: Some("Hermes"),
        device_name: "studio mac",
        agent_keys: keys,
    }
}

fn pairing_at(relay: &str, name: &str, at: i64, secret: bool) -> Pairing {
    let keys = Keys::generate();
    Pairing {
        version: 1,
        relay: relay.into(),
        agent_pubkey: keys.public_key().to_hex(),
        agent_name: Some(name.into()),
        device_name: "pc".into(),
        state: "connected".into(),
        pairing_id: None,
        device_id: None,
        paired_at: at,
        owner: None,
        agent_secret: secret.then(|| keys.secret_key().to_secret_hex()),
    }
}

#[tokio::test]
async fn a_claim_verifies_with_the_relays_check_and_saves_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let (_, device) = file_signer(&dir);
    let store = PairingStore::new(dir.path().join("pairings"));
    let (relay, claims) = fake_relay(connected()).await;

    let pairing = pair(request(&relay, None), &device, &store).await.unwrap();

    let claim = claims.lock().unwrap()[0].clone();
    assert_eq!(claim["code"], CODE);
    assert_eq!(claim["device_pubkey"], device.pubkey().to_hex());
    assert_eq!(claim["device_key_kind"], "software");
    assert_eq!(claim["device_name"], "studio mac");
    assert_eq!(claim["agent_pubkey"], pairing.agent_pubkey);
    assert_eq!(pairing.state, "connected");
    assert_eq!(
        pairing.pairing_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    // The generated key is saved and is the claimed agent.
    let saved = store.list();
    assert_eq!(saved.len(), 1);
    let keys = saved[0].keys().expect("secret saved");
    assert_eq!(keys.public_key().to_hex(), pairing.agent_pubkey);
    assert_eq!(pairing.label(), "Hermes");
}

#[tokio::test]
async fn an_agents_own_key_is_paired_but_not_stored() {
    let dir = tempfile::tempdir().unwrap();
    let (_, device) = file_signer(&dir);
    let store = PairingStore::new(dir.path().join("pairings"));
    let (relay, claims) = fake_relay(connected()).await;
    let own = Keys::generate();

    let pairing = pair(request(&relay, Some(own.clone())), &device, &store)
        .await
        .unwrap();

    assert_eq!(pairing.agent_pubkey, own.public_key().to_hex());
    assert_eq!(
        claims.lock().unwrap()[0]["agent_pubkey"],
        pairing.agent_pubkey
    );
    let raw = std::fs::read_to_string(
        dir.path()
            .join("pairings")
            .join(format!("{}.json", pairing.agent_pubkey)),
    )
    .unwrap();
    assert!(!raw.contains(&own.secret_key().to_secret_hex()));
    assert!(!raw.contains("agent_secret"));
    // Without its secret the pairing is never an identity.
    assert!(store.for_relay(&pairing.relay, None).is_none());
}

#[tokio::test]
async fn refusals_are_clear_and_leave_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (_, device) = file_signer(&dir);
    let store = PairingStore::new(dir.path().join("pairings"));
    for (status, code, expect) in [
        (StatusCode::CONFLICT, "code_used", "code_used"),
        (StatusCode::NOT_FOUND, "invalid_code", "invalid or expired"),
        (StatusCode::CONFLICT, "key_in_use", "key_in_use"),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "rate_limited",
        ),
    ] {
        let (relay, _) = fake_relay(refused(status, code)).await;
        let error = pair(request(&relay, None), &device, &store)
            .await
            .expect_err(code);
        assert!(error.to_string().contains(expect), "{code}: {error}");
        assert!(store.list().is_empty(), "{code}: pairing left behind");
    }
}

#[tokio::test]
async fn a_lost_answer_keeps_the_key_pending() {
    let dir = tempfile::tempdir().unwrap();
    let (_, device) = file_signer(&dir);
    let store = PairingStore::new(dir.path().join("pairings"));
    // Nothing listens there.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let relay = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);

    let error = pair(request(&relay, None), &device, &store)
        .await
        .expect_err("no relay");
    assert!(matches!(error, CliError::DeliveryUnknown(_)), "{error}");
    let saved = store.list();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].state, "pending");
    assert!(saved[0].keys().is_some());
}

#[test]
fn a_claim_signed_for_one_code_fails_for_another() {
    let dir = tempfile::tempdir().unwrap();
    let (_, device) = file_signer(&dir);
    let agent = Keys::generate();
    let body = claim_body(CODE, &agent, &device, "pc").unwrap();
    let verify = |code: &str| {
        verify_pair_claim(
            code,
            &agent.public_key(),
            body["agent_signature"].as_str().unwrap(),
            &device.pubkey(),
            body["device_signature"].as_str().unwrap(),
        )
    };
    assert_eq!(verify(CODE), Ok(()));
    assert!(verify("BBBBBBBBBBBBBBBBBBBBBB").is_err());
}

#[test]
fn the_newest_or_named_pairing_of_the_relay_is_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let store = PairingStore::new(dir.path());
    let relay = "https://relay.example";
    let old = pairing_at(relay, "Old", 100, true);
    let new = pairing_at(relay, "New", 200, true);
    let other = pairing_at("https://other.example", "Elsewhere", 300, true);
    let keyless = pairing_at(relay, "Keyless", 400, false);
    for pairing in [&old, &new, &other, &keyless] {
        store.save(pairing).unwrap();
    }
    let pick = |selector| store.for_relay(relay, selector).map(|p| p.agent_pubkey);
    assert_eq!(pick(None), Some(new.agent_pubkey.clone()));
    assert_eq!(pick(Some("Old")), Some(old.agent_pubkey.clone()));
    assert_eq!(
        pick(Some(old.agent_pubkey.as_str())),
        Some(old.agent_pubkey.clone())
    );
    assert_eq!(pick(Some("Keyless")), None);
    assert_eq!(pick(Some("Nobody")), None);
    // Two relays: no single default relay.
    assert_eq!(store.sole_relay(), None);
    store.remove(&other.agent_pubkey);
    assert_eq!(store.sole_relay().as_deref(), Some(relay));
}

fn session_store(dir: &tempfile::TempDir) -> SessionStore {
    SessionStore::new(
        dir.path().join("session.json"),
        Arc::new(MemoryStore::default()) as Arc<dyn RefreshStore>,
    )
}

#[tokio::test]
async fn the_paired_agent_acts_only_without_a_signed_in_person_or_in_managed_agents() {
    let dir = tempfile::tempdir().unwrap();
    let store = PairingStore::new(dir.path().join("pairings"));
    let relay = "https://relay.example";
    let pairing = pairing_at(relay, "Hermes", 100, true);
    store.save(&pairing).unwrap();
    let sessions = session_store(&dir);
    let no_env = |_: &str| None;

    let (keys, chosen) =
        paired_identity(&format!("{relay}/"), Some(&sessions), Some(&store), no_env)
            .expect("paired identity");
    assert_eq!(keys.public_key().to_hex(), pairing.agent_pubkey);
    assert_eq!(chosen.agent_pubkey, pairing.agent_pubkey);
    assert_eq!(
        paired_relay(Some(&store), no_env).as_deref(),
        Some(relay),
        "the paired relay is the default relay"
    );

    // Another relay has no pairing.
    assert!(paired_identity(
        "https://other.example",
        Some(&sessions),
        Some(&store),
        no_env
    )
    .is_none());

    // A managed agent never acts as the paired agent.
    let managed = |name: &str| {
        (name == kuzz_token_broker::DISABLE_STORED_SESSION_ENV).then(|| "1".to_owned())
    };
    assert!(paired_identity(relay, Some(&sessions), Some(&store), managed).is_none());
    assert!(paired_relay(Some(&store), managed).is_none());

    // A person signed in to this relay keeps acting as themselves.
    sessions
        .save_tokens(
            relay,
            "alice",
            None,
            &IssuedTokens {
                access: kuzz_token_broker::Secret::new("bzs_a".into()),
                refresh: kuzz_token_broker::Secret::new("bzr_r".into()),
                expires_in: 600,
            },
        )
        .await
        .unwrap();
    assert!(paired_identity(relay, Some(&sessions), Some(&store), no_env).is_none());
}

#[test]
fn pairings_never_show_their_secret() {
    let pairing = pairing_at("https://relay.example", "Hermes", 1, true);
    let secret = pairing.agent_secret.clone().unwrap();
    assert!(!format!("{pairing:?}").contains(&secret));
}

#[cfg(unix)]
#[test]
fn pairing_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let store = PairingStore::new(dir.path().join("pairings"));
    let pairing = pairing_at("https://relay.example", "Hermes", 1, true);
    store.save(&pairing).unwrap();
    let mode = |path: std::path::PathBuf| std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode(dir.path().join("pairings")) & 0o777, 0o700);
    assert_eq!(
        mode(
            dir.path()
                .join("pairings")
                .join(format!("{}.json", pairing.agent_pubkey))
        ) & 0o777,
        0o600
    );
}

#[test]
fn names_are_bounded_like_the_relay() {
    assert!(validate_name("--name", "Hermes").is_ok());
    assert!(validate_name("--name", "").is_err());
    assert!(validate_name("--name", &"x".repeat(65)).is_err());
    assert!(validate_name("--name", "a\nb").is_err());
}

#[tokio::test]
async fn whoami_names_the_owner_and_remembers_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = PairingStore::new(dir.path().join("pairings"));
    let (relay, _) = fake_relay(connected()).await;
    let pairing = pairing_at(&relay_origin(&relay).unwrap(), "Hermes", 1, true);
    store.save(&pairing).unwrap();
    let keys = pairing.keys().unwrap();

    let me = whoami(&relay, &keys, pairing.clone(), Some(&store)).await;
    assert_eq!(me["mode"], "paired");
    assert_eq!(me["pubkey"], pairing.agent_pubkey);
    assert_eq!(me["owner"], OWNER);
    assert_eq!(store.list()[0].owner.as_deref(), Some(OWNER));
}

#[tokio::test]
async fn an_owner_in_the_claim_answer_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (_, device) = file_signer(&dir);
    let store = PairingStore::new(dir.path().join("pairings"));
    let (status, mut answer) = connected();
    answer["owner_pubkey"] = json!(OWNER);
    let (relay, _) = fake_relay((status, answer)).await;
    let pairing = pair(request(&relay, None), &device, &store).await.unwrap();
    assert_eq!(pairing.owner.as_deref(), Some(OWNER));
}

#[tokio::test]
async fn server_errors_keep_the_key_pending() {
    let dir = tempfile::tempdir().unwrap();
    let (_, device) = file_signer(&dir);
    for (status, code) in [
        (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        (StatusCode::SERVICE_UNAVAILABLE, "rate_limited"),
        (StatusCode::BAD_REQUEST, "something_new"),
    ] {
        let store = PairingStore::new(dir.path().join(format!("pairings-{}", status.as_u16())));
        let (relay, _) = fake_relay(refused(status, code)).await;
        let error = pair(request(&relay, None), &device, &store)
            .await
            .expect_err(code);
        assert!(
            matches!(error, CliError::DeliveryUnknown(_)),
            "{code}: {error}"
        );
        let saved = store.list();
        assert_eq!(saved.len(), 1, "{code}: the key may have connected");
        assert_eq!(saved[0].state, "pending");
    }
    assert!(is_definite_refusal(409, r#"{"code":"code_used"}"#));
    assert!(!is_definite_refusal(500, r#"{"code":"code_used"}"#));
    assert!(!is_definite_refusal(400, "not json"));
}

#[test]
fn a_connected_pairing_wins_over_a_newer_pending_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = PairingStore::new(dir.path());
    let relay = "https://relay.example";
    let connected = pairing_at(relay, "Connected", 100, true);
    let mut pending = pairing_at(relay, "Pending", 200, true);
    pending.state = "pending".into();
    store.save(&connected).unwrap();
    store.save(&pending).unwrap();
    let pick = || store.for_relay(relay, None).map(|p| p.agent_pubkey);
    assert_eq!(pick(), Some(connected.agent_pubkey.clone()));
    // With nothing connected, the pending one (it may have landed) is used.
    store.remove(&connected.agent_pubkey);
    assert_eq!(pick(), Some(pending.agent_pubkey.clone()));
}

#[test]
fn the_hermes_installer_is_looked_for_only_in_install_roots() {
    let exe = std::path::Path::new("/opt/kuzz/bin/kuzz");
    let roots = hermes_installer_roots(exe);
    assert_eq!(roots[0], std::path::Path::new("/opt/kuzz/bin"));
    assert_eq!(roots[1], std::path::Path::new("/opt/kuzz/share/kuzz"));
    assert!(roots.len() <= 3);
    assert!(!roots
        .iter()
        .any(|root| root == std::path::Path::new("/opt/kuzz")
            || root == std::path::Path::new("/opt")
            || root == std::path::Path::new("/")));
    // Not under a `bin` directory: just the binary's own directory.
    let roots = hermes_installer_roots(std::path::Path::new("/home/me/repo/target/release/kuzz"));
    assert_eq!(
        roots[0],
        std::path::Path::new("/home/me/repo/target/release")
    );
    assert!(!roots.iter().any(|root| root.starts_with("/home/me/repo")
        && root != std::path::Path::new("/home/me/repo/target/release")));
}

#[test]
fn pair_needs_a_named_relay_never_the_localhost_default() {
    let default = "http://localhost:3000";
    let error = pair_relay(None, default, true).expect_err("no relay");
    assert!(matches!(error, CliError::Usage(_)), "{error}");
    assert!(error.to_string().contains("--relay"), "{error}");
    assert_eq!(
        pair_relay(None, "https://env.example", false).unwrap(),
        "https://env.example"
    );
    assert_eq!(
        pair_relay(Some("https://code.example"), "https://env.example", false).unwrap(),
        "https://code.example"
    );
    assert!(pair_relay(Some(" "), default, true).is_err());
}

#[test]
fn the_servers_exact_command_parses() {
    // `pair_command` in crates/kuzz-relay/src/api/auth/pairing.rs.
    let command = format!("kuzz pair --relay wss://relay.example {CODE}");
    let cli = crate::parse_args(command.split(' ')).expect("parses");
    let crate::Cmd::Pair {
        code,
        pair_relay: flag,
        ..
    } = cli.command
    else {
        panic!("pair");
    };
    assert_eq!(code, CODE);
    let relay = pair_relay(flag.as_deref(), &cli.relay, cli.relay_from_default).unwrap();
    assert_eq!(
        crate::client::normalize_relay_url(&relay),
        "https://relay.example"
    );
}

#[test]
fn the_relay_can_follow_the_code_in_the_apps_command() {
    for flag in ["--relay", "--relay-url"] {
        let cli = crate::parse_args(["kuzz", "pair", CODE, flag, "https://relay.example"])
            .expect("parses");
        let crate::Cmd::Pair { pair_relay, .. } = cli.command else {
            panic!("pair");
        };
        assert_eq!(pair_relay.as_deref(), Some("https://relay.example"));
        assert!(cli.relay_from_default);
    }
}
