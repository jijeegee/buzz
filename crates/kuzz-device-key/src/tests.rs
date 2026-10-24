use std::collections::HashMap;
use std::sync::Mutex;

use kuzz_auth::device_proof::{verify_event_device_tag, DeviceProof, DeviceProofError};
use nostr::{Keys, Kind, Tag, Timestamp};

use super::*;

/// The shared vectors of kuzz-auth (A1), which every client must reproduce.
const VECTORS: &str = include_str!("../../kuzz-auth/src/device_proof_vectors.json");

fn vectors() -> serde_json::Value {
    serde_json::from_str(VECTORS).expect("vectors")
}

/// The vectors' device key (secret 0x07 * 32).
fn vector_key() -> DeviceKey {
    let secret = vectors()["device_secret_hex"]
        .as_str()
        .expect("secret")
        .to_owned();
    let secret = software::decode(&secret).expect("decode");
    DeviceKey::software(software::signing_key(&secret).expect("secret")).expect("key")
}

fn hash32(value: &serde_json::Value) -> [u8; 32] {
    hex::decode(value.as_str().expect("hex"))
        .expect("hex")
        .try_into()
        .expect("32 bytes")
}

fn now() -> i64 {
    unix_now()
}

fn auth_event(keys: &Keys) -> UnsignedEvent {
    UnsignedEvent::new(
        keys.public_key(),
        Timestamp::from(1_700_000_000),
        Kind::Custom(22242),
        [
            Tag::parse(["relay", "wss://relay.example"]).expect("tag"),
            Tag::parse(["challenge", "c1"]).expect("tag"),
        ],
        "",
    )
}

#[test]
fn the_vector_key_reproduces_the_header_proof_vector() {
    let vectors = vectors();
    let key = vector_key();
    assert_eq!(key.pubkey().to_hex(), vectors["device_pubkey_hex"]);
    assert_eq!(key.kind(), DeviceKeyKind::Software);
    let header = &vectors["header_proof"];
    let token = hash32(&header["token_sha256_hex"]);
    let body = hash32(&header["body_sha256_hex"]);
    let scope = ProofScope {
        purpose: header["purpose"].as_str().expect("purpose"),
        binding: header["binding"].as_str().expect("binding"),
        token_hash: Some(&token),
        body_hash: Some(&body),
    };
    let proof = key
        .header_proof_hashed(
            header["ts"].as_i64().expect("ts"),
            header["nonce"].as_str().expect("nonce"),
            &scope,
        )
        .expect("proof");
    // RFC 6979 signatures are deterministic, so the whole header matches.
    assert_eq!(proof, header["header"]);
}

#[test]
fn the_vector_key_reproduces_the_event_tag_vectors() {
    let key = vector_key();
    for tagged in vectors()["event_tags"].as_array().expect("event_tags") {
        let event: nostr::Event = serde_json::from_value(tagged["event"].clone()).expect("event");
        let expected = event
            .tags
            .iter()
            .find(|tag| tag.as_slice().first().map(String::as_str) == Some(DEVICE_TAG))
            .expect("device tag")
            .as_slice()
            .to_vec();
        let others = event
            .tags
            .iter()
            .filter(|tag| tag.as_slice().first().map(String::as_str) != Some(DEVICE_TAG))
            .cloned();
        let unsigned = UnsignedEvent::new(
            event.pubkey,
            event.created_at,
            event.kind,
            others,
            event.content.clone(),
        );
        let tagged = key.add_event_tag(unsigned).expect("tag");
        let produced = tagged.tags.last().expect("tag").as_slice().to_vec();
        assert_eq!(produced, expected);
    }
}

