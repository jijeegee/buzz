//! Permanent local message store: one SQLite file per account + server.
//!
//! Every scope (`pubkey` on one server origin) gets its own file under
//! `app_data_dir/message-store/` and one actor: a dedicated thread owning a
//! single long-lived WAL connection, so reads and writes for a scope run in
//! the order they were sent. Coverage ranges (see [`coverage`]) say which
//! parts of a room are provably complete locally; only explicit fetch claims
//! write them, never an event that merely arrives.

mod classify;
pub(crate) mod commands;
mod coverage;
mod ingest;
pub(crate) mod live;
mod read;
pub(crate) mod record;
mod schema;
pub(crate) mod sync;
#[cfg(test)]
mod tests;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex, PoisonError},
    thread::JoinHandle,
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime};

/// Every kind the store keeps for a room: window rows, reactions, edits,
/// deletes, legacy messages, forum posts, job and huddle lifecycle events.
/// The one list the window-adjacent repair, catch-up and live filters share
/// (`CHANNEL_SYNC_KINDS` in `kinds.ts`, checked against
/// `testdata/sync_kinds.json`); bump [`SYNC_KINDS_VERSION`] whenever it
/// changes so coverage proven under the old list is ignored.
pub(crate) const SYNC_KINDS: [u32; 21] = [
    5, 7, 9, 9005, 40001, 40002, 40003, 40008, 40099, 43001, 43002, 43003, 43004, 43005, 43006,
    45001, 45003, 48100, 48101, 48102, 48103,
];
pub(crate) const SYNC_KINDS_VERSION: i64 = 1;

/// Kinds a room window renders as rows (mirrors the channel-window fetch).
pub(crate) const WINDOW_ROW_KINDS: [u32; 11] = [
    9, 40002, 40008, 40099, 43001, 43002, 43003, 43004, 43005, 43006, 48100,
];

const STORE_DIR: &str = "message-store";
const FILE_SUFFIX: &str = ".sqlite3";

/// One account on one server, as the webview names it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoreScope {
    pub(crate) pubkey: String,
    pub(crate) relay_url: String,
}

impl StoreScope {
    fn file_stem(&self) -> Result<String, String> {
        file_stem(&self.pubkey, &self.relay_url)
    }
}

/// `{pubkey_hex}@{sha256(origin)[..16]}` for a pubkey and relay or HTTP URL.
fn file_stem(pubkey: &str, relay_url: &str) -> Result<String, String> {
    let pubkey = pubkey.trim().to_ascii_lowercase();
    if !is_hex64(&pubkey) {
        return Err("invalid message store scope".to_string());
    }
    let origin = crate::auth::origin_for(relay_url);
    let digest = hex::encode(Sha256::digest(origin.as_bytes()));
    Ok(format!("{pubkey}@{}", &digest[..16]))
}

pub(crate) fn is_hex64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn store_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(STORE_DIR))
        .map_err(|error| format!("resolve message store dir: {error}"))
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// The thread owning one scope's connection.
struct Actor {
    tx: Mutex<Option<mpsc::Sender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Actor {
    fn spawn(path: PathBuf, stem: String) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name(format!("message-store-{}", &stem[..8]))
            .spawn(move || {
                let mut conn = match schema::open(&path, &stem) {
                    Ok(conn) => conn,
                    Err(error) => {
                        // Queued jobs are dropped; their callers see an error
                        // and the next call opens again.
                        eprintln!("kuzz-desktop: message store: open failed: {error}");
                        return;
                    }
                };
                if let Err(error) = ingest::recover_open_segments(&mut conn) {
                    eprintln!("kuzz-desktop: message store: segment recovery failed: {error}");
                }
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
                let _ = conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
            })
            .map_err(|error| format!("start message store thread: {error}"))?;
        Ok(Self {
            tx: Mutex::new(Some(tx)),
            thread: Mutex::new(Some(thread)),
        })
    }

    fn send(&self, job: Job) -> bool {
        match self
            .tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            Some(tx) => tx.send(job).is_ok(),
            None => false,
        }
    }

    /// Lets queued jobs finish, then closes the connection. Blocking.
    fn shut_down(&self) {
        self.tx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// Fire-and-forget access to one scope's thread, pinned to the actor that was
/// open when it was made: once the scope is purged its jobs are dropped, so a
/// late write can never recreate a deleted account's file.
#[derive(Clone)]
pub(crate) struct ScopeWriter {
    actor: Arc<Actor>,
}

impl ScopeWriter {
    /// Queues `f` after everything sent before it. Failures are logged only:
    /// the store never fails the read or socket that feeds it.
    pub(crate) fn submit<F>(&self, f: F)
    where
        F: FnOnce(&mut Connection) -> Result<(), String> + Send + 'static,
    {
        let job: Job = Box::new(move |conn| {
            if let Err(error) = f(conn) {
                eprintln!("kuzz-desktop: message store: write failed: {error}");
            }
        });
        let _ = self.actor.send(job);
    }

    /// Runs `f` on the pinned actor's thread after everything sent before it.
    /// Errors once the scope was purged.
    pub(crate) async fn call<T, F>(&self, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
    {
        let (reply, result) = tokio::sync::oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let _ = reply.send(f(conn));
        });
        if !self.actor.send(job) {
            return Err("message store unavailable".to_string());
        }
        result
            .await
            .unwrap_or_else(|_| Err("message store unavailable".to_string()))
    }
}

/// Tauri state: the open scopes.
#[derive(Default)]
pub(crate) struct MessageStore {
    actors: Mutex<HashMap<String, Arc<Actor>>>,
}

