//! Device-key signing for Kuzz agent tooling (device-bound keys plan §6;
//! review S2, F1, F2).
//!
//! Every `kuzz`, `git-credential-nostr`, `kuzz-dev-mcp` and `kuzz-acp`
//! process signs with the one device key this OS user has on this machine
//! ([`DEVICE_KEY_SCOPE`]): in the TPM where Windows has one, otherwise a
//! software key in [`default_store_dir`]. Tools only *open* that key
//! ([`OpenMode::ExistingOnly`]); only `kuzz auth login` and the desktop app
//! create it ([`OpenMode::CreateIfMissing`]).
//!
//! A managed-agent child ([`crate::DISABLE_STORED_SESSION_ENV`]) that cannot
//! open the key itself (WSL, a container, a sandbox) asks the `kuzz-acp`
//! loopback **device broker** instead: `POST /device-tag` with
//! `Authorization: Bearer <KUZZ_DEVICE_BROKER_SECRET>` and the unsigned auth
//! event as JSON, answered by `{"tag": ["device", <pubkey>, <sig>]}`. The
//! broker signs event tags only, never header proofs.
//!
//! A process the desktop app spawned (`kuzz-acp`) that cannot open the key
//! itself (macOS: only the app's keychain access group reaches the Secure
//! Enclave key) asks the app's loopback **app signer** instead
//! ([`crate::app_signer`]): the same `POST /device-tag`, plus
//! `POST /header-proof` for `X-Kuzz-Device-Proof` values.
//!
//! Failing to open the key or to sign is never fatal: callers send what they
//! sent before (no tag, no proof) and report the reason as a warning.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

pub use kuzz_device_key::{DeviceKey, DeviceKeyError, FileStore, DEVICE_KEY_SCOPE};
use nostr::{JsonUtil as _, PublicKey, Tag, UnsignedEvent};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::app_signer::{
    request_header_proof, HeaderProofRequest, APP_SIGNER_SECRET_ENV, APP_SIGNER_URL_ENV,
};
use crate::{
    http_ok_body, parse_broker_url, stored_session_disabled, BrokerError, Secret,
    MAX_RESPONSE_BYTES,
};

/// Loopback device broker URL (`http://127.0.0.1:<port>`), set by `kuzz-acp`.
pub const DEVICE_BROKER_URL_ENV: &str = "KUZZ_DEVICE_BROKER_URL";
/// Per-spawn device broker secret, set by `kuzz-acp`.
pub const DEVICE_BROKER_SECRET_ENV: &str = "KUZZ_DEVICE_BROKER_SECRET";
/// Every device broker variable. Spawners strip these before injecting their own.
pub const DEVICE_ENV_VARS: [&str; 2] = [DEVICE_BROKER_URL_ENV, DEVICE_BROKER_SECRET_ENV];
/// Device broker request path.
pub const DEVICE_TAG_PATH: &str = "/device-tag";
/// Event kinds that carry a device tag: NIP-42 AUTH, NIP-98 HTTP auth and
/// Blossom auth.
pub const DEVICE_TAGGED_KINDS: [u16; 3] = [22242, 27235, 24242];
/// Longest wait for one device signature (TPM or broker) in async callers;
/// past it the request goes out without one, with a warning.
pub const DEVICE_SIGN_TIMEOUT: Duration = Duration::from_secs(2);
/// Largest device broker request (head and body).
pub const MAX_DEVICE_REQUEST_BYTES: usize = 8 * 1024;

/// The software fallback store every Kuzz app of this OS user shares:
/// `<local data dir>/kuzz/device-keys` (`%LOCALAPPDATA%` on Windows,
/// `~/Library/Application Support` on macOS, `$XDG_DATA_HOME` or
/// `~/.local/share` elsewhere).
pub fn default_store_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
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

/// Whether opening the shared key may create it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// Open an existing key only (every tool, every managed-agent process).
    ExistingOnly,
    /// Create the key when there is none (`kuzz auth login` only).
    CreateIfMissing,
}