#[test]
fn the_server_verifier_accepts_a_header_proof() {
    let dir = tempfile::tempdir().expect("dir");
    let key = DeviceKey::open_or_create_file(&dir.path().join("device.key")).expect("key");
    let binding = "POST https://relay.example/auth/refresh?x=1";
    let body = br#"{"refresh_token":"r"}"#;
    let raw = key
        .header_proof_with_body("refresh", binding, Some("tok"), body)
        .expect("proof");
    let proof = DeviceProof::parse(&raw).expect("parse");
    let token_hash = kuzz_auth::hash_token("tok");
    let body_hash: [u8; 32] = sha2::Sha256::digest(body).into();
    let scope = |body_hash| ProofScope {
        purpose: "refresh",
        binding,
        token_hash: Some(&token_hash),
        body_hash,
    };
    assert_eq!(
        proof.verify(&key.pubkey(), &scope(Some(&body_hash)), now()),
        Ok(())
    );
    assert_eq!(
        proof.verify(&key.pubkey(), &scope(None), now()),
        Err(DeviceProofError::BadSignature)
    );

    // No body, no token: the login bind proof.
    let raw = key.header_proof("bind", "login:abcd", None).expect("proof");
    let proof = DeviceProof::parse(&raw).expect("parse");
    let bind = ProofScope {
        purpose: "bind",
        binding: "login:abcd",
        token_hash: None,
        body_hash: None,
    };
    assert_eq!(proof.verify(&key.pubkey(), &bind, now()), Ok(()));
}

#[test]
fn a_hashed_header_proof_verifies_like_the_unhashed_one() {
    let key = vector_key();
    let binding = "POST https://relay.example/auth/refresh";
    let body = br#"{"refresh_token":"r"}"#;
    let token_hash = kuzz_auth::hash_token("tok");
    let body_hash: [u8; 32] = sha2::Sha256::digest(body).into();
    let raw = key
        .header_proof_hashed_at(
            now(),
            "refresh",
            binding,
            Some(&token_hash),
            Some(&body_hash),
        )
        .expect("proof");
    let proof = DeviceProof::parse(&raw).expect("parse");
    let scope = ProofScope {
        purpose: "refresh",
        binding,
        token_hash: Some(&token_hash),
        body_hash: Some(&body_hash),
    };
    assert_eq!(proof.verify(&key.pubkey(), &scope, now()), Ok(()));
    let unhashed = key
        .header_proof_at(now(), "refresh", binding, Some("tok"), body)
        .expect("proof");
    let unhashed = DeviceProof::parse(&unhashed).expect("parse");
    assert_eq!(unhashed.verify(&key.pubkey(), &scope, now()), Ok(()));
}

#[test]
fn header_proofs_use_fresh_nonces() {
    let key = vector_key();
    let first = DeviceProof::parse(&key.header_proof("auth", "GET u", Some("t")).expect("1"))
        .expect("parse");
    let second = DeviceProof::parse(&key.header_proof("auth", "GET u", Some("t")).expect("2"))
        .expect("parse");
    assert_ne!(first.nonce, second.nonce);
}

#[test]
fn the_server_verifier_accepts_a_tagged_event() {
    let keys = Keys::generate();
    let key = vector_key();
    let mut unsigned = auth_event(&keys);
    unsigned.ensure_id();
    let tagged = key.add_event_tag(unsigned).expect("tag");
    assert!(tagged.id.is_none(), "a stale id must be cleared");
    let event = tagged.sign_with_keys(&keys).expect("sign");
    assert_eq!(verify_event_device_tag(&event), Ok(Some(key.pubkey())));
}

#[test]
fn an_event_is_tagged_once() {
    let keys = Keys::generate();
    let key = vector_key();
    let tagged = key.add_event_tag(auth_event(&keys)).expect("tag");
    assert!(matches!(
        key.add_event_tag(tagged),
        Err(DeviceKeyError::AlreadyTagged)
    ));
}

#[test]
fn a_key_file_is_created_once_and_reopened() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("nested").join("device.key");
    let first = DeviceKey::open_or_create_file(&path).expect("create");
    let second = DeviceKey::open_or_create_file(&path).expect("open");
    assert_eq!(first.pubkey(), second.pubkey());
    assert_eq!(first.kind(), DeviceKeyKind::Software);
    let stored = std::fs::read_to_string(&path).expect("read");
    assert_eq!(stored.len(), 64);
    // No temporary file left behind.
    let names: Vec<_> = std::fs::read_dir(path.parent().expect("parent"))
        .expect("dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("device.key")]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn a_corrupt_key_file_is_an_error_and_left_alone() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("device.key");
    std::fs::write(&path, "not hex").expect("write");
    assert!(matches!(
        DeviceKey::open_or_create_file(&path),
        Err(DeviceKeyError::CorruptKey)
    ));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "not hex");
}

