//! The user stream's watermark `room_seq` and its `sync_state` keys
//! (docs/message-sync.md#watermark-room_seq, test-fixtures/user-stream.json
//! `ruleW`, `epoch`, `clearing`).

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::{coverage, Lease, MessageStore};

pub(crate) const ROOM_SEQ_SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS room_seq(room TEXT PRIMARY KEY, seq INTEGER NOT NULL, at INTEGER NOT NULL);";

/// The `sync_state` key holding the last accepted snapshot epoch.
const EPOCH_KEY: &str = "stream_epoch";
/// Other `sync_state` keys the app keeps for the stream, per account: the
/// overflow backoff, the cumulative counters, and the previous connection's
/// live room set with its two bounds.
pub(crate) const STATE_KEYS: [&str; 3] = ["stream_backoff", "stream_counters", "stream_prev_live"];
/// A stored state value is at most this long.
const MAX_STATE_BYTES: usize = 256 * 1024;

/// The largest seq a snapshot may carry (2^53 - 1).
pub(crate) const MAX_SEQ: i64 = (1 << 53) - 1;
/// An epoch is at most this many UTF-16 units, as the app counts them.
pub(crate) const MAX_EPOCH_UNITS: usize = 64;

pub(crate) fn valid_epoch(epoch: &str) -> bool {
    !epoch.is_empty() && epoch.encode_utf16().count() <= MAX_EPOCH_UNITS
}

pub(crate) fn valid_seq(seq: i64) -> bool {
    (0..=MAX_SEQ).contains(&seq)
}

/// Rule-W rows riding a catch-up's span: each room's snapshot seq, under the
/// epoch the catch-up started with. `at` is the span's end (L).
#[derive(Debug, Clone)]
pub(crate) struct SeqWrite {
    pub(crate) epoch: String,
    pub(crate) seqs: HashMap<String, i64>,
}

pub(crate) fn epoch(conn: &Connection) -> rusqlite::Result<Option<String>> {
    state(conn, EPOCH_KEY)
}

/// Stores `epoch` from an accepted part 1; a different one (or the first)
/// clears every row in the same transaction. Returns whether it changed.
pub(crate) fn set_epoch(conn: &mut Connection, epoch: &str) -> rusqlite::Result<bool> {
    let tx = conn.transaction()?;
    let changed = self::epoch(&tx)?.as_deref() != Some(epoch);
    if changed {
        tx.execute("DELETE FROM room_seq", [])?;
        tx.execute(
            "INSERT OR REPLACE INTO sync_state(key, value) VALUES (?1, ?2)",
            params![EPOCH_KEY, epoch],
        )?;
    }
    tx.commit()?;
    Ok(changed)
}

/// Writes `seq`'s rows for `rooms` (the rooms whose span was just recorded
/// in `tx`) at `at`. Refused when the stored epoch is another one; never
/// lowers a stored seq; an equal seq moves `at` (never back).
pub(crate) fn write(
    tx: &Transaction,
    rooms: &[String],
    at: i64,
    seq: &SeqWrite,
) -> rusqlite::Result<usize> {
    if epoch(tx)?.as_deref() != Some(seq.epoch.as_str()) {
        return Ok(0);
    }
    let mut upsert = tx.prepare_cached(
        "INSERT INTO room_seq(room, seq, at) VALUES (?1, ?2, ?3)
         ON CONFLICT(room) DO UPDATE SET seq = excluded.seq, at = max(room_seq.at, excluded.at)
         WHERE excluded.seq >= room_seq.seq",
    )?;
    let mut written = 0;
    for room in rooms {
        if let Some(&value) = seq.seqs.get(room).filter(|&&value| valid_seq(value)) {
            written += upsert.execute(params![room, value, at])?;
        }
    }
    Ok(written)
}

/// Clears every row (a loss or a coverage version change).
pub(crate) fn clear(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM room_seq", []).map(|_| ())
}

/// Clears `room`'s row (the room is forgotten).
pub(crate) fn clear_room(tx: &Transaction, room: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM room_seq WHERE room = ?1", params![room])
        .map(|_| ())
}

/// Clears the rows of `rooms` (a snapshot seq below the stored one).
pub(crate) fn clear_rooms(conn: &mut Connection, rooms: &[String]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    for room in rooms {
        clear_room(&tx, room)?;
    }
    tx.commit()
}

/// The stored rows of `rooms`, as `(seq, at)`.
pub(crate) fn rows(
    conn: &Connection,
    rooms: &[String],
) -> rusqlite::Result<HashMap<String, (i64, i64)>> {
    let mut statement = conn.prepare_cached("SELECT seq, at FROM room_seq WHERE room = ?1")?;
    let mut found = HashMap::new();
    for room in rooms {
        let row = statement
            .query_row(params![room], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        if let Some(row) = row {
            found.insert(room.clone(), row);
        }
    }
    Ok(found)
}

pub(crate) fn state(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM sync_state WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
}

pub(crate) fn set_state(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO sync_state(key, value) VALUES (?1, ?2)",
        params![key, value],
    )
    .map(|_| ())
}

