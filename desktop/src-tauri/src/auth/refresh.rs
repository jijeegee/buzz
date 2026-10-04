//! Background access-token refresh for a signed-in community (plan §4.14).
//!
//! One task per signed-in relay origin. It refreshes [`REFRESH_LEAD_SECS`]
//! (plus jitter) before the access token expires, retries network failures,
//! 429 and 5xx (including the relay's 503 "rotation cache miss") with bounded
//! exponential backoff, and ends the session on a terminal answer
//! (401 `invalid_token | token_expired | token_revoked | refresh_reused |
//! principal_disabled`, 400/403): the keyring refresh token is deleted and the
//! community moves to "sign in again" (Rule 6: the login button stays
//! reachable). Every write is fenced by the login generation it started
//! under (Rule 2), so a logout or a newer login can never be overwritten.

use std::time::Duration;

use nostr::PublicKey;
use zeroize::Zeroizing;

use super::api::{self, ApiError};
use super::{OriginAuth, RefreshStore, UserSession};

/// Refresh this long before the access token expires.
pub(crate) const REFRESH_LEAD_SECS: i64 = 600;
/// Upper bound on the random jitter added to the lead.
pub(crate) const REFRESH_JITTER_SECS: u64 = 120;
/// First retry delay after a transient failure.
pub(crate) const BACKOFF_BASE: Duration = Duration::from_secs(5);
/// Retry delay ceiling.
pub(crate) const BACKOFF_CAP: Duration = Duration::from_secs(300);

/// How a failed refresh is treated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefreshFailure {
    /// The session is over; the user must sign in again. Carries the code.
    Terminal(String),
    /// Try again later.
    Transient,
}

/// Classify a `/auth/refresh` (or `/auth/me`) failure. Pure.
pub(crate) fn classify(error: &ApiError) -> RefreshFailure {
    match error.status {
        None => RefreshFailure::Transient,
        Some(status) if status == 429 || status >= 500 => RefreshFailure::Transient,
        Some(status) if (400..500).contains(&status) => RefreshFailure::Terminal(
            error
                .code
                .clone()
                .unwrap_or_else(|| format!("http_{status}")),
        ),
        Some(_) => RefreshFailure::Transient,
    }
}

/// Delay before attempt `attempt` (0-based) after transient failures. Pure.
pub(crate) fn backoff(attempt: u32) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
    BACKOFF_BASE.saturating_mul(factor).min(BACKOFF_CAP)
}

/// Wait before refreshing a token issued at `issued_at` and expiring at
/// `expires_at`. The lead is [`REFRESH_LEAD_SECS`] but never more than a third
/// of the token's lifetime, and the jitter (`jitter_fraction` in `[0, 1)`) at
/// most [`REFRESH_JITTER_SECS`] or a tenth of the lifetime, so a short dev TTL
/// still refreshes well before expiry without a tight loop. Pure.
pub(crate) fn refresh_delay(
    issued_at: i64,
    expires_at: i64,
    now: i64,
    jitter_fraction: f64,
) -> Duration {
    let ttl = (expires_at - issued_at).max(0);
    let lead = REFRESH_LEAD_SECS.min(ttl / 3);
    let max_jitter = i64::try_from(REFRESH_JITTER_SECS)
        .unwrap_or(0)
        .min(ttl / 10);
    #[allow(clippy::cast_possible_truncation)]
    let jitter = ((max_jitter as f64) * jitter_fraction.clamp(0.0, 1.0)) as i64;
    let at = expires_at - lead - jitter;
    Duration::from_secs(u64::try_from((at - now).max(0)).unwrap_or(0))
}

