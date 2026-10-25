//! Schema, forward migrations, and the move-aside fallback.
//!
//! The data is a cache of the server, kept permanently: a file this build
//! cannot read (corrupt, a newer schema, another scope's file) is moved aside
//! to `*.bad` (replacing any older one) and the scope starts empty; the
//! server refills it.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};

use super::SYNC_KINDS_VERSION;

pub(crate) const SCHEMA_VERSION: i64 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE events(
  rowid INTEGER PRIMARY KEY,
  id TEXT NOT NULL UNIQUE,
  channel_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  kind INTEGER NOT NULL,
  pubkey TEXT NOT NULL,
  role INTEGER NOT NULL,
  root_id TEXT,
  parent_id TEXT,
  raw TEXT NOT NULL,
  received_at INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX ev_timeline ON events(channel_id, created_at DESC, id ASC) WHERE role IN (0, 2);
CREATE INDEX ev_thread ON events(root_id, created_at, id) WHERE root_id IS NOT NULL;
CREATE INDEX ev_room ON events(channel_id, created_at);
CREATE TABLE event_targets(
  target_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  PRIMARY KEY(target_id, event_id)
) WITHOUT ROWID;
CREATE TABLE thread_summary(
  root_id TEXT PRIMARY KEY,
  channel_id TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  payload TEXT NOT NULL
);
CREATE TABLE coverage(
  channel_id TEXT NOT NULL,
  class TEXT NOT NULL CHECK(class IN ('top', 'all')),
  lo_at INTEGER NOT NULL,
  lo_id TEXT NOT NULL,
  lo_is_start INTEGER NOT NULL,
  hi_at INTEGER NOT NULL,
  kinds_version INTEGER NOT NULL,
  PRIMARY KEY(channel_id, class, lo_at, lo_id)
);
CREATE TABLE thread_coverage(
  root_id TEXT NOT NULL,
  lo_at INTEGER NOT NULL,
  lo_id TEXT NOT NULL,
  lo_is_start INTEGER NOT NULL,
  hi_at INTEGER NOT NULL,
  kinds_version INTEGER NOT NULL,
  PRIMARY KEY(root_id, lo_at, lo_id)
);
CREATE TABLE live_segment(
  id INTEGER PRIMARY KEY,
  ws_id INTEGER NOT NULL,
  started_at INTEGER NOT NULL,
  closed_at INTEGER
);
CREATE TABLE live_segment_room(
  segment_id INTEGER NOT NULL,
  channel_id TEXT NOT NULL,
  PRIMARY KEY(segment_id, channel_id)
);
CREATE TABLE room_meta(
  channel_id TEXT PRIMARY KEY,
  last_opened_at INTEGER,
  last_event_at INTEGER,
  bytes_est INTEGER NOT NULL DEFAULT 0,
  evicted_before_at INTEGER
);
";

/// Forward migrations: entry `n` upgrades version `n + 1` to `n + 2`.
const MIGRATIONS: &[&str] = &[];

/// Why an existing file cannot be used.
enum OpenError {
    /// Corrupt, unknown version, or not this scope's file: move it aside.
    Unusable(String),
    /// Anything else (permissions, disk): report, keep the file.
    Other(String),
}

fn sql_error(context: &str, error: rusqlite::Error) -> OpenError {
    let corrupt = matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    );
    let message = format!("{context}: {error}");
    if corrupt {
        OpenError::Unusable(message)
    } else {
        OpenError::Other(message)
    }
}

/// Opens (creating or migrating) the store file for `scope_key`.
pub(crate) fn open(path: &Path, scope_key: &str) -> Result<Connection, String> {
    match try_open(path, scope_key) {
        Ok(conn) => Ok(conn),
        Err(OpenError::Other(error)) => Err(error),
        Err(OpenError::Unusable(reason)) => {
            eprintln!("kuzz-desktop: message store: starting empty: {reason}");
            move_aside(path)?;
            try_open(path, scope_key).map_err(|error| match error {
                OpenError::Unusable(error) | OpenError::Other(error) => error,
            })
        }
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn move_aside(path: &Path) -> Result<(), String> {
    let bad = sibling(path, ".bad");
    let _ = std::fs::remove_file(&bad);
    std::fs::rename(path, &bad).map_err(|error| format!("move message store aside: {error}"))?;
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(sibling(path, suffix));
    }
    Ok(())
}

fn try_open(path: &Path, scope_key: &str) -> Result<Connection, OpenError> {
    let conn = Connection::open(path).map_err(|error| sql_error("open", error))?;
    conn.pragma_update(None, "busy_timeout", 5_000)
        .map_err(|error| sql_error("configure", error))?;
    let tables: i64 = conn
        .query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get(0))
        .map_err(|error| sql_error("inspect", error))?;
    if tables == 0 {
        create(&conn, scope_key).map_err(|error| sql_error("create", error))?;
    } else {
        migrate(&conn, scope_key)?;
    }
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|error| sql_error("enable WAL", error))?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|error| sql_error("configure", error))?;
    Ok(conn)
}

