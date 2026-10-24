//! Write paths. Rust is the only writer: fetch commands and the socket tap
//! (wired in the write-path change) insert events; only fetches whose
//! request bounds are known claim coverage. The webview has no insert command.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

use super::{
    classify::{classify, Classified, StoredEvent, DELETE_KINDS},
    coverage::{apply_claim, Claim, Range, Upper, TIME_FLOOR_ID},
    schema::meta,
    SYNC_KINDS_VERSION,
};

/// Rough on-disk cost of one row beyond its JSON (columns and indexes).
const ROW_OVERHEAD_BYTES: i64 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    /// Proves top-level rows (window pages).
    Top,
    /// Proves every synced kind (repair, catch-up, live segments).
    All,
}

impl Class {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::All => "all",
        }
    }
}

fn db(context: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |error| format!("message store {context}: {error}")
}

/// Deleted messages keep only their structure: no text, no media tags.
fn blank(raw: &str) -> String {
    let Ok(mut event) = serde_json::from_str::<Value>(raw) else {
        return raw.to_string();
    };
    event["content"] = Value::String(String::new());
    if let Some(tags) = event.get_mut("tags").and_then(Value::as_array_mut) {
        tags.retain(|tag| {
            matches!(
                tag.get(0).and_then(Value::as_str),
                Some("h" | "e" | "broadcast")
            )
        });
    }
    event.to_string()
}

fn mark_deleted(tx: &Transaction<'_>, target: &str, channel_id: &str) -> Result<(), String> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT raw FROM events WHERE id=?1 AND channel_id=?2 AND deleted=0",
            params![target, channel_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(db("read delete target"))?;
    if let Some(raw) = raw {
        tx.execute(
            "UPDATE events SET deleted=1, raw=?2 WHERE id=?1",
            params![target, blank(&raw)],
        )
        .map_err(db("mark deleted"))?;
    }
    Ok(())
}

fn insert_event(
    tx: &Transaction<'_>,
    stored: &StoredEvent,
    raw: &str,
    received_at: i64,
) -> Result<bool, String> {
    let inserted = tx
        .execute(
            "INSERT OR IGNORE INTO events(id, channel_id, created_at, kind, pubkey, role, root_id, parent_id, raw, received_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                stored.id,
                stored.channel_id,
                stored.created_at,
                stored.kind,
                stored.pubkey,
                stored.role as i64,
                stored.root_id,
                stored.parent_id,
                raw,
                received_at
            ],
        )
        .map_err(db("insert event"))?
        == 1;
    if !inserted {
        return Ok(false);
    }
    for target in &stored.targets {
        tx.execute(
            "INSERT OR IGNORE INTO event_targets(target_id, event_id) VALUES(?1, ?2)",
            params![target, stored.id],
        )
        .map_err(db("insert target"))?;
    }
    if DELETE_KINDS.contains(&stored.kind) {
        for target in &stored.targets {
            mark_deleted(tx, target, &stored.channel_id)?;
        }
    } else {
        // A delete that arrived first still applies.
        let deleted: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM event_targets t JOIN events d ON d.id=t.event_id
                 WHERE t.target_id=?1 AND d.kind IN (5, 9005) AND d.channel_id=?2)",
                params![stored.id, stored.channel_id],
                |row| row.get(0),
            )
            .map_err(db("check earlier delete"))?;
        if deleted {
            mark_deleted(tx, &stored.id, &stored.channel_id)?;
        }
    }
    tx.execute(
        "INSERT INTO room_meta(channel_id, last_event_at, bytes_est) VALUES(?1, ?2, ?3)
         ON CONFLICT(channel_id) DO UPDATE SET
           last_event_at=MAX(COALESCE(last_event_at, 0), excluded.last_event_at),
           bytes_est=bytes_est + excluded.bytes_est",
        params![
            stored.channel_id,
            stored.created_at,
            raw.len() as i64 + ROW_OVERHEAD_BYTES
        ],
    )
    .map_err(db("update room meta"))?;
    Ok(true)
}

