use super::*;
use std::cell::RefCell;
use std::collections::HashSet;

const ORIGIN: &str = "https://relay.test";

/// In-memory secure storage with injectable write failures.
#[derive(Default)]
struct Mem {
    map: RefCell<HashMap<String, String>>,
    fail_keys: RefCell<HashSet<String>>,
    fail_store_all: RefCell<bool>,
}

impl Mem {
    fn get(&self, key: &str) -> Option<String> {
        self.map.borrow().get(key).cloned()
    }
    fn put(&self, key: &str, value: &str) {
        self.map.borrow_mut().insert(key.into(), value.into());
    }
    fn fail(&self, key: &str) {
        self.fail_keys.borrow_mut().insert(key.into());
    }
}

impl Secrets for Mem {
    fn load(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.get(key))
    }
    fn store(&self, key: &str, value: &str) -> Result<(), String> {
        if self.fail_keys.borrow().contains(key) {
            return Err("injected write failure".into());
        }
        self.put(key, value);
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.map.borrow_mut().remove(key);
        Ok(())
    }
    fn store_all(&self, entries: &HashMap<String, String>) -> Result<(), String> {
        if *self.fail_store_all.borrow() {
            return Err("injected write failure".into());
        }
        for (k, v) in entries {
            self.put(k, v);
        }
        Ok(())
    }
    fn blob_keys(&self) -> Result<Vec<String>, String> {
        Ok(self.map.borrow().keys().cloned().collect())
    }
}

fn nsec(keys: &Keys) -> String {
    keys.secret_key().to_bech32().unwrap()
}

/// An older build's single-account slots for `keys`, signed in on ORIGIN.
fn legacy_signed_in(mem: &Mem, keys: &Keys, token: &str) {
    mem.put("identity", &nsec(keys));
    mem.put("identity.bootstrap", "");
    mem.put(
        &super::super::key_recovery::binding_key(ORIGIN),
        &keys.public_key().to_hex(),
    );
    mem.put(&super::super::refresh_key(ORIGIN), token);
}

#[test]
fn an_established_key_is_never_the_unused_first_install_key() {
    let key = Keys::generate().public_key();
    let hex = key.to_hex();
    // Fresh install: marker names the key, nothing registered, no agents.
    assert!(is_unused_bootstrap_key(Some(&hex), &key, 0, 0));
    // akak-like account: marker cleared on first publication.
    assert!(!is_unused_bootstrap_key(Some(""), &key, 0, 0));
    assert!(!is_unused_bootstrap_key(None, &key, 0, 0));
    // Registered accounts or agents make it established.
    assert!(!is_unused_bootstrap_key(Some(&hex), &key, 1, 0));
    assert!(!is_unused_bootstrap_key(Some(&hex), &key, 0, 3));
    // A marker naming another key does not count.
    let other = Keys::generate().public_key();
    assert!(!is_unused_bootstrap_key(Some(&hex), &other, 0, 0));
}

#[test]
fn sign_in_writes_the_accounts_own_slots_before_the_active_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let first = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &first, "token-a", Some(&first)).unwrap();
    assert_eq!(mem.get("identity"), Some(nsec(&first)));

    // The own refresh slot fails: neither the list nor the active copy moves.
    let second = Keys::generate();
    mem.fail(&refresh_slot(&second.public_key(), ORIGIN));
    assert!(commit_sign_in(&mem, dir.path(), ORIGIN, &second, "token-b", Some(&first)).is_err());
    assert_eq!(mem.get("identity"), Some(nsec(&first)));
    assert_eq!(
        mem.get(&super::super::refresh_key(ORIGIN)).as_deref(),
        Some("token-a")
    );
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(registry.active_pubkey(), Some(first.public_key()));

    // The active copy fails after the own slots: the account is complete in
    // its own slots and the next launch refills the copy from them.
    mem.fail_keys.borrow_mut().clear();
    *mem.fail_store_all.borrow_mut() = true;
    assert!(commit_sign_in(&mem, dir.path(), ORIGIN, &second, "token-b", Some(&first)).is_err());
    assert_eq!(
        mem.get(&refresh_slot(&second.public_key(), ORIGIN))
            .as_deref(),
        Some("token-b")
    );
    assert_eq!(mem.get("identity"), Some(nsec(&first)));
    *mem.fail_store_all.borrow_mut() = false;
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Refilled);
    assert_eq!(mem.get("identity"), Some(nsec(&second)));
    assert_eq!(
        mem.get(&super::super::refresh_key(ORIGIN)).as_deref(),
        Some("token-b")
    );
    // Both accounts are still on this computer.
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert!(registry.contains_pubkey(&first.public_key()));
    assert_eq!(
        registry.inactive_signed_in(),
        vec![(ORIGIN.to_owned(), first.public_key())]
    );
}

