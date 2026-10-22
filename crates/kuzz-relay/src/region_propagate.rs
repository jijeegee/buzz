//! Propagation between regions (global regions stage S4-5): logouts, token
//! revocations and disconnects reach the connections of every region.
//!
//! Each of these closes connections through this region's Redis only. With
//! the link on, the same command is also written to `region_outbox`, one row
//! per other region ([`spread`]). [`run`] delivers the rows over the link to
//! `POST /internal/region/propagate` ([`propagate`]), and the target region
//! applies the command to its own pods through its Redis. A row that fails
//! is retried with backoff (at most a minute apart) for a day.
//!
//! Every command is idempotent (close what matches), so a retry or a
//! duplicate is harmless and order does not matter. Rows are written right
//! after the change commits, like the Redis publish; the per-connection
//! database recheck and token expiry stay the backstop for a crash between
//! the two.
//!
//! The receiving region never writes outbox rows for what it applies, so a
//! command travels one hop.
//!
//! Blocks are different (S4-5b): they change data, not connections, and a
//! DM's home reads them from its own database for every message. A block
//! or unblock is therefore sent to every other region at once, before the
//! answer ([`block_everywhere`]); if one region does not apply it, the
//! request fails and the app retries it (blocking again is harmless). Each
//! region records the block period with its own clock, the clock its own
//! messages are stamped with, so regions' clocks never meet.
//!
//! Only active with the link on (`KUZZ_REGION_LINK`).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::Json;
use chrono::Utc;
use kuzz_core::tenant::TenantContext;
use kuzz_db::region_outbox::RegionOutboxRow;
use kuzz_pubsub::auth_revocation::AuthRevocation;
use kuzz_pubsub::conn_control::ConnControl;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::config::Config;
use crate::state::AppState;
use kuzz_db::Db;

/// The link route commands are delivered to.
pub const PROPAGATE_PATH: &str = "/internal/region/propagate";

/// Lease of a claimed row: another pod retries it after this.
const LEASE_SECS: i64 = 30;
/// Rows claimed per round.
const BATCH_SIZE: i64 = 32;
/// Deliveries in flight at once, across regions.
const MAX_IN_FLIGHT: usize = 64;
/// A row still failing after this long is dropped: the tokens it revokes
/// have expired and the bans it carries are refused again at sign-in.
const MAX_AGE: chrono::Duration = chrono::Duration::days(1);

/// A command applied in every region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "topic", content = "payload", rename_all = "snake_case")]
pub enum Propagate {
    /// Close (or shorten) the connections bound to these tokens.
    AuthRevoked(AuthRevocation),
    /// Close a pubkey's connections, or every connection, of the community.
    ConnControl(ConnControl),
    /// An identity provider's disconnect (NIP-FI): deny the pubkey for that
    /// issuer until `until` and close its sessions.
    NipFiDisconnect(kuzz_pubsub::NipFiDisconnect),
    /// `blocker` blocked (`on`) or unblocked `blocked` (hex pubkeys).
    Block {
        /// Who blocks.
        blocker: String,
        /// Who is blocked.
        blocked: String,
        /// Block (true) or unblock (false).
        on: bool,
        /// The entry region's time of the change in microseconds; a region
        /// ignores a change older than the one it applied last for the pair.
        version: i64,
    },
}

/// Why a block did not reach every region.
#[derive(Debug)]
pub struct BlockNotEverywhere {
    /// Regions that did not apply it.
    pub regions: Vec<String>,
}

/// The version of a block change made now (microseconds of this region's
/// clock). Changes of one pair made in different regions within the clock
/// difference may settle on either one.
pub fn block_version() -> i64 {
    Utc::now().timestamp_micros()
}

