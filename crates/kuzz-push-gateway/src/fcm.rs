//! FCM HTTP v1 transport for the `kuzz-android` profile.
//!
//! The gateway authenticates with a Google service account: it signs an RS256
//! JWT assertion, exchanges it for an OAuth2 access token, caches that token
//! for about 50 minutes, and refreshes it at most once per request when FCM
//! answers 401. Every delivery is the same data-only wake message; the only
//! per-delivery inputs are the device token and the TTL. Provider bodies are
//! read with a byte cap and never logged or returned.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::{
    header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER},
    StatusCode,
};
use serde::Deserialize;
use thiserror::Error;

use crate::apns::{DeliveryAttempt, DeliveryOutcome, PushTransport};

/// Production Google OAuth2 token endpoint. It is also the JWT audience. The
/// service-account file's own `token_uri` is ignored so a credential file
/// cannot redirect the signed assertion elsewhere.
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Production FCM HTTP v1 origin.
pub const FCM_BASE_URL: &str = "https://fcm.googleapis.com";
/// OAuth2 scope for FCM HTTP v1 sends.
pub const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
/// Largest FCM registration token accepted (N11). Longer tokens are refused at
/// enrollment and endpoint rotation.
pub const MAX_FCM_TOKEN_BYTES: usize = 512;
/// FCM's maximum message TTL (28 days).
pub const MAX_FCM_TTL_SECONDS: i64 = 2_419_200;
/// Collapse key shared by every wake, so a device that was offline gets one.
pub const FCM_COLLAPSE_KEY: &str = "kuzz-wake";

const MAX_SERVICE_ACCOUNT_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_ACCESS_TOKEN_BYTES: usize = 4096;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const ASSERTION_LIFETIME_SECONDS: i64 = 3600;
const TOKEN_REFRESH_MARGIN_SECONDS: i64 = 600;
const TOKEN_FAILURE_BACKOFF_SECONDS: i64 = 30;

/// Service-account setup failure. It never carries credential material.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FcmError {
    /// The service-account JSON is missing a field or has an unusable value.
    #[error("invalid FCM service account: {0}")]
    ServiceAccount(&'static str),
    /// The HTTP client could not be built.
    #[error("FCM HTTP client setup failed")]
    Client,
}

#[derive(Deserialize)]
struct ServiceAccountFile {
    #[serde(rename = "type")]
    kind: String,
    project_id: String,
    private_key_id: String,
    private_key: String,
    client_email: String,
}

/// Parsed service account. The private key is held only as a signing key and
/// is not printable through `Debug`.
pub struct ServiceAccount {
    project_id: String,
    private_key_id: String,
    client_email: String,
    signing_key: jsonwebtoken::EncodingKey,
}

impl std::fmt::Debug for ServiceAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceAccount")
            .field("project_id", &self.project_id)
            .field("client_email", &self.client_email)
            .finish_non_exhaustive()
    }
}

impl ServiceAccount {
    /// Parse a Google service-account JSON key file.
    pub fn from_json(bytes: &[u8]) -> Result<Self, FcmError> {
        if bytes.len() > MAX_SERVICE_ACCOUNT_BYTES {
            return Err(FcmError::ServiceAccount("file too large"));
        }
        let file: ServiceAccountFile =
            serde_json::from_slice(bytes).map_err(|_| FcmError::ServiceAccount("not JSON"))?;
        if file.kind != "service_account" {
            return Err(FcmError::ServiceAccount("type"));
        }
        let project_ok = !file.project_id.is_empty()
            && file.project_id.len() <= 64
            && file
                .project_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !project_ok {
            return Err(FcmError::ServiceAccount("project_id"));
        }
        if file.private_key_id.is_empty()
            || file.private_key_id.len() > 128
            || !file
                .private_key_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric())
        {
            return Err(FcmError::ServiceAccount("private_key_id"));
        }
        if file.client_email.is_empty()
            || file.client_email.len() > 320
            || !file.client_email.contains('@')
            || !file.client_email.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(FcmError::ServiceAccount("client_email"));
        }
        let signing_key = jsonwebtoken::EncodingKey::from_rsa_pem(file.private_key.as_bytes())
            .map_err(|_| FcmError::ServiceAccount("private_key"))?;
        Ok(Self {
            project_id: file.project_id,
            private_key_id: file.private_key_id,
            client_email: file.client_email,
            signing_key,
        })
    }

    /// Firebase project that receives the sends.
    pub fn project_id(&self) -> &str {
        &self.project_id
    }
}

