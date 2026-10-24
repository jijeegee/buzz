//! The desktop app signer: a loopback HTTP signer run by Kuzz Desktop with
//! the app's own device key, for the processes it spawns (`kuzz-acp`) that
//! cannot open that key themselves. On macOS only binaries signed with the
//! app's keychain access group reach its Secure Enclave key; elsewhere a
//! helper normally opens the key itself and never asks.
//!
//! - Address `http://127.0.0.1:<random port>`; every spawn gets its own
//!   secret ([`AppSigner::register`]), bound to one agent and one relay
//!   ([`DeviceScope`]), and passed as [`APP_SIGNER_URL_ENV`] and
//!   [`APP_SIGNER_SECRET_ENV`]. Dropping the lease (the process ended) or
//!   [`AppSigner::stop`] (app exit) revokes it.
//! - `POST /device-tag`: exactly the `kuzz-acp` device broker request
//!   ([`crate::device::request_device_tag`]) and admission
//!   ([`DeviceScope::admit_event`]).
//! - `POST /header-proof`: a [`HeaderProofRequest`] (purpose, binding,
//!   unix time, SHA-256 of the token and of the body; never the token
//!   itself) answered by a [`HeaderProofResponse`] whose `proof` is the
//!   `X-Kuzz-Device-Proof` value. Only purposes [`HEADER_PROOF_PURPOSES`],
//!   only for `<METHOD> <URL>` bindings on the lease's relay
//!   ([`DeviceScope::admit_proof`]).
//!
//! Bounds as in the `kuzz-acp` broker: requests of at most
//! [`MAX_DEVICE_REQUEST_BYTES`] read within [`REQUEST_TIMEOUT`], at most
//! [`MAX_CONNECTIONS`] at once, [`APP_SIGNER_REQUESTS_PER_MINUTE`] per lease,
//! at most [`MAX_APP_SIGNER_LEASES`] live leases. Secrets are compared in
//! constant time and never printed.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use nostr::{JsonUtil as _, PublicKey, UnsignedEvent};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::device::{
    parse_device_request, post_json, render_device_tag, DeviceKey, DeviceScope, Refusal,
    DEVICE_TAG_PATH, MAX_DEVICE_REQUEST_BYTES,
};
use crate::{authorization_matches, http_ok_body, render_status, BrokerError, Secret};

/// App signer URL (`http://127.0.0.1:<port>`), set by Kuzz Desktop.
pub const APP_SIGNER_URL_ENV: &str = "KUZZ_APP_SIGNER_URL";
/// Per-spawn app signer secret, set by Kuzz Desktop.
pub const APP_SIGNER_SECRET_ENV: &str = "KUZZ_APP_SIGNER_SECRET";
/// Every app signer variable. Spawners strip these before injecting their own.
pub const APP_SIGNER_ENV_VARS: [&str; 2] = [APP_SIGNER_URL_ENV, APP_SIGNER_SECRET_ENV];
/// Header proof request path.
pub const HEADER_PROOF_PATH: &str = "/header-proof";
/// Proof purposes the app signer signs: Bearer `/auth/*` calls and token
/// refresh. Never `bind`: binding a new sign-in to this device is the app's
/// (or a login CLI's) own act, not a helper's.
pub const HEADER_PROOF_PURPOSES: [&str; 2] = ["auth", "refresh"];
/// How far a header proof's time may be from the app's clock (callers may
/// correct their stamp to the relay's clock).
pub const MAX_PROOF_SKEW_SECS: i64 = 300;
/// Requests per minute one lease is served before `429` (one `kuzz-acp`
/// and every agent child it forwards for).
pub const APP_SIGNER_REQUESTS_PER_MINUTE: u32 = 600;
/// Most live leases.
pub const MAX_APP_SIGNER_LEASES: usize = 256;
/// Most connections served at once.
pub const MAX_CONNECTIONS: usize = 16;
/// Time allowed to read one request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A header proof request: what [`kuzz_device_key::DeviceKey::header_proof_at`]
/// covers, with the token and body replaced by their SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderProofRequest {
    /// Unix time to stamp the proof with.
    pub ts: i64,
    /// `auth` or `refresh`.
    pub purpose: String,
    /// `<METHOD> <full URL>`.
    pub binding: String,
    /// SHA-256 hex of the token the request carries.
    pub token_sha256: Option<String>,
    /// SHA-256 hex of the request body (`None`: no body).
    pub body_sha256: Option<String>,
}

