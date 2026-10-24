//! Pure coverage-range logic.
//!
//! A range proves every event of its class whose position lies between its
//! lower end and `hi_at` is stored locally. Positions follow relay order:
//! a larger `created_at` is newer; within one second a larger id is older
//! (relays page `created_at DESC, id ASC`). A start range (`lo_is_start`)
//! reaches the beginning of the room.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// Sorts after every hex id: the oldest position within its second, so a
/// lower end `(t, TIME_FLOOR_ID)` covers every event created at `t` or later.
pub(crate) const TIME_FLOOR_ID: &str = "~";

/// Upper end of a live-covered run: open-ended.
pub(crate) const LIVE_HI: i64 = i64::MAX;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Cursor {
    pub(crate) created_at: i64,
    pub(crate) event_id: String,
}

/// Position order: `Greater` is newer.
pub(crate) fn cmp_pos(a_at: i64, a_id: &str, b_at: i64, b_id: &str) -> Ordering {
    a_at.cmp(&b_at).then_with(|| b_id.cmp(a_id))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Range {
    pub(crate) lo_at: i64,
    pub(crate) lo_id: String,
    pub(crate) lo_is_start: bool,
    pub(crate) hi_at: i64,
    pub(crate) kinds_version: i64,
}

impl Range {
    /// Whether the event at `(at, id)` lies inside the range.
    pub(crate) fn contains(&self, at: i64, id: &str) -> bool {
        at <= self.hi_at
            && (self.lo_is_start || cmp_pos(at, id, self.lo_at, &self.lo_id) != Ordering::Less)
    }

    fn cmp_lo(&self, other: &Self) -> Ordering {
        match (self.lo_is_start, other.lo_is_start) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => cmp_pos(self.lo_at, &self.lo_id, other.lo_at, &other.lo_id),
        }
    }
}

/// Upper end of a claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Upper {
    /// Everything created at or before this time (a head fetch, a socket close).
    At(i64),
    /// Everything strictly older than this cursor (a page fetched with it).
    #[cfg_attr(not(test), allow(dead_code))] // Wired by the write-path change.
    Before(Cursor),
}

/// What one completed fetch proves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Claim {
    /// Oldest position returned; `None` only for an empty final page.
    pub(crate) lo: Option<Cursor>,
    /// The fetch reached the beginning (`has_more: false`).
    pub(crate) lo_is_start: bool,
    pub(crate) hi: Upper,
}

/// Merges ranges that overlap or meet; drops ranges proven under another kind
/// list. Result is ordered oldest first.
pub(crate) fn normalize(mut ranges: Vec<Range>, kinds_version: i64) -> Vec<Range> {
    ranges.retain(|range| range.kinds_version == kinds_version);
    ranges.sort_by(|a, b| a.cmp_lo(b).then(a.hi_at.cmp(&b.hi_at)));
    let mut merged: Vec<Range> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            // Sorted by lower end, so `range` starts at or above `last`'s
            // lower end; it continues `last` when it starts at or below its top.
            Some(last) if range.lo_is_start || range.lo_at <= last.hi_at => {
                last.hi_at = last.hi_at.max(range.hi_at);
            }
            _ => merged.push(range),
        }
    }
    merged
}

/// Adds one claim to a room's ranges of one class. A page fetched below a
/// cursor joins the range whose lower end is exactly that cursor; on its own
/// it proves only the whole seconds below the cursor.
pub(crate) fn apply_claim(existing: Vec<Range>, claim: &Claim, kinds_version: i64) -> Vec<Range> {
    let mut ranges = normalize(existing, kinds_version);
    let (lo_at, lo_id) = match (&claim.lo, claim.lo_is_start) {
        (Some(lo), _) => (lo.created_at, lo.event_id.clone()),
        (None, true) => (0, String::new()),
        (None, false) => return ranges,
    };
    let hi_at = match &claim.hi {
        Upper::At(at) => *at,
        Upper::Before(cursor) => ranges
            .iter()
            .find(|range| {
                !range.lo_is_start
                    && range.lo_at == cursor.created_at
                    && range.lo_id == cursor.event_id
            })
            .map_or(cursor.created_at - 1, |touched| touched.hi_at),
    };
    let range = Range {
        lo_at,
        lo_id,
        lo_is_start: claim.lo_is_start,
        hi_at,
        kinds_version,
    };
    if !range.lo_is_start && range.lo_at > range.hi_at {
        return ranges;
    }
    ranges.push(range);
    normalize(ranges, kinds_version)
}

/// The range containing position `(at, id)`, if any.
pub(crate) fn run_containing<'a>(ranges: &'a [Range], at: i64, id: &str) -> Option<&'a Range> {
    ranges.iter().find(|range| range.contains(at, id))
}

/// A room's `top` view: window ranges plus `all` ranges (which prove more).
pub(crate) fn effective_top(top: &[Range], all: &[Range], kinds_version: i64) -> Vec<Range> {
    normalize(top.iter().chain(all).cloned().collect(), kinds_version)
}

