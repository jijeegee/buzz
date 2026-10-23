//! Device proofs on account calls (device-bound keys plan §2.1, PR A1b).
//!
//! A login presents the install's P-256 device key with a proof over the
//! login code, and the device row records it. Afterwards every `/auth/*`
//! call made with a user token of that device carries [`PROOF_HEADER`]: a
//! fresh signature by the key over the request method, URL, body and token.
//! A token copied to another machine is then useless.
//!
//! `KUZZ_DEVICE_ADMISSION` (per surface, switchable at runtime in Redis)
//! decides what a missing or failing proof does: nothing (`off`), a deduped
//! "would refuse" log line and metric (`log`), or a refusal (`enforce`).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse as _, Response};
use kuzz_auth::device_proof::{
    DeviceProof, DeviceProofError, DevicePubkey, ProofScope, PROOF_WINDOW_SECS,
};
use kuzz_auth::TokenBinding;
use kuzz_core::principal::AccessTokenKind;
use sha2::{Digest as _, Sha256};

use super::config::DeviceAdmissionMode;
use super::{kv, TokenRejection};
use crate::state::AppState;

/// Header carrying a proof (`<ts>.<nonce>.<sig>`).
pub(crate) const PROOF_HEADER: &str = "x-kuzz-device-proof";
/// Response header with the server clock (unix seconds), so a client with a
/// skewed clock can correct its proofs.
pub(crate) const SERVER_TIME_HEADER: &str = "x-kuzz-server-time";
/// Purpose of a proof on an authenticated `/auth/*` call.
const PURPOSE_AUTH: &str = "auth";
/// Purpose of a proof on `/auth/refresh`.
const PURPOSE_REFRESH: &str = "refresh";
/// Purpose of a login proof (binds the device key to the new session).
const PURPOSE_BIND: &str = "bind";
/// A nonce is remembered for longer than any timestamp it could carry.
const NONCE_TTL_SECS: u64 = (2 * PROOF_WINDOW_SECS + 10) as u64;
/// Redis hash of per-surface mode overrides (`HSET kuzz:device_admission auth log`).
const MODES_KEY: &str = "kuzz:device_admission";
/// How often [`spawn_mode_poller`] re-reads [`MODES_KEY`].
const MODES_POLL: Duration = Duration::from_secs(10);

/// Where a device check runs. Each surface is switched on separately, so
/// enforcement can roll out one entrance at a time (plan S14): `auth` first,
/// then `ws`, `http`, `git`, `media`, `audio` and `admin`, and `token` last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Surface {
    /// Login, refresh and every other `/auth/*` call.
    Auth,
    /// NIP-42 AUTH on the WebSocket (key admission).
    Ws,
    /// NIP-98 HTTP: the bridge (`/events`, `/query`, `/count`, moderation
    /// reads), gifs, groups, workflows, invites and git settings.
    Http,
    /// NIP-98 git smart HTTP (clone, fetch, push).
    Git,
    /// Blossom kind-24242 media upload and read.
    Media,
    /// NIP-42 AUTH on the huddle audio socket.
    Audio,
    /// The admin API and the deployment operator API (NIP-98).
    Admin,
    /// Access tokens on message paths (token WS and audio AUTH, `Bearer` on
    /// the bridge, media and invites, git tokens, bot token issuance). Not a
    /// device check: `log` reports each use, `enforce` closes the path.
    Token,
}

impl Surface {
    const ALL: [Self; 8] = [
        Self::Auth,
        Self::Ws,
        Self::Http,
        Self::Git,
        Self::Media,
        Self::Audio,
        Self::Admin,
        Self::Token,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Ws => "ws",
            Self::Http => "http",
            Self::Git => "git",
            Self::Media => "media",
            Self::Audio => "audio",
            Self::Admin => "admin",
            Self::Token => "token",
        }
    }
}

/// Why a use was not proven by its device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The token's device has no key (an older app).
    Keyless,
    /// No proof was sent.
    Missing,
    /// The key or proof does not parse.
    Malformed,
    /// The signature does not verify for any accepted binding.
    BadSignature,
    /// The proof timestamp is outside the window.
    Stale,
    /// The proof's nonce was already used.
    Replayed,
    /// The login presents another key than its start bound (or none).
    Mismatch,
    /// The nonce store could not be reached; fail closed.
    Unavailable,
}