#[test]
fn racing_file_creators_agree_on_one_key() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("device.key");
    let keys: Vec<DevicePubkey> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| DeviceKey::open_or_create_file(&path).expect("key").pubkey()))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("join"))
            .collect()
    });
    assert!(keys.iter().all(|key| *key == keys[0]));
}

#[test]
fn scopes_are_validated() {
    for bad in ["", "a b", "a/b", "a\\b", "é", &"x".repeat(65)] {
        assert!(matches!(
            DeviceKey::open_or_create(bad, &MemoryStore::default()),
            Err(DeviceKeyError::InvalidScope)
        ));
    }
    assert!(validate_scope("install-0123_abc.v1").is_ok());
    let dir = tempfile::tempdir().expect("dir");
    assert!(matches!(
        FileStore::new(dir.path()).load("../escape"),
        Err(DeviceKeyError::InvalidScope)
    ));
}

#[test]
fn kinds_map_to_the_login_wire_values() {
    assert_eq!(DeviceKeyKind::Tpm.as_wire(), "hardware");
    assert_eq!(DeviceKeyKind::SecureEnclave.as_wire(), "hardware");
    assert_eq!(DeviceKeyKind::Software.as_wire(), "software");
}

/// The Secure Enclave returns DER; the proof wants the raw `r || s` every
/// other backend signs.
#[test]
fn a_der_signature_becomes_the_raw_form() {
    use p256::ecdsa::signature::Signer as _;
    let key = p256::ecdsa::SigningKey::from_slice(&[7; 32]).expect("key");
    let signature: p256::ecdsa::Signature = key.sign(b"message");
    let der = signature.to_der();
    let raw: [u8; 64] = signature.to_bytes().into();
    assert_eq!(raw_signature_from_der(der.as_bytes()).expect("der"), raw);
    assert!(matches!(
        raw_signature_from_der(&raw),
        Err(DeviceKeyError::CorruptKey)
    ));
}

#[test]
fn debug_output_shows_no_secret() {
    let key = vector_key();
    let debug = format!("{key:?}");
    assert!(!debug.contains("0707070707"));
    assert!(debug.contains(&key.pubkey().to_hex()));
}

/// An in-memory store like an app's keyring store: no atomic
/// create-if-absent, so it writes, reads back and reports a winner.
#[derive(Default)]
struct MemoryStore {
    keys: Mutex<HashMap<String, [u8; 32]>>,
    /// A racing creator's key, written just before ours would land.
    race_winner: Option<[u8; 32]>,
}

impl SoftwareKeyStore for MemoryStore {
    fn load(&self, scope: &str) -> Result<Option<SoftwareSecret>> {
        let keys = self.keys.lock().expect("lock");
        Ok(keys.get(scope).map(|secret| SoftwareSecret::new(*secret)))
    }

    fn store_new(&self, scope: &str, secret: &SoftwareSecret) -> Result<StoreOutcome> {
        let mut keys = self.keys.lock().expect("lock");
        if let Some(winner) = self.race_winner {
            keys.insert(scope.to_owned(), winner);
        }
        let stored = *keys
            .entry(scope.to_owned())
            .or_insert(*secret.expose_secret());
        Ok(if stored == *secret.expose_secret() {
            StoreOutcome::Stored
        } else {
            StoreOutcome::Existing(SoftwareSecret::new(stored))
        })
    }

    fn delete(&self, scope: &str) -> Result<()> {
        self.keys.lock().expect("lock").remove(scope);
        Ok(())
    }
}

/// A store without delete (the trait default).
struct NoDeleteStore;

impl SoftwareKeyStore for NoDeleteStore {
    fn load(&self, _scope: &str) -> Result<Option<SoftwareSecret>> {
        Ok(None)
    }

    fn store_new(&self, _scope: &str, _secret: &SoftwareSecret) -> Result<StoreOutcome> {
        Ok(StoreOutcome::Stored)
    }
}

#[test]
fn a_store_key_is_created_once_and_reopened() {
    let store = MemoryStore::default();
    let first = software::open_or_create(&store, "s").expect("create");
    let second = software::open_or_create(&store, "s").expect("open");
    assert_eq!(first.verifying_key(), second.verifying_key());
    assert_eq!(store.keys.lock().expect("lock").len(), 1);
}