fn jitter_fraction() -> f64 {
    let mut bytes = [0u8; 2];
    if getrandom::getrandom(&mut bytes).is_err() {
        return 0.0;
    }
    f64::from(u16::from_le_bytes(bytes)) / f64::from(u16::MAX)
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// One refresh round trip for `origin`'s login `generation`. The caller holds
/// the origin's refresh lock.
///
/// The rotated refresh token is persisted **immediately** after
/// `/auth/refresh` answers, before `/auth/me` (restore) runs, and only while
/// `generation` is still current: the old token is consumed server-side, so
/// losing the new one would end the session with `refresh_reused` on the next
/// attempt, and a logout / newer login (which take the same lock) must never
/// be overwritten. It is also remembered in memory so a keyring failure or a
/// failed `/auth/me` retries with the rotated token, not the consumed one.
pub(crate) async fn refresh_once(
    auth: &super::TokenAuthState,
    client: &reqwest::Client,
    origin: &str,
    generation: u64,
    store: &dyn RefreshStore,
    refresh_token: &str,
    known: Option<(PublicKey, Option<String>)>,
) -> Result<UserSession, RefreshFailure> {
    let tokens = api::refresh(client, origin, refresh_token)
        .await
        .map_err(|e| classify(&e))?;
    if !auth.is_current(origin, generation) {
        return Err(RefreshFailure::Terminal("superseded".into()));
    }
    auth.set_pending_refresh(origin, generation, &tokens.refresh);
    if let Err(error) = store.store(origin, &tokens.refresh) {
        // The in-memory copy keeps this run working. The stored token is now
        // consumed server-side: presenting it on the next launch would trip
        // reuse detection (`refresh_reused`) and revoke the whole session, so
        // drop it and let the next launch ask for a fresh sign-in instead.
        eprintln!("buzz-desktop: auth: could not persist refreshed session: {error}");
        if let Err(error) = store.delete(origin) {
            eprintln!("buzz-desktop: auth: could not drop the consumed refresh token: {error}");
        }
    }
    let (principal, device_id) = match known {
        Some(known) => known,
        None => {
            let me = api::me(client, origin, &tokens.access)
                .await
                .map_err(|e| classify(&e))?;
            let principal = PublicKey::from_hex(&me.principal_id)
                .map_err(|_| RefreshFailure::Terminal("invalid_principal".into()))?;
            (principal, me.device_id)
        }
    };
    Ok(UserSession {
        principal,
        device_id,
        access: Zeroizing::new(tokens.access.0.to_string()),
        refresh: Zeroizing::new(tokens.refresh.0.to_string()),
        access_issued_at: unix_now(),
        access_expires_at: unix_now() + tokens.expires_in.max(0),
    })
}

/// Refresh now, under the per-origin refresh lock, fenced by `generation`.
///
/// `observed_expiry` is the access expiry the caller saw before deciding to
/// refresh; when the session already carries a different one, another caller
/// refreshed while this one waited for the lock and that session is returned
/// without a second round trip.
pub(crate) async fn refresh_now(
    auth: &super::TokenAuthState,
    client: &reqwest::Client,
    origin: &str,
    generation: u64,
    store: &dyn RefreshStore,
    observed_expiry: Option<i64>,
) -> Result<UserSession, RefreshFailure> {
    let _guard = auth.refresh_lock(origin).lock_owned().await;
    refresh_locked(auth, client, origin, generation, store, observed_expiry).await
}

/// [`refresh_now`] for a caller that already holds the refresh lock.
async fn refresh_locked(
    auth: &super::TokenAuthState,
    client: &reqwest::Client,
    origin: &str,
    generation: u64,
    store: &dyn RefreshStore,
    observed_expiry: Option<i64>,
) -> Result<UserSession, RefreshFailure> {
    let current = match auth.get(origin) {
        Some((g, state)) if g == generation => state,
        _ => return Err(RefreshFailure::Terminal("superseded".into())),
    };
    let (refresh_token, known) = match current {
        OriginAuth::Active(session) => {
            if observed_expiry.is_some_and(|seen| seen != session.access_expires_at) {
                return Ok(session);
            }
            let known = Some((session.principal, session.device_id.clone()));
            (session.refresh.clone(), known)
        }
        OriginAuth::Restoring => match auth.pending_refresh(origin, generation) {
            Some(token) => (token, None),
            None => match store.load(origin) {
                Ok(Some(token)) => (Zeroizing::new(token), None),
                Ok(None) => {
                    auth.replace_if(origin, generation, OriginAuth::SignedOut);
                    return Err(RefreshFailure::Terminal("no_session".into()));
                }
                Err(_) => return Err(RefreshFailure::Transient),
            },
        },
        OriginAuth::NeedsLogin(code) => return Err(RefreshFailure::Terminal(code)),
        OriginAuth::SignedOut => return Err(RefreshFailure::Terminal("signed_out".into())),
    };
    match refresh_once(
        auth,
        client,
        origin,
        generation,
        store,
        &refresh_token,
        known,
    )
    .await
    {
        Ok(session) => {
            if auth.replace_if(origin, generation, OriginAuth::Active(session.clone())) {
                auth.clear_pending_refresh(origin);
            }
            Ok(session)
        }
        Err(RefreshFailure::Terminal(code)) => {
            if auth.replace_if(origin, generation, OriginAuth::NeedsLogin(code.clone())) {
                auth.clear_pending_refresh(origin);
                let _ = store.delete(origin);
            }
            Err(RefreshFailure::Terminal(code))
        }
        Err(RefreshFailure::Transient) => Err(RefreshFailure::Transient),
    }
}

/// Restore `origin`'s stored session (single-flight).
///
/// Runs entirely under the origin's refresh lock, so concurrent callers queue
/// behind the one doing the round trip and then see its result instead of
/// starting their own or reading a half-restored origin. Returns
/// `Some((generation, attempt))` when this call began a restore (the caller
/// starts the refresh loop and notifies), `None` when there was nothing to
/// restore or another caller already did.
pub(crate) async fn restore_session(
    auth: &super::TokenAuthState,
    client: &reqwest::Client,
    origin: &str,
    store: &dyn RefreshStore,
) -> Option<(u64, Result<(), RefreshFailure>)> {
    let _guard = auth.refresh_lock(origin).lock_owned().await;
    if !matches!(auth.get(origin), None | Some((_, OriginAuth::SignedOut))) {
        return None;
    }
    if !matches!(store.load(origin), Ok(Some(_))) {
        return None;
    }
    let generation = auth.set(origin, OriginAuth::Restoring);
    let attempt = refresh_locked(auth, client, origin, generation, store, None)
        .await
        .map(|_| ());
    Some((generation, attempt))
}

/// The background loop for one login generation. Returns when the session
/// ends, is superseded, or turns terminal. `notify` is called after every
/// state change.
pub(crate) async fn run_refresh_loop(
    auth: std::sync::Arc<super::TokenAuthState>,
    client: reqwest::Client,
    origin: String,
    generation: u64,
    store: std::sync::Arc<dyn RefreshStore>,
    notify: impl Fn(&str) + Send + 'static,
) {
    let mut attempt: u32 = 0;
    loop {
        let (wait, observed) = match auth.get(&origin) {
            Some((g, OriginAuth::Active(session))) if g == generation => {
                let wait = if attempt == 0 {
                    refresh_delay(
                        session.access_issued_at,
                        session.access_expires_at,
                        unix_now(),
                        jitter_fraction(),
                    )
                } else {
                    backoff(attempt - 1)
                };
                (wait, Some(session.access_expires_at))
            }
            Some((g, OriginAuth::Restoring)) if g == generation => {
                let wait = if attempt == 0 {
                    Duration::ZERO
                } else {
                    backoff(attempt - 1)
                };
                (wait, None)
            }
            _ => return,
        };
        tokio::time::sleep(wait).await;
        match refresh_now(
            &auth,
            &client,
            &origin,
            generation,
            store.as_ref(),
            observed,
        )
        .await
        {
            Ok(_) => {
                attempt = 0;
                notify(&origin);
            }
            Err(RefreshFailure::Transient) => attempt = attempt.saturating_add(1),
            Err(RefreshFailure::Terminal(_)) => {
                notify(&origin);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Default)]
    pub(crate) struct MemStore(Mutex<std::collections::HashMap<String, String>>);
    impl RefreshStore for MemStore {
        fn load(&self, origin: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(origin).cloned())
        }
        fn store(&self, origin: &str, token: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(origin.into(), token.into());
            Ok(())
        }
        fn delete(&self, origin: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(origin);
            Ok(())
        }
    }

    fn err(status: Option<u16>, code: Option<&str>) -> ApiError {
        ApiError {
            status,
            code: code.map(str::to_owned),
            message: String::new(),
        }
    }

    #[test]
    fn classification_table() {
        assert_eq!(classify(&err(None, None)), RefreshFailure::Transient);
        assert_eq!(
            classify(&err(Some(503), Some("unavailable"))),
            RefreshFailure::Transient
        );
        assert_eq!(classify(&err(Some(429), None)), RefreshFailure::Transient);
        for code in [
            "invalid_token",
            "token_expired",
            "token_revoked",
            "refresh_reused",
            "principal_disabled",
        ] {
            assert_eq!(
                classify(&err(Some(401), Some(code))),
                RefreshFailure::Terminal(code.into())
            );
        }
        assert_eq!(
            classify(&err(Some(403), None)),
            RefreshFailure::Terminal("http_403".into())
        );
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(0), Duration::from_secs(5));
        assert_eq!(backoff(1), Duration::from_secs(10));
        assert_eq!(backoff(3), Duration::from_secs(40));
        assert_eq!(backoff(10), BACKOFF_CAP);
        assert_eq!(backoff(u32::MAX), BACKOFF_CAP);
    }

    #[test]
    fn refresh_delay_leads_expiry() {
        // 1 h token: refresh 10 min (+ up to 2 min jitter) early.
        assert_eq!(refresh_delay(0, 3600, 0, 0.0), Duration::from_secs(3000));
        assert_eq!(refresh_delay(0, 3600, 0, 1.0), Duration::from_secs(2880));
        // 3 min dev token: lead is a third of the lifetime, never zero-wait.
        assert_eq!(refresh_delay(0, 180, 0, 0.0), Duration::from_secs(120));
        assert!(refresh_delay(0, 180, 0, 1.0) >= Duration::from_secs(100));
        // Already past the refresh point.
        assert_eq!(refresh_delay(0, 3600, 4000, 0.0), Duration::ZERO);
    }

    /// A stub relay answering from a script, each answer after `delay`, and
    /// recording every full request (head + body).
    async fn stub_full(
        responses: Vec<(u16, &'static str, Duration)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        tokio::spawn(async move {
            for (status, body, delay) in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some(split) = text.find("\r\n\r\n") {
                        let length = text[..split]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                if name.eq_ignore_ascii_case("content-length") {
                                    value.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                        if buf.len() >= split + 4 + length {
                            break;
                        }
                    }
                }
                seen2
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf).into_owned());
                tokio::time::sleep(delay).await;
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (origin, seen)
    }

    /// A refresh store whose writes always fail (keyring unavailable).
    #[derive(Default)]
    struct ReadOnlyStore(MemStore);
    impl RefreshStore for ReadOnlyStore {
        fn load(&self, origin: &str) -> Result<Option<String>, String> {
            self.0.load(origin)
        }
        fn store(&self, _origin: &str, _token: &str) -> Result<(), String> {
            Err("keyring unavailable".into())
        }
        fn delete(&self, origin: &str) -> Result<(), String> {
            self.0.delete(origin)
        }
    }

    const ROTATED: &str = r#"{"access":"bzs_new","refresh":"bzr_new","expires_in":3600}"#;

    fn me_body() -> &'static str {
        Box::leak(format!(r#"{{"principal_id":"{PRINCIPAL}","device_id":"d1"}}"#).into_boxed_str())
    }

    /// B2: the rotated refresh token is durable before `/auth/me` runs, so a
    /// failed (or abandoned) `/auth/me` never leaves the keyring holding the
    /// consumed token.
    #[tokio::test]
    async fn rotated_token_is_persisted_before_me() {
        let (origin, _) = stub_full(vec![
            (200, ROTATED, Duration::ZERO),
            (503, r#"{"code":"unavailable"}"#, Duration::ZERO),
        ])
        .await;
        let auth = super::super::TokenAuthState::default();
        let store = MemStore::default();
        store.store(&origin, "bzr_old").unwrap();
        let generation = auth.set(&origin, OriginAuth::Restoring);
        let result = refresh_now(
            &auth,
            &reqwest::Client::new(),
            &origin,
            generation,
            &store,
            None,
        )
        .await;
        assert_eq!(result.map(|_| ()), Err(RefreshFailure::Transient));
        assert_eq!(store.load(&origin).unwrap().as_deref(), Some("bzr_new"));
    }

    /// B2: with the keyring unavailable, the retry after a failed `/auth/me`
    /// presents the rotated token from memory, not the consumed one.
    #[tokio::test]
    async fn retry_after_failed_me_uses_the_rotated_token() {
        let (origin, seen) = stub_full(vec![
            (200, ROTATED, Duration::ZERO),
            (503, r#"{"code":"unavailable"}"#, Duration::ZERO),
            (
                200,
                r#"{"access":"bzs_2","refresh":"bzr_2","expires_in":3600}"#,
                Duration::ZERO,
            ),
            (200, me_body(), Duration::ZERO),
        ])
        .await;
        let auth = super::super::TokenAuthState::default();
        let store = ReadOnlyStore::default();
        store.0.store(&origin, "bzr_old").unwrap();
        let generation = auth.set(&origin, OriginAuth::Restoring);
        let client = reqwest::Client::new();
        let first = refresh_now(&auth, &client, &origin, generation, &store, None).await;
        assert_eq!(first.map(|_| ()), Err(RefreshFailure::Transient));
        let second = refresh_now(&auth, &client, &origin, generation, &store, None).await;
        assert!(second.is_ok());
        let seen = seen.lock().unwrap().clone();
        assert!(
            seen[2].contains("bzr_new"),
            "retry must use the rotated token"
        );
        assert!(!seen[2].contains("bzr_old"));
        assert!(auth.pending_refresh(&origin, generation).is_none());
    }

    /// A keyring write failure after rotation drops the stored (consumed)
    /// token, so the next launch asks for sign-in instead of presenting it
    /// and tripping `refresh_reused`; this run keeps working from memory.
    #[tokio::test]
    async fn failed_rotation_write_drops_the_consumed_token() {
        let (origin, _) = stub_full(vec![
            (200, ROTATED, Duration::ZERO),
            (200, me_body(), Duration::ZERO),
        ])
        .await;
        let auth = super::super::TokenAuthState::default();
        let store = ReadOnlyStore::default();
        store.0.store(&origin, "bzr_old").unwrap();
        let generation = auth.set(&origin, OriginAuth::Restoring);
        let session = refresh_now(
            &auth,
            &reqwest::Client::new(),
            &origin,
            generation,
            &store,
            None,
        )
        .await
        .expect("this run keeps the rotated session");
        assert_eq!(session.refresh.as_str(), "bzr_new");
        assert_eq!(store.load(&origin).unwrap(), None);
    }

    /// B2: a sign-out while a refresh is in flight wins; the late rotation
    /// never writes its token back.
    #[tokio::test]
    async fn sign_out_during_refresh_is_not_undone() {
        let (origin, _) = stub_full(vec![
            (200, ROTATED, Duration::from_millis(300)),
            (200, me_body(), Duration::ZERO),
        ])
        .await;
        let auth = Arc::new(super::super::TokenAuthState::default());
        let store = Arc::new(MemStore::default());
        store.store(&origin, "bzr_old").unwrap();
        let generation = auth.set(&origin, OriginAuth::Restoring);
        let refresh = {
            let (auth, store, origin) = (Arc::clone(&auth), Arc::clone(&store), origin.clone());
            tokio::spawn(async move {
                refresh_now(
                    &auth,
                    &reqwest::Client::new(),
                    &origin,
                    generation,
                    store.as_ref(),
                    None,
                )
                .await
                .map(|_| ())
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        // A sign-out that skips the lock: the generation fence alone must hold.
        let _ = store.delete(&origin);
        auth.set(&origin, OriginAuth::SignedOut);
        let _ = refresh.await.unwrap();
        assert_eq!(
            store.load(&origin).unwrap(),
            None,
            "no write-back after sign-out"
        );
        assert!(matches!(
            auth.get(&origin),
            Some((_, OriginAuth::SignedOut))
        ));
    }

    /// B2: the production sign-out waits for an in-flight rotation (lock) and
    /// then deletes, so nothing survives it.
    #[tokio::test]
    async fn forget_session_waits_for_the_rotation_then_deletes() {
        let (origin, _) = stub_full(vec![(200, ROTATED, Duration::from_millis(300))]).await;
        let auth = Arc::new(super::super::TokenAuthState::default());
        let store = Arc::new(MemStore::default());
        store.store(&origin, "bzr_old").unwrap();
        let generation = auth.set(&origin, OriginAuth::Restoring);
        // An active-session refresh (no /auth/me) of the current generation.
        let session = UserSession {
            principal: PublicKey::from_hex(PRINCIPAL).unwrap(),
            device_id: None,
            access: Zeroizing::new("bzs_old".into()),
            refresh: Zeroizing::new("bzr_old".into()),
            access_issued_at: 0,
            access_expires_at: 1,
        };
        auth.replace_if(&origin, generation, OriginAuth::Active(session));
        let refresh = {
            let (auth, store, origin) = (Arc::clone(&auth), Arc::clone(&store), origin.clone());
            tokio::spawn(async move {
                refresh_now(
                    &auth,
                    &reqwest::Client::new(),
                    &origin,
                    generation,
                    store.as_ref(),
                    None,
                )
                .await
                .map(|_| ())
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        super::super::commands::forget_session(&auth, &origin, store.as_ref())
            .await
            .unwrap();
        // The rotation finished before the sign-out (it held the lock)...
        assert_eq!(refresh.await.unwrap(), Ok(()));
        // ...and the sign-out then removed everything.
        assert_eq!(store.load(&origin).unwrap(), None);
        assert!(matches!(
            auth.get(&origin),
            Some((_, OriginAuth::SignedOut))
        ));
    }

    /// B3: restore is single-flight; a second caller waits for the first
    /// one's round trip and then reads the settled state, and only one
    /// restore reaches the relay.
    #[tokio::test]
    async fn concurrent_restores_share_one_round_trip() {
        let (origin, seen) = stub_full(vec![
            (200, ROTATED, Duration::from_millis(300)),
            (200, me_body(), Duration::ZERO),
        ])
        .await;
        let auth = Arc::new(super::super::TokenAuthState::default());
        let store = Arc::new(MemStore::default());
        store.store(&origin, "bzr_old").unwrap();
        let first = {
            let (auth, store, origin) = (Arc::clone(&auth), Arc::clone(&store), origin.clone());
            tokio::spawn(async move {
                restore_session(&auth, &reqwest::Client::new(), &origin, store.as_ref()).await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        let second = restore_session(&auth, &reqwest::Client::new(), &origin, store.as_ref()).await;
        assert!(
            second.is_none(),
            "the second caller joins instead of restoring again"
        );
        assert!(
            matches!(auth.get(&origin), Some((_, OriginAuth::Active(_)))),
            "the joining caller re-reads a settled (active) state"
        );
        let first = first.await.unwrap();
        assert!(matches!(first, Some((_, Ok(())))));
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// B2: a caller giving up on a restore never cuts a rotation short; the
    /// wait also outlasts both restore requests.
    #[test]
    fn restore_wait_outlasts_the_restore_requests() {
        assert!(
            super::super::restore::RESTORE_TIMEOUT > super::super::api::AUTH_REQUEST_TIMEOUT * 2
        );
    }

    /// A stub relay answering `/auth/refresh` and `/auth/me` from a script.
    async fn stub(responses: Vec<(u16, &'static str)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        tokio::spawn(async move {
            for (status, body) in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                seen2
                    .lock()
                    .unwrap()
                    .push(head.lines().next().unwrap_or_default().to_owned());
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (origin, seen)
    }

    const PRINCIPAL: &str = "ea9b4d7a7a78a3e3729e5568b14d764d4962be0e1f20f749bcf8d9dbbf9a9328";

    #[tokio::test]
    async fn restore_refreshes_resolves_principal_and_rotates_keyring() {
        let me = Box::leak(
            format!(r#"{{"principal_id":"{PRINCIPAL}","device_id":"d1"}}"#).into_boxed_str(),
        );
        let (origin, seen) = stub(vec![
            (
                200,
                r#"{"access":"bzs_new","refresh":"bzr_new","expires_in":3600}"#,
            ),
            (200, me),
        ])
        .await;
        let auth = super::super::TokenAuthState::default();
        let store = MemStore::default();
        store.store(&origin, "bzr_old").unwrap();
        let generation = auth.set(&origin, OriginAuth::Restoring);
        let session = refresh_now(
            &auth,
            &reqwest::Client::new(),
            &origin,
            generation,
            &store,
            None,
        )
        .await
        .unwrap();
        assert_eq!(session.principal.to_hex(), PRINCIPAL);
        assert_eq!(session.access.as_str(), "bzs_new");
        assert_eq!(store.load(&origin).unwrap().as_deref(), Some("bzr_new"));
        assert!(matches!(auth.get(&origin), Some((g, OriginAuth::Active(_))) if g == generation));
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0], "POST /auth/refresh HTTP/1.1");
        assert_eq!(seen[1], "GET /auth/me HTTP/1.1");
    }

    #[tokio::test]
    async fn terminal_refresh_clears_keyring_and_needs_login() {
        let (origin, _) = stub(vec![(
            401,
            r#"{"error":"authentication failed","code":"refresh_reused"}"#,
        )])
        .await;
        let auth = super::super::TokenAuthState::default();
        let store = MemStore::default();
        store.store(&origin, "bzr_old").unwrap();
        let generation = auth.set(&origin, OriginAuth::Restoring);
        let result = refresh_now(
            &auth,
            &reqwest::Client::new(),
            &origin,
            generation,
            &store,
            None,
        )
        .await;
        assert_eq!(
            result.map(|_| ()),
            Err(RefreshFailure::Terminal("refresh_reused".into()))
        );
        assert_eq!(store.load(&origin).unwrap(), None);
        assert!(matches!(
            auth.get(&origin),
            Some((_, OriginAuth::NeedsLogin(code))) if code == "refresh_reused"
        ));
    }

    #[tokio::test]
    async fn transient_refresh_keeps_session_and_keyring() {
        let (origin, _) = stub(vec![(
            503,
            r#"{"error":"authentication backend unavailable","code":"unavailable"}"#,
        )])
        .await;
        let auth = super::super::TokenAuthState::default();
        let store = MemStore::default();
        store.store(&origin, "bzr_old").unwrap();
        let generation = auth.set(&origin, OriginAuth::Restoring);
        let result = refresh_now(
            &auth,
            &reqwest::Client::new(),
            &origin,
            generation,
            &store,
            None,
        )
        .await;
        assert_eq!(result.map(|_| ()), Err(RefreshFailure::Transient));
        assert_eq!(store.load(&origin).unwrap().as_deref(), Some("bzr_old"));
        assert!(matches!(
            auth.get(&origin),
            Some((_, OriginAuth::Restoring))
        ));
    }

    #[tokio::test]
    async fn superseded_generation_never_writes() {
        let (origin, _) = stub(vec![]).await;
        let auth = super::super::TokenAuthState::default();
        let store = MemStore::default();
        let old = auth.set(&origin, OriginAuth::Restoring);
        let _new = auth.set(&origin, OriginAuth::SignedOut);
        let result = refresh_now(&auth, &reqwest::Client::new(), &origin, old, &store, None).await;
        assert!(matches!(result, Err(RefreshFailure::Terminal(_))));
        assert!(matches!(
            auth.get(&origin),
            Some((_, OriginAuth::SignedOut))
        ));
    }
}
