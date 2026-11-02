//! APNs envelope construction, endpoint encryption, and response classification.

use std::{sync::Mutex, time::Duration};

use async_trait::async_trait;
use reqwest::{header::CONTENT_TYPE, StatusCode};
use serde::Deserialize;
use thiserror::Error;

use crate::{config::ApnsEnvironment, model::APNS_RECONNECT_PAYLOAD};

/// Sanitized delivery outcome. Raw provider bodies never cross this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// APNs accepted the request (not proof of device delivery).
    Accepted,
    /// This endpoint generation is permanently invalid. APNs may provide the time it became invalid.
    InvalidEndpoint {
        /// APNs' timestamp for when the endpoint became invalid, if supplied.
        unregistered_at: Option<i64>,
    },
    /// A bounded retry is safe. A sanitized server hint may raise the delay.
    Retry {
        /// Retry-After delay in seconds, clamped by the transport.
        retry_after_seconds: Option<i64>,
    },
    /// Provider credential/profile configuration is unhealthy; do not invalidate endpoints.
    ConfigurationFault,
    /// The locally-generated request is permanently invalid.
    PermanentRequestFault,
}

/// Classify APNs status/reason without conflating provider faults with endpoints.
pub fn classify(code: u16, reason: Option<&str>, timestamp: Option<i64>) -> DeliveryOutcome {
    match (code, reason) {
        (200, _) => DeliveryOutcome::Accepted,
        (410, Some("Unregistered")) => DeliveryOutcome::InvalidEndpoint {
            unregistered_at: timestamp,
        },
        // Both reasons are ambiguous with deployment profile mistakes: APNs
        // uses BadDeviceToken for environment mismatches and
        // DeviceTokenNotForTopic for topic mismatches. Only Unregistered
        // crosses the permanent endpoint-invalidation boundary.
        (400, Some("BadDeviceToken" | "DeviceTokenNotForTopic")) => {
            DeliveryOutcome::ConfigurationFault
        }
        (403, _) | (429, Some("TooManyProviderTokenUpdates")) => {
            DeliveryOutcome::ConfigurationFault
        }
        (429 | 500 | 503, _)
        | (
            _,
            Some(
                "IdleTimeout"
                | "InternalServerError"
                | "ServiceUnavailable"
                | "Shutdown"
                | "TooManyRequests",
            ),
        ) => DeliveryOutcome::Retry {
            retry_after_seconds: None,
        },
        _ => DeliveryOutcome::PermanentRequestFault,
    }
}

/// Closed APNs transport controls. No field can be serialized into application
/// content; the concrete transport always uses `APNS_RECONNECT_PAYLOAD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliveryAttempt {
    pub request_id: uuid::Uuid,
    pub expires_at: i64,
}

/// APNs sender abstraction for live-validation tests.
#[async_trait]
pub trait PushTransport: Send + Sync {
    /// Send one durable job.
    async fn send(&self, attempt: DeliveryAttempt, endpoint: &str) -> DeliveryOutcome;
}

/// Provider tokens are reused for this long. Apple rejects tokens older than one
/// hour and throttles refreshes more often than every 20 minutes.
const PROVIDER_TOKEN_REFRESH_SECONDS: i64 = 50 * 60;

#[derive(serde::Serialize)]
struct ProviderTokenClaims<'a> {
    iss: &'a str,
    iat: i64,
}

/// ES256 provider-token signer for Apple `.p8` keys, with a cached current token.
struct ProviderTokenSigner {
    key: jsonwebtoken::EncodingKey,
    key_id: String,
    team_id: String,
    cached: Mutex<Option<(i64, String)>>,
}

impl ProviderTokenSigner {
    fn sign(&self, issued_at: i64) -> Result<String, ApnsError> {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        jsonwebtoken::encode(
            &header,
            &ProviderTokenClaims {
                iss: &self.team_id,
                iat: issued_at,
            },
            &self.key,
        )
        .map_err(|_| ApnsError::Credential)
    }

    fn current(&self, now: i64) -> Result<String, ApnsError> {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((issued_at, token)) = cached.as_ref() {
            if (0..PROVIDER_TOKEN_REFRESH_SECONDS).contains(&(now - issued_at)) {
                return Ok(token.clone());
            }
        }
        let token = self.sign(now)?;
        *cached = Some((now, token.clone()));
        Ok(token)
    }

    fn discard(&self, token: &str) {
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if cached.as_ref().is_some_and(|(_, current)| current == token) {
            *cached = None;
        }
    }
}

