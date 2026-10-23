//! Live delivery of room-less events in a copy region (global regions stage
//! S5-3c, decision E6).
//!
//! Room-less events (profiles, contact lists, settings, membership notices,
//! gift wraps) are stored only in the primary region (S5-3a, S5-3b) and
//! reach a copy region's database through the global copy. The copy tells
//! the relay: a trigger on room-less `events` rows, set to fire while the
//! copy is applied, sends `pg_notify` on [`CHANNEL`] with
//! `<community>:<event id>` ([`kuzz_db::Db::guard_global_copy`] installs
//! it). Every relay pod of the region listens ([`run`]), reads the row and
//! sends it to its own subscribers, as it would an event stored here.
//!
//! Delivery waits for the copy (about a second). A notification sent while
//! a pod's listener is reconnecting is not replayed; the app's next query
//! finds the event.

use std::sync::Arc;
use std::time::Duration;

use kuzz_core::tenant::CommunityId;
use sqlx::postgres::PgListener;

use crate::state::AppState;

/// The notification channel the copy's trigger sends on.
pub use kuzz_db::global_copy::LIVE_CHANNEL as CHANNEL;

/// Wait before listening again after the connection dropped.
const RETRY: Duration = Duration::from_secs(5);

/// Listen for copied room-less events and send them to this pod's
/// subscribers. Runs for the life of the relay in a copy region.
pub async fn run(state: Arc<AppState>) {
    loop {
        if let Err(error) = listen(&state).await {
            metrics::counter!("kuzz_global_live_listener_errors_total").increment(1);
            tracing::warn!(%error, "Copied event listener stopped; listening again");
        }
        tokio::time::sleep(RETRY).await;
    }
}

async fn listen(state: &Arc<AppState>) -> anyhow::Result<()> {
    let mut listener = PgListener::connect_with(state.db.pool()).await?;
    listener.listen(CHANNEL).await?;
    tracing::info!("Listening for copied room-less events");
    loop {
        let notification = listener.recv().await?;
        let Some((community, id)) = parse(notification.payload()) else {
            tracing::warn!("Malformed copied event notification");
            continue;
        };
        deliver(state, community, &id).await;
    }
}

/// `<community uuid>:<event id hex>`.
fn parse(payload: &str) -> Option<(CommunityId, [u8; 32])> {
    let (community, id) = payload.split_once(':')?;
    let community = uuid::Uuid::parse_str(community).ok()?;
    let id: [u8; 32] = hex::decode(id).ok()?.try_into().ok()?;
    Some((CommunityId::from_uuid(community), id))
}

async fn deliver(state: &AppState, community: CommunityId, id: &[u8; 32]) {
    let stored = match state.db.get_event_by_id(community, id).await {
        Ok(Some(stored)) => stored,
        // Deleted or replaced since: nothing to tell.
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, "Reading a copied event failed");
            return;
        }
    };
    metrics::counter!("kuzz_global_live_events_total").increment(1);
    crate::handlers::event::fan_out_event_to_local_subscribers(state, community, &stored).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_name_the_community_and_the_event() {
        let community = uuid::Uuid::new_v4();
        let id = [7u8; 32];
        let payload = format!("{community}:{}", hex::encode(id));
        let (parsed_community, parsed_id) = parse(&payload).expect("parsed");
        assert_eq!(parsed_community, CommunityId::from_uuid(community));
        assert_eq!(parsed_id, id);
        assert!(parse("nonsense").is_none());
        assert!(parse(&format!("{community}:abcd")).is_none());
    }
}
