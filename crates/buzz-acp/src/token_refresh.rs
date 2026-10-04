//! Token-mode self-refresh (centralized identity, Phase 1, plan §4.11).
//!
//! A Desktop-hosted bot token (`bzb_`) lives one hour and has no refresh
//! token. Fifteen minutes before it expires the harness swaps it with
//! `POST /auth/token/exchange` ("exchange while valid"), adopts the new token
//! only if no newer one was adopted meanwhile (generation fence, Rule 2), and
//! immediately re-AUTHs the *same* WebSocket so the old token's revocation
//! after its 60 s grace never touches the connection.
//!
//! Failures are classified once, here:
//! - network errors, 408, 429 and 5xx are transient: retried with the startup
//!   backoff ladder, then once a minute until the token expires;
//! - every other HTTP rejection (401 `token_expired|token_revoked|
//!   principal_disabled|invalid_token`, 409 `token_superseded`, 403) and a WS
//!   `OK auth false` carrying one of those codes is terminal: the process
//!   exits with [`EXIT_AUTH_TERMINAL`] and Buzz Desktop reissues a token.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::watch;

use crate::identity::{BotToken, TokenPrincipal, EXIT_AUTH_TERMINAL};
use crate::relay::{RelayReauthHandle, STARTUP_CONNECT_BACKOFFS};

/// `OK` label the relay uses for token AUTH replies.
pub const TOKEN_AUTH_OK_ID: &str = "auth";

/// Assumed lifetime of an initial token whose expiry Desktop did not pass.
pub const DEFAULT_TOKEN_TTL_SECS: i64 = 3600;

/// Exchange this long before the current token expires.
pub const REFRESH_LEAD_SECS: i64 = 15 * 60;

/// Retry interval once the startup backoff ladder is exhausted.
const STEADY_RETRY: Duration = Duration::from_secs(60);

/// The relay's exchange replay window: re-presenting an already-exchanged
/// token within this long returns the same successor instead of
/// `token_superseded` (plan §4.6). Shared with the relay's replay cache.
pub const EXCHANGE_REPLAY_WINDOW: Duration =
    Duration::from_secs(buzz_core::principal::TOKEN_EXCHANGE_REPLAY_WINDOW_SECS);

/// HTTP timeout for `/auth/token/exchange`. It must be shorter than
/// [`EXCHANGE_REPLAY_WINDOW`]: an exchange that succeeded server-side but
/// timed out here is retried inside the window and gets the same successor.
pub const EXCHANGE_HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// How long a graceful auth-terminal shutdown may take before a hard exit.
const TERMINAL_EXIT_GRACE: Duration = Duration::from_secs(10);

/// Codes that make a token permanently unusable for this process.
const TERMINAL_CODES: [&str; 5] = [
    "token_expired",
    "token_revoked",
    "principal_disabled",
    "invalid_token",
    "token_superseded",
];

/// Current unix time in seconds.
pub fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Whether a relay auth message (`OK auth false <message>` or a close NOTICE)
/// carries a terminal token code.
pub fn is_terminal_auth_message(message: &str) -> bool {
    TERMINAL_CODES.iter().any(|code| message.contains(code))
}

/// How a failed token request should be handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// Retry later.
    Transient(String),
    /// Exit with [`EXIT_AUTH_TERMINAL`].
    Terminal(String),
}

/// Classify a non-2xx HTTP response from `/auth/*`.
pub fn classify_http(status: u16, body: &str) -> Failure {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned))
        .unwrap_or_default();
    let detail = format!("HTTP {status} {code}");
    if status == 408 || status == 429 || (500..600).contains(&status) {
        Failure::Transient(detail)
    } else {
        Failure::Terminal(detail)
    }
}

static AUTH_TERMINAL: AtomicBool = AtomicBool::new(false);
static SHUTDOWN: OnceLock<watch::Sender<()>> = OnceLock::new();

/// Register the harness shutdown channel so an auth-terminal failure drains
/// gracefully before exiting.
pub fn register_shutdown(sender: watch::Sender<()>) {
    let _ = SHUTDOWN.set(sender);
}

/// Whether an auth-terminal failure was raised in this process.
pub fn auth_terminal_requested() -> bool {
    AUTH_TERMINAL.load(Ordering::Acquire)
}

