//! How far this computer's clock is from the relay's, from the `Date` header
//! of `/query` answers. Store opens rely on coverage times taken from this
//! clock, so a clock far off (or never compared) opens from the relay
//! (docs/message-sync.md#clock-check).

use std::sync::atomic::{AtomicI64, Ordering};

/// A larger difference turns store opens off; it is not corrected.
pub(crate) const MAX_CLOCK_SKEW_SECS: i64 = 60;

const UNKNOWN: i64 = i64::MIN;

static SKEW_SECS: AtomicI64 = AtomicI64::new(UNKNOWN);

/// Record the relay's `Date` header (RFC 7231 IMF-fixdate) against `now`.
pub(crate) fn note_server_date(header: Option<&str>, now: i64) {
    let Some(date) = header.and_then(|value| chrono::DateTime::parse_from_rfc2822(value).ok())
    else {
        return;
    };
    SKEW_SECS.store(now - date.timestamp(), Ordering::Relaxed);
}

/// Whether the last comparison was within [`MAX_CLOCK_SKEW_SECS`].
pub(crate) fn clock_trusted() -> bool {
    let skew = SKEW_SECS.load(Ordering::Relaxed);
    skew != UNKNOWN && skew.abs() <= MAX_CLOCK_SKEW_SECS
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    SKEW_SECS.store(UNKNOWN, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_far_from_the_relay_or_never_compared_is_not_trusted() {
        reset_for_tests();
        assert!(!clock_trusted());
        let relay = chrono::DateTime::parse_from_rfc2822("Thu, 29 Oct 2026 05:13:43 GMT")
            .unwrap()
            .timestamp();
        note_server_date(Some("Thu, 29 Oct 2026 05:13:43 GMT"), relay + 60);
        assert!(clock_trusted());
        note_server_date(Some("not a date"), relay + 600);
        assert!(clock_trusted());
        note_server_date(Some("Thu, 29 Oct 2026 05:13:43 GMT"), relay - 61);
        assert!(!clock_trusted());
    }
}
