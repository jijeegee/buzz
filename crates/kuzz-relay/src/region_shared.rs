//! Sign-in state, replay marks and per-account sign-in limits shared by every
//! region (global regions stage S4-7).
//!
//! Each region has its own Redis. Without this, a one-time login code taken
//! in one region could be taken again in another, a signed HTTP request could
//! be replayed against another region, and an account's sign-in limits would
//! count per region. With the region link on, a region that is not the
//! primary keeps these in the primary's Redis over the link instead of its
//! own:
//!
//! - `/auth/*` state (`kuzz:auth:*`: pending logins, login codes, signup
//!   tickets, key-backup challenges, refresh/exchange replay caches);
//! - NIP-98 replay marks on admin, operator, invite, workflow, git-settings
//!   and moderation-read requests, and NIP-FI command replay claims;
//! - per-account `/auth/*` limits (bot tokens, key backup/export, people
//!   lookups, refresh).
//!
//! If the primary cannot be reached, these fail closed, exactly as a Redis
//! outage does today. What runs on every message stays in each region, so a
//! round trip to the primary never slows chat and a primary outage never
//! stops it: per-IP limits, the per-pubkey message, API, socket and GIF
//! limits, and NIP-98 marks on HTTP event submit, query, count and GIF
//! search (events are idempotent by id, and a replayed read returns only
//! what its signer could read; a replay needs the stolen header within the
//! 60 s window).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::Json;
use kuzz_auth::{
    command_replay_key, nip98_replay_key_for_scope, AuthError, CommandReplayGuard,
    Nip98ReplayGuard, RateLimitResult, DEFAULT_REPLAY_TTL_SECS, MAX_REPLAY_TTL_SECS,
};
use nostr::EventId;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::state::AppState;

/// `POST` at the primary: run one [`Op`] on its Redis.
pub const PATH: &str = "/internal/region/shared-state";

/// A shared-state call waits at most this long. A one-time value the primary
/// takes after this deadline is lost to the caller, who starts over (sign in
/// again), so the deadline stays well above a normal round trip.
const CALL_TIMEOUT: Duration = Duration::from_secs(8);

/// Per-account `/auth/*` limit keys (`kuzz:ratelimit:{key}`) a region may
/// count at the primary. Per-IP and deployment-wide sign-in limits are not
/// among them: a linked region must not use up the primary's.
const SHARED_LIMITS: [&str; 6] = [
    "auth:bot_token:",
    "auth:exchange:",
    "auth:key-backup:",
    "auth:key-export:",
    "auth:people:",
    "auth:refresh:token:",
];

/// Longest key a region may name.
const MAX_KEY_LEN: usize = 512;

/// Longest value a region may store.
const MAX_VALUE_LEN: usize = 16 * 1024;

/// Longest TTL or limit window a region may ask for.
const MAX_TTL_SECS: u64 = 3600;

/// One operation on the primary's Redis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// `SET key value NX EX ttl`: whether it was set.
    PutNew {
        key: String,
        value: String,
        ttl: u64,
    },
    /// `SET key value EX ttl`.
    Put {
        key: String,
        value: String,
        ttl: u64,
    },
    /// `GET key`.
    Get { key: String },
    /// `GETDEL key`: read and consume once.
    Take { key: String },
    /// One hit on the fixed-window limit `kuzz:ratelimit:{key}`.
    Count {
        key: String,
        window: u64,
        limit: u64,
    },
}

/// Why a shared-state operation failed. Callers fail closed.
#[derive(Debug, thiserror::Error)]
#[error("shared state unavailable: {0}")]
pub struct SharedError(String);

/// Whether this region keeps shared state at the primary.
pub fn forwards(state: &AppState) -> bool {
    state.config.region_link && state.config.primary_region != state.config.region_id
}