#[derive(serde::Serialize)]
struct AssertionClaims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: i64,
    exp: i64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
}

#[derive(Default)]
struct TokenState {
    cached: Option<CachedToken>,
    backoff_until: i64,
}

#[derive(Clone)]
struct CachedToken {
    value: String,
    refresh_at: i64,
}

/// Why no access token is available. Mapped onto a delivery outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenFailure {
    /// The credential was refused; deliveries are a configuration fault.
    Credential,
    /// Transient: network, timeout, 429/5xx or local back-off.
    Transient,
}

impl TokenFailure {
    fn outcome(self) -> DeliveryOutcome {
        match self {
            Self::Credential => DeliveryOutcome::ConfigurationFault,
            Self::Transient => DeliveryOutcome::Retry {
                retry_after_seconds: Some(TOKEN_FAILURE_BACKOFF_SECONDS),
            },
        }
    }
}

/// Clock used for JWT timestamps, token expiry and TTLs.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// FCM HTTP v1 transport bound to one service account and project.
pub struct FcmTransport {
    client: reqwest::Client,
    account: ServiceAccount,
    token_url: String,
    send_url: String,
    now: Clock,
    // Held across the token fetch, so concurrent deliveries share one refresh.
    token: tokio::sync::Mutex<TokenState>,
}

impl FcmTransport {
    /// Production transport: pinned Google token endpoint and FCM origin.
    pub fn new(account: ServiceAccount) -> Result<Self, FcmError> {
        Self::with_urls(
            account,
            GOOGLE_TOKEN_URL.to_owned(),
            FCM_BASE_URL,
            Arc::new(|| chrono::Utc::now().timestamp()),
        )
    }

    fn with_urls(
        account: ServiceAccount,
        token_url: String,
        fcm_base_url: &str,
        now: Clock,
    ) -> Result<Self, FcmError> {
        let client = reqwest::Client::builder()
            .connect_timeout(REQUEST_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| FcmError::Client)?;
        let send_url = format!(
            "{fcm_base_url}/v1/projects/{}/messages:send",
            account.project_id
        );
        Ok(Self {
            client,
            account,
            token_url,
            send_url,
            now,
            token: tokio::sync::Mutex::new(TokenState::default()),
        })
    }

    /// Return a cached token, or fetch one. With `stale`, a cached token equal
    /// to `stale` is treated as rejected and replaced, unless another request
    /// already replaced it.
    async fn access_token(&self, stale: Option<&str>) -> Result<String, TokenFailure> {
        let mut state = self.token.lock().await;
        let now = (self.now)();
        if let Some(cached) = &state.cached {
            let rejected = stale == Some(cached.value.as_str());
            if !rejected && now < cached.refresh_at {
                return Ok(cached.value.clone());
            }
        }
        if now < state.backoff_until {
            return Err(TokenFailure::Transient);
        }
        match self.fetch_token(now).await {
            Ok(fresh) => {
                state.cached = Some(fresh.clone());
                state.backoff_until = 0;
                Ok(fresh.value)
            }
            Err(failure) => {
                state.cached = None;
                state.backoff_until = now.saturating_add(TOKEN_FAILURE_BACKOFF_SECONDS);
                tracing::warn!(failure = ?failure, "FCM access token refresh failed");
                Err(failure)
            }
        }
    }