/// Open this OS user's shared device key. `Ok(None)`: there is none yet
/// (only with [`OpenMode::ExistingOnly`]). Blocks on the TPM: call it from
/// `spawn_blocking` in async code.
pub fn open_shared(mode: OpenMode) -> Result<Option<DeviceKey>, DeviceKeyError> {
    let dir = default_store_dir()
        .ok_or_else(|| DeviceKeyError::Store("no local data directory".into()))?;
    let store = FileStore::new(dir);
    match mode {
        OpenMode::ExistingOnly => DeviceKey::open(DEVICE_KEY_SCOPE, &store),
        OpenMode::CreateIfMissing => DeviceKey::open_or_create(DEVICE_KEY_SCOPE, &store).map(Some),
    }
}

/// Why a device tag could not be made.
#[derive(Debug, thiserror::Error)]
pub enum DeviceSignError {
    /// The local key failed.
    #[error("device key: {0}")]
    Key(#[from] DeviceKeyError),
    /// The device broker or app signer failed or refused.
    #[error("device broker: {0}")]
    Broker(#[from] BrokerError),
    /// This signer cannot sign header proofs (the `kuzz-acp` broker).
    #[error("this device signer does not sign header proofs")]
    Unsupported,
}

/// What signs this process's device tags and proofs.
#[derive(Debug, Clone)]
pub enum DeviceSigner {
    /// This machine's device key, opened by this process.
    Key(DeviceKey),
    /// The `kuzz-acp` device broker (event tags only).
    Broker {
        /// Broker base URL.
        url: String,
        /// This spawn's broker secret.
        secret: Secret,
    },
    /// The desktop app's signer (event tags and header proofs), for a
    /// process the app spawned that cannot open the key itself.
    App {
        /// App signer base URL.
        url: String,
        /// This spawn's app signer secret.
        secret: Secret,
    },
}

impl DeviceSigner {
    /// Pick the signer, in order: the key `open` returns; else the desktop
    /// app signer from `lookup` (set by the app on the processes it spawns);
    /// else (managed-agent children only) the `kuzz-acp` device broker. The
    /// second value is a reason worth a warning: a key that failed to open
    /// with nothing to stand in for it. A machine without a key yet is not
    /// worth one.
    pub fn resolve(
        lookup: impl Fn(&str) -> Option<String>,
        open: impl FnOnce() -> Result<Option<DeviceKey>, DeviceKeyError>,
    ) -> (Option<Self>, Option<String>) {
        let warning = match open() {
            Ok(Some(key)) => return (Some(Self::Key(key)), None),
            Ok(None) => None,
            Err(error) => Some(format!("device key unavailable: {error}")),
        };
        let get = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        if let (Some(url), Some(secret)) = (get(APP_SIGNER_URL_ENV), get(APP_SIGNER_SECRET_ENV)) {
            return (
                Some(Self::App {
                    url,
                    secret: Secret::new(secret),
                }),
                None,
            );
        }
        if stored_session_disabled(&lookup) {
            if let (Some(url), Some(secret)) =
                (get(DEVICE_BROKER_URL_ENV), get(DEVICE_BROKER_SECRET_ENV))
            {
                return (
                    Some(Self::Broker {
                        url,
                        secret: Secret::new(secret),
                    }),
                    None,
                );
            }
        }
        (None, warning)
    }

    /// [`Self::resolve`] from the process environment and the shared key
    /// ([`OpenMode::ExistingOnly`]). Blocks on the TPM.
    pub fn from_env() -> (Option<Self>, Option<String>) {
        Self::resolve(
            |name| std::env::var(name).ok(),
            || open_shared(OpenMode::ExistingOnly),
        )
    }

    /// The local key, when this process holds one.
    pub fn key(&self) -> Option<&DeviceKey> {
        match self {
            Self::Key(key) => Some(key),
            Self::Broker { .. } | Self::App { .. } => None,
        }
    }

    /// Whether [`Self::header_proof_at`] can succeed (not the broker).
    pub fn signs_header_proofs(&self) -> bool {
        !matches!(self, Self::Broker { .. })
    }

    /// The `X-Kuzz-Device-Proof` value at unix time `ts` (see
    /// [`DeviceKey::header_proof_at`]). The app signer gets only the SHA-256
    /// of `token` and `body`, never the token. Blocks on the TPM or the app.
    pub fn header_proof_at(
        &self,
        ts: i64,
        purpose: &str,
        binding: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> Result<String, DeviceSignError> {
        match self {
            Self::Key(key) => Ok(key.header_proof_at(ts, purpose, binding, token, body)?),
            Self::App { url, secret } => {
                let request = HeaderProofRequest::new(ts, purpose, binding, token, body);
                Ok(request_header_proof(
                    url,
                    secret,
                    &request,
                    crate::DEFAULT_BROKER_TIMEOUT,
                )?)
            }
            Self::Broker { .. } => Err(DeviceSignError::Unsupported),
        }
    }

    /// `unsigned` with this device's tag added (see
    /// [`DeviceKey::add_event_tag`]). Blocks on the TPM or the broker.
    pub fn tag_event(&self, unsigned: &UnsignedEvent) -> Result<UnsignedEvent, DeviceSignError> {
        match self {
            Self::Key(key) => Ok(key.add_event_tag(unsigned.clone())?),
            Self::Broker { url, secret } | Self::App { url, secret } => {
                let tag = request_device_tag(url, secret, unsigned, crate::DEFAULT_BROKER_TIMEOUT)?;
                let mut tagged = unsigned.clone();
                tagged.tags.push(tag);
                tagged.id = None;
                Ok(tagged)
            }
        }
    }
}

/// `unsigned` tagged by `signer`, or unchanged (with the reason) when there
/// is no signer or tagging fails. Blocks on the TPM or the broker.
pub fn tag_or_unchanged(
    signer: Option<&DeviceSigner>,
    unsigned: UnsignedEvent,
) -> (UnsignedEvent, Option<DeviceSignError>) {
    let Some(signer) = signer else {
        return (unsigned, None);
    };
    match signer.tag_event(&unsigned) {
        Ok(tagged) => (tagged, None),
        Err(error) => (unsigned, Some(error)),
    }
}

/// The device broker's JSON answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceTagResponse {
    /// `["device", <device pubkey hex>, <sig hex>]`.
    pub tag: Vec<String>,
}

/// One `POST /device-tag` round trip: the device tag for `unsigned`.
pub fn request_device_tag(
    url: &str,
    secret: &Secret,
    unsigned: &UnsignedEvent,
    timeout: Duration,
) -> Result<Tag, BrokerError> {
    let raw = post_json(url, secret, DEVICE_TAG_PATH, &unsigned.as_json(), timeout)?;
    let invalid = || BrokerError::InvalidResponse("body is not a device tag".into());
    let answer: DeviceTagResponse =
        serde_json::from_slice(http_ok_body(&raw)?).map_err(|_| invalid())?;
    if answer.tag.len() != 3 || answer.tag[0] != "device" {
        return Err(invalid());
    }
    Tag::parse(answer.tag).map_err(|_| invalid())
}

/// One loopback `POST <path>` of a JSON `body` with `Bearer <secret>`: the
/// raw response, at most [`MAX_RESPONSE_BYTES`].
pub(crate) fn post_json(
    url: &str,
    secret: &Secret,
    path: &str,
    body: &str,
    timeout: Duration,
) -> Result<Vec<u8>, BrokerError> {
    let addr = parse_broker_url(url)?;
    if body.len() > MAX_DEVICE_REQUEST_BYTES / 2 {
        return Err(BrokerError::InvalidResponse(
            "request too large for the device signer".into(),
        ));
    }
    let io = |e: std::io::Error| BrokerError::Io(e.to_string());
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(io)?;
    stream.set_read_timeout(Some(timeout)).map_err(io)?;
    stream.set_write_timeout(Some(timeout)).map_err(io)?;
    let host = match addr.ip() {
        IpAddr::V4(ip) => format!("{ip}:{}", addr.port()),
        IpAddr::V6(ip) => format!("[{ip}]:{}", addr.port()),
    };
    let request = Zeroizing::new(format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        secret.expose(),
        body.len(),
    ));
    stream.write_all(request.as_bytes()).map_err(io)?;

    let mut raw = Vec::with_capacity(512);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).map_err(io)?;
        if n == 0 {
            break;
        }
        if raw.len() + n > MAX_RESPONSE_BYTES {
            return Err(BrokerError::InvalidResponse("response too large".into()));
        }
        raw.extend_from_slice(&chunk[..n]);
    }
    Ok(raw)
}

