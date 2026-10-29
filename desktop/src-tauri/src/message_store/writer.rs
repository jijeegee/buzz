//! One writer thread per store file. Received events queue here and are
//! written in batches, so neither the socket nor a query waits for the disk.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread::JoinHandle,
};

use rusqlite::{params, Connection, Transaction};

use serde_json::Value;

use super::classify::{classify, Filed};
use super::coverage;
use super::purge;

/// What a receive hands the writer. Parsing happens on the writer thread.
/// Each input queued for a file takes the next number of that file's
/// sequence; a capture (see [`Writer::sequence`]) is the number reached when
/// it was taken.
pub(crate) enum Input {
    /// A raw relay frame; only `["EVENT", sub, event]` is kept. With the
    /// relay's signing key when known: its notices may forget a room
    /// (docs/message-sync.md#forgetting-a-room).
    Frame {
        frame: String,
        relay_key: Option<Arc<str>>,
    },
    Events {
        events: Vec<Value>,
        relay_key: Option<Arc<str>>,
    },
    /// Apply this membership list, fetched from capture `captured` on.
    ForgetExcept {
        keep: HashSet<String>,
        captured: u64,
    },
    /// Every event of `rooms` in `[from, to]` is in the inputs before this
    /// one; its capture began at `captured`.
    Coverage {
        rooms: Vec<String>,
        from: i64,
        to: i64,
        captured: u64,
    },
    /// Answers once everything queued before it is written: `true` when
    /// nothing was lost.
    Barrier(SyncSender<bool>),
}

#[cfg(test)]
impl Input {
    pub(crate) fn events(events: Vec<Value>) -> Self {
        Self::Events {
            events,
            relay_key: None,
        }
    }

    pub(crate) fn frame(frame: String) -> Self {
        Self::Frame {
            frame,
            relay_key: None,
        }
    }
}

/// The rows a receive carries, and the relay key it came with.
fn rows_of(input: Input) -> (Vec<Filed>, Option<Arc<str>>) {
    match input {
        Input::Frame { frame, relay_key } => {
            let mut rows = Vec::new();
            if let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(&frame) {
                if parts.first().and_then(Value::as_str) == Some("EVENT") {
                    rows.extend(parts.get(2).and_then(classify));
                }
            }
            (rows, relay_key)
        }
        Input::Events { events, relay_key } => {
            (events.iter().filter_map(classify).collect(), relay_key)
        }
        Input::Coverage { .. } | Input::ForgetExcept { .. } | Input::Barrier(_) => {
            (Vec::new(), None)
        }
    }
}

/// The room a row is filed under.
fn room_of(row: &Filed) -> Option<&str> {
    match row {
        Filed::Event(row) => row.room.as_deref(),
        Filed::Summary(row) => row.room.as_deref(),
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
CREATE INDEX IF NOT EXISTS events_job_rooms ON events(room) WHERE kind BETWEEN 43001 AND 43006;
CREATE INDEX IF NOT EXISTS events_room_rows ON events(room, created_at DESC, id) WHERE root IS NULL OR broadcast = 1;
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
        .and_then(|()| conn.execute_batch(purge::MARKS_SCHEMA))
        .map_err(|error| error.to_string())?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| error.to_string())?;
    coverage::drop_stale(&conn).map_err(|error| error.to_string())?;
    Ok(conn)
}

/// Writes `rows` in one transaction. The first copy of an event id wins; a
/// thread keeps its newest summary.
pub(crate) fn write(conn: &mut Connection, rows: &[Filed]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    write_in(&tx, rows)?;
    tx.commit()
}

fn write_in(tx: &Transaction, rows: &[Filed]) -> rusqlite::Result<()> {
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
    Ok(())
}

pub(crate) fn note_lossy(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO sync_state(key, value) VALUES ('lossy', '1')",
        [],
    )
    .map(|_| ())
}

/// Whether something received was not written, and when (unix seconds) it
/// last happened: a span starting at or before then may miss it.
#[derive(Default)]
struct Loss {
    pending: AtomicBool,
    at: AtomicI64,
}

impl Loss {
    fn note(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64);
        self.at.fetch_max(now, Ordering::SeqCst);
        self.pending.store(true, Ordering::SeqCst);
    }
}