impl Refusal {
    fn as_str(self) -> &'static str {
        match self {
            Self::Keyless => "keyless",
            Self::Missing => "missing",
            Self::Malformed => "malformed",
            Self::BadSignature => "bad_sig",
            Self::Stale => "stale",
            Self::Replayed => "replayed",
            Self::Mismatch => "mismatch",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Per-process device-check state on [`super::IdentityRuntime`].
pub(crate) struct DeviceRuntime {
    /// Runtime overrides of the configured mode, by surface.
    overrides: RwLock<HashMap<Surface, DeviceAdmissionMode>>,
    /// "would refuse" lines already logged (one per subject and reason).
    logged: moka::sync::Cache<String, ()>,
}

impl Default for DeviceRuntime {
    fn default() -> Self {
        Self {
            overrides: RwLock::new(HashMap::new()),
            logged: moka::sync::Cache::builder()
                .max_capacity(100_000)
                .time_to_live(Duration::from_secs(3600))
                .build(),
        }
    }
}

impl DeviceRuntime {
    /// Whether this "would refuse" line is new (logged at most once an hour).
    fn first_report(&self, key: String) -> bool {
        if self.logged.contains_key(&key) {
            return false;
        }
        self.logged.insert(key, ());
        true
    }

    pub(crate) fn set_overrides(&self, overrides: HashMap<Surface, DeviceAdmissionMode>) {
        let mut current = self
            .overrides
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current != overrides {
            tracing::info!(?overrides, "device admission overrides changed");
            *current = overrides;
        }
    }
}

/// The mode `surface` runs in: a Redis override, else `KUZZ_DEVICE_ADMISSION`.
pub(crate) fn mode(state: &AppState, surface: Surface) -> DeviceAdmissionMode {
    state
        .identity
        .devices
        .overrides
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&surface)
        .copied()
        .unwrap_or(state.identity.config().device_admission)
}

/// Override `surface`'s mode in this process, as the Redis poller would.
#[cfg(test)]
pub(crate) fn override_mode(state: &AppState, surface: Surface, mode: DeviceAdmissionMode) {
    let mut overrides = state
        .identity
        .devices
        .overrides
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    overrides.insert(surface, mode);
    state.identity.devices.set_overrides(overrides);
}

/// Parse the [`MODES_KEY`] hash; unknown surfaces and bad values are skipped.
fn parse_overrides(fields: &HashMap<String, String>) -> HashMap<Surface, DeviceAdmissionMode> {
    Surface::ALL
        .into_iter()
        .filter_map(|surface| {
            let raw = fields.get(surface.as_str())?;
            let mode = DeviceAdmissionMode::parse(raw);
            if mode.is_none() {
                tracing::warn!(surface = surface.as_str(), %raw, "bad device admission override");
            }
            mode.map(|mode| (surface, mode))
        })
        .collect()
}

/// Re-read the per-surface overrides every [`MODES_POLL`] for the life of the
/// process. A Redis error keeps the last values.
pub fn spawn_mode_poller(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut failing = false;
        loop {
            match read_overrides(&state).await {
                Ok(fields) => {
                    if failing {
                        tracing::info!("device admission overrides readable again");
                        failing = false;
                    }
                    state
                        .identity
                        .devices
                        .set_overrides(parse_overrides(&fields));
                }
                Err(error) => {
                    if !failing {
                        tracing::warn!(
                            %error,
                            "device admission overrides unavailable; keeping last"
                        );
                        failing = true;
                    }
                }
            }
            tokio::time::sleep(MODES_POLL).await;
        }
    });
}

