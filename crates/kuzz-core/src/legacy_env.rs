//! Accept environment variables written under the product's former name.
//!
//! Deployed servers, saved agent settings and build scripts still set
//! variables under the former prefix. Each binary copies them to their `KUZZ_*` names at
//! startup so both spellings keep working.

/// Prefix used before the rename. kuzz-rename: keep
pub const LEGACY_PREFIX: &str = "BUZZ_"; // kuzz-rename: keep

/// Current prefix.
pub const PREFIX: &str = "KUZZ_";

/// Maps a former variable name to its current name, if it used the old prefix.
pub fn current_name(name: &str) -> Option<String> {
    name.strip_prefix(LEGACY_PREFIX)
        .map(|rest| format!("{PREFIX}{rest}"))
}

/// Copies every former-prefix variable to its `KUZZ_*` name unless the new name is
/// already set. Call from a synchronous `main` before any thread starts.
pub fn adopt_legacy_env() {
    let pending: Vec<(String, std::ffi::OsString)> = std::env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.to_str()?;
            let current = current_name(key)?;
            if std::env::var_os(&current).is_some() {
                return None;
            }
            Some((current, value))
        })
        .collect();
    for (key, value) in pending {
        std::env::set_var(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_only_the_former_prefix() {
        assert_eq!(
            current_name("BUZZ_RELAY_URL").as_deref(), // kuzz-rename: keep
            Some("KUZZ_RELAY_URL")
        );
        assert_eq!(current_name("KUZZ_RELAY_URL"), None);
        assert_eq!(current_name("PATH"), None);
    }
}
