//! HTTP client for the relay's `/auth/*` surface (plan §3.3).
//!
//! Every call takes the relay's HTTP origin explicitly, so a community switch
//! can never retarget a request already in flight. Errors keep the relay's
//! machine code (`{"error", "code"}`) so callers can classify them.

use serde::Deserialize;
use zeroize::Zeroizing;

/// Per-request deadline for `/auth/*` calls.
pub(crate) const AUTH_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// A token field deserialized straight into zeroizing memory.
#[derive(Clone)]
pub(crate) struct SecretField(pub Zeroizing<String>);

impl<'de> Deserialize<'de> for SecretField {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| Self(Zeroizing::new(s)))
    }
}

impl std::ops::Deref for SecretField {
    type Target = str;
    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

/// A failed `/auth/*` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiError {
    /// HTTP status; `None` when the request never got a response.
    pub status: Option<u16>,
    /// The relay's error code (`invalid_token`, `refresh_reused`, …).
    pub code: Option<String>,
    /// Human-readable message (never contains a token).
    pub message: String,
}

impl ApiError {
    fn network(error: &reqwest::Error) -> Self {
        Self {
            status: None,
            code: None,
            message: crate::relay::classify_request_error(error),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.status, &self.code) {
            (Some(status), Some(code)) => write!(f, "{} (HTTP {status}, {code})", self.message),
            (Some(status), None) => write!(f, "{} (HTTP {status})", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

/// Tokens returned by `POST /auth/oidc/complete`.
#[derive(Deserialize)]
pub(crate) struct LoginTokens {
    pub principal_id: String,
    #[serde(default)]
    pub identity_mode: Option<String>,
    #[serde(default)]
    pub signing_pubkey: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    pub access: SecretField,
    pub refresh: SecretField,
    pub expires_in: i64,
}

/// Tokens returned by `POST /auth/refresh`.
#[derive(Deserialize)]
pub(crate) struct RefreshedTokens {
    pub access: SecretField,
    pub refresh: SecretField,
    pub expires_in: i64,
}

/// One row of `GET /auth/devices`.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub(crate) struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub current: bool,
}

/// `GET /auth/me`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MeInfo {
    pub principal_id: String,
    #[serde(default)]
    pub identity_mode: Option<String>,
    #[serde(default)]
    pub signing_pubkey: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
}

/// A bot access token (`POST /auth/bots/{id}/token`).
#[derive(Deserialize)]
pub(crate) struct BotToken {
    pub token: SecretField,
    /// RFC 3339 expiry.
    pub expires_at: String,
}

impl BotToken {
    /// Expiry as unix seconds, when parseable.
    pub(crate) fn expires_at_unix(&self) -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .ok()
            .map(|at| at.timestamp())
    }
}

#[derive(Deserialize)]
struct BotCreated {
    bot_id: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

async fn error_from(response: reqwest::Response) -> ApiError {
    let status = response.status().as_u16();
    let body: Option<ErrorBody> = response.json().await.ok();
    ApiError {
        status: Some(status),
        code: body.as_ref().and_then(|b| b.code.clone()),
        message: body
            .and_then(|b| b.error)
            .unwrap_or_else(|| format!("relay returned {status}")),
    }
}

async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
    let response = request
        .timeout(AUTH_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| ApiError::network(&e))?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(error_from(response).await)
    }
}

pub(super) async fn send_json<T: for<'de> Deserialize<'de>>(
    request: reqwest::RequestBuilder,
) -> Result<T, ApiError> {
    send(request)
        .await?
        .json::<T>()
        .await
        .map_err(|_| ApiError {
            status: None,
            code: None,
            message: "relay returned a malformed auth response".into(),
        })
}

fn bearer(access: &str) -> String {
    format!("Bearer {access}")
}

/// NIP-11 `buzz_token_auth` descriptor: `Some(providers)` when the relay
/// advertises bearer token auth, `None` otherwise.
pub(crate) async fn fetch_token_auth_support(
    client: &reqwest::Client,
    origin: &str,
) -> Result<(Option<Vec<String>>, bool), ApiError> {
    let value: serde_json::Value = send_json(
        client
            .get(format!("{origin}/"))
            .header("Accept", "application/nostr+json"),
    )
    .await?;
    let backup = value
        .get("buzz_key_backup")
        .and_then(|v| v.get("version"))
        .and_then(serde_json::Value::as_u64)
        == Some(1);
    Ok((parse_token_auth_descriptor(&value), backup))
}