/// A parsed device broker request (server side).
#[derive(Debug, PartialEq, Eq)]
pub struct DeviceRequest {
    /// Request method.
    pub method: String,
    /// Request path.
    pub path: String,
    /// Raw `Authorization` value.
    pub authorization: Option<String>,
    /// Body (`Content-Length` bytes).
    pub body: Vec<u8>,
}

/// Server side: parse `raw` once it holds a whole request. `None`: more
/// bytes are needed, or the request is malformed (the caller bounds the
/// read by [`MAX_DEVICE_REQUEST_BYTES`] and a timeout).
pub fn parse_device_request(raw: &[u8]) -> Option<DeviceRequest> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let path = request_line.next()?.to_owned();
    if !request_line.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let mut authorization = None;
    let mut length = 0usize;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.eq_ignore_ascii_case("authorization") {
            authorization = Some(value.trim().to_owned());
        } else if name.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().ok()?;
        }
    }
    let body = raw.get(end + 4..end + 4 + length)?.to_vec();
    Some(DeviceRequest {
        method,
        path,
        authorization,
        body,
    })
}

/// How far a signed event's `created_at` may be from now.
pub const MAX_EVENT_SKEW_SECS: u64 = 60;

/// Why a device signer refuses a request (answered as `403`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The body does not parse.
    Malformed,
    /// Not one of [`DEVICE_TAGGED_KINDS`], or an unknown proof purpose.
    Kind,
    /// The event is not the agent's.
    Author,
    /// `created_at` (or the proof time) is too far from now.
    Stale,
    /// The event already carries a device tag.
    AlreadyTagged,
    /// The event or proof is not aimed at the agent's relay.
    Origin,
}

