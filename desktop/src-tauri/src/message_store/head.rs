//! Head coverage: a newest window the relay answered while a live REQ
//! carrying its room was receiving holds every row of that window from its
//! oldest row on, and the live REQ has every row since. Rules:
//! docs/message-sync.md#head-spans; shared cases:
//! test-fixtures/message-store-head-coverage.json.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};

use super::coverage::ROOM_STREAM;
use super::read::{parent_of, parse};

/// The stream a room window's head span is recorded under.
pub(crate) const HEAD_STREAM: &str = "head";

/// The stream thread `root`'s head span is recorded under.
pub(crate) fn thread_head_stream(root: &str) -> String {
    format!("{HEAD_STREAM}:{root}")
}

/// A newest window the relay answered under a live token.
#[derive(Debug, Clone)]
pub(crate) struct HeadSpan {
    pub room: String,
    /// `None`: the room's window; else the window of this thread root.
    pub root: Option<String>,
    /// The window's row kinds.
    pub kinds: Vec<u64>,
    /// When the live REQ carrying the room reached EOSE (the token's L).
    pub live_at: i64,
    /// `(created_at, id)` of the answer's oldest row; `None` when the
    /// answer had no more (the whole history).
    pub oldest: Option<(i64, String)>,
    /// Every event id the answer held.
    pub ids: HashSet<String>,
}

impl HeadSpan {
    fn stream(&self) -> String {
        self.root
            .as_deref()
            .map_or_else(|| HEAD_STREAM.to_owned(), thread_head_stream)
    }
}

/// Window requests marked as they went out and not yet answered, at most.
const MAX_MARKS: usize = 64;
/// Rows a mark remembers; beyond this its answer records nothing.
const MAX_MARK_ROWS: usize = 4096;
/// A mark not answered within this is dropped: its answer records nothing.
const MARK_TTL: Duration = Duration::from_secs(120);

struct Mark {
    rooms: HashSet<String>,
    written: HashSet<String>,
    overflow: bool,
    opened: Instant,
}

/// What a closed mark vouches for: its rooms, and the ids of their rows
/// written since it.
pub(crate) struct Closed {
    pub rooms: HashSet<String>,
    pub fresh: HashSet<String>,
}

/// Newest-window requests in flight, by the sequence number of the mark
/// queued as each went out, with the ids of their rooms' rows written since:
/// those may have reached the relay after its answer was read, so the answer
/// lacking them proves nothing.
#[derive(Default)]
pub(crate) struct Marks {
    open: HashMap<u64, Mark>,
}

impl Marks {
    pub(crate) fn open(&mut self, at: u64, rooms: Vec<String>) {
        self.open.retain(|_, mark| mark.opened.elapsed() < MARK_TTL);
        if self.open.len() >= MAX_MARKS {
            if let Some(&oldest) = self.open.keys().min() {
                self.open.remove(&oldest);
            }
        }
        let mark = Mark {
            rooms: rooms.into_iter().collect(),
            written: HashSet::new(),
            overflow: false,
            opened: Instant::now(),
        };
        self.open.insert(at, mark);
    }

    /// Row `id` of `room` is being written.
    pub(crate) fn note(&mut self, room: &str, id: &str) {
        for mark in self.open.values_mut() {
            if !mark.rooms.contains(room) {
                continue;
            }
            if mark.written.len() >= MAX_MARK_ROWS {
                mark.overflow = true;
            } else {
                mark.written.insert(id.to_owned());
            }
        }
    }

    /// Closes mark `at`, or `None` when it is unknown (dropped, or a plain
    /// capture), expired or overflowed.
    pub(crate) fn close(&mut self, at: u64) -> Option<Closed> {
        let mark = self.open.remove(&at)?;
        (!mark.overflow && mark.opened.elapsed() < MARK_TTL).then_some(Closed {
            rooms: mark.rooms,
            fresh: mark.written,
        })
    }

    #[cfg(test)]
    pub(crate) fn age_for_test(&mut self, by: Duration) {
        for mark in self.open.values_mut() {
            mark.opened = mark.opened.checked_sub(by).unwrap_or(mark.opened);
        }
    }
}

/// Drops every head span of `room` (`head` and `head:<root>`): after a
/// history change the relay may show rows a head said were not there.
pub(crate) fn forget_heads(conn: &Connection, room: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM coverage WHERE room = ?1 AND (stream = ?2 OR stream LIKE ?3)",
        params![room, HEAD_STREAM, format!("{HEAD_STREAM}:%")],
    )
    .map(|_| ())
}

