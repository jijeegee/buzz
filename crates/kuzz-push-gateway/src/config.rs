use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::{collections::HashMap, net::SocketAddr, path::PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApnsEnvironment {
    Production,
    Sandbox,
}

/// Server-owned APNs provider credential. Exactly one form is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApnsCredential {
    /// Combined client certificate and private key PEM (certificate-based trust).
    Certificate { path: PathBuf },
    /// Apple `.p8` signing key for provider authentication tokens (token-based trust).
    Token {
        key_path: PathBuf,
        key_id: String,
        team_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct AppProfileConfig {
    pub app_attest_app_id: String,
    /// Exact Apple attestation environment accepted for enrollment.
    pub app_attest_environment: AppAttestEnvironment,
    pub apns_credential: ApnsCredential,
    pub apns_topic: String,
    pub apns_environment: ApnsEnvironment,
}

/// Apple App Attest environment, independent of the APNs transport environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAttestEnvironment {
    /// Distributed applications, also the default for personal development builds.
    Production,
    /// Development-signed applications in an explicitly opted-in gateway build.
    #[cfg(feature = "personal-dev-app-attest")]
    Development,
}

/// Optional `kuzz-android` profile. Present only when an FCM service account
/// is configured; the attestation settings are then required.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidProfileConfig {
    /// Google service-account JSON key file used for FCM HTTP v1.
    pub fcm_service_account_path: PathBuf,
    /// Android application id attested at enrollment (`app.kuzz.mobile`).
    pub package: String,
    /// SHA-256 digests of the allowed app signing certificates.
    pub signing_digests: Vec<[u8; 32]>,
    /// PEM bundle of Google hardware attestation roots.
    pub attestation_roots_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyConfig {
    pub id: String,
    pub key: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub health_addr: SocketAddr,
    pub max_grant_lifetime_seconds: i64,
    pub max_installation_lifetime_seconds: i64,
    pub endpoint_quota_window_seconds: i64,
    pub endpoint_quota_max_deliveries: i64,
    /// Server-owned dogfood application identity and APNs transport.
    pub profile: AppProfileConfig,
    /// Android profile; `None` keeps the gateway iOS-only.
    pub android: Option<AndroidProfileConfig>,
    pub database_url: String,
    pub app_attest_root_cert_path: PathBuf,
    /// Ordered current key first, followed by decrypt-only predecessors.
    pub grant_keys: Vec<KeyConfig>,
    /// Independent token-custody keyring. These keys MUST NOT be reused for
    /// externally presented delivery capabilities.
    pub token_keys: Vec<KeyConfig>,
}

// Registered NIP-PL v1 App Attest audiences are protocol constants, not routing URLs.
/// Fixed NIP-PL v1 App Attest audience for installation enrollment.
pub const ENROLL_AUDIENCE: &str = "https://push.buzz.xyz/v1/installations";
/// Fixed NIP-PL v1 App Attest audience for creating a delegation.
pub const DELEGATE_AUDIENCE: &str = "https://push.buzz.xyz/v1/delegations";
/// Fixed NIP-PL v1 App Attest audience for rotating an installation endpoint.
pub const ROTATE_ENDPOINT_AUDIENCE: &str = "https://push.buzz.xyz/v1/installations/endpoint";
/// Fixed NIP-PL v1 App Attest audience for revoking a delegation.
pub const REVOKE_DELEGATION_AUDIENCE: &str = "https://push.buzz.xyz/v1/delegations/revoke";
/// Fixed NIP-PL v1 App Attest audience for revoking an installation.
pub const REVOKE_INSTALLATION_AUDIENCE: &str = "https://push.buzz.xyz/v1/installations/revoke";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid environment variable {0}")]
    Invalid(&'static str),
}
fn parse_keyring(
    e: &HashMap<String, String>,
    variable: &'static str,
) -> Result<Vec<KeyConfig>, ConfigError> {
    let value = e
        .get(variable)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Missing(variable))?;
    let keys = value
        .split(',')
        .map(|entry| {
            let (id, encoded) = entry
                .split_once(':')
                .filter(|(id, encoded)| !id.is_empty() && !encoded.is_empty())
                .ok_or(ConfigError::Invalid(variable))?;
            let key = STANDARD
                .decode(encoded)
                .map_err(|_| ConfigError::Invalid(variable))?;
            if key.len() != 32 {
                return Err(ConfigError::Invalid(variable));
            }
            Ok(KeyConfig {
                id: id.to_owned(),
                key,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if keys.is_empty() {
        return Err(ConfigError::Invalid(variable));
    }
    Ok(keys)
}

fn parse_profile(e: &HashMap<String, String>) -> Result<AppProfileConfig, ConfigError> {
    let app_id_key = "KUZZ_PUSH_DOGFOOD_APP_ATTEST_APP_ID";
    let cert_key = "KUZZ_PUSH_DOGFOOD_APNS_CERT_PATH";
    let key_path_key = "KUZZ_PUSH_DOGFOOD_APNS_KEY_PATH";
    let key_id_key = "KUZZ_PUSH_DOGFOOD_APNS_KEY_ID";
    let team_id_key = "KUZZ_PUSH_DOGFOOD_APNS_TEAM_ID";
    let topic_key = "KUZZ_PUSH_DOGFOOD_APNS_TOPIC";
    let environment_key = "KUZZ_PUSH_DOGFOOD_APNS_ENVIRONMENT";
    let required = |key: &'static str| {
        e.get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(ConfigError::Missing(key))
    };
    let app_attest_app_id = required(app_id_key)?.to_owned();
    let app_attest_environment = match e
        .get("KUZZ_PUSH_APP_ATTEST_ENVIRONMENT")
        .map(String::as_str)
    {
        None | Some("production") => AppAttestEnvironment::Production,
        #[cfg(feature = "personal-dev-app-attest")]
        Some("development") => AppAttestEnvironment::Development,
        Some(_) => return Err(ConfigError::Invalid("KUZZ_PUSH_APP_ATTEST_ENVIRONMENT")),
    };
    let apns_topic = required(topic_key)?.to_owned();
    let present = |key: &str| e.get(key).is_some_and(|value| !value.is_empty());
    // Apple key and team identifiers are 10-character uppercase alphanumerics.
    let apple_id = |key: &'static str| {
        let value = required(key)?;
        if value.len() == 10
            && value
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            Ok(value.to_owned())
        } else {
            Err(ConfigError::Invalid(key))
        }
    };
    let apns_credential = if present(key_path_key) {
        if present(cert_key) {
            return Err(ConfigError::Invalid(key_path_key));
        }
        ApnsCredential::Token {
            key_path: PathBuf::from(required(key_path_key)?),
            key_id: apple_id(key_id_key)?,
            team_id: apple_id(team_id_key)?,
        }
    } else {
        ApnsCredential::Certificate {
            path: PathBuf::from(required(cert_key)?),
        }
    };
    let apns_environment = match e.get(environment_key).map(String::as_str) {
        None | Some("production") => ApnsEnvironment::Production,
        Some("sandbox") => ApnsEnvironment::Sandbox,
        Some(_) => return Err(ConfigError::Invalid(environment_key)),
    };
    Ok(AppProfileConfig {
        app_attest_app_id,
        app_attest_environment,
        apns_credential,
        apns_topic,
        apns_environment,
    })
}

fn parse_android(e: &HashMap<String, String>) -> Result<Option<AndroidProfileConfig>, ConfigError> {
    let key = "KUZZ_PUSH_ANDROID_FCM_SERVICE_ACCOUNT_PATH";
    let fcm_service_account_path = match e.get(key).map(String::as_str) {
        None => return Ok(None),
        Some("") => return Err(ConfigError::Invalid(key)),
        Some(path) => PathBuf::from(path),
    };
    let required = |key: &'static str| {
        e.get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(ConfigError::Missing(key))
    };
    let package_key = "KUZZ_PUSH_ANDROID_PACKAGE";
    let package = required(package_key)?;
    if package.len() > 255
        || !package
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
    {
        return Err(ConfigError::Invalid(package_key));
    }
    let digests_key = "KUZZ_PUSH_ANDROID_SIGNING_CERT_SHA256";
    let signing_digests = required(digests_key)?
        .split(',')
        .map(|entry| {
            let hex_digest: String = entry.trim().chars().filter(|c| *c != ':').collect();
            hex::decode(hex_digest)
                .ok()
                .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
                .ok_or(ConfigError::Invalid(digests_key))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(AndroidProfileConfig {
        fcm_service_account_path,
        package: package.to_owned(),
        signing_digests,
        attestation_roots_path: PathBuf::from(required(
            "KUZZ_PUSH_ANDROID_ATTESTATION_ROOTS_PATH",
        )?),
    }))
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_map(&std::env::vars().collect())
    }
    pub fn from_map(e: &HashMap<String, String>) -> Result<Self, ConfigError> {
        fn req<'a>(
            e: &'a HashMap<String, String>,
            k: &'static str,
        ) -> Result<&'a str, ConfigError> {
            e.get(k)
                .map(String::as_str)
                .filter(|v| !v.is_empty())
                .ok_or(ConfigError::Missing(k))
        }
        let grant_keys = parse_keyring(e, "KUZZ_PUSH_GRANT_KEYS")?;
        let token_keys = parse_keyring(e, "KUZZ_PUSH_TOKEN_KEYS")?;
        if grant_keys.iter().any(|grant| {
            token_keys
                .iter()
                .any(|token| grant.id == token.id || grant.key == token.key)
        }) {
            return Err(ConfigError::Invalid("KUZZ_PUSH_TOKEN_KEYS"));
        }
        let max_grant_lifetime_seconds = req(e, "KUZZ_PUSH_MAX_GRANT_LIFETIME_SECONDS")?
            .parse::<i64>()
            .ok()
            .filter(|seconds| (1..=31_536_000).contains(seconds))
            .ok_or(ConfigError::Invalid("KUZZ_PUSH_MAX_GRANT_LIFETIME_SECONDS"))?;
        let max_installation_lifetime_seconds = e
            .get("KUZZ_PUSH_MAX_INSTALLATION_LIFETIME_SECONDS")
            .map(String::as_str)
            .unwrap_or("7776000")
            .parse::<i64>()
            .ok()
            .filter(|seconds| (1..=31_536_000).contains(seconds))
            .ok_or(ConfigError::Invalid(
                "KUZZ_PUSH_MAX_INSTALLATION_LIFETIME_SECONDS",
            ))?;
        let bounded_positive = |key: &'static str, default: i64, max: i64| {
            e.get(key)
                .map(String::as_str)
                .unwrap_or("")
                .parse::<i64>()
                .ok()
                .or((!e.contains_key(key)).then_some(default))
                .filter(|value| (1..=max).contains(value))
                .ok_or(ConfigError::Invalid(key))
        };
        let endpoint_quota_window_seconds =
            bounded_positive("KUZZ_PUSH_ENDPOINT_QUOTA_WINDOW_SECONDS", 10, 86_400)?;
        let endpoint_quota_max_deliveries =
            bounded_positive("KUZZ_PUSH_ENDPOINT_QUOTA_MAX_DELIVERIES", 10, 10_000)?;
        let profile = parse_profile(e)?;
        let android = parse_android(e)?;
        let bind_addr = e
            .get("KUZZ_PUSH_BIND_ADDR")
            .map(String::as_str)
            .unwrap_or("0.0.0.0:8080")
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::Invalid("KUZZ_PUSH_BIND_ADDR"))?;
        let health_addr = e
            .get("KUZZ_PUSH_HEALTH_ADDR")
            .map(String::as_str)
            .unwrap_or("0.0.0.0:8081")
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::Invalid("KUZZ_PUSH_HEALTH_ADDR"))?;
        Ok(Self {
            bind_addr,
            health_addr,
            max_grant_lifetime_seconds,
            max_installation_lifetime_seconds,
            endpoint_quota_window_seconds,
            endpoint_quota_max_deliveries,
            profile,
            android,
            database_url: req(e, "DATABASE_URL")?.to_owned(),
            app_attest_root_cert_path: req(e, "KUZZ_PUSH_APP_ATTEST_ROOT_CERT_PATH")?.into(),
            grant_keys,
            token_keys,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn attestation_environment_is_explicit_and_build_gated() {
        let mut env = base();
        assert_eq!(
            Config::from_map(&env)
                .unwrap()
                .profile
                .app_attest_environment,
            AppAttestEnvironment::Production
        );
        for value in ["", "sandbox", "Production", "unknown"] {
            env.insert("KUZZ_PUSH_APP_ATTEST_ENVIRONMENT".into(), value.into());
            assert!(Config::from_map(&env).is_err());
        }
        env.insert(
            "KUZZ_PUSH_APP_ATTEST_ENVIRONMENT".into(),
            "production".into(),
        );
        assert_eq!(
            Config::from_map(&env)
                .unwrap()
                .profile
                .app_attest_environment,
            AppAttestEnvironment::Production
        );
        env.insert(
            "KUZZ_PUSH_APP_ATTEST_ENVIRONMENT".into(),
            "development".into(),
        );
        #[cfg(feature = "personal-dev-app-attest")]
        assert_eq!(
            Config::from_map(&env)
                .unwrap()
                .profile
                .app_attest_environment,
            AppAttestEnvironment::Development
        );
        #[cfg(not(feature = "personal-dev-app-attest"))]
        assert!(Config::from_map(&env).is_err());
    }

    fn base() -> HashMap<String, String> {
        HashMap::from([
            (
                "KUZZ_PUSH_GRANT_KEYS".into(),
                format!(
                    "current:{},old:{}",
                    STANDARD.encode([1; 32]),
                    STANDARD.encode([2; 32])
                ),
            ),
            (
                "KUZZ_PUSH_TOKEN_KEYS".into(),
                format!(
                    "current-token:{},old-token:{}",
                    STANDARD.encode([3; 32]),
                    STANDARD.encode([4; 32])
                ),
            ),
            (
                "KUZZ_PUSH_MAX_GRANT_LIFETIME_SECONDS".into(),
                "2592000".into(),
            ),
            (
                "DATABASE_URL".into(),
                "postgres://kuzz:test@localhost/kuzz".into(), // sadscan:disable np.postgres.1
            ),
            (
                "KUZZ_PUSH_DOGFOOD_APP_ATTEST_APP_ID".into(),
                "TEAM.app.kuzz.dogfood.mobile".into(),
            ),
            (
                "KUZZ_PUSH_APP_ATTEST_ROOT_CERT_PATH".into(),
                "/apple-root.pem".into(),
            ),
            (
                "KUZZ_PUSH_DOGFOOD_APNS_CERT_PATH".into(),
                "/dogfood-identity.pem".into(),
            ),
            (
                "KUZZ_PUSH_DOGFOOD_APNS_TOPIC".into(),
                "app.kuzz.dogfood.mobile".into(),
            ),
            (
                "KUZZ_PUSH_DOGFOOD_APNS_ENVIRONMENT".into(),
                "production".into(),
            ),
            ("KUZZ_PUSH_BIND_ADDR".into(), "127.0.0.1:8080".into()),
            ("KUZZ_PUSH_HEALTH_ADDR".into(), "127.0.0.1:8081".into()),
        ])
    }

    #[test]
    fn dogfood_profile_requires_server_owned_identity_and_certificate() {
        let config = Config::from_map(&base()).unwrap();
        assert_eq!(
            config.profile.apns_credential,
            ApnsCredential::Certificate {
                path: PathBuf::from("/dogfood-identity.pem")
            }
        );
        assert_eq!(config.profile.apns_topic, "app.kuzz.dogfood.mobile");

        for variable in [
            "KUZZ_PUSH_DOGFOOD_APNS_CERT_PATH",
            "KUZZ_PUSH_DOGFOOD_APNS_TOPIC",
            "KUZZ_PUSH_DOGFOOD_APP_ATTEST_APP_ID",
        ] {
            let mut env = base();
            env.remove(variable);
            assert!(
                matches!(Config::from_map(&env), Err(ConfigError::Missing(key)) if key == variable)
            );
        }
    }

    #[test]
    fn token_credential_replaces_certificate_and_requires_apple_ids() {
        let token_env = || {
            let mut env = base();
            env.remove("KUZZ_PUSH_DOGFOOD_APNS_CERT_PATH");
            env.insert(
                "KUZZ_PUSH_DOGFOOD_APNS_KEY_PATH".into(),
                "/AuthKey_ABCDE12345.p8".into(),
            );
            env.insert("KUZZ_PUSH_DOGFOOD_APNS_KEY_ID".into(), "ABCDE12345".into());
            env.insert("KUZZ_PUSH_DOGFOOD_APNS_TEAM_ID".into(), "TEAM123456".into());
            env
        };
        assert_eq!(
            Config::from_map(&token_env())
                .unwrap()
                .profile
                .apns_credential,
            ApnsCredential::Token {
                key_path: PathBuf::from("/AuthKey_ABCDE12345.p8"),
                key_id: "ABCDE12345".into(),
                team_id: "TEAM123456".into(),
            }
        );

        for variable in [
            "KUZZ_PUSH_DOGFOOD_APNS_KEY_ID",
            "KUZZ_PUSH_DOGFOOD_APNS_TEAM_ID",
        ] {
            let mut env = token_env();
            env.remove(variable);
            assert!(
                matches!(Config::from_map(&env), Err(ConfigError::Missing(key)) if key == variable)
            );
        }
        for (key, value) in [
            ("KUZZ_PUSH_DOGFOOD_APNS_KEY_ID", "abcde12345"),
            ("KUZZ_PUSH_DOGFOOD_APNS_KEY_ID", "ABCDE1234"),
            ("KUZZ_PUSH_DOGFOOD_APNS_TEAM_ID", "TEAM-12345"),
        ] {
            let mut env = token_env();
            env.insert(key.into(), value.into());
            assert!(
                matches!(Config::from_map(&env), Err(ConfigError::Invalid(k)) if k == key),
                "accepted {key}={value}"
            );
        }

        // Both credential forms at once is ambiguous and fails startup.
        let mut env = token_env();
        env.insert(
            "KUZZ_PUSH_DOGFOOD_APNS_CERT_PATH".into(),
            "/dogfood-identity.pem".into(),
        );
        assert!(matches!(
            Config::from_map(&env),
            Err(ConfigError::Invalid("KUZZ_PUSH_DOGFOOD_APNS_KEY_PATH"))
        ));
    }

    #[test]
    fn startup_needs_no_origin_and_transcript_audiences_stay_registered() {
        assert!(!base().contains_key("KUZZ_PUSH_GATEWAY_ORIGIN"));
        Config::from_map(&base()).unwrap();
        assert_eq!(ENROLL_AUDIENCE, "https://push.buzz.xyz/v1/installations");
        assert_eq!(DELEGATE_AUDIENCE, "https://push.buzz.xyz/v1/delegations");
        assert_eq!(
            ROTATE_ENDPOINT_AUDIENCE,
            "https://push.buzz.xyz/v1/installations/endpoint"
        );
        assert_eq!(
            REVOKE_DELEGATION_AUDIENCE,
            "https://push.buzz.xyz/v1/delegations/revoke"
        );
        assert_eq!(
            REVOKE_INSTALLATION_AUDIENCE,
            "https://push.buzz.xyz/v1/installations/revoke"
        );
    }

    #[test]
    fn keyrings_preserve_current_then_predecessor_order_and_are_independent() {
        let config = Config::from_map(&base()).unwrap();
        assert_eq!(config.grant_keys[0].id, "current");
        assert_eq!(config.grant_keys[1].id, "old");
        assert_eq!(config.token_keys[0].id, "current-token");
        assert_eq!(config.token_keys[1].id, "old-token");
        assert_ne!(config.grant_keys[0].key, config.token_keys[0].key);
    }

    #[test]
    fn malformed_security_configuration_fails_startup() {
        for (key, value) in [
            ("KUZZ_PUSH_DOGFOOD_APP_ATTEST_APP_ID", ""),
            ("KUZZ_PUSH_DOGFOOD_APNS_ENVIRONMENT", "staging"),
            ("KUZZ_PUSH_MAX_GRANT_LIFETIME_SECONDS", "0"),
            ("KUZZ_PUSH_MAX_GRANT_LIFETIME_SECONDS", "31536001"),
            ("KUZZ_PUSH_MAX_GRANT_LIFETIME_SECONDS", "2.592e+06"),
            ("KUZZ_PUSH_MAX_GRANT_LIFETIME_SECONDS", "2592000.5"),
            ("KUZZ_PUSH_MAX_INSTALLATION_LIFETIME_SECONDS", "0"),
        ] {
            let mut env = base();
            env.insert(key.into(), value.into());
            assert!(Config::from_map(&env).is_err(), "accepted {key}={value}");
        }
    }

    // The chart test supplies actual Helm-rendered environment values. Keep this
    // separate from ordinary unit tests, which do not require Helm or fixtures.
    #[test]
    #[ignore = "run deploy/charts/kuzz-push-gateway/tests/grant-lifetime.sh"]
    fn helm_rendered_grant_lifetimes() {
        let path = std::env::var("KUZZ_TEST_HELM_ENV_FILE").unwrap();
        let cases: Vec<(String, i64, HashMap<String, String>)> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(cases.len(), 9);
        for (label, expected, rendered) in cases {
            let mut env = base();
            env.extend(rendered);
            let config = Config::from_map(&env).unwrap_or_else(|error| panic!("{label}: {error}"));
            assert_eq!(config.max_grant_lifetime_seconds, expected, "{label}");
            assert_eq!(
                env["KUZZ_PUSH_MAX_GRANT_LIFETIME_SECONDS"],
                expected.to_string(),
                "{label} must render decimal integer text"
            );
        }
    }

    #[test]
    fn cross_keyring_id_or_material_reuse_fails_startup() {
        for token_keys in [
            format!("current:{}", STANDARD.encode([9; 32])),
            format!("other:{}", STANDARD.encode([1; 32])),
        ] {
            let mut env = base();
            env.insert("KUZZ_PUSH_TOKEN_KEYS".into(), token_keys);
            assert!(Config::from_map(&env).is_err());
        }
    }

    #[test]
    fn android_profile_is_opt_in_and_fully_specified() {
        assert_eq!(Config::from_map(&base()).unwrap().android, None);
        let mut env = base();
        env.extend([
            (
                "KUZZ_PUSH_ANDROID_FCM_SERVICE_ACCOUNT_PATH".to_owned(),
                "/fcm-service-account.json".to_owned(),
            ),
            (
                "KUZZ_PUSH_ANDROID_PACKAGE".to_owned(),
                "app.kuzz.mobile".to_owned(),
            ),
            (
                "KUZZ_PUSH_ANDROID_SIGNING_CERT_SHA256".to_owned(),
                format!("{},{}", "ab".repeat(32), ["CD"; 32].join(":")),
            ),
            (
                "KUZZ_PUSH_ANDROID_ATTESTATION_ROOTS_PATH".to_owned(),
                "/google-roots.pem".to_owned(),
            ),
        ]);
        assert_eq!(
            Config::from_map(&env).unwrap().android,
            Some(AndroidProfileConfig {
                fcm_service_account_path: PathBuf::from("/fcm-service-account.json"),
                package: "app.kuzz.mobile".to_owned(),
                signing_digests: vec![[0xab; 32], [0xcd; 32]],
                attestation_roots_path: PathBuf::from("/google-roots.pem"),
            })
        );
        for (key, value) in [
            ("KUZZ_PUSH_ANDROID_FCM_SERVICE_ACCOUNT_PATH", ""),
            ("KUZZ_PUSH_ANDROID_PACKAGE", ""),
            ("KUZZ_PUSH_ANDROID_PACKAGE", "app kuzz"),
            ("KUZZ_PUSH_ANDROID_SIGNING_CERT_SHA256", ""),
            ("KUZZ_PUSH_ANDROID_SIGNING_CERT_SHA256", "abcd"),
            ("KUZZ_PUSH_ANDROID_ATTESTATION_ROOTS_PATH", ""),
        ] {
            let mut bad = env.clone();
            bad.insert(key.into(), value.into());
            assert!(Config::from_map(&bad).is_err(), "accepted {key}={value:?}");
        }
        for key in [
            "KUZZ_PUSH_ANDROID_PACKAGE",
            "KUZZ_PUSH_ANDROID_SIGNING_CERT_SHA256",
            "KUZZ_PUSH_ANDROID_ATTESTATION_ROOTS_PATH",
        ] {
            let mut missing = env.clone();
            missing.remove(key);
            assert!(matches!(Config::from_map(&missing), Err(ConfigError::Missing(k)) if k == key));
        }
    }

    #[test]
    fn listener_defaults_remain_public_when_addresses_are_absent() {
        let mut env = base();
        env.remove("KUZZ_PUSH_BIND_ADDR");
        env.remove("KUZZ_PUSH_HEALTH_ADDR");

        let config = Config::from_map(&env).unwrap();
        assert_eq!(config.bind_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(config.health_addr, "0.0.0.0:8081".parse().unwrap());
    }

    #[test]
    fn malformed_or_empty_keyrings_fail_startup() {
        for (variable, value) in [
            ("KUZZ_PUSH_GRANT_KEYS", ""),
            ("KUZZ_PUSH_GRANT_KEYS", "missing_separator"),
            ("KUZZ_PUSH_GRANT_KEYS", "id:bad-base64"),
            ("KUZZ_PUSH_TOKEN_KEYS", ""),
            ("KUZZ_PUSH_TOKEN_KEYS", "missing_separator"),
            ("KUZZ_PUSH_TOKEN_KEYS", "id:bad-base64"),
        ] {
            let mut env = base();
            env.insert(variable.into(), value.into());
            assert!(Config::from_map(&env).is_err());
        }
    }
}
