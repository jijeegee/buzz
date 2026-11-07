//! `kuzz-admin settings`: read and change relay-wide runtime settings
//! (`relay_runtime_settings`). Relays pick a change up within 30 s, without a
//! restart. Only known keys with valid values are accepted.

use anyhow::{anyhow, Result};
use clap::Subcommand;
use kuzz_core::window_check::{self, WindowCheckMode};

/// Runtime setting subcommands.
#[derive(Subcommand)]
pub enum SettingsCommand {
    /// Print a setting's effective value (its default when unset).
    Get {
        /// Setting key, e.g. `window_check_mode`.
        key: String,
    },
    /// Store a setting's value.
    Set {
        /// Setting key, e.g. `window_check_mode`.
        key: String,
        /// New value; `window_check_mode` takes off, shadow or on.
        value: String,
    },
}

/// The canonical value `value` stores for `key`, or why it is refused.
fn validate(key: &str, value: &str) -> Result<&'static str> {
    match key {
        window_check::SETTING_KEY => WindowCheckMode::parse(value)
            .map(WindowCheckMode::as_str)
            .ok_or_else(|| anyhow!("{key} must be one of: off, shadow, on")),
        _ => Err(anyhow!(
            "unknown setting {key:?} (known: {})",
            window_check::SETTING_KEY
        )),
    }
}

/// The effective value of `key` given its stored value.
fn effective(key: &str, stored: Option<&str>) -> Result<&'static str> {
    match key {
        window_check::SETTING_KEY => Ok(WindowCheckMode::from_setting(stored).as_str()),
        _ => Err(anyhow!(
            "unknown setting {key:?} (known: {})",
            window_check::SETTING_KEY
        )),
    }
}

/// Run one settings subcommand; prints one JSON line.
pub async fn run(command: SettingsCommand) -> Result<i32> {
    match command {
        SettingsCommand::Get { key } => {
            effective(&key, None)?;
            let db = crate::connect_db().await?;
            let stored = db.get_runtime_setting(&key).await?;
            println!(
                "{}",
                serde_json::json!({
                    "key": key,
                    "value": effective(&key, stored.as_deref())?,
                    "stored": stored,
                })
            );
        }
        SettingsCommand::Set { key, value } => {
            let value = validate(&key, &value)?;
            let db = crate::connect_db().await?;
            db.set_runtime_setting(&key, value).await?;
            println!("{}", serde_json::json!({ "key": key, "value": value }));
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn validates_known_keys_and_values() {
        assert_eq!(validate("window_check_mode", "shadow").unwrap(), "shadow");
        assert!(validate("window_check_mode", "ON").is_err());
        assert!(validate("other", "on").is_err());
        assert_eq!(effective("window_check_mode", None).unwrap(), "off");
        assert_eq!(effective("window_check_mode", Some("bad")).unwrap(), "off");
        assert!(effective("other", None).is_err());
    }

    #[test]
    fn parses_get_and_set() {
        let cli = crate::Cli::try_parse_from([
            "kuzz-admin",
            "settings",
            "set",
            "window_check_mode",
            "on",
        ])
        .expect("set parses");
        assert!(matches!(
            cli.command,
            crate::Command::Settings {
                command: SettingsCommand::Set { ref key, ref value }
            } if key == "window_check_mode" && value == "on"
        ));
        assert!(
            crate::Cli::try_parse_from(["kuzz-admin", "settings", "get", "window_check_mode"])
                .is_ok()
        );
        assert!(
            crate::Cli::try_parse_from(["kuzz-admin", "settings", "set", "window_check_mode"])
                .is_err()
        );
    }
}
