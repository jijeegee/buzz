//! Sign in with Apple as an OIDC provider.
//!
//! Browser logins (Android, desktop) use the Services ID; the iOS app signs
//! in natively and its id_token is issued for the app's bundle id. Apple has
//! no static client secret: each token request carries a short-lived ES256
//! JWT signed with the team's Sign in with Apple key (`.p8`).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::future::BoxFuture;
use jsonwebtoken::jwk::JwkSet;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{
    bool_or_string, GoogleProvider, OidcError, OidcIdentity, OidcProvider, RevokeToken, JWKS_TTL,
    PROVIDER_TIMEOUT,
};

const APPLE_ISSUER: &str = "https://appleid.apple.com";
const APPLE_AUTH_ENDPOINT: &str = "https://appleid.apple.com/auth/authorize";
const APPLE_TOKEN_ENDPOINT: &str = "https://appleid.apple.com/auth/token";
const APPLE_JWKS_URI: &str = "https://appleid.apple.com/auth/keys";
const APPLE_REVOKE_ENDPOINT: &str = "https://appleid.apple.com/auth/revoke";
/// Lifetime of one client-secret JWT (Apple allows up to six months).
const CLIENT_SECRET_TTL: Duration = Duration::from_secs(300);

/// Server credentials for Sign in with Apple.
#[derive(Clone)]
pub struct AppleConfig {
    /// Services ID: the `client_id` of browser logins.
    pub services_id: String,
    /// iOS bundle id: the `aud` of the app's native id_tokens.
    pub bundle_id: Option<String>,
    /// Apple Developer team id (`iss` of the client secret).
    pub team_id: String,
    /// Key id of the Sign in with Apple key (`kid` of the client secret).
    pub key_id: String,
    /// The key itself, PKCS#8 PEM (the downloaded `.p8`).
    pub private_key_pem: String,
}

impl std::fmt::Debug for AppleConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleConfig")
            .field("services_id", &self.services_id)
            .field("bundle_id", &self.bundle_id)
            .field("team_id", &self.team_id)
            .field("key_id", &self.key_id)
            .field("private_key_pem", &"[REDACTED]")
            .finish()
    }
}

/// Sign in with Apple (`name email`).
pub struct AppleProvider {
    config: AppleConfig,
    signing_key: jsonwebtoken::EncodingKey,
    http: reqwest::Client,
    jwks: RwLock<Option<(Instant, JwkSet)>>,
}

impl std::fmt::Debug for AppleProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: String,
    /// Kept (encrypted) to revoke the sign-in when the account is deleted.
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct AppleClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, deserialize_with = "bool_or_string")]
    email_verified: bool,
    #[serde(default)]
    nonce: Option<String>,
}

#[derive(Serialize)]
struct ClientSecretClaims<'a> {
    iss: &'a str,
    iat: u64,
    exp: u64,
    aud: &'a str,
    sub: &'a str,
}

impl AppleProvider {
    /// Build the provider; fails when the key is not an EC PKCS#8 PEM.
    pub fn new(config: AppleConfig) -> Result<Self, OidcError> {
        let signing_key = jsonwebtoken::EncodingKey::from_ec_pem(config.private_key_pem.as_bytes())
            .map_err(|e| OidcError::Unavailable(format!("Apple key: {e}")))?;
        let http = reqwest::Client::builder()
            .timeout(PROVIDER_TIMEOUT)
            .build()
            .map_err(|e| OidcError::Unavailable(e.to_string()))?;
        Ok(Self {
            config,
            signing_key,
            http,
            jwks: RwLock::new(None),
        })
    }