/// Adds an open live segment as an open-ended range from `since`.
pub(crate) fn with_live(
    ranges: &[Range],
    live_since: Option<i64>,
    kinds_version: i64,
) -> Vec<Range> {
    let mut ranges = ranges.to_vec();
    if let Some(since) = live_since {
        ranges.push(Range {
            lo_at: since,
            lo_id: TIME_FLOOR_ID.to_string(),
            lo_is_start: false,
            hi_at: LIVE_HI,
            kinds_version,
        });
    }
    normalize(ranges, kinds_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: i64 = 1;

    fn cursor(created_at: i64, id: &str) -> Cursor {
        Cursor {
            created_at,
            event_id: id.to_string(),
        }
    }

    fn range(lo_at: i64, lo_id: &str, start: bool, hi_at: i64) -> Range {
        Range {
            lo_at,
            lo_id: lo_id.to_string(),
            lo_is_start: start,
            hi_at,
            kinds_version: V,
        }
    }

    fn claim(lo: Option<Cursor>, start: bool, hi: Upper) -> Claim {
        Claim {
            lo,
            lo_is_start: start,
            hi,
        }
    }

    #[test]
    fn position_order_is_relay_order() {
        assert_eq!(cmp_pos(2, "a", 1, "a"), Ordering::Greater);
        // Same second: the smaller id comes first in relay order, so is newer.
        assert_eq!(cmp_pos(5, "a", 5, "b"), Ordering::Greater);
        assert_eq!(cmp_pos(5, "f", 5, TIME_FLOOR_ID), Ordering::Greater);
    }

    #[test]
    fn overlapping_ranges_merge() {
        let ranges = apply_claim(
            vec![range(100, "a", false, 200)],
            &claim(Some(cursor(50, "b")), false, Upper::At(150)),
            V,
        );
        assert_eq!(ranges, vec![range(50, "b", false, 200)]);
    }

    #[test]
    fn page_below_a_cursor_touches_the_range_above() {
        // The head page ended at (100, "a"); the next page was fetched with
        // that cursor and proves everything strictly older than it.
        let head = vec![range(100, "a", false, 300)];
        let ranges = apply_claim(
            head,
            &claim(
                Some(cursor(100, "c")),
                false,
                Upper::Before(cursor(100, "a")),
            ),
            V,
        );
        assert_eq!(ranges, vec![range(100, "c", false, 300)]);
    }

    #[test]
    fn page_below_an_unknown_cursor_proves_whole_seconds_only() {
        let ranges = apply_claim(
            vec![],
            &claim(Some(cursor(40, "c")), false, Upper::Before(cursor(90, "a"))),
            V,
        );
        assert_eq!(ranges, vec![range(40, "c", false, 89)]);
        // Rows all in the cursor's own second prove nothing on their own.
        let none = apply_claim(
            vec![],
            &claim(Some(cursor(90, "c")), false, Upper::Before(cursor(90, "a"))),
            V,
        );
        assert!(none.is_empty());
    }

    #[test]
    fn separate_ranges_stay_separate() {
        let ranges = apply_claim(
            vec![range(300, "a", false, 400)],
            &claim(Some(cursor(100, "b")), false, Upper::At(200)),
            V,
        );
        assert_eq!(
            ranges,
            vec![range(100, "b", false, 200), range(300, "a", false, 400)]
        );
        // One second apart is still a gap: (201, x) may hide older ids.
        let gap = normalize(
            vec![range(100, "b", false, 200), range(201, "a", false, 300)],
            V,
        );
        assert_eq!(gap.len(), 2);
    }

    #[test]
    fn start_flag_reaches_the_beginning() {
        let ranges = apply_claim(
            vec![range(100, "a", false, 300)],
            &claim(Some(cursor(20, "z")), true, Upper::Before(cursor(100, "a"))),
            V,
        );
        assert_eq!(ranges, vec![range(20, "z", true, 300)]);
        assert!(ranges[0].contains(1, "ff"));
        // An empty final page proves the room starts here.
        let empty = apply_claim(vec![], &claim(None, true, Upper::At(10)), V);
        assert_eq!(empty, vec![range(0, "", true, 10)]);
        // An empty non-final page proves nothing.
        assert!(apply_claim(vec![], &claim(None, false, Upper::At(10)), V).is_empty());
    }

    #[test]
    fn stale_kinds_version_is_ignored() {
        let mut stale = range(0, "", true, 500);
        stale.kinds_version = V - 1;
        let ranges = apply_claim(
            vec![stale],
            &claim(Some(cursor(400, "a")), false, Upper::At(600)),
            V,
        );
        assert_eq!(ranges, vec![range(400, "a", false, 600)]);
    }

    #[test]
    fn live_segment_extends_a_reaching_range() {
        let ranges = with_live(&[range(100, "a", false, 300)], Some(250), V);
        assert_eq!(ranges, vec![range(100, "a", false, LIVE_HI)]);
        let apart = with_live(&[range(100, "a", false, 300)], Some(400), V);
        assert_eq!(apart.len(), 2);
        assert_eq!(apart[1].lo_id, TIME_FLOOR_ID);
    }

    #[test]
    fn all_ranges_count_as_top() {
        let top = vec![range(100, "a", false, 200)];
        let all = vec![range(150, "b", false, 400)];
        assert_eq!(
            effective_top(&top, &all, V),
            vec![range(100, "a", false, 400)]
        );
    }
}
