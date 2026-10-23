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

/// `POST /auth/oidc/complete`: the session, or, when the sign-in asked to
/// confirm new accounts and this Google account has none yet, a one-time
/// ticket to create it with ([`signup`]). Nothing exists on the relay until
/// then.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum CompleteResponse {
    NeedsSignup(SignupNeeded),
    Tokens(LoginTokens),
}

/// A Google account with no Kuzz account yet.
#[derive(Deserialize)]
pub(crate) struct SignupNeeded {
    #[allow(dead_code)]
    pub needs_signup: bool,
    pub signup_token: SecretField,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// Tokens returned by `POST /auth/oidc/complete` and `/auth/oidc/signup`.
#[derive(Deserialize)]
pub(crate) struct LoginTokens {
    pub principal_id: String,
    /// The Google account's email, from relays that send it.
    #[serde(default)]
    pub email: Option<String>,
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
    /// The login's profile (the Google name and picture).
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    /// The Google email of the session's account, from relays that send it.
    #[serde(default)]
    pub email: Option<String>,
    /// The account's Kuzz ID (`principals.username`), when set.
    #[serde(default)]
    pub username: Option<String>,
    /// Whether others may find this account by Kuzz ID or email. Relays
    /// without the setting omit it; their accounts are findable.
    #[serde(default)]
    pub discoverable: Option<bool>,
    /// The app language saved on the account (`ko`, `zh-Hant`, …), for text
    /// the server writes. Relays without the setting omit it.
    #[serde(default)]
    pub locale: Option<String>,
}

/// One person from `GET /auth/people/lookup` or `/auth/people/shared`.
/// Never carries an email.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub(crate) struct PersonInfo {
    pub pubkey: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Deserialize)]
struct SharedPeople {
    people: Vec<PersonInfo>,
}

/// One row of `GET /auth/blocks`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub(crate) struct BlockedPerson {
    pub pubkey: String,
    /// RFC 3339.
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Deserialize)]
struct BlockedList {
    blocked: Vec<BlockedPerson>,
}

/// Most people `GET /auth/people/shared` returns in one call.
pub(crate) const SHARED_PEOPLE_MAX: u32 = 50;

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
    if body.as_ref().and_then(|b| b.error.as_ref()).is_none() {
        eprintln!("kuzz-desktop: auth API returned {status} without an error message");
    }
    ApiError {
        status: Some(status),
        code: body.as_ref().and_then(|b| b.code.clone()),
        message: body
            .and_then(|b| b.error)
            .unwrap_or_else(|| "Something went wrong. Try again.".to_string()),
    }
}

async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
    let (client, request) = request.timeout(AUTH_REQUEST_TIMEOUT).build_split();
    let request = request.map_err(|e| ApiError::network(&e))?;
    let origin = crate::device_key::origin_of(request.url());
    let proof = proof_parts(&request);
    let key = proof.as_ref().and_then(|_| crate::device_key::current());
    let retry = key.as_ref().and_then(|_| request.try_clone());
    let response = send_once(&client, request, key.as_ref(), &origin, proof.as_ref()).await?;
    let clock_moved = crate::device_key::note_server_time(&origin, response.headers());
    if response.status().is_success() {
        return Ok(response);
    }
    let error = error_from(response).await;
    // A proof stamped with a clock the relay just corrected: sign once more.
    let stale = error.code.as_deref() == Some("device_proof_stale");
    let Some(retry) = retry.filter(|_| stale && clock_moved) else {
        return Err(error);
    };
    let response = send_once(&client, retry, key.as_ref(), &origin, proof.as_ref()).await?;
    crate::device_key::note_server_time(&origin, response.headers());
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(error_from(response).await)
    }
}

