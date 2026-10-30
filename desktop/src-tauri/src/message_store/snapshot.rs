//! Small launch snapshots kept beside the account's messages: what the app
//! draws before the relay answers (the Chats rows, other people's profiles).
//! They live in the account's store file for one server, so another account
//! never reads them and a sign-out deletes them with the file. They are
//! drawing aids only: nothing here decides what the store forgets.
//! Rules: docs/message-sync.md#launch-snapshots.

use rusqlite::{params, Connection, OptionalExtension};

use super::writer::Input;
use super::{global, Lease, MessageStore};

pub(crate) const SNAPSHOT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS snapshot(
  kind TEXT PRIMARY KEY, saved_at INTEGER NOT NULL, blob TEXT NOT NULL);
";

/// A snapshot larger than this is refused rather than written.
pub(crate) const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;

/// Snapshot kinds: short lowercase names (`chat_rows`, `profiles`).
pub(crate) fn valid_kind(kind: &str) -> bool {
    (1..=32).contains(&kind.len())
        && kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte.is_ascii_digit())
}

/// Replaces snapshot `kind` with `blob`.
pub(crate) fn write(conn: &Connection, kind: &str, blob: &str) -> rusqlite::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    conn.execute(
        "INSERT INTO snapshot(kind, saved_at, blob) VALUES (?1, ?2, ?3) \
         ON CONFLICT(kind) DO UPDATE SET saved_at = excluded.saved_at, blob = excluded.blob",
        params![kind, now, blob],
    )?;
    Ok(())
}

/// Snapshot `kind`, or `None` when none is kept (or the file predates the
/// table and its writer has not opened it since).
fn read(conn: &Connection, kind: &str) -> rusqlite::Result<Option<String>> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'snapshot'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_table {
        return Ok(None);
    }
    conn.query_row(
        "SELECT blob FROM snapshot WHERE kind = ?1",
        params![kind],
        |row| row.get(0),
    )
    .optional()
}

impl MessageStore {
    /// Snapshot `kind` of `lease`'s file, read at once.
    pub(crate) fn read_snapshot(&self, lease: &Lease, kind: &str) -> Option<String> {
        if !valid_kind(kind) {
            return None;
        }
        self.read(lease, "snapshot read", |conn| read(conn, kind))?
    }

    /// Queues snapshot `kind` for `lease`'s file, behind what was received
    /// before it. Never marks the file lossy: a snapshot is not a receive.
    pub(crate) fn write_snapshot(&self, lease: &Lease, kind: &str, blob: String) -> bool {
        if !valid_kind(kind) || blob.len() > MAX_SNAPSHOT_BYTES {
            return false;
        }
        let mut queued = false;
        self.with_writer(lease, |writer, _| {
            queued = writer.offer(Input::Snapshot {
                kind: kind.to_owned(),
                blob,
            });
        });
        queued
    }
}

/// Snapshot `kind` kept for `relay_url` and `account`.
pub(crate) fn read_snapshot(relay_url: &str, account: &str, kind: &str) -> Option<String> {
    let store = global()?;
    let lease = store.lease(relay_url, account)?;
    store.read_snapshot(&lease, kind)
}

/// Keeps `blob` as snapshot `kind` for `relay_url` and `account`.
pub(crate) fn write_snapshot(relay_url: &str, account: &str, kind: &str, blob: String) -> bool {
    let Some(store) = global() else { return false };
    let Some(lease) = store.lease(relay_url, account) else {
        return false;
    };
    store.write_snapshot(&lease, kind, blob)
}
