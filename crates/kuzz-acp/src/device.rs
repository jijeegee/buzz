//! Device-bound keys in the harness (device-bound keys plan §6, review S2).
//!
//! In key mode the harness opens this OS user's shared device key (never
//! creating it: only `kuzz auth login` and the desktop app do) and:
//!
//! - adds the `device` tag to its own auth events: NIP-42 AUTH and the NIP-98
//!   events of the HTTP bridge ([`tag`]);
//! - asks `GET /agents/{pubkey}/status` before starting, and stays down when
//!   the relay says the agent is bound to another device ([`check_status`]);
//! - runs a loopback **device broker** ([`DeviceBroker`]) so agent children
//!   that cannot open the key themselves get device tags without touching it.
//!
//! No key, or a failed signature, is a warning: the event goes out exactly as
//! it did before and the relay decides (it refuses only under `enforce`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use kuzz_token_broker::device::{
    open_shared, parse_device_request, render_device_tag, DeviceKey, OpenMode,
    DEVICE_BROKER_SECRET_ENV, DEVICE_BROKER_URL_ENV, DEVICE_ENV_VARS, DEVICE_TAGGED_KINDS,
    DEVICE_TAG_PATH, MAX_DEVICE_REQUEST_BYTES,
};
use kuzz_token_broker::{authorization_matches, render_status, Secret, BROKER_REQUESTS_PER_MINUTE};
use nostr::{JsonUtil as _, PublicKey, UnsignedEvent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use crate::token_broker::{BrokerFull, MAX_CONCURRENT, MAX_SECRETS, REQUEST_TIMEOUT};

/// Name of the event tag carrying the device signature.
const DEVICE_TAG: &str = "device";
/// How far a brokered event's `created_at` may be from now.
const BROKER_MAX_SKEW_SECS: u64 = 60;
/// Time allowed for the start-up status check.
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
/// The relay's reason for a key bound to another device.
const WRONG_HOST: &str = "(wrong_host)";
/// The relay mode under which a refusal is binding.
const ENFORCE: &str = "enforce";

/// Process exit code when the relay binds this agent to another device
/// under `enforce` (`EX_NOPERM`). Kuzz Desktop shows "Runs on another
/// device" and must not restart the agent on it; nothing in kuzz-acp
/// restarts it either (the harness stops before any agent child spawns).
pub const EXIT_WRONG_HOST: i32 = 77;

static WRONG_HOST_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask the process to exit with [`EXIT_WRONG_HOST`] once the harness returns.
pub fn request_wrong_host_exit() {
    WRONG_HOST_STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Whether [`request_wrong_host_exit`] was called.
pub fn wrong_host_exit_requested() -> bool {
    WRONG_HOST_STOP.load(std::sync::atomic::Ordering::SeqCst)
}

static DEVICE: OnceLock<DeviceKey> = OnceLock::new();

/// Open the shared device key (existing only, on a blocking thread) and make
/// it this process's device signer. `None`, with a log line, when there is
/// none or it cannot be opened.
pub async fn init() -> Option<&'static DeviceKey> {
    if let Some(key) = DEVICE.get() {
        return Some(key);
    }
    match tokio::task::spawn_blocking(|| open_shared(OpenMode::ExistingOnly)).await {
        Ok(Ok(Some(key))) => {
            tracing::info!(
                device = %key.pubkey().to_hex(),
                kind = key.kind().as_wire(),
                "device key opened"
            );
            Some(DEVICE.get_or_init(|| key))
        }
        Ok(Ok(None)) => {
            tracing::info!("no device key on this machine yet; signing without one");
            None
        }
        Ok(Err(error)) => {
            tracing::warn!("device key unavailable; signing without it: {error}");
            None
        }
        Err(error) => {
            tracing::warn!("device key task failed; signing without it: {error}");
            None
        }
    }
}

/// This process's device key, once [`init`] opened it.
pub fn current() -> Option<&'static DeviceKey> {
    DEVICE.get()
}

