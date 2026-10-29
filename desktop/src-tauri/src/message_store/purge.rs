//! Forgetting a room's stored copy: when the account's membership list
//! drops a room it had, when a room a list dropped comes back (a rejoin),
//! and on the relay's rejoin and `since_join` notices. Rules:
//! docs/message-sync.md#forgetting-a-room; cases:
//! test-fixtures/message-store-purge.json (shared with mobile `purge.dart`).

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

use super::classify::EventRow;

/// Per room: the `created_at` of the last rejoin notice handled
/// (`joined_at`), when the account left it (`left_at`, 0: not left; from a
/// relay notice, or the newest stored row when a list dropped it) and of the
/// last history notice handled (`history_at`). Plus the indexes a room's
/// purge deletes by.
pub(crate) const MARKS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS room_purge(
  room TEXT PRIMARY KEY, joined_at INTEGER NOT NULL DEFAULT 0,
  left_at INTEGER NOT NULL DEFAULT 0, history_at INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS targets_id ON targets(id);
CREATE INDEX IF NOT EXISTS summaries_room ON summaries(room);
";

/// Rooms forgotten per transaction; between chunks the writer takes in what
/// was queued, so a long purge cannot fill the queue.
const ROOMS_PER_TRANSACTION: usize = 16;

/// A list may forget at most this many kept rooms, or half of them, at once;
/// beyond both it is taken for a bad answer and skipped.
const MAX_SURE_LOSS: usize = 5;

/// What a relay system message (40099) about the store's account says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Notice {
    /// `member_joined` naming the account. The relay sends it on every join,
    /// the first one too.
    Joined,
    /// `member_left` by the account, or `member_removed` naming it.
    Left,
    /// `history_changed`; `true` when to `since_join`.
    History(bool),
}

/// The notice `row` carries for `account`, when it is a 40099 signed by
/// `relay_key` (the relay's NIP-11 `self`).
pub(crate) fn notice(row: &EventRow, account: &str, relay_key: &str) -> Option<Notice> {
    if row.kind != 40099 || row.room.is_none() || !row.pubkey.eq_ignore_ascii_case(relay_key) {
        return None;
    }
    let event: Value = serde_json::from_str(&row.raw).ok()?;
    let content: Value = serde_json::from_str(event.get("content")?.as_str()?).ok()?;
    let field = |name: &str| content.get(name).and_then(Value::as_str);
    let names = |name: &str| field(name).is_some_and(|value| value.eq_ignore_ascii_case(account));
    match field("type")? {
        "member_joined" if names("target") => Some(Notice::Joined),
        "member_left" if field("target").map_or(names("actor"), |_| names("target")) => {
            Some(Notice::Left)
        }
        "member_removed" if names("target") => Some(Notice::Left),
        "history_changed" => Some(Notice::History(field("history")? == "since_join")),
        _ => None,
    }
}

/// The rooms of one file, kept in the writer's memory so a list is applied
/// without scanning the file. Loaded once when the writer opens it.
#[derive(Default)]
pub(crate) struct Rooms {
    /// Rooms with stored rows, summaries or coverage (a superset is fine).
    stored: HashSet<String>,
    /// Rooms with a left mark.
    left: HashSet<String>,
    /// The last membership list applied (`None`: none yet).
    kept: Option<HashSet<String>>,
    /// A room was first stored since the last list was applied.
    fresh: bool,
}

