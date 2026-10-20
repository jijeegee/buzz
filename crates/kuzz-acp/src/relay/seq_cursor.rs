//! Channel order numbers (`channel_sequence`) from a relay that speaks
//! `kuzz-seq` (the socket connects with `?kuzz_seq=1`).
//!
//! Only signals from a `kuzz-seq` relay on the current connection pin a
//! replay point, and the next channel REQ then asks for "events after order
//! number N" (`after_seq`) instead of "events since time T":
//! - `KUZZ_RESYNC`: the relay may have lost events for this socket;
//! - an `after_seq` page that ended with `has_more`;
//! - an event dropped by local backpressure.
//!
//! Everything else (first subscribe, explicit subscribe, reconnect) keeps the
//! `since` path, and cursors are dropped with the socket, so a relay without
//! `kuzz-seq` (rollback, mixed deploy) never receives `after_seq`.
//!
//! Live order numbers are not dense for one subscriber (kind and `#p` filters
//! skip numbers), so a jump is no proof of a gap. And a gap can arrive before the
//! relay's resync signal, after which live events have already moved the
//! newest number past it. A resync therefore pins a number that was already
//! the newest [`SEQ_STABLE_AFTER`] ago, taken when the signal arrives (the REQ
//! may go out much later). Events seen again are dropped by the event-id dedup.
use std::collections::HashMap;
use std::time::Duration;

use nostr::Event;
use serde_json::Value;
use tokio::time::Instant;
use uuid::Uuid;

/// How long the newest number must have been the newest before replay may
/// start from it.
pub(super) const SEQ_STABLE_AFTER: Duration = Duration::from_secs(30);

/// Page size for `after_seq` replay (the relay caps it at 500).
pub(super) const AFTER_SEQ_PAGE_LIMIT: u64 = 500;

/// Kind of the relay-signed event that closes an `after_seq` page.
const BOUNDS_KIND: u16 = 39006;

#[derive(Debug, Default, Clone)]
pub(super) struct SeqCursor {
    /// Newest number seen.
    newest: Option<i64>,
    /// A number at least [`SEQ_STABLE_AFTER`] old: replay starts here.
    stable: Option<i64>,
    /// `newest` as of `candidate_at`; becomes `stable` once old enough.
    candidate: Option<i64>,
    candidate_at: Option<Instant>,
    /// Replay point the next channel REQ must start from (`after_seq`).
    pin: Option<i64>,
}

impl SeqCursor {
    /// Record a delivered event's number.
    pub(super) fn observe(&mut self, seq: i64, now: Instant) {
        self.promote(now);
        if self.candidate.is_none() {
            if let Some(newest) = self.newest {
                self.candidate = Some(newest);
                self.candidate_at = Some(now);
            }
        }
        self.newest = Some(self.newest.map_or(seq, |n| n.max(seq)));
    }

    /// An event with this number was dropped before it reached the harness.
    pub(super) fn dropped(&mut self, seq: i64) {
        self.pin_at(seq.saturating_sub(1).max(0));
    }

    /// The relay asked for a resync. Returns whether a replay point is known;
    /// without one the channel keeps the `since` path.
    pub(super) fn resync(&mut self, now: Instant) -> bool {
        match self.stable_at(now) {
            Some(stable) => {
                self.pin_at(stable);
                true
            }
            None => false,
        }
    }

    /// An `after_seq` page ended at `last_seq` with more to fetch.
    pub(super) fn page_continues(&mut self, last_seq: i64, now: Instant) {
        self.observe(last_seq, now);
        self.pin_at(last_seq);
    }

    /// The replay point for the next channel REQ, if any.
    pub(super) fn pinned(&self) -> Option<i64> {
        self.pin
    }

    /// A REQ from the pinned point went out.
    pub(super) fn clear_pin(&mut self) {
        self.pin = None;
    }

    #[cfg(test)]
    pub(super) fn newest_for_test(&self) -> Option<i64> {
        self.newest
    }

    fn pin_at(&mut self, seq: i64) {
        self.pin = Some(self.pin.map_or(seq, |p| p.min(seq)));
    }

