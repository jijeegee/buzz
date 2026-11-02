//! Removal notices for a report kick: the `member_removed` system message
//! (kind 40099) in the room and the member-removed notification (kind 44101)
//! to the kicked user, the same pair a kind:9001 removal publishes.
//!
//! Sent only after [`apply_kick_live_side_effects`] returned
//! [`KickOutcome::Enforced`], so after the membership fence committed and gave
//! its pooled connection back: these inserts take their own connections, and
//! the 40099 insert takes the same channel lock as the fence. A member re-added
//! before the fence gets nothing. A re-add that commits after the fence sends
//! its own later 44100 and `member_joined`, so the room's history stays truthful.
//!
//! Both events use a stable `created_at` from the action row, so a driver retry
//! (`report_resolution`, "worker will retry") produces the same event ids. The
//! store dedupes them and a duplicate is never fanned out again.
//!
//! [`apply_kick_live_side_effects`]: super::side_effects::apply_kick_live_side_effects
//! [`KickOutcome::Enforced`]: super::side_effects::KickOutcome::Enforced

use std::sync::Arc;

use chrono::{DateTime, Utc};
use kuzz_core::kind::KIND_MEMBER_REMOVED_NOTIFICATION;
use kuzz_core::tenant::TenantContext;
use uuid::Uuid;

use super::side_effects::{
    emit_membership_notification_stamped, emit_system_message, NotificationStamp,
};
use crate::state::AppState;

/// Oldest `created_at` the notices may carry, in seconds before now.
///
/// Channel rows must commit within the store's `created_at` floor (960 s, the
/// replica fence). A recovery that runs later than this uses the current time:
/// that keeps the retry able to commit, at the cost that a second recovery run
/// after an already-committed late one could post a second line.
const MAX_NOTICE_AGE_SECS: i64 = 900;

/// The `created_at` both notices carry for an action created at `action_created_at`.
pub(crate) fn notice_timestamp(
    action_created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    if now - action_created_at > chrono::Duration::seconds(MAX_NOTICE_AGE_SECS) {
        now
    } else {
        action_created_at
    }
}

/// Publish the removal notices for an enforced report kick. Propagates an
/// insert failure so the action stays retryable; delivery stays best effort.
pub(crate) async fn send_report_kick_notices(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    channel_id: Uuid,
    target: &[u8],
    actor: &[u8],
    action_created_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let at = notice_timestamp(action_created_at, Utc::now());
    emit_system_message(
        tenant,
        state,
        channel_id,
        serde_json::json!({
            "type": "member_removed",
            "actor": hex::encode(actor),
            "target": hex::encode(target),
        }),
        at,
    )
    .await?;
    emit_membership_notification_stamped(
        tenant,
        state,
        channel_id,
        target,
        actor,
        KIND_MEMBER_REMOVED_NOTIFICATION,
        NotificationStamp::At(at),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_timestamp_is_the_action_time_inside_the_floor() {
        let now = Utc::now();
        let recent = now - chrono::Duration::seconds(MAX_NOTICE_AGE_SECS);
        assert_eq!(notice_timestamp(recent, now), recent);
        let stale = now - chrono::Duration::seconds(MAX_NOTICE_AGE_SECS + 1);
        assert_eq!(notice_timestamp(stale, now), now);
    }
}