#[test]
fn a_rotated_token_reaches_the_active_copy_only_after_its_own_slot() {
    let mem = Mem::default();
    let account = Keys::generate().public_key();
    store_active_refresh(&mem, ORIGIN, Some(account), "rotated").unwrap();
    assert_eq!(
        mem.get(&refresh_slot(&account, ORIGIN)).as_deref(),
        Some("rotated")
    );
    assert_eq!(
        mem.get(&super::super::refresh_key(ORIGIN)).as_deref(),
        Some("rotated")
    );
    mem.fail(&refresh_slot(&account, ORIGIN));
    assert!(store_active_refresh(&mem, ORIGIN, Some(account), "next").is_err());
    assert_eq!(
        mem.get(&super::super::refresh_key(ORIGIN)).as_deref(),
        Some("rotated"),
        "the active copy must not run ahead of the account's own slot"
    );
}

#[test]
fn an_older_single_account_install_is_migrated_and_kept_readable() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let keys = Keys::generate();
    legacy_signed_in(&mem, &keys, "token-a");
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Migrated);
    let pubkey = keys.public_key();
    assert_eq!(mem.get(&identity_slot(&pubkey)), Some(nsec(&keys)));
    assert_eq!(
        mem.get(&refresh_slot(&pubkey, ORIGIN)).as_deref(),
        Some("token-a")
    );
    // Old slots stay for older builds.
    assert_eq!(mem.get("identity"), Some(nsec(&keys)));
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(registry.active_pubkey(), Some(pubkey));
    assert_eq!(
        registry.get(ORIGIN, &pubkey).map(|a| a.state),
        Some(AccountState::SignedIn)
    );
    // Second launch: nothing to do.
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Loaded);
}

#[test]
fn a_bound_account_without_a_token_is_signed_out_only_by_its_marker() {
    // An older build dropped the token when the session needed a fresh
    // sign-in: still an account here, so its agents start (§10.8). `marker`
    // gives the marker's identity JSON for the migrated key, if any.
    let migrated = |marker: &dyn Fn(&PublicKey) -> Option<String>| {
        let dir = tempfile::tempdir().unwrap();
        let mem = Mem::default();
        let keys = Keys::generate();
        legacy_signed_in(&mem, &keys, "unused");
        mem.delete(&super::super::refresh_key(ORIGIN)).unwrap();
        if let Some(identity) = marker(&keys.public_key()) {
            std::fs::write(
                dir.path().join(super::super::signed_out::FILE_NAME),
                format!(
                    r#"{{"version":1,"origins":[{{"origin":"{ORIGIN}","identity":{identity}}}]}}"#
                ),
            )
            .unwrap();
        }
        assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Migrated);
        load_registry(dir.path())
            .unwrap()
            .unwrap()
            .get(ORIGIN, &keys.public_key())
            .map(|a| a.state)
    };
    assert_eq!(migrated(&|_| None), Some(AccountState::NeedsLogin));
    assert_eq!(
        migrated(&|key| Some(format!("\"{}\"", key.to_hex()))),
        Some(AccountState::SignedOut)
    );
    // A whole-origin (token account) marker covers it too.
    assert_eq!(
        migrated(&|_| Some("null".into())),
        Some(AccountState::SignedOut)
    );
    // Another account's marker does not.
    let other = Keys::generate().public_key().to_hex();
    assert_eq!(
        migrated(&|_| Some(format!("\"{other}\""))),
        Some(AccountState::NeedsLogin)
    );
}

#[test]
fn an_unused_first_install_key_is_not_an_account() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let keys = Keys::generate();
    mem.put("identity", &nsec(&keys));
    mem.put("identity.bootstrap", &keys.public_key().to_hex());
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Empty);
    assert!(load_registry(dir.path()).unwrap().is_none());
}

#[test]
fn a_used_key_without_google_is_kept_as_a_key_only_account() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let keys = Keys::generate();
    mem.put("identity", &nsec(&keys));
    mem.put("identity.bootstrap", "");
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Migrated);
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(
        registry
            .get(ANY_ORIGIN, &keys.public_key())
            .map(|a| a.state),
        Some(AccountState::KeyOnly)
    );
    // Its first Google sign-in replaces the key-only placeholder.
    commit_sign_in(&mem, dir.path(), ORIGIN, &keys, "token", Some(&keys)).unwrap();
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(registry.accounts.len(), 1);
    assert_eq!(registry.accounts[0].origin, ORIGIN);
}

