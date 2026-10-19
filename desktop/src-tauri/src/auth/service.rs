//! Stored logins and settings belong to the service, not to a server address.
//!
//! Kuzz is one service. Its address can change (port, domain, a server move),
//! and an address change must not sign anyone out. So everything persisted
//! for the built-in server (secure-storage slot names, `accounts.json`, the
//! sign-out markers, agent records) is keyed by the service scope (`kuzz`),
//! and the address is only used to connect: on load a stored scope (or one of
//! the service's addresses) becomes this build's current origin.
//!
//! The service's addresses are the build's server plus the build env
//! `KUZZ_PREVIOUS_RELAY_URLS` (comma-separated). Builds before this module
//! stored the address itself; [`migrate_secrets`] copies those slots to the
//! scope once at launch and never deletes them. Any other address (a dev
//! override, tests) is stored by its origin as before.

use std::path::Path;

use super::accounts::Secrets;

/// Scope for the prod and dev environments (separate apps, kept apart in case
/// they share secure storage).
fn scope_for(env: crate::relay::server::KuzzEnv) -> &'static str {
    match env {
        crate::relay::server::KuzzEnv::Prod => "kuzz",
        crate::relay::server::KuzzEnv::Dev => "kuzz-dev",
    }
}

/// The stored key standing for this build's service.
pub(crate) fn scope() -> &'static str {
    scope_for(crate::relay::server::kuzz_env())
}

