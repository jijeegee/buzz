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
use kuzz_device_key_pkg::{FileStore, DEVICE_KEY_SCOPE};
use nostr::{EventBuilder, Keys, UnsignedEvent};

/// Header carrying a request's device proof.
pub(crate) const PROOF_HEADER: &str = "X-Kuzz-Device-Proof";
/// Header carrying the relay's unix time on `/auth/*` responses.
pub(crate) const SERVER_TIME_HEADER: &str = "x-kuzz-server-time";

/// Auth event kinds that carry the `device` tag.
const TAGGED_KINDS: [u16; 3] = [22242, 27235, 24242];

/// Retry opening a failed key no sooner than this.
const OPEN_RETRY: Duration = Duration::from_secs(30);
/// Longest wait between background retries of a failed open.
const OPEN_RETRY_MAX: Duration = Duration::from_secs(600);

/// Longest a user action (sign-in, a request) waits for the device key or
/// one device signature; past it, it goes on as if there were no key.
pub(crate) const USER_WAIT: Duration = Duration::from_secs(2);

/// Clock offsets below this are treated as no offset (mobile: 30 s).
const CLOCK_SKEW_THRESHOLD: i64 = 30;

#[derive(Default)]
struct Slot {
    key: Option<DeviceKey>,
    failed_at: Option<Instant>,
}

static SLOT: Mutex<Slot> = Mutex::new(Slot {
    key: None,
    failed_at: None,
});

/// Serializes key creation so two callers never race `open_or_create`.
static OPENING: Mutex<()> = Mutex::new(());

static CLOCK: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);
/// Origins whose server time was already asked for (answered or not).
static TIME_ASKED: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

fn slot() -> std::sync::MutexGuard<'static, Slot> {
    SLOT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Open (or create) the device key in the background, so the first signer