/// Raise an unrecoverable token failure: request a graceful shutdown and make
/// the process exit with [`EXIT_AUTH_TERMINAL`] (hard exit after a short
/// grace if the drain stalls; immediate exit before the harness registered
/// its shutdown channel).
pub fn auth_terminal(reason: &str) {
    if AUTH_TERMINAL.swap(true, Ordering::AcqRel) {
        return;
    }
    tracing::error!(
        exit_code = EXIT_AUTH_TERMINAL,
        "bot token is no longer usable ({reason}); exiting for Desktop to reissue it"
    );
    if cfg!(test) {
        return;
    }
    match SHUTDOWN.get() {
        Some(shutdown) => {
            let _ = shutdown.send(());
            std::thread::spawn(|| {
                std::thread::sleep(TERMINAL_EXIT_GRACE);
                std::process::exit(EXIT_AUTH_TERMINAL);
            });
        }
        None => std::process::exit(EXIT_AUTH_TERMINAL),
    }
}

/// Parse `expires_at` from a token response: unix seconds or RFC 3339.
pub fn parse_expires_at(value: Option<&serde_json::Value>) -> Option<i64> {
    match value? {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.timestamp()),
        _ => None,
    }
}

/// One `POST /auth/token/exchange` with `token`.
pub async fn exchange_once(
    http: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<(String, i64), Failure> {
    let response = http
        .post(format!("{base_url}/auth/token/exchange"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{}")
        .send()
        .await
        .map_err(|e| Failure::Transient(format!("exchange request failed: {e}")))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|e| Failure::Transient(format!("exchange body failed: {e}")))?;
    if !(200..300).contains(&status) {
        return Err(classify_http(status, &body));
    }
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| Failure::Terminal("exchange returned invalid JSON".into()))?;
    let new_token = json
        .get("token")
        .and_then(serde_json::Value::as_str)
        .filter(|t| t.starts_with("bzb_"))
        .ok_or_else(|| Failure::Terminal("exchange response has no bzb_ token".into()))?
        .to_owned();
    let expires_at = parse_expires_at(json.get("expires_at"))
        .unwrap_or_else(|| unix_now() + DEFAULT_TOKEN_TTL_SECS);
    Ok((new_token, expires_at))
}

/// `GET /auth/me` once.
async fn me_once(
    http: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<TokenPrincipal, Failure> {
    let response = http
        .get(format!("{base_url}/auth/me"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| Failure::Transient(format!("/auth/me request failed: {e}")))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|e| Failure::Transient(format!("/auth/me body failed: {e}")))?;
    if !(200..300).contains(&status) {
        return Err(classify_http(status, &body));
    }
    let json: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| Failure::Terminal("/auth/me returned invalid JSON".into()))?;
    crate::identity::parse_me_response(&json).map_err(Failure::Terminal)
}

/// Resolve the bot token's principal at startup (`GET /auth/me`), retrying
/// transient failures over the startup backoff ladder.
pub async fn resolve_principal(
    relay_ws_url: &str,
    token: &str,
    backoffs: &[Duration],
) -> Result<TokenPrincipal, Failure> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| Failure::Transient(format!("HTTP client: {e}")))?;
    let base_url = crate::relay::relay_ws_to_http(relay_ws_url);
    let mut last = Failure::Transient("not attempted".into());
    for delay in std::iter::once(Duration::ZERO).chain(backoffs.iter().copied()) {
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        match me_once(&http, &base_url, token).await {
            Ok(principal) => return Ok(principal),
            Err(Failure::Terminal(reason)) => return Err(Failure::Terminal(reason)),
            Err(transient) => {
                tracing::warn!("resolving bot principal failed: {transient:?}");
                last = transient;
            }
        }
    }
    Err(last)
}

/// Outcome of one refresh attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Exchanged, adopted as this generation, and re-AUTHed.
    Refreshed { generation: u64 },
    /// Exchanged but a newer token was already adopted; result discarded.
    Stale,
    /// Transient failure; retry later.
    Retry(String),
    /// Unrecoverable; the process must exit 78.
    Terminal(String),
}

