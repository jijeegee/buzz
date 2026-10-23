//! Reads a copy region checks with the primary (global regions stage S5-4,
//! decisions D5 and D6).
//!
//! A region holding the global copy (S5-1) reads accounts, tokens, members
//! and bans from its copy, which trails the primary by about a second.
//!
//! - **Too new (D5).** A token, member, allowlist entry or agent owner made
//!   at the primary a moment ago is not in the copy yet. A "no" from the
//!   copy is checked once with the primary
//!   (`POST /internal/region/global-read`), which answers facts, never rows.
//!   A token the primary knows is then waited for in the copy, since its row
//!   (principal, scopes, expiry) is read here. The primary's "no" is not
//!   cached, and when it cannot be asked the copy's "no" stands: a check
//!   never opens access because the primary is away.
//! - **Too old (D6).** When the copy's lag is unknown or above
//!   `KUZZ_GLOBAL_COPY_MAX_LAG_SECS`, a ban or a removal made at the primary
//!   may be missing here. Ban and membership checks the copy allowed are
//!   then asked of the primary and fail closed when it cannot answer. Token
//!   revocations already reach every region live (S4-5).
//!
//! Calls are bounded apart from the room lookups (S4-6): a few at once,
//! none for a few seconds after the primary failed one, and a token the
//! primary did not know is not asked about again for a few seconds, so a
//! flood of unknown tokens cannot pile up calls or starve other checks.
//!
//! The lag is measured with the regions' clocks (S5-1): a copy region whose
//! clock runs behind the primary's sees less lag than there is.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use kuzz_core::tenant::{CommunityId, TenantContext};
use kuzz_db::moderation::RestrictionState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::state::AppState;

/// Primary side: answer a copy region's read.
pub const GLOBAL_READ_PATH: &str = "/internal/region/global-read";

/// How long a read at the primary may take.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a token the primary knows is waited for in the copy. With
/// [`READ_TIMEOUT`] this stays under the socket's 5 s AUTH deadline.
const TOKEN_WAIT: Duration = Duration::from_millis(2500);
/// Most reads at the primary one relay runs at once.
const MAX_READS: usize = 64;
/// How long the primary is not asked again after a failed read, and an
/// unknown token is not asked about again.
const QUIET_FOR: Duration = Duration::from_secs(5);
/// How often the copy is looked at while waiting.
const TOKEN_POLL: Duration = Duration::from_millis(100);

/// A question for the primary. Pubkeys and hashes are hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum GlobalRead {
    /// Whether an access token with this SHA-256 hash exists (in any state:
    /// the row, once copied, says whether it is usable). Asked for no
    /// community.
    TokenKnown {
        /// Hex SHA-256 of the token.
        hash: String,
    },
    /// Whether the pubkey is a relay member.
    RelayMember {
        /// Hex pubkey.
        pubkey: String,
    },
    /// Whether the pubkey is on the allowlist.
    Allowed {
        /// Hex pubkey.
        pubkey: String,
    },
    /// Whether `owner` owns `agent`.
    AgentOwner {
        /// The agent's hex pubkey.
        agent: String,
        /// The owner's hex pubkey.
        owner: String,
    },
    /// The pubkey's ban and timeout state.
    Restriction {
        /// Hex pubkey.
        pubkey: String,
    },
}

impl GlobalRead {
    fn label(&self) -> &'static str {
        match self {
            Self::TokenKnown { .. } => "token",
            Self::RelayMember { .. } => "member",
            Self::Allowed { .. } => "allowlist",
            Self::AgentOwner { .. } => "agent_owner",
            Self::Restriction { .. } => "restriction",
        }
    }
}

/// The primary's answer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadAnswer {
    /// The fact holds (for [`GlobalRead::Restriction`]: banned).
    pub yes: bool,
    /// For [`GlobalRead::Restriction`]: the active timeout's end, in Unix
    /// seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted_until: Option<i64>,
}

/// Bounds and recent answers for reads at the primary, per relay.
#[derive(Debug)]
pub struct GlobalReads {
    slots: Arc<tokio::sync::Semaphore>,
    primary_down_until: std::sync::Mutex<Option<std::time::Instant>>,
    unknown_tokens: moka::sync::Cache<[u8; 32], ()>,
}

impl Default for GlobalReads {
    fn default() -> Self {
        Self {
            slots: Arc::new(tokio::sync::Semaphore::new(MAX_READS)),
            primary_down_until: std::sync::Mutex::new(None),
            unknown_tokens: moka::sync::Cache::builder()
                .max_capacity(100_000)
                .time_to_live(QUIET_FOR)
                .build(),
        }
    }
}

impl GlobalReads {
    fn slot(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let down = self
            .primary_down_until
            .lock()
            .map(|until| until.is_some_and(|until| std::time::Instant::now() < until))
            .unwrap_or(false);
        if down {
            return None;
        }
        Arc::clone(&self.slots).try_acquire_owned().ok()
    }

