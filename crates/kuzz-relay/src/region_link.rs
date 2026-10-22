//! Region link (global regions stage S4): relays of different regions call
//! each other.
//!
//! Every call carries an **entry attestation**: a short-lived Nostr event
//! signed with the calling region's link key (`KUZZ_REGION_KEY`). It names the
//! calling region, the community, the method, path and body hash of the call,
//! and — for calls made on a user's behalf — who the entry region admitted and
//! how. The receiving region checks the signature against the caller's
//! `regions.link_pubkey`, the time window, the request it is attached to, and
//! that it was not seen before. Token-login events carry no real signature,
//! so for forwarded writes this attestation is what tells the home region who
//! the author is.
//!
//! Off by default (`KUZZ_REGION_LINK`). With it off nothing here runs and no
//! route is mounted.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use kuzz_core::tenant::TenantContext;
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, JsonUtil, Keys, Kind, Tag, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::state::AppState;

/// Event kind of an entry attestation. Never stored or published.
pub const ATTESTATION_KIND: u16 = 27250;

/// `Authorization` scheme of a region link call.
pub const AUTH_SCHEME: &str = "KuzzRegion";

/// How long an attestation is valid, and how far apart the two regions'
/// clocks may be.
pub const ATTESTATION_WINDOW_SECS: u64 = 30;

/// Replay scope for attestation ids (shared with the NIP-98 seen-set store).
pub const REPLAY_SCOPE: &str = "kuzz:region-link";

/// Per-call deadline. Shorter than the apps' send retry, so an app retries
/// against a fresh attempt rather than racing a stalled one.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// How the entry region admitted the user a call is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryAuth {
    /// NIP-42 on a WebSocket.
    Nip42,
    /// NIP-98 on HTTP.
    Nip98,
    /// A Google-session access token (events are server-stamped).
    Token,
}

/// The user a call is made for, as the entry region admitted them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryPrincipal {
    /// Hex public key (the principal id for token accounts).
    pub pubkey: String,
    /// How the entry region admitted them.
    pub auth: EntryAuth,
    /// Scopes their credential carries.
    pub scopes: Vec<String>,
    /// Channels the credential is limited to; `None` means no limit.
    pub channel_ids: Option<Vec<Uuid>>,
}

/// What an attestation vouches for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Format version, `1`.
    pub v: u8,
    /// The calling (entry) region.
    pub entry_region: String,
    /// The region the call is meant for.
    pub target_region: String,
    /// The community the call is for (named explicitly, never implied).
    pub community_id: Uuid,
    /// HTTP method of the call.
    pub method: String,
    /// Path of the call.
    pub path: String,
    /// Lowercase hex SHA-256 of the request body.
    pub body_sha256: String,
    /// The room the call is about and the epoch the entry region trusted.
    pub channel_id: Option<Uuid>,
    /// The room's epoch the entry region trusted (see `channel_id`).
    pub epoch: Option<i32>,
    /// The user the call is made for; `None` for region-level calls.
    pub principal: Option<EntryPrincipal>,
    /// Random per call, so two identical calls in one second are still two
    /// attestations (the replay check keys on the event id).
    pub nonce: String,
}

/// Why an attestation was refused. Callers answer every case with the same
/// generic 401, so a caller cannot probe which check failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No `KuzzRegion` authorization.
    Missing,
    /// Not a decodable attestation.
    Malformed,
    /// Id or signature does not verify.
    BadSignature,
    /// Signed, but not an attestation.
    WrongKind,
    /// Outside its time window.
    Expired,
    /// Made for another request (method, path or body), or the request
    /// carries a query string, which attestations do not cover.
    WrongRequest,
    /// The community is not served here.
    UnknownCommunity,
    /// Meant for another region, or claims to come from this one.
    WrongTarget,
    /// The calling region is not registered here.
    UnknownRegion,
    /// Not signed with the calling region's link key.
    WrongKey,
    /// Seen before.
    Replayed,
    /// A lookup failed; refused rather than admitted.
    Unavailable,
}

