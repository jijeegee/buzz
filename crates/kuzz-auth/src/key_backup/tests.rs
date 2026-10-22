use super::*;
use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

fn master() -> BackupMasterKey {
    BackupMasterKey::from_hex("test-v1", &"19".repeat(32)).unwrap()
}

#[test]
fn key_backup_round_trip_restores_same_signing_identity_with_random_nonces() {
    let key = Keys::generate();
    let account = [7; 32];
    let first = master()
        .encrypt(&account, &key.secret_key().to_secret_hex())
        .unwrap();
    let second = master()
        .encrypt(&account, &key.secret_key().to_secret_hex())
        .unwrap();
    assert_ne!(first.nonce, second.nonce);
    assert_ne!(first.ciphertext, second.ciphertext);
    assert_eq!(first.pubkey, key.public_key().to_bytes());
    let secret = master().decrypt(&account, &first).unwrap();
    let recovered = Keys::parse(secret.as_str()).unwrap();
    assert_eq!(recovered.public_key(), key.public_key());
    let message = EventBuilder::text_note("same identity")
        .sign_with_keys(&recovered)
        .unwrap();
    message.verify().unwrap();
    assert_eq!(message.pubkey, key.public_key());
}

#[test]
fn key_backup_corruption_wrong_key_and_every_aad_component_fail_closed() {
    let key = Keys::generate();
    let account = [7; 32];
    let original = master()
        .encrypt(&account, &key.secret_key().to_secret_hex())
        .unwrap();
    assert!(master().decrypt(&[8; 32], &original).is_err());
    let wrong = BackupMasterKey::from_hex("test-v1", &"29".repeat(32)).unwrap();
    assert!(wrong.decrypt(&account, &original).is_err());
    for field in 0..6 {
        let mut envelope = original.clone();
        match field {
            0 => envelope.ciphertext[0] ^= 1,
            1 => envelope.nonce[0] ^= 1,
            2 => envelope.pubkey = Keys::generate().public_key().to_bytes(),
            3 => envelope.version += 1,
            4 => envelope.key_id = "other-key".into(),
            _ => {
                envelope.nonce.pop();
            }
        }
        assert!(
            master().decrypt(&account, &envelope).is_err(),
            "field {field}"
        );
    }
}

#[test]
fn key_backup_invalid_secrets_config_and_debug_fail_closed() {
    for raw in ["", "no", &"00".repeat(31), &"zz".repeat(32)] {
        assert!(BackupMasterKey::from_hex("v1", raw).is_err());
    }
    for id in ["", "a\nb", "a/b", &"a".repeat(65)] {
        assert!(BackupMasterKey::from_hex(id, &"19".repeat(32)).is_err());
    }
    assert!(master().encrypt(&[0; 32], &"00".repeat(32)).is_err());
    assert!(master().encrypt(&[0; 32], &"ff".repeat(32)).is_err());
    assert!(!format!("{:?}", master()).contains(&"19".repeat(32)));
}

fn proof(keys: &Keys, account: &str, challenge: &str, url: &str, when: u64) -> nostr::Event {
    EventBuilder::new(Kind::HttpAuth, "")
        .tags([
            Tag::parse(["u", url]).unwrap(),
            Tag::parse(["method", "POST"]).unwrap(),
            Tag::parse(["challenge", challenge]).unwrap(),
            Tag::parse(["account", account]).unwrap(),
            Tag::parse(["action", "initialize"]).unwrap(),
        ])
        .custom_created_at(Timestamp::from_secs(when))
        .sign_with_keys(keys)
        .unwrap()
}

#[test]
fn key_backup_link_proof_binds_account_action_key_url_time_and_signature() {
    let keys = Keys::generate();
    let account = "12".repeat(32);
    let challenge = "34".repeat(32);
    let url = "https://relay.example/auth/key-backup";
    let event = proof(&keys, &account, &challenge, url, 1000);
    let pubkey = keys.public_key().to_hex();
    validate_link_proof(&event, &account, &pubkey, &challenge, url, 1000).unwrap();
    assert!(validate_link_proof(&event, "other", &pubkey, &challenge, url, 1000).is_err());
    assert!(validate_link_proof(&event, &account, &pubkey, "other", url, 1000).is_err());
    assert!(validate_link_proof(
        &event,
        &account,
        &Keys::generate().public_key().to_hex(),
        &challenge,
        url,
        1000
    )
    .is_err());
    assert!(
        validate_link_proof(&event, &account, &pubkey, &challenge, "https://other", 1000).is_err()
    );
    for now in [939, 1061] {
        assert!(validate_link_proof(&event, &account, &pubkey, &challenge, url, now).is_err());
    }
    let mut invalid = event.clone();
    invalid.content = "key export".into();
    assert!(validate_link_proof(&invalid, &account, &pubkey, &challenge, url, 1000).is_err());
    let extra = EventBuilder::new(Kind::HttpAuth, "")
        .tags(
            event
                .tags
                .clone()
                .into_iter()
                .chain([Tag::parse(["action", "restore"]).unwrap()]),
        )
        .custom_created_at(Timestamp::from_secs(1000))
        .sign_with_keys(&keys)
        .unwrap();
    assert!(validate_link_proof(&extra, &account, &pubkey, &challenge, url, 1000).is_err());
}