/// Stores relay events (insert-only; never a coverage claim). Returns how
/// many were new. `fallback_channel` is the room a fetch asked for.
#[cfg_attr(not(test), allow(dead_code))] // Wired by the write-path change.
pub(crate) fn insert_events(
    conn: &mut Connection,
    fallback_channel: Option<&str>,
    events: &[Value],
    received_at: i64,
) -> Result<usize, String> {
    let tx = conn.transaction().map_err(db("begin insert"))?;
    let mut inserted = 0;
    for event in events {
        match classify(event, fallback_channel) {
            Classified::Skip => {}
            Classified::Summary {
                root_id,
                channel_id,
                created_at,
            } => {
                tx.execute(
                    "INSERT INTO thread_summary(root_id, channel_id, created_at, payload)
                     VALUES(?1, ?2, ?3, ?4)
                     ON CONFLICT(root_id) DO UPDATE SET
                       channel_id=excluded.channel_id,
                       created_at=excluded.created_at,
                       payload=excluded.payload
                     WHERE excluded.created_at > thread_summary.created_at",
                    params![root_id, channel_id, created_at, event.to_string()],
                )
                .map_err(db("store thread summary"))?;
            }
            Classified::Event(stored) => {
                if insert_event(&tx, &stored, &event.to_string(), received_at)? {
                    inserted += 1;
                }
            }
        }
    }
    tx.commit().map_err(db("commit insert"))?;
    Ok(inserted)
}

/// Records the newest socket frame time (live segments close at it after a crash).
#[cfg_attr(not(test), allow(dead_code))] // Wired by the write-path change.
pub(crate) fn note_frame(conn: &Connection, at: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('last_frame_at', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value
         WHERE CAST(excluded.value AS INTEGER) > CAST(meta.value AS INTEGER)",
        [at.to_string()],
    )
    .map_err(db("note frame"))?;
    Ok(())
}

fn read_ranges(
    conn: &Connection,
    sql: &str,
    key: &str,
    class: Option<&str>,
) -> Result<Vec<Range>, String> {
    let mut statement = conn.prepare_cached(sql).map_err(db("prepare ranges"))?;
    let map = |row: &rusqlite::Row<'_>| {
        Ok(Range {
            lo_at: row.get(0)?,
            lo_id: row.get(1)?,
            lo_is_start: row.get(2)?,
            hi_at: row.get(3)?,
            kinds_version: row.get(4)?,
        })
    };
    let rows = match class {
        Some(class) => statement.query_map(params![key, class], map),
        None => statement.query_map(params![key], map),
    }
    .map_err(db("read ranges"))?;
    rows.collect::<Result<_, _>>().map_err(db("read range"))
}

/// A room's stored ranges of one class.
pub(crate) fn room_ranges(
    conn: &Connection,
    channel_id: &str,
    class: Class,
) -> Result<Vec<Range>, String> {
    read_ranges(
        conn,
        "SELECT lo_at, lo_id, lo_is_start, hi_at, kinds_version FROM coverage
         WHERE channel_id=?1 AND class=?2",
        channel_id,
        Some(class.as_str()),
    )
}

/// A thread's stored ranges.
pub(crate) fn thread_ranges(conn: &Connection, root_id: &str) -> Result<Vec<Range>, String> {
    read_ranges(
        conn,
        "SELECT lo_at, lo_id, lo_is_start, hi_at, kinds_version FROM thread_coverage WHERE root_id=?1",
        root_id,
        None,
    )
}

fn write_ranges(
    tx: &Transaction<'_>,
    insert: &str,
    key: &str,
    class: Option<&str>,
    ranges: &[Range],
) -> Result<(), String> {
    let mut statement = tx.prepare_cached(insert).map_err(db("prepare range"))?;
    for r in ranges {
        let result = match class {
            Some(class) => statement.execute(params![
                key,
                class,
                r.lo_at,
                r.lo_id,
                r.lo_is_start,
                r.hi_at,
                r.kinds_version
            ]),
            None => statement.execute(params![
                key,
                r.lo_at,
                r.lo_id,
                r.lo_is_start,
                r.hi_at,
                r.kinds_version
            ]),
        };
        result.map_err(db("write range"))?;
    }
    Ok(())
}

/// Adds what one completed room fetch proves.
pub(crate) fn claim_room(
    conn: &mut Connection,
    channel_id: &str,
    class: Class,
    claim: &Claim,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(db("begin claim"))?;
    let ranges = apply_claim(
        room_ranges(&tx, channel_id, class)?,
        claim,
        SYNC_KINDS_VERSION,
    );
    tx.execute(
        "DELETE FROM coverage WHERE channel_id=?1 AND class=?2",
        params![channel_id, class.as_str()],
    )
    .map_err(db("replace ranges"))?;
    write_ranges(
        &tx,
        "INSERT INTO coverage(channel_id, class, lo_at, lo_id, lo_is_start, hi_at, kinds_version)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        channel_id,
        Some(class.as_str()),
        &ranges,
    )?;
    tx.commit().map_err(db("commit claim"))
}

