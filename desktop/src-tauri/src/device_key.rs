//! This install's device key (device-bound keys plan §2.1): a P-256 key in
//! the TPM where available, else a software key in the OS keyring. It adds
//! the `device` tag to auth events (NIP-42 22242, NIP-98 27235, Blossom
//! 24242, huddle auth) and signs `X-Kuzz-Device-Proof` headers on `/auth/*`.
//!
//! Everything here is best effort: with no device key, or when signing
//! fails, callers send exactly what they sent before and a warning is
//! logged. The device key is never deleted by sign-out.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

pub(crate) use kuzz_device_key_pkg::DeviceKey;
use kuzz_device_key_pkg::{SoftwareKeyStore, DEVICE_KEY_SCOPE};
use nostr::{EventBuilder, Keys, UnsignedEvent};

/// Header carrying a request's device proof.
pub(crate) const PROOF_HEADER: &str = "X-Kuzz-Device-Proof";
/// Header carrying the relay's unix time on `/auth/*` responses.
pub(crate) const SERVER_TIME_HEADER: &str = "x-kuzz-server-time";

/// Auth event kinds that carry the `device` tag.
const TAGGED_KINDS: [u16; 3] = [22242, 27235, 24242];

/// Retry opening a failed key no sooner than this.
const OPEN_RETRY: Duration = Duration::from_secs(30);

/// Clock offsets below this are treated as no offset (mobile: 30 s).
const CLOCK_SKEW_THRESHOLD: i64 = 30;

#[derive(Default)]
struct Slot {
    key: Option<DeviceKey>,
    failed_at: Option<Instant>,
    data_dir: Option<PathBuf>,
}

static SLOT: Mutex<Slot> = Mutex::new(Slot {
    key: None,
    failed_at: None,
    data_dir: None,
});

/// Serializes key creation so two callers never race `open_or_create`.
static OPENING: Mutex<()> = Mutex::new(());

static CLOCK: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);

fn slot() -> std::sync::MutexGuard<'static, Slot> {
    SLOT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Remember where a file store would live and open (or create) the device
/// key in the background, so the first signer does not wait for the TPM.
pub(crate) fn init(data_dir: Option<PathBuf>) {
    slot().data_dir = data_dir;
    if cfg!(test) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("kuzz-device-key".into())
        .spawn(|| {
            let _ = ensure();
        });
}

/// The device key when it is already open. Never blocks on the TPM.
pub(crate) fn current() -> Option<DeviceKey> {
    if cfg!(test) {
        return None;
    }
    slot().key.clone()
}

/// The device key, opening or creating it when needed. Blocks (TPM/keyring);
/// call from a blocking thread. `None` (with a warning) when unavailable.
pub(crate) fn ensure() -> Option<DeviceKey> {
    if cfg!(test) {
        return None;
    }
    if let Some(key) = current() {
        return Some(key);
    }
    let _opening = OPENING.lock().unwrap_or_else(PoisonError::into_inner);
    let data_dir = {
        let slot = slot();
        if let Some(key) = &slot.key {
            return Some(key.clone());
        }
        if slot.failed_at.is_some_and(|at| at.elapsed() < OPEN_RETRY) {
            return None;
        }
        slot.data_dir.clone()
    };
    let store = match store(data_dir) {
        Some(store) => store,
        None => {
            eprintln!("kuzz-desktop: device key unavailable: no key store");
            slot().failed_at = Some(Instant::now());
            return None;
        }
    };
    match DeviceKey::open_or_create(DEVICE_KEY_SCOPE, store.as_ref()) {
        Ok(key) => {
            eprintln!("kuzz-desktop: device key ready ({})", key.kind().as_wire());
            let mut slot = slot();
            slot.key = Some(key.clone());
            slot.failed_at = None;
            Some(key)
        }
        Err(error) => {
            eprintln!("kuzz-desktop: warning: device key unavailable: {error}");
            slot().failed_at = Some(Instant::now());
            None
        }
    }
}

