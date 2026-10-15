//! Which server the backend talks to: the build-time Kuzz environment table
//! and the rule that refuses every other relay.

use super::{configured_env_var, relay_http_base_url};

/// The Kuzz server environment, chosen at build time with `KUZZ_ENV=prod|dev`
/// (default `prod`). Debug vs release does not matter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KuzzEnv {
    /// Always the prod server; every env/baked override is ignored and no
    /// other relay may be applied.
    Prod,
    /// The dev server, unless overridden explicitly by runtime
    /// `BUZZ_RELAY_URL` or the baked build-time `BUZZ_RELAY_URL`.
    Dev,
}

/// One server's WebSocket and HTTP addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayEndpoints {
    pub ws: &'static str,
    pub http: &'static str,
}

/// The server table: the only place the addresses live. Server-pushed
/// address changes are a later feature.
pub const fn relay_endpoints(env: KuzzEnv) -> RelayEndpoints {
    match env {
        KuzzEnv::Prod => RelayEndpoints {
            ws: "wss://jihobuzz.duckdns.org:8443",
            http: "https://jihobuzz.duckdns.org:8443",
        },
        // Same server for now. When dev and prod are split, change only this
        // dev entry (e.g. to the developer PC's address); prod stays as is.
        KuzzEnv::Dev => RelayEndpoints {
            ws: "wss://jihobuzz.duckdns.org:8443",
            http: "https://jihobuzz.duckdns.org:8443",
        },
    }
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

/// Explicit relay overrides (runtime env first, then the baked build value).
/// Only the dev environment honours them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayOverrides {
    pub ws: Option<String>,
    pub http: Option<String>,
}

