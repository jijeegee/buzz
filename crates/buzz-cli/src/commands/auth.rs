//! `buzz auth` — human sign-in and account/bot management (centralized
//! identity, Phase 2; plan §3.2 CLI row, §3.3 endpoints).
//!
//! `login` runs the Google OIDC flow through the relay with a loopback
//! redirect and PKCE, then stores the session (see [`crate::auth_session`]).
//! The management commands authenticate with a token: an explicit token
//! source (`BUZZ_BOT_TOKEN` > `BUZZ_ACCESS_TOKEN` > acp broker) when set,
//! otherwise the stored session. They never use a private key.

use std::time::Duration;

use buzz_token_broker::{Secret, TokenSource};
use clap::Subcommand;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth_loopback::{new_state, pkce_pair, LoopbackListener, LOGIN_TIMEOUT};
use crate::auth_session::{
    relay_origin, session_access_token, session_access_token_detailed, IssuedTokens, SessionError,
    SessionStore, StoredSession,
};
use crate::client::{build_http, relay_error};
use crate::error::CliError;
use crate::OutputFormat;

/// `buzz auth` subcommands.
#[derive(Subcommand)]
pub enum AuthCmd {
    /// Sign in with Google in the browser and store the session for this relay
    Login {
        /// OIDC provider configured on the relay
        #[arg(long, default_value = "google")]
        provider: String,
        /// Print the sign-in URL instead of opening the browser
        #[arg(long)]
        no_browser: bool,
        /// Device name shown in `buzz auth devices list` (default: host name)
        #[arg(long)]
        device_name: Option<String>,
    },
    /// Sign this device out (revokes its session and the bots it hosts)
    Logout {
        /// Forget the stored login even if the relay cannot be reached
        #[arg(long)]
        local_only: bool,
    },
    /// Show who the CLI acts as on this relay
    Whoami,
    /// Update your global profile (name, avatar, @username)
    Profile {
        /// New display name
        #[arg(long)]
        display_name: Option<String>,
        /// New avatar URL (http/https)
        #[arg(long, conflicts_with = "clear_avatar")]
        avatar_url: Option<String>,
        /// Remove the avatar
        #[arg(long)]
        clear_avatar: bool,
        /// New @username (3-32 of a-z, 0-9, _)
        #[arg(long, conflicts_with = "clear_username")]
        username: Option<String>,
        /// Remove the @username
        #[arg(long)]
        clear_username: bool,
    },
    /// List or sign out your devices
    #[command(subcommand)]
    Devices(DevicesCmd),
    /// Manage your sign-in sessions
    #[command(subcommand)]
    Sessions(SessionsCmd),
    /// Register and manage your bots and their tokens
    #[command(subcommand)]
    Bots(BotsCmd),
}

/// `buzz auth devices` subcommands.
#[derive(Subcommand)]
pub enum DevicesCmd {
    /// List your signed-in devices
    List,
    /// Sign a device out remotely (revokes its sessions and hosted bots)
    Revoke {
        /// Device id (from `buzz auth devices list`)
        device_id: Uuid,
    },
}

/// `buzz auth sessions` subcommands.
#[derive(Subcommand)]
pub enum SessionsCmd {
    /// Sign out every other device (bot tokens are not affected)
    RevokeOthers,
}

/// Where a new bot runs.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum BotHost {
    /// Runs anywhere with a long-lived headless token (`bzk_`)
    Headless,
    /// Hosted by this device with short-lived tokens (`bzb_`)
    ThisDevice,
}

/// `buzz auth bots` subcommands.
#[derive(Subcommand)]
pub enum BotsCmd {
    /// Register a bot you own
    Create {
        /// Bot display name
        #[arg(long)]
        name: String,
        /// Where the bot runs
        #[arg(long, value_enum, default_value = "headless")]
        host: BotHost,
    },
    /// Delete a bot (revokes all its tokens; its history stays)
    Delete {
        /// Bot id (hex)
        bot_id: String,
    },
    /// Issue a 1-hour token for a bot this device hosts (revokes earlier ones)
    Token {
        /// Bot id (hex)
        bot_id: String,
    },
    /// Issue a long-lived headless token (shown once)
    HeadlessToken {
        /// Bot id (hex)
        bot_id: String,
    },
    /// Revoke one headless token (second step of a rotation)
    RevokeHeadless {
        /// Bot id (hex)
        bot_id: String,
        /// `hash_prefix` printed by `headless-token`
        hash_prefix: String,
    },
    /// Stop a hosted bot: revoke its short-lived tokens
    Revoke {
        /// Bot id (hex)
        bot_id: String,
    },
    /// Revoke every token of every bot you own (bots are kept)
    RevokeAll,
    /// Update a bot's display name or avatar
    Profile {
        /// Bot id (hex)
        bot_id: String,
        /// New display name
        #[arg(long)]
        display_name: Option<String>,
        /// New avatar URL (http/https)
        #[arg(long, conflicts_with = "clear_avatar")]
        avatar_url: Option<String>,
        /// Remove the avatar
        #[arg(long)]
        clear_avatar: bool,
    },
}

