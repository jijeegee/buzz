#![deny(unsafe_code)]
//! git-credential-buzz — bearer-token git credential helper for Buzz.
//!
//! Git calls this through the credential-helper protocol (stdin/stdout). For a
//! `get` on the Buzz relay's host it answers
//!
//! ```text
//! username=token
//! password=<access token>
//! ```
//!
//! and git sends `Authorization: Basic base64(token:<token>)`, which the relay's
//! git smart-HTTP transport accepts when token auth is enabled.
//!
//! Token sources, in order: `BUZZ_BOT_TOKEN`, `BUZZ_ACCESS_TOKEN`, the
//! `buzz-acp` loopback broker (`BUZZ_TOKEN_BROKER_URL` + `_SECRET`), then the
//! file named by `git config buzz.tokenfile`.
//!
//! The token is only ever released to the Buzz relay host: the request's
//! `host` must equal the host of `BUZZ_RELAY_URL` (or `git config buzz.host`).
//! Any other host gets no answer, so git falls through to its next helper.
//! The request's `protocol` must also be `https`; plain `http` is accepted
//! only when `BUZZ_RELAY_URL` itself is a plain `ws://`/`http://` URL (a
//! local relay). A `git config buzz.host` relay is always https-only.

use std::io::{self, BufRead, Write};

use buzz_token_broker::{Secret, TokenSource};

/// Git username paired with a bearer token.
pub const TOKEN_USERNAME: &str = "token";

/// Upper bound on a `buzz.tokenfile` (tokens are ~50 bytes).
const MAX_TOKENFILE_BYTES: u64 = 1024;

/// A parsed credential request.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CredRequest {
    /// `protocol=`.
    pub protocol: Option<String>,
    /// `host=` (may include `:port`).
    pub host: Option<String>,
}

/// Parse git's `key=value` request lines up to the first blank line.
pub fn parse_request(input: impl BufRead) -> CredRequest {
    let mut req = CredRequest::default();
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("protocol=") {
            req.protocol = Some(v.to_owned());
        } else if let Some(v) = line.strip_prefix("host=") {
            req.host = Some(v.to_owned());
        }
    }
    req
}

/// The relay a token may be released to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTarget {
    /// `host[:port]`, lowercased.
    pub host: String,
    /// The relay itself is plain-text (`ws://`/`http://`), so git over plain
    /// `http` to it is the relay's own transport rather than a downgrade.
    pub plain: bool,
}

impl RelayTarget {
    /// Parse a relay URL (`ws[s]://` or `http[s]://`).
    pub fn from_url(relay_url: &str) -> Option<Self> {
        let trimmed = relay_url.trim();
        let (rest, plain) = [
            ("wss://", false),
            ("https://", false),
            ("ws://", true),
            ("http://", true),
        ]
        .iter()
        .find_map(|(scheme, plain)| trimmed.strip_prefix(scheme).map(|rest| (rest, *plain)))?;
        let authority = rest.split(['/', '?', '#']).next()?;
        let authority = authority.rsplit('@').next()?;
        (!authority.is_empty()).then(|| Self {
            host: authority.to_ascii_lowercase(),
            plain,
        })
    }

    /// A bare `git config buzz.host` value: https only.
    pub fn from_host(host: &str) -> Option<Self> {
        let host = host.trim();
        (!host.is_empty()).then(|| Self {
            host: host.to_ascii_lowercase(),
            plain: false,
        })
    }
}

/// The `host[:port]` of a relay URL (`ws[s]://` or `http[s]://`).
pub fn relay_host(relay_url: &str) -> Option<String> {
    RelayTarget::from_url(relay_url).map(|target| target.host)
}

/// Whether git's request `protocol` may carry the token to `relay`: always
/// `https`; `http` only when the relay itself is plain-text.
pub fn protocol_allowed(protocol: Option<&str>, relay: &RelayTarget) -> bool {
    match protocol {
        Some("https") => true,
        Some("http") => relay.plain,
        _ => false,
    }
}

/// The release decision `run` applies: the request names the relay's host
/// over an allowed protocol.
pub fn should_answer(req: &CredRequest, relay: &RelayTarget) -> bool {
    let Some(request_host) = req.host.as_deref() else {
        return false;
    };
    protocol_allowed(req.protocol.as_deref(), relay)
        && host_matches(request_host, &relay.host, req.protocol.as_deref())
}

/// Whether `request_host` is the Buzz relay's host. A default port on either
/// side is ignored (`h` matches `h:443` for https).
pub fn host_matches(request_host: &str, relay: &str, protocol: Option<&str>) -> bool {
    let strip_default = |host: &str| -> String {
        let host = host.to_ascii_lowercase();
        let default = match protocol {
            Some("https") => ":443",
            Some("http") => ":80",
            _ => "",
        };
        if !default.is_empty() {
            if let Some(stripped) = host.strip_suffix(default) {
                return stripped.to_owned();
            }
        }
        host
    };
    strip_default(request_host) == strip_default(relay)
}

fn git_config(key: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["config", "--get", key])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn token_from_file(path: &str) -> Result<Secret, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot stat tokenfile {path}: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_TOKENFILE_BYTES {
        return Err(format!("tokenfile {path} is not a small regular file"));
    }
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("cannot read tokenfile {path}: {e}"))?;
    let token = raw.trim();
    if token.is_empty() {
        return Err(format!("tokenfile {path} is empty"));
    }
    Ok(Secret::new(token.to_owned()))
}

