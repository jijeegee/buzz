//! Kuzz's own "Sign in with ChatGPT" login (OpenAI's open-source app flow).
//!
//! The first sign-in registers a per-user client through
//! `client_id=dynamic_agent_client`; later sign-ins reuse the issued client.
//! Tokens are scoped to `https://api.openai.com/v1` and are never sent to the
//! ChatGPT `backend-api`. One credential file is shared by the desktop (which
//! signs in and out) and every Kuzz Agent process (which refreshes). Refresh
//! tokens rotate, so refreshes are serialized by a cross-process file lock.
//!
//! Reference: <https://developers.openai.com/siwc/token-sharing-open-source>.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use super::{
    acquire_auth_lock, pkce_pair, random_state, write_private_cache, AbortOnDrop, BrowserOpener,
    TokenSource,
};
use crate::{auth_http::read_oauth_json, types::AgentError};

/// Name shown on the OpenAI consent screen and in ChatGPT settings.
pub const AGENT_NAME: &str = "Kuzz";
/// ChatGPT page where users review and limit this app's plan usage.
pub const MANAGE_USAGE_URL: &str = "https://chatgpt.com/settings/usage";
/// The only resource and API base these tokens may be used against.
pub const API_BASE: &str = "https://api.openai.com/v1";

const AUTH_BASE: &str = "https://auth.openai.com";
const ISSUER: &str = "https://auth.openai.com";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const CALLBACK_PATH: &str = "/auth/callback";
/// Preferred loopback port from OpenAI's examples; any port works after it.
const PREFERRED_PORT: u16 = 1455;
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_TIMEOUT: Duration = Duration::from_secs(60);
/// Refresh this long before expiry so a long request never starts stale.
const REFRESH_LEEWAY_SECS: u64 = 300;
const MAX_CREDENTIAL_BYTES: u64 = 256 * 1024;

fn auth_error(message: &str) -> AgentError {
    AgentError::LlmAuth(format!("ChatGPT plan: {message}"))
}

fn sign_in_required() -> AgentError {
    auth_error("sign in with ChatGPT in Settings › Models › Providers")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Saved registration plus the current token set. The registration (client,
/// identity, host) survives sign-out so a later sign-in reuses the client.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct Credentials {
    client_id: String,
    subject: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: u64,
    #[serde(default)]
    scopes: Vec<String>,
}

impl Credentials {
    fn plan_enabled(&self) -> bool {
        self.scopes.iter().any(|s| s == PLAN_SCOPE)
    }

    fn signed_in(&self) -> bool {
        self.refresh_token.as_deref().is_some_and(|t| !t.is_empty())
    }

    fn fresh_access(&self) -> Option<&str> {
        self.access_token
            .as_deref()
            .filter(|t| !t.is_empty() && self.expires_at > now_secs() + REFRESH_LEEWAY_SECS)
    }
}

/// What the desktop shows. Never contains a token.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptStatus {
    pub signed_in: bool,
    /// Signed in, but the user declined ChatGPT plan use on the consent screen.
    pub plan_enabled: bool,
    pub email: Option<String>,
}

/// Result of a completed sign-in.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptSignIn {
    pub status: ChatGptStatus,
    /// First sign-in on this computer: show the one-time plan notice.
    pub first_sign_in: bool,
}

/// Endpoints, overridable only by tests.
#[derive(Debug, Clone)]
struct Endpoints {
    auth_base: String,
    api_base: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            auth_base: AUTH_BASE.into(),
            api_base: API_BASE.into(),
        }
    }
}

/// The ChatGPT plan login stored under `<config>/kuzz-agent/chatgpt/`.
pub struct ChatGptLogin {
    dir: PathBuf,
    http: Client,
    endpoints: Endpoints,
}

impl ChatGptLogin {
    /// `config_root` is the same root `KUZZ_AGENT_CONFIG_DIR` names for agent
    /// processes (demo builds); `None` uses that env var or `~/.config`.
    pub fn new(config_root: Option<PathBuf>) -> Result<Arc<Self>, AgentError> {
        let root = match config_root
            .or_else(|| std::env::var_os("KUZZ_AGENT_CONFIG_DIR").map(PathBuf::from))
        {
            Some(root) => root.join("kuzz-agent"),
            None => dirs::home_dir()
                .ok_or_else(|| auth_error("cannot locate the home directory"))?
                .join(".config")
                .join("kuzz-agent"),
        };
        Self::at(root.join("chatgpt"), Endpoints::default())
    }