impl Refusal {
    /// Metric label.
    pub fn label(&self) -> &'static str {
        match self {
            Refusal::Missing => "missing",
            Refusal::Malformed => "malformed",
            Refusal::BadSignature => "bad_signature",
            Refusal::WrongKind => "wrong_kind",
            Refusal::Expired => "expired",
            Refusal::WrongRequest => "wrong_request",
            Refusal::UnknownCommunity => "unknown_community",
            Refusal::WrongTarget => "wrong_target",
            Refusal::UnknownRegion => "unknown_region",
            Refusal::WrongKey => "wrong_key",
            Refusal::Replayed => "replayed",
            Refusal::Unavailable => "unavailable",
        }
    }
}

/// Hex SHA-256 of a body.
pub fn body_hash(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

/// Sign `claims` with the region link key; returns the `Authorization` value.
pub fn sign(keys: &Keys, claims: &Claims, now: Timestamp) -> Result<String, String> {
    let content = serde_json::to_string(claims).map_err(|e| e.to_string())?;
    let expires = Timestamp::from(now.as_secs() + ATTESTATION_WINDOW_SECS);
    let event = EventBuilder::new(Kind::Custom(ATTESTATION_KIND), content)
        .tags([Tag::expiration(expires)])
        .custom_created_at(now)
        .sign_with_keys(keys)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "{AUTH_SCHEME} {}",
        BASE64.encode(event.as_json().as_bytes())
    ))
}

/// What the receiving side checks a call against.
pub struct Expected<'a> {
    /// This region.
    pub local_region: &'a str,
    /// The request's method.
    pub method: &'a str,
    /// The request's path.
    pub path: &'a str,
    /// The request's body.
    pub body: &'a [u8],
    /// The current time.
    pub now: Timestamp,
}

/// Check everything that needs no lookup: format, signature, kind, time
/// window, and that the claims match this request and this region. Returns the
/// event (for its pubkey and id) and the claims.
pub fn check(header: Option<&str>, expected: &Expected<'_>) -> Result<(Event, Claims), Refusal> {
    let header = header.ok_or(Refusal::Missing)?;
    let encoded = header
        .strip_prefix(AUTH_SCHEME)
        .and_then(|rest| rest.strip_prefix(' '))
        .ok_or(Refusal::Missing)?;
    let bytes = BASE64
        .decode(encoded.trim())
        .map_err(|_| Refusal::Malformed)?;
    let json = std::str::from_utf8(&bytes).map_err(|_| Refusal::Malformed)?;
    let event = Event::from_json(json).map_err(|_| Refusal::Malformed)?;
    event.verify().map_err(|_| Refusal::BadSignature)?;
    if event.kind != Kind::Custom(ATTESTATION_KIND) {
        return Err(Refusal::WrongKind);
    }
    let now = expected.now.as_secs();
    let created = event.created_at.as_secs();
    let window = ATTESTATION_WINDOW_SECS;
    let expires = event
        .tags
        .expiration()
        .map(|t| t.as_secs())
        .ok_or(Refusal::Malformed)?;
    if created > now + window || now > expires || expires > created + window {
        return Err(Refusal::Expired);
    }
    let claims: Claims = serde_json::from_str(&event.content).map_err(|_| Refusal::Malformed)?;
    if claims.v != 1 {
        return Err(Refusal::Malformed);
    }
    if claims.target_region != expected.local_region || claims.entry_region == expected.local_region
    {
        return Err(Refusal::WrongTarget);
    }
    if !claims.method.eq_ignore_ascii_case(expected.method)
        || claims.path != expected.path
        || claims.body_sha256 != body_hash(expected.body)
    {
        return Err(Refusal::WrongRequest);
    }
    Ok((event, claims))
}

/// An attestation that passed every check, for a handler to act on.
#[derive(Debug, Clone)]
pub struct VerifiedCall {
    /// What the calling region vouched for.
    pub claims: Claims,
    /// The community named in the claims, served here. Region calls name
    /// their community; the `Host` they arrive on (the region's address) does
    /// not decide it.
    pub tenant: TenantContext,
}