/// Which of `ids` were not stored before `since_ms` (absent, or stored at
/// or after it): rows that count as new for the gate counters, whichever
/// path stored them (docs/message-sync.md#counters).
fn fresh_ids(conn: &Connection, ids: &[String], since_ms: i64) -> rusqlite::Result<Vec<String>> {
    let mut stored = conn.prepare_cached("SELECT stored_at FROM events WHERE id = ?1")?;
    let mut fresh = Vec::new();
    for id in ids {
        let at: Option<Option<i64>> = stored.query_row(params![id], |row| row.get(0)).optional()?;
        match at {
            Some(None) => {}
            Some(Some(at)) if at < since_ms => {}
            _ => fresh.push(id.clone()),
        }
    }
    Ok(fresh)
}

/// The stored epoch and the rows of some rooms, read together.
#[derive(Debug, Default, serde::Serialize)]
pub(crate) struct StoredSeqs {
    /// Everything queued was written and nothing was lost when read.
    pub(crate) vouched: bool,
    pub(crate) epoch: Option<String>,
    pub(crate) rows: HashMap<String, (i64, i64)>,
    /// Resume points from coverage that began before the asked time.
    pub(crate) resume: HashMap<String, i64>,
}

impl MessageStore {
    /// Queues an accepted part 1's epoch ahead of that connection's rule-W
    /// writes. `false` when it is invalid or could not be queued: the app
    /// then reads rows of another epoch as none.
    pub(crate) fn stream_epoch(&self, lease: &Lease, epoch: String) -> bool {
        if !valid_epoch(&epoch) {
            return false;
        }
        let mut queued = false;
        self.with_writer(lease, |writer, _| {
            queued = writer.offer(super::Input::StreamEpoch { epoch });
        });
        queued
    }

    /// Queues deleting the rows of `rooms` (ahead of the snapshot).
    pub(crate) fn clear_stream_rows(&self, lease: &Lease, rooms: Vec<String>) -> bool {
        let mut queued = false;
        self.with_writer(lease, |writer, _| {
            queued = writer.offer(super::Input::StreamClearRows { rooms });
        });
        queued
    }

    /// Queues a stream `sync_state` value (one of [`STATE_KEYS`]).
    pub(crate) fn set_stream_state(&self, lease: &Lease, key: &str, value: String) -> bool {
        if !STATE_KEYS.contains(&key) || value.len() > MAX_STATE_BYTES {
            return false;
        }
        let key = key.to_owned();
        let mut queued = false;
        self.with_writer(lease, |writer, _| {
            queued = writer.offer(super::Input::StreamState { key, value });
        });
        queued
    }

    /// A stream `sync_state` value as the file holds it (a value queued
    /// just before may not be written yet).
    pub(crate) fn stream_state(&self, lease: &Lease, key: &str) -> Option<String> {
        if !STATE_KEYS.contains(&key) {
            return None;
        }
        self.read(lease, "stream state", |conn| state(conn, key))
            .flatten()
    }

    /// Queues a catch-up's span for `rooms` over `[from, to]` with its
    /// rule-W rows, if `scope` (its capture) is still current. The rows are
    /// written with the span, in its transaction, and refused with it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn extend_stream_coverage(
        &self,
        relay_url: &str,
        account: &str,
        scope: Option<&str>,
        rooms: Vec<String>,
        from: i64,
        to: i64,
        seq: SeqWrite,
    ) -> Result<(), String> {
        let Some(lease) = self.lease(relay_url, account) else {
            return Ok(());
        };
        let captured = super::captured(scope, &lease)?;
        if !valid_epoch(&seq.epoch) {
            return Err("invalid stream epoch".into());
        }
        let rooms = rooms
            .into_iter()
            .filter(|room| room != super::stream_tap::USER_STREAM_ROOM)
            .collect();
        self.submit(
            &lease,
            super::Input::StreamCoverage {
                rooms,
                from,
                to,
                captured,
                seq,
            },
        );
        Ok(())
    }

    /// The epoch, `rooms`' rows and their resume points from coverage begun
    /// before `before`, once nothing queued is pending and no loss is:
    /// otherwise none, so nothing is skipped on them and no room is handed
    /// to the stream (docs/message-sync.md#handoff).
    pub(crate) fn stream_seqs(
        &self,
        lease: &Lease,
        rooms: &[String],
        before: Option<i64>,
    ) -> StoredSeqs {
        if !self.drained(lease) {
            return StoredSeqs::default();
        }
        self.read(lease, "stream seqs", |conn| {
            Ok(StoredSeqs {
                vouched: true,
                epoch: epoch(conn)?,
                rows: rows(conn, rooms)?,
                resume: match before {
                    Some(before) => coverage::resume_points(conn, rooms, before)?,
                    None => HashMap::new(),
                },
            })
        })
        .unwrap_or_default()
    }

    /// Each room's coverage segments, read before a catch-up walk once
    /// what is queued is written; `None` when that cannot be vouched for.
    pub(crate) fn walk_segments(
        &self,
        lease: &Lease,
        rooms: &[String],
    ) -> Option<HashMap<String, Vec<(i64, i64)>>> {
        if !self.drained(lease) {
            return None;
        }
        self.read(lease, "walk segments", |conn| {
            rooms
                .iter()
                .map(|room| Ok((room.clone(), coverage::segments(conn, room)?)))
                .collect()
        })
    }

    /// Which of `ids` were not stored before `since_ms`, once what is queued
    /// is written; `None` when that cannot be vouched for.
    pub(crate) fn fresh_ids(
        &self,
        lease: &Lease,
        ids: &[String],
        since_ms: i64,
    ) -> Option<Vec<String>> {
        if !self.drained(lease) {
            return None;
        }
        self.read(lease, "fresh ids", |conn| fresh_ids(conn, ids, since_ms))
    }
}