/// Direct HTTP/2 APNs transport using a client certificate identity or an
/// Apple `.p8` provider-token key.
pub struct ApnsTransport {
    client: reqwest::Client,
    topic: String,
    base_url: String,
    provider_token: Option<ProviderTokenSigner>,
}

fn base_url(environment: ApnsEnvironment) -> &'static str {
    match environment {
        ApnsEnvironment::Production => "https://api.push.apple.com",
        ApnsEnvironment::Sandbox => "https://api.sandbox.push.apple.com",
    }
}

fn http2_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        // APNs requires HTTP/2. This no-op method reference is intentionally
        // feature-gated so removing reqwest's `http2` feature fails the build.
        .http2_keep_alive_while_idle(false)
        .timeout(Duration::from_secs(15))
}

impl ApnsTransport {
    /// Build a reusable APNs client from a combined PEM private key and certificate.
    pub fn certificate(
        identity_pem: &[u8],
        topic: String,
        environment: ApnsEnvironment,
    ) -> Result<Self, ApnsError> {
        Self::certificate_with_base_url(identity_pem, topic, base_url(environment).to_owned())
    }

    /// Build a reusable APNs client from an Apple `.p8` signing key (PKCS#8 P-256).
    pub fn token(
        key_pem: &[u8],
        key_id: String,
        team_id: String,
        topic: String,
        environment: ApnsEnvironment,
    ) -> Result<Self, ApnsError> {
        Self::token_with_base_url(
            key_pem,
            key_id,
            team_id,
            topic,
            base_url(environment).to_owned(),
        )
    }

    fn token_with_base_url(
        key_pem: &[u8],
        key_id: String,
        team_id: String,
        topic: String,
        base_url: String,
    ) -> Result<Self, ApnsError> {
        let key =
            jsonwebtoken::EncodingKey::from_ec_pem(key_pem).map_err(|_| ApnsError::Credential)?;
        let signer = ProviderTokenSigner {
            key,
            key_id,
            team_id,
            cached: Mutex::new(None),
        };
        // Sign once at startup so a non-P-256 or otherwise unusable key fails
        // here rather than as configuration faults on live deliveries.
        signer.sign(chrono::Utc::now().timestamp())?;
        let client = http2_client().build().map_err(|_| ApnsError::Credential)?;
        Ok(Self {
            client,
            topic,
            base_url,
            provider_token: Some(signer),
        })
    }

    fn certificate_with_base_url(
        identity_pem: &[u8],
        topic: String,
        base_url: String,
    ) -> Result<Self, ApnsError> {
        let identity =
            reqwest::Identity::from_pem(identity_pem).map_err(|_| ApnsError::Credential)?;
        let client = http2_client()
            .identity(identity)
            // Identity validation completes while the TLS client is built, so a
            // malformed or mismatched certificate/key pair is a credential error.
            .build()
            .map_err(|_| ApnsError::Credential)?;
        Ok(Self {
            client,
            topic,
            base_url,
            provider_token: None,
        })
    }

    fn request(
        &self,
        attempt: DeliveryAttempt,
        endpoint: &str,
        bearer: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let request = self
            .client
            .post(format!("{}/3/device/{endpoint}", self.base_url));
        let request = match bearer {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        request
            .header(CONTENT_TYPE, "application/json")
            .header("apns-id", attempt.request_id.to_string())
            .header("apns-topic", &self.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .header("apns-expiration", attempt.expires_at.to_string())
            // This is the only APNs application body in the program. It is a
            // byte constant, not a serialization of the relay request, grant,
            // endpoint, headers, route, provider response, or any generic JSON map.
            .body(APNS_RECONNECT_PAYLOAD)
    }

    async fn send_response(
        &self,
        attempt: DeliveryAttempt,
        endpoint: &str,
        bearer: Option<&str>,
    ) -> Result<reqwest::Response, reqwest::Error> {
        self.request(attempt, endpoint, bearer).send().await
    }
}

/// APNs transport setup failure. It intentionally carries no credential material.
#[derive(Debug, Error)]
pub enum ApnsError {
    /// Invalid client certificate identity material.
    #[error("invalid APNs credential")]
    Credential,
}

#[derive(Deserialize)]
struct ApnsErrorBody {
    reason: Option<String>,
    timestamp: Option<i64>,
}

#[async_trait]
impl PushTransport for ApnsTransport {
    async fn send(&self, attempt: DeliveryAttempt, endpoint: &str) -> DeliveryOutcome {
        crate::metrics::record_apns_send_attempt();
        let bearer = match &self.provider_token {
            Some(signer) => match signer.current(chrono::Utc::now().timestamp()) {
                Ok(token) => Some(token),
                Err(_) => return DeliveryOutcome::ConfigurationFault,
            },
            None => None,
        };
        let response = self
            .send_response(attempt, endpoint, bearer.as_deref())
            .await;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                return DeliveryOutcome::Retry {
                    retry_after_seconds: None,
                }
            }
        };
        if response.status() == StatusCode::OK {
            return DeliveryOutcome::Accepted;
        }
        let code = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<i64>().ok())
            .map(|seconds| seconds.clamp(1, 3600));
        let detail = response.json::<ApnsErrorBody>().await.ok();
        let timestamp = detail.as_ref().and_then(|d| d.timestamp);
        // A rejected token is never reused; the next attempt signs a fresh one.
        if let (Some(signer), Some(token), Some("ExpiredProviderToken" | "InvalidProviderToken")) = (
            &self.provider_token,
            bearer.as_deref(),
            detail.as_ref().and_then(|d| d.reason.as_deref()),
        ) {
            signer.discard(token);
        }
        match classify(
            code,
            detail.as_ref().and_then(|d| d.reason.as_deref()),
            timestamp,
        ) {
            DeliveryOutcome::Retry { .. } => DeliveryOutcome::Retry {
                retry_after_seconds: retry_after,
            },
            outcome => outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
        Router,
    };
    use std::sync::{Arc, Mutex};