impl Rooms {
    pub(crate) fn load(conn: &Connection) -> rusqlite::Result<Self> {
        let column = |sql: &str| -> rusqlite::Result<HashSet<String>> {
            let mut statement = conn.prepare(sql)?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect()
        };
        let kept = conn
            .query_row(
                "SELECT value FROM sync_state WHERE key = 'kept_rooms'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| serde_json::from_str(&value).ok());
        Ok(Self {
            stored: column(
                "SELECT room FROM events WHERE room IS NOT NULL
                 UNION SELECT room FROM summaries WHERE room IS NOT NULL
                 UNION SELECT room FROM coverage",
            )?,
            left: column("SELECT room FROM room_purge WHERE left_at > 0")?,
            kept,
            fresh: false,
        })
    }

    /// Something of `room` is about to be written.
    pub(crate) fn note(&mut self, room: &str) {
        if !self.stored.contains(room) {
            self.stored.insert(room.to_owned());
            self.fresh = true;
        }
    }
}

#[derive(Default)]
struct Marks {
    joined_at: i64,
    left_at: i64,
    history_at: i64,
}

fn marks(conn: &Connection, room: &str) -> rusqlite::Result<Marks> {
    Ok(conn
        .query_row(
            "SELECT joined_at, left_at, history_at FROM room_purge WHERE room = ?1",
            params![room],
            |row| {
                Ok(Marks {
                    joined_at: row.get(0)?,
                    left_at: row.get(1)?,
                    history_at: row.get(2)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

fn set_marks(tx: &Transaction, room: &str, marks: &Marks) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT OR REPLACE INTO room_purge(room, joined_at, left_at, history_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![room, marks.joined_at, marks.left_at, marks.history_at],
    )?;
    Ok(())
}

/// Deletes `room`'s rows created before `before` (every row when `None`),
/// the room-less overlays (reactions, deletions) aimed only at them (one
/// also naming a row that stays is kept, or that row would lose it), their
/// thread summaries (every summary of the room when `None`) and all the
/// room's coverage.
fn forget_room(tx: &Transaction, room: &str, before: Option<i64>) -> rusqlite::Result<()> {
    let before = before.unwrap_or(i64::MAX);
    let whole = before == i64::MAX;
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS purge_ids(id TEXT PRIMARY KEY); DELETE FROM purge_ids;",
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO purge_ids SELECT id FROM events WHERE room = ?1 AND created_at < ?2",
        params![room, before],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO purge_ids SELECT DISTINCT t.id FROM targets t
         JOIN events o ON o.id = t.id
         WHERE o.room IS NULL
           AND t.target IN (SELECT id FROM events WHERE room = ?1 AND created_at < ?2)
           AND NOT EXISTS (SELECT 1 FROM targets u WHERE u.id = t.id AND u.target NOT IN
                 (SELECT id FROM events WHERE room = ?1 AND created_at < ?2))",
        params![room, before],
    )?;
    tx.execute(
        "DELETE FROM summaries WHERE (room = ?1 AND ?2) OR root IN (SELECT id FROM purge_ids)",
        params![room, whole],
    )?;
    tx.execute_batch(
        "DELETE FROM targets WHERE id IN (SELECT id FROM purge_ids);
         DELETE FROM events WHERE id IN (SELECT id FROM purge_ids);
         DELETE FROM purge_ids;",
    )?;
    tx.execute("DELETE FROM coverage WHERE room = ?1", params![room])?;
    Ok(())
}

/// Forgets a room a list dropped and marks it left as of its newest stored
/// row (0, no mark, when it had none): a rejoin notice older than that is
/// from an earlier membership.
fn forget_dropped(tx: &Transaction, room: &str) -> rusqlite::Result<bool> {
    let newest: i64 = tx.query_row(
        "SELECT max(coalesce((SELECT max(created_at) FROM events WHERE room = ?1), 0),
                    coalesce((SELECT max(created_at) FROM summaries WHERE room = ?1), 0))",
        params![room],
        |row| row.get(0),
    )?;
    forget_room(tx, room, None)?;
    if newest > 0 {
        let mut current = marks(tx, room)?;
        current.left_at = current.left_at.max(newest);
        set_marks(tx, room, &current)?;
    }
    Ok(newest > 0)
}

/// Applies the account's membership list `keep`
/// (docs/message-sync.md#forgetting-a-room):
/// - the first list (none kept yet) is only recorded;
/// - a room of the last list that `keep` lacks is forgotten and marked left;
/// - a room of `keep` marked left (a list dropped it) is forgotten again,
///   its rows are from while the account was out, and loses the mark;
/// - rooms no list had (open channels only browsed) are left alone;
/// - rooms `skip` names (first written after the list's fetch began) are
///   left alone, a dropped one stays kept for the next list;
/// - a list dropping more than [`MAX_SURE_LOSS`] rooms and half the kept
///   ones is skipped, as is one equal to the last with no room stored since
///   and none of it marked left.
///
/// `between` runs between transactions. Returns the rooms forgotten. On an
/// error `rooms` may be ahead of the file: reload it.
pub(crate) fn apply_list(
    conn: &mut Connection,
    rooms: &mut Rooms,
    keep: &HashSet<String>,
    skip: impl Fn(&str) -> bool,
    mut between: impl FnMut(),
) -> rusqlite::Result<Vec<String>> {
    let unchanged =
        rooms.kept.as_ref() == Some(keep) && !rooms.fresh && keep.is_disjoint(&rooms.left);
    if keep.is_empty() || unchanged {
        return Ok(Vec::new());
    }
    let mut kept = keep.clone();
    let mut doomed = Vec::new();
    if let Some(previous) = &rooms.kept {
        let dropped: Vec<&String> = previous.difference(keep).collect();
        if dropped.len() > MAX_SURE_LOSS && dropped.len() * 2 > previous.len() {
            eprintln!(
                "kuzz-desktop: message store: a list dropping {} of {} rooms was not applied",
                dropped.len(),
                previous.len()
            );
            return Ok(Vec::new());
        }
        for room in dropped {
            if skip(room) {
                kept.insert(room.clone());
            } else if rooms.stored.contains(room) {
                doomed.push((room.clone(), false));
            }
        }
        for room in keep.intersection(&rooms.left) {
            if rooms.stored.contains(room) && !skip(room) {
                doomed.push((room.clone(), true));
            }
        }
    }
    let back: Vec<String> = keep.intersection(&rooms.left).cloned().collect();
    let mut forgotten = Vec::new();
    let mut chunks = doomed.chunks(ROOMS_PER_TRANSACTION).peekable();
    while let Some(chunk) = chunks.next() {
        let tx = conn.transaction()?;
        for (room, rejoined) in chunk {
            if *rejoined {
                forget_room(&tx, room, None)?;
            } else if forget_dropped(&tx, room)? {
                rooms.left.insert(room.clone());
            }
            rooms.stored.remove(room);
            forgotten.push(room.clone());
        }
        tx.commit()?;
        if chunks.peek().is_some() {
            between();
        }
    }
    let tx = conn.transaction()?;
    for room in &back {
        let mut current = marks(&tx, room)?;
        current.left_at = 0;
        set_marks(&tx, room, &current)?;
        rooms.left.remove(room);
    }
    let mut sorted: Vec<&String> = kept.iter().collect();
    sorted.sort_unstable();
    tx.execute(
        "INSERT OR REPLACE INTO sync_state(key, value) VALUES ('kept_rooms', ?1)",
        params![serde_json::to_string(&sorted).unwrap_or_default()],
    )?;
    tx.commit()?;
    rooms.kept = Some(kept);
    rooms.fresh = false;
    Ok(forgotten)
}

/// Acts on a relay notice, then writes it. Returns whether the room was
/// forgotten.
/// - `Joined` newer than the last one handled is recorded; only when the
///   room is marked left before it does it forget the rows older than it
///   (and clear the mark): the first join keeps a room's history.
/// - `Left` newer than the last join marks the room left.
/// - `History` newer than the last one handled is recorded; only
///   `since_join` forgets the room's rows older than it: newer ones were
///   sent to this account under the new rule.
///
/// On an error `rooms` may be ahead of the file: reload it.
pub(crate) fn apply_notice(
    conn: &mut Connection,
    rooms: &mut Rooms,
    row: &EventRow,
    notice: Notice,
    write: impl FnOnce(&Transaction) -> rusqlite::Result<()>,
) -> rusqlite::Result<bool> {
    let Some(room) = row.room.as_deref() else {
        return Ok(false);
    };
    let at = row.created_at;
    let tx = conn.transaction()?;
    let mut current = marks(&tx, room)?;
    let mut forgot = false;
    match notice {
        Notice::Joined if at > current.joined_at => {
            current.joined_at = at;
            if current.left_at > 0 && at > current.left_at {
                forget_room(&tx, room, Some(at))?;
                current.left_at = 0;
                rooms.left.remove(room);
                forgot = true;
            }
            set_marks(&tx, room, &current)?;
        }
        Notice::Left if at > current.joined_at && at > current.left_at => {
            current.left_at = at;
            rooms.left.insert(room.to_owned());
            set_marks(&tx, room, &current)?;
        }
        Notice::History(since_join) if at > current.history_at => {
            current.history_at = at;
            if since_join {
                forget_room(&tx, room, Some(at))?;
                forgot = true;
            }
            set_marks(&tx, room, &current)?;
        }
        _ => {}
    }
    write(&tx)?;
    tx.commit()?;
    Ok(forgot)
}