/// `unsigned` with this process's device tag, or unchanged.
pub async fn tag(unsigned: UnsignedEvent) -> UnsignedEvent {
    tag_with(current(), unsigned).await
}

/// `unsigned` with `key`'s device tag (on a blocking thread, the TPM takes
/// ~13 ms), or unchanged with a warning when it cannot be tagged.
pub async fn tag_with(key: Option<&DeviceKey>, unsigned: UnsignedEvent) -> UnsignedEvent {
    let Some(key) = key.cloned() else {
        return unsigned;
    };
    let original = unsigned.clone();
    let task = tokio::task::spawn_blocking(move || key.add_event_tag(unsigned));
    match tokio::time::timeout(kuzz_token_broker::device::DEVICE_SIGN_TIMEOUT, task).await {
        Ok(Ok(Ok(tagged))) => tagged,
        Ok(Ok(Err(error))) => {
            tracing::warn!("device tag failed; sending without it: {error}");
            original
        }
        Ok(Err(error)) => {
            tracing::warn!("device tag task failed; sending without it: {error}");
            original
        }
        Err(_) => {
            tracing::warn!("device tag timed out; sending without it");
            original
        }
    }
}

// ── Start-up admission check ──────────────────────────────────────────────

/// What the relay said about starting this agent here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Start (admitted, admission off, or no usable answer).
    Start,
    /// The agent is bound to another device: do not start here.
    OtherDevice(String),
}

/// `GET /agents/{pubkey}/status` with a device-tagged NIP-98 event. Only a
/// `403` naming `wrong_host` stops the start; every other outcome (admission
/// off, admitted, an older relay without the route, a network error) starts
/// as before, with a warning when the answer was unusable.
pub async fn check_status(
    http: &reqwest::Client,
    http_base: &str,
    keys: &nostr::Keys,
    device: Option<&DeviceKey>,
) -> Admission {
    let url = format!(
        "{}/agents/{}/status",
        http_base.trim_end_matches('/'),
        keys.public_key().to_hex()
    );
    let header = match nip98_header(keys, device, "GET", &url).await {
        Ok(header) => header,
        Err(error) => {
            tracing::warn!("agent status check skipped: {error}");
            return Admission::Start;
        }
    };
    let response = match http
        .get(&url)
        .header("Authorization", header)
        .timeout(STATUS_TIMEOUT)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!("agent status check failed; starting anyway: {error}");
            return Admission::Start;
        }
    };
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    admission_of(status, &body)
}

/// [`Admission`] for a status answer.
/// Only a binding refusal stops the start: `403`, reason `wrong_host`, and
/// `"admission": "enforce"`. Under `log` (or a relay that does not name its
/// mode) the relay still admits the agent, so it starts as before.
fn admission_of(status: u16, body: &str) -> Admission {
    if status == 403 && body.contains(WRONG_HOST) {
        let value = serde_json::from_str::<serde_json::Value>(body).ok();
        let mode = value
            .as_ref()
            .and_then(|value| value.get("admission")?.as_str());
        if mode == Some(ENFORCE) {
            let message = value
                .as_ref()
                .and_then(|value| {
                    ["message", "error"]
                        .iter()
                        .find_map(|field| value.get(field)?.as_str().map(str::to_owned))
                })
                .unwrap_or_else(|| "the agent key is bound to another device".to_owned());
            return Admission::OtherDevice(message);
        }
        tracing::warn!(
            mode = mode.unwrap_or("unnamed"),
            "the relay reports this agent bound to another device; starting anyway (not enforced)"
        );
        return Admission::Start;
    }
    if !(200..300).contains(&status) {
        tracing::warn!(
            status,
            "agent status check was not answered; starting anyway"
        );
    }
    Admission::Start
}