/// Send `request`, adding this device's `X-Kuzz-Device-Proof` when there is
/// a key and the request needs one; otherwise exactly as built.
async fn send_once(
    client: &reqwest::Client,
    mut request: reqwest::Request,
    key: Option<&crate::device_key::DeviceKey>,
    origin: &str,
    proof: Option<&ProofParts>,
) -> Result<reqwest::Response, ApiError> {
    if let (Some(key), Some(parts)) = (key, proof) {
        let signed = crate::device_key::header_proof(
            key,
            origin,
            crate::device_key::ProofRequest {
                purpose: parts.purpose,
                binding: parts.binding.clone(),
                token: Some(parts.token.clone()),
                body: parts.body.clone(),
            },
        )
        .await;
        if let Some(value) = signed.and_then(|v| reqwest::header::HeaderValue::from_str(&v).ok()) {
            request
                .headers_mut()
                .insert(crate::device_key::PROOF_HEADER, value);
        }
    }
    client
        .execute(request)
        .await
        .map_err(|e| ApiError::network(&e))
}

/// What the device proof of an `/auth/*` request covers.
struct ProofParts {
    purpose: &'static str,
    binding: String,
    token: Zeroizing<String>,
    body: Vec<u8>,
}

/// The device proof an `/auth/*` request needs: purpose `auth` for a bearer
/// call, `refresh` for `/auth/refresh` (whose token is the refresh token in
/// the body), bound to `<METHOD> <URL>`, the token and the body. `None` for
/// calls without a token (sign-in, NIP-11). Pure.
fn proof_parts(request: &reqwest::Request) -> Option<ProofParts> {
    let url = request.url();
    if !url.path().starts_with("/auth/") {
        return None;
    }
    let body = request
        .body()
        .and_then(reqwest::Body::as_bytes)
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    let bearer = request
        .headers()
        .get(reqwest::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let (purpose, token) = match bearer {
        Some(token) => ("auth", Zeroizing::new(token.to_owned())),
        None if url.path() == "/auth/refresh" => {
            #[derive(Deserialize)]
            struct RefreshBody {
                refresh: SecretField,
            }
            let parsed: RefreshBody = serde_json::from_slice(&body).ok()?;
            ("refresh", parsed.refresh.0)
        }
        None => return None,
    };
    let mut target = url.clone();
    target.set_fragment(None);
    Some(ProofParts {
        purpose,
        binding: format!("{} {}", request.method().as_str(), target.as_str()),
        token,
        body,
    })
}

/// Learn the relay's clock (`x-kuzz-server-time`) before a sign-in proof,
/// at most once per origin and run (answered or not): an unauthenticated
/// `/auth/me` answers 401 with the header. Failures leave the local clock in
/// use; a stale proof then restarts the sign-in once.
pub(crate) async fn sync_server_time(client: &reqwest::Client, origin: &str) {
    if crate::device_key::has_server_time(origin) || !crate::device_key::first_time_ask(origin) {
        return;
    }
    let _ = send(client.get(format!("{origin}/auth/me"))).await;
}

/// Add the device key and its bind proof to a sign-in body. With no key, or
/// when the proof cannot be signed, the body stays as before.
async fn add_device_fields(
    client: &reqwest::Client,
    origin: &str,
    device: Option<&crate::device_key::DeviceKey>,
    binding: String,
    body: &mut serde_json::Value,
) {
    let Some(device) = device else {
        return;
    };
    sync_server_time(client, origin).await;
    let fields = crate::device_key::login_fields(device, origin, binding).await;
    if let (Some(fields), Some(body)) = (fields, body.as_object_mut()) {
        body.extend(fields);
    }
}

pub(super) async fn send_json<T: for<'de> Deserialize<'de>>(
    request: reqwest::RequestBuilder,
) -> Result<T, ApiError> {
    send(request).await?.json::<T>().await.map_err(|e| {
        // Drop the URL; the decode error never includes the body text.
        eprintln!(
            "kuzz-desktop: auth API returned a malformed response: {}",
            e.without_url()
        );
        ApiError {
            status: None,
            code: None,
            message: "Something went wrong. Try again.".into(),
        }
    })
}

fn bearer(access: &str) -> String {
    format!("Bearer {access}")
}

/// NIP-11 `kuzz_token_auth` descriptor: `Some(providers)` when the relay
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
        .get("kuzz_key_backup")
        .and_then(|v| v.get("version"))
        .and_then(serde_json::Value::as_u64)
        == Some(1);
    Ok((parse_token_auth_descriptor(&value), backup))
}