/// Save a block (`on`) or unblock made here. With the link on it is saved at
/// `version`, so an older change arriving later from another region cannot
/// undo it.
pub async fn save_block(
    state: &AppState,
    community: kuzz_core::CommunityId,
    blocker: &[u8],
    blocked: &[u8],
    on: bool,
    version: i64,
) -> kuzz_db::Result<kuzz_db::blocks::BlockChange> {
    use kuzz_db::blocks::BlockChange;
    if state.config.region_link {
        return state
            .db
            .set_block_at(community, blocker, blocked, on, version, true)
            .await;
    }
    if on {
        state
            .db
            .block_user(community, blocker, blocked)
            .await
            .map(BlockChange::Blocked)
    } else {
        state
            .db
            .unblock_user(community, blocker, blocked)
            .await
            .map(|_| BlockChange::Unblocked)
    }
}

/// Apply a block or unblock in every other region and wait for all of them.
/// Ok with the link off or a single region.
pub async fn block_everywhere(
    state: &AppState,
    community_id: Uuid,
    blocker: &[u8],
    blocked: &[u8],
    on: bool,
    version: i64,
) -> Result<(), BlockNotEverywhere> {
    if !state.config.region_link {
        return Ok(());
    }
    let regions = match state.db.list_regions().await {
        Ok(regions) => regions,
        Err(error) => {
            tracing::warn!(%error, "Could not list regions for a block");
            return Err(BlockNotEverywhere {
                regions: vec!["?".into()],
            });
        }
    };
    let body = serde_json::to_value(Propagate::Block {
        blocker: hex::encode(blocker),
        blocked: hex::encode(blocked),
        on,
        version,
    })
    .unwrap_or(Value::Null);
    let others: Vec<String> = regions
        .into_iter()
        .map(|region| region.id)
        .filter(|id| *id != state.config.region_id)
        .collect();
    let results = futures_util::future::join_all(others.iter().map(|region| {
        crate::region_link::call(
            state,
            region,
            community_id,
            PROPAGATE_PATH,
            &body,
            None,
            None,
        )
    }))
    .await;
    let failed: Vec<String> = others
        .into_iter()
        .zip(results)
        .filter(|(_, result)| {
            // An older region answers an unknown topic with applied: false.
            !matches!(result, Ok(answer) if answer["applied"] == Value::Bool(true))
        })
        .map(|(region, result)| {
            tracing::warn!(target = %region, ?result, "A region did not apply a block");
            region
        })
        .collect();
    metrics::counter!("kuzz_region_blocks_total", "result" => if failed.is_empty() { "ok" } else { "failed" }).increment(1);
    if failed.is_empty() {
        Ok(())
    } else {
        Err(BlockNotEverywhere { regions: failed })
    }
}

/// Queue `command` for every other region. Does nothing with the link off.
/// A failed write is logged and counted, and returned for callers that can
/// offer a retry; the caller's change stands either way.
pub async fn spread(
    state: &AppState,
    community_id: Uuid,
    command: &Propagate,
) -> Result<(), String> {
    spread_with(&state.db, &state.config, community_id, command).await
}

/// [`spread`] from a task that holds the database and config, not the state.
pub async fn spread_with(
    db: &Db,
    config: &Config,
    community_id: Uuid,
    command: &Propagate,
) -> Result<(), String> {
    if !config.region_link {
        return Ok(());
    }
    let (topic, payload) = split(command).map_err(|error| {
        tracing::error!(%error, "Could not encode a region propagation");
        error.to_string()
    })?;
    match db
        .enqueue_region_outbox(&config.region_id, community_id, &topic, &payload)
        .await
    {
        Ok(rows) => {
            metrics::counter!("kuzz_region_outbox_written_total", "topic" => topic).increment(rows);
            Ok(())
        }
        Err(error) => {
            metrics::counter!("kuzz_region_outbox_write_failures_total").increment(1);
            tracing::warn!(%error, topic, "Could not queue a region propagation; other regions keep their connections until rechecked");
            Err(error.to_string())
        }
    }
}