/// How a management command authenticates.
enum Credential {
    /// An explicit token source (env or broker).
    Explicit(Secret),
    /// The stored session (may be refreshed on a 401 `token_expired`).
    Stored,
}

/// HTTP access to `/auth/*` for one command.
struct AuthApi {
    http: reqwest::Client,
    relay_url: String,
    store: Option<SessionStore>,
    credential: Credential,
}

/// Dispatch `buzz auth`. `env_source` is the explicit token source (already
/// suppressed by an argv `--private-key`); `private_key` is the configured key
/// (used only by `whoami` to report key mode).
pub async fn dispatch(
    cmd: AuthCmd,
    relay_url: String,
    env_source: Option<TokenSource>,
    private_key: Option<String>,
    format: &OutputFormat,
) -> Result<(), CliError> {
    if buzz_token_broker::stored_session_disabled(|name| std::env::var(name).ok()) {
        // Managed agents never use or create the machine owner's login.
        if matches!(cmd, AuthCmd::Login { .. } | AuthCmd::Logout { .. }) {
            return Err(CliError::Usage(format!(
                "stored logins are disabled in this process ({})",
                buzz_token_broker::DISABLE_STORED_SESSION_ENV
            )));
        }
    }
    let store = crate::stored_session_for(|name| std::env::var(name).ok());
    match cmd {
        AuthCmd::Login {
            provider,
            no_browser,
            device_name,
        } => {
            let store = store.ok_or_else(|| {
                CliError::Other("cannot locate a config directory for session.json".into())
            })?;
            let device_name = device_name.unwrap_or_else(default_device_name);
            let opener: &(dyn Fn(&str) -> bool + Sync) = if no_browser {
                &|_| false
            } else {
                &|url| webbrowser::open(url).is_ok()
            };
            let out = login(
                &relay_url,
                &store,
                &provider,
                &device_name,
                opener,
                LOGIN_TIMEOUT,
            )
            .await?;
            print_json(&out);
            Ok(())
        }
        AuthCmd::Logout { local_only } => {
            let store =
                store.ok_or_else(|| CliError::Usage("no stored login for this relay".into()))?;
            let out = logout(&relay_url, &store, local_only).await?;
            print_json(&out);
            Ok(())
        }
        AuthCmd::Whoami => {
            if env_source.is_none() && private_key.is_some() {
                let keys = nostr::Keys::parse(private_key.as_deref().unwrap_or_default())
                    .map_err(|e| CliError::Key(format!("invalid BUZZ_PRIVATE_KEY: {e}")))?;
                print_json(&json!({ "mode": "key", "pubkey": keys.public_key().to_hex() }));
                return Ok(());
            }
            let source = match &env_source {
                Some(TokenSource::BotToken(_)) => "env:BUZZ_BOT_TOKEN",
                Some(TokenSource::AccessToken(_)) => "env:BUZZ_ACCESS_TOKEN",
                Some(TokenSource::Broker { .. }) => "broker",
                None => "session",
            };
            let api = AuthApi::new(relay_url, store, env_source).await?;
            let mut me = api.call(reqwest::Method::GET, "/auth/me", None).await?;
            if let Value::Object(map) = &mut me {
                map.insert("mode".into(), json!("token"));
                map.insert("source".into(), json!(source));
            }
            print_json(&me);
            Ok(())
        }
        AuthCmd::Profile {
            display_name,
            avatar_url,
            clear_avatar,
            username,
            clear_username,
        } => {
            let mut body = serde_json::Map::new();
            if let Some(name) = display_name {
                body.insert("display_name".into(), json!(name));
            }
            if let Some(url) = avatar_url {
                body.insert("avatar_url".into(), json!(url));
            } else if clear_avatar {
                body.insert("avatar_url".into(), Value::Null);
            }
            if let Some(name) = username {
                body.insert("username".into(), json!(name));
            } else if clear_username {
                body.insert("username".into(), Value::Null);
            }
            if body.is_empty() {
                return Err(CliError::Usage("nothing to update".into()));
            }
            let api = AuthApi::new(relay_url, store, env_source).await?;
            let out = api
                .call(
                    reqwest::Method::PATCH,
                    "/auth/profile",
                    Some(Value::Object(body)),
                )
                .await?;
            print_json(&out);
            Ok(())
        }
        AuthCmd::Devices(sub) => {
            let api = AuthApi::new(relay_url, store, env_source).await?;
            match sub {
                DevicesCmd::List => {
                    let devices = api
                        .call(reqwest::Method::GET, "/auth/devices", None)
                        .await?;
                    print_json(&compact_devices(devices, format));
                }
                DevicesCmd::Revoke { device_id } => {
                    api.call(
                        reqwest::Method::DELETE,
                        &format!("/auth/devices/{device_id}"),
                        None,
                    )
                    .await?;
                    print_json(&accepted(
                        "device signed out",
                        &[("device_id", json!(device_id))],
                    ));
                }
            }
            Ok(())
        }
        AuthCmd::Sessions(SessionsCmd::RevokeOthers) => {
            let api = AuthApi::new(relay_url, store, env_source).await?;
            api.call(
                reqwest::Method::POST,
                "/auth/sessions/revoke-others",
                Some(json!({})),
            )
            .await?;
            print_json(&accepted("other devices signed out", &[]));
            Ok(())
        }
        AuthCmd::Bots(sub) => {
            let api = AuthApi::new(relay_url, store, env_source).await?;
            bots(&api, sub).await
        }
    }
}

