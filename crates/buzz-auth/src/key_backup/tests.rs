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