#[test]
fn a_racing_creator_key_wins_over_ours() {
    let winner = *software::generate().expose_secret();
    let store = MemoryStore {
        race_winner: Some(winner),
        ..MemoryStore::default()
    };
    let key = software::open_or_create(&store, "s").expect("create");
    let expected = software::signing_key(&SoftwareSecret::new(winner)).expect("winner");
    assert_eq!(key.verifying_key(), expected.verifying_key());
}

#[test]
fn a_file_store_creates_once_and_returns_the_existing_key() {
    let dir = tempfile::tempdir().expect("dir");
    let store = FileStore::new(dir.path().join("keys"));
    assert!(store.load("install-1").expect("load").is_none());
    let first = software::generate();
    assert!(matches!(
        store.store_new("install-1", &first).expect("store"),
        StoreOutcome::Stored
    ));
    let second = software::generate();
    match store.store_new("install-1", &second).expect("store") {
        StoreOutcome::Existing(existing) => {
            assert_eq!(existing.expose_secret(), first.expose_secret())
        }
        StoreOutcome::Stored => panic!("an existing key was overwritten"),
    }
    assert_eq!(
        store
            .load("install-1")
            .expect("load")
            .expect("key")
            .expose_secret(),
        first.expose_secret()
    );
    assert!(dir.path().join("keys").join("install-1.key").exists());
    store.delete("install-1").expect("delete");
    store.delete("install-1").expect("delete a missing entry");
    assert!(store.load("install-1").expect("load").is_none());
}

#[test]
fn a_file_store_without_hard_links_creates_directly() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("k").join("device.key");
    let no_links = |_: &Path, _: &Path| -> std::io::Result<()> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    };
    let first = software::generate();
    assert!(matches!(
        software::file_store_new_with(&path, &first, no_links).expect("store"),
        StoreOutcome::Stored
    ));
    let second = software::generate();
    match software::file_store_new_with(&path, &second, no_links).expect("store") {
        StoreOutcome::Existing(existing) => {
            assert_eq!(existing.expose_secret(), first.expose_secret())
        }
        StoreOutcome::Stored => panic!("an existing key was overwritten"),
    }
    // Only the key file: every temporary file was removed.
    let names: Vec<_> = std::fs::read_dir(path.parent().expect("parent"))
        .expect("dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("device.key")]);
}

#[test]
fn a_file_store_keeps_tpm_markers() {
    let dir = tempfile::tempdir().expect("dir");
    let store = FileStore::new(dir.path());
    let marker = SoftwareSecret::new([9u8; 32]);
    store
        .store_new("kuzz-device-v1#tpm", &marker)
        .expect("marker");
    assert!(store.load("kuzz-device-v1#tpm").expect("load").is_some());
    assert!(matches!(
        store.load("a#b"),
        Err(DeviceKeyError::InvalidScope)
    ));
}

#[test]
fn secrets_are_redacted_in_debug_output() {
    let secret = SoftwareSecret::new([0xab; 32]);
    assert_eq!(format!("{secret:?}"), "[redacted]");
    let outcome = format!("{:?}", StoreOutcome::Existing(secret.clone()));
    assert!(!outcome.to_lowercase().contains("ab, ") && !outcome.contains("171"));
    assert!(!outcome.contains("abab"));
}

#[test]
fn a_tpm_marker_blocks_the_software_fallback() {
    let tpm_error = || DeviceKeyError::Tpm {
        op: "NCryptOpenStorageProvider",
        code: 0x8028_400f,
    };
    let store = MemoryStore::default();
    let pubkey = vector_key().pubkey();
    record_tpm_marker(&store, DEVICE_KEY_SCOPE, &pubkey).expect("marker");
    let expected: [u8; 32] = sha2::Sha256::digest(pubkey.as_bytes()).into();
    let marker = store
        .load("kuzz-device-v1#tpm")
        .expect("load")
        .expect("marker");
    assert_eq!(marker.expose_secret(), &expected);
    for create in [true, false] {
        assert!(matches!(
            software_fallback(&store, DEVICE_KEY_SCOPE, tpm_error(), create),
            Err(DeviceKeyError::Tpm { .. })
        ));
    }
    assert!(
        store.load(DEVICE_KEY_SCOPE).expect("load").is_none(),
        "no software key made"
    );

    // Without a marker, the fallback makes (or finds) the software key.
    let fresh = MemoryStore::default();
    assert!(
        software_fallback(&fresh, DEVICE_KEY_SCOPE, tpm_error(), false)
            .expect("no key")
            .is_none()
    );
    assert!(
        software_fallback(&fresh, DEVICE_KEY_SCOPE, tpm_error(), true)
            .expect("fallback")
            .is_some()
    );
}