/// The build env `KUZZ_PREVIOUS_RELAY_URLS`: earlier addresses of this
/// build's server.
pub(crate) fn previous_relay_urls() -> Vec<String> {
    option_env!("KUZZ_DESKTOP_BUILD_PREVIOUS_RELAY_URLS")
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `scheme://host[:port]` with the scheme's default port dropped, lowercase;
/// `ws`/`wss` fold to `http`/`https`. `None` when unparseable.
pub(crate) fn canonical_origin(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(&crate::relay::relay_http_base_url(url)).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    Some(match parsed.port() {
        Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
        None => format!("{}://{host}", parsed.scheme()),
    })
}

/// The service's addresses as one build sees them. Pure, for tests.
#[derive(Clone, Debug)]
pub(crate) struct Service {
    pub scope: String,
    /// The origin the app uses for the built-in server (`auth::origin_for`).
    pub current: String,
    /// Canonical origins of the current and every previous address.
    canonical: Vec<String>,
    /// Origins as builds before this module wrote them into slot names
    /// (`auth::origin_for` of each address).
    legacy: Vec<String>,
    /// The current and earlier relay URLs, normalised as agent runtimes see
    /// them (`kuzz_core::relay::normalize_relay_url`).
    relay_urls: Vec<String>,
}

impl Service {
    pub(crate) fn new(scope: &str, current_ws: &str, previous: &[&str]) -> Self {
        let mut canonical = Vec::new();
        let mut legacy = Vec::new();
        let mut relay_urls = Vec::new();
        for url in std::iter::once(current_ws).chain(previous.iter().copied()) {
            let url = url.trim();
            if url.is_empty() {
                continue;
            }
            if let Some(origin) = canonical_origin(url) {
                if !canonical.contains(&origin) {
                    canonical.push(origin);
                }
            }
            let origin = super::origin_for(url);
            if !legacy.contains(&origin) {
                legacy.push(origin);
            }
            if let Ok(normalized) = kuzz_core_pkg::relay::normalize_relay_url(url) {
                if !relay_urls.contains(&normalized) {
                    relay_urls.push(normalized);
                }
            }
        }
        Self {
            scope: scope.to_owned(),
            current: super::origin_for(current_ws),
            canonical,
            legacy,
            relay_urls,
        }
    }

    /// This build's service.
    pub(crate) fn built_in() -> Self {
        let previous = previous_relay_urls();
        let previous: Vec<&str> = previous.iter().map(String::as_str).collect();
        let current = crate::relay::server::relay_endpoints().ws;
        Self::new(scope(), &current, &previous)
    }

    /// The current and earlier relay URLs (agent runtimes resume sessions an
    /// older build kept under one of them).
    pub(crate) fn relay_urls(&self) -> &[String] {
        &self.relay_urls
    }

    /// Whether `origin` (an origin or relay URL) is one of the service's
    /// addresses, or the scope itself.
    pub(crate) fn is_service(&self, origin: &str) -> bool {
        origin == self.scope
            || canonical_origin(origin).is_some_and(|origin| self.canonical.contains(&origin))
    }

    /// What to persist for `origin`: the scope for the service, else `origin`.
    pub(crate) fn stored(&self, origin: &str) -> String {
        if self.is_service(origin) {
            self.scope.clone()
        } else {
            origin.to_owned()
        }
    }

    /// What a persisted value means now: the current origin for the service
    /// (the scope or any of its addresses), else the value unchanged.
    pub(crate) fn live(&self, stored: &str) -> String {
        if self.is_service(stored) {
            self.current.clone()
        } else {
            stored.to_owned()
        }
    }

    /// See [`current_url`].
    pub(crate) fn current_url(&self, url: &str) -> String {
        if !self.is_service(url) || url == self.scope {
            return url.to_owned();
        }
        let (Ok(mut parsed), Ok(current)) =
            (reqwest::Url::parse(url), reqwest::Url::parse(&self.current))
        else {
            return url.to_owned();
        };
        if parsed.set_host(current.host_str()).is_err() || parsed.set_port(current.port()).is_err()
        {
            return url.to_owned();
        }
        parsed.to_string()
    }

    /// Copy every secure-storage slot an older build keyed by one of the
    /// service's addresses to the scope's slot, unless that slot is already
    /// set. Old slots are never deleted (an older app still reads them).
    /// Addresses in `done` were moved by an earlier launch and are skipped, so
    /// a slot this build deleted later (sign-out) is never brought back.
    /// `pubkeys`: hex pubkeys whose own refresh slots to copy. Returns how
    /// many slots were copied and the addresses now moved.
    pub(crate) fn migrate_secrets(
        &self,
        secrets: &dyn Secrets,
        pubkeys: &[String],
        done: &[String],
    ) -> Result<(usize, Vec<String>), String> {
        let pending: Vec<&String> = self.legacy.iter().filter(|o| !done.contains(o)).collect();
        if pending.is_empty() {
            return Ok((0, Vec::new()));
        }
        let mut families: Vec<Box<dyn Fn(&str) -> String>> = vec![
            Box::new(|origin| format!("auth.refresh.{origin}")),
            Box::new(|origin| format!("auth.key-backup.{origin}")),
            Box::new(|origin| format!("auth.legacy-token.{origin}")),
        ];
        for hex in pubkeys {
            let hex = hex.clone();
            families.push(Box::new(move |origin| {
                format!("account.{hex}.refresh.{origin}")
            }));
        }
        let mut copied = 0;
        for slot in &families {
            let target = slot(&self.scope);
            if secrets.load(&target)?.is_some() {
                continue;
            }
            for origin in &pending {
                if let Some(value) = secrets.load(&slot(origin))? {
                    let value = zeroize::Zeroizing::new(value);
                    secrets.store(&target, &value)?;
                    copied += 1;
                    break;
                }
            }
        }
        Ok((copied, pending.into_iter().cloned().collect()))
    }
}

/// This build's service (built once).
pub(crate) fn service() -> &'static Service {
    static SERVICE: std::sync::OnceLock<Service> = std::sync::OnceLock::new();
    SERVICE.get_or_init(Service::built_in)
}

/// Map every account and pending-key origin of `registry` with `map`.
fn map_registry_origins(registry: &mut super::accounts::Registry, map: fn(&str) -> String) {
    for account in &mut registry.accounts {
        account.origin = map(&account.origin);
    }
    for pending in &mut registry.pending {
        pending.origin = map(&pending.origin);
    }
}

/// The account list as read from disk, its origins on the current address.
pub(crate) fn live_registry(bytes: &[u8]) -> serde_json::Result<super::accounts::Registry> {
    let mut registry = serde_json::from_slice(bytes)?;
    map_registry_origins(&mut registry, live);
    Ok(registry)
}

/// The account list as written to disk, its origins by service scope.
pub(crate) fn stored_registry_json(
    registry: &super::accounts::Registry,
) -> serde_json::Result<Vec<u8>> {
    let mut stored = registry.clone();
    map_registry_origins(&mut stored, self::stored);
    serde_json::to_vec_pretty(&stored)
}