async fn bots(api: &AuthApi, cmd: BotsCmd) -> Result<(), CliError> {
    use reqwest::Method;
    let out = match cmd {
        BotsCmd::Create { name, host } => {
            let host = match host {
                BotHost::Headless => "headless",
                BotHost::ThisDevice => "this_device",
            };
            api.call(
                Method::POST,
                "/auth/bots",
                Some(json!({ "display_name": name, "host": host })),
            )
            .await?
        }
        BotsCmd::Delete { bot_id } => {
            let id = validate_bot_id(&bot_id)?;
            api.call(Method::DELETE, &format!("/auth/bots/{id}"), None)
                .await?;
            accepted("bot deleted", &[("bot_id", json!(id))])
        }
        BotsCmd::Token { bot_id } => {
            let id = validate_bot_id(&bot_id)?;
            api.call(
                Method::POST,
                &format!("/auth/bots/{id}/token"),
                Some(json!({})),
            )
            .await?
        }
        BotsCmd::HeadlessToken { bot_id } => {
            let id = validate_bot_id(&bot_id)?;
            api.call(
                Method::POST,
                &format!("/auth/bots/{id}/headless-token"),
                Some(json!({})),
            )
            .await?
        }
        BotsCmd::RevokeHeadless {
            bot_id,
            hash_prefix,
        } => {
            let id = validate_bot_id(&bot_id)?;
            if !(8..=64).contains(&hash_prefix.len())
                || !hash_prefix.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(CliError::Usage(
                    "hash_prefix must be 8-64 hex characters".into(),
                ));
            }
            api.call(
                Method::DELETE,
                &format!("/auth/bots/{id}/headless-token/{hash_prefix}"),
                None,
            )
            .await?;
            accepted(
                "headless token revoked",
                &[("bot_id", json!(id)), ("hash_prefix", json!(hash_prefix))],
            )
        }
        BotsCmd::Revoke { bot_id } => {
            let id = validate_bot_id(&bot_id)?;
            api.call(
                Method::POST,
                &format!("/auth/bots/{id}/revoke"),
                Some(json!({})),
            )
            .await?;
            accepted("bot tokens revoked", &[("bot_id", json!(id))])
        }
        BotsCmd::RevokeAll => {
            api.call(Method::POST, "/auth/bots/revoke-all", Some(json!({})))
                .await?;
            accepted("all bot tokens revoked", &[])
        }
        BotsCmd::Profile {
            bot_id,
            display_name,
            avatar_url,
            clear_avatar,
        } => {
            let id = validate_bot_id(&bot_id)?;
            let mut body = serde_json::Map::new();
            if let Some(name) = display_name {
                body.insert("display_name".into(), json!(name));
            }
            if let Some(url) = avatar_url {
                body.insert("avatar_url".into(), json!(url));
            } else if clear_avatar {
                body.insert("avatar_url".into(), Value::Null);
            }
            if body.is_empty() {
                return Err(CliError::Usage("nothing to update".into()));
            }
            api.call(
                Method::PATCH,
                &format!("/auth/bots/{id}/profile"),
                Some(Value::Object(body)),
            )
            .await?
        }
    };
    print_json(&out);
    Ok(())
}