/// Admit a region link request: no query string, then [`verify`] against
/// its own method, path and body. Every `/internal/region/*` handler starts
/// here.
pub async fn admit(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<VerifiedCall, Refusal> {
    if uri.query().is_some() {
        return Err(Refusal::WrongRequest);
    }
    let expected = Expected {
        local_region: &state.config.region_id,
        method: method.as_str(),
        path: uri.path(),
        body,
        now: Timestamp::now(),
    };
    verify(state, headers, &expected).await
}

/// Check a call's attestation fully: [`check`], then the calling region's
/// link key, then that the named community is served here, then that it was
/// not seen before.
pub async fn verify(
    state: &AppState,
    headers: &HeaderMap,
    expected: &Expected<'_>,
) -> Result<VerifiedCall, Refusal> {
    let (event, claims) = check_caller(state, headers, expected).await?;
    let community = CommunityId::from_uuid(claims.community_id);
    let Some(host) = state
        .db
        .lookup_community_host(community)
        .await
        .map_err(|_| Refusal::Unavailable)?
    else {
        return Err(Refusal::UnknownCommunity);
    };
    let tenant = TenantContext::resolved(community, host);
    mark_fresh(state, &event).await?;
    Ok(VerifiedCall { claims, tenant })
}

/// [`check`], then the calling region's link key.
async fn check_caller(
    state: &AppState,
    headers: &HeaderMap,
    expected: &Expected<'_>,
) -> Result<(Event, Claims), Refusal> {
    let header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let (event, claims) = check(header, expected)?;
    let Some((_, key)) = state
        .db
        .region_link(&claims.entry_region)
        .await
        .map_err(|_| Refusal::Unavailable)?
    else {
        return Err(Refusal::UnknownRegion);
    };
    if key.as_deref() != Some(event.pubkey.to_hex().as_str()) {
        return Err(Refusal::WrongKey);
    }
    Ok((event, claims))
}

/// Refuse an attestation this region has seen before.
async fn mark_fresh(state: &AppState, event: &Event) -> Result<(), Refusal> {
    match state
        .nip98_replay
        .try_mark_in_scope(REPLAY_SCOPE, &event.id, ATTESTATION_WINDOW_SECS * 2)
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(Refusal::Replayed),
        Err(_) => Err(Refusal::Unavailable),
    }
}

/// [`admit`] for a call that belongs to no community (sign-in state and
/// limits, S4-7): the claims name the nil community.
pub async fn admit_deployment(
    state: &AppState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Claims, Refusal> {
    if uri.query().is_some() {
        return Err(Refusal::WrongRequest);
    }
    let expected = Expected {
        local_region: &state.config.region_id,
        method: method.as_str(),
        path: uri.path(),
        body,
        now: Timestamp::now(),
    };
    let (event, claims) = check_caller(state, headers, &expected).await?;
    if !claims.community_id.is_nil() {
        return Err(Refusal::UnknownCommunity);
    }
    mark_fresh(state, &event).await?;
    Ok(claims)
}

/// This region's link key (`KUZZ_REGION_KEY`), or `None` when the link is
/// off. Config loading refuses a link without a valid key.
pub fn link_keys(state: &AppState) -> Option<Keys> {
    if !state.config.region_link {
        return None;
    }
    state
        .config
        .region_key
        .as_ref()
        .and_then(|key| Keys::parse(key.expose()).ok())
}

/// The link's HTTP client: per-call deadline, never follows redirects (a
/// link call goes to the registered address or fails).
fn http_client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(CALL_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .ok()
        })
        .as_ref()
}

/// The HTTP base of a region's connect address (`wss://x` → `https://x`).
pub fn http_base(connect_url: &str) -> String {
    let trimmed = connect_url.trim().trim_end_matches('/');
    if let Some(rest) = trimmed.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        trimmed.to_string()
    }
}

/// Why a link call failed. Every case is retryable by the caller's caller:
/// the link never half-applies a call.
#[derive(Debug)]
pub enum CallError {
    /// The link is off here, or this region has no key.
    Off,
    /// The target region is not registered here.
    UnknownRegion,
    /// Could not reach the target, or it timed out.
    Unreachable(String),
    /// The target answered with an error status.
    Status(StatusCode, Value),
}

