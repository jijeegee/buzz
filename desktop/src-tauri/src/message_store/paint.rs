//! Painting a room or thread from the store: its newest stored window, read
//! at once, certain or not. It only draws the first frame while the relay is
//! asked at the same time; it never stands in for that request. With it goes
//! how far back the stored events are known complete, so a stored window
//! with a hole in it never decides where the open lands.
//! Rules: docs/message-sync.md#painting-from-the-store; shared cases:
//! test-fixtures/message-store-paint-first.json.

use rusqlite::{params, Connection};
use serde_json::Value;

use super::coverage::{is_current, ROOM_STREAM};
use super::head::{thread_head_stream, HEAD_STREAM};
use super::read::{has_job_rows, ROW_RECEIVE_MARGIN_SECS};
use super::{global, read, read_thread, Lease, MessageStore};

/// A room's painted window: the channel-window response, and the earliest
/// `created_at` from which every room event up to its newest row is known
/// stored (`None`: not known for its newest row; `i64::MIN`: the whole
/// history).
pub(crate) type RoomPaint = (Vec<Value>, Option<i64>);

/// A thread's painted replies: `(rows newest first, next older cursor when
/// more remain, complete from as for rooms)`.
pub(crate) type ThreadPaint = (Vec<Value>, Option<(i64, String)>, Option<i64>);

fn created_at(event: &Value) -> Option<i64> {
    event.get("created_at").and_then(Value::as_i64)
}

/// The earliest time from which `room`'s events are known stored without a
/// gap up to `newest_at`: room spans (each vouching from its start plus the
/// receive margin) and head spans of `head_stream` (from their oldest row,
/// or the whole history), chained while they overlap.
fn complete_from(
    conn: &Connection,
    room: &str,
    head_stream: &str,
    newest_at: i64,
) -> rusqlite::Result<Option<i64>> {
    if !is_current(conn)? {
        return Ok(None);
    }
    let mut statement = conn.prepare_cached(
        "SELECT stream, lo_at, lo_id, hi_at FROM coverage \
         WHERE room = ?1 AND (stream = ?2 OR stream = ?3)",
    )?;
    let spans: Vec<(i64, i64)> = statement
        .query_map(params![room, ROOM_STREAM, head_stream], |row| {
            let stream: String = row.get(0)?;
            let lo_at: i64 = row.get(1)?;
            let lo_id: Option<String> = row.get(2)?;
            let hi_at: i64 = row.get(3)?;
            Ok(if stream == ROOM_STREAM {
                (lo_at.saturating_add(ROW_RECEIVE_MARGIN_SECS), hi_at)
            } else if lo_id.as_deref().unwrap_or_default().is_empty() {
                (i64::MIN, hi_at)
            } else {
                (lo_at, hi_at)
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut from: Option<i64> = None;
    for &(lo, hi) in &spans {
        if lo <= newest_at && hi >= newest_at {
            from = Some(from.map_or(lo, |at| at.min(lo)));
        }
    }
    let Some(mut reach) = from else {
        return Ok(None);
    };
    loop {
        let further = spans
            .iter()
            .filter(|&&(lo, hi)| lo < reach && hi >= reach)
            .map(|&(lo, _)| lo)
            .min();
        match further {
            Some(lo) => reach = lo,
            None => return Ok(Some(reach)),
        }
    }
}

impl MessageStore {
    /// `room`'s newest stored window as a channel-window response, without
    /// waiting for queued writes; `None` when nothing is stored for it.
    pub(crate) fn paint_room_head(
        &self,
        lease: &Lease,
        room: &str,
        kinds: &[u64],
        limit: usize,
    ) -> Option<RoomPaint> {
        self.read(lease, "room paint", |conn| {
            let head = read::room_head(conn, room, kinds, limit, None)?;
            let Some(newest_at) = head.rows.first().and_then(created_at) else {
                return Ok(None);
            };
            // No live REQ carries job rows, so spans never vouch for them.
            let complete = if has_job_rows(conn, room)? {
                None
            } else {
                complete_from(conn, room, HEAD_STREAM, newest_at)?
            };
            Ok(Some((head.into_response(room), complete)))
        })?
    }

    /// `root`'s newest stored replies in `room`, without waiting for queued
    /// writes; `None` when no reply is stored.
    pub(crate) fn paint_thread_head(
        &self,
        lease: &Lease,
        room: &str,
        root: &str,
        kinds: &[u64],
        limit: usize,
    ) -> Option<ThreadPaint> {
        self.read(lease, "thread paint", |conn| {
            let head = read_thread::thread_head(conn, room, root, kinds, limit, None)?;
            let Some(newest_at) = head.rows.first().and_then(created_at) else {
                return Ok(None);
            };
            let complete = complete_from(conn, room, &thread_head_stream(root), newest_at)?;
            Ok(Some((head.rows, head.next_cursor, complete)))
        })?
    }
}

/// `room`'s newest stored window for `relay_url` and `account`, to draw
/// before the relay answers.
pub(crate) fn paint_room_head(
    relay_url: &str,
    account: &str,
    room: &str,
    kinds: &[u64],
    limit: usize,
) -> Option<RoomPaint> {
    let store = global()?;
    let lease = store.lease(relay_url, account)?;
    store.paint_room_head(&lease, room, kinds, limit)
}

/// `root`'s newest stored replies for `relay_url` and `account`, to draw
/// before the relay answers.
pub(crate) fn paint_thread_head(
    relay_url: &str,
    account: &str,
    room: &str,
    root: &str,
    kinds: &[u64],
    limit: usize,
) -> Option<ThreadPaint> {
    let store = global()?;
    let lease = store.lease(relay_url, account)?;
    store.paint_thread_head(&lease, room, root, kinds, limit)
}