fn validate_bot_id(id: &str) -> Result<String, CliError> {
    let id = id.trim().to_ascii_lowercase();
    if id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(id)
    } else {
        Err(CliError::Usage("bot id must be 64 hex characters".into()))
    }
}

fn accepted(message: &str, fields: &[(&str, Value)]) -> Value {
    let mut out = serde_json::Map::new();
    out.insert("accepted".into(), json!(true));
    out.insert("message".into(), json!(message));
    for (key, value) in fields {
        out.insert((*key).to_owned(), value.clone());
    }
    Value::Object(out)
}

fn compact_devices(devices: Value, format: &OutputFormat) -> Value {
    match (format, devices) {
        (OutputFormat::Compact, Value::Array(items)) => Value::Array(
            items
                .into_iter()
                .map(|d| {
                    json!({
                        "id": d.get("id"),
                        "name": d.get("name"),
                        "platform": d.get("platform"),
                        "current": d.get("current"),
                    })
                })
                .collect(),
        ),
        (_, devices) => devices,
    }
}

fn print_json(value: &Value) {
    println!("{value}");
}

/// The host name, bounded to what the relay accepts (64 printable chars).
pub fn default_device_name() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_owned())
        })
        .unwrap_or_default();
    let name: String = raw.chars().filter(|c| !c.is_control()).take(48).collect();
    let name = name.trim();
    if name.is_empty() {
        "buzz CLI".to_owned()
    } else {
        format!("{name} (CLI)")
    }
}

/// The browser start URL for this login.
pub fn start_url(
    relay_url: &str,
    provider: &str,
    state: &str,
    challenge: &str,
    redirect_uri: &str,
    device_name: &str,
) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("client", "cli")
        .append_pair("identity_mode", "token")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("device_name", device_name)
        .finish();
    let provider: String = url::form_urlencoded::byte_serialize(provider.as_bytes()).collect();
    format!("{relay_url}/auth/oidc/{provider}/start?{query}")
}

/// Run the whole login: loopback listener, browser, `POST
/// /auth/oidc/complete`, store. `open_browser` returns whether it opened the
/// URL (the URL is always printed to stderr too). Returns the JSON result.
pub async fn login(
    relay_url: &str,
    store: &SessionStore,
    provider: &str,
    device_name: &str,
    open_browser: &(dyn Fn(&str) -> bool + Sync),
    timeout: Duration,
) -> Result<Value, CliError> {
    let origin = relay_origin(relay_url)?;
    let listener = LoopbackListener::bind()
        .await
        .map_err(|e| CliError::Other(e.to_string()))?;
    let state = new_state();
    let pkce = pkce_pair();
    let url = start_url(
        relay_url,
        provider,
        &state,
        &pkce.challenge,
        &listener.redirect_uri(),
        device_name,
    );
    let opened = open_browser(&url);
    if opened {
        eprintln!("Opening your browser to sign in. If it did not open, visit:\n{url}");
    } else {
        eprintln!("Open this URL in your browser to sign in:\n{url}");
    }
    let code = listener
        .wait_for_code(&state, timeout)
        .await
        .map_err(|e| CliError::Auth(format!("login failed: {e}")))?;

    let http = build_http()?;
    let resp = http
        .post(format!("{relay_url}/auth/oidc/complete"))
        .json(&json!({ "login_code": code.as_str(), "code_verifier": pkce.verifier.as_str() }))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let body = resp.text().await?;
    if !(200..300).contains(&status) {
        return Err(match relay_error(status, body) {
            CliError::Relay { status: 400, body } => {
                CliError::Auth(format!("login failed: {body}"))
            }
            other => other,
        });
    }
    let parsed: Value = serde_json::from_str(&body)
        .map_err(|e| CliError::Other(format!("invalid login response: {e}")))?;
    if parsed
        .get("identity_mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode != "token")
    {
        return Err(CliError::Auth(
            "Google key recovery requires Buzz desktop or mobile; the CLI signing identity was not changed".into(),
        ));
    }
    let field = |name: &str| parsed.get(name).and_then(|v| v.as_str()).map(str::to_owned);
    let (Some(principal_id), Some(access), Some(refresh)) =
        (field("principal_id"), field("access"), field("refresh"))
    else {
        return Err(CliError::Other("incomplete login response".into()));
    };
    let device_id = field("device_id");
    let tokens = IssuedTokens {
        access: Secret::new(access),
        refresh: Secret::new(refresh),
        expires_in: parsed
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600),
    };
    let storage = store
        .save_tokens(&origin, &principal_id, device_id.clone(), &tokens)
        .await?;
    if storage == crate::auth_session::RefreshStorage::File {
        eprintln!(
            "warning: no OS credential store is available; the refresh token is stored in {}",
            store.path().display()
        );
    }
    Ok(json!({
        "accepted": true,
        "message": "signed in",
        "relay": origin,
        "principal_id": principal_id,
        "device_id": device_id,
        "refresh_storage": storage.as_str(),
    }))
}