    async fn fetch_token(&self, now: i64) -> Result<CachedToken, TokenFailure> {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(self.account.private_key_id.clone());
        let claims = AssertionClaims {
            iss: &self.account.client_email,
            scope: FCM_SCOPE,
            aud: GOOGLE_TOKEN_URL,
            iat: now,
            exp: now.saturating_add(ASSERTION_LIFETIME_SECONDS),
        };
        let assertion = jsonwebtoken::encode(&header, &claims, &self.account.signing_key)
            .map_err(|_| TokenFailure::Credential)?;
        // The assertion is base64url segments joined by '.', all form-safe.
        let body = format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={assertion}"
        );
        let response = self
            .client
            .post(&self.token_url)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|_| TokenFailure::Transient)?;
        let status = response.status();
        let bytes = read_capped(response)
            .await
            .map_err(|_| TokenFailure::Transient)?;
        if status != StatusCode::OK {
            return Err(
                if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    TokenFailure::Transient
                } else {
                    TokenFailure::Credential
                },
            );
        }
        let token: TokenResponse =
            serde_json::from_slice(&bytes).map_err(|_| TokenFailure::Transient)?;
        if token.access_token.is_empty()
            || token.access_token.len() > MAX_ACCESS_TOKEN_BYTES
            || !token.access_token.bytes().all(|b| b.is_ascii_graphic())
            || token.expires_in <= 0
        {
            return Err(TokenFailure::Transient);
        }
        let lifetime = token.expires_in.min(ASSERTION_LIFETIME_SECONDS);
        let usable = (lifetime - TOKEN_REFRESH_MARGIN_SECONDS).max(lifetime / 2);
        Ok(CachedToken {
            value: token.access_token,
            refresh_at: now.saturating_add(usable),
        })
    }

    async fn post_message(
        &self,
        access_token: &str,
        message: &[u8],
    ) -> Result<(StatusCode, Option<i64>, Vec<u8>), reqwest::Error> {
        let response = self
            .client
            .post(&self.send_url)
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(CONTENT_TYPE, "application/json")
            .body(message.to_vec())
            .send()
            .await?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<i64>().ok())
            .map(|seconds| seconds.clamp(1, 3600));
        let body = read_capped(response).await?;
        Ok((status, retry_after, body))
    }
}

/// Read at most `MAX_RESPONSE_BYTES` of a response body; the rest is dropped.
async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, reqwest::Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let room = MAX_RESPONSE_BYTES - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() == MAX_RESPONSE_BYTES {
            break;
        }
    }
    Ok(body)
}

/// Decode a hex endpoint into the FCM registration token it carries.
pub fn fcm_token_from_endpoint(endpoint_hex: &str) -> Option<String> {
    let bytes = hex::decode(endpoint_hex).ok()?;
    valid_fcm_token(&bytes).then(|| String::from_utf8(bytes).ok())?
}

/// FCM registration tokens are printable ASCII and at most 512 bytes.
pub fn valid_fcm_token(token: &[u8]) -> bool {
    !token.is_empty()
        && token.len() <= MAX_FCM_TOKEN_BYTES
        && token.iter().all(|b| b.is_ascii_graphic())
}

/// Seconds FCM may hold the wake, from the relay's delivery deadline.
pub fn ttl_seconds(expires_at: i64, now: i64) -> i64 {
    expires_at.saturating_sub(now).clamp(0, MAX_FCM_TTL_SECONDS)
}

/// The single FCM message shape: data-only wake, high priority, collapsed.
fn wake_message(token: &str, ttl: i64) -> Vec<u8> {
    serde_json::json!({
        "message": {
            "token": token,
            "data": {"k": "wake"},
            "android": {
                "priority": "HIGH",
                "ttl": format!("{ttl}s"),
                "collapse_key": FCM_COLLAPSE_KEY,
            }
        }
    })
    .to_string()
    .into_bytes()
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorStatus,
}

#[derive(Deserialize)]
struct ErrorStatus {
    status: Option<String>,
    #[serde(default)]
    details: Vec<serde_json::Value>,
}

const FCM_ERROR_TYPE: &str = "type.googleapis.com/google.firebase.fcm.v1.FcmError";
const BAD_REQUEST_TYPE: &str = "type.googleapis.com/google.rpc.BadRequest";

