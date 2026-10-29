//! Device message store: every received room event, kept per server and
//! account in its own SQLite file. Capture only so far — nothing reads it
//! yet. Rules: docs/message-sync.md#message-store.

mod classify;
#[cfg(test)]
mod tests;
mod writer;

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

#[cfg(test)]
pub(crate) use classify::{classify, Filed};
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
pub(crate) fn forget_account(account: &str) {
    if let Some(store) = global() {
        store.forget(account);
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

/// The scope a receive belongs to, taken when the request or socket starts.
/// A lease from before the account signed out writes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lease {
    stem: String,
    account: String,
    epoch: u64,
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
}

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

fn file_stem(origin: &str, account: &str) -> String {
    let digest = Sha256::digest(origin.as_bytes());
    let hex: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{hex}-{}", account.to_ascii_lowercase())
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
        if account.len() != 64 || !account.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let epoch = *self.inner.lock().ok()?.epochs.get(&account).unwrap_or(&0);
        Some(Lease {
            stem: file_stem(&origin, &account),
            account,
            epoch,
        })
    }

    /// Hands a receive to the writer of `lease`'s file. Never blocks on the
    /// disk; what to keep is decided on the writer thread.
    fn submit(&self, lease: &Lease, input: Input) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if *inner.epochs.get(&lease.account).unwrap_or(&0) != lease.epoch {
            return;
        }
        let path = self.path_of(lease);
        inner
            .writers
            .entry(lease.stem.clone())
            .or_insert_with(|| Writer::start(path))
            .submit(input);
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

    /// Ends every lease of `account`, stops its writers and deletes its files.
    pub(crate) fn forget(&self, account: &str) {
        let account = account.to_ascii_lowercase();
        let suffix = format!("-{account}");
        let stopping: Vec<Writer> = {
            let Ok(mut inner) = self.inner.lock() else {
                return;
            };
            *inner.epochs.entry(account.clone()).or_insert(0) += 1;
            let stems: Vec<String> = inner
                .writers
                .keys()
                .filter(|stem| stem.ends_with(&suffix))
                .cloned()
                .collect();
            stems
                .iter()
                .filter_map(|stem| inner.writers.remove(stem))
                .collect()
        };
        for writer in stopping {
            writer.stop();
        }
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.split('.').next().unwrap_or_default();
            if stem.ends_with(&suffix) {
                if let Err(error) = std::fs::remove_file(entry.path()) {
                    eprintln!("kuzz-desktop: message store: could not delete {name}: {error}");
                }
            }
        }
    }
}

/// Whether `frame` is a `["<label>", ...]` message, without parsing it.
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

    /// A frame the relay sent. `["EVENT", sub, event]` is kept.
    pub(crate) fn received(&self, frame: &str) {
        let (Some(lease), Some(store)) = (&self.lease, global()) else {
            return;
        };
        if !labelled(frame, "EVENT") {
            return;
        }
        store.submit(lease, Input::Frame(frame.to_owned()));
    }
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
    let values: Vec<Value> = events
        .iter()
        .filter_map(|event| serde_json::to_value(event).ok())
        .collect();
    store.submit(&lease, Input::Events(values));
}