impl MessageStore {
    fn actor(&self, dir: &Path, stem: &str) -> Result<Arc<Actor>, String> {
        let mut actors = self.actors.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(actor) = actors.get(stem) {
            return Ok(Arc::clone(actor));
        }
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("create message store dir: {error}"))?;
        let path = dir.join(format!("{stem}{FILE_SUFFIX}"));
        let actor = Arc::new(Actor::spawn(path, stem.to_string())?);
        actors.insert(stem.to_string(), Arc::clone(&actor));
        Ok(actor)
    }

    fn forget(&self, stem: &str, actor: &Arc<Actor>) {
        let mut actors = self.actors.lock().unwrap_or_else(PoisonError::into_inner);
        if actors
            .get(stem)
            .is_some_and(|held| Arc::ptr_eq(held, actor))
        {
            actors.remove(stem);
        }
    }

    /// A writer for `scope`, opening it if needed.
    pub(crate) fn writer(&self, dir: &Path, scope: &StoreScope) -> Result<ScopeWriter, String> {
        let stem = scope.file_stem()?;
        Ok(ScopeWriter {
            actor: self.actor(dir, &stem)?,
        })
    }

    /// Runs `f` on the scope's thread, after everything sent before it.
    pub(crate) async fn call<T, F>(&self, dir: &Path, scope: &StoreScope, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, String> + Send + 'static,
    {
        let stem = scope.file_stem()?;
        let actor = self.actor(dir, &stem)?;
        let (reply, result) = tokio::sync::oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let _ = reply.send(f(conn));
        });
        if actor.send(job) {
            if let Ok(result) = result.await {
                return result;
            }
        }
        self.forget(&stem, &actor);
        Err("message store unavailable".to_string())
    }

    /// Closes the scope's connection and deletes its files. Blocking.
    fn purge(&self, dir: &Path, stem: &str) -> Result<(), String> {
        let actor = self
            .actors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(stem);
        if let Some(actor) = actor {
            actor.shut_down();
        }
        for suffix in ["", "-wal", "-shm", ".bad"] {
            let path = dir.join(format!("{stem}{FILE_SUFFIX}{suffix}"));
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("delete message store file: {error}")),
            }
        }
        Ok(())
    }

    pub(crate) fn shut_down_all(&self) {
        let actors: Vec<Arc<Actor>> = self
            .actors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, actor)| actor)
            .collect();
        for actor in actors {
            actor.shut_down();
        }
    }
}

/// [`MessageStore::writer`] for the app's store. `None` (logged) when the
/// store cannot open, so callers carry on without it.
pub(crate) fn writer_for<R: Runtime>(
    app: &AppHandle<R>,
    scope: &StoreScope,
) -> Option<ScopeWriter> {
    let result = store_dir(app).and_then(|dir| match app.try_state::<MessageStore>() {
        Some(store) => store.writer(&dir, scope),
        None => Err("message store not managed".to_string()),
    });
    match result {
        Ok(writer) => Some(writer),
        Err(error) => {
            eprintln!("kuzz-desktop: message store: no writer: {error}");
            None
        }
    }
}

async fn purge_blocking(app: &AppHandle, stem: String) -> Result<(), String> {
    let dir = store_dir(app)?;
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || app.state::<MessageStore>().purge(&dir, &stem))
        .await
        .map_err(|error| format!("message store purge task failed: {error}"))?
}

impl MessageStore {
    /// Deletes `account`'s stored messages on `origin`. Blocking.
    pub(crate) fn purge_account_in(
        &self,
        dir: &Path,
        origin: &str,
        account: &nostr::PublicKey,
    ) -> Result<(), String> {
        self.purge(dir, &file_stem(&account.to_hex(), origin)?)
    }
}

/// A sign-out the user asked for: the account's stored messages on `origin`
/// leave this computer with its key. Best effort; launch cleanup retries
/// orphans. Callers: `auth::user_sign_out` only.
pub(crate) async fn purge_account(app: &AppHandle, origin: &str, account: nostr::PublicKey) {
    let result = match store_dir(app) {
        Ok(dir) => {
            let (app, origin) = (app.clone(), origin.to_string());
            tauri::async_runtime::spawn_blocking(move || {
                app.state::<MessageStore>()
                    .purge_account_in(&dir, &origin, &account)
            })
            .await
            .map_err(|error| format!("message store purge task failed: {error}"))
            .and_then(|result| result)
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("kuzz-desktop: message store: sign-out purge failed: {error}");
    }
}

/// Launch: delete store files of accounts no longer listed on this computer.
/// Runs before any scope opens. Skipped when the account list is unreadable.
pub(crate) fn remove_orphans(data_dir: &Path) {
    let Ok(Some(registry)) = crate::auth::accounts::load_registry(data_dir) else {
        return;
    };
    let listed: HashSet<String> = registry
        .accounts
        .iter()
        .map(|account| account.pubkey.as_str())
        .chain(registry.active.as_deref())
        .map(str::to_ascii_lowercase)
        .collect();
    remove_orphans_in(&data_dir.join(STORE_DIR), &listed);
}

fn remove_orphans_in(dir: &Path, listed: &HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((pubkey, _)) = name.split_once('@') else {
            continue;
        };
        if !is_hex64(pubkey) || listed.contains(&pubkey.to_ascii_lowercase()) {
            continue;
        }
        if let Err(error) = std::fs::remove_file(entry.path()) {
            eprintln!("kuzz-desktop: message store: orphan not removed: {error}");
        }
    }
}

/// App exit: finish queued work and close every scope.
pub(crate) fn shut_down(app: &AppHandle) {
    app.state::<MessageStore>().shut_down_all();
}