/// Parse the NIP-11 descriptor. Pure, for tests.
pub(crate) fn parse_token_auth_descriptor(nip11: &serde_json::Value) -> Option<Vec<String>> {
    if let Some(providers) = nip11
        .get("buzz_key_backup")
        .filter(|value| value.get("version").and_then(serde_json::Value::as_u64) == Some(1))
        .and_then(|value| value.get("providers"))
        .and_then(serde_json::Value::as_array)
    {
        return Some(
            providers
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect(),
        );
    }
    let descriptor = nip11.get("buzz_token_auth")?;
    if descriptor
        .get("bearer")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return None;
    }
    Some(
        descriptor
            .get("oidc_providers")
            .and_then(serde_json::Value::as_array)
            .map(|providers| {
                providers
                    .iter()
                    .filter_map(|p| p.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Browser start URL for `provider`.
pub(crate) fn start_url(
    origin: &str,
    provider: &str,
    state: &str,
    challenge: &str,
    redirect_uri: &str,
    device_name: &str,
) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("client", "desktop")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("device_name", device_name)
        .finish();
    format!("{origin}/auth/oidc/{provider}/start?{query}")
}

/// `POST /auth/oidc/complete`.
pub(crate) async fn complete_login(
    client: &reqwest::Client,
    origin: &str,
    login_code: &str,
    verifier: &str,
) -> Result<LoginTokens, ApiError> {
    send_json(
        client
            .post(format!("{origin}/auth/oidc/complete"))
            .json(&serde_json::json!({ "login_code": login_code, "code_verifier": verifier })),
    )
    .await
}

/// `POST /auth/refresh`.
pub(crate) async fn refresh(
    client: &reqwest::Client,
    origin: &str,
    refresh_token: &str,
) -> Result<RefreshedTokens, ApiError> {
    send_json(
        client
            .post(format!("{origin}/auth/refresh"))
            .json(&serde_json::json!({ "refresh": refresh_token })),
    )
    .await
}

/// `GET /auth/me`.
pub(crate) async fn me(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
) -> Result<MeInfo, ApiError> {
    send_json(
        client
            .get(format!("{origin}/auth/me"))
            .header("Authorization", bearer(access)),
    )
    .await
}

/// A bearer-authenticated call with an optional JSON body and no response body.
async fn call(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: String,
    access: &str,
    body: Option<serde_json::Value>,
) -> Result<(), ApiError> {
    let mut request = client
        .request(method, url)
        .header("Authorization", bearer(access));
    if let Some(body) = body {
        request = request.json(&body);
    }
    send(request).await.map(|_| ())
}

/// `POST /auth/logout`.
pub(crate) async fn logout(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
) -> Result<(), ApiError> {
    call(
        client,
        reqwest::Method::POST,
        format!("{origin}/auth/logout"),
        access,
        Some(serde_json::json!({})),
    )
    .await
}

/// `GET /auth/devices`.
pub(crate) async fn list_devices(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
) -> Result<Vec<DeviceInfo>, ApiError> {
    send_json(
        client
            .get(format!("{origin}/auth/devices"))
            .header("Authorization", bearer(access)),
    )
    .await
}

/// `PATCH /auth/devices/{id}` — rename; returns the updated row.
pub(crate) async fn rename_device(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    device_id: &str,
    name: &str,
) -> Result<DeviceInfo, ApiError> {
    let device_id = uuid::Uuid::parse_str(device_id).map_err(|_| ApiError {
        status: None,
        code: None,
        message: "invalid device id".into(),
    })?;
    send_json(
        client
            .patch(format!("{origin}/auth/devices/{device_id}"))
            .header("Authorization", bearer(access))
            .json(&serde_json::json!({ "name": name })),
    )
    .await
}

/// `DELETE /auth/devices/{id}`.
pub(crate) async fn revoke_device(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    device_id: &str,
) -> Result<(), ApiError> {
    let device_id = uuid::Uuid::parse_str(device_id).map_err(|_| ApiError {
        status: None,
        code: None,
        message: "invalid device id".into(),
    })?;
    call(
        client,
        reqwest::Method::DELETE,
        format!("{origin}/auth/devices/{device_id}"),
        access,
        None,
    )
    .await
}

/// `POST <path>` with an empty JSON body (revoke-others, revoke-all, …).
pub(crate) async fn post_empty(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    path: &str,
) -> Result<(), ApiError> {
    call(
        client,
        reqwest::Method::POST,
        format!("{origin}{path}"),
        access,
        Some(serde_json::json!({})),
    )
    .await
}

/// `DELETE /auth/account`.
pub(crate) async fn delete_account(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
) -> Result<(), ApiError> {
    call(
        client,
        reqwest::Method::DELETE,
        format!("{origin}/auth/account"),
        access,
        Some(serde_json::json!({})),
    )
    .await
}

/// `PATCH /auth/profile`.
pub(crate) async fn update_profile(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    body: serde_json::Value,
) -> Result<(), ApiError> {
    call(
        client,
        reqwest::Method::PATCH,
        format!("{origin}/auth/profile"),
        access,
        Some(body),
    )
    .await
}

/// `POST /auth/bots` (hosted on this device).
pub(crate) async fn create_bot(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    display_name: &str,
) -> Result<String, ApiError> {
    let created: BotCreated = send_json(
        client
            .post(format!("{origin}/auth/bots"))
            .header("Authorization", bearer(access))
            .json(&serde_json::json!({ "display_name": display_name, "host": "this_device" })),
    )
    .await?;
    Ok(created.bot_id)
}

fn bot_path(origin: &str, bot_id: &str, suffix: &str) -> Result<String, ApiError> {
    if bot_id.len() != 64 || !bot_id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError {
            status: None,
            code: None,
            message: "invalid bot id".into(),
        });
    }
    Ok(format!("{origin}/auth/bots/{bot_id}{suffix}"))
}

/// `POST /auth/bots/{id}/token`.
pub(crate) async fn issue_bot_token(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    bot_id: &str,
) -> Result<BotToken, ApiError> {
    send_json(
        client
            .post(bot_path(origin, bot_id, "/token")?)
            .header("Authorization", bearer(access))
            .json(&serde_json::json!({})),
    )
    .await
}

/// `POST /auth/bots/{id}/revoke` (agent Stop).
pub(crate) async fn stop_bot(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    bot_id: &str,
) -> Result<(), ApiError> {
    call(
        client,
        reqwest::Method::POST,
        bot_path(origin, bot_id, "/revoke")?,
        access,
        Some(serde_json::json!({})),
    )
    .await
}

/// `DELETE /auth/bots/{id}`.
pub(crate) async fn delete_bot(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    bot_id: &str,
) -> Result<(), ApiError> {
    call(
        client,
        reqwest::Method::DELETE,
        bot_path(origin, bot_id, "")?,
        access,
        None,
    )
    .await
}

/// `PATCH /auth/bots/{id}/profile`.
pub(crate) async fn update_bot_profile(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    bot_id: &str,
    body: serde_json::Value,
) -> Result<(), ApiError> {
    call(
        client,
        reqwest::Method::PATCH,
        bot_path(origin, bot_id, "/profile")?,
        access,
        Some(body),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_requires_bearer_true() {
        let on = serde_json::json!({"buzz_token_auth": {"version": 1, "bearer": true, "oidc_providers": ["google"]}});
        assert_eq!(
            parse_token_auth_descriptor(&on),
            Some(vec!["google".to_string()])
        );
        let off = serde_json::json!({"buzz_token_auth": {"bearer": false}});
        assert_eq!(parse_token_auth_descriptor(&off), None);
        assert_eq!(parse_token_auth_descriptor(&serde_json::json!({})), None);
    }

    #[test]
    fn start_url_carries_pkce_and_loopback() {
        let url = start_url(
            "http://127.0.0.1:3000",
            "google",
            "st",
            "ch",
            "http://127.0.0.1:5555/cb",
            "My Mac",
        );
        assert!(url.starts_with("http://127.0.0.1:3000/auth/oidc/google/start?"));
        assert!(url.contains("state=st"));
        assert!(url.contains("code_challenge=ch"));
        assert!(url.contains("client=desktop"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5555%2Fcb"));
        assert!(url.contains("device_name=My+Mac"));
    }

    #[test]
    fn bot_paths_reject_non_hex_ids() {
        assert!(bot_path("http://h", "../x", "").is_err());
        assert!(bot_path("http://h", &"a".repeat(64), "/token").is_ok());
    }
}
