//! Opening a thread from the store: its newest replies by the relay's
//! thread-window rules, and whether that window is certain.
//! Rules: docs/message-sync.md#opening-a-thread-from-the-store; shared cases:
//! test-fixtures/message-store-thread-head.json.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::coverage::ROOM_STREAM;
use super::read::{is_deleted, parse, ROW_RECEIVE_MARGIN_SECS};

/// A thread's newest replies as the store has them.
#[derive(Debug, Default)]
pub(crate) struct ThreadHead {
    /// Every reply of the window is known stored and the room's live REQ is
    /// still receiving, so the window equals what the relay would send.
    pub certain: bool,
    /// Newest first.
    pub rows: Vec<Value>,
    /// Older replies may exist: the window is full.
    pub has_more: bool,
    /// `(created_at, id)` of the oldest reply, when `has_more`.
    pub next_cursor: Option<(i64, String)>,
}

fn covered(conn: &Connection, room: &str, from: i64, live_at: i64) -> rusqlite::Result<bool> {
    conn.prepare_cached(
        "SELECT 1 FROM coverage WHERE room = ?1 AND stream = ?2 \
         AND lo_at <= ?3 AND hi_at >= ?4 LIMIT 1",
    )?
    .exists(params![room, ROOM_STREAM, from, live_at])
}

/// The newest `limit` replies of `kinds` at any depth under `root` in
/// `room`. `live_at` is when the room's live REQ reached EOSE on the
/// current connection, if it is live.
pub(crate) fn thread_head(
    conn: &Connection,
    room: &str,
    root: &str,
    kinds: &[u64],
    limit: usize,
    live_at: Option<i64>,
) -> rusqlite::Result<ThreadHead> {
    let mut head = ThreadHead::default();
    if kinds.is_empty() || limit == 0 {
        return Ok(head);
    }
    let kind_list = kinds
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut candidates = conn.prepare(&format!(
        "SELECT id, pubkey, created_at, raw FROM events \
         WHERE room = ?1 AND root = ?2 AND kind IN ({kind_list}) \
         ORDER BY created_at DESC, id ASC"
    ))?;
    let mut found = candidates.query(params![room, root])?;
    let mut oldest = None;
    while let Some(row) = found.next()? {
        let (id, author, created_at): (String, String, i64) =
            (row.get(0)?, row.get(1)?, row.get(2)?);
        let Some(event) = parse(row.get(3)?) else {
            continue;
        };
        if is_deleted(conn, &id, &author)? {
            continue;
        }
        oldest = Some((created_at, id));
        head.rows.push(event);
        if head.rows.len() == limit {
            head.has_more = true;
            break;
        }
    }
    drop(found);

    let Some(live_at) = live_at else {
        head.next_cursor = oldest.filter(|_| head.has_more);
        return Ok(head);
    };
    head.certain = if head.has_more {
        let (oldest_at, _) = oldest.as_ref().expect("a full window has rows");
        covered(conn, room, oldest_at - ROW_RECEIVE_MARGIN_SECS, live_at)?
    } else {
        // Fewer replies than asked: only certain when every reply since the
        // root is known stored.
        let root_at: Option<i64> = conn
            .query_row(
                "SELECT created_at FROM events WHERE id = ?1 AND room = ?2",
                params![root, room],
                |row| row.get(0),
            )
            .optional()?;
        match root_at {
            Some(root_at) => covered(conn, room, root_at - ROW_RECEIVE_MARGIN_SECS, live_at)?,
            None => false,
        }
    } && super::coverage::is_current(conn)?;
    head.next_cursor = oldest.filter(|_| head.has_more);
    Ok(head)
}