/// Parse the NIP-11 descriptor. Pure, for tests.
pub(crate) fn parse_token_auth_descriptor(nip11: &serde_json::Value) -> Option<Vec<String>> {
    if let Some(providers) = nip11
        .get("kuzz_key_backup")
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
    let descriptor = nip11.get("kuzz_token_auth")?;
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
    install_id: Option<&str>,
    device: Option<&crate::device_key::DeviceKey>,
) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("client", "desktop")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("device_name", device_name);
    if let Some(install_id) = install_id {
        query.append_pair("install_id", install_id);
    }
    if let Some(device) = device {
        query.append_pair("device_pubkey", &device.pubkey().to_hex());
    }
    let query = query.finish();
    format!("{origin}/auth/oidc/{provider}/start?{query}")
}

/// `POST /auth/oidc/complete`. `device`: the key named at `start`, bound
/// here with a proof over `login:<sha256(login code)>`.
pub(crate) async fn complete_login(
    client: &reqwest::Client,
    origin: &str,
    login_code: &str,
    verifier: &str,
    device: Option<&crate::device_key::DeviceKey>,
) -> Result<CompleteResponse, ApiError> {
    let mut body = serde_json::json!({ "login_code": login_code, "code_verifier": verifier });
    let binding = crate::device_key::hashed_binding("login", login_code);
    add_device_fields(client, origin, device, binding, &mut body).await;
    send_json(
        client
            .post(format!("{origin}/auth/oidc/complete"))
            .json(&body),
    )
    .await
}

