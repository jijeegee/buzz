//! Coverage: the spans of time over which every event of a room is in the
//! store. Rules and shared cases: docs/message-sync.md#coverage,
//! test-fixtures/message-store-coverage.json.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};

/// The stream a room's own timeline is recorded under.
pub(crate) const ROOM_STREAM: &str = "room";

/// What a span promises. Raised when that changes (2: job rows, clock check,
/// scope pinning); spans an older build wrote are then dropped.
pub(crate) const COVERAGE_VERSION: i64 = 2;

/// Drops spans written under another [`COVERAGE_VERSION`] (writer, on open).
pub(crate) fn drop_stale(conn: &Connection) -> rusqlite::Result<()> {
    if is_current(conn)? {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM coverage;")?;
    conn.execute(
        "INSERT OR REPLACE INTO sync_state(key, value) VALUES ('coverage_version', ?1)",
        params![COVERAGE_VERSION.to_string()],
    )?;
    conn.execute_batch("COMMIT;")
}

/// Whether the file's spans were written under [`COVERAGE_VERSION`]; reads
/// trust no span otherwise.
pub(crate) fn is_current(conn: &Connection) -> rusqlite::Result<bool> {
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM sync_state WHERE key = 'coverage_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(version.as_deref() == Some(COVERAGE_VERSION.to_string().as_str()))
}

/// `segments` (sorted by lo) after the span `[from, to]` is known complete:
/// segments it overlaps or touches merge into it.
pub(crate) fn merge(segments: &[(i64, i64)], from: i64, to: i64) -> Vec<(i64, i64)> {
    if from > to {
        return segments.to_vec();
    }
    let (mut lo, mut hi) = (from, to);
    let mut merged = Vec::with_capacity(segments.len() + 1);
    for &(seg_lo, seg_hi) in segments {
        if seg_hi < lo || seg_lo > hi {
            merged.push((seg_lo, seg_hi));
        } else {
            lo = lo.min(seg_lo);
            hi = hi.max(seg_hi);
        }
    }
    merged.push((lo, hi));
    merged.sort_unstable();
    merged
}

fn segments(conn: &Connection, room: &str) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut statement = conn.prepare_cached(
        "SELECT lo_at, hi_at FROM coverage WHERE room = ?1 AND stream = ?2 ORDER BY lo_at",
    )?;
    let rows = statement.query_map(params![room, ROOM_STREAM], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    rows.collect()
}

/// Records that every event of `rooms` in `[from, to]` is stored.
pub(crate) fn extend(
    conn: &mut Connection,
    rooms: &[String],
    from: i64,
    to: i64,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for room in rooms {
        let next = merge(&segments(&tx, room)?, from, to);
        tx.execute(
            "DELETE FROM coverage WHERE room = ?1 AND stream = ?2",
            params![room, ROOM_STREAM],
        )?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO coverage(room, stream, lo_at, lo_id, hi_at, hi_id) VALUES (?1, ?2, ?3, '', ?4, '')",
        )?;
        for (lo, hi) in next {
            insert.execute(params![room, ROOM_STREAM, lo, hi])?;
        }
    }
    tx.commit()
}

/// Forgets every span: some received event may not have been written.
pub(crate) fn clear(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM coverage", []).map(|_| ())
}

/// The end of each room's latest span that began before `before`, for rooms
/// that have one. A span opened at `before` itself (the live REQ that is
/// about to be caught up) may already be written and must not hide the gap.
pub(crate) fn resume_points(
    conn: &Connection,
    rooms: &[String],
    before: i64,
) -> rusqlite::Result<HashMap<String, i64>> {
    let mut statement = conn.prepare_cached(
        "SELECT max(hi_at) FROM coverage WHERE room = ?1 AND stream = ?2 AND lo_at < ?3",
    )?;
    let mut points = HashMap::new();
    for room in rooms {
        let hi: Option<i64> = statement
            .query_row(params![room, ROOM_STREAM, before], |row| row.get(0))
            .optional()?
            .flatten();
        if let Some(hi) = hi {
            points.insert(room.clone(), hi);
        }
    }
    Ok(points)
}