/// Exchange the current token, adopt the result, and re-AUTH the connection.
pub async fn refresh_once(
    http: &reqwest::Client,
    base_url: &str,
    token: &BotToken,
    reauth: &RelayReauthHandle,
) -> RefreshOutcome {
    let generation = token.generation();
    let current = token.secret();
    match exchange_once(http, base_url, &current).await {
        Ok((new_token, expires_at)) => match token.adopt(generation, new_token, expires_at) {
            Some(next) => {
                match reauth.reauth().await {
                    Ok(()) => {
                        tracing::info!(generation = next, "bot token exchanged and re-AUTHed")
                    }
                    Err(message) if is_terminal_auth_message(&message) => {
                        return RefreshOutcome::Terminal(format!("re-AUTH rejected: {message}"))
                    }
                    // The relay keeps the old binding; its grace expiry closes
                    // the socket and the reconnect authenticates with the new
                    // token.
                    Err(message) => tracing::warn!("re-AUTH after exchange failed: {message}"),
                }
                RefreshOutcome::Refreshed { generation: next }
            }
            None => RefreshOutcome::Stale,
        },
        Err(Failure::Transient(reason)) => RefreshOutcome::Retry(reason),
        Err(Failure::Terminal(reason)) => RefreshOutcome::Terminal(reason),
    }
}

/// Seconds to wait before refreshing a token that expires at `expires_at`.
pub fn refresh_delay(expires_at: i64, now: i64) -> Duration {
    Duration::from_secs(u64::try_from(expires_at - REFRESH_LEAD_SECS - now).unwrap_or(0))
}

/// Wait before retry `attempt` (1-based) of a failed exchange: the startup
/// backoff ladder, then [`STEADY_RETRY`] forever.
fn retry_delay(attempt: usize) -> Duration {
    attempt
        .checked_sub(1)
        .and_then(|rung| STARTUP_CONNECT_BACKOFFS.get(rung))
        .copied()
        .unwrap_or(STEADY_RETRY)
}

/// Spawn the refresh loop for the harness's bot token. Runs for the life of
/// the process; raises [`auth_terminal`] on an unrecoverable failure.
pub fn spawn_refresh_task(
    token: BotToken,
    relay_ws_url: &str,
    reauth: RelayReauthHandle,
) -> tokio::task::JoinHandle<()> {
    let base_url = crate::relay::relay_ws_to_http(relay_ws_url);
    tokio::spawn(async move {
        let http = match reqwest::Client::builder()
            .timeout(EXCHANGE_HTTP_TIMEOUT)
            .build()
        {
            Ok(http) => http,
            Err(error) => {
                tracing::error!("token refresh disabled: HTTP client: {error}");
                return;
            }
        };
        let reason = refresh_loop(&http, &base_url, &token, &reauth).await;
        auth_terminal(&reason);
    })
}