#[test]
fn a_lost_account_list_is_rebuilt_from_the_secure_index() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let a = Keys::generate();
    let b = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "token-a", Some(&a)).unwrap();
    commit_sign_in(&mem, dir.path(), ORIGIN, &b, "token-b", Some(&a)).unwrap();
    std::fs::remove_file(registry_path(dir.path())).unwrap();
    assert_eq!(
        reconcile(&mem, dir.path()).unwrap(),
        Reconciled::RebuiltFromIndex
    );
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(registry.active_pubkey(), Some(b.public_key()));
    assert!(registry.contains_pubkey(&a.public_key()));
    assert_eq!(
        registry.inactive_signed_in(),
        vec![(ORIGIN.to_owned(), a.public_key())]
    );
}

#[test]
fn an_identity_imported_outside_the_list_is_adopted_without_dropping_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let a = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "token-a", Some(&a)).unwrap();
    let imported = Keys::generate();
    mem.put("identity", &nsec(&imported));
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Adopted);
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(registry.active_pubkey(), Some(imported.public_key()));
    assert!(registry.contains_pubkey(&a.public_key()));
    assert_eq!(
        mem.get(&identity_slot(&imported.public_key())),
        Some(nsec(&imported))
    );
}

#[test]
fn a_list_left_behind_by_a_reset_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let k = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &k, "t", Some(&k)).unwrap();
    mem.map.borrow_mut().clear();
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Empty);
    assert!(load_registry(dir.path()).unwrap().is_none());
}

#[test]
fn a_new_accounts_key_is_reused_until_its_upload_is_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let first = pending_new_key(&mem, dir.path(), ORIGIN, "account-x").unwrap();
    let retry = pending_new_key(&mem, dir.path(), ORIGIN, "account-x").unwrap();
    assert_eq!(first.public_key(), retry.public_key());
    let other = pending_new_key(&mem, dir.path(), ORIGIN, "account-y").unwrap();
    assert_ne!(first.public_key(), other.public_key());
    // The pending key's slot is listed for reset.
    let index = read_index(&mem).unwrap().unwrap();
    assert!(index.contains(&first.public_key().to_hex()));
    // Committing the sign-in clears it.
    commit_sign_in(&mem, dir.path(), ORIGIN, &first, "token", Some(&first)).unwrap();
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert!(registry
        .pending
        .iter()
        .all(|p| p.pubkey != first.public_key().to_hex()));
}

#[test]
fn signing_out_keeps_the_account_listed_without_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let a = Keys::generate();
    let b = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "token-a", Some(&a)).unwrap();
    commit_sign_in(&mem, dir.path(), ORIGIN, &b, "token-b", Some(&a)).unwrap();
    mark_signed_out(&mem, dir.path(), ORIGIN, &a.public_key()).unwrap();
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(
        registry.get(ORIGIN, &a.public_key()).map(|x| x.state),
        Some(AccountState::SignedOut)
    );
    assert!(registry.inactive_signed_in().is_empty());
    assert_eq!(mem.get(&refresh_slot(&a.public_key(), ORIGIN)), None);
    // Its key stays (agents it owns keep their attestation; it can sign in again).
    assert!(mem.get(&identity_slot(&a.public_key())).is_some());
}

#[test]
fn the_index_round_trips() {
    let mut registry = Registry::default();
    let a = Keys::generate().public_key();
    let b = Keys::generate().public_key();
    registry.upsert(ORIGIN, &a, AccountState::SignedIn);
    registry.upsert(ORIGIN, &b, AccountState::SignedOut);
    let c = Keys::generate().public_key();
    registry.upsert(ANY_ORIGIN, &c, AccountState::KeyOnly);
    registry.active = Some(b.to_hex());
    let rebuilt = Registry::from_index(&registry.index_text()).unwrap();
    assert_eq!(rebuilt.active, registry.active);
    let mut left = rebuilt.accounts.clone();
    let mut right = registry.accounts.clone();
    left.sort_by(|x, y| (&x.pubkey, &x.origin).cmp(&(&y.pubkey, &y.origin)));
    right.sort_by(|x, y| (&x.pubkey, &x.origin).cmp(&(&y.pubkey, &y.origin)));
    assert_eq!(left, right);
}

