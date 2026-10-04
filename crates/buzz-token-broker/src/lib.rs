#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Bearer-token sources for Buzz agent tooling (centralized identity, Phase 1).
//!
//! A Desktop-hosted bot's `buzz-acp` holds a short-lived bot token
//! (`BUZZ_BOT_TOKEN`) that it refreshes itself. The agent processes it spawns
//! must not inherit that token; instead they get a loopback **token broker**
//! (`BUZZ_TOKEN_BROKER_URL` + a per-spawn `BUZZ_TOKEN_BROKER_SECRET`) and ask
//! it for the current token whenever they need one. `buzz` (the CLI) and
//! `git-credential-buzz` resolve their credential with [`TokenSource::resolve`]:
//!
//! 1. `BUZZ_BOT_TOKEN`
//! 2. `BUZZ_ACCESS_TOKEN`
//! 3. `BUZZ_TOKEN_BROKER_URL` + `BUZZ_TOKEN_BROKER_SECRET`
//!
//! The broker protocol is one request: `GET /token` with
//! `Authorization: Bearer <secret>`, answered by a JSON [`BrokerResponse`] of
//! at most [`MAX_RESPONSE_BYTES`]. The client side ([`fetch_from_broker`]) is
//! std-only and talks plain HTTP/1.1 to a loopback address only.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Desktop-hosted bot access token (`bzb_…`), injected by Buzz Desktop.
pub const BOT_TOKEN_ENV: &str = "BUZZ_BOT_TOKEN";
/// Any access token (`bzs_…`, `bzk_…`) supplied directly.
pub const ACCESS_TOKEN_ENV: &str = "BUZZ_ACCESS_TOKEN";
/// Loopback broker URL (`http://127.0.0.1:<port>`), set by `buzz-acp`.
pub const BROKER_URL_ENV: &str = "BUZZ_TOKEN_BROKER_URL";
/// Per-spawn broker secret, set by `buzz-acp`.
pub const BROKER_SECRET_ENV: &str = "BUZZ_TOKEN_BROKER_SECRET";

/// Every environment variable that carries a token credential. Spawners strip
/// these before injecting their own values.
pub const TOKEN_ENV_VARS: [&str; 4] = [
    BOT_TOKEN_ENV,
    ACCESS_TOKEN_ENV,
    BROKER_URL_ENV,
    BROKER_SECRET_ENV,
];

/// Broker request path.
pub const BROKER_PATH: &str = "/token";
/// Upper bound on a broker response (headers + body).
pub const MAX_RESPONSE_BYTES: usize = 4096;
/// Requests per minute a broker serves per spawn before answering 429.
pub const BROKER_REQUESTS_PER_MINUTE: u32 = 60;
/// Default client timeout for one broker round trip.
pub const DEFAULT_BROKER_TIMEOUT: Duration = Duration::from_secs(5);

/// A secret string that is zeroed on drop and never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    /// Wrap `value`.
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// The secret value. Callers must not log it.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

/// Where a token comes from, in precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// `BUZZ_BOT_TOKEN`.
    BotToken(Secret),
    /// `BUZZ_ACCESS_TOKEN`.
    AccessToken(Secret),
    /// The loopback broker.
    Broker {
        /// Broker base URL.
        url: String,
        /// Broker secret.
        secret: Secret,
    },
}

impl TokenSource {
    /// Resolve the highest-precedence configured source from the process env.
    pub fn from_env() -> Option<Self> {
        Self::resolve(|name| std::env::var(name).ok())
    }

    /// Resolve from an injected lookup (tests avoid mutating the process env).
    /// Blank values count as unset. A broker needs both URL and secret.
    pub fn resolve(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let get = |name: &str| {
            lookup(name)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        if let Some(token) = get(BOT_TOKEN_ENV) {
            return Some(Self::BotToken(Secret::new(token)));
        }
        if let Some(token) = get(ACCESS_TOKEN_ENV) {
            return Some(Self::AccessToken(Secret::new(token)));
        }
        match (get(BROKER_URL_ENV), get(BROKER_SECRET_ENV)) {
            (Some(url), Some(secret)) => Some(Self::Broker {
                url,
                secret: Secret::new(secret),
            }),
            _ => None,
        }
    }

    /// Obtain the token: directly for env sources, from the broker (one retry)
    /// otherwise.
    pub fn token(&self) -> Result<Secret, BrokerError> {
        match self {
            Self::BotToken(token) | Self::AccessToken(token) => Ok(token.clone()),
            Self::Broker { url, secret } => {
                fetch_with_retry(url, secret, DEFAULT_BROKER_TIMEOUT).map(|r| r.token)
            }
        }
    }
}

/// The broker's JSON answer to `GET /token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerResponse {
    /// The current access token.
    #[serde(serialize_with = "ser_secret", deserialize_with = "de_secret")]
    pub token: Secret,
    /// Token expiry (unix seconds), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

fn ser_secret<S: serde::Serializer>(secret: &Secret, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(secret.expose())
}

fn de_secret<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Secret, D::Error> {
    String::deserialize(d).map(Secret::new)
}

/// Why a broker request failed.
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    /// The broker URL is not `http://<loopback>:<port>`.
    #[error("invalid token broker URL: {0}")]
    InvalidUrl(String),
    /// Connect/read/write failed.
    #[error("token broker unreachable: {0}")]
    Io(String),
    /// The broker answered with a non-200 status.
    #[error("token broker returned HTTP {0}")]
    Status(u16),
    /// The response was malformed or too large.
    #[error("invalid token broker response: {0}")]
    InvalidResponse(String),
}

