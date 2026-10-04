//! Configuration for centralized-identity token auth (`AUTH_TOKEN_ENABLED`).

use std::time::Duration;

use crate::config::ConfigError;

/// Default user access-token lifetime.
pub const DEFAULT_ACCESS_TTL_SECS: u64 = 3600;
/// Default refresh-token lifetime (90 days, sliding).
pub const DEFAULT_REFRESH_TTL_SECS: u64 = 90 * 24 * 3600;
/// Default desktop-hosted bot token lifetime.
pub const DEFAULT_BOT_TTL_SECS: u64 = 3600;
/// Grace period a superseded bot token keeps working after an exchange.
pub const DEFAULT_EXCHANGE_GRACE_SECS: u64 = 60;
/// Default mobile custom redirect scheme.
pub const DEFAULT_MOBILE_REDIRECT_SCHEME: &str = "xyz.block.buzz";

/// Google OAuth client credentials (server-side only).
#[derive(Clone)]
pub struct GoogleOidcConfig {
    /// OAuth client id.
    pub client_id: String,
    /// OAuth client secret. Never logged.
    pub client_secret: String,
}

impl std::fmt::Debug for GoogleOidcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleOidcConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .finish()
    }
}

/// Token-auth configuration. Everything is inert while `enabled` is false.
#[derive(Debug, Clone)]
pub struct AuthTokenConfig {
    /// `AUTH_TOKEN_ENABLED` (default false). Gates `/auth/*` (unrouted when off:
    /// it answers like any unknown path, a 403),
    /// WS `["AUTH", {"token"}]`, the bridge `Bearer` branch and the operator
    /// bootstrap.
    pub enabled: bool,
    /// `AUTH_ACCESS_TTL_SECS`.
    pub access_ttl: Duration,
    /// `AUTH_REFRESH_TTL_SECS`.
    pub refresh_ttl: Duration,
    /// `AUTH_BOT_TTL_SECS`.
    pub bot_ttl: Duration,
    /// `AUTH_EXCHANGE_GRACE_SECS`.
    pub exchange_grace: Duration,
    /// `AUTH_OIDC_GOOGLE_CLIENT_ID` / `AUTH_OIDC_GOOGLE_CLIENT_SECRET`.
    pub google: Option<GoogleOidcConfig>,
    /// `AUTH_OIDC_FAKE` — dev builds only: a fake provider that logs in
    /// whatever subject the start URL names. Rejected in non-dev builds.
    pub fake_oidc: bool,
    /// `AUTH_PUBLIC_URL` — HTTP(S) origin used for the OIDC callback and the
    /// web redirect. Defaults to `BUZZ_RELAY_URL` with ws→http.
    pub public_url: String,
    /// `AUTH_MOBILE_REDIRECT_SCHEMES` — comma-separated custom schemes allowed
    /// as mobile `redirect_uri` (`<scheme>://auth/cb`).
    pub mobile_redirect_schemes: Vec<String>,
    /// `RELAY_OPERATOR_BOOTSTRAP_EMAIL` — see plan §3.3 (B8).
    pub operator_bootstrap_email: Option<String>,
    /// `AUTH_TRUSTED_PROXY_CIDRS` — reverse proxies whose `X-Forwarded-For` /
    /// `Forwarded` headers name the client for `/auth/*` per-IP limits.
    pub trusted_proxies: super::client_ip::TrustedProxies,
}

impl AuthTokenConfig {
    /// A disabled configuration (the Phase 0 default).
    pub fn disabled(relay_url: &str) -> Self {
        Self {
            enabled: false,
            access_ttl: Duration::from_secs(DEFAULT_ACCESS_TTL_SECS),
            refresh_ttl: Duration::from_secs(DEFAULT_REFRESH_TTL_SECS),
            bot_ttl: Duration::from_secs(DEFAULT_BOT_TTL_SECS),
            exchange_grace: Duration::from_secs(DEFAULT_EXCHANGE_GRACE_SECS),
            google: None,
            fake_oidc: false,
            public_url: http_origin_from_relay_url(relay_url),
            mobile_redirect_schemes: vec![DEFAULT_MOBILE_REDIRECT_SCHEME.to_owned()],
            operator_bootstrap_email: None,
            trusted_proxies: super::client_ip::TrustedProxies::default(),
        }
    }

