//! Relay-wide runtime settings read without a restart: an operator sets them
//! with `kuzz-admin settings set`, every pod picks the new value up within
//! [`RUNTIME_SETTINGS_TTL`].

use std::time::Duration;

use kuzz_core::window_check::{self, WindowCheckMode};
use kuzz_db::DbError;

use crate::state::AppState;

/// How long a pod serves a cached setting before reading it again.
pub const RUNTIME_SETTINGS_TTL: Duration = Duration::from_secs(30);

/// Per-pod cache of runtime setting values (`None` = not set).
pub struct RuntimeSettingsCache {
    values: moka::sync::Cache<&'static str, Option<String>>,
}

impl RuntimeSettingsCache {
    /// Empty cache with the standard TTL.
    pub fn new() -> Self {
        Self::with_ttl(RUNTIME_SETTINGS_TTL)
    }

    fn with_ttl(ttl: Duration) -> Self {
        Self {
            values: moka::sync::Cache::builder()
                .max_capacity(64)
                .time_to_live(ttl)
                .build(),
        }
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
    /// The value of runtime setting `key`, at most [`RUNTIME_SETTINGS_TTL`] old.
    pub async fn runtime_setting(&self, key: &'static str) -> Result<Option<String>, DbError> {
        if let Some(hit) = self.runtime_settings.values.get(&key) {
            return Ok(hit);
        }
        let value = self.db.get_runtime_setting(key).await?;
        self.runtime_settings.values.insert(key, value.clone());
        Ok(value)
    }

    /// The window check mode devices follow (missing or invalid = `off`).
    pub async fn window_check_mode(&self) -> Result<WindowCheckMode, DbError> {
        let value = self.runtime_setting(window_check::SETTING_KEY).await?;
        Ok(WindowCheckMode::from_setting(value.as_deref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_values_expire_after_the_ttl() {
        let cache = RuntimeSettingsCache::with_ttl(Duration::from_millis(50));
        cache.values.insert("k", Some("on".into()));
        assert_eq!(cache.values.get(&"k"), Some(Some("on".into())));
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(cache.values.get(&"k"), None);
    }
}