/// A hardware key without a marker (a stray test or dev build key) never
/// displaces the software key the relay may have bound; with a marker, or
/// with no software key at all, the hardware key is the one in use.
#[test]
fn a_software_key_without_a_marker_stays_in_use_over_a_stray_hardware_key() {
    let scope = DEVICE_KEY_SCOPE;
    // Software key, no marker: software stays (open and open_or_create
    // both return it before touching the hardware key).
    let store = MemoryStore::default();
    let made = software::open_or_create(&store, scope).expect("seed");
    let kept = unmarked_software_key(&store, scope)
        .expect("load")
        .expect("software in use");
    assert_eq!(kept.verifying_key(), made.verifying_key());
    assert!(
        store
            .load(&tpm_marker_scope(scope))
            .expect("load")
            .is_none(),
        "no marker recorded for the stray key"
    );

    // Marker present: the hardware key is in use (as before).
    record_tpm_marker(&store, scope, &vector_key().pubkey()).expect("marker");
    assert!(unmarked_software_key(&store, scope)
        .expect("load")
        .is_none());

    // Fresh install, nothing bound yet: the hardware key is used.
    let fresh = MemoryStore::default();
    assert!(unmarked_software_key(&fresh, scope)
        .expect("load")
        .is_none());
}

#[test]
fn open_never_creates_a_key() {
    let scope = format!("test-opennever-{}", std::process::id());
    let store = MemoryStore::default();
    assert!(DeviceKey::open(&scope, &store).expect("open").is_none());
    assert!(store.keys.lock().expect("lock").is_empty());
    #[cfg(windows)]
    assert!(matches!(
        tpm::open(&tpm_key_name(&scope)),
        Ok(None) | Err(tpm::TpmError::Unavailable(_))
    ));
    // An existing store key opens.
    let made = software::open_or_create(&store, &scope).expect("seed");
    let opened = DeviceKey::open(&scope, &store).expect("open").expect("key");
    assert_eq!(
        opened.pubkey().as_bytes().as_slice(),
        made.verifying_key().to_encoded_point(true).as_bytes()
    );
}

#[test]
fn reset_removes_the_store_key_and_marker() {
    let scope = format!("test-reset-{}", std::process::id());
    let store = MemoryStore::default();
    software::open_or_create(&store, &scope).expect("seed");
    record_tpm_marker(&store, &scope, &vector_key().pubkey()).expect("marker");
    DeviceKey::reset(&scope, &store).expect("reset");
    assert!(store.keys.lock().expect("lock").is_empty());
    assert!(matches!(
        DeviceKey::reset(&scope, &NoDeleteStore),
        Err(DeviceKeyError::Store(_))
    ));
}

#[cfg(windows)]
mod windows_tpm {
    use std::time::{Duration, Instant};

    use super::*;

    fn test_name(label: &str) -> (String, String) {
        let scope = format!("test-{label}-{}", std::process::id());
        let name = tpm_key_name(&scope);
        (scope, name)
    }