    /// The ES256 client secret for a token request.
    fn client_secret(&self) -> Result<String, OidcError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| OidcError::Unavailable(e.to_string()))?
            .as_secs();
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(self.config.key_id.clone());
        let claims = ClientSecretClaims {
            iss: &self.config.team_id,
            iat: now,
            exp: now + CLIENT_SECRET_TTL.as_secs(),
            aud: APPLE_ISSUER,
            sub: &self.config.services_id,
        };
        jsonwebtoken::encode(&header, &claims, &self.signing_key)
            .map_err(|e| OidcError::Unavailable(format!("Apple client secret: {e}")))
    }

    async fn fetch_jwks(&self) -> Result<JwkSet, OidcError> {
        let response = self
            .http
            .get(APPLE_JWKS_URI)
            .send()
            .await
            .map_err(|e| OidcError::Unavailable(e.to_string()))?;
        if !response.status().is_success() {
            return Err(OidcError::Unavailable(format!(
                "JWKS fetch returned {}",
                response.status()
            )));
        }
        let body = GoogleProvider::bounded_body(response).await?;
        serde_json::from_slice(&body).map_err(|e| OidcError::Unavailable(e.to_string()))
    }

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

    /// Validate an Apple id_token issued for `audience` carrying
    /// `expected_nonce`.
    async fn validate_id_token(
        &self,
        id_token: &str,
        audience: &str,
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
        validation.set_audience(&[audience]);
        validation.set_issuer(&[APPLE_ISSUER]);
        validation.leeway = 60;
        let claims = jsonwebtoken::decode::<AppleClaims>(id_token, &key, &validation)
            .map_err(|e| OidcError::InvalidIdToken(e.to_string()))?
            .claims;
        let nonce_ok = claims.nonce.as_deref().is_some_and(|nonce| {
            crate::token::constant_time_eq(nonce.as_bytes(), expected_nonce.as_bytes())
        });
        if !nonce_ok {
            return Err(OidcError::InvalidIdToken("nonce mismatch".into()));
        }
        // Apple sends the name only to the client, on the first sign-in;
        // the relay adds it from there (see `with_first_sign_in_name`).
        Ok(OidcIdentity {
            subject: claims.sub,
            email: claims.email,
            email_verified: claims.email_verified,
            name: None,
            picture: None,
            revoke_token: None,
        })
    }
}

impl OidcProvider for AppleProvider {
    fn name(&self) -> &str {
        "apple"
    }

    // `form_post` is required when asking for name/email: Apple then POSTs
    // the callback instead of redirecting with a query.
    fn authorization_url(&self, callback_url: &str, state: &str, nonce: &str) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", &self.config.services_id)
            .append_pair("redirect_uri", callback_url)
            .append_pair("response_type", "code")
            .append_pair("response_mode", "form_post")
            .append_pair("scope", "name email")
            .append_pair("state", state)
            .append_pair("nonce", nonce)
            .finish();
        format!("{APPLE_AUTH_ENDPOINT}?{query}")
    }

    fn exchange_code<'a>(
        &'a self,
        code: &'a str,
        callback_url: &'a str,
        expected_nonce: &'a str,
    ) -> BoxFuture<'a, Result<OidcIdentity, OidcError>> {
        Box::pin(async move {
            let client_secret = self.client_secret()?;
            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("code", code)
                .append_pair("client_id", &self.config.services_id)
                .append_pair("client_secret", &client_secret)
                .append_pair("redirect_uri", callback_url)
                .append_pair("grant_type", "authorization_code")
                .finish();
            let response = self
                .http
                .post(APPLE_TOKEN_ENDPOINT)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(form)
                .send()
                .await
                .map_err(|e| OidcError::Unavailable(e.to_string()))?;
            let status = response.status();
            let body = GoogleProvider::bounded_body(response).await?;
            if !status.is_success() {
                return Err(OidcError::Exchange(format!(
                    "token endpoint returned {status}"
                )));
            }
            let token: TokenResponse =
                serde_json::from_slice(&body).map_err(|e| OidcError::Unavailable(e.to_string()))?;
            let mut identity = self
                .validate_id_token(&token.id_token, &self.config.services_id, expected_nonce)
                .await?;
            identity.revoke_token = token
                .refresh_token
                .filter(|t| !t.is_empty())
                .map(RevokeToken::new);
            Ok(identity)
        })
    }

    fn app_sign_in_client_id(&self) -> Option<&str> {
        self.config.bundle_id.as_deref()
    }

    fn verify_id_token<'a>(
        &'a self,
        id_token: &'a str,
        expected_nonce: &'a str,
    ) -> BoxFuture<'a, Result<OidcIdentity, OidcError>> {
        Box::pin(async move {
            let Some(bundle_id) = self.config.bundle_id.as_deref() else {
                return Err(OidcError::InvalidIdToken(
                    "provider has no app sign-in".into(),
                ));
            };
            self.validate_id_token(id_token, bundle_id, expected_nonce)
                .await
        })
    }

    // Required by App Store review: deleting an account made with Sign in
    // with Apple also ends the app's access to the Apple ID.
    fn revoke<'a>(&'a self, token: &'a RevokeToken) -> BoxFuture<'a, Result<(), OidcError>> {
        Box::pin(async move {
            let client_secret = self.client_secret()?;
            let form = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("client_id", &self.config.services_id)
                .append_pair("client_secret", &client_secret)
                .append_pair("token", token.expose())
                .append_pair("token_type_hint", "refresh_token")
                .finish();
            let response = self
                .http
                .post(APPLE_REVOKE_ENDPOINT)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(form)
                .send()
                .await
                .map_err(|e| OidcError::Unavailable(e.to_string()))?;
            let status = response.status();
            if !status.is_success() {
                return Err(OidcError::Exchange(format!(
                    "revoke endpoint returned {status}"
                )));
            }
            Ok(())
        })
    }
}