/// Run `op` at the primary. Only meaningful when [`forwards`].
pub async fn at_primary(state: &AppState, op: &Op) -> Result<Value, SharedError> {
    let body = serde_json::to_value(op).map_err(|e| SharedError(e.to_string()))?;
    let answer = tokio::time::timeout(
        CALL_TIMEOUT,
        crate::region_link::call(
            state,
            &state.config.primary_region,
            Uuid::nil(),
            PATH,
            &body,
            None,
            None,
        ),
    )
    .await;
    let outcome = match &answer {
        Ok(Ok(_)) => "ok",
        // The primary answered with an error: a refusal (`not_primary`,
        // `not_shared`) is a configuration mistake, not an outage.
        Ok(Err(crate::region_link::CallError::Status(..))) => "refused",
        Ok(Err(_)) => "unreachable",
        Err(_) => "timeout",
    };
    metrics::counter!("kuzz_region_shared_state_calls_total", "outcome" => outcome).increment(1);
    match answer {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            tracing::warn!(
                outcome,
                ?error,
                "Shared-state call to the primary region failed"
            );
            Err(SharedError(format!("{error:?}")))
        }
        Err(_) => {
            tracing::warn!("Shared-state call to the primary region timed out");
            Err(SharedError("timed out".to_string()))
        }
    }
}

/// Whether the primary set a [`Op::PutNew`] key.
pub fn was_set(answer: &Value) -> Result<bool, SharedError> {
    answer
        .get("set")
        .and_then(Value::as_bool)
        .ok_or_else(|| SharedError("malformed answer".to_string()))
}

/// The value the primary returned for [`Op::Get`] / [`Op::Take`].
pub fn value_of(answer: &Value) -> Result<Option<String>, SharedError> {
    match answer.get("value") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        _ => Err(SharedError("malformed answer".to_string())),
    }
}

/// The primary's answer to [`Op::Count`].
pub fn limit_result(answer: &Value) -> Result<RateLimitResult, SharedError> {
    let field = |name: &str| {
        answer
            .get(name)
            .and_then(Value::as_u64)
            .ok_or_else(|| SharedError("malformed answer".to_string()))
    };
    let (count, limit, reset) = (field("count")?, field("limit")?, field("reset_in_secs")?);
    Ok(if count <= limit {
        RateLimitResult::allowed(count, limit, reset)
    } else {
        RateLimitResult::denied(count, limit, reset)
    })
}

/// NIP-98 replay marks and NIP-FI command claims kept at the primary.
/// Borrowed for one request: `guard(state)` picks it or the local guard.
pub struct PrimaryReplay<'s>(pub &'s AppState);

impl PrimaryReplay<'_> {
    async fn mark(&self, key: String, ttl_secs: u64) -> Result<bool, AuthError> {
        let op = Op::PutNew {
            key,
            value: "1".to_string(),
            ttl: ttl_secs.clamp(DEFAULT_REPLAY_TTL_SECS, MAX_REPLAY_TTL_SECS),
        };
        let answer = at_primary(self.0, &op)
            .await
            .map_err(|e| AuthError::Internal(e.to_string()))?;
        was_set(&answer).map_err(|e| AuthError::Internal(e.to_string()))
    }
}

impl Nip98ReplayGuard for PrimaryReplay<'_> {
    fn try_mark_in_scope<'a>(
        &'a self,
        scope: &'a str,
        event_id: &'a EventId,
        ttl_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AuthError>> + Send + 'a>> {
        Box::pin(self.mark(nip98_replay_key_for_scope(scope, event_id), ttl_secs))
    }
}

impl CommandReplayGuard for PrimaryReplay<'_> {
    fn try_claim<'a>(
        &'a self,
        issuer: &'a str,
        jti: &'a str,
        ttl_secs: u64,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AuthError>> + Send + 'a>> {
        Box::pin(self.mark(command_replay_key(issuer, jti), ttl_secs))
    }
}