/// A Preview-shaped Windows keyring (identity, binding, six agent keys in
/// the blob, the refresh token in its own credential), with generated values
/// under a test-only service: migration fills the account's own slots, keeps
/// the old slots readable, and adding accounts never grows the blob.
#[cfg(all(windows, feature = "system-keyring"))]
#[ignore = "requires real OS keychain (run locally)"]
#[test]
fn preview_shaped_windows_keyring_migrates_without_growing_the_blob() {
    let service = "buzz-test-accounts-preview-shape";
    let store = crate::secret_store::SecretStore::keyring(service);
    let _ = store.delete_all_with_legacy_cleanup();
    let origin = "https://jihobuzz.test:8443";
    let owner = Keys::generate();
    let mut entries = HashMap::from([
        ("identity".to_owned(), nsec(&owner)),
        ("identity.bootstrap".to_owned(), String::new()),
        (
            super::super::key_recovery::binding_key(origin),
            owner.public_key().to_hex(),
        ),
    ]);
    for _ in 0..6 {
        let agent = Keys::generate();
        entries.insert(
            format!("agent:{}", agent.public_key().to_hex()),
            nsec(&agent),
        );
    }
    store.store_all(&entries).unwrap();
    store
        .store(&super::super::refresh_key(origin), &"r".repeat(48))
        .unwrap();
    let blob_units = |store: &crate::secret_store::SecretStore| -> usize {
        let map = store.load_all_readonly().unwrap().unwrap_or_default();
        serde_json::to_string(&map)
            .unwrap()
            .chars()
            .map(char::len_utf16)
            .sum()
    };
    let before = blob_units(&store);

    let dir = tempfile::tempdir().unwrap();
    assert_eq!(reconcile(&store, dir.path()).unwrap(), Reconciled::Migrated);
    let pubkey = owner.public_key();
    assert_eq!(
        Secrets::load(&store, &identity_slot(&pubkey)).unwrap(),
        Some(nsec(&owner))
    );
    assert_eq!(
        Secrets::load(&store, &refresh_slot(&pubkey, origin))
            .unwrap()
            .as_deref(),
        Some("r".repeat(48).as_str())
    );
    assert_eq!(
        Secrets::load(&store, "identity").unwrap(),
        Some(nsec(&owner)),
        "old slot kept for older builds"
    );

    // Three more accounts signed in: per-account slots stay out of the blob.
    let mut outgoing = owner.clone();
    for i in 0..3 {
        let next = Keys::generate();
        commit_sign_in(
            &store,
            dir.path(),
            origin,
            &next,
            &format!("t{i}"),
            Some(&outgoing),
        )
        .unwrap();
        outgoing = next;
    }
    let after = blob_units(&store);
    assert!(
        after <= before + 70,
        "blob grew from {before} to {after} UTF-16 units"
    );
    assert!(after <= 1280);
    // A fresh process rebuilds the list from the secure index.
    std::fs::remove_file(registry_path(dir.path())).unwrap();
    let reopened = crate::secret_store::SecretStore::keyring(service);
    assert_eq!(
        reconcile(&reopened, dir.path()).unwrap(),
        Reconciled::RebuiltFromIndex
    );
    assert_eq!(
        load_registry(dir.path()).unwrap().unwrap().accounts.len(),
        4
    );
    reopened.delete_all_with_legacy_cleanup().unwrap();
    assert_eq!(
        crate::secret_store::SecretStore::keyring(service)
            .load(&identity_slot(&pubkey))
            .unwrap(),
        None,
        "reset deletes per-account slots listed in the index"
    );
}

#[test]
fn signing_in_keeps_an_unlisted_outgoing_key_as_its_own_account() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    // An established key nobody listed (failed launch reconcile, key import).
    let outgoing = Keys::generate();
    mem.put("identity", &nsec(&outgoing));
    mem.put("identity.bootstrap", "");
    let incoming = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &incoming, "t", Some(&outgoing)).unwrap();
    assert_eq!(
        mem.get(&identity_slot(&outgoing.public_key())),
        Some(nsec(&outgoing))
    );
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(
        registry
            .get(ANY_ORIGIN, &outgoing.public_key())
            .map(|a| a.state),
        Some(AccountState::KeyOnly)
    );
    assert_eq!(registry.active_pubkey(), Some(incoming.public_key()));
}