/// `Nostr <base64>` for a NIP-98 event over `method url`, device-tagged.
async fn nip98_header(
    keys: &nostr::Keys,
    device: Option<&DeviceKey>,
    method: &str,
    url: &str,
) -> Result<String, String> {
    use base64::Engine as _;
    use nostr::{EventBuilder, Kind, Tag};
    let tags = vec![
        Tag::parse(["u", url]).map_err(|e| e.to_string())?,
        Tag::parse(["method", method]).map_err(|e| e.to_string())?,
        Tag::parse(["nonce", &uuid::Uuid::new_v4().to_string()]).map_err(|e| e.to_string())?,
    ];
    let unsigned = EventBuilder::new(Kind::HttpAuth, "")
        .tags(tags)
        .build(keys.public_key());
    let event = tag_with(device, unsigned)
        .await
        .sign_with_keys(keys)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(event.as_json())
    ))
}

// ── Device broker ─────────────────────────────────────────────────────────

struct SpawnSecret {
    secret: Secret,
    window_start: Instant,
    count: u32,
}

#[derive(Default)]
struct Secrets {
    next_id: u64,
    live: HashMap<u64, SpawnSecret>,
}

/// Loopback device broker: `POST /device-tag` with a spawn secret and an
/// unsigned auth event answers the event's device tag. It signs only what
/// this harness's agent may send to this relay (review S2): one of
/// [`DEVICE_TAGGED_KINDS`], authored by the agent, fresh, without a device
/// tag, and aimed at this relay (`relay`, `u` or `server` tag). It never
/// signs header proofs. Bounds as in [`crate::token_broker`].
pub struct DeviceBroker {
    addr: SocketAddr,
    key: DeviceKey,
    agent: PublicKey,
    relay_authority: String,
    secrets: Mutex<Secrets>,
}

impl std::fmt::Debug for DeviceBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceBroker")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

/// One live device broker secret; dropping it unregisters the secret.
pub struct DeviceLease {
    broker: Arc<DeviceBroker>,
    id: u64,
    env: Vec<(String, String)>,
}

impl DeviceLease {
    /// The child's device broker env vars.
    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }
}

impl std::fmt::Debug for DeviceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceLease")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for DeviceLease {
    fn drop(&mut self) {
        self.broker.lock_secrets().live.remove(&self.id);
        for (_, value) in &mut self.env {
            zeroize::Zeroize::zeroize(value);
        }
    }
}

static BROKER: OnceLock<Arc<DeviceBroker>> = OnceLock::new();

/// The process device broker, once [`start_global`] started it.
pub fn global() -> Option<&'static Arc<DeviceBroker>> {
    BROKER.get()
}

/// Start the process device broker for `agent` on `relay_url` (key mode with
/// an open device key). Idempotent.
pub async fn start_global(
    key: DeviceKey,
    agent: PublicKey,
    relay_url: &str,
) -> std::io::Result<&'static Arc<DeviceBroker>> {
    if let Some(broker) = BROKER.get() {
        return Ok(broker);
    }
    let broker = DeviceBroker::start(key, agent, relay_url).await?;
    Ok(BROKER.get_or_init(|| broker))
}