/// Whether a region may name `key` for `op`. Only the state listed in the
/// module docs is shared; the link's own replay marks stay per region.
fn allowed(op: &Op) -> bool {
    let (key, ttl) = match op {
        Op::PutNew { key, value, ttl } | Op::Put { key, value, ttl } => {
            if value.len() > MAX_VALUE_LEN {
                return false;
            }
            (key, Some(*ttl))
        }
        Op::Get { key } | Op::Take { key } => (key, None),
        Op::Count { key, window, limit } => {
            return key.len() <= MAX_KEY_LEN
                && SHARED_LIMITS.iter().any(|prefix| key.starts_with(prefix))
                && (1..=MAX_TTL_SECS).contains(window)
                && *limit > 0;
        }
    };
    if key.len() > MAX_KEY_LEN || ttl.is_some_and(|ttl| !(1..=MAX_TTL_SECS).contains(&ttl)) {
        return false;
    }
    if key.starts_with("kuzz:auth:") {
        return true;
    }
    // Replay marks: set-if-absent of "1" only.
    let Op::PutNew { value, .. } = op else {
        return false;
    };
    if value != "1" {
        return false;
    }
    let hex64 = |id: &str| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some(id) = key.strip_prefix("kuzz:nip-fi:command:") {
        return hex64(id);
    }
    match key
        .strip_prefix("kuzz:")
        .and_then(|rest| rest.rsplit_once(":nip98:"))
    {
        Some((scope, id)) => {
            !scope.is_empty() && scope != crate::region_link::REPLAY_SCOPE && hex64(id)
        }
        None => false,
    }
}

/// Run `op` on this region's Redis.
pub async fn run_here(state: &AppState, op: &Op) -> Result<Value, SharedError> {
    let fail = |e: &dyn std::fmt::Display| SharedError(e.to_string());
    if let Op::Count { key, window, limit } = op {
        let result = state
            .admission_rate_limiter
            .check_key(key, *window, *limit)
            .await
            .map_err(|e| fail(&e))?;
        return Ok(json!({
            "count": result.current,
            "limit": result.limit,
            "reset_in_secs": result.reset_in_secs,
        }));
    }
    let mut conn = state.redis_pool.get().await.map_err(|e| fail(&e))?;
    match op {
        Op::PutNew { key, value, ttl } => {
            let set: Option<String> = redis::cmd("SET")
                .arg(key)
                .arg(value)
                .arg("NX")
                .arg("EX")
                .arg(*ttl)
                .query_async(&mut conn)
                .await
                .map_err(|e| fail(&e))?;
            Ok(json!({ "set": set.is_some() }))
        }
        Op::Put { key, value, ttl } => {
            let _: () = redis::cmd("SET")
                .arg(key)
                .arg(value)
                .arg("EX")
                .arg(*ttl)
                .query_async(&mut conn)
                .await
                .map_err(|e| fail(&e))?;
            Ok(json!({}))
        }
        Op::Get { key } | Op::Take { key } => {
            let command = if matches!(op, Op::Take { .. }) {
                "GETDEL"
            } else {
                "GET"
            };
            let value: Option<String> = redis::cmd(command)
                .arg(key)
                .query_async(&mut conn)
                .await
                .map_err(|e| fail(&e))?;
            Ok(json!({ "value": value }))
        }
        Op::Count { .. } => unreachable!("handled above"),
    }
}

fn answer_error(status: StatusCode, error: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": error })))
}