/// POST `body` (JSON) to `path` on `target_region`, attested for `principal`
/// and room `channel` at `epoch`. Returns the target's JSON answer.
pub async fn call(
    state: &AppState,
    target_region: &str,
    community_id: Uuid,
    path: &str,
    body: &Value,
    room: Option<(Uuid, i32)>,
    principal: Option<EntryPrincipal>,
) -> Result<Value, CallError> {
    let keys = link_keys(state).ok_or(CallError::Off)?;
    let (connect_url, _) = state
        .db
        .region_link(target_region)
        .await
        .map_err(|e| CallError::Unreachable(e.to_string()))?
        .ok_or(CallError::UnknownRegion)?;
    let bytes = serde_json::to_vec(body).map_err(|e| CallError::Unreachable(e.to_string()))?;
    let claims = Claims {
        v: 1,
        entry_region: state.config.region_id.clone(),
        target_region: target_region.to_string(),
        community_id,
        method: Method::POST.to_string(),
        path: path.to_string(),
        body_sha256: body_hash(&bytes),
        channel_id: room.map(|(channel, _)| channel),
        epoch: room.map(|(_, epoch)| epoch),
        principal,
        nonce: Uuid::new_v4().simple().to_string(),
    };
    let client =
        http_client().ok_or_else(|| CallError::Unreachable("no HTTP client".to_string()))?;
    let auth = sign(&keys, &claims, Timestamp::now()).map_err(CallError::Unreachable)?;
    let url = format!("{}{path}", http_base(&connect_url));
    let started = std::time::Instant::now();
    let result = client
        .post(&url)
        .header(axum::http::header::AUTHORIZATION, auth)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(bytes)
        .send()
        .await;
    metrics::histogram!("kuzz_region_link_call_seconds", "target" => target_region.to_string())
        .record(started.elapsed().as_secs_f64());
    let response = result.map_err(|e| {
        metrics::counter!("kuzz_region_link_call_failures_total", "target" => target_region.to_string(), "reason" => "unreachable").increment(1);
        CallError::Unreachable(e.to_string())
    })?;
    let status = response.status();
    let value: Value = response.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        Ok(value)
    } else {
        metrics::counter!("kuzz_region_link_call_failures_total", "target" => target_region.to_string(), "reason" => "status").increment(1);
        Err(CallError::Status(status, value))
    }
}