    /// Load from the environment.
    pub fn from_env(relay_url: &str) -> Result<Self, ConfigError> {
        Self::from_lookup(relay_url, |name| std::env::var(name).ok())
    }

    /// Load from an injected variable lookup (tests avoid mutating process env).
    pub fn from_lookup(
        relay_url: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let mut config = Self::disabled(relay_url);
        config.enabled = parse_flag(&lookup, "AUTH_TOKEN_ENABLED")?;
        config.access_ttl = parse_secs(&lookup, "AUTH_ACCESS_TTL_SECS", config.access_ttl)?;
        config.refresh_ttl = parse_secs(&lookup, "AUTH_REFRESH_TTL_SECS", config.refresh_ttl)?;
        config.bot_ttl = parse_secs(&lookup, "AUTH_BOT_TTL_SECS", config.bot_ttl)?;
        config.exchange_grace =
            parse_secs(&lookup, "AUTH_EXCHANGE_GRACE_SECS", config.exchange_grace)?;
        let client_id = non_empty(&lookup, "AUTH_OIDC_GOOGLE_CLIENT_ID");
        let client_secret = non_empty(&lookup, "AUTH_OIDC_GOOGLE_CLIENT_SECRET");
        config.google = match (client_id, client_secret) {
            (Some(client_id), Some(client_secret)) => Some(GoogleOidcConfig {
                client_id,
                client_secret,
            }),
            (None, None) => None,
            _ => {
                return Err(ConfigError::InvalidValue(
                    "AUTH_OIDC_GOOGLE_CLIENT_ID and AUTH_OIDC_GOOGLE_CLIENT_SECRET must be set together"
                        .into(),
                ))
            }
        };
        config.fake_oidc = parse_flag(&lookup, "AUTH_OIDC_FAKE")?;
        if config.fake_oidc && !cfg!(any(test, feature = "dev")) {
            return Err(ConfigError::InvalidValue(
                "AUTH_OIDC_FAKE is only available in builds with the `dev` feature".into(),
            ));
        }
        if let Some(url) = non_empty(&lookup, "AUTH_PUBLIC_URL") {
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(ConfigError::InvalidValue(
                    "AUTH_PUBLIC_URL must be an http(s) origin".into(),
                ));
            }
            config.public_url = url.trim_end_matches('/').to_owned();
        }
        if let Some(schemes) = non_empty(&lookup, "AUTH_MOBILE_REDIRECT_SCHEMES") {
            config.mobile_redirect_schemes = schemes
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
        }
        config.operator_bootstrap_email = non_empty(&lookup, "RELAY_OPERATOR_BOOTSTRAP_EMAIL");
        if let Some(raw) = non_empty(&lookup, "AUTH_TRUSTED_PROXY_CIDRS") {
            config.trusted_proxies = super::client_ip::TrustedProxies::parse(&raw)
                .map_err(|e| ConfigError::InvalidValue(format!("AUTH_TRUSTED_PROXY_CIDRS: {e}")))?;
        }
        if config.enabled && config.google.is_none() && !config.fake_oidc {
            tracing::warn!(
                "AUTH_TOKEN_ENABLED without an OIDC provider: /auth/oidc/* will return 404"
            );
        }
        Ok(config)
    }

    /// The relay's single OIDC callback URL for `provider`.
    pub fn callback_url(&self, provider: &str) -> String {
        format!("{}/auth/oidc/{provider}/callback", self.public_url)
    }
}

