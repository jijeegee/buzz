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
    commit_sign_in(&mem, dir.path(), ORIGIN, &first, "token-a", &first).unwrap();
    assert_eq!(mem.get("identity"), Some(nsec(&first)));

    // The own refresh slot fails: neither the list nor the active copy moves.
    let second = Keys::generate();
    mem.fail(&refresh_slot(&second.public_key(), ORIGIN));
    assert!(commit_sign_in(&mem, dir.path(), ORIGIN, &second, "token-b", &first).is_err());
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
    assert!(commit_sign_in(&mem, dir.path(), ORIGIN, &second, "token-b", &first).is_err());
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &keys, "token", &keys).unwrap();
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "token-a", &a).unwrap();
    commit_sign_in(&mem, dir.path(), ORIGIN, &b, "token-b", &a).unwrap();
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "token-a", &a).unwrap();
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &k, "t", &k).unwrap();
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &first, "token", &first).unwrap();
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "token-a", &a).unwrap();
    commit_sign_in(&mem, dir.path(), ORIGIN, &b, "token-b", &a).unwrap();
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
            &outgoing,
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &incoming, "t", &outgoing).unwrap();
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &incoming, "t", &bootstrap).unwrap();
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &a, "a1", &a).unwrap();
    commit_sign_in(&mem, dir.path(), other_origin, &a, "a2", &a).unwrap();
    // B signs in on ORIGIN only; A's copy on other_origin is left behind.
    commit_sign_in(&mem, dir.path(), ORIGIN, &b, "b1", &a).unwrap();
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
    commit_sign_in(&mem, dir.path(), ORIGIN, &outgoing, "t", &outgoing).unwrap();
    for _ in 0..30 {
        let next = Keys::generate();
        commit_sign_in(&mem, dir.path(), ORIGIN, &next, "t", &outgoing).unwrap();
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
