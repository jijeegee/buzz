//! Loopback token broker (centralized identity, Phase 1, plan §4.11, Q3).
//!
//! In token mode the harness's bot token must not reach agent processes:
//! their environment would expose it to every tool they run. Instead each
//! spawn gets `BUZZ_TOKEN_BROKER_URL` (this listener on `127.0.0.1`) and its
//! own random `BUZZ_TOKEN_BROKER_SECRET`. `buzz` and `git-credential-buzz`
//! call `GET /token` with `Authorization: Bearer <secret>` and receive the
//! harness's *current* token, so they keep working across exchanges.
//!
//! Bounds (Rule 4): requests are read up to [`MAX_REQUEST_BYTES`] within
//! [`REQUEST_TIMEOUT`], at most [`MAX_CONCURRENT`] connections are served at
//! once (extra ones are dropped), each spawn secret may fetch
//! [`BROKER_REQUESTS_PER_MINUTE`] times per minute (429 beyond), responses
//! stay under [`MAX_RESPONSE_BYTES`], and at most [`MAX_SECRETS`] spawn
//! secrets are live at once.
//!
//! Secret lifetime: a secret is a [`BrokerLease`] owned by the agent process
//! it was minted for (`AcpClient`). The agent's MCP servers are spawned by
//! that agent per session and reuse its lease's credentials, so they share
//! its lifetime. Dropping the lease (the agent process is dropped, killed or
//! respawned) unregisters the secret. Live secrets are never evicted: when
//! [`MAX_SECRETS`] are live, a new registration fails instead.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use buzz_token_broker::{
    authorization_matches, render_ok, render_status, BrokerResponse, Secret, BROKER_PATH,
    BROKER_REQUESTS_PER_MINUTE, BROKER_SECRET_ENV, BROKER_URL_ENV, MAX_RESPONSE_BYTES,
    TOKEN_ENV_VARS,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

use crate::identity::BotToken;

/// Largest request the broker reads.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024;
/// Time allowed to receive a request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections served concurrently.
pub const MAX_CONCURRENT: usize = 16;
/// Spawn secrets live at once (one per running agent process).
pub const MAX_SECRETS: usize = 256;

/// Registration refused: [`MAX_SECRETS`] secrets are live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("token broker has {MAX_SECRETS} live spawn secrets; refusing another agent spawn")]
pub struct BrokerFull;

struct SpawnSecret {
    secret: Secret,
    window_start: Instant,
    count: u32,
}

/// A running broker.
pub struct TokenBroker {
    addr: SocketAddr,
    token: BotToken,
    secrets: Mutex<Secrets>,
}

#[derive(Default)]
struct Secrets {
    next_id: u64,
    live: HashMap<u64, SpawnSecret>,
}

/// One live spawn secret. Dropping it unregisters the secret, so every
/// credential handed to a child dies with the child's owner.
pub struct BrokerLease {
    broker: Arc<TokenBroker>,
    id: u64,
    env: Vec<(String, String)>,
}

impl BrokerLease {
    /// The child's broker env vars (`BUZZ_TOKEN_BROKER_URL`,
    /// `BUZZ_TOKEN_BROKER_SECRET`).
    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }
}

impl std::fmt::Debug for BrokerLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrokerLease")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for BrokerLease {
    fn drop(&mut self) {
        let mut secrets = self.broker.lock_secrets();
        secrets.live.remove(&self.id);
        for (_, value) in &mut self.env {
            zeroize::Zeroize::zeroize(value);
        }
    }
}

impl std::fmt::Debug for TokenBroker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenBroker")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

static BROKER: OnceLock<Arc<TokenBroker>> = OnceLock::new();

/// The process broker, once token mode started it.
pub fn global() -> Option<&'static Arc<TokenBroker>> {
    BROKER.get()
}

/// Start the process-wide broker for `token` (token mode only). Idempotent.
pub async fn start_global(token: BotToken) -> std::io::Result<&'static Arc<TokenBroker>> {
    if let Some(broker) = BROKER.get() {
        return Ok(broker);
    }
    let broker = TokenBroker::start(token).await?;
    Ok(BROKER.get_or_init(|| broker))
}

/// A fresh random 32-byte secret, hex encoded.
fn random_secret() -> Secret {
    // `Keys::generate` draws 32 bytes from the OS CSPRNG.
    Secret::new(nostr::Keys::generate().secret_key().to_secret_hex())
}

