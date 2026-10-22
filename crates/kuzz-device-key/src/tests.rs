use std::collections::HashMap;
use std::sync::Mutex;

use kuzz_auth::device_proof::{verify_event_device_tag, DeviceProof, DeviceProofError};
use nostr::{Keys, Kind, Tag, Timestamp};
use zeroize::Zeroizing;

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
    assert_eq!(DeviceKeyKind::Software.as_wire(), "software");
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
        Ok(keys.get(scope).map(|secret| Zeroizing::new(*secret)))
    }

    fn store_new(&self, scope: &str, secret: &SoftwareSecret) -> Result<StoreOutcome> {
        let mut keys = self.keys.lock().expect("lock");
        if let Some(winner) = self.race_winner {
            keys.insert(scope.to_owned(), winner);
        }
        let stored = *keys.entry(scope.to_owned()).or_insert(**secret);
        Ok(if stored == **secret {
            StoreOutcome::Stored
        } else {
            StoreOutcome::Existing(Zeroizing::new(stored))
        })
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
    let winner = *software::generate();
    let store = MemoryStore {
        race_winner: Some(winner),
        ..MemoryStore::default()
    };
    let key = software::open_or_create(&store, "s").expect("create");
    let expected = software::signing_key(&Zeroizing::new(winner)).expect("winner");
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
        StoreOutcome::Existing(existing) => assert_eq!(*existing, *first),
        StoreOutcome::Stored => panic!("an existing key was overwritten"),
    }
    assert_eq!(
        *store.load("install-1").expect("load").expect("key"),
        *first
    );
    assert!(dir.path().join("keys").join("install-1.key").exists());
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
            assert!(!tpm::UNAVAILABLE.contains(&transient), "{transient:#x}");
        }
        assert!(tpm::UNAVAILABLE.contains(&windows_sys::Win32::Foundation::TBS_E_TPM_NOT_FOUND));
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
        let _cleanup = Cleanup(name);
        let store = MemoryStore::default();
        let key = DeviceKey::open_or_create(&scope, &store).expect("create");
        assert_eq!(key.kind(), DeviceKeyKind::Tpm);
        assert!(store.keys.lock().expect("lock").is_empty());
        let again = DeviceKey::open_or_create(&scope, &store).expect("open");
        assert_eq!(again.pubkey(), key.pubkey());

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
