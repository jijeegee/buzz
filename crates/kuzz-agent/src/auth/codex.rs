//! Read existing file-backed Codex credentials. The login owner handles renewal.
use std::path::PathBuf;

use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::TokenSource;
use crate::types::AgentError;

const MAX_AUTH_BYTES: u64 = 1024 * 1024;

/// Existing file-backed Codex ChatGPT login. No token copies are persisted.
pub struct CodexTokenSource {
    home: PathBuf,
}

#[derive(Deserialize)]
struct Login {
    auth_mode: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    account_id: String,
}

fn auth_error(message: &str) -> AgentError {
    AgentError::LlmAuth(format!("ChatGPT subscription: {message}"))
}

impl CodexTokenSource {
    pub fn from_env() -> Result<Self, AgentError> {
        let home = std::env::var_os("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|p| p.join(".codex")))
            .ok_or_else(|| auth_error("cannot locate Codex home"))?;
        Ok(Self { home })
    }

    /// Use the selected agent's Codex home without changing process-global env.
    pub fn with_home(home: Option<PathBuf>) -> Result<Self, AgentError> {
        let mut source = Self::from_env()?;
        if let Some(home) = home {
            source.home = home;
        }
        Ok(source)
    }

    async fn read(&self) -> Result<Tokens, AgentError> {
        let file = tokio::fs::File::open(self.home.join("auth.json"))
            .await
            .map_err(|_| auth_error("sign in using the existing OpenAI subscription connection; a file-backed Codex login is required"))?;
        let mut bytes = Vec::new();
        file.take(MAX_AUTH_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| auth_error("could not read the Codex login"))?;
        if bytes.len() as u64 > MAX_AUTH_BYTES {
            return Err(auth_error("Codex login file is too large"));
        }
        Self::parse_login(&bytes)
    }

    /// Check local authentication for desktop readiness without network or writes.
    pub fn check_login(&self) -> Result<(), AgentError> {
        use std::io::Read;
        let file = std::fs::File::open(self.home.join("auth.json"))
            .map_err(|_| auth_error("sign in using the existing OpenAI subscription connection; a file-backed Codex login is required"))?;
        let mut bytes = Vec::new();
        file.take(MAX_AUTH_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| auth_error("could not read the Codex login"))?;
        let tokens = Self::parse_login(&bytes)?;
        validate_expiry(&tokens.access_token)
    }

    fn parse_login(bytes: &[u8]) -> Result<Tokens, AgentError> {
        if bytes.len() as u64 > MAX_AUTH_BYTES {
            return Err(auth_error("Codex login file is too large"));
        }
        let login: Login = serde_json::from_slice(bytes)
            .map_err(|_| auth_error("could not parse the Codex login"))?;
        if login
            .auth_mode
            .as_deref()
            .is_some_and(|mode| mode != "chatgpt")
        {
            return Err(auth_error(
                "the current Codex login is not a ChatGPT subscription",
            ));
        }
        let tokens = login
            .tokens
            .ok_or_else(|| auth_error("ChatGPT sign-in required"))?;
        if tokens.access_token.is_empty() || tokens.account_id.is_empty() {
            return Err(auth_error("ChatGPT sign-in required"));
        }
        if account_id(&tokens.access_token)? != tokens.account_id {
            return Err(auth_error("Codex account and access token do not match"));
        }
        Ok(tokens)
    }

    async fn renew(&self, rejected: &str) -> Result<String, AgentError> {
        // Adopt an already renewed bearer from the existing owner. Never start
        // a second auth owner: installed Codex's refresh lock is process-local.
        let current = self.read().await?.access_token;
        if current != rejected {
            if account_id(&current)? != account_id(rejected)? {
                return Err(auth_error(
                    "the signed-in account changed; restart the agent",
                ));
            }
            validate_expiry(&current)?;
            return Ok(current);
        }
        Err(auth_error(
            "check the existing OpenAI subscription sign-in; its login owner must renew the authentication",
        ))
    }
}

fn validate_expiry(token: &str) -> Result<(), AgentError> {
    let expiry = claims(token)?["exp"].as_u64().unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| auth_error("invalid system clock"))?
        .as_secs();
    if expiry <= now.saturating_add(60) {
        return Err(auth_error(
            "check the existing OpenAI subscription sign-in; its login owner must renew the authentication",
        ));
    }
    Ok(())
}

fn claims(bearer: &str) -> Result<Value, AgentError> {
    let payload = bearer
        .split('.')
        .nth(1)
        .ok_or_else(|| auth_error("invalid access token"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| auth_error("invalid access token"))?;
    serde_json::from_slice(&bytes).map_err(|_| auth_error("invalid access token"))
}

/// Routing metadata from the same bearer, never used as an authorization verdict.
pub(crate) fn account_id(bearer: &str) -> Result<String, AgentError> {
    claims(bearer)?["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| auth_error("missing ChatGPT account identifier"))
}

#[async_trait]
impl TokenSource for CodexTokenSource {
    async fn bearer(&self) -> Result<String, AgentError> {
        let token = self.read().await?.access_token;
        validate_expiry(&token)?;
        Ok(token)
    }

    async fn refresh_now(&self, rejected: &str) -> Result<String, AgentError> {
        self.renew(rejected).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn token(account: &str, expiry: u64, signature: &str) -> String {
        let payload =
            json!({"exp": expiry, "https://api.openai.com/auth": {"chatgpt_account_id": account}});
        format!(
            "test.{}.{signature}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    }

    fn login(home: &std::path::Path, bearer: &str, account: &str) {
        std::fs::write(
            home.join("auth.json"),
            json!({"auth_mode":"chatgpt", "tokens":{"access_token":bearer,"account_id":account}})
                .to_string(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn rejected_login_does_not_refresh_or_write_and_adopts_only_same_account() {
        let home = tempfile::tempdir().unwrap();
        let source = CodexTokenSource {
            home: home.path().to_owned(),
        };
        let first = token("account-a", u64::MAX, "first");
        login(home.path(), &first, "account-a");
        let before = std::fs::read(home.path().join("auth.json")).unwrap();
        assert_eq!(source.bearer().await.unwrap(), first);
        assert!(source.check_login().is_ok());
        assert!(source.refresh_now(&first).await.is_err());
        assert_eq!(
            before,
            std::fs::read(home.path().join("auth.json")).unwrap()
        );
        let renewed = token("account-a", u64::MAX, "renewed");
        login(home.path(), &renewed, "account-a");
        assert_eq!(source.refresh_now(&first).await.unwrap(), renewed);
        assert_eq!(source.bearer().await.unwrap(), renewed);
        login(
            home.path(),
            &token("account-b", u64::MAX, "other"),
            "account-b",
        );
        assert!(source.refresh_now(&first).await.is_err());
    }

    #[tokio::test]
    async fn expired_and_mismatched_logins_fail_closed_without_writes() {
        let home = tempfile::tempdir().unwrap();
        let source = CodexTokenSource {
            home: home.path().to_owned(),
        };
        for (bearer, account) in [
            (token("a", 0, "expired"), "a"),
            (token("a", u64::MAX, "wrong"), "b"),
        ] {
            login(home.path(), &bearer, account);
            let before = std::fs::read(home.path().join("auth.json")).unwrap();
            assert!(source.bearer().await.is_err());
            assert!(source.check_login().is_err());
            assert_eq!(
                before,
                std::fs::read(home.path().join("auth.json")).unwrap()
            );
        }
    }
}