/// The refresh loop: exchange [`REFRESH_LEAD_SECS`] before expiry, retry
/// transient failures on [`retry_delay`], and return the reason once the
/// token can no longer be refreshed (terminal failure, or expiry while
/// retrying).
async fn refresh_loop(
    http: &reqwest::Client,
    base_url: &str,
    token: &BotToken,
    reauth: &RelayReauthHandle,
) -> String {
    let mut attempt = 0usize;
    loop {
        let wait = if attempt == 0 {
            refresh_delay(token.expires_at(), unix_now())
        } else {
            retry_delay(attempt)
        };
        tokio::time::sleep(wait).await;
        match refresh_once(http, base_url, token, reauth).await {
            RefreshOutcome::Refreshed { .. } | RefreshOutcome::Stale => attempt = 0,
            RefreshOutcome::Retry(reason) => {
                attempt += 1;
                tracing::warn!(attempt, "bot token exchange failed, will retry: {reason}");
                // Past expiry nothing can be exchanged; the relay closes the
                // socket and the reconnect's token_expired ends the process.
                if unix_now() >= token.expires_at() {
                    return "bot token expired before it could be exchanged".into();
                }
            }
            RefreshOutcome::Terminal(reason) => return reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, routing::post, Json, Router};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    #[test]
    fn retries_walk_the_ladder_then_hold_steady() {
        assert_eq!(retry_delay(1), STARTUP_CONNECT_BACKOFFS[0]);
        assert_eq!(
            retry_delay(STARTUP_CONNECT_BACKOFFS.len()),
            STARTUP_CONNECT_BACKOFFS[STARTUP_CONNECT_BACKOFFS.len() - 1]
        );
        assert_eq!(
            retry_delay(STARTUP_CONNECT_BACKOFFS.len() + 1),
            STEADY_RETRY
        );
        assert_eq!(retry_delay(10_000), STEADY_RETRY);
    }

    /// A token that expires while its exchange keeps failing transiently ends
    /// the loop with a terminal reason instead of retrying forever.
    #[tokio::test]
    async fn refresh_loop_ends_when_the_token_expires_while_retrying() {
        // A closed port: every exchange fails transiently (connection refused).
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let token = BotToken::new("bzb_old".into(), unix_now() - 1);
        let (reauth, _requests) = RelayReauthHandle::test_pair();
        let reason = tokio::time::timeout(
            Duration::from_secs(10),
            refresh_loop(&reqwest::Client::new(), &base, &token, &reauth),
        )
        .await
        .expect("the loop ends instead of retrying forever");
        assert!(reason.contains("expired"), "{reason}");
    }

    #[test]
    fn exchange_timeout_fits_inside_the_replay_window() {
        // A retry after a client-side timeout must land inside the window.
        assert!(EXCHANGE_HTTP_TIMEOUT < EXCHANGE_REPLAY_WINDOW);
        // ...including the first retry backoff of the refresh loop.
        assert!(EXCHANGE_HTTP_TIMEOUT + STARTUP_CONNECT_BACKOFFS[0] < EXCHANGE_REPLAY_WINDOW);
    }

    #[test]
    fn classification_table() {
        for (status, body) in [
            (401, r#"{"code":"token_expired"}"#),
            (401, r#"{"code":"token_revoked"}"#),
            (401, r#"{"code":"principal_disabled"}"#),
            (409, r#"{"code":"token_superseded"}"#),
            (403, r#"{"code":"forbidden"}"#),
        ] {
            assert!(
                matches!(classify_http(status, body), Failure::Terminal(_)),
                "{status} {body}"
            );
        }
        for status in [408, 429, 500, 502, 503] {
            assert!(matches!(classify_http(status, ""), Failure::Transient(_)));
        }
        assert!(is_terminal_auth_message("auth-required: token_revoked"));
        assert!(is_terminal_auth_message("auth-revoked: token_expired"));
        assert!(!is_terminal_auth_message("auth-required: unavailable"));
        assert!(!is_terminal_auth_message("error: internal"));
    }

    #[test]
    fn expires_at_accepts_rfc3339_and_unix() {
        assert_eq!(
            parse_expires_at(Some(&serde_json::json!("2026-10-04T12:00:00Z"))),
            Some(1_791_115_200)
        );
        assert_eq!(parse_expires_at(Some(&serde_json::json!(42))), Some(42));
        assert_eq!(parse_expires_at(Some(&serde_json::json!(true))), None);
        assert_eq!(parse_expires_at(None), None);
    }

    #[test]
    fn refresh_waits_until_fifteen_minutes_before_expiry() {
        assert_eq!(refresh_delay(10_000, 0), Duration::from_secs(9_100));
        assert_eq!(refresh_delay(100, 0), Duration::ZERO);
    }

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}")
    }

    /// Exchange success → the new token is adopted as generation 1 and a
    /// same-connection re-AUTH is requested (and awaited).
    #[tokio::test]
    async fn exchange_success_adopts_and_reauths() {
        let seen_auth = Arc::new(std::sync::Mutex::new(String::new()));
        let seen = Arc::clone(&seen_auth);
        let base = serve(Router::new().route(
            "/auth/token/exchange",
            post(move |headers: axum::http::HeaderMap| {
                let seen = Arc::clone(&seen);
                async move {
                    *seen.lock().unwrap() = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    Json(serde_json::json!({"token": "bzb_new", "expires_at": "2030-01-01T00:00:00Z"}))
                }
            }),
        ))
        .await;
        let token = BotToken::new("bzb_old".into(), 0);
        let (reauth, mut requests) = RelayReauthHandle::test_pair();
        let answer = tokio::spawn(async move {
            let ack = requests.recv().await.expect("re-AUTH requested");
            ack.send(Ok(())).unwrap();
        });
        let outcome = refresh_once(&reqwest::Client::new(), &base, &token, &reauth).await;
        answer.await.unwrap();
        assert_eq!(outcome, RefreshOutcome::Refreshed { generation: 1 });
        assert_eq!(token.secret().as_str(), "bzb_new");
        assert_eq!(token.generation(), 1);
        assert_eq!(seen_auth.lock().unwrap().as_str(), "Bearer bzb_old");
    }

    /// A WS `OK auth false auth-required: token_revoked` on the re-AUTH is terminal.
    #[tokio::test]
    async fn reauth_rejection_with_token_code_is_terminal() {
        let base = serve(Router::new().route(
            "/auth/token/exchange",
            post(|| async { Json(serde_json::json!({"token": "bzb_new", "expires_at": 9})) }),
        ))
        .await;
        let token = BotToken::new("bzb_old".into(), 0);
        let (reauth, mut requests) = RelayReauthHandle::test_pair();
        tokio::spawn(async move {
            let ack = requests.recv().await.unwrap();
            let _ = ack.send(Err("auth-required: token_revoked".into()));
        });
        let outcome = refresh_once(&reqwest::Client::new(), &base, &token, &reauth).await;
        assert!(
            matches!(outcome, RefreshOutcome::Terminal(_)),
            "{outcome:?}"
        );
    }

    #[tokio::test]
    async fn superseded_and_revoked_exchanges_are_terminal() {
        for (status, code) in [
            (StatusCode::CONFLICT, "token_superseded"),
            (StatusCode::UNAUTHORIZED, "token_revoked"),
        ] {
            let base = serve(Router::new().route(
                "/auth/token/exchange",
                post(move || async move { (status, Json(serde_json::json!({"code": code}))) }),
            ))
            .await;
            let token = BotToken::new("bzb_old".into(), 0);
            let (reauth, _requests) = RelayReauthHandle::test_pair();
            let outcome = refresh_once(&reqwest::Client::new(), &base, &token, &reauth).await;
            assert!(
                matches!(outcome, RefreshOutcome::Terminal(_)),
                "{code}: {outcome:?}"
            );
            assert_eq!(token.generation(), 0, "nothing adopted");
        }
    }

    #[tokio::test]
    async fn network_errors_are_retryable_and_resolve_is_bounded() {
        // A closed port: connection refused.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let token = BotToken::new("bzb_old".into(), 0);
        let (reauth, _requests) = RelayReauthHandle::test_pair();
        let outcome = refresh_once(&reqwest::Client::new(), &base, &token, &reauth).await;
        assert!(matches!(outcome, RefreshOutcome::Retry(_)), "{outcome:?}");

        // resolve_principal makes exactly 1 + backoffs.len() attempts on 503.
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        let base = serve(Router::new().route(
            "/auth/me",
            axum::routing::get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::SERVICE_UNAVAILABLE }
            }),
        ))
        .await;
        let ws = base.replace("http://", "ws://");
        let result =
            resolve_principal(&ws, "bzb_x", &[Duration::ZERO, Duration::from_millis(1)]).await;
        assert!(matches!(result, Err(Failure::Transient(_))));
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn resolve_principal_stops_on_terminal_and_parses_success() {
        let principal = nostr::Keys::generate().public_key();
        let base = serve(
            Router::new().route(
                "/auth/me",
                axum::routing::get(move |headers: axum::http::HeaderMap| async move {
                    if headers.get("authorization").and_then(|v| v.to_str().ok())
                        == Some("Bearer bzb_good")
                    {
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({"principal_id": principal.to_hex(), "kind": "bot", "bot": null})),
                        )
                    } else {
                        (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"code": "token_revoked"})))
                    }
                }),
            ),
        )
        .await;
        let ws = base.replace("http://", "ws://");
        let ok = resolve_principal(&ws, "bzb_good", &[]).await.unwrap();
        assert_eq!(ok.principal, principal);
        let bad = resolve_principal(&ws, "bzb_bad", &[Duration::from_secs(60)]).await;
        assert!(
            matches!(bad, Err(Failure::Terminal(_))),
            "no retry on terminal"
        );
    }
}