fn random_secret() -> Secret {
    // `Keys::generate` draws 32 bytes from the OS CSPRNG.
    Secret::new(nostr::Keys::generate().secret_key().to_secret_hex())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Why the broker refuses to tag an event (answered as `403`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Malformed,
    Kind,
    Author,
    Stale,
    AlreadyTagged,
    Origin,
}

impl DeviceBroker {
    /// Bind `127.0.0.1:0` and serve until the process exits.
    pub async fn start(
        key: DeviceKey,
        agent: PublicKey,
        relay_url: &str,
    ) -> std::io::Result<Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let broker = Arc::new(Self {
            addr: listener.local_addr()?,
            key,
            agent,
            relay_authority: kuzz_core::tenant::relay_url_authority(relay_url),
            secrets: Mutex::new(Secrets::default()),
        });
        let serving = Arc::clone(&broker);
        tokio::spawn(async move { serving.serve(listener).await });
        Ok(broker)
    }

    /// The broker URL (`http://127.0.0.1:<port>`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn lock_secrets(&self) -> std::sync::MutexGuard<'_, Secrets> {
        self.secrets.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Register a spawn secret, valid until the lease is dropped.
    pub fn register(self: &Arc<Self>) -> Result<DeviceLease, BrokerFull> {
        let secret = random_secret();
        let value = secret.expose().to_owned();
        let mut secrets = self.lock_secrets();
        if secrets.live.len() >= MAX_SECRETS {
            return Err(BrokerFull);
        }
        let id = secrets.next_id;
        secrets.next_id = secrets.next_id.wrapping_add(1);
        secrets.live.insert(
            id,
            SpawnSecret {
                secret,
                window_start: Instant::now(),
                count: 0,
            },
        );
        drop(secrets);
        Ok(DeviceLease {
            broker: Arc::clone(self),
            id,
            env: vec![
                (DEVICE_BROKER_URL_ENV.to_owned(), self.url()),
                (DEVICE_BROKER_SECRET_ENV.to_owned(), value),
            ],
        })
    }

    fn authorize(&self, authorization: Option<&str>) -> Result<(), u16> {
        let mut secrets = self.lock_secrets();
        let entry = secrets
            .live
            .values_mut()
            .find(|entry| authorization_matches(authorization, &entry.secret))
            .ok_or(401u16)?;
        if entry.window_start.elapsed() >= Duration::from_secs(60) {
            entry.window_start = Instant::now();
            entry.count = 0;
        }
        if entry.count >= BROKER_REQUESTS_PER_MINUTE {
            return Err(429);
        }
        entry.count += 1;
        Ok(())
    }

    /// Whether this broker may tag `unsigned` (review S2).
    fn admit(&self, unsigned: &UnsignedEvent, now: u64) -> Result<(), Refusal> {
        let kind = unsigned.kind.as_u16();
        if !DEVICE_TAGGED_KINDS.contains(&kind) {
            return Err(Refusal::Kind);
        }
        if unsigned.pubkey != self.agent {
            return Err(Refusal::Author);
        }
        if unsigned.created_at.as_secs().abs_diff(now) > BROKER_MAX_SKEW_SECS {
            return Err(Refusal::Stale);
        }
        let first = |name: &str| {
            unsigned.tags.iter().find_map(|tag| {
                let values = tag.as_slice();
                (values.first().map(String::as_str) == Some(name))
                    .then(|| values.get(1).cloned())
                    .flatten()
            })
        };
        if unsigned
            .tags
            .iter()
            .any(|tag| tag.as_slice().first().map(String::as_str) == Some(DEVICE_TAG))
        {
            return Err(Refusal::AlreadyTagged);
        }
        let authority = match kind {
            22242 => first("relay").map(|url| kuzz_core::tenant::relay_url_authority(&url)),
            27235 => first("u").map(|url| kuzz_core::tenant::relay_url_authority(&url)),
            _ => first("server").map(|server| kuzz_core::tenant::normalize_host(&server)),
        };
        match authority {
            Some(authority) if !authority.is_empty() && authority == self.relay_authority => Ok(()),
            _ => Err(Refusal::Origin),
        }
    }

    /// The raw HTTP response for one request.
    async fn respond(&self, request: &[u8]) -> Vec<u8> {
        let Some(request) = parse_device_request(request) else {
            return render_status(400, "Bad Request").into_bytes();
        };
        if request.method != "POST" || request.path != DEVICE_TAG_PATH {
            return render_status(404, "Not Found").into_bytes();
        }
        match self.authorize(request.authorization.as_deref()) {
            Ok(()) => {}
            Err(429) => return render_status(429, "Too Many Requests").into_bytes(),
            Err(_) => return render_status(401, "Unauthorized").into_bytes(),
        }
        let unsigned = std::str::from_utf8(&request.body)
            .ok()
            .and_then(|body| UnsignedEvent::from_json(body).ok());
        let admitted = match &unsigned {
            Some(unsigned) => self.admit(unsigned, unix_now()),
            None => Err(Refusal::Malformed),
        };
        let unsigned = match (unsigned, admitted) {
            (Some(unsigned), Ok(())) => unsigned,
            (_, Err(refusal)) => {
                tracing::warn!(?refusal, "device broker refused an event");
                return render_status(403, "Forbidden").into_bytes();
            }
            (None, Ok(())) => return render_status(400, "Bad Request").into_bytes(),
        };
        let key = self.key.clone();
        let tagged = tokio::task::spawn_blocking(move || key.add_event_tag(unsigned)).await;
        let tag = match tagged {
            Ok(Ok(tagged)) => tagged.tags.iter().find_map(|tag| {
                let values = tag.as_slice();
                (values.first().map(String::as_str) == Some(DEVICE_TAG)).then(|| values.to_vec())
            }),
            Ok(Err(error)) => {
                tracing::warn!("device broker signing failed: {error}");
                None
            }
            Err(error) => {
                tracing::warn!("device broker signing task failed: {error}");
                None
            }
        };
        match tag {
            Some(tag) => render_device_tag(&tag).into_bytes(),
            None => render_status(500, "Internal Server Error").into_bytes(),
        }
    }

    async fn serve(self: Arc<Self>, listener: TcpListener) {
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT));
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!("device broker accept failed: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                continue;
            };
            let broker = Arc::clone(&self);
            tokio::spawn(async move {
                let _permit = permit;
                broker.handle(stream).await;
            });
        }
    }

    async fn handle(&self, mut stream: TcpStream) {
        let request = match tokio::time::timeout(REQUEST_TIMEOUT, read_request(&mut stream)).await {
            Ok(Some(request)) => request,
            _ => {
                let _ = stream
                    .write_all(render_status(400, "Bad Request").as_bytes())
                    .await;
                return;
            }
        };
        let response = self.respond(&request).await;
        let _ = tokio::time::timeout(REQUEST_TIMEOUT, stream.write_all(&response)).await;
        let _ = stream.shutdown().await;
    }
}