    fn at(dir: PathBuf, endpoints: Endpoints) -> Result<Arc<Self>, AgentError> {
        let http = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| auth_error("HTTP setup failed"))?;
        Ok(Arc::new(Self {
            dir,
            http,
            endpoints,
        }))
    }

    pub fn api_base(&self) -> &str {
        &self.endpoints.api_base
    }

    fn credentials_path(&self) -> PathBuf {
        self.dir.join("credentials.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.dir.join("credentials.lock")
    }

    /// Marks that the one-time "You're using your ChatGPT plan" notice ran.
    fn notice_path(&self) -> PathBuf {
        self.dir.join("plan-notice-shown")
    }

    /// Drop the whole credential record (tokens and registration).
    fn forget(&self) -> Result<(), AgentError> {
        match fs::remove_file(self.credentials_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(auth_error("could not clear the sign-in")),
        }
    }

    fn host_id_path(&self) -> PathBuf {
        self.dir.join("host-id")
    }

    fn read(&self) -> Option<Credentials> {
        let path = self.credentials_path();
        let meta = fs::symlink_metadata(&path).ok()?;
        if !meta.file_type().is_file() || meta.len() > MAX_CREDENTIAL_BYTES {
            return None;
        }
        #[cfg(unix)]
        let bytes = super::read_private_cache(&path).ok()?;
        // Windows: the file lives in the user's profile and inherits its
        // per-user ACL, the same protection Codex gives `~/.codex/auth.json`.
        #[cfg(not(unix))]
        let bytes = fs::read(&path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn write(&self, creds: &Credentials) -> Result<(), AgentError> {
        let body = serde_json::to_vec_pretty(creds)
            .map_err(|_| auth_error("could not save the sign-in"))?;
        write_private_cache(&self.credentials_path(), &body)
            .map_err(|_| auth_error("could not save the sign-in"))
    }

    /// Stable opaque host identifier, created once before the first sign-in.
    fn host_id(&self) -> Result<String, AgentError> {
        let path = self.host_id_path();
        if let Ok(existing) = fs::read_to_string(&path) {
            let existing = existing.trim();
            if existing.starts_with("urn:uuid:") {
                return Ok(existing.to_owned());
            }
        }
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|_| auth_error("could not create a host id"))?;
        b[6] = (b[6] & 0x0f) | 0x40;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h = hex::encode(b);
        let id = format!(
            "urn:uuid:{}-{}-{}-{}-{}",
            &h[0..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..32]
        );
        fs::create_dir_all(&self.dir).map_err(|_| auth_error("could not save the host id"))?;
        fs::write(&path, &id).map_err(|_| auth_error("could not save the host id"))?;
        Ok(id)
    }

    /// Local status for the desktop. No network, no writes.
    pub fn status(&self) -> ChatGptStatus {
        match self.read() {
            Some(c) if c.signed_in() => ChatGptStatus {
                signed_in: true,
                plan_enabled: c.plan_enabled(),
                email: c.email,
            },
            _ => ChatGptStatus {
                signed_in: false,
                plan_enabled: false,
                email: None,
            },
        }
    }

    /// Readiness check: signed in with plan use allowed. No network.
    pub fn check_login(&self) -> Result<(), AgentError> {
        let status = self.status();
        if !status.signed_in {
            return Err(sign_in_required());
        }
        if !status.plan_enabled {
            return Err(auth_error(
                "ChatGPT plan use was not allowed; sign in again and allow it",
            ));
        }
        Ok(())
    }

    /// Run the browser sign-in and save the result.
    pub async fn sign_in(&self, opener: &dyn BrowserOpener) -> Result<ChatGptSignIn, AgentError> {
        let host_id = self.host_id()?;
        // Re-authorize the saved client only while signed in (to re-request
        // plan consent). After sign-out or a dead session a fresh registration
        // lets the user pick any ChatGPT account.
        let saved = self.read().filter(Credentials::signed_in);
        let first_sign_in = !self.notice_path().exists();
        let attempt = self.authorize(opener, &host_id, saved.as_ref()).await?;
        let token = self
            .exchange(
                &attempt.client_id,
                &attempt.code,
                &attempt.verifier,
                &attempt.redirect_uri,
            )
            .await?;
        let id_token = token["id_token"]
            .as_str()
            .ok_or_else(|| auth_error("sign-in returned no identity"))?;
        let identity = self
            .validate_id_token(id_token, &attempt.client_id, &attempt.nonce)
            .await?;
        if let Some(saved) = &saved {
            if saved.client_id == attempt.client_id && saved.subject != identity.sub {
                return Err(auth_error(
                    "a different ChatGPT account signed in; sign in with the original account",
                ));
            }
        }
        let mut creds = Credentials {
            client_id: attempt.client_id,
            subject: identity.sub,
            email: identity.email,
            id_token: Some(id_token.to_owned()),
            ..Credentials::default()
        };
        apply_token_response(&mut creds, &token, None)?;
        let _lock = acquire_auth_lock(&self.lock_path(), Instant::now() + LOCK_TIMEOUT)
            .await
            .map_err(|_| auth_error("another sign-in is in progress"))?;
        self.write(&creds)?;
        if creds.plan_enabled() {
            let _ = fs::write(self.notice_path(), b"");
        }
        Ok(ChatGptSignIn {
            status: ChatGptStatus {
                signed_in: true,
                plan_enabled: creds.plan_enabled(),
                email: creds.email,
            },
            first_sign_in,
        })
    }

    /// Revoke the renewable session and clear tokens. Returns whether OpenAI
    /// confirmed the revocation; tokens are cleared locally either way.
    pub async fn sign_out(&self) -> Result<bool, AgentError> {
        let _lock = acquire_auth_lock(&self.lock_path(), Instant::now() + LOCK_TIMEOUT)
            .await
            .map_err(|_| auth_error("the sign-in is busy; try again"))?;
        let Some(creds) = self.read() else {
            return Ok(true);
        };
        let mut revoked = true;
        if let Some(refresh) = creds.refresh_token.clone() {
            revoked = self.revoke(&creds.client_id, &refresh).await;
        }
        self.forget()?;
        Ok(revoked)
    }

    async fn revoke(&self, client_id: &str, refresh: &str) -> bool {
        let endpoint = match self.discovery().await {
            Ok(d) => d["revocation_endpoint"].as_str().map(str::to_owned),
            Err(_) => None,
        };
        let Some(endpoint) = endpoint else {
            return false;
        };
        for attempt in 0..3u32 {
            let sent = self
                .http
                .post(&endpoint)
                .form(&[
                    ("token", refresh),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", client_id),
                ])
                .send()
                .await;
            match sent {
                Ok(r) if r.status().is_success() => return true,
                Ok(r) if !r.status().is_server_error() => return false,
                _ => tokio::time::sleep(Duration::from_millis(500 << attempt)).await,
            }
        }
        false
    }

    async fn discovery(&self) -> Result<Value, AgentError> {
        let url = format!(
            "{}/.well-known/openid-configuration",
            self.endpoints.auth_base
        );
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|_| auth_error("could not reach OpenAI sign-in"))?;
        if !response.status().is_success() {
            return Err(auth_error("OpenAI sign-in is unavailable"));
        }
        read_oauth_json(response)
            .await
            .map_err(|_| auth_error("OpenAI sign-in returned an invalid response"))
    }

    async fn authorize(
        &self,
        opener: &dyn BrowserOpener,
        host_id: &str,
        saved: Option<&Credentials>,
    ) -> Result<Authorization, AgentError> {
        use axum::{extract::Query, response::Html, routing::get, Router};
        use tokio::sync::oneshot;

        let (verifier, challenge) = pkce_pair()?;
        let state = random_state()?;
        let nonce = random_state()?;

        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).await {
            Ok(l) => l,
            Err(_) => tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|_| auth_error("could not start the sign-in listener"))?,
        };
        let port = listener
            .local_addr()
            .map_err(|_| auth_error("could not start the sign-in listener"))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

        type Callback = HashMap<String, String>;
        let (tx, rx) = oneshot::channel::<Callback>();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let expected = state.clone();
        let app = Router::new().route(
            CALLBACK_PATH,
            get(move |Query(params): Query<Callback>| {
                let tx = Arc::clone(&tx);
                let expected = expected.clone();
                async move {
                    // Ignore stray requests that do not carry this attempt's state.
                    if params.get("state") != Some(&expected) {
                        return Html(page(
                            "This sign-in link has expired. Return to Kuzz and try again.",
                        ));
                    }
                    let ok = params.contains_key("code");
                    if let Some(sender) = tx.lock().await.take() {
                        let _ = sender.send(params);
                    }
                    Html(page(if ok {
                        "Signed in. You can close this window and return to Kuzz."
                    } else {
                        "Sign-in was not completed. You can close this window."
                    }))
                }
            }),
        );
        let _server = AbortOnDrop(tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        }));

        let saved_client = saved.map(|c| c.client_id.as_str());
        let mut query: Vec<(&str, &str)> = vec![
            ("client_id", saved_client.unwrap_or(DYNAMIC_CLIENT)),
            ("ext_agent_host_id", host_id),
            ("response_type", "code"),
            ("redirect_uri", &redirect_uri),
            ("scope", SCOPES),
            ("resource", API_BASE),
            ("state", &state),
            ("nonce", &nonce),
            ("code_challenge_method", "S256"),
            ("code_challenge", &challenge),
        ];
        match saved {
            None => query.push(("agent_name_hint", AGENT_NAME)),
            Some(saved) => {
                // After sign-out the ID token is kept only as an account hint;
                // a plan-disabled account re-requests consent.
                if let Some(email) = saved.email.as_deref() {
                    query.push(("login_hint", email));
                }
                if saved.signed_in() && !saved.plan_enabled() {
                    query.push(("prompt", "consent"));
                }
            }
        }
        let url = format!(
            "{}/api/accounts/authorize?{}",
            self.endpoints.auth_base,
            query
                .iter()
                .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        );
        opener
            .open(&url)
            .map_err(|_| auth_error("could not open the browser"))?;

        let params = tokio::time::timeout(SIGN_IN_TIMEOUT, rx)
            .await
            .map_err(|_| auth_error("sign-in timed out"))?
            .map_err(|_| auth_error("sign-in was interrupted"))?;
        if params.get("error").map(String::as_str) == Some("access_denied") {
            return Err(auth_error("sign-in was cancelled"));
        }
        let code = params
            .get("code")
            .filter(|c| !c.is_empty())
            .cloned()
            .ok_or_else(|| auth_error("sign-in was not completed"))?;
        let returned = params.get("client_id").filter(|c| !c.is_empty());
        let client_id = match (saved_client, returned) {
            (Some(saved), Some(returned)) if saved != returned => {
                return Err(auth_error("sign-in returned a different app registration"));
            }
            (Some(saved), _) => saved.to_owned(),
            (None, Some(returned)) if returned != DYNAMIC_CLIENT => returned.clone(),
            (None, _) => return Err(auth_error("app registration was not completed")),
        };
        Ok(Authorization {
            client_id,
            code,
            verifier,
            nonce,
            redirect_uri,
        })
    }

    async fn exchange(
        &self,
        client_id: &str,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Value, AgentError> {
        let response = self
            .http
            .post(format!(
                "{}/api/accounts/oauth/token",
                self.endpoints.auth_base
            ))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", redirect_uri),
                ("resource", API_BASE),
            ])
            .send()
            .await
            .map_err(|_| auth_error("could not reach OpenAI sign-in"))?;
        let ok = response.status().is_success();
        let body = read_oauth_json(response).await.unwrap_or(Value::Null);
        if !ok {
            return Err(auth_error(if body["error"] == "invalid_grant" {
                "the sign-in code expired; try again"
            } else {
                "OpenAI rejected the sign-in"
            }));
        }
        Ok(body)
    }

    async fn validate_id_token(
        &self,
        id_token: &str,
        client_id: &str,
        nonce: &str,
    ) -> Result<Identity, AgentError> {
        use jsonwebtoken::{decode, decode_header, jwk::JwkSet, DecodingKey, Validation};
        let invalid = || auth_error("sign-in returned an invalid identity");
        let header = decode_header(id_token).map_err(|_| invalid())?;
        let jwks_uri = self.discovery().await?["jwks_uri"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(invalid)?;
        let response = self
            .http
            .get(jwks_uri)
            .send()
            .await
            .map_err(|_| auth_error("could not reach OpenAI sign-in"))?;
        let jwks: JwkSet =
            serde_json::from_value(read_oauth_json(response).await.map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
        let jwk = match header.kid.as_deref() {
            Some(kid) => jwks.find(kid),
            None => jwks.keys.first(),
        }
        .ok_or_else(invalid)?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| invalid())?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&[client_id]);
        let claims = decode::<Identity>(id_token, &key, &validation)
            .map_err(|_| invalid())?
            .claims;
        check_identity(claims, nonce)
    }

    /// Current credentials, refreshing under the cross-process lock when the
    /// access token is near expiry or was just rejected.
    async fn access_token(&self, rejected: Option<&str>) -> Result<String, AgentError> {
        // Read the shared file on every call (it is small): another process
        // may have refreshed, and a desktop sign-out must take effect at once.
        let creds = self.read().ok_or_else(sign_in_required)?;
        if !creds.signed_in() {
            return Err(sign_in_required());
        }
        if !creds.plan_enabled() {
            return Err(auth_error(
                "ChatGPT plan use was not allowed; sign in again and allow it",
            ));
        }
        if let Some(token) = creds.fresh_access().filter(|t| Some(*t) != rejected) {
            let token = token.to_owned();
            return Ok(token);
        }
        let _lock = acquire_auth_lock(&self.lock_path(), Instant::now() + LOCK_TIMEOUT)
            .await
            .map_err(|_| auth_error("timed out waiting to renew the sign-in"))?;
        let mut creds = self.read().ok_or_else(sign_in_required)?;
        if let Some(token) = creds.fresh_access().filter(|t| Some(*t) != rejected) {
            let token = token.to_owned();
            return Ok(token);
        }
        if !creds.plan_enabled() && creds.signed_in() {
            return Err(auth_error(
                "ChatGPT plan use was not allowed; sign in again and allow it",
            ));
        }
        let refresh = creds.refresh_token.clone().ok_or_else(sign_in_required)?;
        match self.refresh(&creds.client_id, &refresh).await? {
            Refresh::Token(token) => {
                apply_token_response(&mut creds, &token, Some(&refresh))?;
                self.write(&creds)?;
                let access = creds.access_token.clone().ok_or_else(sign_in_required)?;
                Ok(access)
            }
            Refresh::Unusable => {
                // Dead session or client: the next sign-in registers afresh.
                self.forget()?;
                Err(auth_error(
                    "the ChatGPT sign-in expired or was disconnected; sign in again",
                ))
            }
        }
    }

    async fn refresh(&self, client_id: &str, refresh: &str) -> Result<Refresh, AgentError> {
        let response = self
            .http
            .post(format!(
                "{}/api/accounts/oauth/token",
                self.endpoints.auth_base
            ))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("refresh_token", refresh),
                ("resource", API_BASE),
            ])
            .send()
            .await
            .map_err(|_| {
                AgentError::Llm("ChatGPT plan: could not reach OpenAI to renew the sign-in".into())
            })?;
        let status = response.status();
        let body = read_oauth_json(response).await.unwrap_or(Value::Null);
        if status.is_success() {
            return Ok(Refresh::Token(body));
        }
        let code = body["error"]["code"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("");
        if status.is_client_error() && is_unusable_refresh(code) {
            return Ok(Refresh::Unusable);
        }
        Err(AgentError::Llm(format!(
            "ChatGPT plan: renewing the sign-in failed (HTTP {})",
            status.as_u16()
        )))
    }
}