impl BrokerError {
    /// Whether retrying could help (network trouble or a 5xx/429).
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Io(_) => true,
            Self::Status(status) => *status == 429 || *status >= 500,
            Self::InvalidUrl(_) | Self::InvalidResponse(_) => false,
        }
    }
}

/// Parse `http://127.0.0.1:<port>[/]` (or `[::1]`) into a socket address.
/// Anything that is not a loopback HTTP origin is refused so a broker secret
/// can never be sent off-host.
pub fn parse_broker_url(url: &str) -> Result<SocketAddr, BrokerError> {
    let rest = url
        .trim()
        .strip_prefix("http://")
        .ok_or_else(|| BrokerError::InvalidUrl("must start with http://".into()))?;
    let authority = rest.trim_end_matches('/');
    if authority.contains('/') || authority.contains('@') {
        return Err(BrokerError::InvalidUrl("must be an origin".into()));
    }
    let addr: SocketAddr = authority
        .parse()
        .map_err(|_| BrokerError::InvalidUrl("must be <loopback-ip>:<port>".into()))?;
    if !addr.ip().is_loopback() || addr.port() == 0 {
        return Err(BrokerError::InvalidUrl(
            "must be a loopback address with a port".into(),
        ));
    }
    Ok(addr)
}

/// One `GET /token` round trip.
pub fn fetch_from_broker(
    url: &str,
    secret: &Secret,
    timeout: Duration,
) -> Result<BrokerResponse, BrokerError> {
    let addr = parse_broker_url(url)?;
    let io = |e: std::io::Error| BrokerError::Io(e.to_string());
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(io)?;
    stream.set_read_timeout(Some(timeout)).map_err(io)?;
    stream.set_write_timeout(Some(timeout)).map_err(io)?;
    let host = match addr.ip() {
        IpAddr::V4(ip) => format!("{ip}:{}", addr.port()),
        IpAddr::V6(ip) => format!("[{ip}]:{}", addr.port()),
    };
    let request = Zeroizing::new(format!(
        "GET {BROKER_PATH} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        secret.expose()
    ));
    stream.write_all(request.as_bytes()).map_err(io)?;

    let mut raw = Zeroizing::new(Vec::with_capacity(1024));
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
    parse_http_response(&raw)
}

/// Fetch with exactly one retry on a transient failure.
pub fn fetch_with_retry(
    url: &str,
    secret: &Secret,
    timeout: Duration,
) -> Result<BrokerResponse, BrokerError> {
    match fetch_from_broker(url, secret, timeout) {
        Err(error) if error.is_transient() => {
            std::thread::sleep(Duration::from_millis(250));
            fetch_from_broker(url, secret, timeout)
        }
        other => other,
    }
}

fn parse_http_response(raw: &[u8]) -> Result<BrokerResponse, BrokerError> {
    let invalid = |m: &str| BrokerError::InvalidResponse(m.to_owned());
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| invalid("missing header terminator"))?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| invalid("non-UTF-8 headers"))?;
    let status_line = head.lines().next().ok_or_else(|| invalid("empty"))?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(invalid("not HTTP/1.x"));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid("bad status"))?;
    if status != 200 {
        return Err(BrokerError::Status(status));
    }
    let body = &raw[split + 4..];
    serde_json::from_slice(body).map_err(|_| invalid("body is not a token JSON object"))
}

/// Server side: whether `authorization` (the raw header value) carries
/// `Bearer <expected>`. Compares in constant time with respect to the secret.
pub fn authorization_matches(authorization: Option<&str>, expected: &Secret) -> bool {
    let Some(value) = authorization else {
        return false;
    };
    let Some((scheme, presented)) = value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer") && constant_time_eq(presented.trim(), expected.expose())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = u8::from(a.len() != b.len());
    for (i, byte) in b.iter().enumerate() {
        diff |= a.get(i).copied().unwrap_or(0) ^ byte;
    }
    diff == 0
}

