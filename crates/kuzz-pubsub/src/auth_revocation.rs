//! Cross-instance access-token revocation fan-out (centralized identity).
//!
//! After a revoke transaction commits, the revoking instance publishes the
//! revoked token hashes on [`AUTH_REVOKED_CHANNEL`]. Every instance closes its
//! connections whose **currently bound** token hash is in the set. Matching on
//! the token hash — never the principal, session or bot — means revoking one
//! token cannot close another connection of the same principal.
//!
//! A message may instead carry `not_after`: the bound connections are not
//! closed now but their deadline is lowered (used when an exchange starts the
//! superseded token's grace period). The DB row stays the durable backstop: a
//! dropped message is caught by the connection's periodic DB recheck and the
//! token's own expiry.
//!
//! Device-bound keys (plan §5, PR A4): a message may also name device rows
//! (`device_ids`) whose key-authenticated sockets close, and message keys
//! (`pubkeys`) whose sockets are checked against the registry again at
//! once. Both default to empty, so older publishers and consumers
//! interoperate: an older consumer ignores the fields, an older publisher
//! sends none.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Global Redis pub/sub channel for token revocations.
pub const AUTH_REVOKED_CHANNEL: &str = "kuzz:auth:revoked";

/// Maximum hashes per published message; larger sets are chunked.
pub const MAX_HASHES_PER_MESSAGE: usize = 256;

/// A batch of revoked (or deadline-shortened) access-token hashes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthRevocation {
    /// Hex SHA-256 hashes of the affected tokens.
    pub token_hashes: Vec<String>,
    /// Revocation reason (`logout`, `device_revoked`, `exchanged`, ...).
    pub reason: String,
    /// When set, do not close now: lower the bound connection's deadline to
    /// this unix time (seconds) instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<i64>,
    /// Device rows signed out, revoked or disabled with their account: key
    /// sockets admitted from them close.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_ids: Vec<uuid::Uuid>,
    /// Hex message keys whose registry row changed (an agent moved to
    /// another host): their key sockets are checked again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pubkeys: Vec<String>,
}

impl AuthRevocation {
    /// Split `hashes` into messages of at most [`MAX_HASHES_PER_MESSAGE`].
    pub fn batches(hashes: &[[u8; 32]], reason: &str, not_after: Option<i64>) -> Vec<Self> {
        hashes
            .chunks(MAX_HASHES_PER_MESSAGE)
            .map(|chunk| Self {
                token_hashes: chunk.iter().map(hex::encode).collect(),
                reason: reason.to_owned(),
                not_after,
                device_ids: Vec::new(),
                pubkeys: Vec::new(),
            })
            .collect()
    }

    /// Split `device_ids` and `pubkeys` into messages that carry no token
    /// hashes, at most [`MAX_HASHES_PER_MESSAGE`] entries of each.
    pub fn key_batches(device_ids: &[uuid::Uuid], pubkeys: &[[u8; 32]], reason: &str) -> Vec<Self> {
        let devices = device_ids.chunks(MAX_HASHES_PER_MESSAGE);
        let keys = pubkeys.chunks(MAX_HASHES_PER_MESSAGE);
        let count = devices.len().max(keys.len());
        let mut devices = devices.map(<[uuid::Uuid]>::to_vec);
        let mut keys = keys.map(|chunk| chunk.iter().map(hex::encode).collect());
        (0..count)
            .map(|_| Self {
                token_hashes: Vec::new(),
                reason: reason.to_owned(),
                not_after: None,
                device_ids: devices.next().unwrap_or_default(),
                pubkeys: keys.next().unwrap_or_default(),
            })
            .collect()
    }

    /// Decoded message keys; malformed entries are skipped.
    pub fn key_bytes(&self) -> Vec<[u8; 32]> {
        decode_32(&self.pubkeys)
    }

    /// Decoded hashes; malformed entries are skipped.
    pub fn hashes(&self) -> Vec<[u8; 32]> {
        decode_32(&self.token_hashes)
    }
}