struct Authorization {
    client_id: String,
    code: String,
    verifier: String,
    nonce: String,
    redirect_uri: String,
}

enum Refresh {
    Token(Value),
    Unusable,
}

#[derive(Debug, Deserialize)]
struct Identity {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

fn check_identity(identity: Identity, nonce: &str) -> Result<Identity, AgentError> {
    if identity.nonce.as_deref() != Some(nonce) || identity.sub.is_empty() {
        return Err(auth_error("sign-in returned an invalid identity"));
    }
    Ok(identity)
}

fn is_unusable_refresh(code: &str) -> bool {
    matches!(
        code,
        "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused"
            // The registered client itself is gone: register again.
            | "invalid_client"
    )
}

/// Replace access token, expiry, scopes and the rotating refresh token together.
fn apply_token_response(
    creds: &mut Credentials,
    token: &Value,
    previous_refresh: Option<&str>,
) -> Result<(), AgentError> {
    let access = token["access_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| auth_error("OpenAI returned no access token"))?;
    let refresh = token["refresh_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .or(previous_refresh)
        .ok_or_else(|| auth_error("OpenAI returned no refresh token"))?;
    let expires_in = token["expires_in"].as_u64().unwrap_or(3600);
    if let Some(scope) = token["scope"].as_str() {
        creds.scopes = scope.split_whitespace().map(str::to_owned).collect();
    }
    if let Some(id_token) = token["id_token"].as_str().filter(|t| !t.is_empty()) {
        creds.id_token = Some(id_token.to_owned());
    }
    creds.access_token = Some(access.to_owned());
    creds.refresh_token = Some(refresh.to_owned());
    creds.expires_at = now_secs().saturating_add(expires_in);
    Ok(())
}

fn page(message: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>Kuzz</title>\
         <body style=\"font-family:system-ui;margin:4rem;text-align:center\">\
         <h2>Kuzz</h2><p>{message}</p></body>"
    )
}

#[async_trait]
impl TokenSource for ChatGptLogin {
    async fn bearer(&self) -> Result<String, AgentError> {
        self.access_token(None).await
    }

    async fn refresh_now(&self, rejected: &str) -> Result<String, AgentError> {
        self.access_token(Some(rejected)).await
    }
}

/// Test helpers that seed a credential file without a browser.
#[cfg(test)]
impl ChatGptLogin {
    fn for_test(dir: &std::path::Path, auth_base: &str) -> Arc<Self> {
        Self::at(
            dir.to_path_buf(),
            Endpoints {
                auth_base: auth_base.into(),
                api_base: API_BASE.into(),
            },
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Form, Json, Router};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn seeded(expires_at: u64) -> Credentials {
        Credentials {
            client_id: "oaiapp_test".into(),
            subject: "user-1".into(),
            email: Some("a@example.com".into()),
            id_token: Some("id".into()),
            access_token: Some("access-old".into()),
            refresh_token: Some("refresh-old".into()),
            expires_at,
            scopes: SCOPES.split(' ').map(str::to_owned).collect(),
        }
    }

    async fn token_server(response: Value, status: u16) -> (String, Arc<AtomicUsize>, AbortOnDrop) {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        let app = Router::new().route(
            "/api/accounts/oauth/token",
            post(move |Form(form): Form<HashMap<String, String>>| {
                let counter = Arc::clone(&counter);
                let response = response.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(form["grant_type"], "refresh_token");
                    assert_eq!(form["client_id"], "oaiapp_test");
                    assert_eq!(form["resource"], API_BASE);
                    assert!(!form.contains_key("scope"));
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        Json(response),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = AbortOnDrop(tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        }));
        (base, hits, server)
    }

    #[tokio::test]
    async fn fresh_token_is_used_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let login = ChatGptLogin::for_test(dir.path(), "http://127.0.0.1:9");
        login.write(&seeded(now_secs() + 3600)).unwrap();
        assert_eq!(login.bearer().await.unwrap(), "access-old");
        assert!(login.check_login().is_ok());
        assert_eq!(
            login.status(),
            ChatGptStatus {
                signed_in: true,
                plan_enabled: true,
                email: Some("a@example.com".into())
            }
        );
    }

    #[tokio::test]
    async fn expiring_token_refreshes_and_rotates_refresh_token() {
        let dir = tempfile::tempdir().unwrap();
        let (base, hits, _server) = token_server(
            json!({"access_token":"access-new","refresh_token":"refresh-new","expires_in":3600,"scope":SCOPES}),
            200,
        )
        .await;
        let login = ChatGptLogin::for_test(dir.path(), &base);
        login.write(&seeded(now_secs() + 10)).unwrap();
        assert_eq!(login.bearer().await.unwrap(), "access-new");
        let saved = login.read().unwrap();
        assert_eq!(saved.refresh_token.as_deref(), Some("refresh-new"));
        assert!(saved.expires_at > now_secs() + 3000);
        // A second process sees the replacement instead of refreshing again.
        let other = ChatGptLogin::for_test(dir.path(), &base);
        assert_eq!(other.bearer().await.unwrap(), "access-new");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rejected_token_forces_one_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let (base, hits, _server) = token_server(
            json!({"access_token":"access-new","refresh_token":"refresh-new","expires_in":3600}),
            200,
        )
        .await;
        let login = ChatGptLogin::for_test(dir.path(), &base);
        login.write(&seeded(now_secs() + 3600)).unwrap();
        assert_eq!(login.refresh_now("access-old").await.unwrap(), "access-new");
        // Scopes are kept when a refresh omits them.
        assert!(login.read().unwrap().plan_enabled());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_refreshes_rotate_once() {
        let dir = tempfile::tempdir().unwrap();
        let (base, hits, _server) = token_server(
            json!({"access_token":"access-new","refresh_token":"refresh-new","expires_in":3600}),
            200,
        )
        .await;
        let a = ChatGptLogin::for_test(dir.path(), &base);
        a.write(&seeded(now_secs())).unwrap();
        let b = ChatGptLogin::for_test(dir.path(), &base);
        let (x, y) = tokio::join!(a.bearer(), b.bearer());
        assert_eq!(x.unwrap(), "access-new");
        assert_eq!(y.unwrap(), "access-new");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dead_refresh_token_forgets_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let (base, _hits, _server) = token_server(json!({"error":"invalid_grant"}), 400).await;
        let login = ChatGptLogin::for_test(dir.path(), &base);
        login.write(&seeded(now_secs())).unwrap();
        let err = login.bearer().await.unwrap_err();
        assert!(matches!(err, AgentError::LlmAuth(_)), "{err:?}");
        // The next sign-in registers afresh, so any account can be chosen.
        assert!(login.read().is_none());
        assert!(!login.status().signed_in);
    }

    #[tokio::test]
    async fn server_error_on_refresh_keeps_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let (base, _hits, _server) = token_server(json!({"detail":"busy"}), 503).await;
        let login = ChatGptLogin::for_test(dir.path(), &base);
        login.write(&seeded(now_secs())).unwrap();
        assert!(matches!(login.bearer().await, Err(AgentError::Llm(_))));
        assert!(login.read().unwrap().signed_in());
    }

    #[tokio::test]
    async fn missing_plan_scope_is_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let login = ChatGptLogin::for_test(dir.path(), "http://127.0.0.1:9");
        let mut creds = seeded(now_secs() + 3600);
        creds.scopes.retain(|s| s != PLAN_SCOPE);
        login.write(&creds).unwrap();
        assert!(login.check_login().is_err());
        assert!(!login.status().plan_enabled);
        let empty = ChatGptLogin::for_test(&dir.path().join("none"), "http://127.0.0.1:9");
        assert!(empty.check_login().is_err());
        assert!(matches!(empty.bearer().await, Err(AgentError::LlmAuth(_))));
    }

    #[tokio::test]
    async fn sign_out_without_revocation_endpoint_still_clears_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let login = ChatGptLogin::for_test(dir.path(), "http://127.0.0.1:9");
        login.write(&seeded(now_secs() + 3600)).unwrap();
        assert!(!login.sign_out().await.unwrap());
        assert!(login.read().is_none());
        assert!(matches!(login.bearer().await, Err(AgentError::LlmAuth(_))));
    }

    #[tokio::test]
    async fn fresh_token_without_plan_scope_is_never_used() {
        let dir = tempfile::tempdir().unwrap();
        let login = ChatGptLogin::for_test(dir.path(), "http://127.0.0.1:9");
        let mut creds = seeded(now_secs() + 3600);
        creds.scopes.retain(|s| s != PLAN_SCOPE);
        login.write(&creds).unwrap();
        assert!(matches!(login.bearer().await, Err(AgentError::LlmAuth(_))));
    }

    #[test]
    fn host_id_is_stable_uuid_urn() {
        let dir = tempfile::tempdir().unwrap();
        let login = ChatGptLogin::for_test(dir.path(), "http://127.0.0.1:9");
        let first = login.host_id().unwrap();
        assert!(
            first.starts_with("urn:uuid:") && first.len() == 45,
            "{first}"
        );
        assert_eq!(&first[23..24], "4");
        assert_eq!(login.host_id().unwrap(), first);
    }

    #[test]
    fn identity_requires_matching_nonce() {
        let identity = |nonce: Option<&str>| Identity {
            sub: "u".into(),
            email: None,
            nonce: nonce.map(str::to_owned),
        };
        assert!(check_identity(identity(Some("n1")), "n1").is_ok());
        assert!(check_identity(identity(Some("n2")), "n1").is_err());
        assert!(check_identity(identity(None), "n1").is_err());
    }

    #[test]
    fn unusable_refresh_codes_match_openai_list() {
        for code in [
            "invalid_grant",
            "refresh_token_reused",
            "refresh_token_expired",
            "refresh_token_invalidated",
            "invalid_refresh_token",
            "token_expired",
        ] {
            assert!(is_unusable_refresh(code));
        }
        assert!(is_unusable_refresh("invalid_client"));
        assert!(!is_unusable_refresh("invalid_request"));
    }
}