impl HeaderProofRequest {
    /// The request for a proof over `token` and `body` (empty: none), hashed
    /// here so the token never leaves the caller.
    pub fn new(ts: i64, purpose: &str, binding: &str, token: Option<&str>, body: &[u8]) -> Self {
        Self {
            ts,
            purpose: purpose.to_owned(),
            binding: binding.to_owned(),
            token_sha256: token.map(|token| hex::encode(kuzz_auth::hash_token(token))),
            body_sha256: (!body.is_empty()).then(|| hex::encode(sha2::Sha256::digest(body))),
        }
    }
}

/// The app signer's answer to a header proof request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderProofResponse {
    /// The `X-Kuzz-Device-Proof` value.
    pub proof: String,
}

/// One `POST /header-proof` round trip: the proof for `request`.
pub fn request_header_proof(
    url: &str,
    secret: &Secret,
    request: &HeaderProofRequest,
    timeout: Duration,
) -> Result<String, BrokerError> {
    let body = serde_json::to_string(request)
        .map_err(|_| BrokerError::InvalidResponse("unserializable request".into()))?;
    let raw = post_json(url, secret, HEADER_PROOF_PATH, &body, timeout)?;
    let answer: HeaderProofResponse = serde_json::from_slice(http_ok_body(&raw)?)
        .map_err(|_| BrokerError::InvalidResponse("body is not a header proof".into()))?;
    Ok(answer.proof)
}

fn hash32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    hex::decode(value).ok()?.try_into().ok()
}

/// The admitted hashes of a header proof request.
#[derive(Debug, PartialEq, Eq)]
pub struct AdmittedProof {
    /// SHA-256 of the token.
    pub token_hash: [u8; 32],
    /// SHA-256 of the body, if any.
    pub body_hash: Option<[u8; 32]>,
}

impl DeviceScope {
    /// Whether the app signer may sign `request` at unix time `now`: a
    /// purpose of [`HEADER_PROOF_PURPOSES`], a token hash (and, if any, a
    /// body hash) of 32 bytes, a time within [`MAX_PROOF_SKEW_SECS`], and a
    /// `<METHOD> <http(s) URL>` binding on this scope's relay.
    pub fn admit_proof(
        &self,
        request: &HeaderProofRequest,
        now: i64,
    ) -> Result<AdmittedProof, Refusal> {
        if !HEADER_PROOF_PURPOSES.contains(&request.purpose.as_str()) {
            return Err(Refusal::Kind);
        }
        let token_hash = request
            .token_sha256
            .as_deref()
            .and_then(hash32)
            .ok_or(Refusal::Malformed)?;
        let body_hash = match request.body_sha256.as_deref() {
            Some(value) => Some(hash32(value).ok_or(Refusal::Malformed)?),
            None => None,
        };
        if request.ts.abs_diff(now) > MAX_PROOF_SKEW_SECS.unsigned_abs() {
            return Err(Refusal::Stale);
        }
        let (method, url) = request.binding.split_once(' ').ok_or(Refusal::Malformed)?;
        if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(Refusal::Malformed);
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(Refusal::Origin);
        }
        self.same_relay(Some(kuzz_core::tenant::relay_url_authority(url)))?;
        Ok(AdmittedProof {
            token_hash,
            body_hash,
        })
    }
}

/// Registration refused: [`MAX_APP_SIGNER_LEASES`] leases are live, or the
/// signer has stopped.
#[derive(Debug, thiserror::Error)]
#[error("the app signer is stopped or has {MAX_APP_SIGNER_LEASES} live leases")]
pub struct AppSignerFull;

struct Entry {
    secret: Secret,
    scope: DeviceScope,
    window_start: Instant,
    count: u32,
}

#[derive(Default)]
struct Leases {
    next_id: u64,
    live: HashMap<u64, Entry>,
}

