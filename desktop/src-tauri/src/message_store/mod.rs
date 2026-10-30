//! Device message store: every received room event, kept per server and
//! account in its own SQLite file, read to open rooms when certain.
//! Rules: docs/message-sync.md#message-store.

mod classify;
mod clock;
mod coverage;
mod head;
#[cfg(test)]
mod head_tests;
mod paint;
#[cfg(test)]
mod paint_tests;
mod purge;
mod read;
mod read_thread;
mod snapshot;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod tests;
mod writer;

pub(crate) use clock::{clock_trusted, note_server_date};

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[cfg(test)]
pub(crate) use classify::{classify, Filed};
pub(crate) use head::HeadSpan;
pub(crate) use paint::{paint_room_head, paint_thread_head};
pub(crate) use snapshot::{read_snapshot, write_snapshot};
use writer::{Input, Writer};

static STORE: OnceLock<MessageStore> = OnceLock::new();

/// Opens the app's store under `data_dir`. Called once at startup.
pub(crate) fn init(data_dir: &Path) {
    let _ = STORE.set(MessageStore::new(data_dir.join("messages")));
}

pub(crate) fn global() -> Option<&'static MessageStore> {
    STORE.get()
}

/// Deletes everything kept for `account` (hex pubkey) on this device.
/// Deletes what `account` kept from `relay_url` (sign-out of that server).
pub(crate) fn forget_account(relay_url: &str, account: &str) {
    if let Some(store) = global() {
        store.forget(relay_url, account);
    }
}

/// The `["AUTH", {"token"}]` payload for `token`, noting that sockets
/// authenticated with it receive as `account` (a token names no pubkey).
pub(crate) fn token_frame(token: &str, account: &str) -> Value {
    if let Some(store) = global() {
        store.note_token(token, account);
    }
    serde_json::json!({ "token": token })
}

/// Recently issued AUTH tokens remembered, by hash, for their account.
const TOKENS_KEPT: usize = 32;
/// How long a store open waits for queued writes before asking the relay.
const DRAIN_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// The scope a receive belongs to, taken when the request or socket starts.
/// A lease from before the account signed out of that server writes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lease {
    /// `<origin hash>-<account>`: one file, one epoch.
    stem: String,
    epoch: u64,
}

impl Lease {
    fn token(&self) -> String {
        format!("{}:{}", self.stem, self.epoch)
    }

    /// The file's server part (origin hash) and account.
    fn parts(&self) -> (&str, &str) {
        self.stem.split_once('-').unwrap_or((&self.stem, ""))
    }
}

pub(crate) struct MessageStore {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    writers: HashMap<String, Writer>,
    epochs: HashMap<String, u64>,
    /// sha256 of a recent AUTH token and its account; never the token.
    tokens: VecDeque<([u8; 32], String)>,
    /// Each server's signing key (NIP-11 `self`), by origin hash: only its
    /// notices may forget a room.
    relay_keys: HashMap<String, Arc<str>>,
    /// The capture taken when each subscription last reached EOSE, by
    /// subscription id, until the app asks for it.
    eose: HashMap<String, (u64, String)>,
    /// Orders [`Self::eose`] entries, oldest first.
    eose_noted: u64,
}

/// EOSE captures kept at most; beyond this the oldest are not needed.
const MAX_EOSE_CAPTURES: usize = 512;

