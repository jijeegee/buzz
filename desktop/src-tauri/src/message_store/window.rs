//! Store-first windows checked against the relay
//! (docs/message-sync.md#window-check): the stored window a check compares
//! (oldest row id and row count), whether the store holds that window at
//! all, and replacing a window's stored rows with the page the relay
//! answered.

use std::collections::HashSet;

use rusqlite::{params, Connection, Transaction};
use serde_json::Value;

use super::head::{thread_head_stream, HEAD_STREAM};
use super::read::{self, parent_of, parse};
use super::read_thread;

/// The stream the relay pages of a room's windows are recorded under;
/// `page:<root>` for a thread's.
pub(crate) const PAGE_STREAM: &str = "page";

fn page_stream(root: Option<&str>) -> String {
    root.map_or_else(
        || PAGE_STREAM.to_owned(),
        |root| format!("{PAGE_STREAM}:{root}"),
    )
}

/// `(created_at, id)` of a row.
pub(crate) type RowKey = (i64, String);

/// Whether `a` comes before `b` in relay order (`created_at DESC, id ASC`).
fn precedes(a: (i64, &str), b: (i64, &str)) -> bool {
    a.0 > b.0 || (a.0 == b.0 && a.1 < b.1)
}

fn key(row: &RowKey) -> (i64, &str) {
    (row.0, row.1.as_str())
}

/// A stretch of a window's rows, in relay order, that relay pages answered:
/// from `hi` (`None`: the newest end) down to `lo` (`None`: the start of
/// the history), both inclusive.
#[derive(Debug, Clone, PartialEq)]
struct Segment {
    hi: Option<RowKey>,
    lo: Option<RowKey>,
}

impl Segment {
    fn holds(&self, row: (i64, &str)) -> bool {
        self.hi.as_ref().is_none_or(|hi| !precedes(row, key(hi)))
            && self.lo.as_ref().is_none_or(|lo| !precedes(key(lo), row))
    }

    /// Overlapping or touching segments merge.
    fn meets(&self, other: &Segment) -> bool {
        let apart = |a: &Segment, b: &Segment| match (&a.lo, &b.hi) {
            (Some(lo), Some(hi)) => precedes(key(lo), key(hi)),
            _ => false,
        };
        !apart(self, other) && !apart(other, self)
    }

    fn join(&self, other: &Segment) -> Segment {
        let hi = match (&self.hi, &other.hi) {
            (Some(a), Some(b)) => Some(if precedes(key(a), key(b)) { a } else { b }.clone()),
            _ => None,
        };
        let lo = match (&self.lo, &other.lo) {
            (Some(a), Some(b)) => Some(if precedes(key(a), key(b)) { b } else { a }.clone()),
            _ => None,
        };
        Segment { hi, lo }
    }
}

fn segments(conn: &Connection, room: &str, stream: &str) -> rusqlite::Result<Vec<Segment>> {
    let mut statement = conn.prepare_cached(
        "SELECT hi_at, hi_id, lo_at, lo_id FROM coverage WHERE room = ?1 AND stream = ?2",
    )?;
    let rows = statement.query_map(params![room, stream], |row| {
        let end = |at: i64, id: String| (!id.is_empty()).then_some((at, id));
        Ok(Segment {
            hi: end(row.get(0)?, row.get(1)?),
            lo: end(row.get(2)?, row.get(3)?),
        })
    })?;
    rows.collect()
}

