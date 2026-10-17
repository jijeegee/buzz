//! Which server the backend talks to: the build-time server address and the
//! rule that refuses every other relay.

use super::{configured_env_var, relay_http_base_url};

/// The Kuzz server environment, chosen at build time with `KUZZ_ENV=prod|dev`
/// (default `prod`). Debug vs release does not matter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KuzzEnv {
    /// Always the built-in server; runtime overrides are ignored and no other
    /// relay may be applied.
    Prod,
    /// The built-in server, unless overridden explicitly by the runtime
    /// `KUZZ_RELAY_URL` / `KUZZ_RELAY_HTTP`.
    Dev,
}

/// One server's WebSocket and HTTP addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayEndpoints {
    pub ws: String,
    pub http: String,
}

/// Stand-in address for builds made without `KUZZ_RELAY_URL` (debug builds and
/// tests only: `build.rs` fails release builds without it). It never resolves,
/// so such a build visibly connects nowhere instead of to a guessed server.
const UNSET_RELAY_WS: &str = "wss://kuzz-relay-url-not-set.invalid";

/// Endpoints from a build's baked `KUZZ_RELAY_URL` (WebSocket) and optional
/// `KUZZ_RELAY_HTTP`; the HTTP base defaults to the WebSocket URL's.
fn built_in_endpoints(ws: Option<&str>, http: Option<&str>) -> RelayEndpoints {
    let present = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let ws = present(ws).unwrap_or_else(|| UNSET_RELAY_WS.to_string());
    let http = present(http)
        .map(|http| http.trim_end_matches('/').to_string())
        .unwrap_or_else(|| relay_http_base_url(&ws));
    RelayEndpoints { ws, http }
}

/// This build's server, baked from the build environment's `KUZZ_RELAY_URL`
/// (and optional `KUZZ_RELAY_HTTP`). Server-pushed address changes are a
/// later feature.
pub fn relay_endpoints() -> RelayEndpoints {
    built_in_endpoints(
        option_env!("KUZZ_DESKTOP_BUILD_RELAY_URL"),
        option_env!("KUZZ_DESKTOP_BUILD_RELAY_HTTP"),
    )
}

/// Parse the baked `KUZZ_DESKTOP_BUILD_ENV`; anything but `dev` is prod.
pub fn parse_kuzz_env(value: Option<&str>) -> KuzzEnv {
    match value.map(str::trim) {
        Some("dev") => KuzzEnv::Dev,
        _ => KuzzEnv::Prod,
    }
}

/// The environment this binary was built for.
pub fn kuzz_env() -> KuzzEnv {
    parse_kuzz_env(option_env!("KUZZ_DESKTOP_BUILD_ENV"))
}

/// Explicit runtime relay overrides. Only the dev environment honours them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayOverrides {
    pub ws: Option<String>,
    pub http: Option<String>,
}

fn relay_overrides() -> RelayOverrides {
    RelayOverrides {
        ws: configured_env_var("KUZZ_RELAY_URL"),
        http: configured_env_var("KUZZ_RELAY_HTTP"),
    }
}

/// Pure relay choice: `(WebSocket URL, HTTP base URL)` for `env`, given the
/// build's `built_in` server.
pub fn choose_relay_urls(
    env: KuzzEnv,
    built_in: &RelayEndpoints,
    overrides: &RelayOverrides,
) -> (String, String) {
    if env == KuzzEnv::Prod {
        return (built_in.ws.clone(), built_in.http.clone());
    }
    match overrides.ws.as_deref() {
        Some(ws) => {
            let ws = ws.trim().to_string();
            let http = overrides
                .http
                .as_deref()
                .map(|http| http.trim().trim_end_matches('/').to_string())
                .unwrap_or_else(|| relay_http_base_url(&ws));
            (ws, http)
        }
        None => (built_in.ws.clone(), built_in.http.clone()),
    }
}

/// Where a relay URL points: whether it is encrypted (`wss`/`https`), and its
/// host and port with the scheme's default port filled in. `ws` and `http`
/// (or `wss` and `https`) name the same endpoint. `None` for anything
/// unparseable.
#[derive(Debug, PartialEq, Eq)]
struct RelayEndpoint {
    secure: bool,
    host: String,
    port: u16,
}

fn relay_endpoint(url: &str) -> Option<RelayEndpoint> {
    let parsed = reqwest::Url::parse(&relay_http_base_url(url)).ok()?;
    let secure = match parsed.scheme() {
        "https" => true,
        "http" => false,
        _ => return None,
    };
    Some(RelayEndpoint {
        secure,
        host: parsed.host_str()?.to_ascii_lowercase(),
        port: parsed.port_or_known_default()?,
    })
}