/// does not wait for the TPM. A failed open is retried in the background
/// (after [`OPEN_RETRY`], then less often); `on_ready` runs once it is open.
pub(crate) fn init(on_ready: impl FnOnce() + Send + 'static) {
    if cfg!(test) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("kuzz-device-key".into())
        .spawn(move || {
            let mut wait = OPEN_RETRY;
            while ensure().is_none() {
                std::thread::sleep(wait);
                wait = (wait * 2).min(OPEN_RETRY_MAX);
            }
            on_ready();
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
    {
        let slot = slot();
        if let Some(key) = &slot.key {
            return Some(key.clone());
        }
        if slot.failed_at.is_some_and(|at| at.elapsed() < OPEN_RETRY) {
            return None;
        }
    }
    let Some(dir) = shared_store_dir() else {
        eprintln!("kuzz-desktop: warning: device key unavailable: no local data directory");
        slot().failed_at = Some(Instant::now());
        return None;
    };
    match DeviceKey::open_or_create(DEVICE_KEY_SCOPE, &FileStore::new(dir)) {
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

/// [`ensure`] off the async runtime, waiting at most `limit` (a TPM that
/// hangs, or a startup open holding the lock). `None` with a warning when
/// the key is not ready in time; the open goes on in the background.
pub(crate) async fn ensure_within(limit: Duration) -> Option<DeviceKey> {
    if let Some(key) = current() {
        return Some(key);
    }
    if cfg!(test) {
        return None;
    }
    match tokio::time::timeout(limit, tokio::task::spawn_blocking(ensure)).await {
        Ok(opened) => opened.ok().flatten(),
        Err(_) => {
            eprintln!("kuzz-desktop: warning: device key not ready in time; going on without it");
            None
        }
    }
}

/// The software fallback store (and TPM marker) every Kuzz app of this OS
/// user shares with the CLI and `kuzz-acp`: `<local data dir>/kuzz/device-keys`
/// (`%LOCALAPPDATA%` on Windows, `~/Library/Application Support` on macOS,
/// `$XDG_DATA_HOME` or `~/.local/share` elsewhere). Same rule as
/// `kuzz_token_broker::device::default_store_dir`, so one machine has one
/// device key.
pub(crate) fn shared_store_dir() -> Option<PathBuf> {
    shared_store_dir_from(|name| std::env::var_os(name))
}

/// [`shared_store_dir`] reading variables through `var`. Pure.
fn shared_store_dir_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let var = |name: &str| {
        var(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let base = if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library").join("Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|home| home.join(".local").join("share")))
    };
    base.map(|base| base.join("kuzz").join("device-keys"))
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
    let kind = original.kind.as_u16();
    // On its own thread, so a TPM that hangs costs a caller at most
    // USER_WAIT (the event then goes out as before).
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let signer = key.clone();
    let spawned = std::thread::Builder::new()
        .name("kuzz-device-tag".into())
        .spawn(move || {
            let _ = sender.send(signer.add_event_tag(unsigned));
        });
    if let Err(error) = spawned {
        eprintln!("kuzz-desktop: warning: device tag not added (kind {kind}): {error}");
        return original;
    }
    match run_blocking(|| receiver.recv_timeout(USER_WAIT)) {
        Ok(Ok(tagged)) => tagged,
        Ok(Err(error)) => {
            eprintln!("kuzz-desktop: warning: device tag not added (kind {kind}): {error}");
            original
        }
        Err(_) => {
            eprintln!("kuzz-desktop: warning: device tag not added in time (kind {kind})");
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

/// [`origin_of`] for a configured origin string (any path or trailing
/// slash dropped), so every clock lookup uses one spelling.
pub(crate) fn normalize_origin(origin: &str) -> String {
    match url::Url::parse(origin) {
        Ok(url) => origin_of(&url),
        Err(_) => origin.trim_end_matches('/').to_ascii_lowercase(),
    }
}

/// Whether the relay's time at `origin` has never been asked for this run;
/// marks it asked. One ask per origin, answered or not.
pub(crate) fn first_time_ask(origin: &str) -> bool {
    TIME_ASKED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(Default::default)
        .insert(normalize_origin(origin))
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
    map.insert(normalize_origin(origin), offset) != Some(offset)
}

/// Whether the relay's time at `origin` is known.
pub(crate) fn has_server_time(origin: &str) -> bool {
    let origin = normalize_origin(origin);
    clock()
        .as_ref()
        .is_some_and(|map| map.contains_key(&origin))
}

/// Unix time for proofs sent to `origin`: local time corrected by the
/// relay's clock when they differ by 30 s or more.
pub(crate) fn proof_now(origin: &str) -> i64 {
    let origin = normalize_origin(origin);
    let offset = clock()
        .as_ref()
        .and_then(|map| map.get(&origin).copied())
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
    let signing = tokio::task::spawn_blocking(move || {
        key.header_proof_at(
            ts,
            request.purpose,
            &request.binding,
            request.token.as_deref().map(String::as_str),
            &request.body,
        )
    });
    match tokio::time::timeout(USER_WAIT, signing).await {
        Ok(Ok(Ok(proof))) => Some(proof),
        Ok(Ok(Err(error))) => {
            eprintln!("kuzz-desktop: warning: device proof not signed: {error}");
            None
        }
        Ok(Err(error)) => {
            eprintln!("kuzz-desktop: warning: device proof task failed: {error}");
            None
        }
        Err(_) => {
            eprintln!("kuzz-desktop: warning: device proof not signed in time");
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
    fn the_store_is_the_one_the_cli_and_runner_use() {
        let base = if cfg!(windows) {
            r"C:\Users\u\AppData\Local"
        } else {
            "/home/u/.data"
        };
        let var = |name: &str| {
            let wanted = if cfg!(windows) {
                "LOCALAPPDATA"
            } else if cfg!(target_os = "macos") {
                "HOME"
            } else {
                "XDG_DATA_HOME"
            };
            (name == wanted).then(|| std::ffi::OsString::from(base))
        };
        let mut expected = PathBuf::from(base);
        if cfg!(target_os = "macos") {
            expected = expected.join("Library").join("Application Support");
        }
        assert_eq!(
            shared_store_dir_from(var),
            Some(expected.join("kuzz").join("device-keys"))
        );
        // A relative or empty base is never used.
        assert_eq!(shared_store_dir_from(|_| Some("rel".into())), None);
        assert_eq!(shared_store_dir_from(|_| Some("".into())), None);
    }

    #[test]
    fn clock_and_time_asks_use_one_origin_spelling() {
        assert_eq!(
            normalize_origin("https://Norm-Test.example/"),
            "https://norm-test.example"
        );
        note_offset("https://norm-test.example/some/path", 300);
        assert!(has_server_time("https://NORM-TEST.example"));
        assert!(first_time_ask("https://ask-test.example/"));
        assert!(!first_time_ask("https://ask-test.example"));
    }

    #[test]
    fn hashed_binding_is_prefix_and_sha256_hex() {
        assert_eq!(
            hashed_binding("login", "abc"),
            "login:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