fn relay_overrides() -> RelayOverrides {
    let baked = |value: Option<&str>| {
        value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    RelayOverrides {
        ws: configured_env_var("BUZZ_RELAY_URL")
            .or_else(|| baked(option_env!("BUZZ_DESKTOP_BUILD_RELAY_URL"))),
        http: configured_env_var("BUZZ_RELAY_HTTP")
            .or_else(|| baked(option_env!("BUZZ_DESKTOP_BUILD_RELAY_HTTP"))),
    }
}

/// Pure relay choice: `(WebSocket URL, HTTP base URL)` for `env`.
pub fn choose_relay_urls(env: KuzzEnv, overrides: &RelayOverrides) -> (String, String) {
    let table = relay_endpoints(env);
    if env == KuzzEnv::Prod {
        return (table.ws.to_string(), table.http.to_string());
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
        None => (table.ws.to_string(), table.http.to_string()),
    }
}

/// Host and port of a relay URL (ws/wss/http/https alike), with the scheme's
/// default port filled in. `None` for anything unparseable.
fn relay_host_port(url: &str) -> Option<(String, u16)> {
    let parsed = reqwest::Url::parse(&relay_http_base_url(url)).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some((host, parsed.port_or_known_default()?))
}

/// Pure check: may the backend be pointed at `relay_url`? Only the server
/// [`choose_relay_urls`] picks for `env` is allowed.
pub fn check_relay_allowed(
    env: KuzzEnv,
    overrides: &RelayOverrides,
    relay_url: &str,
) -> Result<(), String> {
    let (allowed, _) = choose_relay_urls(env, overrides);
    match (relay_host_port(relay_url), relay_host_port(&allowed)) {
        (Some(actual), Some(wanted)) if actual == wanted => Ok(()),
        _ => Err("Kuzz only connects to its own server.".to_string()),
    }
}

/// Refuse (and log) any command that would reach a server other than the
/// built-in one. `context` names the command for the log line; the refused
/// host:port is logged, never credentials or paths.
pub fn ensure_relay_allowed(context: &str, relay_url: &str) -> Result<(), String> {
    check_relay_allowed(kuzz_env(), &relay_overrides(), relay_url).inspect_err(|_| {
        let target = relay_host_port(relay_url)
            .map(|(host, port)| format!("{host}:{port}"))
            .unwrap_or_else(|| "an unparseable address".to_string());
        eprintln!("buzz-desktop: refused {context} for {target}: not the built-in server");
    })
}

/// The built-in server's WebSocket URL for this build.
pub fn relay_ws_url() -> String {
    choose_relay_urls(kuzz_env(), &relay_overrides()).0
}

pub fn relay_api_base_url() -> String {
    choose_relay_urls(kuzz_env(), &relay_overrides()).1
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
        check_relay_allowed, choose_relay_urls, parse_kuzz_env, relay_endpoints, KuzzEnv,
        RelayOverrides,
    };

    fn overrides(ws: Option<&str>, http: Option<&str>) -> RelayOverrides {
        RelayOverrides {
            ws: ws.map(str::to_string),
            http: http.map(str::to_string),
        }
    }

    fn table(env: KuzzEnv) -> (String, String) {
        let endpoints = relay_endpoints(env);
        (endpoints.ws.to_string(), endpoints.http.to_string())
    }

    #[test]
    fn kuzz_env_defaults_to_prod() {
        assert_eq!(parse_kuzz_env(None), KuzzEnv::Prod);
        assert_eq!(parse_kuzz_env(Some("prod")), KuzzEnv::Prod);
        assert_eq!(parse_kuzz_env(Some("dev")), KuzzEnv::Dev);
        assert_eq!(
            relay_endpoints(KuzzEnv::Prod).ws,
            "wss://jihobuzz.duckdns.org:8443"
        );
        assert_eq!(
            relay_endpoints(KuzzEnv::Prod).http,
            "https://jihobuzz.duckdns.org:8443"
        );
    }

    #[test]
    fn prod_always_uses_its_table_url() {
        assert_eq!(
            choose_relay_urls(KuzzEnv::Prod, &overrides(None, None)),
            table(KuzzEnv::Prod)
        );
        assert_eq!(
            choose_relay_urls(
                KuzzEnv::Prod,
                &overrides(Some("ws://localhost:3000"), Some("http://localhost:3000"))
            ),
            table(KuzzEnv::Prod)
        );
    }

    #[test]
    fn dev_uses_its_table_url_unless_overridden() {
        assert_eq!(
            choose_relay_urls(KuzzEnv::Dev, &overrides(None, None)),
            table(KuzzEnv::Dev)
        );
        assert_eq!(
            choose_relay_urls(KuzzEnv::Dev, &overrides(Some("ws://localhost:3000"), None)),
            (
                "ws://localhost:3000".to_string(),
                "http://localhost:3000".to_string()
            )
        );
        assert_eq!(
            choose_relay_urls(
                KuzzEnv::Dev,
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
            "wss://jihobuzz.duckdns.org:8443",
            "wss://jihobuzz.duckdns.org:8443/",
            "https://JIHOBUZZ.duckdns.org:8443",
            "ws://jihobuzz.duckdns.org:8443",
        ] {
            assert!(
                check_relay_allowed(KuzzEnv::Prod, &none, allowed).is_ok(),
                "{allowed}"
            );
            assert!(
                check_relay_allowed(KuzzEnv::Prod, &local, allowed).is_ok(),
                "{allowed}"
            );
        }
        for refused in [
            "ws://localhost:3000",
            "wss://jihobuzz.duckdns.org",
            "wss://jihobuzz.duckdns.org:9443",
            "wss://other.example:8443",
            "not a url",
        ] {
            assert!(
                check_relay_allowed(KuzzEnv::Prod, &none, refused).is_err(),
                "{refused}"
            );
            assert!(
                check_relay_allowed(KuzzEnv::Prod, &local, refused).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn dev_allows_only_the_chosen_server() {
        let none = overrides(None, None);
        let dev_table = relay_endpoints(KuzzEnv::Dev).ws;
        assert!(check_relay_allowed(KuzzEnv::Dev, &none, dev_table).is_ok());
        assert!(check_relay_allowed(KuzzEnv::Dev, &none, "ws://localhost:3000").is_err());
        let local = overrides(Some("ws://localhost:3000"), None);
        assert!(check_relay_allowed(KuzzEnv::Dev, &local, "ws://localhost:3000/").is_ok());
        assert!(check_relay_allowed(KuzzEnv::Dev, &local, dev_table).is_err());
    }

    #[test]
    fn ensure_relay_allowed_refuses_other_servers_for_this_build() {
        // Unit tests build the default `prod` environment.
        assert_eq!(super::kuzz_env(), KuzzEnv::Prod);
        let ours = relay_endpoints(KuzzEnv::Prod);
        assert!(super::ensure_relay_allowed("test", ours.ws).is_ok());
        assert!(super::ensure_relay_allowed("test", ours.http).is_ok());
        assert_eq!(
            super::ensure_relay_allowed("test", "wss://other.example").unwrap_err(),
            "Kuzz only connects to its own server."
        );
        assert!(super::ensure_relay_allowed("test", "ws://localhost:3000").is_err());
    }
}
