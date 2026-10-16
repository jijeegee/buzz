//! OpenID Connect relying-party support for centralized identity.
//!
//! The relay is the OIDC relying party: the IdP client secret lives only on
//! the server, and clients prove they started the login with PKCE (see
//! [`crate::token::verify_pkce`]). Providers implement [`OidcProvider`] so a
//! second IdP (Apple, ...) is an added implementation plus an
//! `identities.provider` CHECK extension; tests use a fake implementation.

use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use jsonwebtoken::jwk::JwkSet;
use serde::Deserialize;
use tokio::sync::RwLock;

/// Identity asserted by a provider after a successful code exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcIdentity {
    /// Stable provider subject (`sub`). The login key, never the email.
    pub subject: String,
    /// Email claim, if present. Display/bootstrap only.
    pub email: Option<String>,
    /// Whether the provider asserted the email as verified.
    pub email_verified: bool,
    /// Display name claim, used as the initial profile name.
    pub name: Option<String>,
    /// Avatar URL claim, used as the initial profile avatar.
    pub picture: Option<String>,
}

/// Failure while completing an OIDC login with a provider.
#[derive(Debug, Clone, thiserror::Error)]
pub enum OidcError {
    /// The provider rejected the authorization code exchange.
    #[error("code exchange rejected: {0}")]
    Exchange(String),
    /// The id_token failed validation (signature, iss, aud, exp, nonce).
    #[error("invalid id_token: {0}")]
    InvalidIdToken(String),
    /// The provider could not be reached or returned an unusable response.
    #[error("provider unavailable: {0}")]
    Unavailable(String),
}

/// An OpenID Connect identity provider.
pub trait OidcProvider: Send + Sync {
    /// Provider name as used in `/auth/oidc/{provider}/…` and
    /// `identities.provider`.
    fn name(&self) -> &str;

    /// Browser URL that starts the provider login. `callback_url` is the
    /// relay's single registered callback; `state` and `nonce` are echoed.
    fn authorization_url(&self, callback_url: &str, state: &str, nonce: &str) -> String;

    /// Exchange an authorization `code` for a validated identity whose
    /// id_token carries `expected_nonce`.
    fn exchange_code<'a>(
        &'a self,
        code: &'a str,
        callback_url: &'a str,
        expected_nonce: &'a str,
    ) -> BoxFuture<'a, Result<OidcIdentity, OidcError>>;
}

const GOOGLE_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_JWKS_URI: &str = "https://www.googleapis.com/oauth2/v3/certs";
const GOOGLE_ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
/// JWKS cache lifetime; an unknown `kid` forces one early refresh.
const JWKS_TTL: Duration = Duration::from_secs(3600);
/// Bound on provider HTTP calls.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on provider response bodies.
const MAX_PROVIDER_BODY_BYTES: usize = 256 * 1024;

/// Google as an OIDC provider (`openid email profile`).
pub struct GoogleProvider {
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
    jwks: RwLock<Option<(Instant, JwkSet)>>,
}

impl std::fmt::Debug for GoogleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleProvider")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
}

#[derive(Deserialize)]
struct GoogleClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string")]
    email_verified: bool,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    picture: Option<String>,
}

fn bool_or_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        Bool(bool),
        Text(String),
    }
    Ok(match Flag::deserialize(deserializer)? {
        Flag::Bool(value) => value,
        Flag::Text(value) => value.eq_ignore_ascii_case("true"),
    })
}

impl GoogleProvider {
    /// Build a Google provider from the server's OAuth client credentials.
    pub fn new(client_id: String, client_secret: String) -> Result<Self, OidcError> {
        let http = reqwest::Client::builder()
            .timeout(PROVIDER_TIMEOUT)
            .build()
            .map_err(|e| OidcError::Unavailable(e.to_string()))?;
        Ok(Self {
            client_id,
            client_secret,
            http,
            jwks: RwLock::new(None),
        })
    }

