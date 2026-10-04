//! Loopback redirect listener for the desktop Google login (RFC 8252 §7.3).
//!
//! Bound to `127.0.0.1:0` only for the duration of one login. It accepts
//! exactly `GET /cb?code=bzl_…&state=…`; a request whose `state` differs from
//! the one this login sent fails the login (the code is discarded), and the
//! whole wait is bounded by a deadline. Other paths get a 404 and the wait
//! continues, so a stray probe cannot end the login.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zeroize::Zeroizing;

/// How long the user has to finish signing in.
pub(crate) const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Per-connection read deadline.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on a request head.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

const SUCCESS_PAGE: &str =
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>Buzz</title></head>\
<body style=\"font-family:system-ui,sans-serif;text-align:center;padding-top:4rem\">\
<h1>Signed in</h1><p>You can close this tab and return to Buzz.</p></body></html>";

const FAILURE_PAGE: &str =
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>Buzz</title></head>\
<body style=\"font-family:system-ui,sans-serif;text-align:center;padding-top:4rem\">\
<h1>Sign-in failed</h1><p>Return to Buzz and try again.</p></body></html>";

/// Why the loopback wait ended without a login code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoopbackError {
    /// The user did not finish within the deadline.
    Timeout,
    /// The callback's `state` did not match this login.
    StateMismatch,
    /// The relay or identity provider reported an error (`error=`).
    Denied(String),
    /// The callback carried no usable login code.
    Malformed,
    /// Socket failure.
    Io(String),
}

impl std::fmt::Display for LoopbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("sign-in timed out"),
            Self::StateMismatch => f.write_str("sign-in response did not match this request"),
            Self::Denied(reason) => write!(f, "sign-in was refused ({reason})"),
            Self::Malformed => f.write_str("sign-in response was malformed"),
            Self::Io(error) => write!(f, "sign-in listener failed: {error}"),
        }
    }
}

/// A bound loopback listener.
pub(crate) struct LoopbackListener {
    listener: TcpListener,
    port: u16,
}

impl LoopbackListener {
    /// Bind `127.0.0.1` on an ephemeral port.
    pub(crate) async fn bind() -> Result<Self, LoopbackError> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|e| LoopbackError::Io(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| LoopbackError::Io(e.to_string()))?
            .port();
        Ok(Self { listener, port })
    }

    /// The `redirect_uri` the relay must send the browser back to.
    pub(crate) fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/cb", self.port)
    }

    /// Wait for the callback carrying `expected_state` and return its login
    /// code. Consumes the listener: the port closes when this returns.
    pub(crate) async fn wait_for_code(
        self,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<Zeroizing<String>, LoopbackError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let accepted = tokio::time::timeout_at(deadline, self.listener.accept()).await;
            let (stream, _) = match accepted {
                Err(_) => return Err(LoopbackError::Timeout),
                Ok(Err(error)) => return Err(LoopbackError::Io(error.to_string())),
                Ok(Ok(conn)) => conn,
            };
            match handle_connection(stream, expected_state).await {
                Callback::Ignored => continue,
                Callback::Done(result) => return result,
            }
        }
    }
}

enum Callback {
    Ignored,
    Done(Result<Zeroizing<String>, LoopbackError>),
}

async fn handle_connection(mut stream: TcpStream, expected_state: &str) -> Callback {
    let Some(head) = read_head(&mut stream).await else {
        return Callback::Ignored;
    };
    let target = match parse_request_target(&head) {
        Some(target) => target,
        None => {
            respond(&mut stream, "400 Bad Request", FAILURE_PAGE).await;
            return Callback::Ignored;
        }
    };
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    if path != "/cb" {
        respond(&mut stream, "404 Not Found", "").await;
        return Callback::Ignored;
    }
    let result = evaluate_callback(query, expected_state);
    let (status, page) = if result.is_ok() {
        ("200 OK", SUCCESS_PAGE)
    } else {
        ("400 Bad Request", FAILURE_PAGE)
    };
    respond(&mut stream, status, page).await;
    Callback::Done(result)
}

