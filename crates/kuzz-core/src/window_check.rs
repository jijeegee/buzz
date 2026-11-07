//! NIP-CW window checks: the relay-wide mode switch devices follow.
//!
//! A window check (`"check": true` on a room or thread window filter) runs
//! the page query and answers only the window's oldest row id and row count.
//! The mode tells devices whether to rely on it: `off` (today's page fetches
//! only), `shadow` (check and log, still fetch) or `on` (skip the page fetch
//! when the check matches the store).

/// Runtime setting key holding the mode (`relay_runtime_settings.key`).
pub const SETTING_KEY: &str = "window_check_mode";

/// How devices use window checks. Missing or unknown values mean [`Self::Off`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowCheckMode {
    /// Devices fetch pages as before and send no checks for their own use.
    #[default]
    Off,
    /// Devices check, log the result, and still fetch pages as before.
    Shadow,
    /// Devices skip the page fetch when the check matches their store.
    On,
}

impl WindowCheckMode {
    /// Every mode, in wire order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Shadow, Self::On];

    /// Wire and setting value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Shadow => "shadow",
            Self::On => "on",
        }
    }

    /// Parse an exact wire value (`off`, `shadow`, `on`).
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }

    /// The mode a stored setting value names: missing or invalid is `off`.
    pub fn from_setting(value: Option<&str>) -> Self {
        value.and_then(Self::parse).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_values_and_defaults_to_off() {
        for mode in WindowCheckMode::ALL {
            assert_eq!(WindowCheckMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(WindowCheckMode::parse("ON"), None);
        assert_eq!(WindowCheckMode::parse(" on"), None);
        assert_eq!(WindowCheckMode::from_setting(None), WindowCheckMode::Off);
        assert_eq!(
            WindowCheckMode::from_setting(Some("bogus")),
            WindowCheckMode::Off
        );
        assert_eq!(
            WindowCheckMode::from_setting(Some("shadow")),
            WindowCheckMode::Shadow
        );
    }
}