    async fn bounded_body(response: reqwest::Response) -> Result<Vec<u8>, OidcError> {
        if response
            .content_length()
            .is_some_and(|len| len > MAX_PROVIDER_BODY_BYTES as u64)
        {
            return Err(OidcError::Unavailable("provider response too large".into()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| OidcError::Unavailable(e.to_string()))?;
        if bytes.len() > MAX_PROVIDER_BODY_BYTES {
            return Err(OidcError::Unavailable("provider response too large".into()));
        }
        Ok(bytes.to_vec())
    }

    async fn fetch_jwks(&self) -> Result<JwkSet, OidcError> {
        let response = self
            .http
            .get(GOOGLE_JWKS_URI)
            .send()
            .await
            .map_err(|e| OidcError::Unavailable(e.to_string()))?;
        if !response.status().is_success() {
            return Err(OidcError::Unavailable(format!(
                "JWKS fetch returned {}",
                response.status()
            )));
        }
        let body = Self::bounded_body(response).await?;
        serde_json::from_slice(&body).map_err(|e| OidcError::Unavailable(e.to_string()))
    }

    /// The JWKS, refreshed when stale or when `kid` is unknown (at most one
    /// fetch per call).
    async fn jwks_for(&self, kid: &str) -> Result<JwkSet, OidcError> {
        {
            let cached = self.jwks.read().await;
            if let Some((fetched_at, set)) = cached.as_ref() {
                if fetched_at.elapsed() < JWKS_TTL && set.find(kid).is_some() {
                    return Ok(set.clone());
                }
            }
        }
        let fresh = self.fetch_jwks().await?;
        *self.jwks.write().await = Some((Instant::now(), fresh.clone()));
        Ok(fresh)
    }

    async fn validate_id_token(
        &self,
        id_token: &str,
        expected_nonce: &str,
    ) -> Result<OidcIdentity, OidcError> {
        let header = jsonwebtoken::decode_header(id_token)
            .map_err(|e| OidcError::InvalidIdToken(e.to_string()))?;
        if header.alg != jsonwebtoken::Algorithm::RS256 {
            return Err(OidcError::InvalidIdToken("unexpected alg".into()));
        }
        let kid = header
            .kid
            .ok_or_else(|| OidcError::InvalidIdToken("missing kid".into()))?;
        let jwks = self.jwks_for(&kid).await?;
        let jwk = jwks
            .find(&kid)
            .ok_or_else(|| OidcError::InvalidIdToken("unknown kid".into()))?;
        let key = jsonwebtoken::DecodingKey::from_jwk(jwk)
            .map_err(|e| OidcError::InvalidIdToken(e.to_string()))?;
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validation.set_audience(&[self.client_id.as_str()]);
        validation.set_issuer(&GOOGLE_ISSUERS);
        validation.leeway = 60;
        let claims = jsonwebtoken::decode::<GoogleClaims>(id_token, &key, &validation)
            .map_err(|e| OidcError::InvalidIdToken(e.to_string()))?
            .claims;
        let nonce_ok = claims.nonce.as_deref().is_some_and(|nonce| {
            crate::token::constant_time_eq(nonce.as_bytes(), expected_nonce.as_bytes())
        });
        if !nonce_ok {
            return Err(OidcError::InvalidIdToken("nonce mismatch".into()));
        }
        Ok(OidcIdentity {
            subject: claims.sub,
            email: claims.email,
            email_verified: claims.email_verified,
            name: claims.name,
            picture: claims.picture,
        })
    }
}

impl OidcProvider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
    }

    fn authorization_url(&self, callback_url: &str, state: &str, nonce: &str) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", callback_url)
            .append_pair("response_type", "code")
            .append_pair("scope", "openid email profile")
            .append_pair("state", state)
            .append_pair("nonce", nonce)
            .append_pair("prompt", "select_account")
            .finish();
        format!("{GOOGLE_AUTH_ENDPOINT}?{query}")
    }

    fn exchange_code<'a>(
        &'a self,
        code: &'a str,
        callback_url: &'a str,
        expected_nonce: &'a str,
    ) -> BoxFuture<'a, Result<OidcIdentity, OidcError>> {
        Box::pin(async move {
            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("code", code)
                .append_pair("client_id", &self.client_id)
                .append_pair("client_secret", &self.client_secret)
                .append_pair("redirect_uri", callback_url)
                .append_pair("grant_type", "authorization_code")
                .finish();
            let response = self
                .http
                .post(GOOGLE_TOKEN_ENDPOINT)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(form)
                .send()
                .await
                .map_err(|e| OidcError::Unavailable(e.to_string()))?;
            let status = response.status();
            let body = Self::bounded_body(response).await?;
            if !status.is_success() {
                return Err(OidcError::Exchange(format!(
                    "token endpoint returned {status}"
                )));
            }
            let token: TokenResponse =
                serde_json::from_slice(&body).map_err(|e| OidcError::Unavailable(e.to_string()))?;
            self.validate_id_token(&token.id_token, expected_nonce)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_url_carries_state_nonce_and_callback() {
        let provider = GoogleProvider::new("client-1".into(), "secret".into()).expect("provider");
        let url = provider.authorization_url("https://relay.example/cb", "st", "nn");
        assert!(url.starts_with(GOOGLE_AUTH_ENDPOINT));
        assert!(url.contains("client_id=client-1"));
        assert!(url.contains("state=st"));
        assert!(url.contains("nonce=nn"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Frelay.example%2Fcb"));
        assert!(
            !url.contains("secret"),
            "client secret never leaves the server"
        );
        assert!(!format!("{provider:?}").contains("secret\""));
    }

    #[test]
    fn email_verified_accepts_bool_and_string() {
        let claims: GoogleClaims =
            serde_json::from_str(r#"{"sub":"1","email_verified":"true"}"#).unwrap();
        assert!(claims.email_verified);
        let claims: GoogleClaims =
            serde_json::from_str(r#"{"sub":"1","email_verified":false}"#).unwrap();
        assert!(!claims.email_verified);
        let claims: GoogleClaims = serde_json::from_str(r#"{"sub":"1"}"#).unwrap();
        assert!(!claims.email_verified);
    }
}
