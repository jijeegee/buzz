//! Relay-signed kind:9005 deletion records (see `kuzz_db::deletion_record`).
//!
//! The relay owns the signing key; the store writes the record inside the
//! deleting (or a follow-up) transaction. Records are published to the channel
//! only after their transaction committed and only when one was inserted.

use std::sync::Arc;

use kuzz_core::tenant::TenantContext;
use kuzz_core::StoredEvent;
use kuzz_db::deletion_record::{
    is_deletion_record_content_kind, is_relay_deletion_record, AdminDeleteOutcome,
};
use kuzz_pubsub::EventTopic;
use nostr::Event;
use tracing::warn;
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

/// Fan a committed record out to its channel. Best effort: clients also get it
/// from reconnect repair, since it is stored with a current `created_at`.
pub(crate) async fn publish_deletion_record(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    record: &Event,
) {
    let Some(channel_id) = record_channel(record) else {
        return;
    };
    if let Err(error) = state
        .pubsub
        .publish_event(tenant, EventTopic::Channel(channel_id), record)
        .await
    {
        warn!(%error, "deletion record fan-out failed");
    }
}

/// After a kind:5 soft-deleted `target`, record it when the signer is not the
/// target's raw author: device stores honour a kind:5 only from that pubkey.
///
/// The deletion is already committed and stays; a failed record write is
/// logged and left to `kuzz-admin backfill-deletion-records`.
pub(crate) async fn record_non_author_deletion(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    deletion: &Event,
    target: &StoredEvent,
) {
    if deletion.pubkey == target.event.pubkey
        || target.channel_id.is_none()
        || !is_deletion_record_content_kind(i32::from(target.event.kind.as_u16()))
    {
        return;
    }
    match state
        .db
        .write_deletion_record(
            tenant.community(),
            target.event.id.as_bytes(),
            &state.relay_keypair,
        )
        .await
    {
        Ok(Some(record)) => publish_deletion_record(tenant, state, &record).await,
        Ok(None) => {}
        Err(error) => {
            count_record_failure("kind5");
            warn!(
                %error,
                target = %target.event.id.to_hex(),
                "deletion record write failed; the backfill command repairs it"
            );
        }
    }
}

/// After a staff delete: publish the committed record, or log that the delete
/// committed without one (the backfill command repairs it).
pub(crate) async fn finish_staff_delete(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    outcome: &AdminDeleteOutcome,
    target: &[u8],
) {
    if let Some(record) = &outcome.record {
        publish_deletion_record(tenant, state, record).await;
    }
    if outcome.record_failed {
        count_record_failure("staff");
        warn!(
            target = %hex::encode(target),
            "staff delete committed without its deletion record; the backfill command repairs it"
        );
    }
}

fn count_record_failure(path: &'static str) {
    metrics::counter!("kuzz_deletion_record_failures_total", "path" => path).increment(1);
}

fn record_channel(record: &Event) -> Option<Uuid> {
    record
        .tags
        .iter()
        .find(|tag| tag.kind().to_string() == "h")
        .and_then(|tag| tag.content()?.parse().ok())
}
