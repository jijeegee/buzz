//! Relay-signed kind:9005 deletion records (see `kuzz_db::deletion_record`).
//!
//! The relay owns the signing key; the store writes the record inside the
//! deleting transaction, and a delete whose record cannot be stored does not
//! happen. Records are published to the channel only after their transaction
//! committed and only when one was inserted.

use std::sync::Arc;

use kuzz_core::tenant::TenantContext;
use kuzz_core::StoredEvent;
use kuzz_db::deletion_record::{
    is_deletion_record_kind, is_relay_deletion_record, AdminDeleteOutcome,
};
use kuzz_pubsub::EventTopic;
use nostr::Event;
use uuid::Uuid;

use crate::state::AppState;

/// Reject deleting a relay-signed deletion record: it is the removed row's only
/// replay record, so neither a channel admin's 9005 nor a kind:5 may erase it.
pub(crate) fn ensure_not_relay_record(target: &Event, state: &AppState) -> anyhow::Result<()> {
    if is_relay_deletion_record(target, &state.relay_keypair.public_key()) {
        anyhow::bail!("relay deletion records cannot be deleted");
    }
    Ok(())
}

/// Fan a committed record out to its channel, locally and through Redis.
/// Callers pass only a record their transaction inserted, so a retry that
/// found the record already stored delivers nothing again. Best effort:
/// clients also get it from reconnect repair, since it is stored with a
/// current `created_at`.
pub(crate) async fn publish_deletion_record(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    record: &Event,
) {
    let Some(channel_id) = record_channel(record) else {
        return;
    };
    let stored = StoredEvent::new(record.clone(), Some(channel_id));
    crate::handlers::side_effects::publish_relay_event(
        tenant,
        state,
        EventTopic::Channel(channel_id),
        &stored,
    )
    .await;
}

/// Whether a kind:5 `deletion` of `target` needs a relay record: device
/// stores honour a kind:5 only from the target's raw author, so another
/// signer's (agent owner, actor) removal of a channel row or overlay needs one.
pub(crate) fn kind5_needs_record(deletion: &Event, target: &StoredEvent) -> bool {
    deletion.pubkey != target.event.pubkey
        && target.channel_id.is_some()
        && is_deletion_record_kind(i32::from(target.event.kind.as_u16()))
}

/// Count a delete that was due a record and failed (nothing was deleted).
pub(crate) fn count_record_failure(path: &'static str) {
    metrics::counter!("kuzz_deletion_record_failures_total", "path" => path).increment(1);
}

/// After a staff delete: publish the record committed with it, if any.
pub(crate) async fn finish_staff_delete(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    outcome: &AdminDeleteOutcome,
) {
    if let Some(record) = &outcome.record {
        publish_deletion_record(tenant, state, record).await;
    }
}

fn record_channel(record: &Event) -> Option<Uuid> {
    record
        .tags
        .iter()
        .find(|tag| tag.kind().to_string() == "h")
        .and_then(|tag| tag.content()?.parse().ok())
}
