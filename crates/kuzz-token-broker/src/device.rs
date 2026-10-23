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
//! Failing to open the key or to sign is never fatal: callers send what they
//! sent before (no tag, no proof) and report the reason as a warning.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

pub use kuzz_device_key::{DeviceKey, DeviceKeyError, FileStore, DEVICE_KEY_SCOPE};
use nostr::{JsonUtil as _, Tag, UnsignedEvent};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

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
    /// The device broker failed or refused.
    #[error("device broker: {0}")]
    Broker(#[from] BrokerError),
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
}

impl DeviceSigner {
    /// Pick the signer: the key `open` returns, else (managed-agent children
    /// only) the device broker from `lookup`. The second value is a reason
    /// worth a warning: a key that failed to open. A machine without a key
    /// yet is not worth one.
    pub fn resolve(
        lookup: impl Fn(&str) -> Option<String>,
        open: impl FnOnce() -> Result<Option<DeviceKey>, DeviceKeyError>,
    ) -> (Option<Self>, Option<String>) {
        let warning = match open() {
            Ok(Some(key)) => return (Some(Self::Key(key)), None),
            Ok(None) => None,
            Err(error) => Some(format!("device key unavailable: {error}")),
        };
        if stored_session_disabled(&lookup) {
            let get = |name: &str| {
                lookup(name)
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
            };
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

    /// The local key, when this process holds one (header proofs need it).
    pub fn key(&self) -> Option<&DeviceKey> {
        match self {
            Self::Key(key) => Some(key),
            Self::Broker { .. } => None,
        }
    }

    /// `unsigned` with this device's tag added (see
    /// [`DeviceKey::add_event_tag`]). Blocks on the TPM or the broker.
    pub fn tag_event(&self, unsigned: &UnsignedEvent) -> Result<UnsignedEvent, DeviceSignError> {
        match self {
            Self::Key(key) => Ok(key.add_event_tag(unsigned.clone())?),
            Self::Broker { url, secret } => {
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
    let addr = parse_broker_url(url)?;
    let body = unsigned.as_json();
    if body.len() > MAX_DEVICE_REQUEST_BYTES / 2 {
        return Err(BrokerError::InvalidResponse(
            "event too large for the device broker".into(),
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
        "POST {DEVICE_TAG_PATH} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    let invalid = || BrokerError::InvalidResponse("body is not a device tag".into());
    let answer: DeviceTagResponse =
        serde_json::from_slice(http_ok_body(&raw)?).map_err(|_| invalid())?;
    if answer.tag.len() != 3 || answer.tag[0] != "device" {
        return Err(invalid());
    }
    Tag::parse(answer.tag).map_err(|_| invalid())
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