/// Read one whole request (head and `Content-Length` body), at most
/// [`MAX_DEVICE_REQUEST_BYTES`].
async fn read_request(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
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

/// Strip inherited device broker variables from a child and, when the
/// harness runs a device broker, give it fresh per-spawn credentials. The
/// lease must live exactly as long as the child.
pub fn apply_child_device(
    cmd: &mut tokio::process::Command,
    broker: Option<&Arc<DeviceBroker>>,
) -> Result<Option<DeviceLease>, BrokerFull> {
    for name in DEVICE_ENV_VARS {
        cmd.env_remove(name);
    }
    let Some(broker) = broker else {
        return Ok(None);
    };
    let lease = broker.register()?;
    cmd.envs(lease.env().iter().cloned());
    Ok(Some(lease))
}

/// Give MCP server definitions the device broker credentials of the agent
/// process that spawns them, replacing any already present.
pub fn apply_mcp_device(servers: &mut [crate::acp::McpServer], lease: Option<&DeviceLease>) {
    let Some(lease) = lease else {
        return;
    };
    for server in servers {
        server
            .env
            .retain(|var| !DEVICE_ENV_VARS.contains(&var.name.as_str()));
        server
            .env
            .extend(lease.env().iter().map(|(name, value)| crate::acp::EnvVar {
                name: name.clone(),
                value: value.clone(),
            }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuzz_auth::device_proof::verify_event_device_tag;
    use kuzz_token_broker::device::{request_device_tag, DeviceSigner};
    use kuzz_token_broker::DEFAULT_BROKER_TIMEOUT;
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};

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

    fn secret_of(lease: &DeviceLease) -> Secret {
        Secret::new(
            lease
                .env()
                .iter()
                .find(|(name, _)| name == DEVICE_BROKER_SECRET_ENV)
                .map(|(_, value)| value.clone())
                .unwrap(),
        )
    }

    async fn ask(
        lease: &DeviceLease,
        unsigned: UnsignedEvent,
    ) -> Result<nostr::Tag, kuzz_token_broker::BrokerError> {
        let url = lease.broker.url();
        let secret = secret_of(lease);
        tokio::task::spawn_blocking(move || {
            request_device_tag(&url, &secret, &unsigned, DEFAULT_BROKER_TIMEOUT)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn broker_tags_the_agents_events_for_this_relay() {
        let dir = tempfile::tempdir().unwrap();
        let key = file_key(&dir);
        let agent = Keys::generate();
        let broker = DeviceBroker::start(key.clone(), agent.public_key(), RELAY)
            .await
            .unwrap();
        let lease = broker.register().unwrap();

        let unsigned = nip98(&agent, "https://relay.example/query");
        let tag = ask(&lease, unsigned.clone()).await.unwrap();
        let mut tagged = unsigned;
        tagged.tags.push(tag);
        tagged.id = None;
        let event = tagged.sign_with_keys(&agent).unwrap();
        assert_eq!(verify_event_device_tag(&event).unwrap(), Some(key.pubkey()));

        // A child resolving the broker signer gets the same result.
        let signer = DeviceSigner::Broker {
            url: broker.url(),
            secret: secret_of(&lease),
        };
        let auth = EventBuilder::auth("challenge", nostr::RelayUrl::parse(RELAY).unwrap())
            .build(agent.public_key());
        let tagged = tokio::task::spawn_blocking(move || signer.tag_event(&auth))
            .await
            .unwrap()
            .unwrap();
        let event = tagged.sign_with_keys(&agent).unwrap();
        assert_eq!(verify_event_device_tag(&event).unwrap(), Some(key.pubkey()));
    }

    #[tokio::test]
    async fn broker_refuses_what_the_agent_may_not_send_here() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Keys::generate();
        let broker = DeviceBroker::start(file_key(&dir), agent.public_key(), RELAY)
            .await
            .unwrap();
        let lease = broker.register().unwrap();

        let other_author = nip98(&Keys::generate(), "https://relay.example/query");
        let other_origin = nip98(&agent, "https://evil.example/query");
        let note = EventBuilder::text_note("hi").build(agent.public_key());
        let mut stale = nip98(&agent, "https://relay.example/query");
        stale.created_at = Timestamp::from(unix_now() - 3600);
        let blossom_elsewhere = EventBuilder::new(Kind::from(24242), "Get")
            .tags([Tag::parse(["server", "evil.example"]).unwrap()])
            .build(agent.public_key());
        for unsigned in [other_author, other_origin, note, stale, blossom_elsewhere] {
            assert!(ask(&lease, unsigned).await.is_err());
        }
        let blossom = EventBuilder::new(Kind::from(24242), "Get")
            .tags([Tag::parse(["server", "relay.example"]).unwrap()])
            .build(agent.public_key());
        assert!(ask(&lease, blossom).await.is_ok());
    }

    #[tokio::test]
    async fn broker_secrets_die_with_their_lease() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Keys::generate();
        let broker = DeviceBroker::start(file_key(&dir), agent.public_key(), RELAY)
            .await
            .unwrap();
        let lease = broker.register().unwrap();
        let url = broker.url();
        let secret = secret_of(&lease);
        drop(lease);
        let unsigned = nip98(&agent, "https://relay.example/query");
        let result = tokio::task::spawn_blocking(move || {
            request_device_tag(&url, &secret, &unsigned, DEFAULT_BROKER_TIMEOUT)
        })
        .await
        .unwrap();
        assert!(result.is_err());
        assert_eq!(broker.lock_secrets().live.len(), 0);
    }

    #[tokio::test]
    async fn tag_with_adds_a_verifiable_tag_and_none_leaves_the_event() {
        let dir = tempfile::tempdir().unwrap();
        let key = file_key(&dir);
        let agent = Keys::generate();
        let unsigned = nip98(&agent, "https://relay.example/query");
        let event = tag_with(Some(&key), unsigned.clone())
            .await
            .sign_with_keys(&agent)
            .unwrap();
        assert_eq!(verify_event_device_tag(&event).unwrap(), Some(key.pubkey()));
        let plain = tag_with(None, unsigned)
            .await
            .sign_with_keys(&agent)
            .unwrap();
        assert_eq!(verify_event_device_tag(&plain).unwrap(), None);
    }

    #[test]
    fn only_a_wrong_host_refusal_stops_the_start() {
        let wrong_host = r#"{"error":"the signing key is not admitted from this device (wrong_host)","code":"not_admitted","admission":"enforce"}"#;
        assert!(matches!(
            admission_of(403, wrong_host),
            Admission::OtherDevice(message) if message.contains("wrong_host")
        ));
        for (status, body) in [
            // `log`: the relay still admits the agent elsewhere.
            (
                403,
                r#"{"error":"... (wrong_host)","code":"not_admitted","admission":"log"}"#,
            ),
            // An older relay that does not name its mode.
            (403, r#"{"error":"... (wrong_host)","code":"not_admitted"}"#),
            (200, r#"{"admission":"off","keys":[]}"#),
            (
                403,
                r#"{"code":"not_admitted","message":"... (unregistered)"}"#,
            ),
            (404, "not found"),
            (500, ""),
        ] {
            assert_eq!(
                admission_of(status, body),
                Admission::Start,
                "{status} {body}"
            );
        }
    }

    #[tokio::test]
    async fn status_check_sends_a_device_tagged_nip98_and_reads_the_answer() {
        use axum::{http::HeaderMap, routing::get, Router};
        let dir = tempfile::tempdir().unwrap();
        let key = file_key(&dir);
        let agent = Keys::generate();
        let seen = Arc::new(Mutex::new(None::<String>));
        let record = Arc::clone(&seen);
        let app = Router::new().route(
            "/agents/{pubkey}/status",
            get(move |headers: HeaderMap| {
                let record = Arc::clone(&record);
                async move {
                    *record.lock().unwrap() = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    (
                        axum::http::StatusCode::FORBIDDEN,
                        r#"{"code":"not_admitted","error":"the signing key is not admitted from this device (wrong_host)","admission":"enforce"}"#,
                    )
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let admission = check_status(&reqwest::Client::new(), &base, &agent, Some(&key)).await;
        assert!(matches!(admission, Admission::OtherDevice(_)));

        use base64::Engine as _;
        let header = seen.lock().unwrap().clone().expect("authorization");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(header.strip_prefix("Nostr ").unwrap())
            .unwrap();
        let event = nostr::Event::from_json(std::str::from_utf8(&raw).unwrap()).unwrap();
        event.verify().unwrap();
        assert_eq!(event.kind, Kind::HttpAuth);
        assert_eq!(verify_event_device_tag(&event).unwrap(), Some(key.pubkey()));
    }

    #[tokio::test]
    async fn child_env_gets_device_broker_credentials_only_with_a_broker() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Keys::generate();
        let broker = DeviceBroker::start(file_key(&dir), agent.public_key(), RELAY)
            .await
            .unwrap();
        let mut cmd = tokio::process::Command::new("unused");
        cmd.env(DEVICE_BROKER_SECRET_ENV, "inherited");
        assert!(apply_child_device(&mut cmd, None).unwrap().is_none());
        let removed = cmd
            .as_std()
            .get_envs()
            .any(|(name, value)| name == DEVICE_BROKER_SECRET_ENV && value.is_none());
        assert!(removed);

        let lease = apply_child_device(&mut cmd, Some(&broker))
            .unwrap()
            .unwrap();
        let set: Vec<_> = cmd
            .as_std()
            .get_envs()
            .filter_map(|(name, value)| {
                Some((name.to_str()?.to_owned(), value?.to_str()?.to_owned()))
            })
            .collect();
        assert!(set
            .iter()
            .any(|(name, value)| name == DEVICE_BROKER_URL_ENV && *value == broker.url()));
        assert!(set
            .iter()
            .any(|(name, value)| name == DEVICE_BROKER_SECRET_ENV
                && *value == secret_of(&lease).expose()));
    }
}