/// Resolve the token with the documented precedence.
pub fn resolve_token(
    source: Option<TokenSource>,
    tokenfile: Option<String>,
) -> Result<Option<Secret>, String> {
    if let Some(source) = source {
        return source.token().map(Some).map_err(|e| e.to_string());
    }
    tokenfile.map(|path| token_from_file(&path)).transpose()
}

/// Render the `get` answer.
pub fn render_answer(token: &Secret) -> String {
    format!("username={TOKEN_USERNAME}\npassword={}\n\n", token.expose())
}

/// Run the helper. Returns the process exit code. Errors go to stderr only.
pub fn run() -> i32 {
    match std::env::args().nth(1).as_deref() {
        Some("get") | None => {}
        Some(_) => return 0, // store / erase / unknown: nothing to do
    }
    let req = parse_request(io::stdin().lock());

    if req.host.is_none() {
        return 0;
    }
    let relay = std::env::var("BUZZ_RELAY_URL")
        .ok()
        .and_then(|url| RelayTarget::from_url(&url))
        .or_else(|| git_config("buzz.host").and_then(|host| RelayTarget::from_host(&host)));
    let Some(relay) = relay else {
        // Not configured for a Buzz relay: let git try the next helper.
        return 0;
    };
    if !should_answer(&req, &relay) {
        return 0;
    }

    let token = match resolve_token(TokenSource::from_env(), git_config("buzz.tokenfile")) {
        Ok(Some(token)) => token,
        Ok(None) => return 0,
        Err(error) => {
            eprintln!("git-credential-buzz: {error}");
            return 1;
        }
    };
    let mut stdout = io::stdout().lock();
    if stdout.write_all(render_answer(&token).as_bytes()).is_err() {
        return 1;
    }
    let _ = stdout.flush();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_until_blank_line() {
        let req = parse_request(
            "protocol=https\nhost=relay.example\npath=x\n\nhost=ignored\n".as_bytes(),
        );
        assert_eq!(req.protocol.as_deref(), Some("https"));
        assert_eq!(req.host.as_deref(), Some("relay.example"));
    }

    #[test]
    fn relay_host_and_matching() {
        assert_eq!(
            relay_host("wss://Relay.Example/x").as_deref(),
            Some("relay.example")
        );
        assert_eq!(
            relay_host("ws://127.0.0.1:3000").as_deref(),
            Some("127.0.0.1:3000")
        );
        assert_eq!(relay_host("relay.example"), None);
        assert!(host_matches(
            "relay.example:443",
            "relay.example",
            Some("https")
        ));
        assert!(host_matches(
            "127.0.0.1:3000",
            "127.0.0.1:3000",
            Some("http")
        ));
        assert!(!host_matches("github.com", "relay.example", Some("https")));
        assert!(!host_matches(
            "127.0.0.1:3001",
            "127.0.0.1:3000",
            Some("http")
        ));
    }

    fn req(protocol: &str, host: &str) -> CredRequest {
        CredRequest {
            protocol: Some(protocol.into()),
            host: Some(host.into()),
        }
    }

    #[test]
    fn tls_relay_releases_only_over_https() {
        let relay = RelayTarget::from_url("wss://relay.example").unwrap();
        assert!(!relay.plain);
        assert!(should_answer(&req("https", "relay.example"), &relay));
        assert!(
            !should_answer(&req("http", "relay.example"), &relay),
            "an http:// remote on a TLS relay's host must not get the token"
        );
        assert!(!should_answer(&req("http", "relay.example:80"), &relay));
        assert!(!should_answer(
            &CredRequest {
                protocol: None,
                host: Some("relay.example".into()),
            },
            &relay
        ));
        let https = RelayTarget::from_url("https://relay.example/x").unwrap();
        assert!(!should_answer(&req("http", "relay.example"), &https));
    }

    #[test]
    fn plain_relay_accepts_its_own_http_and_https() {
        let relay = RelayTarget::from_url("ws://127.0.0.1:3000").unwrap();
        assert!(relay.plain);
        assert!(should_answer(&req("http", "127.0.0.1:3000"), &relay));
        assert!(should_answer(&req("https", "127.0.0.1:3000"), &relay));
        assert!(!should_answer(&req("ssh", "127.0.0.1:3000"), &relay));
    }

    #[test]
    fn configured_bare_host_is_https_only() {
        let relay = RelayTarget::from_host("Relay.Example").unwrap();
        assert!(should_answer(&req("https", "relay.example"), &relay));
        assert!(!should_answer(&req("http", "relay.example"), &relay));
        assert_eq!(RelayTarget::from_host("  "), None);
    }

    #[test]
    fn env_source_wins_over_tokenfile() {
        let source = TokenSource::BotToken(Secret::new("bzb_env".into()));
        let token = resolve_token(Some(source), Some("/nonexistent".into())).unwrap();
        assert_eq!(token.unwrap().expose(), "bzb_env");
        assert!(resolve_token(None, None).unwrap().is_none());
        assert!(resolve_token(None, Some("/nonexistent/tok".into())).is_err());
    }

    #[test]
    fn tokenfile_is_read_and_trimmed() {
        let dir = std::env::temp_dir().join(format!("gcb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tok");
        std::fs::write(&path, "bzk_file\n").unwrap();
        let token = resolve_token(None, Some(path.to_string_lossy().into_owned())).unwrap();
        assert_eq!(token.unwrap().expose(), "bzk_file");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn answer_uses_token_username() {
        let answer = render_answer(&Secret::new("bzb_x".into()));
        assert_eq!(answer, "username=token\npassword=bzb_x\n\n");
    }
}