/// Pure check: may the backend be pointed at `relay_url`? Only the server
/// [`choose_relay_urls`] picks for `env` is allowed, over the same kind of
/// scheme: prod builds bake a `wss` server (enforced by `build.rs`), so prod
/// never allows `ws`/`http`.
/// Only a dev build's explicit unencrypted override allows `ws`/`http`.
pub fn check_relay_allowed(
    env: KuzzEnv,
    built_in: &RelayEndpoints,
    overrides: &RelayOverrides,
    relay_url: &str,
) -> Result<(), String> {
    let (allowed, _) = choose_relay_urls(env, built_in, overrides);
    match (relay_endpoint(relay_url), relay_endpoint(&allowed)) {
        (Some(actual), Some(wanted)) if actual == wanted => Ok(()),
        _ => Err("Kuzz only connects to its own server.".to_string()),
    }
}

/// Refuse (and log) any command that would reach a server other than the
/// built-in one. `context` names the command for the log line; the refused
/// host:port is logged, never credentials or paths.
pub fn ensure_relay_allowed(context: &str, relay_url: &str) -> Result<(), String> {
    check_relay_allowed(
        kuzz_env(),
        &relay_endpoints(),
        &relay_overrides(),
        relay_url,
    )
    .inspect_err(|_| {
        let target = relay_endpoint(relay_url)
            .map(|RelayEndpoint { host, port, .. }| format!("{host}:{port}"))
            .unwrap_or_else(|| "an unparseable address".to_string());
        eprintln!("kuzz-desktop: refused {context} for {target}: not the built-in server");
    })
}

/// The built-in server's WebSocket URL for this build.
pub fn relay_ws_url() -> String {
    choose_relay_urls(kuzz_env(), &relay_endpoints(), &relay_overrides()).0
}

pub fn relay_api_base_url() -> String {
    choose_relay_urls(kuzz_env(), &relay_endpoints(), &relay_overrides()).1
}

/// The refusal every guarded command returns, for command-level tests.
#[cfg(test)]
pub(crate) const REFUSED: &str = "Kuzz only connects to its own server.";

/// A mock app with fresh app state, for refusal tests of commands that take
/// `State` (the keyring is the in-memory mock, so nothing touches the system).
#[cfg(test)]
pub(crate) fn refusal_test_app() -> tauri::App<tauri::test::MockRuntime> {
    #[cfg(feature = "system-keyring")]
    keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    tauri::test::mock_builder()
        .manage(crate::app_state::build_app_state())
        .manage(crate::native_websocket::WebSocketManager::default())
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app")
}

#[cfg(test)]
mod tests {
    use super::{
        built_in_endpoints, check_relay_allowed, choose_relay_urls, parse_kuzz_env,
        relay_endpoints, KuzzEnv, RelayEndpoints, RelayOverrides, UNSET_RELAY_WS,
    };

    fn overrides(ws: Option<&str>, http: Option<&str>) -> RelayOverrides {
        RelayOverrides {
            ws: ws.map(str::to_string),
            http: http.map(str::to_string),
        }
    }

    /// A build baked with `KUZZ_RELAY_URL=wss://kuzz.example:8443`.
    fn ours() -> RelayEndpoints {
        built_in_endpoints(Some("wss://kuzz.example:8443"), None)
    }

    fn pair(endpoints: &RelayEndpoints) -> (String, String) {
        (endpoints.ws.clone(), endpoints.http.clone())
    }

    #[test]
    fn kuzz_env_defaults_to_prod() {
        assert_eq!(parse_kuzz_env(None), KuzzEnv::Prod);
        assert_eq!(parse_kuzz_env(Some("prod")), KuzzEnv::Prod);
        assert_eq!(parse_kuzz_env(Some("dev")), KuzzEnv::Dev);
    }

    #[test]
    fn built_in_server_comes_from_the_baked_address() {
        assert_eq!(
            pair(&ours()),
            (
                "wss://kuzz.example:8443".to_string(),
                "https://kuzz.example:8443".to_string()
            )
        );
        assert_eq!(
            built_in_endpoints(
                Some(" wss://kuzz.example:8443 "),
                Some("https://api.kuzz.example/")
            ),
            RelayEndpoints {
                ws: "wss://kuzz.example:8443".to_string(),
                http: "https://api.kuzz.example".to_string(),
            }
        );
        // Builds without the address (debug builds and tests only) point
        // nowhere real.
        assert_eq!(built_in_endpoints(None, None).ws, UNSET_RELAY_WS);
        assert_eq!(built_in_endpoints(Some("  "), None).ws, UNSET_RELAY_WS);
    }

