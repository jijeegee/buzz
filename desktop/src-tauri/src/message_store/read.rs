//! Read queries. A read never crosses a gap: it returns rows only from the
//! one contiguous covered run it starts in, and says when it stopped at the
//! run's lower end (`gap`) so the caller asks the server below that point.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use super::{
    coverage::{
        effective_top, normalize, run_containing, with_live, Cursor, Range, LIVE_HI, TIME_FLOOR_ID,
    },
    ingest::{room_ranges, thread_ranges, Class},
    SYNC_KINDS_VERSION, WINDOW_ROW_KINDS,
};

/// One page of a room's top-level timeline, newest first.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WindowPage {
    pub(crate) start_cursor: Option<Cursor>,
    pub(crate) rows: Vec<Value>,
    /// Reactions, edits and deletes targeting `rows`, oldest first.
    pub(crate) aux: Vec<Value>,
    /// Newest thread summary (39005) per row, by root id.
    pub(crate) summaries: Vec<Value>,
    pub(crate) next_cursor: Option<Cursor>,
    pub(crate) has_more: bool,
    /// Stopped at the lower end of local coverage, not at `limit` or the
    /// room start: the server is needed below `next_cursor`.
    pub(crate) gap: bool,
}

/// Rows around an anchor, newest first.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AroundPage {
    pub(crate) rows: Vec<Value>,
    pub(crate) aux: Vec<Value>,
    pub(crate) summaries: Vec<Value>,
    pub(crate) has_older: bool,
    pub(crate) has_newer: bool,
    pub(crate) gap_below: bool,
    pub(crate) gap_above: bool,
}

/// Thread replies, oldest first.
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadPage {
    pub(crate) rows: Vec<Value>,
    pub(crate) aux: Vec<Value>,
    pub(crate) summaries: Vec<Value>,
    pub(crate) next_cursor: Option<Cursor>,
    pub(crate) has_more: bool,
    /// Replies are proven complete up to this time (absent when unproven or live).
    pub(crate) covered_through: Option<i64>,
    /// Replies are proven complete up to now by an open live segment.
    pub(crate) live_covered: bool,
}