/// [`ensure`] off the async runtime.
pub(crate) async fn ensure_async() -> Option<DeviceKey> {
    if let Some(key) = current() {
        return Some(key);
    }
    if cfg!(test) {
        return None;
    }
    tokio::task::spawn_blocking(ensure).await.ok().flatten()
}

#[cfg(feature = "system-keyring")]
fn store(_data_dir: Option<PathBuf>) -> Option<Box<dyn SoftwareKeyStore + Send>> {
    Some(Box::new(KeyringDeviceStore {
        service: crate::app_state::keyring_service().to_owned(),
    }))
}

#[cfg(not(feature = "system-keyring"))]
fn store(data_dir: Option<PathBuf>) -> Option<Box<dyn SoftwareKeyStore + Send>> {
    data_dir.map(|dir| {
        Box::new(kuzz_device_key_pkg::FileStore::new(dir.join("device-key")))
            as Box<dyn SoftwareKeyStore + Send>
    })
}

/// Device key secrets and TPM markers in the OS keyring: one entry per
/// scope (`device-key:<scope>`, value hex), never the shared secret blob
/// (the Windows credential blob is near its 2560-byte limit).
#[cfg(feature = "system-keyring")]
pub(crate) struct KeyringDeviceStore {
    service: String,
}

#[cfg(feature = "system-keyring")]
impl KeyringDeviceStore {
    fn entry(&self, scope: &str) -> kuzz_device_key_pkg::Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &format!("device-key:{scope}")).map_err(store_error)
    }
}

#[cfg(feature = "system-keyring")]
fn store_error(error: keyring::Error) -> kuzz_device_key_pkg::DeviceKeyError {
    kuzz_device_key_pkg::DeviceKeyError::Store(error.to_string())
}

#[cfg(feature = "system-keyring")]
impl SoftwareKeyStore for KeyringDeviceStore {
    fn load(
        &self,
        scope: &str,
    ) -> kuzz_device_key_pkg::Result<Option<kuzz_device_key_pkg::SoftwareSecret>> {
        match self.entry(scope)?.get_password() {
            Ok(stored) => {
                let stored = zeroize::Zeroizing::new(stored);
                decode_secret(stored.trim()).map(Some)
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(store_error(error)),
        }
    }

    fn store_new(
        &self,
        scope: &str,
        secret: &kuzz_device_key_pkg::SoftwareSecret,
    ) -> kuzz_device_key_pkg::Result<kuzz_device_key_pkg::StoreOutcome> {
        use kuzz_device_key_pkg::StoreOutcome;
        if let Some(existing) = self.load(scope)? {
            return Ok(StoreOutcome::Existing(existing));
        }
        let encoded = zeroize::Zeroizing::new(hex::encode(secret.expose_secret()));
        self.entry(scope)?
            .set_password(&encoded)
            .map_err(store_error)?;
        // No atomic create-if-absent in OS keyrings: read back, and follow
        // a racing creator that wrote last.
        match self.load(scope)? {
            Some(stored) if stored.expose_secret() == secret.expose_secret() => {
                Ok(StoreOutcome::Stored)
            }
            Some(stored) => Ok(StoreOutcome::Existing(stored)),
            None => Err(kuzz_device_key_pkg::DeviceKeyError::Store(
                "device key entry vanished after write".into(),
            )),
        }
    }

    fn delete(&self, scope: &str) -> kuzz_device_key_pkg::Result<()> {
        match self.entry(scope)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(store_error(error)),
        }
    }
}

#[cfg(feature = "system-keyring")]
fn decode_secret(stored: &str) -> kuzz_device_key_pkg::Result<kuzz_device_key_pkg::SoftwareSecret> {
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(stored, &mut bytes).map_err(|_| {
        kuzz_device_key_pkg::DeviceKeyError::Store("device key entry is malformed".into())
    })?;
    let secret = kuzz_device_key_pkg::SoftwareSecret::new(bytes);
    zeroize::Zeroize::zeroize(&mut bytes);
    Ok(secret)
}

/// Run a short blocking call (a TPM signature, ~13 ms) from sync code that
/// may be on an async worker thread.
fn run_blocking<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// Whether events of `kind` carry the `device` tag.
pub(crate) fn tags_kind(kind: nostr::Kind) -> bool {
    TAGGED_KINDS.contains(&kind.as_u16())
}

