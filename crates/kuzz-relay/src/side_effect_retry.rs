//! Background retry of side effects that failed or were cut off (outbox rows,
//! migration 0077). Runs only with `KUZZ_SIDE_EFFECT_RETRY` on, and only for
//! kinds safe to replay ([`side_effect_retry_safe`]).
//!
//! [`side_effect_retry_safe`]: crate::handlers::side_effects::side_effect_retry_safe

use std::sync::Arc;
use std::time::Duration;

use kuzz_core::tenant::TenantContext;
use tracing::{error, warn};

use crate::handlers::side_effects::{run_outboxed_side_effects, SIDE_EFFECT_RETRY_KINDS};
use crate::state::AppState;

/// How often the worker looks for due rows.
pub const SIDE_EFFECT_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// Rows claimed per pass.
const SIDE_EFFECT_RETRY_BATCH: i64 = 50;

/// Retry every due outbox row once. Returns how many rows were claimed.
pub async fn retry_due_side_effects(state: &Arc<AppState>) -> usize {
    let claimed = match state
        .db
        .claim_due_side_effects(&SIDE_EFFECT_RETRY_KINDS, SIDE_EFFECT_RETRY_BATCH)
        .await
    {
        Ok(claimed) => claimed,
        Err(e) => {
            warn!("side-effect retry claim failed: {e}");
            return 0;
        }
    };
    for row in &claimed {
        let host = match state.db.lookup_community_host(row.community_id).await {
            Ok(Some(host)) => host,
            // Archived or deleting community: leave the row; its lease
            // backs off like a failure.
            Ok(None) => {
                let _ = state
                    .db
                    .record_side_effect_failure(
                        row.community_id,
                        &row.event_id,
                        "community inactive",
                    )
                    .await;
                continue;
            }
            Err(e) => {
                warn!("side-effect retry host lookup failed: {e}");
                continue;
            }
        };
        let stored = match state
            .db
            .get_event_by_id_for_event_write(row.community_id, &row.event_id)
            .await
        {
            Ok(stored) => stored,
            Err(e) => {
                warn!("side-effect retry event lookup failed: {e}");
                continue;
            }
        };
        // Deleted or replaced since: nothing left to apply.
        let Some(stored) = stored else {
            if let Err(e) = state
                .db
                .complete_side_effect(row.community_id, &row.event_id)
                .await
            {
                warn!("side-effect retry settle failed: {e}");
            }
            continue;
        };
        let tenant = TenantContext::resolved(row.community_id, host);
        metrics::counter!("kuzz_side_effect_retries_total", "kind" => row.kind.to_string())
            .increment(1);
        if let Err(e) =
            run_outboxed_side_effects(&tenant, row.kind as u32, &stored.event, state).await
        {
            error!(
                event_id = %stored.event.id,
                kind = row.kind,
                attempts = row.attempts,
                "Side effect retry failed: {e}"
            );
        }
    }
    claimed.len()
}

/// Run [`retry_due_side_effects`] forever, and export the pending-row gauge.
pub async fn run_side_effect_retry_loop(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(SIDE_EFFECT_RETRY_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        retry_due_side_effects(&state).await;
        if let Ok(pending) = state.db.count_pending_side_effects().await {
            metrics::gauge!("kuzz_side_effects_pending").set(pending as f64);
        }
    }
}