/// [`spread`] for a command that is not about one community (token
/// revocations): named with the community every region serves. Failures are
/// logged; the revoked rows and token expiry are the backstop.
pub async fn spread_global(state: &AppState, command: &Propagate) {
    if let Some(community) = state.config.community_id {
        let _ = spread(state, community, command).await;
    }
}

/// The `topic` and `payload` columns of a command.
fn split(command: &Propagate) -> Result<(String, Value), serde_json::Error> {
    let mut value = serde_json::to_value(command)?;
    let topic = value
        .get("topic")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let payload = value
        .get_mut("payload")
        .map(Value::take)
        .unwrap_or(Value::Null);
    Ok((topic, payload))
}

/// Deliver `region_outbox` rows for the life of the process. Several pods may
/// run it: rows are leased.
pub async fn run(state: Arc<AppState>) {
    let worker = format!("region-outbox-{}", Uuid::new_v4());
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT));
    let mut idle = Duration::from_millis(500);
    loop {
        // Claim only what can start now, so no lease runs out while waiting.
        let free = slots.available_permits().min(BATCH_SIZE as usize);
        if free == 0 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        let lease_until = Utc::now() + chrono::Duration::seconds(LEASE_SECS);
        let rows = match state
            .db
            .claim_region_outbox(&worker, lease_until, free as i64)
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, "Region outbox claim failed");
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        if rows.is_empty() {
            tokio::time::sleep(idle).await;
            idle = (idle * 2).min(Duration::from_secs(5));
            continue;
        }
        idle = Duration::from_millis(500);
        // Each row on its own task: a slow or unreachable region does not
        // hold up the others.
        for row in rows {
            let Ok(slot) = Arc::clone(&slots).try_acquire_owned() else {
                break;
            };
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                deliver(&state, &row).await;
                drop(slot);
            });
        }
    }
}

async fn deliver(state: &AppState, row: &RegionOutboxRow) {
    let body = json!({ "topic": row.topic, "payload": row.payload });
    let result = crate::region_link::call(
        state,
        &row.target_region,
        row.community_id,
        PROPAGATE_PATH,
        &body,
        None,
        None,
    )
    .await;
    let target = row.target_region.clone();
    let finished = match result {
        Ok(_) => {
            metrics::counter!("kuzz_region_outbox_delivered_total", "target" => target)
                .increment(1);
            state.db.finish_region_outbox(row.id, row.claim_token).await
        }
        Err(error) if Utc::now() - row.created_at > MAX_AGE => {
            metrics::counter!("kuzz_region_outbox_dropped_total", "target" => target).increment(1);
            tracing::error!(target = %row.target_region, topic = %row.topic, ?error, attempts = row.attempt_count, "Dropped a region propagation that kept failing");
            state.db.finish_region_outbox(row.id, row.claim_token).await
        }
        Err(error) => {
            metrics::counter!("kuzz_region_outbox_retries_total", "target" => target).increment(1);
            tracing::warn!(target = %row.target_region, topic = %row.topic, ?error, attempts = row.attempt_count, "Region propagation failed; retrying");
            state
                .db
                .retry_region_outbox(row.id, row.claim_token, &format!("{error:?}"))
                .await
        }
    };
    if let Err(error) = finished {
        tracing::warn!(%error, id = row.id, "Could not update a region outbox row; it is retried after its lease");
    }
}

/// `POST /internal/region/propagate`: apply a command from another region to
/// this region's connections (this pod, then the others through Redis).
/// An unknown topic (a newer sender) is answered as applied so it is not
/// retried forever.
pub async fn propagate(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let call = match crate::region_link::admit(&state, &Method::POST, &uri, &headers, &body).await {
        Ok(call) => call,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a region propagation");
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "refused", "message": "region link refused" })),
            );
        }
    };
    let command = match serde_json::from_slice::<Propagate>(&body) {
        Ok(command) => command,
        Err(error) => {
            tracing::warn!(from = %call.claims.entry_region, %error, "Ignoring an unknown region propagation");
            return (StatusCode::OK, Json(json!({ "applied": false })));
        }
    };
    match apply_here(&state, &call.tenant, &command).await {
        Ok(()) => {
            metrics::counter!("kuzz_region_propagations_applied_total", "from" => call.claims.entry_region.clone()).increment(1);
            (StatusCode::OK, Json(json!({ "applied": true })))
        }
        Err(error) => {
            tracing::warn!(%error, "Could not apply a region propagation here; the sender retries");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "error: try again" })),
            )
        }
    }
}