impl TokenBroker {
    /// Bind `127.0.0.1:0` and serve until the process exits.
    pub async fn start(token: BotToken) -> std::io::Result<Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let broker = Arc::new(Self {
            addr: listener.local_addr()?,
            token,
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

    /// Register a new spawn secret. The secret stays valid until the
    /// returned lease is dropped; live secrets are never evicted.
    pub fn register(self: &Arc<Self>) -> Result<BrokerLease, BrokerFull> {
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
        Ok(BrokerLease {
            broker: Arc::clone(self),
            id,
            env: vec![
                (BROKER_URL_ENV.to_owned(), self.url()),
                (BROKER_SECRET_ENV.to_owned(), value),
            ],
        })
    }

    /// Live spawn secrets.
    pub fn live_secrets(&self) -> usize {
        self.lock_secrets().live.len()
    }

    /// Authorize one request: `Ok` when a known secret is presented and under
    /// its rate limit, otherwise the status to answer.
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

    /// The raw HTTP response for one request.
    fn respond(&self, request: &[u8]) -> Vec<u8> {
        let Some((method, path, authorization)) = parse_request(request) else {
            return render_status(400, "Bad Request").into_bytes();
        };
        if method != "GET" || path != BROKER_PATH {
            return render_status(404, "Not Found").into_bytes();
        }
        match self.authorize(authorization.as_deref()) {
            Ok(()) => {
                let body = BrokerResponse {
                    token: Secret::new(self.token.secret().to_string()),
                    expires_at: Some(self.token.expires_at()),
                };
                let rendered = render_ok(&body);
                if rendered.len() > MAX_RESPONSE_BYTES {
                    return render_status(500, "Internal Server Error").into_bytes();
                }
                rendered.as_bytes().to_vec()
            }
            Err(429) => render_status(429, "Too Many Requests").into_bytes(),
            Err(_) => render_status(401, "Unauthorized").into_bytes(),
        }
    }

    async fn serve(self: Arc<Self>, listener: TcpListener) {
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT));
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!("token broker accept failed: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                // Saturated: drop the connection rather than queue unboundedly.
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
        let mut response = self.respond(&request);
        let _ = tokio::time::timeout(REQUEST_TIMEOUT, stream.write_all(&response)).await;
        zeroize::Zeroize::zeroize(&mut response);
        let _ = stream.shutdown().await;
    }
}

/// Read until the end of the headers, at most [`MAX_REQUEST_BYTES`].
async fn read_request(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(buf);
        }
        if buf.len() > MAX_REQUEST_BYTES {
            return None;
        }
    }
}

/// `(method, path, Authorization)` of an HTTP/1.x request head.
fn parse_request(raw: &[u8]) -> Option<(String, String, Option<String>)> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_owned();
    let path = request_line.next()?.to_owned();
    if !request_line.next()?.starts_with("HTTP/1.") {
        return None;
    }
    let authorization = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("authorization")
            .then(|| value.trim().to_owned())
    });
    Some((method, path, authorization))
}

/// Scrub token credentials from a child command and, in token mode, give it
/// fresh broker credentials. Every agent spawn goes through this, after all
/// other environment is applied, so neither the inherited environment nor
/// persona/launch overrides can hand a child the bot token.
///
/// `broker` is the process broker ([`global`]) in production; tests pass
/// their own so they never install process-wide state. The returned lease
/// must live exactly as long as the child: its owner keeps it and drops it
/// with the child, which unregisters the secret.
pub fn apply_child_credentials(
    cmd: &mut tokio::process::Command,
    broker: Option<&Arc<TokenBroker>>,
) -> Result<Option<BrokerLease>, BrokerFull> {
    for name in TOKEN_ENV_VARS {
        cmd.env_remove(name);
    }
    let Some(broker) = broker else {
        return Ok(None);
    };
    // Token mode has no private key; never forward a stale one.
    cmd.env_remove("BUZZ_PRIVATE_KEY");
    let lease = broker.register()?;
    cmd.envs(lease.env().iter().cloned());
    Ok(Some(lease))
}

