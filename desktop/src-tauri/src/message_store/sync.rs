//! Reconnect catch-up state for one scope (one account on one server).
//!
//! The watermark is the time before which the live socket is known to have
//! delivered every event it would deliver. Only the live socket moves it
//! (a round trip it answered, an event it delivered, or a catch-up run for
//! it); an HTTP `/query` answer never does, because it says nothing about
//! what the socket delivered. The store file is already per server origin,
//! so a second server never catches up from this server's time.

use rusqlite::Connection;
use serde_json::Value;

use super::{
    coverage::{Claim, Cursor, Upper, TIME_FLOOR_ID},
    ingest::{self, Class},
    schema::meta,
};

const WATERMARK_KEY: &str = "watermark";

/// How far a catch-up reaches below the watermark: the relay accepts events
/// up to 900 s in the future and stores them up to 960 s in the past, plus a
/// margin (the same lookback the per-room repair uses).
pub(crate) const CATCH_UP_LOOKBACK_SECS: i64 = 900 + 960 + 5;
/// A watermark older than this is not caught up from.
pub(crate) const CATCH_UP_MAX_AGE_SECS: i64 = 24 * 60 * 60;
/// Rooms per catch-up query (the relay's `#h` cap).
pub(crate) const CATCH_UP_CHUNK_ROOMS: usize = 128;
/// Rows per catch-up page (the relay's limit cap).
pub(crate) const CATCH_UP_PAGE_LIMIT: usize = 1000;
/// Pages per chunk before its rooms count as gaps.
pub(crate) const CATCH_UP_MAX_PAGES: usize = 3;

fn db(context: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |error| format!("message store {context}: {error}")
}

pub(crate) fn watermark(conn: &Connection) -> Result<Option<i64>, String> {
    Ok(meta(conn, WATERMARK_KEY)
        .map_err(db("read watermark"))?
        .and_then(|value| value.parse::<i64>().ok()))
}

/// Moves the watermark forward to `at` (never back).
pub(crate) fn advance_watermark(conn: &Connection, at: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value
         WHERE CAST(excluded.value AS INTEGER) > CAST(meta.value AS INTEGER)",
        rusqlite::params![WATERMARK_KEY, at.to_string()],
    )
    .map_err(db("advance watermark"))?;
    Ok(())
}

/// What a catch-up run decided before querying.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Nothing stored vouches for this server: nothing to catch up from.
    Fresh,
    /// The watermark is too old to catch up from.
    Stale,
    /// Query every room from `since`.
    From { since: i64 },
}

pub(crate) fn plan(watermark: Option<i64>, started_at: i64) -> Plan {
    match watermark {
        None => Plan::Fresh,
        Some(at) if started_at - at > CATCH_UP_MAX_AGE_SECS => Plan::Stale,
        Some(at) => Plan::From {
            since: (at - CATCH_UP_LOOKBACK_SECS).max(0),
        },
    }
}

/// The page cursor below `events` (the oldest in relay order), if the page
/// came back full.
pub(crate) fn next_page_cursor(events: &[Value]) -> Option<(i64, String)> {
    if events.len() < CATCH_UP_PAGE_LIMIT {
        return None;
    }
    events
        .iter()
        .filter_map(|event| {
            Some((
                event["created_at"].as_i64()?,
                event["id"].as_str()?.to_ascii_lowercase(),
            ))
        })
        // Relay order: newest first, then id ascending; the oldest is the
        // smallest time and, within it, the largest id.
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
}

/// What a complete chunk proves for each of its rooms: every synced event
/// from `since` up to when its first page was fetched.
pub(crate) fn chunk_claim(since: i64, fetched_at: i64) -> Claim {
    Claim {
        lo: Some(Cursor {
            created_at: since,
            event_id: TIME_FLOOR_ID.to_string(),
        }),
        lo_is_start: since == 0,
        hi: Upper::At(fetched_at),
    }
}

/// One catch-up run's writes. Applied in order: events, then the claims they
/// back, then the watermark. A crash in between leaves the old watermark, so
/// the next run simply repeats the window.
pub(crate) struct Commit {
    pub(crate) events: Vec<Value>,
    pub(crate) claims: Vec<(String, Claim)>,
}