/// What one signer secret may sign (review S2): device tags only on auth
/// events of `agent` aimed at `relay_authority`, header proofs only for
/// requests to it. Shared by the `kuzz-acp` broker and the app signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceScope {
    /// The agent whose events may be tagged.
    pub agent: PublicKey,
    /// [`kuzz_core::tenant::relay_url_authority`] of the agent's relay.
    pub relay_authority: String,
}

impl DeviceScope {
    /// The scope of `agent` on `relay_url`.
    pub fn new(agent: PublicKey, relay_url: &str) -> Self {
        Self {
            agent,
            relay_authority: kuzz_core::tenant::relay_url_authority(relay_url),
        }
    }

    /// Whether `unsigned` may get a device tag at unix time `now`: one of
    /// [`DEVICE_TAGGED_KINDS`], authored by the agent, fresh, without a
    /// device tag, and aimed at the relay (`relay`, `u` or `server` tag).
    pub fn admit_event(&self, unsigned: &UnsignedEvent, now: u64) -> Result<(), Refusal> {
        let kind = unsigned.kind.as_u16();
        if !DEVICE_TAGGED_KINDS.contains(&kind) {
            return Err(Refusal::Kind);
        }
        if unsigned.pubkey != self.agent {
            return Err(Refusal::Author);
        }
        if unsigned.created_at.as_secs().abs_diff(now) > MAX_EVENT_SKEW_SECS {
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
            .any(|tag| tag.as_slice().first().map(String::as_str) == Some("device"))
        {
            return Err(Refusal::AlreadyTagged);
        }
        let authority = match kind {
            22242 => first("relay").map(|url| kuzz_core::tenant::relay_url_authority(&url)),
            27235 => first("u").map(|url| kuzz_core::tenant::relay_url_authority(&url)),
            _ => first("server").map(|server| kuzz_core::tenant::normalize_host(&server)),
        };
        self.same_relay(authority)
    }

    /// `Ok` when `authority` is this scope's (non-empty) relay authority.
    pub(crate) fn same_relay(&self, authority: Option<String>) -> Result<(), Refusal> {
        match authority {
            Some(authority) if !authority.is_empty() && authority == self.relay_authority => Ok(()),
            _ => Err(Refusal::Origin),
        }
    }
}

/// Server side: a `200 OK` answer carrying `tag`.
pub fn render_device_tag(tag: &[String]) -> String {
    let body = serde_json::to_string(&DeviceTagResponse { tag: tag.to_vec() }).unwrap_or_default();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    use kuzz_auth::device_proof::verify_event_device_tag;
    use nostr::{Keys, Kind, Timestamp};

    fn nip98(keys: &Keys) -> UnsignedEvent {
        UnsignedEvent::new(
            keys.public_key(),
            Timestamp::now(),
            Kind::Custom(27235),
            [
                Tag::parse(["u", "https://relay.example/query"]).unwrap(),
                Tag::parse(["method", "POST"]).unwrap(),
            ],
            "",
        )
    }

    fn file_key(dir: &tempfile::TempDir) -> DeviceKey {
        DeviceKey::open_or_create_file(&dir.path().join("device.key")).unwrap()
    }

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn a_local_key_tags_events_the_relay_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let key = file_key(&dir);
        let keys = Keys::generate();
        let signer = DeviceSigner::Key(key.clone());
        let (tagged, error) = tag_or_unchanged(Some(&signer), nip98(&keys));
        assert!(error.is_none());
        let event = tagged.sign_with_keys(&keys).unwrap();
        let device = verify_event_device_tag(&event)
            .unwrap()
            .expect("device tag");
        assert_eq!(device.to_hex(), key.pubkey().to_hex());
    }