fn decode_32(values: &[String]) -> Vec<[u8; 32]> {
    values
        .iter()
        .filter_map(|h| hex::decode(h).ok())
        .filter_map(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
        .collect()
}

const BACKOFF_INITIAL_SECS: u64 = 1;
const BACKOFF_MAX_SECS: u64 = 30;

/// Subscribe to [`AUTH_REVOKED_CHANNEL`] and forward messages to the local
/// broadcast. Reconnects with exponential backoff; never returns.
pub async fn run_auth_revocation_subscriber(
    redis_url: String,
    broadcast_tx: broadcast::Sender<AuthRevocation>,
) {
    let mut backoff_secs = BACKOFF_INITIAL_SECS;
    loop {
        match connect_and_subscribe(&redis_url, &broadcast_tx).await {
            Ok(()) => {
                backoff_secs = BACKOFF_INITIAL_SECS;
                tracing::warn!(
                    "Redis auth-revocation stream ended — reconnecting in {backoff_secs}s"
                );
            }
            Err(e) => {
                tracing::error!(
                    "Redis auth-revocation error: {e} — reconnecting in {backoff_secs}s"
                );
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
        backoff_secs = (backoff_secs * 2).min(BACKOFF_MAX_SECS);
    }
}

async fn connect_and_subscribe(
    redis_url: &str,
    broadcast_tx: &broadcast::Sender<AuthRevocation>,
) -> Result<(), redis::RedisError> {
    let client = redis::Client::open(redis_url)?;
    let mut conn = client.get_async_pubsub().await?;
    conn.subscribe(AUTH_REVOKED_CHANNEL).await?;
    tracing::info!("Redis auth-revocation subscriber listening on {AUTH_REVOKED_CHANNEL}");
    let mut stream = conn.on_message();
    while let Some(msg) = stream.next().await {
        let payload: String = match msg.get_payload() {
            Ok(payload) => payload,
            Err(e) => {
                tracing::warn!("Failed to read auth-revocation payload: {e}");
                continue;
            }
        };
        match serde_json::from_str::<AuthRevocation>(&payload) {
            Ok(message) => {
                let _ = broadcast_tx.send(message);
            }
            Err(e) => tracing::warn!("Malformed auth-revocation message: {e}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_chunk_and_round_trip() {
        let hashes: Vec<[u8; 32]> = (0..600u32)
            .map(|i| {
                let mut h = [0u8; 32];
                h[..4].copy_from_slice(&i.to_be_bytes());
                h
            })
            .collect();
        let batches = AuthRevocation::batches(&hashes, "logout", None);
        assert_eq!(batches.len(), 3);
        let decoded: Vec<[u8; 32]> = batches.iter().flat_map(AuthRevocation::hashes).collect();
        assert_eq!(decoded, hashes);
        let json = serde_json::to_string(&batches[0]).unwrap();
        assert!(!json.contains("not_after"));
        let back: AuthRevocation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, batches[0]);
    }

    /// Messages without the device-bound key fields (older publishers) still
    /// parse, and token-only messages do not grow them on the wire.
    #[test]
    fn device_fields_default_to_empty_and_round_trip() {
        let old = r#"{"token_hashes":["00"],"reason":"logout"}"#;
        let parsed: AuthRevocation = serde_json::from_str(old).unwrap();
        assert!(parsed.device_ids.is_empty() && parsed.pubkeys.is_empty());
        let json = serde_json::to_string(&parsed).unwrap();
        assert!(!json.contains("device_ids") && !json.contains("pubkeys"));

        let devices: Vec<uuid::Uuid> = (0..300u128).map(uuid::Uuid::from_u128).collect();
        let key = [7u8; 32];
        let batches = AuthRevocation::key_batches(&devices, &[key], "logout");
        assert_eq!(batches.len(), 2);
        assert!(batches.iter().all(|batch| batch.token_hashes.is_empty()));
        assert_eq!(batches[0].key_bytes(), vec![key]);
        assert!(batches[1].pubkeys.is_empty());
        let all: Vec<uuid::Uuid> = batches.iter().flat_map(|b| b.device_ids.clone()).collect();
        assert_eq!(all, devices);
        let json = serde_json::to_string(&batches[0]).unwrap();
        let back: AuthRevocation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, batches[0]);
        assert!(AuthRevocation::key_batches(&[], &[], "logout").is_empty());
    }
}
