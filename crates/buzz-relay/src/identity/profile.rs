//! Server-published kind:0 profiles for principals (plan §3.7).
//!
//! In token mode `principals` is the source of truth for names and avatars;
//! clients never publish kind:0 themselves. The relay stamps a kind:0 for the
//! principal (pubkey = principal id, sentinel signature) into communities the
//! principal takes part in, so existing clients — which already subscribe to
//! and cache kind:0 — render names and avatars unchanged.
//!
//! Every publish runs under a per-(community, principal) advisory lock and
//! re-reads the principal row inside it, so concurrent publishers (a profile
//! PATCH, an AUTH reconcile, a second PATCH) serialize and the last one always
//! publishes the newest profile; an event is written only when the
//! community's newest kind:0 differs from it.
//!
//! Triggers:
//! - [`spawn_publish_everywhere`] after a profile write commits
//!   (`PATCH /auth/profile`, bot create, `PATCH /auth/bots/{id}/profile`):
//!   every community whose `users` projection holds the principal.
//! - [`spawn_reconcile`] with [`Scope::Member`] after a token WS AUTH and after
//!   a same-connection re-AUTH (≤ 1 h): repairs a failed or missed publish,
//!   but only where the principal already has a `users` row — a principal
//!   that merely reads a community (e.g. repo browsing) is not announced.
//! - [`note_token_write`] after a token principal's first accepted write in
//!   a community ([`Scope::Participant`]): writing is taking part, so the
//!   profile is published there (and the kind:0 side effect creates the
//!   `users` row). De-duplicated per process.

use std::sync::Arc;
use std::time::Duration;

use buzz_core::kind::event_kind_u32;
use buzz_core::principal::{PrincipalId, PrincipalKind};
use buzz_core::tenant::{CommunityId, TenantContext};
use buzz_db::identity::{PrincipalRecord, ProfileEventState, ProfilePublishSnapshot};
use futures_util::StreamExt as _;
use serde_json::{json, Value};

use crate::state::AppState;

/// Communities published to concurrently by one fan-out.
const FAN_OUT_CONCURRENCY: usize = 4;
/// How long a (community, principal) write trigger is remembered.
const WRITE_TRIGGER_TTL: Duration = Duration::from_secs(600);
/// Bound on remembered write triggers per process.
const WRITE_TRIGGER_CAPACITY: u64 = 100_000;

/// Process-wide bound on concurrent kind:0 publishes.
///
/// A publish holds one pool connection for its lock transaction and needs a
/// second for the insert and side effects; waiters on the same key each pin
/// their lock connection too. Unbounded (every AUTH, re-AUTH and first write
/// spawns one), a reconnect storm could hold every pool connection while each
/// waits for a second — starving ingest, REQ and auth. Publishers therefore
/// take a permit *before* any connection, and at most a quarter of the pool
/// is ever pinned by publishing.
#[derive(Default)]
pub(crate) struct PublishSlots(std::sync::OnceLock<Arc<tokio::sync::Semaphore>>);

impl PublishSlots {
    /// The semaphore, sized `max(1, pool_max / 4)` on first use.
    pub(crate) fn semaphore(&self, pool_max: u32) -> Arc<tokio::sync::Semaphore> {
        Arc::clone(self.0.get_or_init(|| {
            let permits = usize::try_from(pool_max / 4).unwrap_or(1).max(1);
            Arc::new(tokio::sync::Semaphore::new(permits))
        }))
    }
}

/// Where a reconcile may publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// Only where the principal already has a `users` row.
    Member,
    /// Unconditionally (the principal wrote in this community).
    Participant,
}

/// Per-process memory of write-triggered reconciles, so every write does not
/// cost a profile check.
pub(crate) struct WriteTriggers(moka::sync::Cache<(CommunityId, PrincipalId), ()>);

impl Default for WriteTriggers {
    fn default() -> Self {
        Self(
            moka::sync::Cache::builder()
                .max_capacity(WRITE_TRIGGER_CAPACITY)
                .time_to_live(WRITE_TRIGGER_TTL)
                .build(),
        )
    }
}

/// The kind:0 content the relay publishes for `record`.
pub(crate) fn profile_metadata(record: &PrincipalRecord) -> Value {
    let mut metadata = json!({
        "name": record.display_name,
        "display_name": record.display_name,
    });
    if let Some(avatar) = record.avatar_url.as_deref() {
        metadata["picture"] = json!(avatar);
    }
    if record.kind == PrincipalKind::Bot {
        metadata["bot"] = json!(true);
    }
    metadata
}

/// Whether the community's newest kind:0 already says `desired`.
fn is_current(existing: Option<&ProfileEventState>, desired: &Value) -> bool {
    existing
        .and_then(|state| serde_json::from_str::<Value>(&state.content).ok())
        .is_some_and(|content| &content == desired)
}

