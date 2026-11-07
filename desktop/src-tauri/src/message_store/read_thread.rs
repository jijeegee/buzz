//! Opening a thread from the store: its newest replies by the relay's
//! thread-window rules, and whether that window is certain; and its root by
//! id (docs/message-sync.md#opening-a-thread-from-its-root-id).
//! Rules: docs/message-sync.md#opening-a-thread-from-the-store; shared cases:
//! test-fixtures/message-store-thread-head.json.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::coverage::ROOM_STREAM;
use super::head::{thread_head_stream, vouches};
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
    if kinds.is_empty() || limit == 0 {
        return Ok(ThreadHead::default());
    }
    let (mut head, oldest) = thread_window(conn, room, root, kinds, limit, None)?;
    certify(conn, room, root, &mut head, oldest, live_at)?;
    Ok(head)
}

/// The `limit` replies of `kinds` at any depth under `root` in `room` below
/// `before` (`(created_at, id)` in relay order, exclusive; `None`: the
/// newest), never certain, and the oldest of them.
pub(crate) fn thread_window(
    conn: &Connection,
    room: &str,
    root: &str,
    kinds: &[u64],
    limit: usize,
    before: Option<(i64, &str)>,
) -> rusqlite::Result<(ThreadHead, Option<(i64, String)>)> {
    let mut head = ThreadHead::default();
    if kinds.is_empty() || limit == 0 {
        return Ok((head, None));
    }
    let kind_list = kinds
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut candidates = conn.prepare(&format!(
        "SELECT id, pubkey, created_at, raw FROM events \
         WHERE room = ?1 AND root = ?2 AND kind IN ({kind_list}) \
         AND (?3 IS NULL OR created_at < ?3 OR (created_at = ?3 AND id > ?4)) \
         ORDER BY created_at DESC, id ASC"
    ))?;
    let (before_at, before_id) = before.map_or((None, ""), |(at, id)| (Some(at), id));
    let mut found = candidates.query(params![room, root, before_at, before_id])?;
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
    head.next_cursor = oldest.clone().filter(|_| head.has_more);
    Ok((head, oldest))
}

/// Sets whether `head` (newest replies, oldest `oldest`) is certain.
fn certify(
    conn: &Connection,
    room: &str,
    root: &str,
    head: &mut ThreadHead,
    oldest: Option<(i64, String)>,
    live_at: Option<i64>,
) -> rusqlite::Result<()> {
    let Some(live_at) = live_at else {
        return Ok(());
    };
    let oldest_row = oldest.as_ref().map(|(at, id)| (*at, id.as_str()));
    let stream = thread_head_stream(root);
    // A head span of the thread's own window (docs/message-sync.md#head-spans).
    let headed = vouches(conn, room, &stream, oldest_row, head.has_more, live_at)?
        && super::coverage::is_current(conn)?;
    head.certain = headed
        || if head.has_more {
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
    Ok(())
}

/// `root` of `room` as the store holds it, and whether a stored deletion
/// removes it: a removal (9005), or a 5 by the root's author, filed in `room`
/// either way. A removal counts even when the root row is not stored; an
/// author's 5 cannot be judged without the root, so then it does not. `None`
/// when neither the root nor a removal of it is stored.
pub(crate) fn thread_root(
    conn: &Connection,
    room: &str,
    root: &str,
) -> rusqlite::Result<Option<(Option<Value>, bool)>> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT pubkey, raw FROM events WHERE id = ?1 AND room = ?2",
            params![root, room],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let author = row.as_ref().map(|(author, _)| author.as_str());
    let deleted = conn
        .prepare_cached(
            "SELECT 1 FROM targets t JOIN events e ON e.id = t.id \
             WHERE t.target = ?1 AND e.room = ?2 \
             AND (e.kind = 9005 OR (e.kind = 5 AND e.pubkey = ?3)) LIMIT 1",
        )?
        .exists(params![root, room, author])?;
    Ok(match row {
        Some((_, raw)) => Some((parse(raw), deleted)),
        None => deleted.then_some((None, true)),
    })
}
