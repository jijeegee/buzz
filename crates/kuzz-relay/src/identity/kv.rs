//! Short-lived Redis state for the `/auth/*` flows: OIDC pending logins,
//! one-time login codes, and the 10-second refresh/exchange replay caches.
//!
//! All keys live under `buzz:auth:` with a TTL; every read that consumes a
//! one-time value uses `GETDEL` so two requests can never both redeem it.

use deadpool_redis::Pool;

/// Redis error surfaced to handlers as "unavailable".
#[derive(Debug, thiserror::Error)]
#[error("auth state store unavailable: {0}")]
pub(crate) struct KvError(String);

fn key(namespace: &str, id: &str) -> String {
    format!("buzz:auth:{namespace}:{id}")
}

/// `SET key value NX EX ttl`; `false` when the key already existed.
pub(crate) async fn put_new(
    pool: &Pool,
    namespace: &str,
    id: &str,
    value: &str,
    ttl_secs: u64,
) -> Result<bool, KvError> {
    let mut conn = pool.get().await.map_err(|e| KvError(e.to_string()))?;
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
    pool: &Pool,
    namespace: &str,
    id: &str,
    value: &str,
    ttl_secs: u64,
) -> Result<(), KvError> {
    let mut conn = pool.get().await.map_err(|e| KvError(e.to_string()))?;
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
pub(crate) async fn get(pool: &Pool, namespace: &str, id: &str) -> Result<Option<String>, KvError> {
    let mut conn = pool.get().await.map_err(|e| KvError(e.to_string()))?;
    redis::cmd("GET")
        .arg(key(namespace, id))
        .query_async(&mut conn)
        .await
        .map_err(|e| KvError(e.to_string()))
}

/// `GETDEL key` — read and consume a one-time value atomically.
pub(crate) async fn take(
    pool: &Pool,
    namespace: &str,
    id: &str,
) -> Result<Option<String>, KvError> {
    let mut conn = pool.get().await.map_err(|e| KvError(e.to_string()))?;
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
    pool: &Pool,
    namespace: &str,
    id: &str,
    value: &str,
    ttl_secs: u64,
) -> Result<(), KvError> {
    let mut attempt = 1;
    loop {
        match put(pool, namespace, id, value, ttl_secs).await {
            Ok(()) => return Ok(()),
            Err(error) if attempt >= REPLAY_WRITE_ATTEMPTS => return Err(error),
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(50 * u64::from(attempt))).await;
                attempt += 1;
            }
        }
    }
}