/// Version 1 envelope as the relay wrote it before the protocol rename.
fn legacy_v1(account: &[u8; 32], key: &Keys) -> EncryptedKeyBackup {
    let raw = key.secret_key().to_secret_bytes();
    master()
        .seal(
            account,
            key.public_key().to_bytes(),
            &raw,
            LEGACY_V1_BACKUP_VERSION,
            LEGACY_V1_AAD_DOMAIN,
        )
        .unwrap()
}

#[test]
fn legacy_v1_backup_reencrypts_to_current_version_with_same_key() {
    let key = Keys::generate();
    let account = [7; 32];
    let v1 = legacy_v1(&account, &key);
    assert_eq!(v1.version, 1);
    // The current reader does not accept version 1 envelopes.
    assert_eq!(
        master().decrypt(&account, &v1).unwrap_err(),
        BackupError::Decryption
    );

    let v2 = master().reencrypt_legacy_v1(&account, &v1).unwrap();
    assert_eq!(v2.version, BACKUP_VERSION);
    assert_eq!(v2.version, 2);
    assert_eq!(v2.pubkey, v1.pubkey);
    assert_eq!(v2.key_id, v1.key_id);
    assert_ne!(v2.nonce, v1.nonce);
    assert_eq!(v2.ciphertext.len(), 48);
    let secret = master().decrypt(&account, &v2).unwrap();
    assert_eq!(secret.as_str(), key.secret_key().to_secret_hex());

    // Already-current envelopes, wrong accounts and tampered v1 rows are
    // refused rather than rewritten.
    assert!(master().reencrypt_legacy_v1(&account, &v2).is_err());
    assert!(master().reencrypt_legacy_v1(&[8; 32], &v1).is_err());
    let mut tampered = v1.clone();
    tampered.ciphertext[0] ^= 1;
    assert!(master().reencrypt_legacy_v1(&account, &tampered).is_err());
    let wrong = BackupMasterKey::from_hex("test-v1", &"29".repeat(32)).unwrap();
    assert!(wrong.reencrypt_legacy_v1(&account, &v1).is_err());
}

#[test]
fn legacy_v1_envelope_is_bound_to_the_pre_rename_domain() {
    // A v1 envelope sealed under the current domain must not authenticate as
    // legacy data: the rewrite only accepts genuine pre-rename ciphertexts.
    let key = Keys::generate();
    let account = [7; 32];
    let raw = key.secret_key().to_secret_bytes();
    let forged = master()
        .seal(
            &account,
            key.public_key().to_bytes(),
            &raw,
            LEGACY_V1_BACKUP_VERSION,
            AAD_DOMAIN,
        )
        .unwrap();
    assert!(master().reencrypt_legacy_v1(&account, &forged).is_err());
}

#[test]
fn agent_copy_round_trips_and_never_opens_as_another_envelope() {
    let agent = Keys::generate();
    let owner = [5; 32];
    let copy = master()
        .encrypt_agent(&owner, &agent.secret_key().to_secret_hex())
        .unwrap();
    assert_eq!(copy.version, AGENT_BACKUP_VERSION);
    assert_eq!(copy.pubkey, agent.public_key().to_bytes());
    let secret = master().decrypt_agent(&owner, &copy).unwrap();
    assert_eq!(
        Keys::parse(secret.as_str()).unwrap().public_key(),
        agent.public_key()
    );
    // Another owner, the account-backup domain and a tampered key all fail.
    assert!(master().decrypt_agent(&[6; 32], &copy).is_err());
    assert!(master().decrypt(&owner, &copy).is_err());
    let account = master()
        .encrypt(&owner, &agent.secret_key().to_secret_hex())
        .unwrap();
    assert!(master().decrypt_agent(&owner, &account).is_err());
    let mut moved = copy.clone();
    moved.pubkey = Keys::generate().public_key().to_bytes();
    assert!(master().decrypt_agent(&owner, &moved).is_err());
}

#[test]
fn agent_register_signature_binds_challenge_owner_and_device() {
    use nostr::hashes::{sha256::Hash as Sha256Hash, Hash as _};

    let agent = Keys::generate();
    let (challenge, owner, device) = ("c".repeat(64), "a".repeat(64), "02".repeat(33));
    assert_eq!(
        agent_register_message(&challenge, &owner, &device),
        format!("kuzz-agent-register/v1\n{challenge}\n{owner}\n{device}")
    );
    let digest = Sha256Hash::hash(agent_register_message(&challenge, &owner, &device).as_bytes());
    let signature = agent
        .sign_schnorr(&nostr::secp256k1::Message::from_digest(
            digest.to_byte_array(),
        ))
        .to_string();
    let pubkey = agent.public_key();
    assert!(verify_agent_register(&pubkey, &signature, &challenge, &owner, &device).is_ok());
    let other = "b".repeat(64);
    assert!(verify_agent_register(&pubkey, &signature, &other, &owner, &device).is_err());
    assert!(verify_agent_register(&pubkey, &signature, &challenge, &other, &device).is_err());
    assert!(verify_agent_register(&pubkey, &signature, &challenge, &owner, "03").is_err());
    assert!(verify_agent_register(
        &Keys::generate().public_key(),
        &signature,
        &challenge,
        &owner,
        &device
    )
    .is_err());
    assert!(verify_agent_register(&pubkey, "00", &challenge, &owner, &device).is_err());
}