pub(crate) struct Writer {
    sender: Option<SyncSender<Input>>,
    stop: Arc<AtomicBool>,
    lossy: Arc<Loss>,
    /// Inputs queued so far (the file's sequence). Advanced only under the
    /// store's lock, so it follows the queue's order.
    queued: AtomicU64,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    /// Starts the writer of `account`'s file at `path`.
    pub(crate) fn start(path: PathBuf, account: String) -> Self {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_INPUTS);
        let stop = Arc::new(AtomicBool::new(false));
        let lossy = Arc::new(Loss::default());
        let thread = {
            let (stop, lossy) = (stop.clone(), lossy.clone());
            std::thread::Builder::new()
                .name("message-store".into())
                .spawn(move || run(path, &account, receiver, stop, lossy))
                .ok()
        };
        Self {
            sender: Some(sender),
            stop,
            lossy,
            queued: AtomicU64::new(0),
            thread,
        }
    }

    /// The file's sequence now: a capture taken here precedes every input
    /// queued after it.
    pub(crate) fn sequence(&self) -> u64 {
        self.queued.load(Ordering::SeqCst)
    }

    fn send(&self, input: Input) -> bool {
        let sent = self
            .sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(input).is_ok());
        if sent {
            self.queued.fetch_add(1, Ordering::SeqCst);
        }
        sent
    }

    /// Queues `input`; a full queue drops them and marks the file lossy, so
    /// a later catch-up never takes it for complete.
    pub(crate) fn submit(&self, input: Input) -> bool {
        if self.sender.is_none() {
            return false;
        }
        let sent = self.send(input);
        if !sent {
            self.lossy.note();
        }
        sent
    }

    /// Asks to be told once everything queued so far is written. `None` when
    /// the writer is gone or full, or something was lost: a read then cannot
    /// vouch for what was received. Wait on the answer without holding the
    /// store's lock.
    pub(crate) fn barrier(&self) -> Option<Receiver<bool>> {
        if self.sender.is_none() || self.lossy.pending.load(Ordering::SeqCst) {
            return None;
        }
        let (reply, answer) = mpsc::sync_channel(1);
        self.send(Input::Barrier(reply)).then_some(answer)
    }

    /// Stops after the batch in hand, discarding queued ones, and waits.
    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_lossy_for_test(&self) {
        self.lossy.note();
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

fn run(
    path: PathBuf,
    account: &str,
    receiver: Receiver<Input>,
    stop: Arc<AtomicBool>,
    lossy: Arc<Loss>,
) {
    let mut conn = match open(&path) {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("kuzz-desktop: message store: open failed: {error}");
            return;
        }
    };
    let mut known = match purge::Rooms::load(&conn) {
        Ok(rooms) => rooms,
        Err(error) => {
            eprintln!("kuzz-desktop: message store: reading rooms failed: {error}");
            return;
        }
    };
    // Sequence number of the input in hand; per room, the input that first
    // wrote it this run, and the last input that forgot it.
    let mut sequence = 0u64;
    let mut born: HashMap<String, u64> = HashMap::new();
    let mut forgot: HashMap<String, u64> = HashMap::new();
    // The capture of the last list applied: an older list is stale.
    let mut list_captured = 0u64;
    // Inputs taken off the queue during a long purge, in order.
    let mut backlog: VecDeque<Input> = VecDeque::new();
    while let Some(first) = backlog.pop_front().or_else(|| receiver.recv().ok()) {
        let mut rows = Vec::new();
        let mut next = Some(first);
        while let Some(input) = next.take() {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            sequence += 1;
            match input {
                Input::Coverage {
                    mut rooms,
                    from,
                    to,
                    captured,
                } => {
                    // A span is only recorded once the events before it are
                    // written, never over a dropped one, and not for a room
                    // forgotten after its capture began.
                    let lost = flush(&mut conn, &mut rows, &lossy);
                    rooms.retain(|room| forgot.get(room).is_none_or(|&at| at <= captured));
                    for room in &rooms {
                        born.entry(room.clone()).or_insert(sequence);
                        known.note(room);
                    }
                    if !lost && from > lossy.at.load(Ordering::SeqCst) {
                        if let Err(error) = coverage::extend(&mut conn, &rooms, from, to) {
                            eprintln!("kuzz-desktop: message store: coverage failed: {error}");
                        }
                    }
                }
                Input::Barrier(reply) => {
                    let lost = flush(&mut conn, &mut rows, &lossy);
                    let _ = reply.try_send(!lost);
                }
                Input::ForgetExcept { keep, captured } if captured >= list_captured => {
                    list_captured = captured;
                    flush(&mut conn, &mut rows, &lossy);
                    let new_since = |room: &str| born.get(room).is_some_and(|&at| at > captured);
                    let take_in = || backlog.extend(receiver.try_iter());
                    match purge::apply_list(&mut conn, &mut known, &keep, new_since, take_in) {
                        Ok(rooms) => forgot.extend(rooms.into_iter().map(|room| (room, sequence))),
                        Err(error) => {
                            failed(Err(error), "forgetting rooms", &lossy);
                            reload(&conn, &mut known);
                        }
                    }
                }
                Input::ForgetExcept { .. } => {}
                other => {
                    let (received, relay_key) = rows_of(other);
                    for row in received {
                        if let Some(room) = room_of(&row) {
                            born.entry(room.to_owned()).or_insert(sequence);
                            known.note(room);
                        }
                        let notice = match (&row, &relay_key) {
                            (Filed::Event(event), Some(key)) => purge::notice(event, account, key),
                            _ => None,
                        };
                        let (Some(notice), Filed::Event(event)) = (notice, &row) else {
                            rows.push(row);
                            continue;
                        };
                        // Rows received before the notice are written first,
                        // so it forgets them too.
                        flush(&mut conn, &mut rows, &lossy);
                        let applied =
                            purge::apply_notice(&mut conn, &mut known, event, notice, |tx| {
                                write_in(tx, std::slice::from_ref(&row))
                            });
                        match applied {
                            Ok(true) => {
                                if let Some(room) = &event.room {
                                    forgot.insert(room.clone(), sequence);
                                }
                            }
                            Ok(false) => {}
                            Err(error) => {
                                failed(Err(error), "forgetting a room", &lossy);
                                reload(&conn, &mut known);
                            }
                        }
                    }
                }
            }
            if rows.len() >= ROWS_PER_TRANSACTION {
                flush(&mut conn, &mut rows, &lossy);
            }
            next = backlog.pop_front().or_else(|| receiver.try_recv().ok());
        }
        flush(&mut conn, &mut rows, &lossy);
    }
}