/// Classify a non-200 FCM response without exposing the body.
pub fn classify(status: u16, body: &[u8], retry_after: Option<i64>) -> DeliveryOutcome {
    let parsed = serde_json::from_slice::<ErrorEnvelope>(body).ok();
    let rpc_status = parsed.as_ref().and_then(|e| e.error.status.as_deref());
    let details = parsed
        .as_ref()
        .map(|e| e.error.details.as_slice())
        .unwrap_or(&[]);
    let of_type = |kind: &'static str| {
        details
            .iter()
            .filter(move |d| d.get("@type").and_then(|t| t.as_str()) == Some(kind))
    };
    let fcm_code = of_type(FCM_ERROR_TYPE)
        .find_map(|d| d.get("errorCode").and_then(|c| c.as_str()))
        .or(rpc_status);
    let names_token = of_type(BAD_REQUEST_TYPE).any(|d| {
        d.get("fieldViolations")
            .and_then(|v| v.as_array())
            .is_some_and(|violations| {
                violations
                    .iter()
                    .any(|v| v.get("field").and_then(|f| f.as_str()) == Some("message.token"))
            })
    });
    let retry = DeliveryOutcome::Retry {
        retry_after_seconds: retry_after,
    };
    match (status, fcm_code) {
        (404, _) | (_, Some("UNREGISTERED" | "NOT_FOUND")) => DeliveryOutcome::InvalidEndpoint {
            unregistered_at: None,
        },
        (401 | 403, _)
        | (_, Some("SENDER_ID_MISMATCH" | "THIRD_PARTY_AUTH_ERROR" | "PERMISSION_DENIED")) => {
            DeliveryOutcome::ConfigurationFault
        }
        (429 | 500 | 502 | 503 | 504, _)
        | (_, Some("QUOTA_EXCEEDED" | "UNAVAILABLE" | "INTERNAL" | "RESOURCE_EXHAUSTED")) => retry,
        (400, _) | (_, Some("INVALID_ARGUMENT")) => {
            if names_token {
                DeliveryOutcome::InvalidEndpoint {
                    unregistered_at: None,
                }
            } else {
                DeliveryOutcome::PermanentRequestFault
            }
        }
        (code, _) if code >= 500 => retry,
        _ => DeliveryOutcome::PermanentRequestFault,
    }
}

#[async_trait]
impl PushTransport for FcmTransport {
    async fn send(&self, attempt: DeliveryAttempt, endpoint: &str) -> DeliveryOutcome {
        crate::metrics::record_fcm_send_attempt();
        let Some(token) = fcm_token_from_endpoint(endpoint) else {
            // Enrollment admits only valid tokens, so this is local corruption.
            return DeliveryOutcome::PermanentRequestFault;
        };
        let message = wake_message(&token, ttl_seconds(attempt.expires_at, (self.now)()));
        let mut stale: Option<String> = None;
        for _ in 0..2 {
            let access_token = match self.access_token(stale.as_deref()).await {
                Ok(value) => value,
                Err(failure) => return failure.outcome(),
            };
            let (status, retry_after, body) = match self.post_message(&access_token, &message).await
            {
                Ok(response) => response,
                Err(_) => {
                    return DeliveryOutcome::Retry {
                        retry_after_seconds: None,
                    }
                }
            };
            if status == StatusCode::OK {
                return DeliveryOutcome::Accepted;
            }
            if status == StatusCode::UNAUTHORIZED && stale.is_none() {
                // Expired or revoked access token: refresh once and resend.
                stale = Some(access_token);
                continue;
            }
            let outcome = classify(status.as_u16(), &body, retry_after);
            if outcome == DeliveryOutcome::ConfigurationFault {
                tracing::warn!(
                    status = status.as_u16(),
                    "FCM refused the gateway credential"
                );
            }
            return outcome;
        }
        DeliveryOutcome::ConfigurationFault
    }
}

#[cfg(test)]
#[path = "fcm_tests.rs"]
mod tests;