/// Adds `segment` to `stream`, merged with the segments it meets.
fn record_segment(
    tx: &Transaction,
    room: &str,
    stream: &str,
    segment: Segment,
) -> rusqlite::Result<()> {
    let mut merged = segment;
    let mut kept = Vec::new();
    for old in segments(tx, room, stream)? {
        if old.meets(&merged) {
            merged = merged.join(&old);
        } else {
            kept.push(old);
        }
    }
    kept.push(merged);
    tx.execute(
        "DELETE FROM coverage WHERE room = ?1 AND stream = ?2",
        params![room, stream],
    )?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO coverage(room, stream, lo_at, lo_id, hi_at, hi_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for segment in kept {
        let (lo_at, lo_id) = segment.lo.unwrap_or((0, String::new()));
        let (hi_at, hi_id) = segment.hi.unwrap_or((0, String::new()));
        insert.execute(params![room, stream, lo_at, lo_id, hi_at, hi_id])?;
    }
    Ok(())
}

/// Drops the head and page spans of `rooms`: a catch-up left a gap the
/// store cannot see, so their windows are fetched again
/// (docs/message-sync.md#window-check).
pub(crate) fn forget_windows(conn: &mut Connection, rooms: &[String]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for room in rooms {
        super::head::forget_heads(&tx, room)?;
    }
    tx.commit()
}

/// A window as the store holds it, to compare with the relay's check.
#[derive(Debug, Default)]
pub(crate) struct StoredWindow {
    /// A room: the channel-window response (rows, overlays, summaries,
    /// bounds); a thread: its replies, newest first.
    pub events: Vec<Value>,
    pub count: usize,
    /// The window's oldest row.
    pub oldest: Option<RowKey>,
    /// The window is full (`count` = limit).
    pub has_more: bool,
    /// Relay pages (or a head span) answered this stretch of the window, so
    /// a check can tell whether it still matches; otherwise it is fetched.
    pub fully_stored: bool,
}

/// `room`'s window (`root`: that thread's) of `limit` rows of `kinds` below
/// `before` (`None`: the newest), by the same rules as the relay's page:
/// top-level rows and direct broadcast replies (a thread: its replies at any
/// depth, the root left out), deleted rows left out.
pub(crate) fn stored_window(
    conn: &Connection,
    room: &str,
    root: Option<&str>,
    kinds: &[u64],
    limit: usize,
    before: Option<(i64, &str)>,
) -> rusqlite::Result<StoredWindow> {
    let mut window = StoredWindow::default();
    match root {
        None => {
            let mut head = read::room_window(conn, room, kinds, limit, before)?;
            window.count = head.rows.len();
            window.oldest = head.next_cursor.clone();
            window.has_more = window.count == limit;
            if !window.has_more {
                head.next_cursor = None;
            }
            window.events = head.into_window_response(room, before);
        }
        Some(root) => {
            let (head, oldest) =
                read_thread::thread_window(conn, room, root, kinds, limit, before)?;
            window.count = head.rows.len();
            window.oldest = oldest;
            window.has_more = head.has_more;
            // A thread page carries its replies' overlays (`include_aux`).
            let ids: Vec<String> = head
                .rows
                .iter()
                .filter_map(|row| row.get("id").and_then(Value::as_str).map(str::to_owned))
                .collect();
            let aux = read::overlays_of(conn, &ids)?;
            window.events = head.rows;
            window.events.extend(aux);
        }
    }
    window.fully_stored =
        reached(conn, room, root, before, &window, limit)? && super::coverage::is_current(conn)?;
    Ok(window)
}

/// Whether a page span holds `window` (its oldest row and, below a cursor,
/// the cursor; a short window only down to the start of the history), or,
/// for a newest window, a head span reaches it.
fn reached(
    conn: &Connection,
    room: &str,
    root: Option<&str>,
    before: Option<(i64, &str)>,
    window: &StoredWindow,
    limit: usize,
) -> rusqlite::Result<bool> {
    let Some(oldest) = window.oldest.as_ref() else {
        return Ok(false);
    };
    let full = window.count == limit;
    for segment in segments(conn, room, &page_stream(root))? {
        if segment.holds(key(oldest))
            && before.is_none_or(|cursor| segment.holds(cursor))
            && (full || segment.lo.is_none())
        {
            return Ok(true);
        }
    }
    if before.is_some() {
        return Ok(false);
    }
    let stream = root.map_or_else(|| HEAD_STREAM.to_owned(), thread_head_stream);
    let mut statement =
        conn.prepare_cached("SELECT lo_at, lo_id FROM coverage WHERE room = ?1 AND stream = ?2")?;
    let heads: Vec<(i64, String)> = statement
        .query_map(params![room, stream], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    // A head holds every row from its oldest one on in relay order.
    Ok(heads.iter().any(|(lo_at, lo_id)| {
        lo_id.is_empty() || (full && !precedes((*lo_at, lo_id.as_str()), key(oldest)))
    }))
}

/// A page the relay answered for a window, to replace its stored rows.
#[derive(Debug, Clone)]
pub(crate) struct WindowReplace {
    pub room: String,
    /// `None`: the room's window; else this thread root's.
    pub root: Option<String>,
    pub kinds: Vec<u64>,
    /// The page's cursor (exclusive); `None`: the newest page.
    pub before: Option<RowKey>,
    /// The page's oldest row; `None` when it had no more (down to the start).
    pub oldest: Option<RowKey>,
    /// Every event id the page held.
    pub ids: HashSet<String>,
}

/// Replaces `page`'s stretch of the window in one transaction: stored rows
/// of the window from below its cursor down to its oldest row (to the start
/// when it had no more) that the page lacks were deleted on the relay or are
/// not visible to the reader, and are dropped, unless written since the
/// request went out (`fresh`). The stretch is then recorded as a page span.
pub(crate) fn replace(
    conn: &mut Connection,
    page: &WindowReplace,
    fresh: &HashSet<String>,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let kinds = page
        .kinds
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let range = "(?3 IS NULL OR created_at < ?3 OR (created_at = ?3 AND id > ?4)) \
                 AND (?5 IS NULL OR created_at > ?5 OR (created_at = ?5 AND id <= ?6))";
    let sql = match &page.root {
        None => format!(
            "SELECT id, root, raw FROM events WHERE room = ?1 AND kind IN ({kinds}) \
             AND (root IS NULL OR broadcast = 1) AND {range}"
        ),
        Some(_) => format!(
            "SELECT id, NULL, NULL FROM events WHERE room = ?1 AND kind IN ({kinds}) \
             AND root = ?2 AND {range}"
        ),
    };
    let (before_at, before_id) = page
        .before
        .as_ref()
        .map_or((None, ""), |(at, id)| (Some(*at), id.as_str()));
    let (oldest_at, oldest_id) = page
        .oldest
        .as_ref()
        .map_or((None, ""), |(at, id)| (Some(*at), id.as_str()));
    let mut doomed = Vec::new();
    {
        let mut statement = tx.prepare(&sql)?;
        let mut found = statement.query(params![
            page.room,
            page.root.as_deref().unwrap_or_default(),
            before_at,
            before_id,
            oldest_at,
            oldest_id
        ])?;
        while let Some(row) = found.next()? {
            let id: String = row.get(0)?;
            if page.ids.contains(&id) || fresh.contains(&id) {
                continue;
            }
            // A broadcast reply is a window row only as a direct reply.
            if let Some(root) = row.get::<_, Option<String>>(1)? {
                let direct = row
                    .get::<_, Option<String>>(2)?
                    .and_then(parse)
                    .is_some_and(|event| parent_of(&event) == Some(root.as_str()));
                if !direct {
                    continue;
                }
            }
            doomed.push(id);
        }
    }
    for id in &doomed {
        tx.execute("DELETE FROM events WHERE id = ?1", params![id])?;
        tx.execute("DELETE FROM targets WHERE id = ?1", params![id])?;
        tx.execute("DELETE FROM summaries WHERE root = ?1", params![id])?;
    }
    let segment = Segment {
        hi: page.before.clone(),
        lo: page.oldest.clone(),
    };
    record_segment(&tx, &page.room, &page_stream(page.root.as_deref()), segment)?;
    tx.commit()
}