/// Sign out the stored session: `POST /auth/logout`, then forget it locally.
/// Expired access is refreshed and retried once; only successful revocation
/// or a terminal refresh rejection forgets locally. Other failures preserve
/// the session for retry unless `local_only`.
pub async fn logout(
    relay_url: &str,
    store: &SessionStore,
    local_only: bool,
) -> Result<Value, CliError> {
    let origin = relay_origin(relay_url)?;
    if store.get(&origin)?.is_none() {
        return Err(CliError::Usage(format!(
            "no stored login for {origin}; nothing to sign out"
        )));
    }
    if local_only {
        store.forget(&origin).await?;
        return Ok(json!({
            "accepted": true,
            "message": "forgot the local login; the relay session was not revoked",
            "relay": origin,
        }));
    }
    let http = build_http()?;
    let mut force_refresh = false;
    let remote = loop {
        match session_access_token_detailed(&http, relay_url, store, force_refresh).await {
            Ok(Some((token, _))) => {
                let resp = http
                    .post(format!("{relay_url}/auth/logout"))
                    .bearer_auth(token.expose())
                    .json(&json!({}))
                    .send()
                    .await?;
                let status = resp.status().as_u16();
                let body = resp.text().await?;
                if (200..300).contains(&status) {
                    break "revoked";
                }
                let expired = status == 401
                    && serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|v| v.get("code").and_then(Value::as_str).map(str::to_owned))
                        .as_deref()
                        == Some("token_expired");
                if expired && !force_refresh {
                    force_refresh = true;
                    continue;
                }
                return Err(relay_error(status, body));
            }
            // Only a terminal refresh rejection proves the session is ended.
            Ok(None) | Err(SessionError::Ended(_)) => break "already_ended",
            Err(SessionError::Failed(error)) => return Err(error),
        }
    };
    store.forget(&origin).await?;
    Ok(json!({
        "accepted": true,
        "message": "signed out",
        "relay": origin,
        "relay_session": remote,
    }))
}

impl AuthApi {
    async fn new(
        relay_url: String,
        store: Option<SessionStore>,
        env_source: Option<TokenSource>,
    ) -> Result<Self, CliError> {
        let credential = match env_source {
            Some(source) => {
                let token = tokio::task::spawn_blocking(move || source.token())
                    .await
                    .map_err(|e| CliError::Other(format!("token fetch task failed: {e}")))?
                    .map_err(crate::client::broker_error)?;
                Credential::Explicit(token)
            }
            None => Credential::Stored,
        };
        Ok(Self {
            http: build_http()?,
            relay_url,
            store,
            credential,
        })
    }

    async fn token(&self, force_refresh: bool) -> Result<Secret, CliError> {
        match &self.credential {
            Credential::Explicit(token) => Ok(token.clone()),
            Credential::Stored => {
                let store = self.store.as_ref().ok_or_else(not_signed_in)?;
                session_access_token(&self.http, &self.relay_url, store, force_refresh)
                    .await?
                    .map(|(token, _): (Secret, StoredSession)| token)
                    .ok_or_else(not_signed_in)
            }
        }
    }

    /// Send one request; a stored session whose access token the relay calls
    /// expired is refreshed and the request retried once.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, CliError> {
        let mut force_refresh = false;
        loop {
            let token = self.token(force_refresh).await?;
            let mut req = self
                .http
                .request(method.clone(), format!("{}{path}", self.relay_url))
                .bearer_auth(token.expose());
            if let Some(body) = &body {
                req = req.json(body);
            }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            let text = resp.text().await?;
            if (200..300).contains(&status) {
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&text)
                    .map_err(|e| CliError::Other(format!("invalid relay response: {e}")));
            }
            let expired = status == 401
                && serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned))
                    .as_deref()
                    == Some("token_expired");
            if expired && !force_refresh && matches!(self.credential, Credential::Stored) {
                force_refresh = true;
                continue;
            }
            return Err(match relay_error(status, text) {
                CliError::Relay { status: 404, body } if path.starts_with("/auth/") => {
                    if body.contains("not found") {
                        CliError::NotFound(body)
                    } else {
                        CliError::Relay { status: 404, body }
                    }
                }
                other => other,
            });
        }
    }
}

fn not_signed_in() -> CliError {
    CliError::Auth("not signed in to this relay; run `buzz auth login`".into())
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