/// Give MCP server definitions the broker credentials of the agent process
/// that will spawn them (token mode), replacing any broker or token
/// variables already present. Key mode (`lease` is `None`) leaves them as is.
pub fn apply_mcp_credentials(servers: &mut [crate::acp::McpServer], lease: Option<&BrokerLease>) {
    let Some(lease) = lease else {
        return;
    };
    for server in servers {
        server.env.retain(|var| {
            var.name != BROKER_URL_ENV
                && var.name != BROKER_SECRET_ENV
                && var.name != "BUZZ_PRIVATE_KEY"
                && !TOKEN_ENV_VARS.contains(&var.name.as_str())
        });
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
    use buzz_token_broker::{fetch_from_broker, BrokerError, DEFAULT_BROKER_TIMEOUT};

    fn secret_of(env: &[(String, String)]) -> Secret {
        Secret::new(
            env.iter()
                .find(|(k, _)| k == BROKER_SECRET_ENV)
                .map(|(_, v)| v.clone())
                .unwrap(),
        )
    }

    async fn fetch(url: String, secret: Secret) -> Result<BrokerResponse, BrokerError> {
        tokio::task::spawn_blocking(move || {
            fetch_from_broker(&url, &secret, DEFAULT_BROKER_TIMEOUT)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn serves_the_current_token_to_a_spawn_secret() {
        let token = BotToken::new("bzb_first".into(), 100);
        let broker = TokenBroker::start(token.clone()).await.unwrap();
        let lease = broker.register().unwrap();
        let env = lease.env();
        assert!(env
            .iter()
            .any(|(k, v)| k == BROKER_URL_ENV && v == &broker.url()));
        let secret = secret_of(env);
        assert_eq!(secret.expose().len(), 64);

        let got = fetch(broker.url(), secret.clone()).await.unwrap();
        assert_eq!(got.token.expose(), "bzb_first");
        assert_eq!(got.expires_at, Some(100));

        // After an exchange the broker hands out the new token.
        token.adopt(0, "bzb_second".into(), 200).unwrap();
        let got = fetch(broker.url(), secret).await.unwrap();
        assert_eq!(got.token.expose(), "bzb_second");
    }

    #[tokio::test]
    async fn rejects_unknown_secret_and_other_paths() {
        let broker = TokenBroker::start(BotToken::new("bzb_x".into(), 0))
            .await
            .unwrap();
        let _lease = broker.register().unwrap();
        let err = fetch(broker.url(), Secret::new("nope".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, BrokerError::Status(401)), "{err:?}");
        assert!(broker
            .respond(b"GET /other HTTP/1.1\r\n\r\n")
            .starts_with(b"HTTP/1.1 404"));
        assert!(broker.respond(b"garbage").starts_with(b"HTTP/1.1 400"));
    }

    fn offline_broker() -> Arc<TokenBroker> {
        Arc::new(TokenBroker {
            addr: "127.0.0.1:1".parse().unwrap(),
            token: BotToken::new("bzb_x".into(), 0),
            secrets: Mutex::new(Secrets::default()),
        })
    }

    fn bearer(lease: &BrokerLease) -> String {
        format!("Bearer {}", secret_of(lease.env()).expose())
    }

    #[test]
    fn rate_limits_each_secret() {
        let broker = offline_broker();
        let a = broker.register().unwrap();
        let b = broker.register().unwrap();
        for _ in 0..BROKER_REQUESTS_PER_MINUTE {
            assert_eq!(broker.authorize(Some(&bearer(&a))), Ok(()));
        }
        assert_eq!(broker.authorize(Some(&bearer(&a))), Err(429));
        assert!(broker
            .respond(
                format!(
                    "GET /token HTTP/1.1\r\nAuthorization: {}\r\n\r\n",
                    bearer(&a)
                )
                .as_bytes()
            )
            .starts_with(b"HTTP/1.1 429"));
        assert_eq!(
            broker.authorize(Some(&bearer(&b))),
            Ok(()),
            "per-secret window"
        );
        assert_eq!(broker.authorize(None), Err(401));
    }

    /// Each registration is a distinct secret, dropping the lease revokes it
    /// at once, and a full table refuses new spawns instead of evicting a
    /// live secret.
    #[test]
    fn leases_are_per_spawn_revoked_on_drop_and_never_evicted() {
        let broker = offline_broker();
        let first = broker.register().unwrap();
        let second = broker.register().unwrap();
        assert_ne!(bearer(&first), bearer(&second), "per-spawn secret");
        let first_header = bearer(&first);
        drop(first);
        assert_eq!(broker.authorize(Some(&first_header)), Err(401), "revoked");
        assert_eq!(broker.authorize(Some(&bearer(&second))), Ok(()));

        let mut held: Vec<BrokerLease> = (1..MAX_SECRETS)
            .map(|_| broker.register().unwrap())
            .collect();
        assert_eq!(broker.live_secrets(), MAX_SECRETS);
        assert_eq!(broker.register().err(), Some(BrokerFull));
        assert_eq!(
            broker.authorize(Some(&bearer(&second))),
            Ok(()),
            "oldest live secret survives a full table"
        );
        held.pop();
        assert!(broker.register().is_ok(), "a freed slot is reusable");
    }

    fn child_env_command() -> tokio::process::Command {
        if cfg!(windows) {
            let mut cmd = tokio::process::Command::new("cmd");
            cmd.args(["/C", "set"]);
            cmd
        } else {
            tokio::process::Command::new("env")
        }
    }

    async fn child_env(mut cmd: tokio::process::Command) -> String {
        let out = cmd.output().await.expect("spawn env dump");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A real child process: whatever the command carried (inherited or
    /// persona/launch env), token mode leaves it no bot token or private key,
    /// only per-spawn broker credentials that actually work against the broker.
    #[tokio::test]
    async fn token_mode_child_gets_broker_credentials_and_no_token() {
        let broker = TokenBroker::start(BotToken::new("bzb_harness".into(), 7))
            .await
            .unwrap();
        let mut cmd = child_env_command();
        cmd.env("BUZZ_BOT_TOKEN", "bzb_harness")
            .env("BUZZ_ACCESS_TOKEN", "bzs_leak")
            .env("BUZZ_PRIVATE_KEY", "nsec_leak")
            .env("BUZZ_TOKEN_BROKER_SECRET", "forged");
        let lease = apply_child_credentials(&mut cmd, Some(&broker))
            .unwrap()
            .expect("token mode leases a secret");
        let env = child_env(cmd).await;
        for leaked in [
            "BUZZ_BOT_TOKEN=",
            "BUZZ_ACCESS_TOKEN=",
            "BUZZ_PRIVATE_KEY=",
            "bzb_harness",
            "forged",
        ] {
            assert!(!env.contains(leaked), "child env leaked {leaked}");
        }
        let url = env
            .lines()
            .find_map(|l| l.strip_prefix("BUZZ_TOKEN_BROKER_URL="))
            .expect("broker url injected")
            .trim()
            .to_owned();
        let secret = env
            .lines()
            .find_map(|l| l.strip_prefix("BUZZ_TOKEN_BROKER_SECRET="))
            .expect("broker secret injected")
            .trim()
            .to_owned();
        let got = fetch(url.clone(), Secret::new(secret.clone()))
            .await
            .unwrap();
        assert_eq!(got.token.expose(), "bzb_harness");
        drop(lease);
        let err = fetch(url, Secret::new(secret)).await.unwrap_err();
        assert!(matches!(err, BrokerError::Status(401)), "{err:?}");
    }

    /// Key mode: token variables are still scrubbed, nothing is injected and
    /// the key-mode private key is left alone.
    #[tokio::test]
    async fn key_mode_child_keeps_private_key_and_gets_no_broker() {
        let mut cmd = child_env_command();
        cmd.env("BUZZ_BOT_TOKEN", "bzb_stray")
            .env("BUZZ_PRIVATE_KEY", "nsec_keymode");
        assert!(apply_child_credentials(&mut cmd, None).unwrap().is_none());
        let env = child_env(cmd).await;
        assert!(!env.contains("bzb_stray"));
        assert!(!env.contains("BUZZ_TOKEN_BROKER_URL="));
        assert!(env.contains("BUZZ_PRIVATE_KEY=nsec_keymode"));
    }

    #[test]
    fn oversized_request_is_not_parsed() {
        let mut raw = b"GET /token HTTP/1.1\r\nX: ".to_vec();
        raw.extend(std::iter::repeat_n(b'a', MAX_REQUEST_BYTES));
        assert!(parse_request(&raw).is_none(), "no header terminator");
        let parsed = parse_request(b"GET /token HTTP/1.1\r\nauthorization: Bearer s\r\n\r\n");
        assert_eq!(
            parsed,
            Some(("GET".into(), "/token".into(), Some("Bearer s".into())))
        );
    }
}