#[test]
fn an_unused_first_install_key_is_not_kept_when_signing_in() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let bootstrap = Keys::generate();
    mem.put("identity", &nsec(&bootstrap));
    mem.put("identity.bootstrap", &bootstrap.public_key().to_hex());
    let incoming = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &incoming, "t", Some(&bootstrap)).unwrap();
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert!(!registry.contains_pubkey(&bootstrap.public_key()));
    assert_eq!(mem.get(&identity_slot(&bootstrap.public_key())), None);
}

#[test]
fn a_first_install_key_that_owns_agents_is_an_account() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let keys = Keys::generate();
    mem.put("identity", &nsec(&keys));
    mem.put("identity.bootstrap", &keys.public_key().to_hex());
    std::fs::create_dir_all(dir.path().join("agents")).unwrap();
    std::fs::write(
        dir.path().join("agents").join("managed-agents.json"),
        format!(
            r#"[{{"pubkey":"{}"}}]"#,
            Keys::generate().public_key().to_hex()
        ),
    )
    .unwrap();
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Migrated);
    assert!(load_registry(dir.path())
        .unwrap()
        .unwrap()
        .contains_pubkey(&keys.public_key()));
}

#[test]
fn launch_hands_every_origin_to_the_active_account() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let other_origin = "https://other.test";
    let a = Keys::generate();
    let b = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "a1", Some(&a)).unwrap();
    commit_sign_in(&mem, dir.path(), other_origin, &a, "a2", Some(&a)).unwrap();
    // B signs in on ORIGIN only; A's copy on other_origin is left behind.
    commit_sign_in(&mem, dir.path(), ORIGIN, &b, "b1", Some(&a)).unwrap();
    assert_eq!(
        mem.get(&super::super::refresh_key(other_origin)).as_deref(),
        Some("a2")
    );
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Refilled);
    // A's token is no longer in the active copy (only in A's own slot), so it
    // can never be refreshed twice.
    assert_eq!(mem.get(&super::super::refresh_key(other_origin)), None);
    assert_eq!(
        mem.get(&super::super::key_recovery::binding_key(other_origin)),
        None
    );
    assert_eq!(
        mem.get(&refresh_slot(&a.public_key(), other_origin))
            .as_deref(),
        Some("a2")
    );
    assert_eq!(
        mem.get(&super::super::refresh_key(ORIGIN)).as_deref(),
        Some("b1")
    );
}

#[test]
fn a_large_account_list_is_split_across_index_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let mut outgoing = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &outgoing, "t", Some(&outgoing)).unwrap();
    for _ in 0..30 {
        let next = Keys::generate();
        commit_sign_in(&mem, dir.path(), ORIGIN, &next, "t", Some(&outgoing)).unwrap();
        outgoing = next;
    }
    assert!(mem.get(&index_chunk_key(1)).is_some());
    for n in 0..MAX_INDEX_CHUNKS {
        if let Some(chunk) = mem.get(&index_chunk_key(n)) {
            assert!(chunk.chars().count() <= INDEX_CHUNK_CHARS);
        }
    }
    std::fs::remove_file(registry_path(dir.path())).unwrap();
    assert_eq!(
        reconcile(&mem, dir.path()).unwrap(),
        Reconciled::RebuiltFromIndex
    );
    assert_eq!(
        load_registry(dir.path()).unwrap().unwrap().accounts.len(),
        31
    );
}