/// Adds what one completed thread fetch proves.
#[cfg_attr(not(test), allow(dead_code))] // Wired by the write-path change.
pub(crate) fn claim_thread(
    conn: &mut Connection,
    root_id: &str,
    claim: &Claim,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(db("begin thread claim"))?;
    let ranges = apply_claim(thread_ranges(&tx, root_id)?, claim, SYNC_KINDS_VERSION);
    tx.execute("DELETE FROM thread_coverage WHERE root_id=?1", [root_id])
        .map_err(db("replace thread ranges"))?;
    write_ranges(
        &tx,
        "INSERT INTO thread_coverage(root_id, lo_at, lo_id, lo_is_start, hi_at, kinds_version)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        root_id,
        None,
        &ranges,
    )?;
    tx.commit().map_err(db("commit thread claim"))
}

/// Registers a live subscription that reached EOSE for `channel_ids` since
/// `since`. Returns the segment id.
pub(crate) fn begin_live(
    conn: &mut Connection,
    ws_id: i64,
    channel_ids: &[String],
    since: i64,
) -> Result<i64, String> {
    let tx = conn.transaction().map_err(db("begin live"))?;
    tx.execute(
        "INSERT INTO live_segment(ws_id, started_at) VALUES(?1, ?2)",
        params![ws_id, since],
    )
    .map_err(db("insert live segment"))?;
    let segment_id = tx.last_insert_rowid();
    for channel_id in channel_ids {
        tx.execute(
            "INSERT OR IGNORE INTO live_segment_room(segment_id, channel_id) VALUES(?1, ?2)",
            params![segment_id, channel_id],
        )
        .map_err(db("insert live room"))?;
    }
    tx.commit().map_err(db("commit live"))?;
    Ok(segment_id)
}

/// Closes open segments (of `ws_id`, or all) at `closed_at`: each room gains
/// an `all` range `[since, closed_at]`.
pub(crate) fn close_live(
    conn: &mut Connection,
    ws_id: Option<i64>,
    closed_at: i64,
) -> Result<(), String> {
    let segments: Vec<(i64, i64)> = {
        let mut statement = conn
            .prepare(
                "SELECT id, started_at FROM live_segment
                 WHERE closed_at IS NULL AND (?1 IS NULL OR ws_id=?1)",
            )
            .map_err(db("prepare open segments"))?;
        let rows = statement
            .query_map([ws_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(db("read open segments"))?;
        rows.collect::<Result<_, _>>()
            .map_err(db("read open segment"))?
    };
    for (segment_id, since) in segments {
        let channel_ids: Vec<String> = {
            let mut statement = conn
                .prepare("SELECT channel_id FROM live_segment_room WHERE segment_id=?1")
                .map_err(db("prepare live rooms"))?;
            let rows = statement
                .query_map([segment_id], |row| row.get(0))
                .map_err(db("read live rooms"))?;
            rows.collect::<Result<_, _>>()
                .map_err(db("read live room"))?
        };
        let claim = Claim {
            lo: Some(super::coverage::Cursor {
                created_at: since,
                event_id: TIME_FLOOR_ID.to_string(),
            }),
            lo_is_start: false,
            hi: Upper::At(closed_at),
        };
        for channel_id in &channel_ids {
            claim_room(conn, channel_id, Class::All, &claim)?;
        }
        conn.execute(
            "UPDATE live_segment SET closed_at=?2 WHERE id=?1",
            params![segment_id, closed_at.max(since)],
        )
        .map_err(db("close live segment"))?;
    }
    Ok(())
}

/// Boot: segments left open by a crash close at the last frame written.
pub(crate) fn recover_open_segments(conn: &mut Connection) -> Result<(), String> {
    let last_frame_at = meta(conn, "last_frame_at")
        .map_err(db("read last frame"))?
        .and_then(|value| value.parse::<i64>().ok());
    match last_frame_at {
        Some(at) => close_live(conn, None, at),
        None => conn
            .execute(
                "UPDATE live_segment SET closed_at=started_at WHERE closed_at IS NULL",
                [],
            )
            .map(|_| ())
            .map_err(db("drop open segments")),
    }
}