async fn read_overrides(state: &AppState) -> Result<HashMap<String, String>, String> {
    let mut conn = state
        .redis_pool
        .get()
        .await
        .map_err(|error| error.to_string())?;
    redis::cmd("HGETALL")
        .arg(MODES_KEY)
        .query_async(&mut conn)
        .await
        .map_err(|error| error.to_string())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Apply the surface's mode to a check outcome: `Err` only when enforced.
fn judge(
    state: &AppState,
    surface: Surface,
    subject: &str,
    outcome: Result<(), Refusal>,
) -> Result<(), Refusal> {
    let Err(refusal) = outcome else {
        return Ok(());
    };
    let enforced = mode(state, surface) == DeviceAdmissionMode::Enforce;
    report(state, surface, subject, refusal.as_str(), enforced);
    if enforced {
        Err(refusal)
    } else {
        Ok(())
    }
}

/// Count a refusal (or, unless `enforced`, a "would refuse") and log it once
/// an hour per subject and reason.
pub(crate) fn report(
    state: &AppState,
    surface: Surface,
    subject: &str,
    reason: &'static str,
    enforced: bool,
) {
    metrics::counter!(
        "kuzz_device_proof_refusals_total",
        "surface" => surface.as_str(),
        "reason" => reason,
        "enforced" => if enforced { "true" } else { "false" },
    )
    .increment(1);
    if state
        .identity
        .devices
        .first_report(format!("{}:{reason}:{subject}", surface.as_str()))
    {
        tracing::info!(
            surface = surface.as_str(),
            reason,
            subject,
            enforced,
            "device.would_refuse"
        );
    }
}

/// Verify `raw` by `key` for one of `bindings`, then burn its nonce.
async fn verify_proof(
    state: &AppState,
    key: &DevicePubkey,
    raw: &str,
    purpose: &str,
    bindings: &[String],
    token_hash: Option<&[u8; 32]>,
    body_hash: Option<&[u8; 32]>,
) -> Result<(), Refusal> {
    let proof = DeviceProof::parse(raw).map_err(|_| Refusal::Malformed)?;
    let now = now();
    let mut refusal = Refusal::BadSignature;
    let verified = bindings.iter().any(|binding| {
        let scope = ProofScope {
            purpose,
            binding,
            token_hash,
            body_hash,
        };
        match proof.verify(key, &scope, now) {
            Ok(()) => true,
            Err(DeviceProofError::Stale) => {
                refusal = Refusal::Stale;
                false
            }
            Err(_) => false,
        }
    });
    if !verified {
        return Err(refusal);
    }
    // Keyed on (device key, nonce), never on the signature: ECDSA signatures
    // are malleable.
    match kv::put_new(
        &state.redis_pool,
        "device:nonce",
        &format!("{}:{}", key.to_hex(), proof.nonce),
        "1",
        NONCE_TTL_SECS,
    )
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(Refusal::Replayed),
        Err(error) => {
            tracing::warn!(%error, "device proof nonce store unavailable");
            Err(Refusal::Unavailable)
        }
    }
}

/// The `/auth/*` request being handled, set by [`proof_context_layer`].
#[derive(Clone)]
struct RequestContext {
    /// `<METHOD> <path and query>`.
    target: String,
    /// SHA-256 of a non-empty body.
    body_hash: Option<[u8; 32]>,
}

tokio::task_local! {
    static REQUEST: RequestContext;
}

/// Route trees whose device proof is checked in every mode
/// ([`check_request_proof_always`]): each root and everything below it.
const ALWAYS_PROVEN_ROOTS: &[&str] = &["/auth/agents", "/auth/pairing"];