/// Add the `device` tag to an auth event with `key`. Other kinds, no key,
/// or a signing failure: the event unchanged. Pure apart from the signature.
pub(crate) fn tag_event_with(key: Option<&DeviceKey>, unsigned: UnsignedEvent) -> UnsignedEvent {
    let Some(key) = key else {
        return unsigned;
    };
    if !tags_kind(unsigned.kind) {
        return unsigned;
    }
    let original = unsigned.clone();
    match run_blocking(|| key.add_event_tag(unsigned)) {
        Ok(tagged) => tagged,
        Err(error) => {
            eprintln!(
                "kuzz-desktop: warning: device tag not added (kind {}): {error}",
                original.kind.as_u16()
            );
            original
        }
    }
}

/// Build an auth event for `keys`, add the `device` tag, and sign it.
pub(crate) fn sign_auth_event(builder: EventBuilder, keys: &Keys) -> Result<nostr::Event, String> {
    sign_auth_event_with(current().as_ref(), builder, keys)
}

/// [`sign_auth_event`] with an explicit device key.
pub(crate) fn sign_auth_event_with(
    key: Option<&DeviceKey>,
    builder: EventBuilder,
    keys: &Keys,
) -> Result<nostr::Event, String> {
    let unsigned = builder.build(keys.public_key());
    tag_event_with(key, unsigned)
        .sign_with_keys(keys)
        .map_err(|error| error.to_string())
}

/// The relay origin (`scheme://host[:port]`) of `url`, the clock key.
pub(crate) fn origin_of(url: &url::Url) -> String {
    url.origin().ascii_serialization()
}

fn clock() -> std::sync::MutexGuard<'static, Option<HashMap<String, i64>>> {
    CLOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Record the relay's time from a response's [`SERVER_TIME_HEADER`].