/// Thread replies newest first, within one covered run (like [`WindowPage`]).
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadHeadPage {
    pub(crate) rows: Vec<Value>,
    pub(crate) aux: Vec<Value>,
    pub(crate) summaries: Vec<Value>,
    pub(crate) next_cursor: Option<Cursor>,
    pub(crate) has_more: bool,
    /// Stopped at the lower end of the run before `limit` or the thread start.
    pub(crate) gap: bool,
    /// The run's replies are complete up to this time (absent when live or none).
    pub(crate) covered_through: Option<i64>,
    /// The run is continued by an open live segment of the root's room.
    pub(crate) live_covered: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RangeView {
    pub(crate) class: &'static str,
    pub(crate) lo_at: i64,
    pub(crate) lo_id: String,
    pub(crate) lo_is_start: bool,
    pub(crate) hi_at: i64,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoomCoverage {
    pub(crate) channel_id: String,
    /// `top` (including what `all` proves) then `all`, each oldest first.
    pub(crate) ranges: Vec<RangeView>,
    /// An open live segment continues the newest range: the head is current.
    pub(crate) live_covered: bool,
    /// Newest time everything in the room is proven through (`all` ranges).
    pub(crate) proven_through: Option<i64>,
    pub(crate) newest_local_at: Option<i64>,
    /// The newest covered run does not reach the room start.
    pub(crate) gap_below: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoomHead {
    pub(crate) channel_id: String,
    pub(crate) page: WindowPage,
}

fn db(context: &str) -> impl Fn(rusqlite::Error) -> String + '_ {
    move |error| format!("message store {context}: {error}")
}

fn kinds_sql() -> String {
    WINDOW_ROW_KINDS.map(|kind| kind.to_string()).join(",")
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

struct Row {
    event: Value,
    cursor: Cursor,
}

fn collect_rows(
    statement: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<Row>, String> {
    let rows = statement
        .query_map(params, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(db("query rows"))?;
    let mut out = Vec::new();
    for row in rows {
        let (raw, created_at, event_id) = row.map_err(db("read row"))?;
        if let Ok(event) = serde_json::from_str(&raw) {
            out.push(Row {
                event,
                cursor: Cursor {
                    created_at,
                    event_id,
                },
            });
        }
    }
    Ok(out)
}

fn live_since(conn: &Connection, channel_id: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT MIN(s.started_at) FROM live_segment s JOIN live_segment_room r ON r.segment_id=s.id
         WHERE s.closed_at IS NULL AND r.channel_id=?1",
        [channel_id],
        |row| row.get(0),
    )
    .map_err(db("read live segment"))
}

/// The room's `top` runs, the open live segment included.
fn top_runs(conn: &Connection, channel_id: &str) -> Result<(Vec<Range>, Vec<Range>), String> {
    let top = room_ranges(conn, channel_id, Class::Top)?;
    let all = room_ranges(conn, channel_id, Class::All)?;
    let effective = effective_top(&top, &all, SYNC_KINDS_VERSION);
    let runs = with_live(
        &effective,
        live_since(conn, channel_id)?,
        SYNC_KINDS_VERSION,
    );
    Ok((effective, runs))
}

/// Timeline rows strictly older than `below` (or from the top) inside `run`.
fn rows_below(
    conn: &Connection,
    channel_id: &str,
    run: &Range,
    below: Option<&Cursor>,
    limit: usize,
) -> Result<Vec<Row>, String> {
    let sql = format!(
        "SELECT raw, created_at, id FROM events
         WHERE channel_id=?1 AND role IN (0, 2) AND deleted=0 AND kind IN ({})
           AND created_at <= ?2
           AND (?3 IS NULL OR created_at < ?3 OR (created_at = ?3 AND id > ?4))
           AND (?5 OR created_at > ?6 OR (created_at = ?6 AND id <= ?7))
         ORDER BY created_at DESC, id ASC LIMIT ?8",
        kinds_sql()
    );
    let mut statement = conn.prepare_cached(&sql).map_err(db("prepare rows"))?;
    collect_rows(
        &mut statement,
        params![
            channel_id,
            run.hi_at,
            below.map(|cursor| cursor.created_at),
            below.map(|cursor| cursor.event_id.as_str()),
            run.lo_is_start,
            run.lo_at,
            run.lo_id,
            limit as i64
        ],
    )
}

/// Timeline rows at or newer than `from`, up to the run's top, oldest first.
fn rows_from(
    conn: &Connection,
    channel_id: &str,
    run: &Range,
    from: &Cursor,
    limit: usize,
) -> Result<Vec<Row>, String> {
    let sql = format!(
        "SELECT raw, created_at, id FROM events
         WHERE channel_id=?1 AND role IN (0, 2) AND deleted=0 AND kind IN ({})
           AND created_at <= ?2 AND (created_at > ?3 OR (created_at = ?3 AND id <= ?4))
         ORDER BY created_at ASC, id DESC LIMIT ?5",
        kinds_sql()
    );
    let mut statement = conn.prepare_cached(&sql).map_err(db("prepare rows"))?;
    collect_rows(
        &mut statement,
        params![
            channel_id,
            run.hi_at,
            from.created_at,
            from.event_id,
            limit as i64
        ],
    )
}

fn ids(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|row| row.cursor.event_id.clone()).collect()
}

fn aux_for(conn: &Connection, ids: &[String]) -> Result<Vec<Value>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT DISTINCT e.raw, e.created_at, e.id FROM event_targets t JOIN events e ON e.id=t.event_id
         WHERE t.target_id IN ({}) AND e.role=3 AND e.deleted=0
         ORDER BY e.created_at ASC, e.id ASC",
        placeholders(ids.len())
    );
    let mut statement = conn.prepare(&sql).map_err(db("prepare aux"))?;
    Ok(collect_rows(&mut statement, params_from_iter(ids))?
        .into_iter()
        .map(|row| row.event)
        .collect())
}

fn summaries_for(conn: &Connection, ids: &[String]) -> Result<Vec<Value>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT payload FROM thread_summary WHERE root_id IN ({}) ORDER BY root_id ASC",
        placeholders(ids.len())
    );
    let mut statement = conn.prepare(&sql).map_err(db("prepare summaries"))?;
    let rows = statement
        .query_map(params_from_iter(ids), |row| row.get::<_, String>(0))
        .map_err(db("query summaries"))?;
    let mut out = Vec::new();
    for row in rows {
        if let Ok(event) = serde_json::from_str(&row.map_err(db("read summary"))?) {
            out.push(event);
        }
    }
    Ok(out)
}

