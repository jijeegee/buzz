//! Short-lived Redis state for the `/auth/*` flows: OIDC pending logins,
//! one-time login codes, and the 10-second refresh/exchange replay caches.
//!
//! All keys live under `kuzz:auth:` with a TTL; every read that consumes a
//! one-time value uses `GETDEL` so two requests can never both redeem it.
//!
//! With the region link on, a region that is not the primary keeps this
//! state in the primary's Redis (S4-7), so a code taken in one region cannot
//! be taken again in another.

use crate::region_shared::{self, Op};
use crate::state::AppState;

/// Redis error surfaced to handlers as "unavailable".
#[derive(Debug, thiserror::Error)]
#[error("auth state store unavailable: {0}")]
pub(crate) struct KvError(String);

fn key(namespace: &str, id: &str) -> String {
    format!("kuzz:auth:{namespace}:{id}")
}

/// Run `op` at the primary; `None` when this region keeps it itself.
async fn shared(
    state: &AppState,
    op: impl FnOnce() -> Op,
) -> Option<Result<serde_json::Value, KvError>> {
    if !region_shared::forwards(state) {
        return None;
    }
    Some(
        region_shared::at_primary(state, &op())
            .await
            .map_err(|e| KvError(e.to_string())),
    )
}

/// `SET key value NX EX ttl`; `false` when the key already existed.
pub(crate) async fn put_new(
    state: &AppState,
    namespace: &str,
    id: &str,
    value: &str,
    ttl_secs: u64,
) -> Result<bool, KvError> {
    let op = || Op::PutNew {
        key: key(namespace, id),
        value: value.to_string(),
        ttl: ttl_secs,
    };
    if let Some(answer) = shared(state, op).await {
        return region_shared::was_set(&answer?).map_err(|e| KvError(e.to_string()));
    }
    let mut conn = state
        .redis_pool
        .get()
        .await
        .map_err(|e| KvError(e.to_string()))?;
    let set: Option<String> = redis::cmd("SET")
        .arg(key(namespace, id))
        .arg(value)
        .arg("NX")
        .arg("EX")
        .arg(ttl_secs)
        .query_async(&mut conn)
        .await
        .map_err(|e| KvError(e.to_string()))?;
    Ok(set.is_some())
}

/// `SET key value EX ttl` (overwrite).
pub(crate) async fn put(
    state: &AppState,
    namespace: &str,
    id: &str,
    value: &str,
    ttl_secs: u64,
) -> Result<(), KvError> {
    let op = || Op::Put {
        key: key(namespace, id),
        value: value.to_string(),
        ttl: ttl_secs,
    };
    if let Some(answer) = shared(state, op).await {
        return answer.map(|_| ());
    }
    let mut conn = state
        .redis_pool
        .get()
        .await
        .map_err(|e| KvError(e.to_string()))?;
    let _: () = redis::cmd("SET")
        .arg(key(namespace, id))
        .arg(value)
        .arg("EX")
        .arg(ttl_secs)
        .query_async(&mut conn)
        .await
        .map_err(|e| KvError(e.to_string()))?;
    Ok(())
}

/// `GET key`.
pub(crate) async fn get(
    state: &AppState,
    namespace: &str,
    id: &str,
) -> Result<Option<String>, KvError> {
    let op = || Op::Get {
        key: key(namespace, id),
    };
    if let Some(answer) = shared(state, op).await {
        return region_shared::value_of(&answer?).map_err(|e| KvError(e.to_string()));
    }
    let mut conn = state
        .redis_pool
        .get()
        .await
        .map_err(|e| KvError(e.to_string()))?;
    redis::cmd("GET")
        .arg(key(namespace, id))
        .query_async(&mut conn)
        .await
        .map_err(|e| KvError(e.to_string()))
}

/// `GETDEL key` — read and consume a one-time value atomically.
pub(crate) async fn take(
    state: &AppState,
    namespace: &str,
    id: &str,
) -> Result<Option<String>, KvError> {
    let op = || Op::Take {
        key: key(namespace, id),
    };
    if let Some(answer) = shared(state, op).await {
        return region_shared::value_of(&answer?).map_err(|e| KvError(e.to_string()));
    }
    let mut conn = state
        .redis_pool
        .get()
        .await
        .map_err(|e| KvError(e.to_string()))?;
    redis::cmd("GETDEL")
        .arg(key(namespace, id))
        .query_async(&mut conn)
        .await
        .map_err(|e| KvError(e.to_string()))
}

/// Attempts for [`put_replay`].
const REPLAY_WRITE_ATTEMPTS: u32 = 3;

/// [`put`] for a replay-cache entry, retried a bounded number of times: the
/// entry is the only way a retry of a committed rotation or exchange can
/// recover the new token, so a transient Redis error is worth a second try.
pub(crate) async fn put_replay(
    state: &AppState,
    namespace: &str,
    id: &str,
    value: &str,
    ttl_secs: u64,
) -> Result<(), KvError> {
    let mut attempt = 1;
    loop {
        match put(state, namespace, id, value, ttl_secs).await {
            Ok(()) => return Ok(()),
            Err(error) if attempt >= REPLAY_WRITE_ATTEMPTS => return Err(error),
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(50 * u64::from(attempt))).await;
                attempt += 1;
            }
        }
    }
}