    fn note_primary_down(&self) {
        if let Ok(mut until) = self.primary_down_until.lock() {
            *until = Some(std::time::Instant::now() + QUIET_FOR);
        }
    }
}

/// Whether this region's copy is too far behind to be trusted for bans and
/// removals: lag unknown, or above the limit. Always false outside a copy
/// region.
pub fn copy_lagging(state: &AppState) -> bool {
    state.config.global_copy
        && state
            .global_copy_lag
            .get()
            .is_none_or(|lag| lag > state.config.global_copy_max_lag)
}

/// Ask the primary. `None` outside a copy region, when the bound is full,
/// when the primary failed a moment ago, or on any failure.
async fn ask(
    state: &AppState,
    community: Option<CommunityId>,
    read: &GlobalRead,
) -> Option<ReadAnswer> {
    if !state.config.global_copy {
        return None;
    }
    let label = read.label();
    let Some(_slot) = state.global_reads.slot() else {
        metrics::counter!("kuzz_global_reads_total", "ask" => label, "outcome" => "skipped")
            .increment(1);
        return None;
    };
    let body = serde_json::to_value(read).ok()?;
    let community = community.map_or(uuid::Uuid::nil(), |community| *community.as_uuid());
    let answer = tokio::time::timeout(
        READ_TIMEOUT,
        crate::region_link::call(
            state,
            &state.config.primary_region,
            community,
            GLOBAL_READ_PATH,
            &body,
            None,
            None,
        ),
    )
    .await;
    let answer = match answer {
        Ok(Ok(answer)) => serde_json::from_value::<ReadAnswer>(answer).ok(),
        Ok(Err(error)) => {
            tracing::warn!(?error, ask = label, "Could not ask the primary region");
            None
        }
        Err(_) => {
            tracing::warn!(ask = label, "Asking the primary region timed out");
            None
        }
    };
    let outcome = if answer.is_some() { "ok" } else { "failed" };
    if answer.is_none() {
        state.global_reads.note_primary_down();
    }
    metrics::counter!("kuzz_global_reads_total", "ask" => label, "outcome" => outcome).increment(1);
    answer
}

/// D5: whether the primary says yes to a fact the copy said no to. `false`
/// when it says no or cannot be asked (the copy's "no" stands).
pub async fn primary_confirms(state: &AppState, community: CommunityId, read: &GlobalRead) -> bool {
    ask(state, Some(community), read)
        .await
        .is_some_and(|answer| answer.yes)
}

/// D6: a "yes" from a lagging copy, checked with the primary. `Ok(true)` when
/// the copy is current or the primary agrees; `Err(())` when the primary
/// cannot answer (the caller fails closed).
pub async fn confirm_when_lagging(
    state: &AppState,
    community: CommunityId,
    read: &GlobalRead,
) -> Result<bool, ()> {
    if !copy_lagging(state) {
        return Ok(true);
    }
    ask(state, Some(community), read)
        .await
        .map(|answer| answer.yes)
        .ok_or(())
}

/// Relay membership: the copy's answer, checked with the primary when it is
/// no (D5) or when the copy lags (D6). An error fails closed.
pub async fn relay_member(
    state: &AppState,
    community: CommunityId,
    pubkey_hex: &str,
    here: bool,
) -> Result<bool, String> {
    if !state.config.global_copy {
        return Ok(here);
    }
    let read = GlobalRead::RelayMember {
        pubkey: pubkey_hex.to_owned(),
    };
    if !here {
        return Ok(primary_confirms(state, community, &read).await);
    }
    confirm_when_lagging(state, community, &read)
        .await
        .map_err(|()| "the primary region cannot confirm membership while the copy lags".to_owned())
}

/// The ban and timeout state of `pubkey`: from the copy, or from the primary
/// while the copy lags (D6). An error fails closed.
pub async fn restriction_state(
    state: &AppState,
    community: CommunityId,
    pubkey: &[u8],
) -> anyhow::Result<RestrictionState> {
    let here = state
        .db
        .moderation_restriction_state(community, pubkey)
        .await?;
    if !copy_lagging(state) || here.banned {
        return Ok(here);
    }
    let read = GlobalRead::Restriction {
        pubkey: hex::encode(pubkey),
    };
    let answer = ask(state, Some(community), &read).await.ok_or_else(|| {
        anyhow::anyhow!("the primary region cannot confirm restrictions while the copy lags")
    })?;
    Ok(RestrictionState {
        banned: answer.yes,
        muted_until: answer
            .muted_until
            .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0))
            .max(here.muted_until),
    })
}