#[test]
fn recovering_a_lost_key_neither_keeps_the_stand_in_nor_records_it_as_owner() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    // An older agent without an attestation, and no account list yet.
    let agent = Keys::generate().public_key().to_hex();
    std::fs::create_dir_all(dir.path().join("agents")).unwrap();
    std::fs::write(
        dir.path().join("agents").join("managed-agents.json"),
        format!(
            r#"[{{"pubkey":"{agent}","name":"a","relay_url":"wss://r","acp_command":"x","agent_command":"x","agent_args":[],"mcp_command":"x","turn_timeout_seconds":1,"created_at":"t","updated_at":"t"}}]"#
        ),
    )
    .unwrap();
    let restored = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &restored, "t", None).unwrap();
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(
        registry.accounts.len(),
        1,
        "only the recovered account is listed"
    );
    let owners: std::collections::BTreeMap<String, String> = serde_json::from_slice(
        &std::fs::read(dir.path().join("agents").join("agent-owners.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(owners.get(&agent), Some(&restored.public_key().to_hex()));
}

#[test]
fn a_failed_copy_write_keeps_the_rotated_token_in_its_own_slot() {
    let mem = Mem::default();
    let account = Keys::generate().public_key();
    mem.fail(&super::super::refresh_key(ORIGIN));
    store_active_refresh(&mem, ORIGIN, Some(account), "rotated").unwrap();
    assert_eq!(
        mem.get(&refresh_slot(&account, ORIGIN)).as_deref(),
        Some("rotated")
    );
    assert_eq!(mem.get(&super::super::refresh_key(ORIGIN)), None);
}

#[test]
fn a_background_session_that_ends_is_listed_as_needing_sign_in() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let a = Keys::generate();
    let b = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "ta", Some(&a)).unwrap();
    commit_sign_in(&mem, dir.path(), ORIGIN, &b, "tb", Some(&a)).unwrap();
    mark_needs_login(&mem, dir.path(), ORIGIN, &a.public_key()).unwrap();
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(
        registry.get(ORIGIN, &a.public_key()).map(|x| x.state),
        Some(AccountState::NeedsLogin)
    );
    assert!(
        registry.inactive_signed_in().is_empty(),
        "no longer refreshed"
    );
    // It survives a lost list.
    std::fs::remove_file(registry_path(dir.path())).unwrap();
    reconcile(&mem, dir.path()).unwrap();
    let rebuilt = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(
        rebuilt.get(ORIGIN, &a.public_key()).map(|x| x.state),
        Some(AccountState::NeedsLogin)
    );
    // Signing in again restores it.
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "ta2", Some(&b)).unwrap();
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(
        registry.get(ORIGIN, &a.public_key()).map(|x| x.state),
        Some(AccountState::SignedIn)
    );
}

#[test]
fn handing_an_origin_over_never_leaves_a_token_under_the_wrong_binding() {
    let mem = Mem::default();
    let previous = Keys::generate().public_key();
    let current = Keys::generate().public_key();
    mem.put(
        &super::super::key_recovery::binding_key(ORIGIN),
        &previous.to_hex(),
    );
    mem.put(&super::super::refresh_key(ORIGIN), "previous-token");
    mem.put(&refresh_slot(&current, ORIGIN), "current-token");
    // The token copy fails: the binding moved, but no token is left that a
    // restore could refresh under it.
    mem.fail(&super::super::refresh_key(ORIGIN));
    assert!(hand_over_copy(&mem, ORIGIN, &current, Some(AccountState::SignedIn)).is_err());
    assert_eq!(mem.get(&super::super::refresh_key(ORIGIN)), None);
    // A clean hand-over copies the current account's own token.
    mem.fail_keys.borrow_mut().clear();
    assert!(hand_over_copy(&mem, ORIGIN, &current, Some(AccountState::SignedIn)).unwrap());
    assert_eq!(
        mem.get(&super::super::refresh_key(ORIGIN)).as_deref(),
        Some("current-token")
    );
    assert_eq!(
        mem.get(&super::super::key_recovery::binding_key(ORIGIN)),
        Some(current.to_hex())
    );
    // An account bound nowhere here clears the binding.
    assert!(!hand_over_copy(&mem, ORIGIN, &previous, Some(AccountState::KeyOnly)).unwrap());
    assert_eq!(
        mem.get(&super::super::key_recovery::binding_key(ORIGIN)),
        None
    );
}

#[test]
fn a_kept_outgoing_key_is_listed_before_its_slot_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let outgoing = Keys::generate();
    mem.put("identity.bootstrap", "");
    // Its slot cannot be written: the list already names it, so a reset
    // still finds anything a retry writes.
    mem.fail(&identity_slot(&outgoing.public_key()));
    let incoming = Keys::generate();
    assert!(commit_sign_in(&mem, dir.path(), ORIGIN, &incoming, "t", Some(&outgoing)).is_err());
    let index = read_index(&mem).unwrap().unwrap();
    assert!(index.contains(&outgoing.public_key().to_hex()));
}

/// Thread-safe in-memory storage for the async hand-over, with one failing key.
#[cfg(not(target_os = "windows"))]
#[derive(Default)]
struct SyncMem {
    map: std::sync::Mutex<HashMap<String, String>>,
    fail_key: String,
}

#[cfg(not(target_os = "windows"))]
impl Secrets for SyncMem {
    fn load(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.map.lock().unwrap().get(key).cloned())
    }
    fn store(&self, key: &str, value: &str) -> Result<(), String> {
        if key == self.fail_key {
            return Err("injected write failure".into());
        }
        self.map.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
    fn store_all(&self, entries: &HashMap<String, String>) -> Result<(), String> {
        self.map.lock().unwrap().extend(entries.clone());
        Ok(())
    }
    fn blob_keys(&self) -> Result<Vec<String>, String> {
        Ok(self.map.lock().unwrap().keys().cloned().collect())
    }
}