/// An account's refresh-token slot for `origin` (keyed by the scope).
pub(crate) fn account_refresh_slot(pubkey_hex: &str, origin: &str) -> String {
    format!("account.{pubkey_hex}.refresh.{}", stored(origin))
}

/// What to persist for `origin` (see [`Service::stored`]).
pub(crate) fn stored(origin: &str) -> String {
    service().stored(origin)
}

/// What a persisted origin means now (see [`Service::live`]).
pub(crate) fn live(stored: &str) -> String {
    service().live(stored)
}

/// `url` on the current address when it points at an earlier (or the
/// current) address of the service, path and query kept; otherwise `url`.
pub(crate) fn current_url(url: &str) -> String {
    service().current_url(url)
}

/// Files that start storing the scope instead of an address, copied once
/// beside themselves before the first launch rewrites them, so the older app
/// can be restored by putting them back.
const BACKED_UP_FILES: &[&str] = &[
    super::accounts::FILE_NAME,
    super::signed_out::FILE_NAME,
    "agents/managed-agents.json",
];

const BACKUP_SUFFIX: &str = "before-service-scope";

/// Which old addresses' slots were already moved: `{"moved": [origin, …]}`.
const MIGRATION_FILE: &str = "service-scope.json";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct MigrationState {
    #[serde(default)]
    moved: Vec<String>,
}