/// Records `head` in one transaction. The stored rows of its window from
/// its oldest row on (every one, for a whole history) up to `live_at` that
/// the answer lacks were deleted on the relay and are dropped, unless
/// written since the request went out (`fresh`). Then the stream's span
/// becomes `[oldest, live_at]` (`lo_at` 0 and an empty `lo_id` for a whole
/// history), unless the span already there reaches further back and is
/// joined to `live_at` by a room span: that one keeps its `lo`.
pub(crate) fn record(
    conn: &mut Connection,
    head: &HeadSpan,
    fresh: &HashSet<String>,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let kinds = head
        .kinds
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let (lo_at, lo_id) = head.oldest.clone().unwrap_or((i64::MIN, String::new()));
    let newer = "(created_at > ?2 OR (created_at = ?2 AND id <= ?3)) AND created_at <= ?5";
    let mut doomed = Vec::new();
    {
        let (sql, scope) = match &head.root {
            None => (
                format!(
                    "SELECT id, root, raw FROM events WHERE room = ?1 AND kind IN ({kinds}) \
                     AND (root IS NULL OR broadcast = 1) AND {newer}"
                ),
                String::new(),
            ),
            Some(root) => (
                format!(
                    "SELECT id, NULL, NULL FROM events WHERE room = ?1 AND kind IN ({kinds}) \
                     AND root = ?4 AND {newer}"
                ),
                root.clone(),
            ),
        };
        let mut statement = tx.prepare(&sql)?;
        let mut found = statement.query(params![head.room, lo_at, lo_id, scope, head.live_at])?;
        while let Some(row) = found.next()? {
            let id: String = row.get(0)?;
            if head.ids.contains(&id) || fresh.contains(&id) {
                continue;
            }
            // A broadcast reply is a window row only as a direct reply.
            let root: Option<String> = row.get(1)?;
            if let Some(root) = root {
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
    let stream = head.stream();
    let mut span = (
        head.oldest.as_ref().map_or(0, |(at, _)| *at),
        head.oldest
            .as_ref()
            .map_or(String::new(), |(_, id)| id.clone()),
        head.live_at,
    );
    if let Some(wider) = wider_span(&tx, head, &stream)? {
        span = wider;
    }
    tx.execute(
        "DELETE FROM coverage WHERE room = ?1 AND stream = ?2",
        params![head.room, stream],
    )?;
    tx.execute(
        "INSERT INTO coverage(room, stream, lo_at, lo_id, hi_at, hi_id) VALUES (?1, ?2, ?3, ?4, ?5, '')",
        params![head.room, stream, span.0, span.1, span.2],
    )?;
    tx.commit()
}

/// The span already recorded for `stream`, widened to `head`'s L, when it
/// reaches further back than `head` (or is the whole history while `head`
/// is not) and one room span joins its end to `head`'s L.
fn wider_span(
    conn: &Connection,
    head: &HeadSpan,
    stream: &str,
) -> rusqlite::Result<Option<(i64, String, i64)>> {
    let old: Option<(i64, String, i64)> = conn
        .query_row(
            "SELECT lo_at, lo_id, hi_at FROM coverage WHERE room = ?1 AND stream = ?2",
            params![head.room, stream],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((lo_at, lo_id, hi_at)) = old else {
        return Ok(None);
    };
    // Relay order is `created_at DESC, id ASC`: further back is older, or
    // the same second with a greater id.
    let further = match &head.oldest {
        None => false,
        Some((at, id)) => lo_id.is_empty() || lo_at < *at || (lo_at == *at && lo_id > *id),
    };
    if !further {
        return Ok(None);
    }
    let joined = conn
        .prepare_cached(
            "SELECT 1 FROM coverage WHERE room = ?1 AND stream = ?2 \
             AND lo_at <= ?3 AND hi_at >= ?4 LIMIT 1",
        )?
        .exists(params![head.room, ROOM_STREAM, hi_at, head.live_at])?;
    Ok(joined.then(|| (lo_at, lo_id, hi_at.max(head.live_at))))
}

/// Whether a head span of `stream` vouches for a window whose oldest row is
/// `oldest` (`full`: the window holds as many rows as asked) while the live
/// REQ carrying the room reached EOSE at `live_at`: the span reaches that
/// row (or is the whole history), and one room span begun at or before the
/// head's L runs to `live_at`.
pub(crate) fn vouches(
    conn: &Connection,
    room: &str,
    stream: &str,
    oldest: Option<(i64, &str)>,
    full: bool,
    live_at: i64,
) -> rusqlite::Result<bool> {
    let mut heads = conn.prepare_cached(
        "SELECT lo_at, lo_id, hi_at FROM coverage WHERE room = ?1 AND stream = ?2",
    )?;
    let spans: Vec<(i64, String, i64)> = heads
        .query_map(params![room, stream], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut joined = conn.prepare_cached(
        "SELECT 1 FROM coverage WHERE room = ?1 AND stream = ?2 \
         AND lo_at <= ?3 AND hi_at >= ?4 LIMIT 1",
    )?;
    for (lo_at, lo_id, head_at) in spans {
        // Relay order is `created_at DESC, id ASC`: the head holds every row
        // at or before its oldest one in that order.
        let reaches = lo_id.is_empty()
            || (full
                && oldest
                    .is_some_and(|(at, id)| at > lo_at || (at == lo_at && id <= lo_id.as_str())));
        if reaches && joined.exists(params![room, ROOM_STREAM, head_at, live_at])? {
            return Ok(true);
        }
    }
    Ok(false)
}