/// `POST /auth/oidc/signup`: create the account the user confirmed and sign
/// in to it. `device`: bound with a proof over `signup:<sha256(ticket)>`.
pub(crate) async fn signup(
    client: &reqwest::Client,
    origin: &str,
    signup_token: &str,
    verifier: &str,
    device: Option<&crate::device_key::DeviceKey>,
) -> Result<LoginTokens, ApiError> {
    let mut body = serde_json::json!({ "signup_token": signup_token, "code_verifier": verifier });
    let binding = crate::device_key::hashed_binding("signup", signup_token);
    add_device_fields(client, origin, device, binding, &mut body).await;
    send_json(
        client
            .post(format!("{origin}/auth/oidc/signup"))
            .json(&body),
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

/// `POST /auth/agents/challenge`: a one-use agent registration challenge for
/// this account and device.
pub(crate) async fn agent_challenge(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
) -> Result<String, ApiError> {
    #[derive(Deserialize)]
    struct Challenge {
        challenge: String,
    }
    let challenge: Challenge = send_json(
        client
            .post(format!("{origin}/auth/agents/challenge"))
            .header("Authorization", bearer(access)),
    )
    .await?;
    Ok(challenge.challenge)
}

/// `POST /auth/agents/register`: host the agent on this device and give the
/// relay its encrypted copy. Returns `registered`, `unchanged` or `moved`.
pub(crate) async fn register_agent(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    body: &serde_json::Value,
) -> Result<String, ApiError> {
    #[derive(Deserialize)]
    struct Registered {
        result: String,
    }
    let registered: Registered = send_json(
        client
            .post(format!("{origin}/auth/agents/register"))
            .header("Authorization", bearer(access))
            .json(body),
    )
    .await?;
    Ok(registered.result)
}

/// One agent key copy of `GET /auth/agents/keys` (hex secret).
#[derive(Deserialize)]
pub(crate) struct AgentKeyCopy {
    pub agent_pubkey: String,
    pub secret_key: SecretField,
}

/// `GET /auth/agents/keys`: this device's agent key copies (needs a sign-in
/// from the last five minutes).
pub(crate) async fn agent_keys(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
) -> Result<Vec<AgentKeyCopy>, ApiError> {
    #[derive(Deserialize)]
    struct Copies {
        keys: Vec<AgentKeyCopy>,
    }
    let copies: Copies = send_json(
        client
            .get(format!("{origin}/auth/agents/keys"))
            .header("Authorization", bearer(access)),
    )
    .await?;
    Ok(copies.keys)
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

/// A person's pubkey as the path segment of `/auth/blocks/{pubkey}`:
/// 64 hex characters, lowercased.
pub(crate) fn person_pubkey(pubkey: &str) -> Result<String, ApiError> {
    if pubkey.len() != 64 || !pubkey.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError {
            status: None,
            code: None,
            message: "invalid person".into(),
        });
    }
    Ok(pubkey.to_ascii_lowercase())
}

/// `GET /auth/blocks`: the people this account blocked.
pub(crate) async fn list_blocks(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
) -> Result<Vec<BlockedPerson>, ApiError> {
    let list: BlockedList = send_json(
        client
            .get(format!("{origin}/auth/blocks"))
            .header("Authorization", bearer(access)),
    )
    .await?;
    Ok(list.blocked)
}

/// `PUT /auth/blocks/{pubkey}` (`blocked`) or `DELETE` (unblock). Both are
/// idempotent.
pub(crate) async fn set_block(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    pubkey: &str,
    blocked: bool,
) -> Result<(), ApiError> {
    let pubkey = person_pubkey(pubkey)?;
    let method = if blocked {
        reqwest::Method::PUT
    } else {
        reqwest::Method::DELETE
    };
    call(
        client,
        method,
        format!("{origin}/auth/blocks/{pubkey}"),
        access,
        None,
    )
    .await
}

/// `GET /auth/people/lookup?q=`: the one person whose Kuzz ID or email is
/// exactly `query`, or `None` (404: no such person, or not findable).
pub(crate) async fn lookup_person(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    query: &str,
) -> Result<Option<PersonInfo>, ApiError> {
    let result = send_json(
        client
            .get(format!("{origin}/auth/people/lookup"))
            .query(&[("q", query)])
            .header("Authorization", bearer(access)),
    )
    .await;
    match result {
        Ok(person) => Ok(Some(person)),
        Err(error) if error.status == Some(404) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `GET /auth/people/shared?q=&limit=`: people in my channels whose name
/// starts with `query`.
pub(crate) async fn shared_people(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    query: &str,
    limit: u32,
) -> Result<Vec<PersonInfo>, ApiError> {
    let limit = limit.clamp(1, SHARED_PEOPLE_MAX).to_string();
    let found: SharedPeople = send_json(
        client
            .get(format!("{origin}/auth/people/shared"))
            .query(&[("q", query), ("limit", limit.as_str())])
            .header("Authorization", bearer(access)),
    )
    .await?;
    Ok(found.people)
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
        let on = serde_json::json!({"kuzz_token_auth": {"version": 1, "bearer": true, "oidc_providers": ["google"]}});
        assert_eq!(
            parse_token_auth_descriptor(&on),
            Some(vec!["google".to_string()])
        );
        let off = serde_json::json!({"kuzz_token_auth": {"bearer": false}});
        assert_eq!(parse_token_auth_descriptor(&off), None);
        assert_eq!(parse_token_auth_descriptor(&serde_json::json!({})), None);
    }

    #[test]
    fn complete_is_either_a_session_or_a_new_account_ticket() {
        let session: CompleteResponse = serde_json::from_value(serde_json::json!({
            "principal_id": "ab", "identity_mode": "key_backup", "signing_pubkey": null,
            "device_id": "d", "access": "a", "refresh": "r", "expires_in": 3600,
            "email": "j@test",
        }))
        .unwrap();
        assert!(
            matches!(session, CompleteResponse::Tokens(t) if t.email.as_deref() == Some("j@test"))
        );
        let ticket: CompleteResponse = serde_json::from_value(serde_json::json!({
            "needs_signup": true, "signup_token": "t", "email": "new@test",
            "display_name": "New", "avatar_url": null,
        }))
        .unwrap();
        let CompleteResponse::NeedsSignup(ticket) = ticket else {
            panic!("expected a signup ticket");
        };
        assert_eq!(&*ticket.signup_token, "t");
        assert_eq!(ticket.email.as_deref(), Some("new@test"));
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
            Some("5f0c6b1e-2a4d-4c8e-9b7a-1d2e3f4a5b6c"),
            None,
        );
        assert!(!url.contains("device_pubkey"));
        assert!(url.starts_with("http://127.0.0.1:3000/auth/oidc/google/start?"));
        assert!(url.contains("state=st"));
        assert!(url.contains("code_challenge=ch"));
        assert!(url.contains("client=desktop"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A5555%2Fcb"));
        assert!(url.contains("device_name=My+Mac"));
        assert!(url.contains("install_id=5f0c6b1e-2a4d-4c8e-9b7a-1d2e3f4a5b6c"));
    }

    #[test]
    fn start_url_names_the_device_key() {
        let dir = tempfile::tempdir().unwrap();
        let key =
            crate::device_key::DeviceKey::open_or_create_file(&dir.path().join("k.key")).unwrap();
        let url = start_url(
            "http://r",
            "google",
            "s",
            "c",
            "http://cb",
            "d",
            None,
            Some(&key),
        );
        assert!(url.contains(&format!("device_pubkey={}", key.pubkey().to_hex())));
    }

    #[test]
    fn device_proofs_cover_bearer_and_refresh_calls_only() {
        let client = reqwest::Client::new();
        let me = client
            .get("https://relay.example/auth/me?x=1#frag")
            .header("Authorization", "Bearer acc")
            .build()
            .unwrap();
        let parts = proof_parts(&me).unwrap();
        assert_eq!(parts.purpose, "auth");
        assert_eq!(parts.binding, "GET https://relay.example/auth/me?x=1");
        assert_eq!(parts.token.as_str(), "acc");
        assert!(parts.body.is_empty());

        let refresh = client
            .post("https://relay.example/auth/refresh")
            .json(&serde_json::json!({ "refresh": "rt" }))
            .build()
            .unwrap();
        let parts = proof_parts(&refresh).unwrap();
        assert_eq!(parts.purpose, "refresh");
        assert_eq!(parts.token.as_str(), "rt");
        assert_eq!(parts.body, br#"{"refresh":"rt"}"#);

        for request in [
            client
                .post("https://relay.example/auth/oidc/complete")
                .json(&serde_json::json!({})),
            client
                .get("https://relay.example/")
                .header("Authorization", "Bearer acc"),
            client
                .get("https://relay.example/api/groups")
                .header("Authorization", "Bearer acc"),
        ] {
            assert!(proof_parts(&request.build().unwrap()).is_none());
        }
    }

    #[test]
    fn me_reads_kuzz_id_and_findability() {
        let me: MeInfo = serde_json::from_value(serde_json::json!({
            "principal_id": "ab", "username": "jiho_1", "discoverable": false,
        }))
        .unwrap();
        assert_eq!(me.username.as_deref(), Some("jiho_1"));
        assert_eq!(me.discoverable, Some(false));
        let older: MeInfo =
            serde_json::from_value(serde_json::json!({ "principal_id": "ab" })).unwrap();
        assert_eq!((older.username, older.discoverable), (None, None));
    }

    #[test]
    fn people_and_blocks_parse_the_contract_shapes() {
        let key = "a".repeat(64);
        let shared: SharedPeople = serde_json::from_value(serde_json::json!({
            "people": [{ "pubkey": key, "display_name": "Ann", "avatar_url": null, "username": "ann" }],
        }))
        .unwrap();
        assert_eq!(shared.people[0].display_name.as_deref(), Some("Ann"));
        let ui = serde_json::to_value(&shared.people[0]).unwrap();
        assert_eq!(ui["displayName"], "Ann");
        assert!(ui.get("email").is_none());
        let blocks: BlockedList = serde_json::from_value(serde_json::json!({
            "blocked": [{ "pubkey": key, "created_at": "2026-10-18T00:00:00Z" }],
        }))
        .unwrap();
        assert_eq!(blocks.blocked[0].pubkey, key);
    }

    #[test]
    fn block_paths_take_only_a_hex_pubkey() {
        assert_eq!(person_pubkey(&"AB".repeat(32)).unwrap(), "ab".repeat(32));
        assert!(person_pubkey("../me").is_err());
        assert!(person_pubkey(&"g".repeat(64)).is_err());
    }

    #[test]
    fn bot_paths_reject_non_hex_ids() {
        assert!(bot_path("http://h", "../x", "").is_err());
        assert!(bot_path("http://h", &"a".repeat(64), "/token").is_ok());
    }
}