// `build_app_state()` in a mock app is gated off Windows CI like the
// admission tests.
#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn hand_over_goes_on_past_a_failing_server_and_reports_it() {
    use tauri::Manager;
    #[cfg(feature = "system-keyring")]
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    let data = tempfile::tempdir().unwrap();
    let mut context = tauri::test::mock_context(tauri::test::noop_assets());
    context.config_mut().identifier = data.path().to_str().unwrap().to_owned();
    let app = tauri::test::mock_builder()
        .manage(crate::app_state::build_app_state())
        .build(context)
        .unwrap();
    let data_dir = app.path().app_data_dir().unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();

    const FAILING: &str = "https://a.relay.test";
    const LATER: &str = "https://b.relay.test";
    let current = Keys::generate().public_key();
    let previous = Keys::generate().public_key();
    let secrets = SyncMem {
        fail_key: super::super::key_recovery::binding_key(FAILING),
        ..SyncMem::default()
    };
    let mut registry = Registry::default();
    for origin in [FAILING, LATER] {
        registry.accounts.push(Account {
            pubkey: current.to_hex(),
            origin: origin.into(),
            state: AccountState::SignedIn,
        });
    }
    registry.active = Some(current.to_hex());
    save_registry(&secrets, &data_dir, &registry).unwrap();

    let error = hand_over_origins_with(app.handle(), &secrets, previous, current, None, None)
        .await
        .unwrap_err();

    // The failing server (sorted first) is reported, and the one after it
    // was still handed over to `current`.
    assert!(error.contains(FAILING), "{error}");
    assert!(!error.contains(LATER), "{error}");
    assert_eq!(
        secrets
            .load(&super::super::key_recovery::binding_key(LATER))
            .unwrap(),
        Some(current.to_hex())
    );
    let state = app.state::<crate::app_state::AppState>();
    assert_eq!(state.token_auth.signing_pubkey(LATER), Some(current));
    assert_eq!(state.token_auth.signing_pubkey(FAILING), None);
}

#[test]
fn a_failed_sign_in_commit_keeps_its_pending_new_key() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let first = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &first, "token-a", Some(&first)).unwrap();
    let new = pending_new_key(&mem, dir.path(), ORIGIN, "google:new").unwrap();
    let before = sign_in_before(&mem, dir.path(), ORIGIN, &new.public_key()).unwrap();
    *mem.fail_store_all.borrow_mut() = true;
    assert!(commit_sign_in(&mem, dir.path(), ORIGIN, &new, "token-n", Some(&first)).is_err());
    *mem.fail_store_all.borrow_mut() = false;
    roll_back_sign_in(
        &mem,
        dir.path(),
        ORIGIN,
        &new.public_key(),
        "token-n",
        &before,
    )
    .unwrap();
    // A retry gets the same key back, not a new one.
    let again = pending_new_key(&mem, dir.path(), ORIGIN, "google:new").unwrap();
    assert_eq!(again.public_key(), new.public_key());
}

#[test]
fn a_failed_sign_in_commit_is_rolled_back_to_the_previous_account() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Mem::default();
    let first = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &first, "token-a", Some(&first)).unwrap();

    // An account already signed in here, in the background, signs in again
    // and the active copy fails after its own slots were written.
    let second = Keys::generate();
    commit_sign_in(&mem, dir.path(), ORIGIN, &second, "token-b", Some(&first)).unwrap();
    commit_sign_in(&mem, dir.path(), ORIGIN, &first, "token-a2", Some(&second)).unwrap();
    let before = sign_in_before(&mem, dir.path(), ORIGIN, &second.public_key()).unwrap();
    assert_eq!(before.old_token(), Some("token-b"));
    *mem.fail_store_all.borrow_mut() = true;
    assert!(commit_sign_in(&mem, dir.path(), ORIGIN, &second, "token-b2", Some(&first)).is_err());
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(registry.active_pubkey(), Some(second.public_key()));

    let restored = roll_back_sign_in(
        &mem,
        dir.path(),
        ORIGIN,
        &second.public_key(),
        "token-b2",
        &before,
    )
    .unwrap();
    assert!(restored);
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(registry.active_pubkey(), Some(first.public_key()));
    assert_eq!(
        registry.get(ORIGIN, &second.public_key()).map(|a| a.state),
        Some(AccountState::SignedIn)
    );
    assert_eq!(
        mem.get(&refresh_slot(&second.public_key(), ORIGIN))
            .as_deref(),
        Some("token-b")
    );

    // A new account with no session before: its revoked new token is not
    // left behind as a signed-in session.
    let third = Keys::generate();
    let before = sign_in_before(&mem, dir.path(), ORIGIN, &third.public_key()).unwrap();
    assert!(commit_sign_in(&mem, dir.path(), ORIGIN, &third, "token-c", Some(&first)).is_err());
    assert!(!roll_back_sign_in(
        &mem,
        dir.path(),
        ORIGIN,
        &third.public_key(),
        "token-c",
        &before,
    )
    .unwrap());
    let registry = load_registry(dir.path()).unwrap().unwrap();
    assert_eq!(registry.active_pubkey(), Some(first.public_key()));
    assert_eq!(
        registry.get(ORIGIN, &third.public_key()).map(|a| a.state),
        Some(AccountState::SignedOut)
    );
    assert_eq!(mem.get(&refresh_slot(&third.public_key(), ORIGIN)), None);

    // The copy was written but its check failed (here: written by hand after
    // the rollback): the next launch puts the list's active account back.
    *mem.fail_store_all.borrow_mut() = false;
    mem.put("identity", &nsec(&third));
    assert_eq!(reconcile(&mem, dir.path()).unwrap(), Reconciled::Refilled);
    assert_eq!(mem.get("identity"), Some(nsec(&first)));
    assert_eq!(
        mem.get(&super::super::refresh_key(ORIGIN)).as_deref(),
        Some("token-a2")
    );
}