/// Decide a `/cb` callback from its query string. Pure, for tests.
pub(crate) fn evaluate_callback(
    query: &str,
    expected_state: &str,
) -> Result<Zeroizing<String>, LoopbackError> {
    let mut state = None;
    let mut code = None;
    let mut error = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "state" => state = Some(value.into_owned()),
            "code" => code = Some(Zeroizing::new(value.into_owned())),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    if !state
        .as_deref()
        .is_some_and(|state| constant_time_eq(state.as_bytes(), expected_state.as_bytes()))
    {
        return Err(LoopbackError::StateMismatch);
    }
    if let Some(error) = error {
        let reason: String = error
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(64)
            .collect();
        return Err(LoopbackError::Denied(reason));
    }
    match code {
        Some(code) if code.starts_with("bzl_") && code.len() <= 256 => Ok(code),
        _ => Err(LoopbackError::Malformed),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = u8::from(a.len() != b.len());
    for (i, byte) in b.iter().enumerate() {
        diff |= a.get(i).copied().unwrap_or(0) ^ byte;
    }
    diff == 0
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + READ_TIMEOUT;
    loop {
        let n = tokio::time::timeout_at(deadline, stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return String::from_utf8(buf).ok();
        }
        if buf.len() > MAX_REQUEST_BYTES {
            return None;
        }
    }
}

/// `GET <target> HTTP/1.x` → `<target>`.
fn parse_request_target(head: &str) -> Option<String> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    (method == "GET" && version.starts_with("HTTP/1.")).then(|| target.to_owned())
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_rules() {
        assert_eq!(
            evaluate_callback("code=bzl_abc&state=s1", "s1").map(|c| c.to_string()),
            Ok("bzl_abc".to_string())
        );
        assert_eq!(
            evaluate_callback("code=bzl_abc&state=other", "s1"),
            Err(LoopbackError::StateMismatch)
        );
        assert_eq!(
            evaluate_callback("code=bzl_abc", "s1"),
            Err(LoopbackError::StateMismatch)
        );
        assert_eq!(
            evaluate_callback("error=access_denied&state=s1", "s1"),
            Err(LoopbackError::Denied("access_denied".into()))
        );
        assert_eq!(
            evaluate_callback("code=nope&state=s1", "s1"),
            Err(LoopbackError::Malformed)
        );
    }

    async fn get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut out = String::new();
        let _ = stream.read_to_string(&mut out).await;
        out
    }

    #[tokio::test]
    async fn listener_returns_code_for_matching_state_after_ignoring_strays() {
        let listener = LoopbackListener::bind().await.unwrap();
        let port: u16 = listener
            .redirect_uri()
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/cb")
            .parse()
            .unwrap();
        let waiter = tokio::spawn(async move { listener.wait_for_code("st", LOGIN_TIMEOUT).await });
        let stray = get(port, "/favicon.ico").await;
        assert!(stray.starts_with("HTTP/1.1 404"), "{stray}");
        let page = get(port, "/cb?code=bzl_xyz&state=st").await;
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        let code = waiter.await.unwrap().unwrap();
        assert_eq!(code.as_str(), "bzl_xyz");
    }

    #[tokio::test]
    async fn listener_rejects_wrong_state() {
        let listener = LoopbackListener::bind().await.unwrap();
        let uri = listener.redirect_uri();
        let port: u16 = uri[17..uri.len() - 3].parse().unwrap();
        let waiter =
            tokio::spawn(async move { listener.wait_for_code("good", LOGIN_TIMEOUT).await });
        let page = get(port, "/cb?code=bzl_xyz&state=evil").await;
        assert!(page.starts_with("HTTP/1.1 400"), "{page}");
        assert_eq!(
            waiter.await.unwrap().map(|c| c.to_string()),
            Err(LoopbackError::StateMismatch)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn listener_times_out() {
        let listener = LoopbackListener::bind().await.unwrap();
        let result = listener
            .wait_for_code("st", Duration::from_secs(300))
            .await
            .map(|c| c.to_string());
        assert_eq!(result, Err(LoopbackError::Timeout));
    }
}
