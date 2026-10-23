use super::*;
use crate::auth::accounts::Account;

const ORIGIN: &str = "https://relay.example";

fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| OsString::from(value))
    }
}

#[cfg(windows)]
const HOME: &str = r"C:\Users\u\hermes-home";
#[cfg(not(windows))]
const HOME: &str = "/home/u/hermes-home";
#[cfg(windows)]
const LOCAL: &str = r"C:\Users\u\AppData\Local";
#[cfg(not(windows))]
const LOCAL: &str = "/home/u/local";

fn record(owner: &str, agent: &str) -> HermesRecord {
    HermesRecord {
        origin: ORIGIN.into(),
        owner: owner.into(),
        agent_pubkey: agent.into(),
        secret_key: "00".repeat(32),
        device_id: None,
    }
}

fn remember(dir: &Path, record: HermesRecord) {
    update_hermes(dir, |records| records.push(record)).unwrap();
}

fn api_error(status: Option<u16>, code: Option<&str>) -> api::ApiError {
    api::ApiError {
        status,
        code: code.map(str::to_owned),
        message: "refused".into(),
    }
}

#[test]
fn hermes_is_found_by_hermes_home_first() {
    let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
    assert_eq!(hermes_home_from(&var, |_| true), Some(PathBuf::from(HOME)));
}

#[test]
fn hermes_is_found_in_local_app_data() {
    let var = vars(&[("LOCALAPPDATA", LOCAL)]);
    let expected = PathBuf::from(LOCAL).join("hermes");
    let found = hermes_home_from(&var, |path| path == expected.as_path());
    assert_eq!(found, Some(PathBuf::from(LOCAL).join("hermes")));
}

#[test]
fn hermes_home_that_does_not_exist_falls_back() {
    let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
    let local = PathBuf::from(LOCAL).join("hermes");
    let found = hermes_home_from(&var, |path| path == local.as_path());
    assert_eq!(found, Some(local));
}

#[test]
fn no_hermes_without_folders_or_with_relative_paths() {
    let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
    assert_eq!(hermes_home_from(&var, |_| false), None);
    assert_eq!(hermes_home_from(vars(&[]), |_| true), None);
    let relative = vars(&[("HERMES_HOME", "hermes"), ("LOCALAPPDATA", "")]);
    assert_eq!(hermes_home_from(relative, |_| true), None);
}

#[test]
fn hermes_flag_is_read_from_pair_help() {
    assert!(lists_hermes_flag(
        "Usage: kuzz pair <CODE>\n  --hermes  Install the Hermes plugin"
    ));
    assert!(lists_hermes_flag("Usage: kuzz pair [--hermes] <CODE>"));
    assert!(!lists_hermes_flag(
        "Usage: kuzz pair <CODE>\n  --hermes-home <DIR>"
    ));
    assert!(!lists_hermes_flag(""));
}

#[test]
fn claim_body_verifies_with_the_relay_check() {
    let dir = tempfile::tempdir().unwrap();
    let device =
        crate::device_key::DeviceKey::open_or_create_file(&dir.path().join("device.key")).unwrap();
    let agent = Keys::generate();
    let code = "AAAAAAAAAAAAAAAAAAAAAA";
    let body = claim_body(code, &agent, &device, Some("akak-PC")).unwrap();
    assert_eq!(body["code"], code);
    assert_eq!(body["device_name"], "akak-PC");
    assert_eq!(body["device_key_kind"], "software");
    let device_pubkey = kuzz_auth_pkg::device_proof::DevicePubkey::from_hex(
        body["device_pubkey"].as_str().unwrap(),
    )
    .unwrap();
    kuzz_auth_pkg::pairing::verify_pair_claim(
        code,
        &agent.public_key(),
        body["agent_signature"].as_str().unwrap(),
        &device_pubkey,
        body["device_signature"].as_str().unwrap(),
    )
    .expect("the relay accepts the claim signatures");
    // Another code does not verify with these signatures.
    assert!(kuzz_auth_pkg::pairing::verify_pair_claim(
        "BBBBBBBBBBBBBBBBBBBBBB",
        &agent.public_key(),
        body["agent_signature"].as_str().unwrap(),
        &device_pubkey,
        body["device_signature"].as_str().unwrap(),
    )
    .is_err());
    let without_name = claim_body(code, &agent, &device, None).unwrap();
    assert!(without_name.get("device_name").is_none());
}

#[test]
fn hermes_records_are_kept_per_account_and_connected_by_device() {
    let dir = tempfile::tempdir().unwrap();
    remember(dir.path(), record("o1", "a1"));
    remember(dir.path(), record("o2", "a2"));
    set_hermes_device_in(dir.path(), ORIGIN, "a1", "d1").unwrap();
    let mine = hermes_agents_in(dir.path(), ORIGIN, "o1").unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].agent_pubkey, "a1");
    assert_eq!(mine[0].device_id.as_deref(), Some("d1"));
    assert!(hermes_agents_in(dir.path(), "https://other.example", "o1")
        .unwrap()
        .is_empty());
    // The same account reuses its record instead of a new key.
    let reused = record_for(dir.path(), ORIGIN, "o1").unwrap().unwrap();
    assert_eq!(reused.agent_pubkey, "a1");
    assert!(record_for(dir.path(), ORIGIN, "o3").unwrap().is_none());
    forget_hermes_in(dir.path(), ORIGIN, "a2").unwrap();
    assert!(hermes_agents_in(dir.path(), ORIGIN, "o2")
        .unwrap()
        .is_empty());
    // The view never carries the key.
    let view = serde_json::to_string(&mine).unwrap();
    assert!(!view.contains(&"00".repeat(32)));
}