    // Self-signed test-only identity material. None of these are Apple credentials.
    const TEST_IDENTITY_PEM: &[u8] = include_bytes!("../tests/fixtures/apns-test-identity.pem");
    const TEST_CERT_ONLY_PEM: &[u8] = include_bytes!("../tests/fixtures/apns-test-cert-only.pem");
    const TEST_KEY_ONLY_PEM: &[u8] = include_bytes!("../tests/fixtures/apns-test-key-only.pem");
    const TEST_ENCRYPTED_IDENTITY_PEM: &[u8] =
        include_bytes!("../tests/fixtures/apns-test-encrypted-identity.pem");
    const TEST_MISMATCHED_IDENTITY_PEM: &[u8] =
        include_bytes!("../tests/fixtures/apns-test-mismatched-identity.pem");

    #[derive(Default)]
    struct CapturedRequest {
        headers: HeaderMap,
        body: Vec<u8>,
    }

    async fn capture_request(
        State(requests): State<Arc<Mutex<Vec<CapturedRequest>>>>,
        headers: HeaderMap,
        body: Bytes,
    ) -> StatusCode {
        requests.lock().unwrap().push(CapturedRequest {
            headers,
            body: body.to_vec(),
        });
        StatusCode::OK
    }

    #[tokio::test]
    async fn certificate_transport_sends_no_bearer_and_exact_body_for_every_attempt() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/3/device/{endpoint}", post(capture_request))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let transport = ApnsTransport::certificate_with_base_url(
            TEST_IDENTITY_PEM,
            "app.topic".to_owned(),
            base_url,
        )
        .unwrap();
        for (request_id, expires_at, endpoint) in [
            (uuid::Uuid::nil(), 1, "00".repeat(32)),
            (uuid::Uuid::max(), i64::MAX, "ff".repeat(32)),
        ] {
            assert_eq!(
                transport
                    .send(
                        DeliveryAttempt {
                            request_id,
                            expires_at,
                        },
                        &endpoint,
                    )
                    .await,
                DeliveryOutcome::Accepted
            );
        }
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert!(captured
            .iter()
            .all(|request| request.body.as_slice() == APNS_RECONNECT_PAYLOAD));
        assert!(captured
            .iter()
            .all(|request| !request.headers.contains_key(reqwest::header::AUTHORIZATION)));
        assert!(captured.iter().all(|request| request
            .headers
            .get("apns-topic")
            .is_some_and(|topic| topic == "app.topic")));
    }

    #[tokio::test]
    #[ignore = "requires the exported dogfood Apple Push Services PEM"]
    async fn live_sandbox_probe_reports_literal_status_and_body() {
        let cert_path = std::env::var("KUZZ_PUSH_LIVE_APNS_CERT_PATH")
            .expect("set KUZZ_PUSH_LIVE_APNS_CERT_PATH to the dogfood identity PEM");
        let topic = std::env::var("KUZZ_PUSH_LIVE_APNS_TOPIC")
            .expect("set KUZZ_PUSH_LIVE_APNS_TOPIC to the dogfood bundle id");
        let identity = std::fs::read(cert_path).unwrap();
        let transport =
            ApnsTransport::certificate(&identity, topic, ApnsEnvironment::Sandbox).unwrap();
        let response = transport
            .send_response(
                DeliveryAttempt {
                    request_id: uuid::Uuid::nil(),
                    expires_at: chrono::Utc::now().timestamp() + 60,
                },
                &"00".repeat(32),
                None,
            )
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        eprintln!("live APNs response: status={status}, body={body}");
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(body, r#"{"reason":"BadDeviceToken"}"#);
    }

    fn test_p256_key() -> rcgen::KeyPair {
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap()
    }

    fn pem(label: &str, der: &[u8]) -> String {
        use base64::Engine as _;
        let body = base64::engine::general_purpose::STANDARD.encode(der);
        let lines: Vec<&str> = body
            .as_bytes()
            .chunks(64)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect();
        format!(
            "-----BEGIN {label}-----
{}
-----END {label}-----
",
            lines.join(
                "
"
            )
        )
    }

    fn private_pem(key: &rcgen::KeyPair) -> String {
        pem("PRIVATE KEY", &key.serialize_der())
    }

    #[tokio::test]
    async fn token_transport_sends_cached_es256_bearer_and_exact_body() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/3/device/{endpoint}", post(capture_request))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let key = test_p256_key();
        let transport = ApnsTransport::token_with_base_url(
            private_pem(&key).as_bytes(),
            "ABCDE12345".to_owned(),
            "TEAM123456".to_owned(),
            "app.topic".to_owned(),
            base_url,
        )
        .unwrap();
        for _ in 0..2 {
            assert_eq!(
                transport
                    .send(
                        DeliveryAttempt {
                            request_id: uuid::Uuid::nil(),
                            expires_at: 1,
                        },
                        &"00".repeat(32),
                    )
                    .await,
                DeliveryOutcome::Accepted
            );
        }
        let captured = requests.lock().unwrap();
        assert_eq!(captured.len(), 2);
        assert!(captured
            .iter()
            .all(|request| request.body.as_slice() == APNS_RECONNECT_PAYLOAD));
        let bearers: Vec<&str> = captured
            .iter()
            .map(|request| {
                request.headers[reqwest::header::AUTHORIZATION]
                    .to_str()
                    .unwrap()
                    .strip_prefix("Bearer ")
                    .unwrap()
            })
            .collect();
        assert_eq!(
            bearers[0], bearers[1],
            "token is reused, not re-minted per send"
        );

        let header = jsonwebtoken::decode_header(bearers[0]).unwrap();
        assert_eq!(header.alg, jsonwebtoken::Algorithm::ES256);
        assert_eq!(header.kid.as_deref(), Some("ABCDE12345"));
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::ES256);
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        let claims = jsonwebtoken::decode::<serde_json::Value>(
            bearers[0],
            &jsonwebtoken::DecodingKey::from_ec_pem(
                pem(
                    "PUBLIC KEY",
                    &rcgen::PublicKeyData::subject_public_key_info(&key),
                )
                .as_bytes(),
            )
            .unwrap(),
            &validation,
        )
        .unwrap()
        .claims;
        assert_eq!(claims["iss"], "TEAM123456");
        let iat = claims["iat"].as_i64().unwrap();
        assert!((chrono::Utc::now().timestamp() - iat).abs() < 60);
        assert_eq!(claims.as_object().unwrap().len(), 2, "only iss and iat");
    }

    #[test]
    fn provider_token_refreshes_after_fifty_minutes_and_after_rejection() {
        let signer = ProviderTokenSigner {
            key: jsonwebtoken::EncodingKey::from_ec_pem(private_pem(&test_p256_key()).as_bytes())
                .unwrap(),
            key_id: "ABCDE12345".into(),
            team_id: "TEAM123456".into(),
            cached: Mutex::new(None),
        };
        let first = signer.current(1_000).unwrap();
        assert_eq!(signer.current(1_000 + 49 * 60).unwrap(), first);
        let refreshed = signer.current(1_000 + 50 * 60).unwrap();
        assert_ne!(refreshed, first);
        // A token from a clock that moved backwards is not trusted either.
        assert_ne!(signer.current(900).unwrap(), refreshed);

        let current = signer.current(900).unwrap();
        signer.discard("some other token");
        assert_eq!(signer.current(900).unwrap(), current);
        signer.discard(&current);
        assert!(signer.cached.lock().unwrap().is_none());
    }

    #[test]
    fn unusable_p8_keys_fail_as_credential_errors() {
        let p384 =
            private_pem(&rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384).unwrap());
        for key in [
            b"".as_slice(),
            b"not a PEM key",
            TEST_CERT_ONLY_PEM,
            p384.as_bytes(),
        ] {
            assert!(matches!(
                ApnsTransport::token(
                    key,
                    "ABCDE12345".to_owned(),
                    "TEAM123456".to_owned(),
                    "app.topic".to_owned(),
                    ApnsEnvironment::Production,
                ),
                Err(ApnsError::Credential)
            ));
        }
    }

    #[tokio::test]
    #[ignore = "requires an Apple APNs .p8 key"]
    async fn live_token_probe_reports_literal_status_and_body() {
        let env = |key: &str| std::env::var(key).unwrap_or_else(|_| panic!("set {key}"));
        let environment = match env("KUZZ_PUSH_LIVE_APNS_ENVIRONMENT").as_str() {
            "sandbox" => ApnsEnvironment::Sandbox,
            _ => ApnsEnvironment::Production,
        };
        let transport = ApnsTransport::token(
            &std::fs::read(env("KUZZ_PUSH_LIVE_APNS_KEY_PATH")).unwrap(),
            env("KUZZ_PUSH_LIVE_APNS_KEY_ID"),
            env("KUZZ_PUSH_LIVE_APNS_TEAM_ID"),
            env("KUZZ_PUSH_LIVE_APNS_TOPIC"),
            environment,
        )
        .unwrap();
        let bearer = transport
            .provider_token
            .as_ref()
            .unwrap()
            .current(chrono::Utc::now().timestamp())
            .unwrap();
        let response = transport
            .send_response(
                DeliveryAttempt {
                    request_id: uuid::Uuid::nil(),
                    expires_at: chrono::Utc::now().timestamp() + 60,
                },
                &"00".repeat(32),
                Some(&bearer),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        eprintln!("live APNs response: status={status}, body={body}");
        // An accepted key reaches device-token validation; a bad key gets 403.
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(body, r#"{"reason":"BadDeviceToken"}"#);
    }

    #[test]
    fn empty_certificate_identity_fails_as_a_credential_error() {
        assert_credential_error(b"");
    }

    #[test]
    fn malformed_certificate_identity_fails_as_a_credential_error() {
        assert_credential_error(b"not a PEM identity");
    }

    #[test]
    fn certificate_without_private_key_fails_as_a_credential_error() {
        assert_credential_error(TEST_CERT_ONLY_PEM);
    }

    #[test]
    fn private_key_without_certificate_fails_as_a_credential_error() {
        assert_credential_error(TEST_KEY_ONLY_PEM);
    }

    #[test]
    fn encrypted_private_key_fails_as_a_credential_error() {
        assert_credential_error(TEST_ENCRYPTED_IDENTITY_PEM);
    }

    #[test]
    fn mismatched_private_key_fails_as_a_credential_error() {
        // reqwest parses both PEM blocks, then rejects the mismatched pair while
        // building the TLS client. This locks the ClientBuilder error mapping.
        assert_credential_error(TEST_MISMATCHED_IDENTITY_PEM);
    }

    fn assert_credential_error(identity_pem: &[u8]) {
        assert!(matches!(
            ApnsTransport::certificate(
                identity_pem,
                "app.topic".to_owned(),
                ApnsEnvironment::Production,
            ),
            Err(ApnsError::Credential)
        ));
    }

    #[test]
    fn response_classes_do_not_massacre_endpoints_on_provider_faults() {
        assert_eq!(
            classify(410, Some("Unregistered"), Some(7)),
            DeliveryOutcome::InvalidEndpoint {
                unregistered_at: Some(7)
            }
        );
        for reason in ["InvalidProviderToken", "ExpiredProviderToken"] {
            assert_eq!(
                classify(403, Some(reason), None),
                DeliveryOutcome::ConfigurationFault
            );
        }
        for reason in ["BadDeviceToken", "DeviceTokenNotForTopic"] {
            assert_eq!(
                classify(400, Some(reason), None),
                DeliveryOutcome::ConfigurationFault
            );
        }
        assert_eq!(
            classify(429, Some("TooManyRequests"), None),
            DeliveryOutcome::Retry {
                retry_after_seconds: None
            }
        );
        assert_eq!(
            classify(400, Some("BadTopic"), None),
            DeliveryOutcome::PermanentRequestFault
        );
    }
}
