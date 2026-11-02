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
//! Both notices are stamped with the current time and carry the tag
//! `["action", <action id>]`. The store inserts each one only when no notice of
//! that kind with that tag exists ([`kuzz_db::Db::insert_event_once_per_action`]),
//! so a driver retry (`report_resolution`, "worker will retry"), a recovery
//! re-drive after a crash before finalize, or two drivers at once never post a
//! second notice, however late they run. Stamping "now" also keeps a late
//! notice inside the `since` of subscriptions opened after the action.
//!
//! Like the outbox `admin_kick` line, neither notice names the operator.
//!
//! [`apply_kick_live_side_effects`]: super::side_effects::apply_kick_live_side_effects
//! [`KickOutcome::Enforced`]: super::side_effects::KickOutcome::Enforced

use std::sync::Arc;

use chrono::{DateTime, Utc};
use kuzz_core::kind::KIND_MEMBER_REMOVED_NOTIFICATION;
use kuzz_core::tenant::TenantContext;
use kuzz_pubsub::EventTopic;
use nostr::{EventBuilder, Kind, Tag};
use uuid::Uuid;

use super::side_effects::{
    emit_membership_notification_stamped, publish_relay_event, NotificationStamp,
};
use crate::state::AppState;

/// Tag name carrying the admin action id on its notices.
pub(crate) const ACTION_TAG: &str = "action";

/// The `member_removed` line for a report kick. No `actor`, like `admin_kick`.
fn member_removed_content(target: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "type": "member_removed",
        "target": hex::encode(target),
    })
}

/// Publish the removal notices for an enforced report kick. Propagates an
/// insert failure so the action stays retryable; delivery stays best effort.
pub(crate) async fn send_report_kick_notices(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    channel_id: Uuid,
    target: &[u8],
    action_id: Uuid,
    action_created_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let action = action_id.to_string();
    let event = EventBuilder::new(
        Kind::Custom(40099),
        member_removed_content(target).to_string(),
    )
    .tags([
        Tag::parse(["h", &channel_id.to_string()])?,
        Tag::parse([ACTION_TAG, &action])?,
    ])
    .sign_with_keys(&state.relay_keypair)
    .map_err(|e| anyhow::anyhow!("failed to sign member_removed line: {e}"))?;
    let stored = state
        .db
        .insert_event_once_per_action(
            tenant.community(),
            &event,
            Some(channel_id),
            &action,
            action_created_at,
        )
        .await
        .map_err(|e| anyhow::anyhow!("member_removed line insert failed: {e}"))?;
    if let Some(stored) = stored {
        publish_relay_event(tenant, state, EventTopic::Channel(channel_id), &stored).await;
    }
    emit_membership_notification_stamped(
        tenant,
        state,
        channel_id,
        target,
        &[],
        KIND_MEMBER_REMOVED_NOTIFICATION,
        NotificationStamp::ForAction {
            action_id: action,
            action_created_at,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_removed_line_does_not_name_the_operator() {
        let content = member_removed_content(&[0x31; 32]);
        assert_eq!(content["type"], "member_removed");
        assert_eq!(content["target"], "31".repeat(32));
        assert!(content.get("actor").is_none());
    }
}