fn always_proven(path: &str) -> bool {
    ALWAYS_PROVEN_ROOTS.iter().any(|root| {
        path.strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// `/auth/*` middleware: while the auth surface is on (always for
/// [`ALWAYS_PROVEN_ROOTS`] routes), buffer the (already size-limited) body,
/// record what proofs bind to, and tell the client the server time.
pub(crate) async fn proof_context_layer(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if mode(&state, Surface::Auth) == DeviceAdmissionMode::Off
        && !always_proven(request.uri().path())
    {
        return next.run(request).await;
    }
    let target = format!(
        "{} {}",
        request.method(),
        request
            .uri()
            .path_and_query()
            .map_or_else(|| request.uri().path(), |pq| pq.as_str())
    );
    let (parts, body) = request.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let body_hash = (!bytes.is_empty()).then(|| Sha256::digest(&bytes).into());
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    let mut response = REQUEST
        .scope(RequestContext { target, body_hash }, next.run(request))
        .await;
    response
        .headers_mut()
        .insert(SERVER_TIME_HEADER, HeaderValue::from(now()));
    response
}

/// What a request proof may be bound to: method and full URL under the
/// public URL or an extra accepted origin.
fn request_bindings(state: &AppState, target: &str) -> Vec<String> {
    let config = state.identity.config();
    let Some((method, path)) = target.split_once(' ') else {
        return Vec::new();
    };
    std::iter::once(&config.public_url)
        .chain(&config.device_proof_origins)
        .map(|origin| format!("{method} {}{path}", origin.trim_end_matches('/')))
        .collect()
}

/// Check a request's [`PROOF_HEADER`] for a token whose device key is `key`.
async fn check_header(
    state: &AppState,
    key: Option<&DevicePubkey>,
    headers: &HeaderMap,
    purpose: &str,
    token_hash: &[u8; 32],
) -> Result<(), TokenRejection> {
    let Ok(request) = REQUEST.try_with(Clone::clone) else {
        // Outside the /auth router (other HTTP APIs): checked by a later PR.
        return Ok(());
    };
    let outcome = match (key, headers.get(PROOF_HEADER).and_then(|v| v.to_str().ok())) {
        (None, _) => Err(Refusal::Keyless),
        (Some(_), None) => Err(Refusal::Missing),
        (Some(key), Some(raw)) => {
            verify_proof(
                state,
                key,
                raw,
                purpose,
                &request_bindings(state, &request.target),
                Some(token_hash),
                request.body_hash.as_ref(),
            )
            .await
        }
    };
    judge(
        state,
        Surface::Auth,
        &hex::encode(&token_hash[..8]),
        outcome,
    )
    .map_err(|refusal| match refusal {
        Refusal::Unavailable => TokenRejection::Unavailable,
        Refusal::Stale => TokenRejection::DeviceProofStale,
        _ => TokenRejection::DeviceProofRequired,
    })
}

/// The device proof of an `/auth/*` call authenticated by a user token.
/// Bot tokens have no device of their own; they close with token messaging.
pub(crate) async fn check_request_proof(
    state: &AppState,
    binding: &TokenBinding,
    headers: &HeaderMap,
) -> Result<(), TokenRejection> {
    if binding.kind != AccessTokenKind::User
        || mode(state, Surface::Auth) == DeviceAdmissionMode::Off
    {
        return Ok(());
    }
    // A stored key that no longer parses counts as keyless (fails closed).
    let key = binding
        .device_pubkey
        .and_then(|key| DevicePubkey::from_sec1(&key).ok());
    check_header(
        state,
        key.as_ref(),
        headers,
        PURPOSE_AUTH,
        &binding.token_hash,
    )
    .await
}

/// The device proof of a user-token call that hands out or binds keys
/// (`/auth/agents/*`, `/auth/pairing*`), checked in every auth surface
/// mode: purpose `auth`, bound to the request target, the token hash and the
/// body hash, nonce burnt. `key` is the token's device key; keyless tokens
/// are refused by the caller first. Fails closed without a request context.
pub(crate) async fn check_request_proof_always(
    state: &AppState,
    key: &DevicePubkey,
    headers: &HeaderMap,
    token_hash: &[u8; 32],
) -> Result<(), TokenRejection> {
    let outcome = match (
        REQUEST.try_with(Clone::clone),
        headers.get(PROOF_HEADER).and_then(|v| v.to_str().ok()),
    ) {
        (Err(_), _) | (Ok(_), None) => Err(Refusal::Missing),
        (Ok(request), Some(raw)) => {
            verify_proof(
                state,
                key,
                raw,
                PURPOSE_AUTH,
                &request_bindings(state, &request.target),
                Some(token_hash),
                request.body_hash.as_ref(),
            )
            .await
        }
    };
    outcome.map_err(|refusal| {
        report(
            state,
            Surface::Auth,
            &hex::encode(&token_hash[..8]),
            refusal.as_str(),
            true,
        );
        match refusal {
            Refusal::Unavailable => TokenRejection::Unavailable,
            Refusal::Stale => TokenRejection::DeviceProofStale,
            _ => TokenRejection::DeviceProofRequired,
        }
    })
}

/// The device proof of `/auth/refresh` for refresh token `refresh_hash`.
/// Runs before the replay cache, so a copied refresh token cannot even read
/// a cached rotation. Unknown tokens are left to the rotation's own answer.
pub(crate) async fn check_refresh_proof(
    state: &AppState,
    headers: &HeaderMap,
    refresh_hash: &[u8; 32],
) -> Result<(), Response> {
    if mode(state, Surface::Auth) == DeviceAdmissionMode::Off {
        return Ok(());
    }
    let stored = match state.db.refresh_device_key(refresh_hash).await {
        Ok(Some(stored)) => stored,
        Ok(None) => return Ok(()),
        Err(error) if mode(state, Surface::Auth) == DeviceAdmissionMode::Enforce => {
            return Err(crate::api::auth::internal("refresh_device_key", &error));
        }
        Err(error) => {
            tracing::warn!(%error, "refresh device key unavailable; not enforced");
            return Ok(());
        }
    };
    // A stored key that no longer parses counts as keyless (fails closed).
    let key = stored.and_then(|key| DevicePubkey::from_sec1(&key).ok());
    check_header(state, key.as_ref(), headers, PURPOSE_REFRESH, refresh_hash)
        .await
        .map_err(crate::api::auth::rejection_response)
}

/// A message-path use of an access token (device-bound keys plan §3). Tokens
/// carry no device signature, so device-bound messaging closes them: `log`
/// reports the use (reason `token_path`), `enforce` refuses it with
/// [`TokenRejection::MessagePathClosed`]. Account calls (`/auth/*`) stay open.
pub(crate) fn close_token_path(
    state: &AppState,
    binding: &TokenBinding,
) -> Result<(), TokenRejection> {
    let mode = mode(state, Surface::Token);
    if mode == DeviceAdmissionMode::Off {
        return Ok(());
    }
    let enforced = mode == DeviceAdmissionMode::Enforce;
    report(
        state,
        Surface::Token,
        &binding.principal.to_hex(),
        "token_path",
        enforced,
    );
    if enforced {
        Err(TokenRejection::MessagePathClosed)
    } else {
        Ok(())
    }
}

/// The device key a login `start` binds (`device_pubkey` query value),
/// normalized to compressed hex. `Ok(None)`: none, or the auth surface is off.
#[allow(clippy::result_large_err)] // Response is the natural error type for axum handlers
pub(crate) fn start_device_key(
    state: &AppState,
    raw: Option<&str>,
) -> Result<Option<String>, Response> {
    let Some(raw) = raw.filter(|_| mode(state, Surface::Auth) != DeviceAdmissionMode::Off) else {
        return Ok(None);
    };
    let outcome = DevicePubkey::from_hex(raw).map_err(|_| Refusal::Malformed);
    match outcome {
        Ok(key) => Ok(Some(key.to_hex())),
        Err(refusal) => judge(state, Surface::Auth, "start", Err(refusal))
            .map(|()| None)
            .map_err(login_refused),
    }
}

/// A device key a login proved.
pub(crate) struct LoginDeviceKey {
    /// The key.
    pub(crate) key: DevicePubkey,
    /// `hardware` or `software`.
    pub(crate) kind: &'static str,
}

/// The device key a login `complete`/`signup` presents (`device_pubkey`,
/// `device_key_kind`, `device_proof` for purpose `bind` over `binding`),
/// which must be the key its `start` bound. `Ok(None)`: no key (an older
/// app), the auth surface is off, or a failing key under `log` (the device
/// is then recorded keyless, never with an unproven key).
pub(crate) async fn login_device_key(
    state: &AppState,
    body: &serde_json::Map<String, serde_json::Value>,
    binding: &str,
    bound: Option<&str>,
) -> Result<Option<LoginDeviceKey>, Response> {
    if mode(state, Surface::Auth) == DeviceAdmissionMode::Off {
        return Ok(None);
    }
    let raw_key = body.get("device_pubkey").and_then(|v| v.as_str());
    let outcome = if raw_key.is_none() && bound.is_none() {
        Err(Refusal::Keyless)
    } else {
        presented(state, body, raw_key, binding, bound).await
    };
    match outcome {
        Ok(key) => Ok(Some(key)),
        Err(refusal) => judge(state, Surface::Auth, binding, Err(refusal))
            .map(|()| None)
            .map_err(login_refused),
    }
}

async fn presented(
    state: &AppState,
    body: &serde_json::Map<String, serde_json::Value>,
    raw_key: Option<&str>,
    binding: &str,
    bound: Option<&str>,
) -> Result<LoginDeviceKey, Refusal> {
    let key = DevicePubkey::from_hex(raw_key.ok_or(Refusal::Mismatch)?)
        .map_err(|_| Refusal::Malformed)?;
    if bound != Some(key.to_hex().as_str()) {
        return Err(Refusal::Mismatch);
    }
    let kind = match body.get("device_key_kind").and_then(|v| v.as_str()) {
        Some("hardware") => "hardware",
        Some("software") => "software",
        _ => return Err(Refusal::Malformed),
    };
    let proof = body
        .get("device_proof")
        .and_then(|v| v.as_str())
        .ok_or(Refusal::Missing)?;
    verify_proof(
        state,
        &key,
        proof,
        PURPOSE_BIND,
        &[binding.to_owned()],
        None,
        None,
    )
    .await?;
    Ok(LoginDeviceKey { key, kind })
}

/// The answer to an enforced login refusal. The login code is already
/// spent (one attempt only), so the client signs in again from the start;
/// a stale clock is corrected from [`SERVER_TIME_HEADER`] of `start` first.
fn login_refused(refusal: Refusal) -> Response {
    let (code, message) = match refusal {
        Refusal::Unavailable => return crate::api::auth::unavailable(),
        Refusal::Keyless => ("device_proof_required", "a device key is required"),
        Refusal::Stale => ("device_proof_stale", "device key proof time is off"),
        _ => ("device_proof_invalid", "device key proof did not verify"),
    };
    crate::api::auth::auth_error(StatusCode::UNAUTHORIZED, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_parse_known_surfaces_only() {
        let fields: HashMap<String, String> = [
            ("auth".to_owned(), "Enforce".to_owned()),
            ("ws".to_owned(), "log".to_owned()),
            ("git".to_owned(), "enforce".to_owned()),
            ("push".to_owned(), "enforce".to_owned()),
        ]
        .into();
        assert_eq!(
            parse_overrides(&fields),
            [
                (Surface::Auth, DeviceAdmissionMode::Enforce),
                (Surface::Ws, DeviceAdmissionMode::Log),
                (Surface::Git, DeviceAdmissionMode::Enforce),
            ]
            .into()
        );
        let bad: HashMap<String, String> = [("auth".to_owned(), "on".to_owned())].into();
        assert!(parse_overrides(&bad).is_empty());
        // Every surface has its own override name.
        let names: std::collections::HashSet<_> =
            Surface::ALL.into_iter().map(Surface::as_str).collect();
        assert_eq!(names.len(), Surface::ALL.len());
    }

    /// A token on a message path: `off` does nothing, `log` admits and
    /// reports, `enforce` closes it with `token_message_path_closed`.
    #[tokio::test(flavor = "current_thread")]
    async fn the_token_path_closes_only_under_enforce() {
        let binding = TokenBinding {
            principal: kuzz_core::principal::PrincipalId::from_slice(
                &nostr::Keys::generate().public_key().to_bytes(),
            )
            .expect("principal"),
            kind: AccessTokenKind::User,
            token_hash: [0u8; 32],
            expires_at: None,
            device_id: None,
            session_id: None,
            bot_id: None,
            bot_owner: None,
            is_operator: false,
            device_pubkey: None,
        };
        for (mode, closed, reports) in [
            (DeviceAdmissionMode::Off, false, 0),
            (DeviceAdmissionMode::Log, false, 1),
            (DeviceAdmissionMode::Enforce, true, 1),
        ] {
            let recorder = metrics_util::debugging::DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            let _guard = metrics::set_default_local_recorder(&recorder);
            let state = crate::state::tests::test_state_with_config(
                "postgres://kuzz:kuzz_dev@127.0.0.1:1/kuzz",
                |config| config.auth_token.device_admission = mode,
            )
            .await;
            let outcome = close_token_path(&state, &binding);
            assert_eq!(outcome.is_err(), closed, "{mode:?}");
            if let Err(rejection) = outcome {
                assert_eq!(rejection.code(), "token_message_path_closed");
            }
            let counted = snapshotter
                .snapshot()
                .into_vec()
                .into_iter()
                .filter(|(key, ..)| key.key().name() == "kuzz_device_proof_refusals_total")
                .count();
            assert_eq!(counted, reports, "{mode:?}");
        }
    }

    #[test]
    fn a_would_refuse_line_is_reported_once() {
        let runtime = DeviceRuntime::default();
        assert!(runtime.first_report("a".into()));
        assert!(!runtime.first_report("a".into()));
        assert!(runtime.first_report("b".into()));
    }
}