    #[test]
    fn no_signer_or_a_failing_one_leaves_the_event_unchanged() {
        let keys = Keys::generate();
        let unsigned = nip98(&keys);
        let (same, error) = tag_or_unchanged(None, unsigned.clone());
        assert!(error.is_none());
        assert_eq!(same.tags, unsigned.tags);
        // A broker nobody listens on: the event goes out as before.
        let signer = DeviceSigner::Broker {
            url: "http://127.0.0.1:1".into(),
            secret: Secret::new("s".into()),
        };
        let (same, error) = tag_or_unchanged(Some(&signer), unsigned.clone());
        assert!(matches!(error, Some(DeviceSignError::Broker(_))));
        assert_eq!(same.tags, unsigned.tags);
    }

    #[test]
    fn resolve_prefers_the_key_and_gives_children_the_broker() {
        let dir = tempfile::tempdir().unwrap();
        let key = file_key(&dir);
        let broker_env = env(&[
            (crate::DISABLE_STORED_SESSION_ENV, "1"),
            (DEVICE_BROKER_URL_ENV, "http://127.0.0.1:9"),
            (DEVICE_BROKER_SECRET_ENV, "s"),
        ]);
        let (signer, warning) = DeviceSigner::resolve(&broker_env, || Ok(Some(key)));
        assert!(matches!(signer, Some(DeviceSigner::Key(_))) && warning.is_none());

        let (signer, warning) = DeviceSigner::resolve(&broker_env, || Ok(None));
        assert!(matches!(signer, Some(DeviceSigner::Broker { .. })) && warning.is_none());

        // Not a managed-agent child: broker variables are ignored.
        let person = env(&[
            (DEVICE_BROKER_URL_ENV, "http://127.0.0.1:9"),
            (DEVICE_BROKER_SECRET_ENV, "s"),
        ]);
        let (signer, warning) = DeviceSigner::resolve(&person, || Ok(None));
        assert!(signer.is_none() && warning.is_none(), "no key yet: silent");
        let (signer, warning) =
            DeviceSigner::resolve(&person, || Err(DeviceKeyError::Store("locked".into())));
        assert!(signer.is_none());
        assert!(warning.unwrap().contains("locked"));
    }