/// Returns whether the stored offset for `origin` changed.
pub(crate) fn note_server_time(origin: &str, headers: &reqwest::header::HeaderMap) -> bool {
    let Some(server) = headers
        .get(SERVER_TIME_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
    else {
        return false;
    };
    note_offset(origin, server - unix_now())
}

fn note_offset(origin: &str, raw: i64) -> bool {
    let offset = if raw.abs() < CLOCK_SKEW_THRESHOLD {
        0
    } else {
        raw
    };
    let mut clock = clock();
    let map = clock.get_or_insert_with(HashMap::new);
    map.insert(origin.to_owned(), offset) != Some(offset)
}

/// Whether the relay's time at `origin` is known.
pub(crate) fn has_server_time(origin: &str) -> bool {
    clock().as_ref().is_some_and(|map| map.contains_key(origin))
}

/// Unix time for proofs sent to `origin`: local time corrected by the
/// relay's clock when they differ by 30 s or more.
pub(crate) fn proof_now(origin: &str) -> i64 {
    let offset = clock()
        .as_ref()
        .and_then(|map| map.get(origin).copied())
        .unwrap_or(0);
    unix_now() + offset
}

/// What a header proof covers.
pub(crate) struct ProofRequest {
    pub purpose: &'static str,
    pub binding: String,
    pub token: Option<zeroize::Zeroizing<String>>,
    pub body: Vec<u8>,
}

/// Sign `request` as an `X-Kuzz-Device-Proof` value at the relay's time.
/// `None` (with a warning) when signing fails.
pub(crate) async fn header_proof(
    key: &DeviceKey,
    origin: &str,
    request: ProofRequest,
) -> Option<String> {
    let key = key.clone();
    let ts = proof_now(origin);
    let signed = tokio::task::spawn_blocking(move || {
        key.header_proof_at(
            ts,
            request.purpose,
            &request.binding,
            request.token.as_deref().map(String::as_str),
            &request.body,
        )
    })
    .await;
    match signed {
        Ok(Ok(proof)) => Some(proof),
        Ok(Err(error)) => {
            eprintln!("kuzz-desktop: warning: device proof not signed: {error}");
            None
        }
        Err(error) => {
            eprintln!("kuzz-desktop: warning: device proof task failed: {error}");
            None
        }
    }
}

/// The device fields of a sign-in completion (`login:<code sha256>`) or a
/// signup (`signup:<ticket sha256>`): `device_pubkey`, `device_key_kind`,
/// `device_proof` (purpose `bind`). `None` when the proof cannot be signed.
pub(crate) async fn login_fields(
    key: &DeviceKey,
    origin: &str,
    binding: String,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let proof = header_proof(
        key,
        origin,
        ProofRequest {
            purpose: "bind",
            binding,
            token: None,
            body: Vec::new(),
        },
    )
    .await?;
    let mut fields = serde_json::Map::new();
    fields.insert("device_pubkey".into(), key.pubkey().to_hex().into());
    fields.insert("device_key_kind".into(), key.kind().as_wire().into());
    fields.insert("device_proof".into(), proof.into());
    Some(fields)
}

/// `<prefix>:<sha256 hex of secret>`, the bind proof binding.
pub(crate) fn hashed_binding(prefix: &str, secret: &str) -> String {
    use sha2::Digest as _;
    format!(
        "{prefix}:{}",
        hex::encode(sha2::Sha256::digest(secret.as_bytes()))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuzz_device_key_pkg::{FileStore, DEVICE_KEY_SCOPE};

    fn test_key(dir: &std::path::Path) -> DeviceKey {
        // A software key: FileStore never touches the TPM off Windows; on
        // Windows open_or_create_file is software-only.
        let _ = FileStore::new(dir);
        DeviceKey::open_or_create_file(&dir.join(format!("{DEVICE_KEY_SCOPE}.key"))).unwrap()
    }

    fn auth_event(kind: u16, keys: &Keys) -> UnsignedEvent {
        EventBuilder::new(nostr::Kind::from(kind), "")
            .tags([nostr::Tag::parse(["relay", "wss://relay.example"]).unwrap()])
            .build(keys.public_key())
    }

    #[test]
    fn tags_only_auth_kinds_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let key = test_key(dir.path());
        let keys = Keys::generate();
        for kind in [22242u16, 27235, 24242] {
            let event = sign_auth_event_with(
                Some(&key),
                EventBuilder::new(nostr::Kind::from(kind), "").tags([nostr::Tag::parse([
                    "relay",
                    "wss://relay.example",
                ])
                .unwrap()]),
                &keys,
            )
            .unwrap();
            event.verify().unwrap();
            let tag = event
                .tags
                .iter()
                .find(|tag| tag.as_slice().first().map(String::as_str) == Some("device"))
                .expect("device tag");
            assert_eq!(tag.as_slice()[1], key.pubkey().to_hex());
            assert_eq!(
                kuzz_auth_pkg::device_proof::verify_event_device_tag(&event).unwrap(),
                Some(key.pubkey())
            );
        }
        let note = tag_event_with(Some(&key), auth_event(1, &keys));
        assert!(note.tags.iter().all(|tag| tag.as_slice()[0] != "device"));
    }

    #[test]
    fn no_key_leaves_the_event_unchanged() {
        let keys = Keys::generate();
        let unsigned = auth_event(22242, &keys);
        let before = unsigned.clone();
        assert_eq!(tag_event_with(None, unsigned), before);
        assert!(current().is_none());
    }

    #[test]
    fn already_tagged_events_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let key = test_key(dir.path());
        let keys = Keys::generate();
        let once = tag_event_with(Some(&key), auth_event(22242, &keys));
        let twice = tag_event_with(Some(&key), once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn clock_offsets_below_threshold_are_ignored() {
        let origin = "https://clock-test.example";
        assert!(!has_server_time(origin));
        note_offset(origin, 5);
        assert!(has_server_time(origin));
        assert!((proof_now(origin) - unix_now()).abs() <= 1);
        assert!(note_offset(origin, 120));
        assert!(!note_offset(origin, 120));
        assert!((proof_now(origin) - unix_now() - 120).abs() <= 1);
    }

    #[test]
    fn hashed_binding_is_prefix_and_sha256_hex() {
        assert_eq!(
            hashed_binding("login", "abc"),
            "login:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