    #[test]
    fn prod_always_uses_the_built_in_server() {
        assert_eq!(
            choose_relay_urls(KuzzEnv::Prod, &ours(), &overrides(None, None)),
            pair(&ours())
        );
        assert_eq!(
            choose_relay_urls(
                KuzzEnv::Prod,
                &ours(),
                &overrides(Some("ws://localhost:3000"), Some("http://localhost:3000"))
            ),
            pair(&ours())
        );
    }

    #[test]
    fn dev_uses_the_built_in_server_unless_overridden() {
        assert_eq!(
            choose_relay_urls(KuzzEnv::Dev, &ours(), &overrides(None, None)),
            pair(&ours())
        );
        assert_eq!(
            choose_relay_urls(
                KuzzEnv::Dev,
                &ours(),
                &overrides(Some("ws://localhost:3000"), None)
            ),
            (
                "ws://localhost:3000".to_string(),
                "http://localhost:3000".to_string()
            )
        );
        assert_eq!(
            choose_relay_urls(
                KuzzEnv::Dev,
                &ours(),
                &overrides(Some("wss://dev.example"), Some("https://api.dev.example/"))
            )
            .1,
            "https://api.dev.example"
        );
    }

    #[test]
    fn prod_refuses_every_relay_but_its_own() {
        let none = overrides(None, None);
        let local = overrides(Some("ws://localhost:3000"), None);
        for allowed in [
            "wss://kuzz.example:8443",
            "wss://kuzz.example:8443/",
            "https://KUZZ.example:8443",
        ] {
            assert!(
                check_relay_allowed(KuzzEnv::Prod, &ours(), &none, allowed).is_ok(),
                "{allowed}"
            );
            assert!(
                check_relay_allowed(KuzzEnv::Prod, &ours(), &local, allowed).is_ok(),
                "{allowed}"
            );
        }
        for refused in [
            "ws://localhost:3000",
            "wss://kuzz.example",
            "wss://kuzz.example:9443",
            "wss://other.example:8443",
            // Unencrypted schemes are never allowed in prod, even to our host.
            "ws://kuzz.example:8443",
            "http://kuzz.example:8443",
            "not a url",
        ] {
            assert!(
                check_relay_allowed(KuzzEnv::Prod, &ours(), &none, refused).is_err(),
                "{refused}"
            );
            assert!(
                check_relay_allowed(KuzzEnv::Prod, &ours(), &local, refused).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn dev_allows_only_the_chosen_server() {
        let built_in = ours();
        let none = overrides(None, None);
        assert!(check_relay_allowed(KuzzEnv::Dev, &built_in, &none, &built_in.ws).is_ok());
        assert!(
            check_relay_allowed(KuzzEnv::Dev, &built_in, &none, "ws://localhost:3000").is_err()
        );
        let local = overrides(Some("ws://localhost:3000"), None);
        assert!(
            check_relay_allowed(KuzzEnv::Dev, &built_in, &local, "ws://localhost:3000/").is_ok()
        );
        assert!(check_relay_allowed(KuzzEnv::Dev, &built_in, &local, &built_in.ws).is_err());
        // The built-in server is encrypted: no downgrade without an explicit
        // unencrypted override.
        assert!(
            check_relay_allowed(KuzzEnv::Dev, &built_in, &none, "ws://kuzz.example:8443").is_err()
        );
        assert!(
            check_relay_allowed(KuzzEnv::Dev, &built_in, &none, "http://kuzz.example:8443")
                .is_err()
        );
        // The explicit override's own scheme kind is allowed (ws and http alike).
        assert!(
            check_relay_allowed(KuzzEnv::Dev, &built_in, &local, "http://localhost:3000").is_ok()
        );
        let secure = overrides(Some("wss://dev.example"), None);
        assert!(check_relay_allowed(KuzzEnv::Dev, &built_in, &secure, "wss://dev.example").is_ok());
        assert!(
            check_relay_allowed(KuzzEnv::Dev, &built_in, &secure, "ws://dev.example:443").is_err()
        );
    }

    #[test]
    fn ensure_relay_allowed_refuses_other_servers_for_this_build() {
        // Unit tests build the default `prod` environment.
        assert_eq!(super::kuzz_env(), KuzzEnv::Prod);
        let ours = relay_endpoints();
        assert!(super::ensure_relay_allowed("test", &ours.ws).is_ok());
        assert!(super::ensure_relay_allowed("test", &ours.http).is_ok());
        assert_eq!(
            super::ensure_relay_allowed("test", "wss://other.example").unwrap_err(),
            "Kuzz only connects to its own server."
        );
        assert!(super::ensure_relay_allowed("test", "ws://localhost:3000").is_err());
        let downgraded = ours.ws.replacen("wss://", "ws://", 1);
        assert!(super::ensure_relay_allowed("test", &downgraded).is_err());
    }
}