    #[test]
    fn a_helper_whose_key_fails_falls_back_to_the_app_signer() {
        let dir = tempfile::tempdir().unwrap();
        let key = file_key(&dir);
        let agent = Keys::generate();
        let signer = crate::app_signer::AppSigner::start(key.clone()).unwrap();
        let lease = signer
            .register(agent.public_key(), "wss://relay.example")
            .unwrap();
        let mut vars: Vec<(String, String)> = lease.env().to_vec();
        // The app signer wins over the kuzz-acp broker.
        vars.push((crate::DISABLE_STORED_SESSION_ENV.into(), "1".into()));
        vars.push((DEVICE_BROKER_URL_ENV.into(), "http://127.0.0.1:9".into()));
        vars.push((DEVICE_BROKER_SECRET_ENV.into(), "s".into()));
        let lookup = |name: &str| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
        // The own key still comes first (Windows: the TPM, as before).
        let (own, _) = DeviceSigner::resolve(lookup, || Ok(Some(key.clone())));
        assert!(matches!(own, Some(DeviceSigner::Key(_))));

        // macOS helper without the keychain access group: open fails.
        let (fallback, warning) = DeviceSigner::resolve(lookup, || {
            Err(DeviceKeyError::SecureEnclave {
                op: "open",
                code: -34018,
            })
        });
        assert!(warning.is_none());
        let fallback = fallback.expect("app signer");
        assert!(matches!(fallback, DeviceSigner::App { .. }));
        assert!(fallback.signs_header_proofs() && fallback.key().is_none());
        let (tagged, error) = tag_or_unchanged(Some(&fallback), nip98(&agent));
        assert!(error.is_none());
        let event = tagged.sign_with_keys(&agent).unwrap();
        assert_eq!(verify_event_device_tag(&event).unwrap(), Some(key.pubkey()));
        signer.stop();
    }

    #[test]
    fn the_broker_never_signs_header_proofs() {
        let broker = DeviceSigner::Broker {
            url: "http://127.0.0.1:1".into(),
            secret: Secret::new("s".into()),
        };
        assert!(!broker.signs_header_proofs());
        assert!(matches!(
            broker.header_proof_at(0, "auth", "GET https://r/auth/me", Some("t"), b""),
            Err(DeviceSignError::Unsupported)
        ));
    }

    /// A fake broker answers with a real tag from a file key; the client
    /// sends path, secret and the event, and the tag verifies.
    #[test]
    fn the_broker_round_trip_returns_a_verifiable_tag() {
        let dir = tempfile::tempdir().unwrap();
        let key = file_key(&dir);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0u8; 1024];
            let request = loop {
                let n = stream.read(&mut chunk).unwrap();
                raw.extend_from_slice(&chunk[..n]);
                if let Some(request) = parse_device_request(&raw) {
                    break request;
                }
                assert!(n > 0, "client closed early");
            };
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, DEVICE_TAG_PATH);
            assert_eq!(
                request.authorization.as_deref(),
                Some("Bearer spawn-secret")
            );
            let unsigned = UnsignedEvent::from_json(&request.body).unwrap();
            let tagged = key.add_event_tag(unsigned).unwrap();
            let tag = tagged.tags.iter().last().unwrap().as_slice().to_vec();
            stream
                .write_all(render_device_tag(&tag).as_bytes())
                .unwrap();
        });
        let keys = Keys::generate();
        let signer = DeviceSigner::Broker {
            url,
            secret: Secret::new("spawn-secret".into()),
        };
        let tagged = signer.tag_event(&nip98(&keys)).unwrap();
        server.join().unwrap();
        let event = tagged.sign_with_keys(&keys).unwrap();
        assert!(verify_event_device_tag(&event).unwrap().is_some());
    }

    #[test]
    fn requests_wait_for_their_whole_body() {
        let head =
            b"POST /device-tag HTTP/1.1\r\nContent-Length: 4\r\nAuthorization: Bearer x\r\n\r\n";
        let mut raw = head.to_vec();
        raw.extend_from_slice(b"ab");
        assert!(parse_device_request(&raw).is_none());
        raw.extend_from_slice(b"cd");
        let request = parse_device_request(&raw).unwrap();
        assert_eq!(request.body, b"abcd");
        assert!(parse_device_request(b"POST / HTTP/1.1\r\nContent-Length: x\r\n\r\n").is_none());
    }
}