/// `POST /internal/region/ping`: answers who this region is, after checking
/// the caller's attestation. Used by operators and the 2-region test to prove
/// a link works before anything rides on it.
pub async fn ping(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let refused = || crate::api::api_error(StatusCode::UNAUTHORIZED, "region link refused");
    match admit(&state, &Method::POST, &uri, &headers, &body).await {
        Ok(call) => {
            metrics::counter!("kuzz_region_link_calls_accepted_total", "from" => call.claims.entry_region.clone()).increment(1);
            Ok(Json(json!({
                "region": state.config.region_id,
                "from": call.claims.entry_region,
            })))
        }
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a region link call");
            Err(refused())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(body: &[u8]) -> Claims {
        Claims {
            v: 1,
            entry_region: "jp".into(),
            target_region: "kr".into(),
            community_id: Uuid::nil(),
            method: "POST".into(),
            path: "/internal/region/ping".into(),
            body_sha256: body_hash(body),
            channel_id: None,
            epoch: None,
            principal: Some(EntryPrincipal {
                pubkey: "ab".repeat(32),
                auth: EntryAuth::Token,
                scopes: vec!["messages:write".into()],
                channel_ids: None,
            }),
            nonce: "n1".into(),
        }
    }

    fn expected<'a>(body: &'a [u8], now: u64) -> Expected<'a> {
        Expected {
            local_region: "kr",
            method: "POST",
            path: "/internal/region/ping",
            body,
            now: Timestamp::from(now),
        }
    }

    #[test]
    fn a_signed_call_checks_out_for_its_own_request_only() {
        let keys = Keys::generate();
        let body = br#"{"x":1}"#;
        let now = 1_800_000_000;
        let auth = sign(&keys, &claims(body), Timestamp::from(now)).unwrap();

        let (event, got) = check(Some(&auth), &expected(body, now + 5)).unwrap();
        assert_eq!(event.pubkey, keys.public_key());
        assert_eq!(got, claims(body));

        // Another body, path, method or target: refused. (The community is
        // checked against this region's own list in `verify`.)
        assert_eq!(
            check(Some(&auth), &expected(b"{}", now)).unwrap_err(),
            Refusal::WrongRequest
        );
        let mut other = expected(body, now);
        other.path = "/internal/region/events";
        assert_eq!(
            check(Some(&auth), &other).unwrap_err(),
            Refusal::WrongRequest
        );
        let mut other = expected(body, now);
        other.method = "GET";
        assert_eq!(
            check(Some(&auth), &other).unwrap_err(),
            Refusal::WrongRequest
        );
        let mut other = expected(body, now);
        other.local_region = "us";
        assert_eq!(
            check(Some(&auth), &other).unwrap_err(),
            Refusal::WrongTarget
        );
    }

    #[test]
    fn an_attestation_is_short_lived() {
        let keys = Keys::generate();
        let body = b"{}";
        let now = 1_800_000_000;
        let auth = sign(&keys, &claims(body), Timestamp::from(now)).unwrap();
        assert!(check(Some(&auth), &expected(body, now + ATTESTATION_WINDOW_SECS)).is_ok());
        assert_eq!(
            check(
                Some(&auth),
                &expected(body, now + ATTESTATION_WINDOW_SECS + 1)
            )
            .unwrap_err(),
            Refusal::Expired
        );
        // Signed too far in the future (clock skew beyond the window).
        assert_eq!(
            check(
                Some(&auth),
                &expected(body, now - ATTESTATION_WINDOW_SECS - 1)
            )
            .unwrap_err(),
            Refusal::Expired
        );
    }

    #[test]
    fn a_region_cannot_attest_to_itself() {
        let keys = Keys::generate();
        let body = b"{}";
        let now = 1_800_000_000;
        let mut own = claims(body);
        own.entry_region = "kr".into();
        let auth = sign(&keys, &own, Timestamp::from(now)).unwrap();
        assert_eq!(
            check(Some(&auth), &expected(body, now)).unwrap_err(),
            Refusal::WrongTarget
        );
    }

    #[test]
    fn tampered_or_foreign_headers_are_refused() {
        let keys = Keys::generate();
        let body = b"{}";
        let now = 1_800_000_000;
        assert_eq!(
            check(None, &expected(body, now)).unwrap_err(),
            Refusal::Missing
        );
        assert_eq!(
            check(Some("Bearer abc"), &expected(body, now)).unwrap_err(),
            Refusal::Missing
        );
        assert_eq!(
            check(Some("KuzzRegion !!!"), &expected(body, now)).unwrap_err(),
            Refusal::Malformed
        );

        // Content changed after signing.
        let auth = sign(&keys, &claims(body), Timestamp::from(now)).unwrap();
        let json = String::from_utf8(
            BASE64
                .decode(auth.strip_prefix("KuzzRegion ").unwrap())
                .unwrap(),
        )
        .unwrap();
        let tampered = json.replace("\\\"jp\\\"", "\\\"us\\\"");
        assert_ne!(tampered, json);
        let forged = format!("KuzzRegion {}", BASE64.encode(tampered.as_bytes()));
        assert_eq!(
            check(Some(&forged), &expected(body, now)).unwrap_err(),
            Refusal::BadSignature
        );

        // A NIP-98 event (another kind) signed by the same key.
        let nip98 = EventBuilder::new(
            Kind::HttpAuth,
            serde_json::to_string(&claims(body)).unwrap(),
        )
        .tags([Tag::expiration(Timestamp::from(now + 10))])
        .custom_created_at(Timestamp::from(now))
        .sign_with_keys(&keys)
        .unwrap();
        let other_kind = format!("KuzzRegion {}", BASE64.encode(nip98.as_json().as_bytes()));
        assert_eq!(
            check(Some(&other_kind), &expected(body, now)).unwrap_err(),
            Refusal::WrongKind
        );
    }

    #[test]
    fn identical_calls_in_one_second_are_distinct_attestations() {
        let keys = Keys::generate();
        let now = Timestamp::from(1_800_000_000);
        let mut first = claims(b"{}");
        let mut second = first.clone();
        first.nonce = Uuid::new_v4().simple().to_string();
        second.nonce = Uuid::new_v4().simple().to_string();
        let a = check(
            Some(&sign(&keys, &first, now).unwrap()),
            &expected(b"{}", now.as_secs()),
        )
        .unwrap()
        .0;
        let b = check(
            Some(&sign(&keys, &second, now).unwrap()),
            &expected(b"{}", now.as_secs()),
        )
        .unwrap()
        .0;
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn connect_addresses_map_to_http() {
        assert_eq!(
            http_base("wss://jp.kuzz.example/"),
            "https://jp.kuzz.example"
        );
        assert_eq!(http_base("ws://localhost:3042"), "http://localhost:3042");
        assert_eq!(
            http_base("https://kr.kuzz.example"),
            "https://kr.kuzz.example"
        );
    }
}
