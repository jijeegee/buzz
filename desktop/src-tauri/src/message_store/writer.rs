//! One writer thread per store file. Received events queue here and are
//! written in batches, so neither the socket nor a query waits for the disk.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    thread::JoinHandle,
};

use rusqlite::{params, Connection};

use serde_json::Value;

use super::classify::{classify, Filed};

/// What a receive hands the writer. Parsing happens on the writer thread.
pub(crate) enum Input {
    /// A raw relay frame; only `["EVENT", sub, event]` is kept.
    Frame(String),
    Events(Vec<Value>),
}

fn rows_of(input: Input, rows: &mut Vec<Filed>) {
    match input {
        Input::Frame(frame) => {
            if let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(&frame) {
                if parts.first().and_then(Value::as_str) == Some("EVENT") {
                    rows.extend(parts.get(2).and_then(classify));
                }
            }
        }
        Input::Events(events) => rows.extend(events.iter().filter_map(classify)),
    }
}

/// Receives waiting for the writer; beyond this one is dropped.
const QUEUE_INPUTS: usize = 4096;
/// Rows written per transaction.
const ROWS_PER_TRANSACTION: usize = 512;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS events(
  id TEXT PRIMARY KEY, kind INTEGER NOT NULL, pubkey TEXT NOT NULL,
  created_at INTEGER NOT NULL, room TEXT, root TEXT,
  broadcast INTEGER NOT NULL DEFAULT 0, raw TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS events_room_order ON events(room, root, created_at, id);
CREATE TABLE IF NOT EXISTS targets(id TEXT NOT NULL, target TEXT NOT NULL, PRIMARY KEY(target, id));
CREATE TABLE IF NOT EXISTS summaries(
  root TEXT PRIMARY KEY, room TEXT, created_at INTEGER NOT NULL, raw TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS coverage(
  room TEXT NOT NULL, stream TEXT NOT NULL,
  lo_at INTEGER NOT NULL, lo_id TEXT NOT NULL, hi_at INTEGER NOT NULL, hi_id TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS coverage_stream ON coverage(room, stream);
CREATE TABLE IF NOT EXISTS sync_state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
";
pub(crate) const SCHEMA_VERSION: i64 = 1;

pub(crate) fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let conn = Connection::open(path).map_err(|error| error.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
    )
    .map_err(|error| error.to_string())?;
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "message store version {version} is newer than this app"
        ));
    }
    conn.execute_batch(SCHEMA)
        .map_err(|error| error.to_string())?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| error.to_string())?;
    Ok(conn)
}

/// Writes `rows` in one transaction. The first copy of an event id wins; a
/// thread keeps its newest summary.
pub(crate) fn write(conn: &mut Connection, rows: &[Filed]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut event = tx.prepare_cached(
            "INSERT OR IGNORE INTO events(id, kind, pubkey, created_at, room, root, broadcast, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut target =
            tx.prepare_cached("INSERT OR IGNORE INTO targets(id, target) VALUES (?1, ?2)")?;
        let mut summary = tx.prepare_cached(
            "INSERT INTO summaries(root, room, created_at, raw) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(root) DO UPDATE SET room = excluded.room,
               created_at = excluded.created_at, raw = excluded.raw
             WHERE excluded.created_at >= summaries.created_at",
        )?;
        for row in rows {
            match row {
                Filed::Event(row) => {
                    let added = event.execute(params![
                        row.id,
                        row.kind as i64,
                        row.pubkey,
                        row.created_at,
                        row.room,
                        row.root,
                        row.broadcast,
                        row.raw
                    ])?;
                    if added > 0 {
                        for id in &row.targets {
                            target.execute(params![row.id, id])?;
                        }
                    }
                }
                Filed::Summary(row) => {
                    summary.execute(params![row.root, row.room, row.created_at, row.raw])?;
                }
            }
        }
    }
    tx.commit()
}

pub(crate) fn note_lossy(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO sync_state(key, value) VALUES ('lossy', '1')",
        [],
    )
    .map(|_| ())
}

pub(crate) struct Writer {
    sender: Option<SyncSender<Input>>,
    stop: Arc<AtomicBool>,
    lossy: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub(crate) fn start(path: PathBuf) -> Self {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_INPUTS);
        let stop = Arc::new(AtomicBool::new(false));
        let lossy = Arc::new(AtomicBool::new(false));
        let thread = {
            let (stop, lossy) = (stop.clone(), lossy.clone());
            std::thread::Builder::new()
                .name("message-store".into())
                .spawn(move || run(path, receiver, stop, lossy))
                .ok()
        };
        Self {
            sender: Some(sender),
            stop,
            lossy,
            thread,
        }
    }

    /// Queues `input`; a full queue drops them and marks the file lossy, so
    /// a later catch-up never takes it for complete.
    pub(crate) fn submit(&self, input: Input) {
        let Some(sender) = &self.sender else { return };
        match sender.try_send(input) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.lossy.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Stops after the batch in hand, discarding queued ones, and waits.
    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Writes what is queued, then waits for the thread.
    #[cfg(test)]
    pub(crate) fn finish(mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Dropping without `stop` (app exit) lets queued batches drain.
        self.sender.take();
    }
}

fn run(path: PathBuf, receiver: Receiver<Input>, stop: Arc<AtomicBool>, lossy: Arc<AtomicBool>) {
    let mut conn = match open(&path) {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("kuzz-desktop: message store: open failed: {error}");
            return;
        }
    };
    while let Ok(first) = receiver.recv() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let mut rows = Vec::new();
        rows_of(first, &mut rows);
        while rows.len() < ROWS_PER_TRANSACTION {
            match receiver.try_recv() {
                Ok(more) => rows_of(more, &mut rows),
                Err(_) => break,
            }
        }
        if rows.is_empty() {
            continue;
        }
        if let Err(error) = write(&mut conn, &rows) {
            eprintln!("kuzz-desktop: message store: write failed: {error}");
            lossy.store(true, Ordering::Relaxed);
        }
        if lossy.swap(false, Ordering::Relaxed) {
            let _ = note_lossy(&conn);
        }
    }
}