/// The app signer server. Start with [`AppSigner::start`]; it serves until
/// [`AppSigner::stop`].
pub struct AppSigner {
    addr: SocketAddr,
    key: Mutex<Option<DeviceKey>>,
    leases: Mutex<Leases>,
    stopped: AtomicBool,
    connections: AtomicUsize,
}

impl std::fmt::Debug for AppSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppSigner")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// One spawn's app signer secret; dropping it revokes the secret.
pub struct AppSignerLease {
    signer: Arc<AppSigner>,
    id: u64,
    env: Vec<(String, String)>,
}

impl AppSignerLease {
    /// The spawn's app signer env vars ([`APP_SIGNER_ENV_VARS`]).
    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }
}

impl std::fmt::Debug for AppSignerLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppSignerLease")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for AppSignerLease {
    fn drop(&mut self) {
        self.signer.lock_leases().live.remove(&self.id);
        for (_, value) in &mut self.env {
            zeroize::Zeroize::zeroize(value);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

fn render_json(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Decrements the connection count when a handler ends (also on panic).
struct Connection<'a>(&'a AtomicUsize);

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AppSigner {
    /// Bind `127.0.0.1:0` and serve with `key` on a background thread.
    pub fn start(key: DeviceKey) -> std::io::Result<Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let signer = Arc::new(Self {
            addr: listener.local_addr()?,
            key: Mutex::new(Some(key)),
            leases: Mutex::new(Leases::default()),
            stopped: AtomicBool::new(false),
            connections: AtomicUsize::new(0),
        });
        let serving = Arc::clone(&signer);
        std::thread::Builder::new()
            .name("kuzz-app-signer".into())
            .spawn(move || serving.serve(listener))?;
        Ok(signer)
    }

    /// The signer URL (`http://127.0.0.1:<port>`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Whether [`Self::stop`] ran.
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Stop serving: revoke every lease, release the device key handle and
    /// close the listener. Idempotent.
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.lock_leases().live.clear();
        lock(&self.key).take();
        // Wake the accept loop so it sees the flag and drops the listener.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
    }

    /// Sign with `key` from now on (the install's key was rotated). Leases,
    /// URL and secrets stay, so running helpers keep working. Ignored once
    /// stopped.
    pub fn replace_key(&self, key: DeviceKey) {
        let mut slot = lock(&self.key);
        if !self.is_stopped() {
            *slot = Some(key);
        }
    }

    fn lock_leases(&self) -> MutexGuard<'_, Leases> {
        lock(&self.leases)
    }

    /// A new secret for one spawn of `agent` on `relay_url`, valid until the
    /// lease is dropped or the signer stops.
    pub fn register(
        self: &Arc<Self>,
        agent: PublicKey,
        relay_url: &str,
    ) -> Result<AppSignerLease, AppSignerFull> {
        // `Keys::generate` draws 32 bytes from the OS CSPRNG.
        let secret = Secret::new(nostr::Keys::generate().secret_key().to_secret_hex());
        let value = secret.expose().to_owned();
        let mut leases = self.lock_leases();
        if self.is_stopped() || leases.live.len() >= MAX_APP_SIGNER_LEASES {
            return Err(AppSignerFull);
        }
        let id = leases.next_id;
        leases.next_id = leases.next_id.wrapping_add(1);
        leases.live.insert(
            id,
            Entry {
                secret,
                scope: DeviceScope::new(agent, relay_url),
                window_start: Instant::now(),
                count: 0,
            },
        );
        drop(leases);
        Ok(AppSignerLease {
            signer: Arc::clone(self),
            id,
            env: vec![
                (APP_SIGNER_URL_ENV.to_owned(), self.url()),
                (APP_SIGNER_SECRET_ENV.to_owned(), value),
            ],
        })
    }

    fn authorize(&self, authorization: Option<&str>) -> Result<DeviceScope, u16> {
        let mut leases = self.lock_leases();
        let entry = leases
            .live
            .values_mut()
            .find(|entry| authorization_matches(authorization, &entry.secret))
            .ok_or(401u16)?;
        if entry.window_start.elapsed() >= Duration::from_secs(60) {
            entry.window_start = Instant::now();
            entry.count = 0;
        }
        if entry.count >= APP_SIGNER_REQUESTS_PER_MINUTE {
            return Err(429);
        }
        entry.count += 1;
        Ok(entry.scope.clone())
    }

    /// The raw HTTP response for one request.
    fn respond(&self, raw: &[u8]) -> Vec<u8> {
        let status = |code: u16, reason: &str| render_status(code, reason).into_bytes();
        let Some(request) = parse_device_request(raw) else {
            return status(400, "Bad Request");
        };
        let tag = request.path == DEVICE_TAG_PATH;
        if request.method != "POST" || !(tag || request.path == HEADER_PROOF_PATH) {
            return status(404, "Not Found");
        }
        let scope = match self.authorize(request.authorization.as_deref()) {
            Ok(scope) => scope,
            Err(429) => return status(429, "Too Many Requests"),
            Err(_) => return status(401, "Unauthorized"),
        };
        let Some(key) = lock(&self.key).clone() else {
            return status(503, "Service Unavailable");
        };
        let body = std::str::from_utf8(&request.body).ok();
        if tag {
            let unsigned = body.and_then(|body| UnsignedEvent::from_json(body).ok());
            let Some(unsigned) = unsigned else {
                return status(403, "Forbidden");
            };
            if scope
                .admit_event(&unsigned, unix_now().unsigned_abs())
                .is_err()
            {
                return status(403, "Forbidden");
            }
            let tag = key.add_event_tag(unsigned).ok().and_then(|tagged| {
                tagged.tags.iter().find_map(|tag| {
                    let values = tag.as_slice();
                    (values.first().map(String::as_str) == Some("device")).then(|| values.to_vec())
                })
            });
            return match tag {
                Some(tag) => render_device_tag(&tag).into_bytes(),
                None => status(500, "Internal Server Error"),
            };
        }
        let proof_request =
            body.and_then(|body| serde_json::from_str::<HeaderProofRequest>(body).ok());
        let Some(proof_request) = proof_request else {
            return status(403, "Forbidden");
        };
        let Ok(admitted) = scope.admit_proof(&proof_request, unix_now()) else {
            return status(403, "Forbidden");
        };
        let signed = key.header_proof_hashed_at(
            proof_request.ts,
            &proof_request.purpose,
            &proof_request.binding,
            Some(&admitted.token_hash),
            admitted.body_hash.as_ref(),
        );
        let answer = signed
            .ok()
            .and_then(|proof| serde_json::to_string(&HeaderProofResponse { proof }).ok());
        match answer {
            Some(answer) => render_json(&answer),
            None => status(500, "Internal Server Error"),
        }
    }

    fn serve(self: Arc<Self>, listener: TcpListener) {
        for stream in listener.incoming() {
            if self.is_stopped() {
                break;
            }
            let Ok(stream) = stream else {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            if !stream.peer_addr().is_ok_and(|peer| peer.ip().is_loopback()) {
                continue;
            }
            if self.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                self.connections.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let signer = Arc::clone(&self);
            let spawned = std::thread::Builder::new()
                .name("kuzz-app-signer-conn".into())
                .spawn(move || {
                    let _connection = Connection(&signer.connections);
                    signer.handle(stream);
                });
            if spawned.is_err() {
                self.connections.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    fn handle(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(REQUEST_TIMEOUT));
        let _ = stream.set_write_timeout(Some(REQUEST_TIMEOUT));
        let response = match read_request(&mut stream) {
            Some(request) => self.respond(&request),
            None => render_status(400, "Bad Request").into_bytes(),
        };
        let _ = stream.write_all(&response);
        let _ = stream.shutdown(Shutdown::Both);
    }
}

/// Read one whole request, at most [`MAX_DEVICE_REQUEST_BYTES`] within
/// [`REQUEST_TIMEOUT`].
fn read_request(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let started = Instant::now();
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        if started.elapsed() > REQUEST_TIMEOUT {
            return None;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_DEVICE_REQUEST_BYTES {
            return None;
        }
        if parse_device_request(&buf).is_some() {
            return Some(buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceSignError, DeviceSigner};
    use crate::DEFAULT_BROKER_TIMEOUT;
    use kuzz_auth::device_proof::{verify_event_device_tag, DeviceProof, ProofScope};
    use nostr::{EventBuilder, Keys, Kind, Tag};

    const RELAY: &str = "wss://relay.example";

    fn file_key(dir: &tempfile::TempDir) -> DeviceKey {
        DeviceKey::open_or_create_file(&dir.path().join("device.key")).unwrap()
    }

    fn nip98(keys: &Keys, url: &str) -> UnsignedEvent {
        EventBuilder::new(Kind::HttpAuth, "")
            .tags([
                Tag::parse(["u", url]).unwrap(),
                Tag::parse(["method", "POST"]).unwrap(),
            ])
            .build(keys.public_key())
    }

    fn app(lease: &AppSignerLease) -> DeviceSigner {
        let get = |name: &str| {
            lease
                .env()
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap()
        };
        DeviceSigner::App {
            url: get(APP_SIGNER_URL_ENV),
            secret: Secret::new(get(APP_SIGNER_SECRET_ENV)),
        }
    }

    fn verifies(key: &DeviceKey, proof: &str, binding: &str, token: &str, body: &[u8]) -> bool {
        let token_hash = kuzz_auth::hash_token(token);
        let body_hash: Option<[u8; 32]> =
            (!body.is_empty()).then(|| sha2::Sha256::digest(body).into());
        let scope = ProofScope {
            purpose: "refresh",
            binding,
            token_hash: Some(&token_hash),
            body_hash: body_hash.as_ref(),
        };
        DeviceProof::parse(proof)
            .unwrap()
            .verify(&key.pubkey(), &scope, unix_now())
            .is_ok()
    }

    #[test]
    fn the_app_signer_signs_tags_and_header_proofs_that_verify() {
        let dir = tempfile::tempdir().unwrap();
        let key = file_key(&dir);
        let agent = Keys::generate();
        let signer = AppSigner::start(key.clone()).unwrap();
        let lease = signer.register(agent.public_key(), RELAY).unwrap();
        let client = app(&lease);

        let tagged = client
            .tag_event(&nip98(&agent, "https://relay.example/query"))
            .unwrap();
        let event = tagged.sign_with_keys(&agent).unwrap();
        assert_eq!(verify_event_device_tag(&event).unwrap(), Some(key.pubkey()));

        let binding = "POST https://relay.example/auth/refresh";
        let body = br#"{"refresh_token":"r"}"#;
        let proof = client
            .header_proof_at(unix_now(), "refresh", binding, Some("bzs_tok"), body)
            .unwrap();
        assert!(verifies(&key, &proof, binding, "bzs_tok", body));
        assert!(!verifies(&key, &proof, binding, "bzs_other", body));
        signer.stop();
    }

    #[test]
    fn a_replaced_key_signs_for_the_leases_already_handed_out() {
        let dir = tempfile::tempdir().unwrap();
        let old = file_key(&dir);
        let new = DeviceKey::open_or_create_file(&dir.path().join("new.key")).unwrap();
        let agent = Keys::generate();
        let signer = AppSigner::start(old.clone()).unwrap();
        let lease = signer.register(agent.public_key(), RELAY).unwrap();
        let client = app(&lease);
        signer.replace_key(new.clone());

        let binding = "POST https://relay.example/auth/refresh";
        let proof = client
            .header_proof_at(unix_now(), "refresh", binding, Some("bzs_tok"), b"")
            .unwrap();
        assert!(verifies(&new, &proof, binding, "bzs_tok", b""));
        assert!(!verifies(&old, &proof, binding, "bzs_tok", b""));

        // A stopped signer never takes a key back.
        signer.stop();
        signer.replace_key(new);
        assert!(lock(&signer.key).is_none());
    }

    #[test]
    fn a_wrong_or_revoked_secret_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Keys::generate();
        let signer = AppSigner::start(file_key(&dir)).unwrap();
        let lease = signer.register(agent.public_key(), RELAY).unwrap();
        let wrong = DeviceSigner::App {
            url: signer.url(),
            secret: Secret::new("not-the-secret".into()),
        };
        let unsigned = nip98(&agent, "https://relay.example/query");
        assert!(matches!(
            wrong.tag_event(&unsigned),
            Err(DeviceSignError::Broker(BrokerError::Status(401)))
        ));
        assert!(matches!(
            wrong.header_proof_at(
                unix_now(),
                "auth",
                "GET https://relay.example/auth/me",
                Some("t"),
                b""
            ),
            Err(DeviceSignError::Broker(BrokerError::Status(401)))
        ));

        let client = app(&lease);
        assert!(client.tag_event(&unsigned).is_ok());
        drop(lease);
        assert!(matches!(
            client.tag_event(&unsigned),
            Err(DeviceSignError::Broker(BrokerError::Status(401)))
        ));

        let lease = signer.register(agent.public_key(), RELAY).unwrap();
        let client = app(&lease);
        signer.stop();
        assert!(client.tag_event(&unsigned).is_err());
        assert!(signer.register(agent.public_key(), RELAY).is_err());
    }

    #[test]
    fn the_app_signer_refuses_what_the_agent_may_not_send_here() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Keys::generate();
        let signer = AppSigner::start(file_key(&dir)).unwrap();
        let lease = signer.register(agent.public_key(), RELAY).unwrap();
        let client = app(&lease);

        let note = EventBuilder::text_note("hi").build(agent.public_key());
        let other_author = nip98(&Keys::generate(), "https://relay.example/query");
        let other_origin = nip98(&agent, "https://evil.example/query");
        for unsigned in [note, other_author, other_origin] {
            assert!(matches!(
                client.tag_event(&unsigned),
                Err(DeviceSignError::Broker(BrokerError::Status(403)))
            ));
        }
        let now = unix_now();
        let me = "GET https://relay.example/auth/me";
        for (ts, purpose, binding, token) in [
            (now, "bind", "login:abcd", None),
            (now, "auth", me, None),
            (now, "unknown", me, Some("t")),
            (now, "auth", "GET https://evil.example/auth/me", Some("t")),
            (now, "auth", "GET wss://relay.example/auth/me", Some("t")),
            (now, "auth", "get https://relay.example/auth/me", Some("t")),
            (now - 3600, "auth", me, Some("t")),
        ] {
            assert!(
                matches!(
                    client.header_proof_at(ts, purpose, binding, token, b""),
                    Err(DeviceSignError::Broker(BrokerError::Status(403)))
                ),
                "{purpose} {binding}"
            );
        }
        assert!(client
            .header_proof_at(now, "auth", me, Some("t"), b"")
            .is_ok());
        signer.stop();
    }

    #[test]
    fn header_proof_requests_carry_hashes_never_the_token() {
        let request = HeaderProofRequest::new(1, "auth", "GET u", Some("bzs_topsecret"), b"body");
        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains("topsecret"));
        assert_eq!(
            request.token_sha256.as_deref(),
            Some(hex::encode(kuzz_auth::hash_token("bzs_topsecret")).as_str())
        );
        let empty = HeaderProofRequest::new(1, "auth", "GET u", Some("t"), b"");
        assert!(empty.body_sha256.is_none());
    }

    #[test]
    fn unknown_paths_methods_and_oversize_requests_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let signer = AppSigner::start(file_key(&dir)).unwrap();
        let send = |raw: &[u8]| {
            let mut stream = TcpStream::connect(signer.addr).unwrap();
            stream
                .set_read_timeout(Some(DEFAULT_BROKER_TIMEOUT))
                .unwrap();
            let _ = stream.write_all(raw);
            let mut answer = Vec::new();
            let _ = stream.read_to_end(&mut answer);
            String::from_utf8_lossy(&answer).into_owned()
        };
        assert!(send(b"GET /device-tag HTTP/1.1\r\nContent-Length: 0\r\n\r\n").contains(" 404 "));
        assert!(send(b"POST /token HTTP/1.1\r\nContent-Length: 0\r\n\r\n").contains(" 404 "));
        // Exactly one byte over the limit, so the server reads all of it.
        let mut big = b"POST /device-tag HTTP/1.1\r\nContent-Length: 99999\r\n\r\n".to_vec();
        big.resize(MAX_DEVICE_REQUEST_BYTES + 1, b'x');
        assert!(send(&big).contains(" 400 "));
        signer.stop();
    }
}