/// One page of a room below `before` (or the head), within one covered run.
pub(crate) fn window(
    conn: &Connection,
    channel_id: &str,
    before: Option<&Cursor>,
    limit: usize,
) -> Result<WindowPage, String> {
    let (_, runs) = top_runs(conn, channel_id)?;
    let run = match before {
        // Rows strictly older than the cursor continue the run holding it.
        Some(cursor) => run_containing(&runs, cursor.created_at, &cursor.event_id),
        None => runs.last(),
    };
    let mut rows = match run {
        Some(run) => rows_below(conn, channel_id, run, before, limit + 1)?,
        None => Vec::new(),
    };
    let full = rows.len() > limit;
    rows.truncate(limit);
    let last = rows
        .last()
        .map(|row| row.cursor.clone())
        .or_else(|| before.cloned());
    let reached_start = run.is_some_and(|run| run.lo_is_start) && !full;
    let gap = !full && !reached_start;
    let next_cursor = if reached_start { None } else { last };
    let row_ids = ids(&rows);
    Ok(WindowPage {
        start_cursor: before.cloned(),
        aux: aux_for(conn, &row_ids)?,
        summaries: summaries_for(conn, &row_ids)?,
        rows: rows.into_iter().map(|row| row.event).collect(),
        has_more: next_cursor.is_some(),
        next_cursor,
        gap,
    })
}

/// Rows around an anchor: `before` strictly older, `after` at or newer.
pub(crate) fn around(
    conn: &Connection,
    channel_id: &str,
    anchor: &Cursor,
    before: usize,
    after: usize,
) -> Result<AroundPage, String> {
    let (_, runs) = top_runs(conn, channel_id)?;
    let Some(run) = run_containing(&runs, anchor.created_at, &anchor.event_id) else {
        return Ok(AroundPage {
            rows: Vec::new(),
            aux: Vec::new(),
            summaries: Vec::new(),
            has_older: true,
            has_newer: true,
            gap_below: true,
            gap_above: true,
        });
    };
    let mut newer = rows_from(conn, channel_id, run, anchor, after + 1)?;
    let mut older = rows_below(conn, channel_id, run, Some(anchor), before + 1)?;
    let newer_full = newer.len() > after;
    let older_full = older.len() > before;
    newer.truncate(after);
    older.truncate(before);
    newer.reverse();
    newer.append(&mut older);
    let gap_below = !older_full && !run.lo_is_start;
    let gap_above = !newer_full && run.hi_at != LIVE_HI;
    let row_ids = ids(&newer);
    Ok(AroundPage {
        aux: aux_for(conn, &row_ids)?,
        summaries: summaries_for(conn, &row_ids)?,
        rows: newer.into_iter().map(|row| row.event).collect(),
        has_older: older_full || gap_below,
        has_newer: newer_full || gap_above,
        gap_below,
        gap_above,
    })
}