/// Stores `commit`; then, if `watermark` (asked only after the claims are
/// written) gives a time, moves the watermark to it.
pub(crate) fn commit(
    conn: &mut Connection,
    commit: Commit,
    watermark: impl FnOnce() -> Option<i64>,
) -> Result<(), String> {
    ingest::insert_events(conn, None, &commit.events, super::record::unix_now())?;
    for (channel_id, claim) in &commit.claims {
        ingest::claim_room(conn, channel_id, Class::All, claim)?;
    }
    match watermark() {
        Some(at) => advance_watermark(conn, at),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_store::{ingest::room_ranges, schema};

    fn open() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = schema::open(&dir.path().join("s.sqlite3"), "scope").expect("open");
        (dir, conn)
    }

    #[test]
    fn watermark_only_moves_forward() {
        let (_dir, conn) = open();
        assert_eq!(watermark(&conn).unwrap(), None);
        advance_watermark(&conn, 100).unwrap();
        advance_watermark(&conn, 50).unwrap();
        assert_eq!(watermark(&conn).unwrap(), Some(100));
        advance_watermark(&conn, 150).unwrap();
        assert_eq!(watermark(&conn).unwrap(), Some(150));
    }

    #[test]
    fn plan_follows_the_watermark_age() {
        assert_eq!(plan(None, 1_000_000), Plan::Fresh);
        assert_eq!(
            plan(Some(1_000_000 - CATCH_UP_MAX_AGE_SECS - 1), 1_000_000),
            Plan::Stale
        );
        assert_eq!(
            plan(Some(999_000), 1_000_000),
            Plan::From {
                since: 999_000 - CATCH_UP_LOOKBACK_SECS
            }
        );
        assert_eq!(plan(Some(10), 20), Plan::From { since: 0 });
    }

    fn event(created_at: i64, id: char) -> Value {
        serde_json::json!({"created_at": created_at, "id": id.to_string().repeat(64)})
    }

    #[test]
    fn next_cursor_only_for_full_pages_and_is_the_oldest() {
        assert_eq!(next_page_cursor(&[event(5, 'a')]), None);
        let mut page: Vec<Value> = (0..CATCH_UP_PAGE_LIMIT - 2)
            .map(|_| event(50, 'b'))
            .collect();
        page.push(event(10, 'a'));
        page.push(event(10, 'c'));
        assert_eq!(next_page_cursor(&page), Some((10, "c".repeat(64))));
    }

    fn room_event(id: char, room: &str, created_at: i64) -> Value {
        serde_json::json!({
            "id": id.to_string().repeat(64),
            "pubkey": "f".repeat(64),
            "created_at": created_at,
            "kind": 9,
            "tags": [["h", room]],
            "content": "hi",
            "sig": "0".repeat(128),
        })
    }

    #[test]
    fn commit_stores_events_then_claims_then_the_watermark() {
        let (_dir, mut conn) = open();
        let room = "270f6caf-0feb-4055-93f3-cdbeb567ff28";
        commit(
            &mut conn,
            Commit {
                events: vec![room_event('a', room, 150)],
                claims: vec![(room.to_string(), chunk_claim(100, 200))],
            },
            || Some(190),
        )
        .unwrap();
        let stored: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, 1);
        let ranges = room_ranges(&conn, room, Class::All).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!((ranges[0].lo_at, ranges[0].hi_at), (100, 200));
        assert_eq!(watermark(&conn).unwrap(), Some(190));
    }

    #[test]
    fn a_catch_up_joins_the_range_a_closed_live_segment_left() {
        let (_dir, mut conn) = open();
        let room = "270f6caf-0feb-4055-93f3-cdbeb567ff28";
        let segment = ingest::begin_live(&mut conn, 1, &[room.to_string()], 1_000).unwrap();
        ingest::close_segment(&mut conn, segment, 5_000).unwrap();
        commit(
            &mut conn,
            Commit {
                events: vec![],
                claims: vec![(room.to_string(), chunk_claim(4_000, 9_000))],
            },
            || None,
        )
        .unwrap();
        let ranges = room_ranges(&conn, room, Class::All).unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!((ranges[0].lo_at, ranges[0].hi_at), (1_000, 9_000));
        // A dead socket's catch-up leaves the watermark alone.
        assert_eq!(watermark(&conn).unwrap(), None);
    }
}