/// Apply on this pod, then publish to this region's other pods. Never
/// queues for other regions.
async fn apply_here(
    state: &AppState,
    tenant: &TenantContext,
    command: &Propagate,
) -> Result<(), String> {
    match command {
        Propagate::NipFiDisconnect(message) => {
            crate::api::nip_fi::apply_nip_fi_disconnect(state, message, Utc::now());
            state
                .pubsub
                .publish_nip_fi_disconnect(message)
                .await
                .map(drop)
                .map_err(|error| error.to_string())
        }
        Propagate::Block {
            blocker,
            blocked,
            on,
            version,
        } => {
            let (Ok(blocker), Ok(blocked)) = (hex::decode(blocker), hex::decode(blocked)) else {
                return Err("malformed block".into());
            };
            // The entry already checked the limit; a copy over it here still
            // blocks, so regions never disagree. An older change is ignored.
            let change = state
                .db
                .set_block_at(tenant.community(), &blocker, &blocked, *on, *version, false)
                .await
                .map_err(|error| error.to_string())?;
            if change != kuzz_db::blocks::BlockChange::Stale {
                state.invalidate_block(tenant, &blocker, &blocked);
            }
            Ok(())
        }
        Propagate::AuthRevoked(message) => {
            crate::identity::apply_revocation(state, message);
            state
                .pubsub
                .publish_auth_revocation(message)
                .await
                .map(drop)
                .map_err(|error| error.to_string())
        }
        Propagate::ConnControl(control) => {
            match control {
                ConnControl::DisconnectCommunity => {
                    state
                        .community_connections
                        .disconnect_community(tenant.community());
                }
                ConnControl::DisconnectPubkey {
                    pubkey,
                    event_id,
                    reason,
                    unowned_only,
                } => {
                    state.disconnect_pubkey_local(
                        tenant.community(),
                        pubkey,
                        event_id,
                        reason,
                        *unowned_only,
                    );
                }
            }
            state
                .pubsub
                .publish_conn_control(tenant, control)
                .await
                .map(drop)
                .map_err(|error| error.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split_into_topic_and_payload_and_back() {
        let revoked = Propagate::AuthRevoked(
            AuthRevocation::batches(&[[7u8; 32]], "logout", None)
                .pop()
                .expect("one batch"),
        );
        let ban = Propagate::ConnControl(ConnControl::DisconnectPubkey {
            pubkey: vec![1; 32],
            event_id: "0".repeat(64),
            reason: "blocked: banned".into(),
            unowned_only: false,
        });
        for command in [
            revoked,
            ban,
            Propagate::ConnControl(ConnControl::DisconnectCommunity),
            Propagate::Block {
                blocker: "ab".repeat(32),
                blocked: "cd".repeat(32),
                on: true,
                version: 1,
            },
            Propagate::NipFiDisconnect(kuzz_pubsub::NipFiDisconnect {
                issuer: "https://idp.example".into(),
                pubkey_bytes: vec![2; 32],
                until_unix: 1,
                until_unix_nanos: 0,
            }),
        ] {
            let (topic, payload) = split(&command).expect("encode");
            assert!(!topic.is_empty());
            // The receiver reads the body the worker sends.
            let body = json!({ "topic": topic, "payload": payload });
            assert_eq!(
                serde_json::from_value::<Propagate>(body).expect("decode"),
                command
            );
        }
        assert!(
            serde_json::from_value::<Propagate>(json!({"topic": "later", "payload": {}})).is_err()
        );
    }
}