/// After a failed purge the rooms in memory may not match the file.
fn reload(conn: &Connection, known: &mut purge::Rooms) {
    match purge::Rooms::load(conn) {
        Ok(rooms) => *known = rooms,
        Err(error) => eprintln!("kuzz-desktop: message store: reading rooms failed: {error}"),
    }
}

/// A failed forget leaves rows coverage may still vouch for: treat it as a
/// loss, so every span is cleared and later ones from before it refused.
fn failed(result: rusqlite::Result<()>, what: &str, lossy: &Loss) {
    if let Err(error) = result {
        eprintln!("kuzz-desktop: message store: {what} failed: {error}");
        lossy.note();
    }
}

/// Writes `rows`; returns whether anything was lost since the last flush, in
/// which case the file is marked lossy and its coverage forgotten.
fn flush(conn: &mut Connection, rows: &mut Vec<Filed>, lossy: &Loss) -> bool {
    if !rows.is_empty() {
        if let Err(error) = write(conn, rows) {
            eprintln!("kuzz-desktop: message store: write failed: {error}");
            lossy.note();
        }
        rows.clear();
    }
    if !lossy.pending.swap(false, Ordering::SeqCst) {
        return false;
    }
    // Until the spans are gone the loss stays pending, so reads stay
    // uncertain and the next flush tries again.
    if let Err(error) = note_lossy(conn).and_then(|()| coverage::clear(conn)) {
        eprintln!("kuzz-desktop: message store: forgetting coverage failed: {error}");
        lossy.pending.store(true, Ordering::SeqCst);
    }
    true
}