    /// Deletes the test key `0` when dropped, also when the test panics.
    struct Cleanup(String);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Ok(Some(key)) = tpm::open(&self.0) {
                key.delete().expect("delete test key");
            }
            assert!(matches!(tpm::open(&self.0), Ok(None)), "test key deleted");
        }
    }

    #[test]
    fn only_no_tpm_codes_allow_a_software_key() {
        use windows_sys::Win32::Foundation::{
            E_ACCESSDENIED, NTE_DEVICE_NOT_READY, NTE_PERM, TBS_E_SERVICE_NOT_RUNNING,
            TPM_E_PCP_DEVICE_NOT_READY,
        };
        for transient in [
            E_ACCESSDENIED,
            NTE_DEVICE_NOT_READY,
            NTE_PERM,
            TBS_E_SERVICE_NOT_RUNNING,
            TPM_E_PCP_DEVICE_NOT_READY,
            windows_sys::Win32::Foundation::NTE_BAD_KEYSET,
        ] {
            assert!(
                !tpm::CREATE_UNAVAILABLE.contains(&transient),
                "{transient:#x}"
            );
            assert!(
                !tpm::PROVIDER_UNAVAILABLE.contains(&transient),
                "{transient:#x}"
            );
        }
        use windows_sys::Win32::Foundation::{
            NTE_BAD_ALGID, NTE_NOT_SUPPORTED, TBS_E_TPM_NOT_FOUND,
        };
        assert!(tpm::PROVIDER_UNAVAILABLE.contains(&TBS_E_TPM_NOT_FOUND));
        assert!(tpm::CREATE_UNAVAILABLE.contains(&TBS_E_TPM_NOT_FOUND));
        // "Cannot make P-256 keys" counts only from create / finalize.
        for code in [NTE_NOT_SUPPORTED, NTE_BAD_ALGID] {
            assert!(!tpm::PROVIDER_UNAVAILABLE.contains(&code));
            assert!(tpm::CREATE_UNAVAILABLE.contains(&code));
        }
    }

    /// A software key already in the store wins over making a TPM key (no
    /// drift for installs that started without a TPM). Only opens the TPM.
    #[test]
    fn an_existing_store_key_is_used_before_creating_a_tpm_key() {
        let (scope, name) = test_name("storewins");
        let _cleanup = Cleanup(name.clone());
        let store = MemoryStore::default();
        let expected = software::open_or_create(&store, &scope).expect("seed");
        let key = DeviceKey::open_or_create(&scope, &store).expect("open");
        assert_eq!(key.kind(), DeviceKeyKind::Software);
        assert_eq!(
            key.pubkey().as_bytes().as_slice(),
            expected.verifying_key().to_encoded_point(true).as_bytes()
        );
        assert!(matches!(
            tpm::open(&name),
            Ok(None) | Err(tpm::TpmError::Unavailable(_))
        ));
    }

    /// Creates, uses and deletes a `Kuzz-Device-test-*` TPM key; run by hand
    /// on a machine with a TPM.
    #[test]
    #[ignore = "needs a TPM; creates and deletes a test key"]
    fn a_tpm_key_signs_proofs_the_server_accepts() {
        let (scope, name) = test_name("roundtrip");
        let name_for_check = name.clone();
        let _cleanup = Cleanup(name);
        let store = MemoryStore::default();
        assert!(DeviceKey::open(&scope, &store).expect("open").is_none());
        let key = DeviceKey::open_or_create(&scope, &store).expect("create");
        assert_eq!(key.kind(), DeviceKeyKind::Tpm);
        // Only the marker: the SHA-256 of the TPM public key.
        let expected: [u8; 32] = sha2::Sha256::digest(key.pubkey().as_bytes()).into();
        {
            let keys = store.keys.lock().expect("lock");
            assert_eq!(keys.len(), 1);
            assert_eq!(keys.get(&format!("{scope}#tpm")), Some(&expected));
        }
        let again = DeviceKey::open_or_create(&scope, &store).expect("open");
        assert_eq!(again.pubkey(), key.pubkey());
        let child = DeviceKey::open(&scope, &store).expect("open").expect("key");
        assert_eq!(child.pubkey(), key.pubkey());
        assert_eq!(child.kind(), DeviceKeyKind::Tpm);

        let raw = key
            .header_proof("auth", "GET https://relay.example/auth/me", Some("tok"))
            .expect("proof");
        let token_hash = kuzz_auth::hash_token("tok");
        let proof_scope = ProofScope {
            purpose: "auth",
            binding: "GET https://relay.example/auth/me",
            token_hash: Some(&token_hash),
            body_hash: None,
        };
        let proof = DeviceProof::parse(&raw).expect("parse");
        assert_eq!(proof.verify(&key.pubkey(), &proof_scope, now()), Ok(()));

        let keys = Keys::generate();
        let event = key
            .add_event_tag(auth_event(&keys))
            .expect("tag")
            .sign_with_keys(&keys)
            .expect("sign");
        assert_eq!(verify_event_device_tag(&event), Ok(Some(key.pubkey())));

        drop((key, again, child));
        DeviceKey::reset(&scope, &store).expect("reset");
        assert!(matches!(tpm::open(&name_for_check), Ok(None)));
        assert!(store.keys.lock().expect("lock").is_empty());
    }

    fn stats(label: &str, mut samples: Vec<Duration>) {
        samples.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let p95 = samples[(samples.len() * 95).div_ceil(100) - 1];
        println!(
            "{label}: n={} median={:.2} ms p95={:.2} ms min={:.2} ms max={:.2} ms",
            samples.len(),
            ms(samples[samples.len() / 2]),
            ms(p95),
            ms(samples[0]),
            ms(samples[samples.len() - 1]),
        );
    }

    /// B0 speed spike: TPM key create, open and sign latency against
    /// software signing. Run with
    /// `cargo test -p kuzz-device-key -- --ignored --nocapture tpm_latency`.
    #[test]
    #[ignore = "B0 speed spike; needs a TPM"]
    fn tpm_latency_spike() {
        let (scope_name, name) = test_name("bench");
        let scope = scope_name.as_str();
        let _cleanup = Cleanup(name);
        let start = Instant::now();
        let key = DeviceKey::open_or_create(scope, &MemoryStore::default()).expect("create");
        let create = start.elapsed();
        assert_eq!(key.kind(), DeviceKeyKind::Tpm);
        drop(key);
        println!("create: {:.2} ms", create.as_secs_f64() * 1000.0);

        let opens: Vec<Duration> = (0..30)
            .map(|_| {
                let start = Instant::now();
                let key = DeviceKey::open_or_create(scope, &MemoryStore::default()).expect("open");
                let elapsed = start.elapsed();
                assert_eq!(key.kind(), DeviceKeyKind::Tpm);
                elapsed
            })
            .collect();
        stats("tpm open (provider + key + public export)", opens);

        let key = DeviceKey::open_or_create(scope, &MemoryStore::default()).expect("open");
        let message = b"kuzz-device-proof/v1\nauth\nGET https://relay.example/auth/me";
        let signs: Vec<Duration> = (0..50)
            .map(|_| {
                let start = Instant::now();
                key.sign(message).expect("sign");
                start.elapsed()
            })
            .collect();
        stats("tpm sign", signs);

        let threads = 4;
        let per_thread = 10;
        let start = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    for _ in 0..per_thread {
                        key.sign(message).expect("sign");
                    }
                });
            }
        });
        let total = start.elapsed();
        println!(
            "tpm sign throughput, {threads} threads sharing one handle: {:.1} signatures/s",
            f64::from(threads * per_thread) / total.as_secs_f64()
        );
        drop(key);

        // Separate processes (agents) hold separate handles.
        let start = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let key = DeviceKey::open_or_create(&scope_name, &MemoryStore::default())
                        .expect("open");
                    for _ in 0..per_thread {
                        key.sign(message).expect("sign");
                    }
                });
            }
        });
        let total = start.elapsed();
        println!(
            "tpm sign throughput, {threads} threads with own handles (incl. open): {:.1} signatures/s",
            f64::from(threads * per_thread) / total.as_secs_f64()
        );

        let software = vector_key();
        let signs: Vec<Duration> = (0..500)
            .map(|_| {
                let start = Instant::now();
                software.sign(message).expect("sign");
                start.elapsed()
            })
            .collect();
        stats("software sign", signs);
    }
}