/// Replies of `root_id` after `cursor`, oldest first.
pub(crate) fn thread(
    conn: &Connection,
    root_id: &str,
    cursor: Option<&Cursor>,
    limit: usize,
) -> Result<ThreadPage, String> {
    let mut statement = conn
        .prepare_cached(
            "SELECT raw, created_at, id FROM events
             WHERE root_id=?1 AND role IN (1, 2) AND deleted=0
               AND (?2 IS NULL OR created_at > ?2 OR (created_at = ?2 AND id > ?3))
             ORDER BY created_at ASC, id ASC LIMIT ?4",
        )
        .map_err(db("prepare thread"))?;
    let mut rows = collect_rows(
        &mut statement,
        params![
            root_id,
            cursor.map(|cursor| cursor.created_at),
            cursor.map(|cursor| cursor.event_id.as_str()),
            (limit + 1) as i64
        ],
    )?;
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    // Proven through: a thread range from the thread start, or a room `all`
    // run holding the root (replies are newer than their root).
    let mut through = normalize(thread_ranges(conn, root_id)?, SYNC_KINDS_VERSION)
        .iter()
        .filter(|range| range.lo_is_start)
        .map(|range| range.hi_at)
        .max();
    if let Some((channel_id, created_at)) = thread_root(conn, root_id)? {
        let all = normalize(
            room_ranges(conn, &channel_id, Class::All)?,
            SYNC_KINDS_VERSION,
        );
        let runs = with_live(&all, live_since(conn, &channel_id)?, SYNC_KINDS_VERSION);
        if let Some(run) = run_containing(&runs, created_at, root_id) {
            through = through.max(Some(run.hi_at));
        }
    }
    let row_ids = ids(&rows);
    Ok(ThreadPage {
        next_cursor: if has_more {
            rows.last().map(|row| row.cursor.clone())
        } else {
            None
        },
        aux: aux_for(conn, &row_ids)?,
        summaries: summaries_for(conn, &row_ids)?,
        rows: rows.into_iter().map(|row| row.event).collect(),
        has_more,
        covered_through: through.filter(|at| *at != LIVE_HI),
        live_covered: through == Some(LIVE_HI),
    })
}