/// D5: a token missing from the copy that the primary knows is waited for
/// here. Returns the row once copied; `Ok(None)` when the primary does not
/// know it (or cannot be asked); `Err` when it does but the copy did not
/// bring it in time.
pub async fn wait_for_token(
    state: &AppState,
    hash: &[u8; 32],
) -> Result<Option<kuzz_db::identity::AccessTokenRecord>, ()> {
    if state.global_reads.unknown_tokens.contains_key(hash) {
        return Ok(None);
    }
    let read = GlobalRead::TokenKnown {
        hash: hex::encode(hash),
    };
    match ask(state, None, &read).await {
        Some(answer) if answer.yes => {}
        Some(_) => {
            state.global_reads.unknown_tokens.insert(*hash, ());
            return Ok(None);
        }
        None => return Ok(None),
    }
    let deadline = tokio::time::Instant::now() + TOKEN_WAIT;
    loop {
        tokio::time::sleep(TOKEN_POLL).await;
        match state.db.lookup_access_token(hash).await {
            Ok(Some(record)) => return Ok(Some(record)),
            Ok(None) if tokio::time::Instant::now() < deadline => {}
            Ok(None) | Err(_) => return Err(()),
        }
    }
}

fn answer_error(status: StatusCode, kind: &str, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": kind, "message": message })))
}

/// Primary side: `POST /internal/region/global-read`.
pub async fn global_read(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let read = serde_json::from_slice::<GlobalRead>(&body).ok();
    // A token belongs to no community; everything else to the call's. The
    // body is covered by the attestation, so it may choose.
    let tenant = match &read {
        Some(GlobalRead::TokenKnown { .. }) => {
            crate::region_link::admit_deployment(&state, &Method::POST, &uri, &headers, &body)
                .await
                .map(|_| None)
        }
        _ => crate::region_link::admit(&state, &Method::POST, &uri, &headers, &body)
            .await
            .map(|call| Some(call.tenant)),
    };
    let tenant = match tenant {
        Ok(tenant) => tenant,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a global read");
            return answer_error(StatusCode::UNAUTHORIZED, "refused", "region link refused");
        }
    };
    let Some(read) = read else {
        return answer_error(StatusCode::BAD_REQUEST, "malformed", "malformed read");
    };
    if state.config.global_copy || state.config.region_id != state.config.primary_region {
        return answer_error(
            StatusCode::CONFLICT,
            "not_primary",
            "not the primary region",
        );
    }
    match answer(&state, tenant.as_ref(), read).await {
        Ok(Some(answer)) => (StatusCode::OK, Json(json!(answer))),
        Ok(None) => answer_error(StatusCode::BAD_REQUEST, "malformed", "malformed read"),
        Err(error) => {
            tracing::warn!(%error, "A region's global read failed");
            answer_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
        }
    }
}

/// Answer a read here. `Ok(None)`: malformed.
async fn answer(
    state: &AppState,
    tenant: Option<&TenantContext>,
    read: GlobalRead,
) -> anyhow::Result<Option<ReadAnswer>> {
    let pubkey = |hex: &str| nostr::PublicKey::from_hex(hex).ok();
    let yes = |yes| {
        Some(ReadAnswer {
            yes,
            muted_until: None,
        })
    };
    if let GlobalRead::TokenKnown { hash } = &read {
        let Some(hash) = hex::decode(hash)
            .ok()
            .and_then(|hash| <[u8; 32]>::try_from(hash).ok())
        else {
            return Ok(None);
        };
        return Ok(yes(state.db.lookup_access_token(&hash).await?.is_some()));
    }
    let Some(community) = tenant.map(TenantContext::community) else {
        return Ok(None);
    };
    Ok(match read {
        GlobalRead::TokenKnown { .. } => None,
        GlobalRead::RelayMember { pubkey: key } => {
            let Some(key) = pubkey(&key) else {
                return Ok(None);
            };
            yes(state
                .db
                .is_relay_member_writer(community, &key.to_hex())
                .await?)
        }
        GlobalRead::Allowed { pubkey: key } => {
            let Some(key) = pubkey(&key) else {
                return Ok(None);
            };
            yes(state
                .db
                .is_pubkey_allowed(community, key.as_bytes())
                .await?)
        }
        GlobalRead::AgentOwner { agent, owner } => {
            let (Some(agent), Some(owner)) = (pubkey(&agent), pubkey(&owner)) else {
                return Ok(None);
            };
            yes(state
                .db
                .is_agent_owner(community, agent.as_bytes(), owner.as_bytes())
                .await?)
        }
        GlobalRead::Restriction { pubkey: key } => {
            let Some(key) = pubkey(&key) else {
                return Ok(None);
            };
            let restriction = state
                .db
                .moderation_restriction_state(community, key.as_bytes())
                .await?;
            Some(ReadAnswer {
                yes: restriction.banned,
                muted_until: restriction.muted_until.map(|until| until.timestamp()),
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_travel_as_tagged_questions() {
        let read = GlobalRead::AgentOwner {
            agent: "aa".into(),
            owner: "bb".into(),
        };
        let value = serde_json::to_value(&read).expect("json");
        assert_eq!(value["ask"], "agent_owner");
        assert_eq!(
            serde_json::from_value::<GlobalRead>(value).expect("back"),
            read
        );
        let answer: ReadAnswer = serde_json::from_value(json!({ "yes": true })).expect("answer");
        assert_eq!(
            answer,
            ReadAnswer {
                yes: true,
                muted_until: None
            }
        );
    }
}