/// Render a `200 OK` broker response for `token`.
pub fn render_ok(response: &BrokerResponse) -> Zeroizing<String> {
    let body = Zeroizing::new(serde_json::to_string(response).unwrap_or_default());
    Zeroizing::new(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body.as_str()
    ))
}

/// Render an empty-bodied error response.
pub fn render_status(status: u16, reason: &str) -> String {
    format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn lookup(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn precedence_is_bot_then_access_then_broker() {
        let all = lookup(&[
            (BOT_TOKEN_ENV, "bzb_bot"),
            (ACCESS_TOKEN_ENV, "bzs_user"),
            (BROKER_URL_ENV, "http://127.0.0.1:9"),
            (BROKER_SECRET_ENV, "s"),
        ]);
        assert!(
            matches!(TokenSource::resolve(all), Some(TokenSource::BotToken(t)) if t.expose() == "bzb_bot")
        );
        let no_bot = lookup(&[
            (BOT_TOKEN_ENV, "  "),
            (ACCESS_TOKEN_ENV, "bzs_user"),
            (BROKER_URL_ENV, "http://127.0.0.1:9"),
            (BROKER_SECRET_ENV, "s"),
        ]);
        assert!(
            matches!(TokenSource::resolve(no_bot), Some(TokenSource::AccessToken(t)) if t.expose() == "bzs_user")
        );
        let broker = lookup(&[
            (BROKER_URL_ENV, "http://127.0.0.1:9"),
            (BROKER_SECRET_ENV, "s"),
        ]);
        assert!(matches!(
            TokenSource::resolve(broker),
            Some(TokenSource::Broker { .. })
        ));
        let half = lookup(&[(BROKER_URL_ENV, "http://127.0.0.1:9")]);
        assert!(TokenSource::resolve(half).is_none());
        assert!(TokenSource::resolve(lookup(&[])).is_none());
    }

    #[test]
    fn broker_url_must_be_loopback() {
        assert!(parse_broker_url("http://127.0.0.1:4000").is_ok());
        assert!(parse_broker_url("http://[::1]:4000/").is_ok());
        assert!(parse_broker_url("https://127.0.0.1:4000").is_err());
        assert!(parse_broker_url("http://10.0.0.1:4000").is_err());
        assert!(parse_broker_url("http://example.com:4000").is_err());
        assert!(parse_broker_url("http://127.0.0.1:4000/x").is_err());
        assert!(parse_broker_url("http://127.0.0.1:0").is_err());
    }

    #[test]
    fn debug_never_prints_secrets() {
        let source = TokenSource::BotToken(Secret::new("bzb_topsecret".into()));
        assert!(!format!("{source:?}").contains("topsecret"));
    }

    #[test]
    fn authorization_check() {
        let secret = Secret::new("abc".into());
        assert!(authorization_matches(Some("Bearer abc"), &secret));
        assert!(authorization_matches(Some("bearer abc"), &secret));
        assert!(!authorization_matches(Some("Bearer abd"), &secret));
        assert!(!authorization_matches(Some("Bearer ab"), &secret));
        assert!(!authorization_matches(Some("Basic abc"), &secret));
        assert!(!authorization_matches(None, &secret));
    }

    fn serve_once(response: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap();
            stream.write_all(&response).unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (url, handle)
    }

    #[test]
    fn fetch_round_trip_sends_secret_and_parses_token() {
        let body = BrokerResponse {
            token: Secret::new("bzb_abc".into()),
            expires_at: Some(42),
        };
        let (url, handle) = serve_once(render_ok(&body).as_bytes().to_vec());
        let response =
            fetch_from_broker(&url, &Secret::new("sek".into()), DEFAULT_BROKER_TIMEOUT).unwrap();
        assert_eq!(response.token.expose(), "bzb_abc");
        assert_eq!(response.expires_at, Some(42));
        let request = handle.join().unwrap();
        assert!(request.starts_with("GET /token HTTP/1.1\r\n"));
        assert!(request.contains("Authorization: Bearer sek\r\n"));
    }

    #[test]
    fn fetch_maps_status_and_rejects_oversize() {
        let (url, _h) = serve_once(render_status(401, "Unauthorized").into_bytes());
        let err =
            fetch_from_broker(&url, &Secret::new("x".into()), DEFAULT_BROKER_TIMEOUT).unwrap_err();
        assert!(matches!(err, BrokerError::Status(401)) && !err.is_transient());

        let mut big = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        big.extend(std::iter::repeat_n(b'x', MAX_RESPONSE_BYTES));
        let (url, _h) = serve_once(big);
        let err =
            fetch_from_broker(&url, &Secret::new("x".into()), DEFAULT_BROKER_TIMEOUT).unwrap_err();
        assert!(matches!(err, BrokerError::InvalidResponse(_)));
    }
}