fn thread_root(conn: &Connection, root_id: &str) -> Result<Option<(String, i64)>, String> {
    conn.query_row(
        "SELECT channel_id, created_at FROM events WHERE id=?1",
        [root_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(db("read thread root"))
}

/// A thread's covered runs: its own claims, plus its room's `all` runs (live
/// included) at or above the root. The run holding the root counts from the
/// thread start, since replies are newer than their root.
fn thread_runs(conn: &Connection, root_id: &str) -> Result<Vec<Range>, String> {
    let mut ranges = normalize(thread_ranges(conn, root_id)?, SYNC_KINDS_VERSION);
    if let Some((channel_id, root_at)) = thread_root(conn, root_id)? {
        let all = normalize(
            room_ranges(conn, &channel_id, Class::All)?,
            SYNC_KINDS_VERSION,
        );
        for run in with_live(&all, live_since(conn, &channel_id)?, SYNC_KINDS_VERSION) {
            if run.contains(root_at, root_id) {
                ranges.push(Range {
                    lo_at: 0,
                    lo_id: String::new(),
                    lo_is_start: true,
                    ..run
                });
            } else if run.hi_at >= root_at {
                ranges.push(run);
            }
        }
    }
    Ok(normalize(ranges, SYNC_KINDS_VERSION))
}

/// Replies of `root_id` strictly older than `before` (or from the newest),
/// newest first, within one covered run: the thread's [`window`].
pub(crate) fn thread_head(
    conn: &Connection,
    root_id: &str,
    before: Option<&Cursor>,
    limit: usize,
) -> Result<ThreadHeadPage, String> {
    let runs = thread_runs(conn, root_id)?;
    let run = match before {
        Some(cursor) => run_containing(&runs, cursor.created_at, &cursor.event_id),
        None => runs.last(),
    };
    let mut rows = match run {
        Some(run) => {
            let mut statement = conn
                .prepare_cached(
                    "SELECT raw, created_at, id FROM events
                     WHERE root_id=?1 AND role IN (1, 2) AND deleted=0 AND created_at <= ?2
                       AND (?3 IS NULL OR created_at < ?3 OR (created_at = ?3 AND id > ?4))
                       AND (?5 OR created_at > ?6 OR (created_at = ?6 AND id <= ?7))
                     ORDER BY created_at DESC, id ASC LIMIT ?8",
                )
                .map_err(db("prepare thread head"))?;
            collect_rows(
                &mut statement,
                params![
                    root_id,
                    run.hi_at,
                    before.map(|cursor| cursor.created_at),
                    before.map(|cursor| cursor.event_id.as_str()),
                    run.lo_is_start,
                    run.lo_at,
                    run.lo_id,
                    (limit + 1) as i64
                ],
            )?
        }
        None => Vec::new(),
    };
    let full = rows.len() > limit;
    rows.truncate(limit);
    let last = rows
        .last()
        .map(|row| row.cursor.clone())
        .or_else(|| before.cloned());
    let reached_start = run.is_some_and(|run| run.lo_is_start) && !full;
    let next_cursor = if reached_start { None } else { last };
    let hi_at = run.map(|run| run.hi_at);
    let row_ids = ids(&rows);
    Ok(ThreadHeadPage {
        aux: aux_for(conn, &row_ids)?,
        summaries: summaries_for(conn, &row_ids)?,
        rows: rows.into_iter().map(|row| row.event).collect(),
        has_more: next_cursor.is_some(),
        next_cursor,
        gap: !full && !reached_start,
        covered_through: hi_at.filter(|at| *at != LIVE_HI),
        live_covered: hi_at == Some(LIVE_HI),
    })
}

fn view<'a>(class: &'static str, ranges: &'a [Range]) -> impl Iterator<Item = RangeView> + 'a {
    ranges.iter().map(move |range| RangeView {
        class,
        lo_at: range.lo_at,
        lo_id: range.lo_id.clone(),
        lo_is_start: range.lo_is_start,
        hi_at: range.hi_at,
    })
}

/// What is proven locally for one room.
pub(crate) fn coverage(conn: &Connection, channel_id: &str) -> Result<RoomCoverage, String> {
    let (effective, runs) = top_runs(conn, channel_id)?;
    let all = normalize(
        room_ranges(conn, channel_id, Class::All)?,
        SYNC_KINDS_VERSION,
    );
    let live = live_since(conn, channel_id)?;
    let newest_local_at: Option<i64> = conn
        .query_row(
            "SELECT MAX(created_at) FROM events WHERE channel_id=?1 AND role IN (0, 2)",
            [channel_id],
            |row| row.get(0),
        )
        .map_err(db("read newest event"))?;
    Ok(RoomCoverage {
        channel_id: channel_id.to_string(),
        ranges: view("top", &effective).chain(view("all", &all)).collect(),
        live_covered: live
            .is_some_and(|since| effective.last().is_some_and(|range| range.hi_at >= since)),
        proven_through: all.iter().map(|range| range.hi_at).max(),
        newest_local_at,
        gap_below: runs.last().is_none_or(|run| !run.lo_is_start),
    })
}

/// Head pages of the most recently opened (then most recently active) rooms.
pub(crate) fn heads(conn: &Connection, rooms: usize, rows: usize) -> Result<Vec<RoomHead>, String> {
    let channel_ids: Vec<String> = {
        let mut statement = conn
            .prepare_cached(
                "SELECT channel_id FROM room_meta
                 ORDER BY COALESCE(last_opened_at, 0) DESC, COALESCE(last_event_at, 0) DESC, channel_id ASC
                 LIMIT ?1",
            )
            .map_err(db("prepare heads"))?;
        let ids = statement
            .query_map([rooms as i64], |row| row.get(0))
            .map_err(db("query heads"))?;
        ids.collect::<Result<_, _>>().map_err(db("read head"))?
    };
    let mut heads = Vec::new();
    for channel_id in channel_ids {
        let page = window(conn, &channel_id, None, rows)?;
        if !page.rows.is_empty() {
            heads.push(RoomHead { channel_id, page });
        }
    }
    Ok(heads)
}

/// Anchor cursor for a time without an event id: every event of that second
/// counts as at-or-after it.
pub(crate) fn anchor(created_at: i64, event_id: Option<String>) -> Cursor {
    Cursor {
        created_at,
        event_id: event_id.unwrap_or_else(|| TIME_FLOOR_ID.to_string()),
    }
}
