//! Relay-wide runtime settings read without a restart: an operator sets them
//! with `kuzz-admin settings set`, every pod picks the new value up within
//! [`RUNTIME_SETTINGS_TTL`].

use std::time::{Duration, Instant};

use kuzz_core::window_check::{self, WindowCheckMode};

use crate::state::AppState;

/// How long a pod serves a cached setting before reading it again.
pub const RUNTIME_SETTINGS_TTL: Duration = Duration::from_secs(30);

/// How long a pod serves the default after a failed read before trying again.
pub const RUNTIME_SETTINGS_ERROR_TTL: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct Cached {
    value: Option<String>,
    fresh_until: Instant,
}

/// Per-pod cache of runtime setting values (`None` = not set).
pub struct RuntimeSettingsCache {
    values: moka::sync::Cache<&'static str, Cached>,
    ttl: Duration,
    error_ttl: Duration,
}

impl RuntimeSettingsCache {
    /// Empty cache with the standard TTLs.
    pub fn new() -> Self {
        Self::with_ttls(RUNTIME_SETTINGS_TTL, RUNTIME_SETTINGS_ERROR_TTL)
    }

    /// Empty cache with explicit TTLs (tests use short ones).
    pub(crate) fn with_ttls(ttl: Duration, error_ttl: Duration) -> Self {
        Self {
            values: moka::sync::Cache::builder()
                .max_capacity(64)
                .time_to_live(ttl.max(error_ttl))
                .build(),
            ttl,
            error_ttl,
        }
    }

    fn get(&self, key: &'static str) -> Option<Option<String>> {
        self.values
            .get(&key)
            .filter(|cached| Instant::now() < cached.fresh_until)
            .map(|cached| cached.value)
    }

    fn insert(&self, key: &'static str, value: Option<String>, ttl: Duration) {
        let fresh_until = Instant::now() + ttl;
        self.values.insert(key, Cached { value, fresh_until });
    }
}

impl Default for RuntimeSettingsCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RuntimeSettingsCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeSettingsCache")
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// The value of runtime setting `key`, at most [`RUNTIME_SETTINGS_TTL`]
    /// old. A failed read is logged and served as unset (the setting's
    /// default) for [`RUNTIME_SETTINGS_ERROR_TTL`], so a settings outage never
    /// fails the request that asked.
    pub async fn runtime_setting(&self, key: &'static str) -> Option<String> {
        let cache = &self.runtime_settings;
        if let Some(hit) = cache.get(key) {
            return hit;
        }
        match self.db.get_runtime_setting(key).await {
            Ok(value) => {
                cache.insert(key, value.clone(), cache.ttl);
                value
            }
            Err(e) => {
                tracing::warn!(key, error = %e, "runtime setting read failed; serving the default");
                cache.insert(key, None, cache.error_ttl);
                None
            }
        }
    }

    /// The window check mode devices follow (missing, invalid or unreadable
    /// = `off`).
    pub async fn window_check_mode(&self) -> WindowCheckMode {
        let value = self.runtime_setting(window_check::SETTING_KEY).await;
        WindowCheckMode::from_setting(value.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_values_expire_after_their_ttl() {
        let cache =
            RuntimeSettingsCache::with_ttls(Duration::from_secs(30), Duration::from_millis(50));
        cache.insert("k", Some("on".into()), cache.ttl);
        cache.insert("e", None, cache.error_ttl);
        assert_eq!(cache.get("k"), Some(Some("on".into())));
        assert_eq!(cache.get("e"), Some(None));
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(
            cache.get("k"),
            Some(Some("on".into())),
            "a good read lasts the full TTL"
        );
        assert_eq!(cache.get("e"), None, "a failed read is retried soon");
    }
}