    /// A number that has been the newest for at least [`SEQ_STABLE_AFTER`].
    fn stable_at(&self, now: Instant) -> Option<i64> {
        match (self.candidate, self.candidate_at) {
            (Some(c), Some(at)) if now.duration_since(at) >= SEQ_STABLE_AFTER => {
                Some(self.stable.map_or(c, |s| s.max(c)))
            }
            _ => self.stable,
        }
    }

    fn promote(&mut self, now: Instant) {
        if let (Some(c), Some(at)) = (self.candidate, self.candidate_at) {
            if now.duration_since(at) >= SEQ_STABLE_AFTER {
                self.stable = Some(self.stable.map_or(c, |s| s.max(c)));
                self.candidate = None;
                self.candidate_at = None;
            }
        }
    }
}

/// Per-channel cursors.
pub(super) type SeqCursors = HashMap<Uuid, SeqCursor>;

/// The page summary of an `after_seq` bounds event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PageBounds {
    pub(super) last_seq: i64,
    pub(super) has_more: bool,
}

/// `Some` when `event` is the bounds event closing an `after_seq` page (kind
/// 39006 with `order: "seq"`; the channel-window bounds share the kind).
pub(super) fn after_seq_bounds(event: &Event) -> Option<PageBounds> {
    if event.kind.as_u16() != BOUNDS_KIND {
        return None;
    }
    let content: Value = serde_json::from_str(&event.content).ok()?;
    if content.get("order").and_then(Value::as_str) != Some("seq") {
        return None;
    }
    Some(PageBounds {
        last_seq: content.get("last_seq")?.as_i64()?,
        has_more: content
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};

    #[tokio::test(start_paused = true)]
    async fn resync_pins_a_number_that_has_been_newest_long_enough() {
        let mut cursor = SeqCursor::default();
        let t0 = Instant::now();
        assert!(!cursor.resync(t0), "no numbers: keep since");
        assert_eq!(cursor.pinned(), None);

        cursor.observe(10, t0);
        cursor.observe(12, t0 + Duration::from_secs(1));
        // 10 became the candidate at t0+1s; it is not old enough yet.
        assert!(!cursor.resync(t0 + Duration::from_secs(5)));
        // A gap right before a resync may already be behind the newest (12);
        // replay still starts at 10.
        assert!(cursor.resync(t0 + Duration::from_secs(31)));
        assert_eq!(cursor.pinned(), Some(10));

        // The pin holds while live numbers move on before the REQ goes out.
        cursor.observe(20, t0 + Duration::from_secs(40));
        cursor.observe(30, t0 + Duration::from_secs(200));
        assert_eq!(cursor.pinned(), Some(10));
        cursor.clear_pin();
        assert!(cursor.resync(t0 + Duration::from_secs(300)));
        assert_eq!(cursor.pinned(), Some(20));
    }

    #[tokio::test(start_paused = true)]
    async fn pins_keep_the_lowest_point() {
        let mut cursor = SeqCursor::default();
        let t0 = Instant::now();
        cursor.page_continues(500, t0);
        assert_eq!(cursor.pinned(), Some(500));
        cursor.dropped(3);
        assert_eq!(cursor.pinned(), Some(2));
        cursor.page_continues(900, t0);
        assert_eq!(cursor.pinned(), Some(2));
        assert_eq!(cursor.newest_for_test(), Some(900));
    }

    #[test]
    fn bounds_event_is_told_apart_from_window_bounds() {
        let keys = Keys::generate();
        let seq_page = EventBuilder::new(
            Kind::Custom(39006),
            r#"{"order":"seq","after_seq":4,"last_seq":9,"has_more":true,"sequences":[]}"#,
        )
        .sign_with_keys(&keys)
        .unwrap();
        assert_eq!(
            after_seq_bounds(&seq_page),
            Some(PageBounds {
                last_seq: 9,
                has_more: true
            })
        );
        let window = EventBuilder::new(
            Kind::Custom(39006),
            r#"{"has_more":false,"next_cursor":null}"#,
        )
        .sign_with_keys(&keys)
        .unwrap();
        assert_eq!(after_seq_bounds(&window), None);
        let message = EventBuilder::new(Kind::Custom(9), r#"{"order":"seq","last_seq":1}"#)
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(after_seq_bounds(&message), None);
    }
}