/// Launch step, before anything reads the account list: back up the files
/// above once, then copy the address-keyed secure-storage slots to the scope.
pub(crate) fn migrate_on_launch(secrets: &dyn Secrets, data_dir: &Path) {
    for name in BACKED_UP_FILES {
        let path = data_dir.join(name);
        let backup = path.with_file_name(format!(
            "{}.{BACKUP_SUFFIX}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        ));
        if path.exists() && !backup.exists() {
            if let Err(error) = std::fs::copy(&path, &backup) {
                eprintln!("kuzz-desktop: backing up {name} failed: {error}");
            }
        }
    }
    // An unreadable account list (locked, invalid) would skip the accounts'
    // own slots: copy what can be copied, but leave the move unrecorded so
    // the next launch retries with the accounts.
    let registry = super::accounts::load_registry(data_dir);
    let complete = registry.is_ok();
    let mut pubkeys: Vec<String> = match registry {
        Ok(Some(registry)) => {
            let pending = registry.pending.into_iter().map(|p| p.pubkey);
            registry
                .accounts
                .into_iter()
                .map(|a| a.pubkey)
                .chain(pending)
                .collect()
        }
        Ok(None) => Vec::new(),
        Err(error) => {
            eprintln!("kuzz-desktop: account list unreadable for the sign-in move: {error}");
            Vec::new()
        }
    };
    pubkeys.sort();
    pubkeys.dedup();
    let state_path = data_dir.join(MIGRATION_FILE);
    let mut state: MigrationState = std::fs::read(&state_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    match service().migrate_secrets(secrets, &pubkeys, &state.moved) {
        Ok((_, moved)) if moved.is_empty() || !complete => {}
        Ok((copied, moved)) => {
            eprintln!("kuzz-desktop: moved {copied} sign-in slots to the service scope");
            state.moved.extend(moved);
            let written = serde_json::to_vec_pretty(&state)
                .map_err(|e| e.to_string())
                .and_then(|json| std::fs::write(&state_path, json).map_err(|e| e.to_string()));
            if let Err(error) = written {
                eprintln!("kuzz-desktop: recording the sign-in move failed: {error}");
            }
        }
        Err(error) => eprintln!("kuzz-desktop: moving sign-in slots failed: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn service() -> Service {
        Service::new(
            "kuzz",
            "wss://jihobuzz.duckdns.org",
            &["wss://jihobuzz.duckdns.org:8443", ""],
        )
    }

    #[test]
    fn service_addresses_map_to_the_scope_and_back_to_the_current_origin() {
        let s = service();
        assert_eq!(s.current, "https://jihobuzz.duckdns.org");
        for origin in [
            "https://jihobuzz.duckdns.org:8443",
            "wss://jihobuzz.duckdns.org:8443/",
            "https://jihobuzz.duckdns.org",
            "https://jihobuzz.duckdns.org:443",
            "https://JIHOBUZZ.duckdns.org",
            "kuzz",
        ] {
            assert!(s.is_service(origin), "{origin}");
            assert_eq!(s.stored(origin), "kuzz", "{origin}");
            assert_eq!(s.live(origin), "https://jihobuzz.duckdns.org", "{origin}");
        }
    }

    #[test]
    fn other_addresses_are_unchanged() {
        let s = service();
        for origin in [
            "https://relay.example",
            "http://jihobuzz.duckdns.org",
            "https://jihobuzz.duckdns.org:9443",
            "*",
            "",
        ] {
            assert!(!s.is_service(origin), "{origin}");
            assert_eq!(s.stored(origin), origin);
            assert_eq!(s.live(origin), origin);
        }
    }

    #[test]
    fn media_urls_move_to_the_current_address() {
        let s = service();
        assert_eq!(
            s.current_url("https://jihobuzz.duckdns.org:8443/media/ab.jpg?x=1"),
            "https://jihobuzz.duckdns.org/media/ab.jpg?x=1"
        );
        assert_eq!(
            s.current_url("https://relay.example/media/ab.jpg"),
            "https://relay.example/media/ab.jpg"
        );
    }

    #[derive(Default)]
    struct MemSecrets(Mutex<HashMap<String, String>>);

    impl Secrets for MemSecrets {
        fn load(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn store(&self, key: &str, value: &str) -> Result<(), String> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
        fn store_all(&self, entries: &HashMap<String, String>) -> Result<(), String> {
            self.0.lock().unwrap().extend(entries.clone());
            Ok(())
        }
        fn blob_keys(&self) -> Result<Vec<String>, String> {
            Ok(self.0.lock().unwrap().keys().cloned().collect())
        }
    }

    #[test]
    fn migration_copies_old_slots_once_and_keeps_them() {
        let s = service();
        let secrets = MemSecrets::default();
        let old = "https://jihobuzz.duckdns.org:8443";
        let hex = "ab".repeat(32);
        for (key, value) in [
            (format!("auth.refresh.{old}"), "bzr_old"),
            (format!("auth.key-backup.{old}"), "pub"),
            (format!("account.{hex}.refresh.{old}"), "bzr_slot"),
            // A scope slot already written by this build wins.
            ("auth.legacy-token.kuzz".to_owned(), "token"),
            (format!("auth.legacy-token.{old}"), "stale"),
        ] {
            secrets.store(&key, value).unwrap();
        }
        let (copied, moved) = s.migrate_secrets(&secrets, &[hex.clone()], &[]).unwrap();
        assert_eq!(copied, 3);
        assert_eq!(
            moved,
            [
                "https://jihobuzz.duckdns.org",
                "https://jihobuzz.duckdns.org:8443"
            ]
        );
        let get = |key: &str| secrets.load(key).unwrap();
        assert_eq!(get("auth.refresh.kuzz").as_deref(), Some("bzr_old"));
        assert_eq!(get("auth.key-backup.kuzz").as_deref(), Some("pub"));
        assert_eq!(
            get(&format!("account.{hex}.refresh.kuzz")).as_deref(),
            Some("bzr_slot")
        );
        assert_eq!(get("auth.legacy-token.kuzz").as_deref(), Some("token"));
        assert_eq!(
            get(&format!("auth.refresh.{old}")).as_deref(),
            Some("bzr_old")
        );
        // Moved addresses are never moved again: a slot deleted after the
        // move (sign-out) stays deleted.
        secrets.delete("auth.refresh.kuzz").unwrap();
        assert_eq!(
            s.migrate_secrets(&secrets, &[hex], &moved).unwrap(),
            (0, Vec::new())
        );
        assert_eq!(secrets.load("auth.refresh.kuzz").unwrap(), None);
    }

    #[test]
    fn migration_reads_the_current_address_slot_too() {
        // A build whose address did not change still moves to the scope.
        let s = Service::new("kuzz", "wss://jihobuzz.duckdns.org:8443", &[]);
        let secrets = MemSecrets::default();
        secrets
            .store("auth.refresh.https://jihobuzz.duckdns.org:8443", "bzr")
            .unwrap();
        assert_eq!(s.migrate_secrets(&secrets, &[], &[]).unwrap().0, 1);
        assert_eq!(
            secrets.load("auth.refresh.kuzz").unwrap().as_deref(),
            Some("bzr")
        );
    }
}
