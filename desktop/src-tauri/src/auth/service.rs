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
//! stored the address itself; [`Service::move_secrets`] moves those slots to
//! the scope once at launch (the old names are removed, so the size-capped
//! Windows blob never holds both and no spent token survives under an old
//! name). Any other address (a dev override, tests) is stored by its origin
//! as before.

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

    /// Move every secure-storage slot an older build keyed by one of the
    /// service's addresses to the scope's slot. `done`: addresses an earlier
    /// launch already moved. `pubkeys`: accounts whose own refresh slots move.
    ///
    /// Two rules keep a multi-account upgrade whole on every platform:
    /// - Bindings live in the shared blob, which Windows caps at 2,560
    ///   bytes. They move in one blob write that drops the old names, so the
    ///   blob shrinks instead of needing room for both. The old token-account
    ///   mark (`auth.legacy-token`) is dropped, not moved: Kuzz signs in with
    ///   Google only, and a mark under the scope refuses that sign-in.
    /// - A refresh token is spent once used. It is copied only into an empty
    ///   slot, only from an address never moved before, and the old slot is
    ///   deleted right after: a token used (or signed out) under the scope can
    ///   never come back from an old slot and trip reuse detection.
    ///
    /// Idempotent: a failed step leaves either the new slot or the old one,
    /// and the next launch finishes the move.
    pub(crate) fn move_secrets(
        &self,
        secrets: &dyn Secrets,
        pubkeys: &[String],
        done: &[String],
    ) -> Result<Moved, String> {
        let scope = self.scope.as_str();
        let fresh: Vec<&str> = self
            .legacy
            .iter()
            .filter(|origin| !done.contains(origin))
            .map(String::as_str)
            .collect();
        let mut out = Moved::default();

        // 1. Bindings move and old token-account marks go: one blob write.
        let mut moves = Vec::new();
        for origin in &self.legacy {
            moves.push((
                format!("auth.key-backup.{origin}"),
                Some(format!("auth.key-backup.{scope}")),
            ));
            moves.push((format!("auth.legacy-token.{origin}"), None));
        }
        secrets.move_in_blob(&moves)?;

        // 2. Refresh tokens: the active copy, then each account's own slot.
        self.move_token(
            secrets,
            &|origin| format!("auth.refresh.{origin}"),
            &fresh,
            &mut out,
        )?;
        for hex in pubkeys {
            self.move_token(
                secrets,
                &|origin| format!("account.{hex}.refresh.{origin}"),
                &fresh,
                &mut out,
            )?;
        }
        Ok(out)
    }

    /// Move one refresh-token family (`slot(origin)` names it) to the scope;
    /// see [`Self::move_secrets`].
    fn move_token(
        &self,
        secrets: &dyn Secrets,
        slot: &dyn Fn(&str) -> String,
        fresh: &[&str],
        out: &mut Moved,
    ) -> Result<(), String> {
        let target = slot(&self.scope);
        if secrets.load(&target)?.is_none() {
            let mut found = None;
            for origin in fresh {
                let source = slot(origin);
                if let Some(value) = secrets.load(&source)? {
                    found = Some((source, zeroize::Zeroizing::new(value)));
                    break;
                }
            }
            if let Some((source, value)) = found {
                // Rename the source's blob entry (a Windows marker, or the
                // token itself elsewhere) first, so the store below never
                // grows a nearly full blob.
                secrets.move_in_blob(&[(source, Some(target.clone()))])?;
                secrets.store(&target, &value)?;
                if secrets.load(&target)?.as_deref() != Some(value.as_str()) {
                    return Err("OS secure storage verification failed".into());
                }
                out.copied += 1;
            }
        }
        for origin in &self.legacy {
            let old = slot(origin);
            if secrets.load(&old)?.is_some() {
                out.removed += 1;
            }
            // Also drops an orphaned blob marker of a missing credential.
            secrets.delete(&old)?;
        }
        Ok(())
    }

    /// Launch step, before anything reads the account list: back up the
    /// files that now store the scope, then move the address-keyed slots
    /// ([`Self::move_secrets`]) and record it in [`MIGRATION_FILE`].
    pub(crate) fn migrate_on_launch(&self, secrets: &dyn Secrets, data_dir: &Path) {
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
        let state_path = data_dir.join(MIGRATION_FILE);
        let mut state: MigrationState = std::fs::read(&state_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if self
            .legacy
            .iter()
            .all(|origin| state.moved.contains(origin))
        {
            return;
        }
        // An unreadable account list (locked, invalid) would skip the
        // accounts' own slots: move what can be moved, but leave the move
        // unrecorded so the next launch retries with the accounts.
        let registry = super::accounts::load_registry(data_dir);
        let complete = registry.is_ok();
        let mut pubkeys: Vec<String> = match registry {
            Ok(Some(registry)) => {
                let pending = registry.pending.into_iter().map(|p| p.pubkey);
                registry
                    .accounts
                    .into_iter()
                    .filter(|a| a.origin != super::accounts::ANY_ORIGIN)
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
        let moved = match self.move_secrets(secrets, &pubkeys, &state.moved) {
            Ok(moved) => moved,
            Err(error) => {
                eprintln!("kuzz-desktop: moving sign-in slots failed: {error}");
                return;
            }
        };
        eprintln!(
            "kuzz-desktop: sign-in move: {} copied, {} old removed",
            moved.copied, moved.removed
        );
        if !complete {
            return;
        }
        for origin in &self.legacy {
            if !state.moved.contains(origin) {
                state.moved.push(origin.clone());
            }
        }
        let written = serde_json::to_vec_pretty(&state)
            .map_err(|e| e.to_string())
            .and_then(|json| std::fs::write(&state_path, json).map_err(|e| e.to_string()));
        if let Err(error) = written {
            eprintln!("kuzz-desktop: recording the sign-in move failed: {error}");
        }
    }
}

/// What one launch's move did (for logs and tests).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Moved {
    /// Slots copied to the scope.
    pub copied: usize,
    /// Old slots removed.
    pub removed: usize,
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

/// Launch step, before anything reads the account list (see
/// [`Service::migrate_on_launch`]).
pub(crate) fn migrate_on_launch(secrets: &dyn Secrets, data_dir: &Path) {
    service().migrate_on_launch(secrets, data_dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn test_service() -> Service {
        Service::new(
            "kuzz",
            "wss://jihobuzz.duckdns.org",
            &["wss://jihobuzz.duckdns.org:8443", ""],
        )
    }

    #[test]
    fn service_addresses_map_to_the_scope_and_back_to_the_current_origin() {
        let s = test_service();
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
        let s = test_service();
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
        let s = test_service();
        assert_eq!(
            s.current_url("https://jihobuzz.duckdns.org:8443/media/ab.jpg?x=1"),
            "https://jihobuzz.duckdns.org/media/ab.jpg?x=1"
        );
        assert_eq!(
            s.current_url("https://relay.example/media/ab.jpg"),
            "https://relay.example/media/ab.jpg"
        );
    }

    /// Secure storage as `SecretStore` behaves on Windows: refresh tokens and
    /// account slots are standalone credentials (`auth.refresh.*` leaves an
    /// empty blob marker), everything else shares one blob capped at 1,280
    /// UTF-16 units, and a blob write past the cap fails.
    #[derive(Default)]
    struct WinSecrets {
        blob: Mutex<HashMap<String, String>>,
        creds: Mutex<HashMap<String, String>>,
    }

    const BLOB_CAP: usize = 1280;

    fn standalone(key: &str) -> bool {
        key.starts_with("auth.refresh.") || key.starts_with("account.")
    }

    impl WinSecrets {
        fn units(map: &HashMap<String, String>) -> usize {
            serde_json::to_string(map).unwrap().encode_utf16().count()
        }
        fn blob_units(&self) -> usize {
            Self::units(&self.blob.lock().unwrap())
        }
        fn mutate(&self, f: impl FnOnce(&mut HashMap<String, String>)) -> Result<(), String> {
            let mut blob = self.blob.lock().unwrap();
            let mut next = blob.clone();
            f(&mut next);
            if Self::units(&next) > BLOB_CAP {
                return Err("keyring write failed: blob over the platform limit".into());
            }
            *blob = next;
            Ok(())
        }
        fn has(&self, key: &str) -> bool {
            self.load(key).unwrap().is_some()
        }
        fn get(&self, key: &str) -> Option<String> {
            self.load(key).unwrap()
        }
    }

    impl Secrets for WinSecrets {
        fn load(&self, key: &str) -> Result<Option<String>, String> {
            if standalone(key) {
                return Ok(self.creds.lock().unwrap().get(key).cloned());
            }
            Ok(self.blob.lock().unwrap().get(key).cloned())
        }
        fn store(&self, key: &str, value: &str) -> Result<(), String> {
            if standalone(key) {
                if !key.starts_with("account.") {
                    self.mutate(|map| {
                        map.insert(key.into(), String::new());
                    })?;
                }
                self.creds.lock().unwrap().insert(key.into(), value.into());
                return Ok(());
            }
            self.mutate(|map| {
                map.insert(key.into(), value.into());
            })
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            if standalone(key) {
                self.creds.lock().unwrap().remove(key);
                if key.starts_with("account.") {
                    return Ok(());
                }
            }
            self.mutate(|map| {
                map.remove(key);
            })
        }
        fn store_all(&self, entries: &HashMap<String, String>) -> Result<(), String> {
            self.mutate(|map| {
                for (k, v) in entries {
                    if !standalone(k) {
                        map.insert(k.clone(), v.clone());
                    } else if !k.starts_with("account.") {
                        map.insert(k.clone(), String::new());
                    }
                }
            })?;
            for (k, v) in entries.iter().filter(|(k, _)| standalone(k)) {
                self.creds.lock().unwrap().insert(k.clone(), v.clone());
            }
            Ok(())
        }
        fn blob_keys(&self) -> Result<Vec<String>, String> {
            Ok(self.blob.lock().unwrap().keys().cloned().collect())
        }
        fn move_in_blob(&self, moves: &[(String, Option<String>)]) -> Result<(), String> {
            self.mutate(|map| {
                for (from, to) in moves {
                    if let Some(value) = map.remove(from) {
                        if let Some(to) = to {
                            map.entry(to.clone()).or_insert(value);
                        }
                    }
                }
            })
        }
    }

    const OLD: &str = "https://jihobuzz.duckdns.org:8443";

    /// This test build's service plus the earlier `:8443` address, so the
    /// account code (which maps origins through the built-in service) and
    /// the move agree on the scope.
    fn upgrade_service() -> Service {
        Service::new(
            super::scope(),
            &super::service().current,
            &["wss://jihobuzz.duckdns.org:8443"],
        )
    }

    fn nsec(keys: &nostr::Keys) -> String {
        use nostr::ToBech32;
        keys.secret_key().to_bech32().unwrap()
    }

    fn write_registry(dir: &Path, active: &nostr::Keys, accounts: &[(&nostr::Keys, &str)]) {
        let accounts: Vec<serde_json::Value> = accounts
            .iter()
            .map(|(keys, state)| {
                serde_json::json!({
                    "pubkey": keys.public_key().to_hex(),
                    "origin": super::scope(),
                    "state": state,
                })
            })
            .collect();
        let json = serde_json::json!({
            "version": 1,
            "active": active.public_key().to_hex(),
            "accounts": accounts,
            "pending": [],
            "owners_migrated": true,
        });
        std::fs::write(
            dir.join(super::super::accounts::FILE_NAME),
            serde_json::to_vec_pretty(&json).unwrap(),
        )
        .unwrap();
    }

    /// Agent keys filling the blob up to `free` units below the cap.
    fn fill_blob(secrets: &WinSecrets, free: usize) {
        let mut index = 0;
        loop {
            let name = format!("agent:{index:064x}");
            let value = format!("nsec1{}", "q".repeat(58));
            let mut next = secrets.blob.lock().unwrap().clone();
            next.insert(name.clone(), value.clone());
            if WinSecrets::units(&next) > BLOB_CAP - free {
                break;
            }
            secrets.blob.lock().unwrap().insert(name, value);
            index += 1;
        }
        // Pad the rest with one short entry (`,"pad":"…"` costs 9 units more
        // than its value) so exactly `free` units remain.
        let room = BLOB_CAP - free - secrets.blob_units();
        if room > 9 {
            let pad = "p".repeat(room - 9);
            secrets.blob.lock().unwrap().insert("pad".into(), pad);
        }
        assert!(BLOB_CAP - secrets.blob_units() >= free);
        assert!(BLOB_CAP - secrets.blob_units() <= free + 9);
    }

    fn slot(keys: &nostr::Keys, origin: &str) -> String {
        format!("account.{}.refresh.{origin}", keys.public_key().to_hex())
    }

    /// What an app built before the move leaves with two accounts signed in
    /// (`a` active) on the old address, with agent keys filling the blob to
    /// the same 34 units of room found on a real upgraded Windows install.
    fn old_two_account_install(dir: &Path, a: &nostr::Keys, b: &nostr::Keys) -> WinSecrets {
        let secrets = WinSecrets::default();
        for keys in [a, b] {
            secrets
                .store(
                    &format!("account.{}.identity", keys.public_key().to_hex()),
                    &nsec(keys),
                )
                .unwrap();
        }
        secrets.store("identity", &nsec(a)).unwrap();
        secrets.store("identity.bootstrap", "").unwrap();
        secrets
            .store(&format!("auth.key-backup.{OLD}"), &a.public_key().to_hex())
            .unwrap();
        secrets
            .store(&format!("auth.refresh.{OLD}"), "bzr_a1")
            .unwrap();
        secrets.store(&slot(a, OLD), "bzr_a1").unwrap();
        secrets.store(&slot(b, OLD), "bzr_b1").unwrap();
        fill_blob(&secrets, 34);
        write_registry(dir, a, &[(a, "signed_in"), (b, "signed_in")]);
        secrets
    }

    fn state(dir: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(dir.join(MIGRATION_FILE)).unwrap()).unwrap()
    }

    #[test]
    fn two_signed_in_accounts_upgrade_on_a_nearly_full_windows_blob() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (nostr::Keys::generate(), nostr::Keys::generate());
        let secrets = old_two_account_install(dir.path(), &a, &b);
        let s = upgrade_service();
        let scope = super::scope();

        s.migrate_on_launch(&secrets, dir.path());

        // Both accounts keep their own, unspent tokens under the scope; the
        // active copy and binding are the active account's.
        assert_eq!(secrets.get(&slot(&a, scope)).as_deref(), Some("bzr_a1"));
        assert_eq!(secrets.get(&slot(&b, scope)).as_deref(), Some("bzr_b1"));
        assert_eq!(
            secrets.get(&format!("auth.refresh.{scope}")).as_deref(),
            Some("bzr_a1")
        );
        assert_eq!(
            secrets.get(&format!("auth.key-backup.{scope}")),
            Some(a.public_key().to_hex())
        );
        // No old name is left to be copied (or spent) again.
        for key in [
            format!("auth.refresh.{OLD}"),
            format!("auth.key-backup.{OLD}"),
            slot(&a, OLD),
            slot(&b, OLD),
        ] {
            assert!(!secrets.has(&key), "{key}");
            assert!(!secrets.blob_keys().unwrap().contains(&key), "{key}");
        }
        assert!(secrets.blob_units() <= BLOB_CAP);
        assert_eq!(
            state(dir.path())["moved"].as_array().map(Vec::len),
            Some(s.legacy.len())
        );

        // The account code then finds both accounts signed in, `a` active.
        super::super::accounts::reconcile(&secrets, dir.path()).unwrap();
        let registry = super::super::accounts::load_registry(dir.path())
            .unwrap()
            .unwrap();
        assert_eq!(registry.active, Some(a.public_key().to_hex()));
        assert!(registry
            .accounts
            .iter()
            .all(|account| account.state == super::super::accounts::AccountState::SignedIn));
        assert_eq!(registry.inactive_signed_in().len(), 1);
    }

    #[test]
    fn a_token_used_or_signed_out_under_the_scope_never_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (nostr::Keys::generate(), nostr::Keys::generate());
        let secrets = old_two_account_install(dir.path(), &a, &b);
        let s = upgrade_service();
        let scope = super::scope();
        s.migrate_on_launch(&secrets, dir.path());

        // `b` signs out (its slot is deleted); `a`'s session rotates.
        secrets.delete(&slot(&b, scope)).unwrap();
        secrets.store(&slot(&a, scope), "bzr_a2").unwrap();
        // Later launches, even with the record lost, bring nothing back.
        s.migrate_on_launch(&secrets, dir.path());
        std::fs::remove_file(dir.path().join(MIGRATION_FILE)).unwrap();
        s.migrate_on_launch(&secrets, dir.path());
        assert_eq!(secrets.get(&slot(&b, scope)), None);
        assert_eq!(secrets.get(&slot(&a, scope)).as_deref(), Some("bzr_a2"));
    }

    #[test]
    fn an_old_token_account_mark_is_dropped_not_moved() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = WinSecrets::default();
        let scope = super::scope();
        secrets
            .store(&format!("auth.legacy-token.{OLD}"), "token")
            .unwrap();
        secrets
            .store(&format!("auth.refresh.{OLD}"), "bzr_t")
            .unwrap();

        upgrade_service().migrate_on_launch(&secrets, dir.path());

        assert!(!secrets.has(&format!("auth.legacy-token.{scope}")));
        assert!(!secrets.has(&format!("auth.legacy-token.{OLD}")));
        assert_eq!(
            secrets.get(&format!("auth.refresh.{scope}")).as_deref(),
            Some("bzr_t")
        );
    }

    #[test]
    fn the_move_never_grows_a_full_blob() {
        // No binding to shrink first, and less room than one more marker.
        let dir = tempfile::tempdir().unwrap();
        let secrets = WinSecrets::default();
        let scope = super::scope();
        secrets
            .store(&format!("auth.refresh.{OLD}"), "bzr_t")
            .unwrap();
        fill_blob(&secrets, 12);

        upgrade_service().migrate_on_launch(&secrets, dir.path());

        assert_eq!(
            secrets.get(&format!("auth.refresh.{scope}")).as_deref(),
            Some("bzr_t")
        );
        assert!(!secrets.has(&format!("auth.refresh.{OLD}")));
        assert!(!secrets
            .blob_keys()
            .unwrap()
            .contains(&format!("auth.refresh.{OLD}")));
    }

    #[test]
    fn migration_reads_the_current_address_slot_too() {
        // A build whose address did not change still moves to the scope.
        let s = Service::new("kuzz", "wss://jihobuzz.duckdns.org:8443", &[]);
        let secrets = WinSecrets::default();
        secrets
            .store("auth.refresh.https://jihobuzz.duckdns.org:8443", "bzr")
            .unwrap();
        let moved = s.move_secrets(&secrets, &[], &[]).unwrap();
        assert_eq!(moved.copied, 1);
        assert_eq!(
            secrets.load("auth.refresh.kuzz").unwrap().as_deref(),
            Some("bzr")
        );
        assert!(!secrets.has("auth.refresh.https://jihobuzz.duckdns.org:8443"));
    }
}