/// `created_at` for a new kind:0: now, but strictly after the newest existing
/// one so a same-second edit is not lost to the NIP-01 lowest-id tie-break.
/// Safe only because the caller holds the publish lock (no stale publisher
/// can run after a newer one).
fn next_created_at(existing: Option<&ProfileEventState>, now: i64) -> i64 {
    existing.map_or(now, |state| now.max(state.created_at + 1))
}

/// What to publish, given the snapshot read under the lock; `None` = nothing.
fn desired_profile(snapshot: &ProfilePublishSnapshot, scope: Scope) -> Option<Value> {
    let record = snapshot.record.as_ref()?;
    if record.disabled_at.is_some() || record.kind == PrincipalKind::Relay {
        return None;
    }
    if scope == Scope::Member && !snapshot.has_user_row && snapshot.latest.is_none() {
        return None;
    }
    Some(profile_metadata(record))
}

/// Publish `principal`'s current kind:0 into one community when it is not
/// already current there. Returns whether a new event was stored.
pub(crate) async fn publish_profile_in_community(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    principal: &PrincipalId,
    scope: Scope,
) -> anyhow::Result<bool> {
    // Permit first, connections second; held until the lock is released.
    let _slot = state
        .identity
        .publish_slots
        .semaphore(state.db.pool().options().get_max_connections())
        .acquire_owned()
        .await
        .map_err(|_| anyhow::anyhow!("kind:0 publish slots closed"))?;
    let (lock, snapshot) = state
        .db
        .lock_principal_profile_publish(tenant.community(), principal)
        .await?;
    let Some(desired) = desired_profile(&snapshot, scope) else {
        lock.release().await?;
        return Ok(false);
    };
    // Content equality proves storage, not completion of the projection. A
    // committed kind:0 is also the durable retry record for its side effects.
    if let Some(latest) = snapshot
        .latest
        .as_ref()
        .filter(|_| is_current(snapshot.latest.as_ref(), &desired))
    {
        let stored = state
            .db
            .get_event_by_id_for_event_write(tenant.community(), &latest.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("profile event disappeared under publish lock"))?;
        // Replay only the idempotent projection. Re-dispatching an already
        // stored event would duplicate event-created audits and workflow runs
        // on every AUTH; the canonical event remains available to REQ.
        crate::handlers::side_effects::handle_side_effects(tenant, 0, &stored.event, state).await?;
        lock.release().await?;
        return Ok(false);
    }
    let created_at = next_created_at(snapshot.latest.as_ref(), chrono::Utc::now().timestamp());
    let draft = json!({
        "kind": 0,
        "created_at": created_at,
        "tags": [],
        "content": desired.to_string(),
    });
    // The server chose `created_at` (possibly bumped past a future-dated
    // predecessor so LWW holds); it is authoritative, not a client clock.
    let event = buzz_core::draft::stamp_draft(&draft, principal, created_at)
        .map_err(|error| anyhow::anyhow!("kind:0 stamp failed: {error:?}"))?;
    let (stored, inserted) = state
        .db
        .replace_addressable_event(tenant.community(), &event, None)
        .await?;
    if inserted {
        let kind = event_kind_u32(&stored.event);
        // Project into `users` exactly as a client kind:0 does today; still
        // under the lock so a later publisher sees the projection too.
        crate::handlers::side_effects::handle_side_effects(tenant, kind, &stored.event, state)
            .await?;
        crate::handlers::event::dispatch_persistent_event(
            tenant,
            state,
            &stored,
            kind,
            &principal.to_hex(),
            None,
        )
        .await;
    }
    lock.release().await?;
    Ok(inserted)
}

async fn reconcile(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    principal: &PrincipalId,
    scope: Scope,
) -> bool {
    match publish_profile_in_community(state, tenant, principal, scope).await {
        Ok(_) => true,
        Err(error) => {
            metrics::counter!("buzz_auth_profile_publish_failures_total").increment(1);
            tracing::warn!(%principal, community = %tenant.community(), %error,
                "kind:0 publish failed; reconciled on the next auth or re-auth");
            false
        }
    }
}

/// Publish `principal`'s profile into every community whose `users`
/// projection holds it, a few at a time. Failures are logged and converge on
/// the principal's next AUTH or re-AUTH there.
pub(crate) async fn publish_profile_everywhere(state: Arc<AppState>, principal: PrincipalId) {
    let communities = match state.db.principal_profile_communities(&principal).await {
        Ok(communities) => communities,
        Err(error) => {
            tracing::warn!(%principal, %error, "kind:0 publish: community lookup failed");
            return;
        }
    };
    futures_util::stream::iter(communities)
        .for_each_concurrent(FAN_OUT_CONCURRENCY, |target| {
            let state = Arc::clone(&state);
            async move {
                let tenant = TenantContext::resolved(target.community, target.host);
                reconcile(&state, &tenant, &principal, Scope::Member).await;
            }
        })
        .await;
}