/// `ws://h` → `http://h`, `wss://h` → `https://h`; trailing slash dropped.
pub fn http_origin_from_relay_url(relay_url: &str) -> String {
    let origin = if let Some(rest) = relay_url.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = relay_url.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        relay_url.to_owned()
    };
    origin.trim_end_matches('/').to_owned()
}

fn non_empty(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    lookup(name)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn parse_flag(lookup: &impl Fn(&str) -> Option<String>, name: &str) -> Result<bool, ConfigError> {
    match non_empty(lookup, name).map(|v| v.to_ascii_lowercase()) {
        None => Ok(false),
        Some(v) if matches!(v.as_str(), "1" | "true" | "on") => Ok(true),
        Some(v) if matches!(v.as_str(), "0" | "false" | "off") => Ok(false),
        Some(_) => Err(ConfigError::InvalidValue(format!(
            "{name} must be true or false"
        ))),
    }
}

fn parse_secs(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &str,
    default: Duration,
) -> Result<Duration, ConfigError> {
    match non_empty(lookup, name) {
        None => Ok(default),
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|secs| *secs > 0)
            .map(Duration::from_secs)
            .ok_or_else(|| ConfigError::InvalidValue(format!("{name} must be a positive integer"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn defaults_are_disabled() {
        let config = AuthTokenConfig::from_lookup("wss://relay.example", lookup(&[])).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.public_url, "https://relay.example");
        assert_eq!(
            config.callback_url("google"),
            "https://relay.example/auth/oidc/google/callback"
        );
    }

    #[test]
    fn parses_flag_ttls_and_google_pair() {
        let config = AuthTokenConfig::from_lookup(
            "ws://localhost:3000/",
            lookup(&[
                ("AUTH_TOKEN_ENABLED", "true"),
                ("AUTH_ACCESS_TTL_SECS", "180"),
                ("AUTH_OIDC_GOOGLE_CLIENT_ID", "id"),
                ("AUTH_OIDC_GOOGLE_CLIENT_SECRET", "shh"),
                ("RELAY_OPERATOR_BOOTSTRAP_EMAIL", "Owner@Example.com"),
            ]),
        )
        .unwrap();
        assert!(config.enabled);
        assert_eq!(config.access_ttl, Duration::from_secs(180));
        assert_eq!(config.public_url, "http://localhost:3000");
        assert!(!format!("{config:?}").contains("shh"), "secret is redacted");
        assert_eq!(
            config.operator_bootstrap_email.as_deref(),
            Some("Owner@Example.com")
        );
    }

    #[test]
    fn rejects_half_configured_google_and_bad_values() {
        assert!(AuthTokenConfig::from_lookup(
            "ws://h",
            lookup(&[("AUTH_OIDC_GOOGLE_CLIENT_ID", "id")])
        )
        .is_err());
        assert!(
            AuthTokenConfig::from_lookup("ws://h", lookup(&[("AUTH_TOKEN_ENABLED", "maybe")]))
                .is_err()
        );
        assert!(
            AuthTokenConfig::from_lookup("ws://h", lookup(&[("AUTH_ACCESS_TTL_SECS", "0")]))
                .is_err()
        );
        assert!(AuthTokenConfig::from_lookup(
            "ws://h",
            lookup(&[("AUTH_TRUSTED_PROXY_CIDRS", "10.0.0.0/99")])
        )
        .is_err());
    }

    #[test]
    fn parses_trusted_proxies() {
        let config = AuthTokenConfig::from_lookup(
            "ws://h",
            lookup(&[("AUTH_TRUSTED_PROXY_CIDRS", "10.0.0.0/8,unix")]),
        )
        .unwrap();
        assert!(!config.trusted_proxies.is_empty());
        assert!(AuthTokenConfig::from_lookup("ws://h", lookup(&[]))
            .unwrap()
            .trusted_proxies
            .is_empty());
    }
}
