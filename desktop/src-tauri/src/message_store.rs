//! Received message originals, isolated by relay and account. This stage writes
//! alongside existing UI delivery; it never claims complete history coverage.
use crate::app_state::AppState;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Manager, State};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Lease {
    relay: String,
    account: String,
    epoch: u64,
}

#[derive(Default)]
struct Inner {
    epochs: HashMap<(String, String), u64>,
    last_error: Option<String>,
    removed_relays: HashSet<String>,
    pending_events: usize,
    pending_bytes: usize,
}

/// Serializes writes and deletion, including their generation checks.
#[derive(Clone, Default)]
pub(crate) struct MessageStore(Arc<Mutex<Inner>>, Arc<Mutex<()>>);

fn supported(kind: u64) -> bool {
    matches!(kind, 5 | 7 | 9 | 9005 | 39005 | 40001 | 40002 | 40003 | 40008 | 40099 |
        43001..=43006 | 45001 | 45003 | 48100..=48103)
}

fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "secure_delete", true)
        .map_err(|e| e.to_string())?;
    let version: u32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version > 1 {
        return Err("Unsupported message store schema; originals preserved".into());
    }
    conn.execute_batch("BEGIN;
      CREATE TABLE IF NOT EXISTS scopes(relay TEXT NOT NULL, account TEXT NOT NULL, needs_repair INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(relay,account));
      CREATE TABLE IF NOT EXISTS events(relay TEXT NOT NULL, account TEXT NOT NULL, id TEXT NOT NULL, kind INTEGER NOT NULL, created_at INTEGER NOT NULL, raw_json TEXT NOT NULL, PRIMARY KEY(relay,account,id));
      CREATE TABLE IF NOT EXISTS event_tags(relay TEXT NOT NULL, account TEXT NOT NULL, id TEXT NOT NULL, tag TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(relay,account,id,tag,value));
      CREATE INDEX IF NOT EXISTS event_tag_target ON event_tags(relay,account,tag,value);
      CREATE INDEX IF NOT EXISTS event_order ON events(relay,account,created_at DESC,id);
      PRAGMA user_version=1; COMMIT;").map_err(|e| e.to_string())?;
    Ok(conn)
}

impl MessageStore {
    fn reserve(&self, count: usize, bytes: usize) -> Result<(), String> {
        let mut inner = self.0.lock().map_err(|e| e.to_string())?;
        if count > 4096usize.saturating_sub(inner.pending_events)
            || bytes > (8 * 1024 * 1024usize).saturating_sub(inner.pending_bytes)
        {
            let error = "Message store queue full; history repair required".to_string();
            inner.last_error = Some(error.clone());
            return Err(error);
        }
        inner.pending_events += count;
        inner.pending_bytes += bytes;
        Ok(())
    }

    fn lease(&self, relay: &str, account: &str) -> Result<Lease, String> {
        let mut inner = self.0.lock().map_err(|e| e.to_string())?;
        let key = (crate::auth::origin_for(relay), account.to_ascii_lowercase());
        if inner.removed_relays.contains(&key.0) {
            return Err("Relay removed from message store".into());
        }
        let epoch = *inner.epochs.entry(key.clone()).or_default();
        Ok(Lease {
            epoch,
            relay: key.0,
            account: key.1,
        })
    }

    fn put(&self, path: &Path, lease: &Lease, events: &[Value]) -> Result<(), String> {
        if events.len() > 256 {
            return Err("Message store batch exceeds 256".into());
        }
        let _writer = self.1.lock().map_err(|e| e.to_string())?;
        let inner = self.0.lock().map_err(|e| e.to_string())?;
        if inner.removed_relays.contains(&lease.relay) {
            return Ok(());
        }
        if *inner
            .epochs
            .get(&(lease.relay.clone(), lease.account.clone()))
            .unwrap_or(&0)
            != lease.epoch
        {
            return Ok(());
        }
        drop(inner);
        let mut conn = open(path)?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO scopes(relay,account) VALUES(?1,?2)",
            params![lease.relay, lease.account],
        )
        .map_err(|e| e.to_string())?;
        for event in events {
            let Some(kind) = event["kind"].as_u64().filter(|kind| supported(*kind)) else {
                continue;
            };
            let id = event["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("Missing event id")?;
            let created_at = event["created_at"].as_i64().ok_or("Missing event time")?;
            let tags = event["tags"].as_array().ok_or("Missing event tags")?;
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO events VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        lease.relay,
                        lease.account,
                        id,
                        kind,
                        created_at,
                        event.to_string()
                    ],
                )
                .map_err(|e| e.to_string())?;
            if inserted == 0 {
                continue;
            }
            for tag in tags {
                let (Some(name), Some(value)) = (
                    tag.get(0).and_then(Value::as_str),
                    tag.get(1).and_then(Value::as_str),
                ) else {
                    continue;
                };
                if !matches!(name, "h" | "e" | "a") {
                    continue;
                }
                tx.execute(
                    "INSERT OR IGNORE INTO event_tags VALUES(?1,?2,?3,?4,?5)",
                    params![lease.relay, lease.account, id, name, value],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    fn clear(&self, path: &Path, relay: &str, account: &str) -> Result<(), String> {
        let _writer = self.1.lock().map_err(|e| e.to_string())?;
        let mut inner = self.0.lock().map_err(|e| e.to_string())?;
        let relay = crate::auth::origin_for(relay);
        let account = account.to_ascii_lowercase();
        *inner
            .epochs
            .entry((relay.clone(), account.clone()))
            .or_default() += 1;
        drop(inner);
        let mut conn = open(path)?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for table in ["event_tags", "events", "scopes"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE relay=?1 AND account=?2"),
                params![relay, account],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|e| e.to_string())
    }
}

fn path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("messages.db"))
}