#[test]
fn a_missing_file_is_empty_but_a_damaged_one_stops_every_change() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_hermes(dir.path()).unwrap().is_empty());
    std::fs::write(dir.path().join(HERMES_FILE), b"{not json").unwrap();
    assert!(read_hermes(dir.path()).is_err());
    assert!(hermes_agents_in(dir.path(), ORIGIN, "o1").is_err());
    assert!(update_hermes(dir.path(), |records| records.push(record("o1", "a1"))).is_err());
    // The damaged file is left as it was, not replaced by an empty list.
    assert_eq!(
        std::fs::read(dir.path().join(HERMES_FILE)).unwrap(),
        b"{not json"
    );
}

#[test]
fn only_a_definite_refusal_forgets_the_key() {
    assert!(refused_for_good(&api_error(
        Some(400),
        Some("invalid_code")
    )));
    assert!(refused_for_good(&api_error(Some(409), Some("code_used"))));
    assert!(refused_for_good(&api_error(Some(403), None)));
    assert!(!refused_for_good(&api_error(Some(409), Some(KEY_IN_USE))));
    assert!(!refused_for_good(&api_error(Some(500), None)));
    assert!(!refused_for_good(&api_error(
        Some(503),
        Some("unavailable")
    )));
    assert!(!refused_for_good(&api_error(None, None)));
}

#[test]
fn hermes_keys_stay_only_while_their_account_is_signed_in() {
    let account = |pubkey: &str, origin: &str, state| Account {
        pubkey: pubkey.into(),
        origin: origin.into(),
        state,
    };
    let registry = Registry {
        accounts: vec![
            account("aa", ORIGIN, AccountState::SignedIn),
            account("bb", ORIGIN, AccountState::NeedsLogin),
            account("cc", ORIGIN, AccountState::SignedOut),
            account("dd", ANY_ORIGIN, AccountState::SignedIn),
            account("ee", ANY_ORIGIN, AccountState::KeyOnly),
        ],
        ..Registry::default()
    };
    assert!(still_signed_in(&registry, ORIGIN, "aa"));
    assert!(still_signed_in(&registry, ORIGIN, "AA"));
    assert!(still_signed_in(&registry, ORIGIN, "bb"));
    assert!(!still_signed_in(&registry, ORIGIN, "cc"));
    assert!(still_signed_in(&registry, ORIGIN, "dd"));
    assert!(!still_signed_in(&registry, ORIGIN, "ee"));
    // Removed from this computer.
    assert!(!still_signed_in(&registry, ORIGIN, "ff"));
    // Signed in to another relay only.
    assert!(!still_signed_in(&registry, "https://other.example", "aa"));
}

#[test]
fn signed_out_accounts_lose_their_hermes_keys() {
    let dir = tempfile::tempdir().unwrap();
    remember(dir.path(), record("aa", "a1"));
    remember(dir.path(), record("cc", "a2"));
    // No account list: nothing is removed.
    prune_signed_out(dir.path());
    assert_eq!(read_hermes(dir.path()).unwrap().len(), 2);
    let mut registry = Registry::default();
    registry.accounts = vec![
        Account {
            pubkey: "aa".into(),
            origin: ORIGIN.into(),
            state: AccountState::SignedIn,
        },
        Account {
            pubkey: "cc".into(),
            origin: ORIGIN.into(),
            state: AccountState::SignedOut,
        },
    ];
    let json = serde_json::to_vec(&registry).unwrap();
    std::fs::write(crate::auth::accounts::registry_path(dir.path()), json).unwrap();
    prune_signed_out(dir.path());
    let left = read_hermes(dir.path()).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].owner, "aa");
}

#[test]
fn a_lost_claim_is_settled_from_the_relay_pairings() {
    let dir = tempfile::tempdir().unwrap();
    remember(dir.path(), record("o1", "a1"));
    let pairings: Vec<api::PairingInfo> = serde_json::from_value(serde_json::json!([
        {"id": "p1", "state": "waiting", "created_at": "t", "expires_at": "t"},
        {"id": "p2", "state": "connected", "agent_pubkey": "a9", "device_id": "d9",
         "created_at": "t", "expires_at": "t"},
    ]))
    .unwrap();
    settle_lost_claim(dir.path(), ORIGIN, "o1", &pairings).unwrap();
    assert_eq!(
        record_for(dir.path(), ORIGIN, "o1")
            .unwrap()
            .unwrap()
            .device_id,
        None
    );
    let connected: Vec<api::PairingInfo> = serde_json::from_value(serde_json::json!([
        {"id": "p3", "state": "connected", "agent_pubkey": "A1", "device_id": "d1",
         "created_at": "t", "expires_at": "t"},
    ]))
    .unwrap();
    settle_lost_claim(dir.path(), ORIGIN, "o1", &connected).unwrap();
    assert_eq!(
        record_for(dir.path(), ORIGIN, "o1")
            .unwrap()
            .unwrap()
            .device_id
            .as_deref(),
        Some("d1")
    );
}