#[cfg(target_os = "macos")]
mod macos_secure_enclave {
    use super::*;

    fn test_name(label: &str) -> (String, String) {
        let scope = format!("test-{label}-{}", std::process::id());
        let name = tpm_key_name(&scope);
        (scope, name)
    }

    /// Deletes the test key `0` when dropped, also when the test panics.
    struct Cleanup(String);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Ok(Some(key)) = secure_enclave::open(&self.0) {
                key.delete().expect("delete test key");
            }
            assert!(
                matches!(
                    secure_enclave::open(&self.0),
                    Ok(None) | Err(secure_enclave::HwError::Unavailable(_))
                ),
                "test key deleted"
            );
        }
    }

    #[test]
    fn only_no_secure_enclave_codes_allow_a_software_key() {
        // errSecMissingEntitlement, errSecUnimplemented, errSecNotAvailable.
        for code in [-34018, -4, -25291] {
            assert!(secure_enclave::UNAVAILABLE.contains(&code), "{code}");
        }
        // errSecInteractionNotAllowed, errSecAuthFailed, errSecItemNotFound,
        // errSecParam: retry later, never a software key.
        for code in [-25308, -25293, -25300, -50] {
            assert!(!secure_enclave::UNAVAILABLE.contains(&code), "{code}");
        }
    }

    /// A software key already in the store wins over making a Secure
    /// Enclave key: an install that signed in before this backend keeps its
    /// key. Runs unsigned too (the Secure Enclave is then unavailable).
    #[test]
    fn an_existing_store_key_is_used_before_creating_a_secure_enclave_key() {
        let (scope, name) = test_name("storewins");
        let _cleanup = Cleanup(name.clone());
        let store = MemoryStore::default();
        let expected = software::open_or_create(&store, &scope).expect("seed");
        let key = DeviceKey::open_or_create(&scope, &store).expect("open");
        assert_eq!(key.kind(), DeviceKeyKind::Software);
        assert_eq!(
            key.pubkey().as_bytes().as_slice(),
            expected.verifying_key().to_encoded_point(true).as_bytes()
        );
        assert!(matches!(
            secure_enclave::open(&name),
            Ok(None) | Err(secure_enclave::HwError::Unavailable(_))
        ));
    }

    /// Creates, uses and deletes a `Kuzz-Device-test-*` Secure Enclave key.
    /// Needs the test binary signed with a keychain access group (see
    /// `docs/apple-device-keys.md`); unsigned, it gets a software key.
    #[test]
    #[ignore = "needs a Secure Enclave and a signed test binary; creates and deletes a test key"]
    fn a_secure_enclave_key_signs_proofs_the_server_accepts() {
        let (scope, name) = test_name("roundtrip");
        let name_for_check = name.clone();
        let _cleanup = Cleanup(name);
        let store = MemoryStore::default();
        assert!(DeviceKey::open(&scope, &store).expect("open").is_none());
        let key = DeviceKey::open_or_create(&scope, &store).expect("create");
        assert_eq!(key.kind(), DeviceKeyKind::SecureEnclave);
        // Only the marker: the SHA-256 of the Secure Enclave public key.
        let expected: [u8; 32] = sha2::Sha256::digest(key.pubkey().as_bytes()).into();
        {
            let keys = store.keys.lock().expect("lock");
            assert_eq!(keys.len(), 1);
            assert_eq!(keys.get(&format!("{scope}#tpm")), Some(&expected));
        }
        let again = DeviceKey::open_or_create(&scope, &store).expect("open");
        assert_eq!(again.pubkey(), key.pubkey());
        let child = DeviceKey::open(&scope, &store).expect("open").expect("key");
        assert_eq!(child.pubkey(), key.pubkey());

        let raw = key
            .header_proof("auth", "GET https://relay.example/auth/me", Some("tok"))
            .expect("proof");
        let token_hash = kuzz_auth::hash_token("tok");
        let proof_scope = ProofScope {
            purpose: "auth",
            binding: "GET https://relay.example/auth/me",
            token_hash: Some(&token_hash),
            body_hash: None,
        };
        let proof = DeviceProof::parse(&raw).expect("parse");
        assert_eq!(proof.verify(&key.pubkey(), &proof_scope, now()), Ok(()));

        let keys = Keys::generate();
        let event = key
            .add_event_tag(auth_event(&keys))
            .expect("tag")
            .sign_with_keys(&keys)
            .expect("sign");
        assert_eq!(verify_event_device_tag(&event), Ok(Some(key.pubkey())));

        drop((key, again, child));
        DeviceKey::reset(&scope, &store).expect("reset");
        assert!(matches!(secure_enclave::open(&name_for_check), Ok(None)));
        assert!(store.keys.lock().expect("lock").is_empty());
    }
}