/// Spawn [`publish_profile_everywhere`]; the profile write already committed,
/// so the HTTP response does not wait on the fan-out.
pub(crate) fn spawn_publish_everywhere(state: &Arc<AppState>, principal: PrincipalId) {
    tokio::spawn(publish_profile_everywhere(Arc::clone(state), principal));
}

/// Spawn a reconcile of `principal`'s kind:0 in `tenant`'s community.
pub(crate) fn spawn_reconcile(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    principal: PrincipalId,
    scope: Scope,
) {
    let state = Arc::clone(state);
    let tenant = tenant.clone();
    tokio::spawn(async move {
        reconcile(&state, &tenant, &principal, scope).await;
    });
}

/// A token principal's write was accepted in `tenant`: publish its profile
/// there once per process window ([`Scope::Participant`]).
pub(crate) fn note_token_write(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    principal: PrincipalId,
) {
    let key = (tenant.community(), principal);
    if state.identity.write_triggers.0.contains_key(&key) {
        return;
    }
    state.identity.write_triggers.0.insert(key, ());
    let state = Arc::clone(state);
    let tenant = tenant.clone();
    tokio::spawn(async move {
        if !reconcile(&state, &tenant, &principal, Scope::Participant).await {
            // Let the next write retry.
            state.identity.write_triggers.0.invalidate(&key);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: PrincipalKind, avatar: Option<&str>) -> PrincipalRecord {
        PrincipalRecord {
            id: PrincipalId::generate(),
            kind,
            display_name: "Ada".into(),
            avatar_url: avatar.map(str::to_owned),
            username: None,
            disabled_at: None,
            purge_after: None,
        }
    }

    fn snapshot(record: PrincipalRecord, has_user_row: bool) -> ProfilePublishSnapshot {
        ProfilePublishSnapshot {
            record: Some(record),
            latest: None,
            has_user_row,
        }
    }

    #[test]
    fn metadata_carries_name_picture_and_bot_flag() {
        let user = profile_metadata(&record(PrincipalKind::User, Some("https://a/x.png")));
        assert_eq!(user["name"], "Ada");
        assert_eq!(user["display_name"], "Ada");
        assert_eq!(user["picture"], "https://a/x.png");
        assert!(user.get("bot").is_none());
        let bot = profile_metadata(&record(PrincipalKind::Bot, None));
        assert_eq!(bot["bot"], true);
        assert!(bot.get("picture").is_none());
    }

    #[test]
    fn current_only_when_content_matches() {
        let desired = json!({"name": "Ada", "display_name": "Ada"});
        let same = ProfileEventState {
            id: vec![],
            created_at: 10,
            content: r#"{"display_name":"Ada","name":"Ada"}"#.into(),
        };
        let stale = ProfileEventState {
            id: vec![],
            created_at: 10,
            content: r#"{"name":"Old"}"#.into(),
        };
        assert!(is_current(Some(&same), &desired));
        assert!(!is_current(Some(&stale), &desired));
        assert!(!is_current(None, &desired));
    }

    #[test]
    fn created_at_moves_past_a_same_second_predecessor() {
        let existing = ProfileEventState {
            id: vec![],
            created_at: 100,
            content: String::new(),
        };
        assert_eq!(next_created_at(Some(&existing), 100), 101);
        assert_eq!(next_created_at(Some(&existing), 200), 200);
        assert_eq!(next_created_at(None, 50), 50);
    }

    /// The publish decision skips disabled principals, the relay principal,
    /// and — for member-scoped reconciles — communities without a `users`
    /// row; a participant (writer) is published regardless.
    #[test]
    fn publish_decision_skips_disabled_relay_and_non_members() {
        let user = record(PrincipalKind::User, None);
        assert!(desired_profile(&snapshot(user.clone(), true), Scope::Member).is_some());
        assert!(desired_profile(&snapshot(user.clone(), false), Scope::Member).is_none());
        assert!(desired_profile(&snapshot(user.clone(), false), Scope::Participant).is_some());

        let mut disabled = user.clone();
        disabled.disabled_at = Some(chrono::Utc::now());
        assert!(desired_profile(&snapshot(disabled, true), Scope::Participant).is_none());

        let relay = record(PrincipalKind::Relay, None);
        assert!(desired_profile(&snapshot(relay, true), Scope::Participant).is_none());

        let missing = ProfilePublishSnapshot {
            record: None,
            latest: None,
            has_user_row: true,
        };
        assert!(desired_profile(&missing, Scope::Participant).is_none());
    }
}