/// Apple's first-sign-in `user` form field (`{"name":{"firstName",
/// "lastName"}}`) as a display name. Unsigned, so only ever a display name.
pub fn first_sign_in_name(user_json: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct User {
        name: Option<Name>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Name {
        first_name: Option<String>,
        last_name: Option<String>,
    }
    let name = serde_json::from_str::<User>(user_json).ok()?.name?;
    let full = [name.first_name, name.last_name]
        .into_iter()
        .flatten()
        .map(|part| part.trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!full.is_empty()).then_some(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway P-256 key pair (PKCS#8), generated for these tests only.
    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgMQoPZ9ThzskUrNhI
nlD6ecp4d1+khnxCayAp/700gq+hRANCAARPqu21rrE4X3srcnBNM5UeLGcVHP/N
1VgOObnFEg1B0cLznon1d0kIijKFEeZfSrNdbaBgaCR1phCyJlnvZoZd
-----END PRIVATE KEY-----";
    const TEST_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAET6rtta6xOF97K3JwTTOVHixnFRz/
zdVYDjm5xRINQdHC856J9XdJCIoyhRHmX0qzXW2gYGgkdaYQsiZZ72aGXQ==
-----END PUBLIC KEY-----";

    fn provider(bundle_id: Option<&str>) -> AppleProvider {
        AppleProvider::new(AppleConfig {
            services_id: "app.kuzz.signin".into(),
            bundle_id: bundle_id.map(str::to_owned),
            team_id: "TEAM123456".into(),
            key_id: "KEY1234567".into(),
            private_key_pem: TEST_KEY.into(),
        })
        .expect("provider")
    }

    #[test]
    fn authorization_url_posts_the_callback_and_asks_for_name_and_email() {
        let url = provider(None).authorization_url("https://relay.example/cb", "st", "nn");
        assert!(url.starts_with(APPLE_AUTH_ENDPOINT));
        assert!(url.contains("client_id=app.kuzz.signin"));
        assert!(url.contains("response_mode=form_post"));
        assert!(url.contains("scope=name+email"));
        assert!(url.contains("state=st"));
        assert!(url.contains("nonce=nn"));
    }

    #[test]
    fn client_secret_is_an_es256_jwt_for_apple() {
        let provider = provider(None);
        let secret = provider.client_secret().expect("secret");
        let header = jsonwebtoken::decode_header(&secret).expect("header");
        assert_eq!(header.alg, jsonwebtoken::Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some("KEY1234567"));
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
        validation.set_audience(&[APPLE_ISSUER]);
        validation.set_issuer(&["TEAM123456"]);
        let claims = jsonwebtoken::decode::<serde_json::Value>(
            &secret,
            &jsonwebtoken::DecodingKey::from_ec_pem(TEST_PUBLIC_KEY.as_bytes()).unwrap(),
            &validation,
        )
        .expect("claims")
        .claims;
        assert_eq!(claims["sub"], "app.kuzz.signin");
        assert!(!format!("{provider:?}").contains("PRIVATE KEY"));
    }

    #[test]
    fn app_sign_in_needs_a_bundle_id() {
        assert_eq!(provider(None).app_sign_in_client_id(), None);
        assert_eq!(
            provider(Some("app.kuzz.mobile")).app_sign_in_client_id(),
            Some("app.kuzz.mobile")
        );
    }

    #[test]
    fn first_sign_in_name_joins_the_parts() {
        assert_eq!(
            first_sign_in_name(r#"{"name":{"firstName":"Ji","lastName":"Ho"},"email":"a@b"}"#),
            Some("Ji Ho".into())
        );
        assert_eq!(
            first_sign_in_name(r#"{"name":{"firstName":" Ji "}}"#),
            Some("Ji".into())
        );
        assert_eq!(first_sign_in_name(r#"{"email":"a@b"}"#), None);
        assert_eq!(first_sign_in_name("not json"), None);
    }

    #[test]
    fn token_response_keeps_the_refresh_token_when_sent() {
        let with: TokenResponse =
            serde_json::from_str(r#"{"id_token":"x","refresh_token":"r.1","access_token":"a"}"#)
                .unwrap();
        assert_eq!(with.refresh_token.as_deref(), Some("r.1"));
        let without: TokenResponse = serde_json::from_str(r#"{"id_token":"x"}"#).unwrap();
        assert_eq!(without.refresh_token, None);
        assert!(!format!("{:?}", RevokeToken::new("r.1".into())).contains("r.1"));
    }

    #[test]
    fn a_non_ec_key_is_refused() {
        let result = AppleProvider::new(AppleConfig {
            private_key_pem: "not a key".into(),
            ..provider(None).config
        });
        assert!(result.is_err());
    }
}