/// `POST /internal/region/shared-state` (primary): run one shared-state
/// operation for a linked region.
pub async fn shared_state(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    if let Err(refusal) =
        crate::region_link::admit_deployment(&state, &Method::POST, &uri, &headers, &body).await
    {
        metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
            .increment(1);
        tracing::warn!(reason = refusal.label(), "Refused a shared-state call");
        return answer_error(StatusCode::UNAUTHORIZED, "region link refused");
    }
    if forwards(&state) || !state.config.region_link {
        return answer_error(StatusCode::CONFLICT, "not_primary");
    }
    let Ok(op) = serde_json::from_slice::<Op>(&body) else {
        return answer_error(StatusCode::BAD_REQUEST, "malformed");
    };
    if !allowed(&op) {
        return answer_error(StatusCode::BAD_REQUEST, "not_shared");
    }
    match run_here(&state, &op).await {
        Ok(answer) => (StatusCode::OK, Json(answer)),
        Err(error) => {
            tracing::warn!(%error, "Shared-state operation failed");
            answer_error(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_new(key: &str) -> Op {
        Op::PutNew {
            key: key.to_string(),
            value: "1".to_string(),
            ttl: 120,
        }
    }

    #[test]
    fn only_sign_in_state_replay_marks_and_sign_in_limits_are_shared() {
        let id = "ab".repeat(32);
        assert!(allowed(&put_new("kuzz:auth:login:abc")));
        assert!(allowed(&Op::Take {
            key: "kuzz:auth:login:abc".to_string()
        }));
        assert!(allowed(&put_new(&format!(
            "kuzz:{}:nip98:{id}",
            Uuid::nil()
        ))));
        assert!(allowed(&put_new(&format!(
            "kuzz:admin-moderation:nip98:{id}"
        ))));
        assert!(allowed(&put_new(&format!("kuzz:nip-fi:command:{id}"))));
        assert!(allowed(&Op::Count {
            key: "auth:key-backup:abc".to_string(),
            window: 60,
            limit: 5
        }));

        // The link's own replay marks stay per region.
        assert!(!allowed(&put_new(&format!(
            "kuzz:{}:nip98:{id}",
            crate::region_link::REPLAY_SCOPE
        ))));
        // Replay marks are set-if-absent only.
        assert!(!allowed(&Op::Take {
            key: format!("kuzz:admin-moderation:nip98:{id}")
        }));
        // Anything else in Redis is not reachable.
        assert!(!allowed(&put_new("kuzz:ratelimit:ip:1.2.3.4:conn")));
        assert!(!allowed(&Op::Get {
            key: "kuzz:sessions:abc".to_string()
        }));
        assert!(!allowed(&put_new("kuzz:x:nip98:short")));
        assert!(!allowed(&Op::Count {
            key: "ip:1.2.3.4:conn".to_string(),
            window: 60,
            limit: 5
        }));
        // Per-IP and deployment-wide sign-in limits stay in each region.
        for key in ["auth:login:start:1.2.3.4", "auth:login:start:global"] {
            assert!(!allowed(&Op::Count {
                key: key.to_string(),
                window: 60,
                limit: 5
            }));
        }
        // Replay marks hold only "1", under a full-length id.
        assert!(!allowed(&Op::PutNew {
            key: format!("kuzz:admin-moderation:nip98:{id}"),
            value: "x".to_string(),
            ttl: 120
        }));
        assert!(!allowed(&put_new("kuzz:nip-fi:command:short")));
        assert!(!allowed(&Op::PutNew {
            key: "kuzz:auth:login:abc".to_string(),
            value: "1".to_string(),
            ttl: 0
        }));
        assert!(!allowed(&Op::PutNew {
            key: "kuzz:auth:login:abc".to_string(),
            value: "1".to_string(),
            ttl: MAX_TTL_SECS + 1
        }));
    }

    #[test]
    fn answers_are_read_strictly() {
        assert!(was_set(&json!({ "set": true })).unwrap());
        assert!(was_set(&json!({})).is_err());
        assert_eq!(value_of(&json!({ "value": null })).unwrap(), None);
        assert_eq!(
            value_of(&json!({ "value": "x" })).unwrap().as_deref(),
            Some("x")
        );
        assert!(value_of(&json!({})).is_err());
        let denied = limit_result(&json!({ "count": 6, "limit": 5, "reset_in_secs": 9 })).unwrap();
        assert!(!denied.allowed);
        assert!(limit_result(&json!({ "count": 1 })).is_err());
    }
}