fn create(conn: &Connection, scope_key: &str) -> rusqlite::Result<()> {
    // Must precede the first table; it cannot be switched on later, and the
    // later deletion stage reclaims space with incremental vacuum.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    conn.execute_batch(&format!("BEGIN; {SCHEMA_V1} COMMIT;"))?;
    let meta = [
        ("schema_version", SCHEMA_VERSION.to_string()),
        ("scope_key", scope_key.to_string()),
        ("sync_kinds_version", SYNC_KINDS_VERSION.to_string()),
    ];
    for (key, value) in meta {
        conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)",
            params![key, value],
        )?;
    }
    Ok(())
}

pub(crate) fn meta(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
        row.get(0)
    })
    .optional()
}

fn migrate(conn: &Connection, scope_key: &str) -> Result<(), OpenError> {
    let has_meta: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql_error("inspect", error))?;
    if !has_meta {
        return Err(OpenError::Unusable("not a message store".to_string()));
    }
    let read = |key| meta(conn, key).map_err(|error| sql_error("read meta", error));
    let version = read("schema_version")?
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| OpenError::Unusable("no schema version".to_string()))?;
    if read("scope_key")?.as_deref() != Some(scope_key) {
        return Err(OpenError::Unusable(
            "file belongs to another scope".to_string(),
        ));
    }
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(OpenError::Unusable(format!(
            "unknown schema version {version}"
        )));
    }
    for (index, step) in MIGRATIONS.iter().enumerate().skip((version - 1) as usize) {
        let next = index as i64 + 2;
        conn.execute_batch(&format!(
            "BEGIN; {step} UPDATE meta SET value='{next}' WHERE key='schema_version'; COMMIT;"
        ))
        .map_err(|error| sql_error("migrate", error))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_with_incremental_vacuum_and_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sqlite3");
        let conn = open(&path, "scope").unwrap();
        let vacuum: i64 = conn
            .query_row("PRAGMA auto_vacuum", [], |row| row.get(0))
            .unwrap();
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(vacuum, 2);
        assert_eq!(journal, "wal");
        assert_eq!(meta(&conn, "schema_version").unwrap().as_deref(), Some("1"));
        drop(conn);
        // Reopening keeps the data.
        let conn = open(&path, "scope").unwrap();
        assert_eq!(meta(&conn, "scope_key").unwrap().as_deref(), Some("scope"));
    }

    fn rows_after_reopen(path: &Path, scope_key: &str) -> i64 {
        let conn = open(path, scope_key).unwrap();
        conn.query_row("SELECT COUNT(*) FROM room_meta", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn unknown_version_moves_aside_and_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sqlite3");
        let conn = open(&path, "scope").unwrap();
        conn.execute("INSERT INTO room_meta(channel_id) VALUES('room')", [])
            .unwrap();
        conn.execute("UPDATE meta SET value='99' WHERE key='schema_version'", [])
            .unwrap();
        drop(conn);
        assert_eq!(rows_after_reopen(&path, "scope"), 0);
        assert!(sibling(&path, ".bad").exists());
    }

    #[test]
    fn corrupt_or_foreign_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sqlite3");
        std::fs::write(&path, vec![0x5a; 8192]).unwrap();
        assert_eq!(rows_after_reopen(&path, "scope"), 0);

        let conn = open(&path, "scope").unwrap();
        conn.execute("INSERT INTO room_meta(channel_id) VALUES('room')", [])
            .unwrap();
        drop(conn);
        assert_eq!(rows_after_reopen(&path, "other-scope"), 0);
    }
}