#[test]
fn an_agent_owner_counts_as_signed_out_only_after_an_explicit_sign_out() {
    let owner = Keys::generate().public_key();
    let listed = |state| {
        let mut registry = Registry::default();
        registry.upsert(ORIGIN, &owner, state);
        registry
    };
    use OwnerHere::*;
    for (state, expected) in [
        (AccountState::SignedIn, SignedIn),
        // Its session needs a fresh sign-in (offline, expired): still signed
        // in here, so an offline launch starts its agents.
        (AccountState::NeedsLogin, SignedIn),
        (AccountState::SignedOut, SignedOut),
    ] {
        assert_eq!(
            owner_here(Some(&listed(state)), ORIGIN, &owner, false, None),
            expected
        );
    }
    // A key-only account is listed for every server ("*").
    let mut key_only = Registry::default();
    key_only.upsert(ANY_ORIGIN, &owner, AccountState::KeyOnly);
    assert_eq!(
        owner_here(Some(&key_only), ORIGIN, &owner, false, None),
        SignedIn
    );
    // Another server, or not listed at all.
    assert_eq!(
        owner_here(
            Some(&listed(AccountState::SignedIn)),
            "https://other.test",
            &owner,
            false,
            None
        ),
        Unlisted
    );
    assert_eq!(
        owner_here(Some(&Registry::default()), ORIGIN, &owner, false, None),
        Unlisted
    );
    // The token account's principal (server bots).
    assert_eq!(
        owner_here(
            Some(&Registry::default()),
            ORIGIN,
            &owner,
            false,
            Some(owner)
        ),
        SignedIn
    );
    // The sign-out marker wins over everything.
    assert_eq!(
        owner_here(
            Some(&listed(AccountState::SignedIn)),
            ORIGIN,
            &owner,
            true,
            Some(owner)
        ),
        SignedOut
    );
    // No account list yet (first key-mode launch) or an unreadable one: only
    // the marker decides.
    assert_eq!(owner_here(None, ORIGIN, &owner, false, None), SignedIn);
    assert_eq!(owner_here(None, ORIGIN, &owner, true, None), SignedOut);
}

#[test]
fn signing_out_the_active_account_switches_to_another_signed_in_one() {
    let (a, b, c) = (
        Keys::generate().public_key(),
        Keys::generate().public_key(),
        Keys::generate().public_key(),
    );
    let mut registry = Registry::default();
    registry.upsert(ORIGIN, &a, AccountState::SignedIn);
    registry.upsert(ORIGIN, &b, AccountState::SignedOut);
    registry.upsert("https://other.test", &c, AccountState::SignedIn);
    // B is signed out and C is on another server: nothing to switch to.
    assert_eq!(next_signed_in_of(&registry, ORIGIN, Some(a)), None);
    registry.upsert(ORIGIN, &c, AccountState::SignedIn);
    assert_eq!(next_signed_in_of(&registry, ORIGIN, Some(a)), Some(c));
    assert_eq!(next_signed_in_of(&registry, ORIGIN, Some(c)), Some(a));
}