/// `https://host[:port]` for any relay URL, so a socket and a query to the
/// same server share one file.
pub(crate) fn http_origin(relay_url: &str) -> Option<String> {
    let url = url::Url::parse(relay_url.trim()).ok()?;
    let scheme = match url.scheme() {
        "wss" | "https" => "https",
        "ws" | "http" => "http",
        _ => return None,
    };
    let host = url.host_str()?.to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

fn origin_hash(origin: &str) -> String {
    Sha256::digest(origin.as_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn file_stem(origin: &str, account: &str) -> String {
    format!("{}-{}", origin_hash(origin), account.to_ascii_lowercase())
}

fn is_pubkey(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The capture a `<scope>:<sequence>` token from [`MessageStore::capture`]
/// names, when its scope is still `lease`'s.
fn captured(token: Option<&str>, lease: &Lease) -> Result<u64, String> {
    token
        .and_then(|token| token.rsplit_once(':'))
        .filter(|(scope, _)| *scope == lease.token())
        .and_then(|(_, sequence)| sequence.parse().ok())
        .ok_or_else(|| "message store scope changed".to_owned())
}

impl MessageStore {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub(crate) fn path_of(&self, lease: &Lease) -> PathBuf {
        self.dir.join(format!("{}.db", lease.stem))
    }

    /// The scope for events received from `relay_url` as `account`.
    pub(crate) fn lease(&self, relay_url: &str, account: &str) -> Option<Lease> {
        let origin = http_origin(relay_url)?;
        let account = account.to_ascii_lowercase();
        if !is_pubkey(&account) {
            return None;
        }
        let stem = file_stem(&origin, &account);
        let epoch = *self.inner.lock().ok()?.epochs.get(&stem).unwrap_or(&0);
        Some(Lease { stem, epoch })
    }

    /// A capture of `lease`'s file now: its scope and the file's sequence.
    /// A span or membership list carrying it is refused once the scope ends,
    /// and for rooms forgotten after it (docs/message-sync.md#forgetting-a-room).
    pub(crate) fn capture(&self, lease: &Lease) -> Option<String> {
        let inner = self.inner.lock().ok()?;
        Some(capture_in(&inner, lease))
    }

    /// Takes the capture of `lease`'s file as subscription `sub` reaches
    /// EOSE, in socket order: what arrived before it has a lower number.
    pub(crate) fn note_eose(&self, lease: &Lease, sub: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let capture = capture_in(&inner, lease);
        if inner.eose.len() >= MAX_EOSE_CAPTURES && !inner.eose.contains_key(sub) {
            let oldest = inner
                .eose
                .iter()
                .min_by_key(|(_, (noted, _))| *noted)
                .map(|(sub, _)| sub.clone());
            if let Some(oldest) = oldest {
                inner.eose.remove(&oldest);
            }
        }
        inner.eose_noted += 1;
        let noted = inner.eose_noted;
        inner.eose.insert(sub.to_owned(), (noted, capture));
    }

    /// The capture taken when `sub` last reached EOSE, once.
    pub(crate) fn take_eose(&self, sub: &str) -> Option<String> {
        self.inner
            .lock()
            .ok()?
            .eose
            .remove(sub)
            .map(|(_, capture)| capture)
    }

    /// Queues a coverage span for the file current for `relay_url` and
    /// `account`, if `scope` (captured at the span's EOSE) is still that file
    /// and sign-in. A span without a scope cannot be placed and is refused too.
    pub(crate) fn extend_coverage(
        &self,
        relay_url: &str,
        account: &str,
        scope: Option<&str>,
        rooms: Vec<String>,
        from: i64,
        to: i64,
    ) -> Result<(), String> {
        let Some(lease) = self.lease(relay_url, account) else {
            return Ok(());
        };
        let captured = captured(scope, &lease)?;
        self.submit(
            &lease,
            Input::Coverage {
                rooms,
                from,
                to,
                captured,
            },
        );
        Ok(())
    }

    /// Applies `keep`, the account's full membership list from a successful
    /// fetch, to the rooms stored for `relay_url` and `account`
    /// (docs/message-sync.md#forgetting-a-room). Refused when `scope`
    /// (captured before that fetch) is no longer current, and for an empty
    /// list.
    pub(crate) fn forget_rooms_except(
        &self,
        relay_url: &str,
        account: &str,
        scope: Option<&str>,
        keep: Vec<String>,
    ) -> Result<(), String> {
        let lease = self
            .lease(relay_url, account)
            .ok_or("message store scope changed")?;
        let captured = captured(scope, &lease)?;
        if keep.is_empty() {
            return Err("an empty membership list is never acted on".into());
        }
        let keep = keep.into_iter().collect();
        self.submit(&lease, Input::ForgetExcept { keep, captured });
        Ok(())
    }

    /// Marks a newest-window request for `rooms` going out on `lease`'s
    /// file: its capture, which [`Self::record_heads`] needs, or `None`
    /// when it could not be queued (docs/message-sync.md#head-spans).
    pub(crate) fn head_sent(&self, lease: &Lease, rooms: Vec<String>) -> Option<String> {
        let mut capture = None;
        self.with_writer(lease, |writer, _| {
            if writer.submit(Input::HeadSent { rooms }) {
                capture = Some(format!("{}:{}", lease.token(), writer.sequence()));
            }
        });
        capture
    }

    /// Records the head spans read from the answer to the request marked
    /// `scope` (an empty list only releases the mark). Refused when `scope`
    /// is no longer current; the writer refuses it too unless it is a mark.
    pub(crate) fn record_heads(
        &self,
        relay_url: &str,
        account: &str,
        scope: Option<&str>,
        heads: Vec<HeadSpan>,
    ) -> Result<(), String> {
        let lease = self
            .lease(relay_url, account)
            .ok_or("message store scope changed")?;
        let captured = captured(scope, &lease)?;
        self.submit(&lease, Input::Heads { heads, captured });
        Ok(())
    }

    /// Notes `relay_url`'s signing key (its NIP-11 `self`).
    pub(crate) fn note_relay_key(&self, relay_url: &str, key: &str) {
        let key = key.to_ascii_lowercase();
        let (Some(origin), true) = (http_origin(relay_url), is_pubkey(&key)) else {
            return;
        };
        if let Ok(mut inner) = self.inner.lock() {
            inner.relay_keys.insert(origin_hash(&origin), key.into());
        }
    }

    /// Hands a socket frame to the writer, with the server's signing key
    /// when known, so its relay notices can be acted on.
    pub(crate) fn submit_frame(&self, lease: &Lease, frame: String) {
        self.with_writer(lease, |writer, relay_key| {
            writer.submit(Input::Frame { frame, relay_key });
        });
    }

    /// Hands received events (a `/query` answer) to the writer, as frames.
    pub(crate) fn submit_events(&self, lease: &Lease, events: Vec<Value>) {
        self.with_writer(lease, |writer, relay_key| {
            writer.submit(Input::Events { events, relay_key });
        });
    }

    /// Hands an input to the writer of `lease`'s file. Never blocks on the
    /// disk; what to keep is decided on the writer thread.
    fn submit(&self, lease: &Lease, input: Input) {
        self.with_writer(lease, |writer, _| {
            writer.submit(input);
        });
    }

    /// Runs `act` on the writer of `lease`'s file, started if needed, with
    /// the server's signing key when known, unless the lease has ended.
    fn with_writer(&self, lease: &Lease, act: impl FnOnce(&Writer, Option<Arc<str>>)) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if *inner.epochs.get(&lease.stem).unwrap_or(&0) != lease.epoch {
            return;
        }
        let relay_key = inner.relay_keys.get(lease.parts().0).cloned();
        let path = self.path_of(lease);
        let account = lease.parts().1.to_owned();
        let writer = inner
            .writers
            .entry(lease.stem.clone())
            .or_insert_with(|| Writer::start(path, account));
        act(writer, relay_key);
    }

    fn note_token(&self, token: &str, account: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.tokens.len() >= TOKENS_KEPT {
            inner.tokens.pop_front();
        }
        inner
            .tokens
            .push_back((Sha256::digest(token.as_bytes()).into(), account.to_owned()));
    }

    fn token_account(&self, token: &str) -> Option<String> {
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        let inner = self.inner.lock().ok()?;
        inner
            .tokens
            .iter()
            .rev()
            .find(|(kept, _)| *kept == hash)
            .map(|(_, account)| account.clone())
    }

    /// Runs `read` on a read-only connection to `lease`'s file; `None` when
    /// there is no file yet or it fails.
    fn read<T>(
        &self,
        lease: &Lease,
        what: &str,
        read: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> Option<T> {
        let path = self.path_of(lease);
        if !path.exists() {
            return None;
        }
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        rusqlite::Connection::open_with_flags(&path, flags)
            .and_then(|conn| {
                conn.busy_timeout(std::time::Duration::from_secs(5))?;
                read(&conn)
            })
            .map_err(|error| eprintln!("kuzz-desktop: message store: {what} failed: {error}"))
            .ok()
    }

    pub(crate) fn resume_points(
        &self,
        lease: &Lease,
        rooms: &[String],
        before: i64,
    ) -> HashMap<String, i64> {
        self.read(lease, "resume points", |conn| {
            coverage::resume_points(conn, rooms, before)
        })
        .unwrap_or_default()
    }

    /// Whether everything received for `lease` so far is written and nothing
    /// was lost (docs/message-sync.md#opening-a-room-from-the-store). A file
    /// with no writer has nothing queued.
    fn drained(&self, lease: &Lease) -> bool {
        let answer = {
            let Ok(inner) = self.inner.lock() else {
                return false;
            };
            match inner.writers.get(&lease.stem) {
                None => return true,
                Some(writer) => writer.barrier(),
            }
        };
        answer.is_some_and(|answer| answer.recv_timeout(DRAIN_WAIT).unwrap_or(false))
    }

    pub(crate) fn room_head(
        &self,
        lease: &Lease,
        room: &str,
        kinds: &[u64],
        limit: usize,
        live_at: Option<i64>,
    ) -> read::RoomHead {
        if !self.drained(lease) {
            return read::RoomHead::default();
        }
        self.read(lease, "room head", |conn| {
            read::room_head(conn, room, kinds, limit, live_at)
        })
        .unwrap_or_default()
    }

    /// The newest windows of `rooms` (each with its `live_at`), after one
    /// drain and in one read.
    pub(crate) fn room_heads(
        &self,
        lease: &Lease,
        rooms: &[(String, i64)],
        kinds: &[u64],
        limit: usize,
    ) -> Vec<(String, read::RoomHead)> {
        if !self.drained(lease) {
            return Vec::new();
        }
        self.read(lease, "room heads", |conn| {
            rooms
                .iter()
                .map(|(room, live_at)| {
                    read::room_head(conn, room, kinds, limit, Some(*live_at))
                        .map(|head| (room.clone(), head))
                })
                .collect()
        })
        .unwrap_or_default()
    }

    pub(crate) fn thread_head(
        &self,
        lease: &Lease,
        room: &str,
        root: &str,
        kinds: &[u64],
        limit: usize,
        live_at: Option<i64>,
    ) -> read_thread::ThreadHead {
        if !self.drained(lease) {
            return read_thread::ThreadHead::default();
        }
        self.read(lease, "thread head", |conn| {
            read_thread::thread_head(conn, room, root, kinds, limit, live_at)
        })
        .unwrap_or_default()
    }

    /// Ends every lease of `account` on `relay_url`, stops that file's
    /// writer and deletes the file. The account's files from other servers
    /// are kept and keep recording.
    pub(crate) fn forget(&self, relay_url: &str, account: &str) {
        let Some(origin) = http_origin(relay_url) else {
            return;
        };
        let stem = file_stem(&origin, account);
        let stopping = {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            *inner.epochs.entry(stem.clone()).or_insert(0) += 1;
            inner.writers.remove(&stem)
        };
        if let Some(writer) = stopping {
            writer.stop();
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.split('.').next() == Some(stem.as_str()) {
                if let Err(error) = std::fs::remove_file(entry.path()) {
                    eprintln!("kuzz-desktop: message store: could not delete {name}: {error}");
                }
            }
        }
    }
}

/// Whether `frame` is a `["<label>", ...]` message, without parsing it.
fn capture_in(inner: &Inner, lease: &Lease) -> String {
    let sequence = inner.writers.get(&lease.stem).map_or(0, Writer::sequence);
    format!("{}:{sequence}", lease.token())
}

fn labelled(frame: &str, label: &str) -> bool {
    frame
        .trim_start()
        .strip_prefix('[')
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('"'))
        .and_then(|rest| rest.strip_prefix(label))
        .is_some_and(|rest| rest.starts_with('"'))
}

/// Watches one relay socket for the store: the account is the author of the
/// socket's `AUTH` reply, and events received after it are kept.
pub(crate) struct SocketTap {
    url: String,
    lease: Option<Lease>,
}

impl SocketTap {
    pub(crate) fn new(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            lease: None,
        }
    }

    /// A frame the app sends. `["AUTH", event]` names the account by the
    /// event's author, `["AUTH", {"token"}]` by the token's account.
    pub(crate) fn sent(&mut self, frame: &str) {
        if !labelled(frame, "AUTH") {
            return;
        }
        let (Ok(Value::Array(parts)), Some(store)) =
            (serde_json::from_str::<Value>(frame), global())
        else {
            return;
        };
        let Some(payload) = parts.get(1) else { return };
        let account = match (payload.get("pubkey"), payload.get("token")) {
            (Some(Value::String(pubkey)), _) => Some(pubkey.clone()),
            (_, Some(Value::String(token))) => store.token_account(token),
            _ => None,
        };
        self.lease = account.and_then(|account| store.lease(&self.url, &account));
    }

    /// A frame the relay sent. `["EVENT", sub, event]` is kept;
    /// `["EOSE", sub]` takes the capture its coverage is recorded with.
    pub(crate) fn received(&self, frame: &str) {
        let (Some(lease), Some(store)) = (&self.lease, global()) else {
            return;
        };
        if labelled(frame, "EOSE") {
            if let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(frame) {
                if let Some(Value::String(sub)) = parts.get(1) {
                    store.note_eose(lease, sub);
                }
            }
            return;
        }
        if !labelled(frame, "EVENT") {
            return;
        }
        store.submit_frame(lease, frame.to_owned());
    }
}

/// Records that every event of `rooms` in `[from, to]` received from
/// `relay_url` as `account` is stored. Ordered after everything received so
/// far, and dropped if any of it was lost.
pub(crate) fn extend_coverage(
    relay_url: &str,
    account: &str,
    scope: Option<&str>,
    rooms: Vec<String>,
    from: i64,
    to: i64,
) -> Result<(), String> {
    let Some(store) = global() else { return Ok(()) };
    store.extend_coverage(relay_url, account, scope, rooms, from, to)
}

/// Applies `keep`, a membership list fetched from capture `scope` on, to
/// the rooms stored for `relay_url` and `account`.
pub(crate) fn forget_rooms_except(
    relay_url: &str,
    account: &str,
    scope: Option<&str>,
    keep: Vec<String>,
) -> Result<(), String> {
    let Some(store) = global() else { return Ok(()) };
    store.forget_rooms_except(relay_url, account, scope, keep)
}

/// Marks a newest-window request for `rooms` to `relay_url` as `account`
/// going out; its capture, or `None` (nothing will be recorded).
pub(crate) fn head_sent(relay_url: &str, account: &str, rooms: Vec<String>) -> Option<String> {
    let store = global()?;
    let lease = store.lease(relay_url, account)?;
    store.head_sent(&lease, rooms)
}

/// Records the head spans of the request marked `scope`.
pub(crate) fn record_heads(
    relay_url: &str,
    account: &str,
    scope: Option<&str>,
    heads: Vec<HeadSpan>,
) -> Result<(), String> {
    let Some(store) = global() else { return Ok(()) };
    store.record_heads(relay_url, account, scope, heads)
}

/// Notes `relay_url`'s signing key (NIP-11 `self`): relay notices signed
/// by it may forget a room; none are trusted before it is known.
pub(crate) fn note_relay_key(relay_url: &str, key: &str) {
    if let Some(store) = global() {
        store.note_relay_key(relay_url, key);
    }
}

/// A capture of the file and sign-in for `relay_url` and `account`, taken
/// when a live REQ reached EOSE or a membership fetch starts: a span or list
/// carrying it is refused after an account or server switch or a sign-out,
/// and for rooms forgotten after it.
pub(crate) fn scope_token(relay_url: &str, account: &str) -> Option<String> {
    let store = global()?;
    store.capture(&store.lease(relay_url, account)?)
}

/// The capture subscription `sub` took at its EOSE on the socket, or one
/// taken now when there is none (a span from another scope is refused when
/// it is recorded).
pub(crate) fn eose_scope_token(relay_url: &str, account: &str, sub: &str) -> Option<String> {
    global()?
        .take_eose(sub)
        .or_else(|| scope_token(relay_url, account))
}

/// The end of each room's latest coverage span begun before `before`.
pub(crate) fn resume_points(
    relay_url: &str,
    account: &str,
    rooms: &[String],
    before: i64,
) -> HashMap<String, i64> {
    let Some(store) = global() else {
        return HashMap::new();
    };
    let Some(lease) = store.lease(relay_url, account) else {
        return HashMap::new();
    };
    store.resume_points(&lease, rooms, before)
}

/// `room`'s newest window from the store as a channel-window response, only
/// when it is certain (docs/message-sync.md#opening-a-room-from-the-store).
pub(crate) fn certain_room_head(
    relay_url: &str,
    account: &str,
    room: &str,
    kinds: &[u64],
    limit: usize,
    live_at: i64,
) -> Option<Vec<Value>> {
    if !clock::clock_trusted() {
        return None;
    }
    let store = global()?;
    let lease = store.lease(relay_url, account)?;
    let head = store.room_head(&lease, room, kinds, limit, Some(live_at));
    head.certain.then(|| head.into_response(room))
}

/// Of `rooms` (each with when its live REQ reached EOSE), those whose newest
/// window is certain, each with that window as a channel-window response
/// (docs/message-sync.md#store-first-preload).
pub(crate) fn certain_room_heads(
    relay_url: &str,
    account: &str,
    rooms: &[(String, i64)],
    kinds: &[u64],
    limit: usize,
) -> Vec<(String, Vec<Value>)> {
    if !clock::clock_trusted() {
        return Vec::new();
    }
    let Some(store) = global() else {
        return Vec::new();
    };
    let Some(lease) = store.lease(relay_url, account) else {
        return Vec::new();
    };
    store
        .room_heads(&lease, rooms, kinds, limit)
        .into_iter()
        .filter(|(_, head)| head.certain)
        .map(|(room, head)| {
            let window = head.into_response(&room);
            (room, window)
        })
        .collect()
}

/// `root`'s newest replies in `room` from the store, only when certain:
/// `(rows newest first, next older cursor when more remain)`.
pub(crate) fn certain_thread_head(
    relay_url: &str,
    account: &str,
    room: &str,
    root: &str,
    kinds: &[u64],
    limit: usize,
    live_at: i64,
) -> Option<(Vec<Value>, Option<(i64, String)>)> {
    if !clock::clock_trusted() {
        return None;
    }
    let store = global()?;
    let lease = store.lease(relay_url, account)?;
    let head = store.thread_head(&lease, room, root, kinds, limit, Some(live_at));
    head.certain.then_some((head.rows, head.next_cursor))
}

/// The scope of a `/query` to `api_base_url` as `account`, taken before it
/// is sent.
pub(crate) fn query_lease(api_base_url: &str, account: &str) -> Option<Lease> {
    global()?.lease(api_base_url, account)
}

/// Keeps the events a `/query` returned under `lease`.
pub(crate) fn query_received(lease: Option<Lease>, events: &[nostr::Event]) {
    let (Some(lease), Some(store)) = (lease, global()) else {
        return;
    };
    if events.is_empty() {
        return; // starts no writer and creates no file
    }
    let values: Vec<Value> = events
        .iter()
        .filter_map(|event| serde_json::to_value(event).ok())
        .collect();
    store.submit_events(&lease, values);
}