/// Capture before network I/O; only the current signed-in user's requests enter
/// this store (not agent credentials or background requests to another relay).
pub(crate) fn capture(state: &AppState, relay: &str, account: &str) -> Option<(AppHandle, Lease)> {
    let app = state.app_handle.lock().ok()?.clone()?;
    // Mint the generation before checking auth: sign-out between the two
    // either invalidates this lease or makes the following check reject it.
    let lease = app.state::<MessageStore>().lease(relay, account).ok()?;
    let credential = state.user_credential().ok()?;
    if credential.public_key().to_hex() != account
        || state.current_auth_origin() != crate::auth::origin_for(relay)
    {
        return None;
    }
    Some((app, lease))
}

/// Queue bounded background writes without delaying the HTTP response.
/// Missing or failed batches never advance coverage: every scope is needs_repair.
pub(crate) async fn received(context: Option<(AppHandle, Lease)>, events: Vec<Value>) {
    let Some((app, lease)) = context else {
        return;
    };
    let events: Vec<_> = events
        .into_iter()
        .filter(|event| event["kind"].as_u64().is_some_and(supported))
        .collect();
    if events.is_empty() {
        return;
    }
    let store = app.state::<MessageStore>().inner().clone();
    let count = events.len();
    let bytes = events
        .iter()
        .map(|event| event.to_string().len() + 512)
        .sum();
    if let Err(error) = store.reserve(count, bytes) {
        eprintln!("message-store: persistence incomplete: {error}");
        return;
    }
    tauri::async_runtime::spawn(async move {
        let result = persist(app, lease, events).await;
        if let Ok(mut inner) = store.0.lock() {
            inner.pending_events -= count;
            inner.pending_bytes -= bytes;
            if let Err(error) = &result {
                inner.last_error = Some(error.clone());
            }
        }
        if let Err(error) = result {
            eprintln!("message-store: persistence incomplete: {error}");
        }
    });
}

async fn persist(app: AppHandle, lease: Lease, events: Vec<Value>) -> Result<(), String> {
    let store = app.state::<MessageStore>().inner().clone();
    let path = path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        for batch in events.chunks(256) {
            store.put(&path, &lease, batch)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Begin a renderer receive context using the backend's authenticated scope.
#[tauri::command]
pub(crate) fn message_store_begin(state: State<'_, AppState>) -> Result<Lease, String> {
    let account = state.user_credential()?.public_key().to_hex();
    capture(&state, &state.current_auth_origin(), &account)
        .map(|(_, lease)| lease)
        .ok_or_else(|| "Message store receive context retired".into())
}

/// Write an already received live batch using its original receive context.
#[tauri::command]
pub(crate) async fn message_store_put(
    app: AppHandle,
    lease: Lease,
    events: Vec<Value>,
) -> Result<(), String> {
    if events.len() > 256 {
        return Err("Message store batch exceeds 256".into());
    }
    persist(app, lease, events).await
}

/// Delete one account/community, fencing all its existing receive contexts.
#[tauri::command]
pub(crate) async fn message_store_clear(
    app: AppHandle,
    relay: String,
    account: String,
) -> Result<(), String> {
    let store = app.state::<MessageStore>().inner().clone();
    let path = path(&app)?;
    tauri::async_runtime::spawn_blocking(move || store.clear(&path, &relay, &account))
        .await
        .map_err(|e| e.to_string())?
}

/// Clear all accounts on a removed relay under one write fence.
pub(crate) async fn clear_relay(app: AppHandle, relay: String) -> Result<(), String> {
    let store = app.state::<MessageStore>().inner().clone();
    let path = path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _writer = store.1.lock().map_err(|e| e.to_string())?;
        let relay = crate::auth::origin_for(&relay);
        let mut inner = store.0.lock().map_err(|e| e.to_string())?;
        inner.removed_relays.insert(relay.clone());
        for ((r, _), epoch) in &mut inner.epochs {
            if r == &relay {
                *epoch += 1;
            }
        }
        drop(inner);
        let mut conn = open(&path)?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for table in ["event_tags", "events", "scopes"] {
            tx.execute(&format!("DELETE FROM {table} WHERE relay=?1"), [&relay])
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Explicit community re-add allows new leases, never previously retired leases.
pub(crate) fn readd_relay(app: &AppHandle, relay: &str) -> Result<(), String> {
    app.state::<MessageStore>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .removed_relays
        .remove(&crate::auth::origin_for(relay));
    Ok(())
}

#[cfg(test)]
#[path = "message_store_tests.rs"]
mod tests;
