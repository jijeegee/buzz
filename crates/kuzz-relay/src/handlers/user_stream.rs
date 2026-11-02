//! User stream REQ: one live subscription for all of the account's rooms
//! (`docs/message-sync.md#user-stream`).
//!
//! `["REQ", <sub>, {"#h": ["$joined"], "kinds": <frozen kinds>, "limit": 0}]`
//! is answered with every snapshot part (relay-signed kind 24220, never
//! stored), then EOSE, then live frames; frames that arrive meanwhile wait in
//! the stream's queue ([`StreamHandle`]). The stream is an ordinary scoped
//! registry entry, so fan-out, the access gate, topic retention and teardown
//! are the generic ones.
//!
//! Registration order (plan V2 §3.2): claim the sub id; query A (member rooms
//! on the writer, intersected with a token's channel scope); register the
//! scope and retain its topics; query B (each room's `accepted_sequence`);
//! send the parts, then EOSE, then flush the queue.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use kuzz_pubsub::EventTopic;
use nostr::{EventBuilder, Filter, Kind, SingleLetterTag, Tag};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::connection::ConnectionState;
use crate::protocol::RelayMessage;
use crate::state::AppState;
use crate::subscription::ConnId;
use crate::subscription_stream::{Delivery, StreamHandle};
use crate::user_stream_config::StreamMode;

/// Snapshot event kind (relay-signed, never stored or routed).
pub const SNAPSHOT_KIND: u16 = 24220;
/// The frozen v1 kinds; the REQ must name exactly this set.
pub const FROZEN_KINDS: [u16; 17] = [
    5, 7, 9, 9005, 39005, 40001, 40002, 40003, 40008, 40009, 40099, 45001, 45003, 48100, 48101,
    48102, 48103,
];
/// The `#h` value that marks a stream REQ.
pub const JOINED: &str = "$joined";
/// Most rooms one stream carries; more is refused (chunks instead).
pub const MAX_ROOMS: usize = 5000;
/// Most rooms in one snapshot part.
pub const MAX_ROOMS_PER_PART: usize = 500;
/// Largest seq the clients accept (2^53 − 1).
pub const MAX_SEQ: i64 = (1 << 53) - 1;
/// A second stream REQ on the same sub id within this window is refused.
pub const REPEAT_WINDOW: Duration = Duration::from_secs(5);
/// Upper bound of the pre-EOSE live-frame queue (also ≤ send buffer / 4).
pub const MAX_QUEUED_FRAMES: usize = 250;

/// CLOSED reasons (`closedReasons` in the client fixture).
pub mod reason {
    /// `KUZZ_USER_STREAM` is off.
    pub const OFF: &str = "unsupported: user-stream off";
    /// The account is outside the stream set.
    pub const NOT_ENABLED: &str = "unsupported: user-stream not enabled";
    /// More than [`super::MAX_ROOMS`] member rooms.
    pub const TOO_MANY_ROOMS: &str = "unsupported: user-stream too many rooms";
    /// Kinds differ from the frozen set.
    pub const KINDS: &str = "invalid: user-stream kinds";
    /// Not exactly one `{"#h":["$joined"],"kinds":…,"limit":0}` filter.
    pub const SHAPE: &str = "invalid: user-stream shape";
    /// Same sub id again within 5 s.
    pub const REPEAT: &str = "invalid: user-stream repeat";
    /// A second stream on the connection under another sub id.
    pub const SECOND: &str = "invalid: user-stream second";
    /// The pre-EOSE queue or the send buffer filled.
    pub const OVERFLOW: &str = "error: user-stream overflow";
    /// The relay lost live events (Redis lag).
    pub const LAGGED: &str = "error: user-stream lagged";
    /// The relay could not keep the stream correct.
    pub const CLOSED: &str = "error: user-stream closed";
}

/// Whether a REQ is a user-stream REQ: any filter whose `#h` names `$joined`.
pub fn is_stream_req(filters: &[Filter]) -> bool {
    let h = SingleLetterTag::lowercase(nostr::Alphabet::H);
    filters.iter().any(|filter| {
        filter
            .generic_tags
            .get(&h)
            .is_some_and(|values| values.contains(JOINED))
    })
}

/// The filter the stream is registered under: the frozen kinds.
pub fn stream_filter() -> Filter {
    Filter::new().kinds(FROZEN_KINDS.iter().map(|&k| Kind::Custom(k)))
}

/// Check the REQ shape; `Err` is the CLOSED reason.
pub fn check_shape(filters: &[Filter]) -> Result<(), &'static str> {
    let [filter] = filters else {
        return Err(reason::SHAPE);
    };
    let h = SingleLetterTag::lowercase(nostr::Alphabet::H);
    let joined_only = filter.generic_tags.len() == 1
        && filter
            .generic_tags
            .get(&h)
            .is_some_and(|values| values.len() == 1 && values.contains(JOINED));
    if !joined_only
        || filter.limit != Some(0)
        || filter.ids.is_some()
        || filter.authors.is_some()
        || filter.since.is_some()
        || filter.until.is_some()
        || filter.search.is_some()
    {
        return Err(reason::SHAPE);
    }
    let frozen: BTreeSet<Kind> = FROZEN_KINDS.iter().map(|&k| Kind::Custom(k)).collect();
    match &filter.kinds {
        Some(kinds) if *kinds == frozen => Ok(()),
        _ => Err(reason::KINDS),
    }
}

/// Pre-EOSE queue cap for a send buffer of `send_buffer_size` frames.
pub fn queue_cap(send_buffer_size: usize) -> usize {
    MAX_QUEUED_FRAMES.min(send_buffer_size / 4).max(1)
}

/// Snapshot part contents: part 1 with the header, later parts rooms only.
pub fn snapshot_parts(mode: StreamMode, epoch: &str, rooms: &BTreeMap<Uuid, i64>) -> Vec<String> {
    let entries: Vec<(&Uuid, &i64)> = rooms.iter().collect();
    let chunks: Vec<&[(&Uuid, &i64)]> = if entries.is_empty() {
        vec![&[]]
    } else {
        entries.chunks(MAX_ROOMS_PER_PART).collect()
    };
    let parts = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let rooms: serde_json::Map<String, serde_json::Value> = chunk
                .iter()
                .map(|(room, seq)| (room.to_string(), serde_json::Value::from(**seq)))
                .collect();
            if index == 0 {
                // Field order follows the contract text.
                let mut part = serde_json::Map::new();
                part.insert("v".into(), 1.into());
                part.insert("op".into(), "snapshot".into());
                part.insert("part".into(), 1.into());
                part.insert("parts".into(), parts.into());
                part.insert("mode".into(), mode.as_str().into());
                part.insert("epoch".into(), epoch.into());
                part.insert("kinds".into(), FROZEN_KINDS.to_vec().into());
                part.insert("rooms".into(), rooms.into());
                serde_json::Value::Object(part).to_string()
            } else {
                serde_json::json!({ "rooms": rooms }).to_string()
            }
        })
        .collect()
}

/// Handle a stream REQ from an authenticated connection. The caller already
/// answered unauthenticated and out-of-scope REQs, so an `auth-required`
/// refusal never reaches the 5 s repeat rule.
pub async fn handle_stream_req(
    sub_id: String,
    filters: Vec<Filter>,
    pubkey_bytes: Vec<u8>,
    token_channel_ids: Option<Vec<Uuid>>,
    conn: Arc<ConnectionState>,
    state: Arc<AppState>,
) {
    let conn_id = conn.conn_id;
    let previous = state
        .sub_registry
        .note_stream_req(conn_id, &sub_id, Instant::now());

    let config = &state.config.user_stream;
    if config.is_off() {
        refuse(&conn, &sub_id, reason::OFF);
        return;
    }
    let pubkey_hex = hex::encode(&pubkey_bytes);
    let Some(mode) = config.mode_for(&pubkey_hex) else {
        refuse(&conn, &sub_id, reason::NOT_ENABLED);
        return;
    };

    let active = state
        .sub_registry
        .stream_of(conn_id)
        .filter(|handle| handle.is_open());
    if let Some(active) = &active {
        if active.sub_id != sub_id {
            refuse(&conn, &sub_id, reason::SECOND);
            return;
        }
    }
    if previous.is_some_and(|(prev_sub, at)| prev_sub == sub_id && at.elapsed() < REPEAT_WINDOW) {
        // A CLOSED on the stream's own id ends it for the client, so the
        // relay ends it too.
        if let Some(active) = active {
            close_stream(&state, conn_id, &active, Some(reason::REPEAT), false).await;
        } else {
            refuse(&conn, &sub_id, reason::REPEAT);
        }
        return;
    }
    if let Err(reason) = check_shape(&filters) {
        refuse(&conn, &sub_id, reason);
        return;
    }

    let _permit = match conn.nip_fi_gate.acquire_effect().await {
        Ok(permit) => permit,
        Err(crate::nip_fi_gate::SessionExpired) => {
            refuse(&conn, &sub_id, "restricted: authorization denied");
            return;
        }
    };

    // Claim the id: whatever holds it (a chunk REQ, or this stream's earlier
    // REQ after 5 s) is retired first.
    let owner = {
        let mut subs = conn.subscriptions.lock().await;
        if conn.cancel.is_cancelled() {
            return;
        }
        super::close::retire_locked(&mut subs, &sub_id, &conn, &state).await;
        let owner = super::close::next_owner();
        subs.insert(sub_id.clone(), owner);
        owner
    };
    let started = Instant::now();
    let community = conn.tenant.community();

    // Query A: member rooms, on the writer.
    let mut rooms = match state
        .db
        .get_member_channel_ids(community, &pubkey_bytes)
        .await
    {
        Ok(rooms) => rooms,
        Err(e) => {
            warn!(%conn_id, "user stream: member rooms query failed: {e}");
            super::close::close_if_owner(&sub_id, owner, Some(reason::CLOSED), &conn, &state).await;
            return;
        }
    };
    if let Some(allowed) = token_channel_ids.as_deref() {
        rooms.retain(|room| allowed.contains(room));
    }
    rooms.sort_unstable();
    rooms.dedup();
    if rooms.len() > MAX_ROOMS {
        super::close::close_if_owner(&sub_id, owner, Some(reason::TOO_MANY_ROOMS), &conn, &state)
            .await;
        return;
    }

    // Register the scope and retain its topics, under the lifecycle lock.
    let handle = Arc::new(StreamHandle::new(
        sub_id.clone(),
        owner,
        mode,
        conn.tenant.clone(),
        queue_cap(state.config.send_buffer_size),
    ));
    {
        let subs = conn.subscriptions.lock().await;
        if conn.cancel.is_cancelled() || subs.get(&sub_id) != Some(&owner) {
            return;
        }
        let replaced = state.sub_registry.register_stream(
            community,
            conn_id,
            Arc::clone(&handle),
            vec![stream_filter()],
            rooms.clone(),
        );
        for &room in &rooms {
            state
                .pubsub
                .retain_topic(&conn.tenant, EventTopic::Channel(room))
                .await;
        }
        if let Some(replaced) = replaced {
            super::close::release_scope_topics(&state, &conn.tenant, &replaced.scope).await;
        }
        drop(subs);
    }
    spawn_teardown(Arc::clone(&state), conn_id, Arc::clone(&handle));

    // Query B: each room's accepted sequence, after registration, so a frame
    // committed after this read is either queued or already counted.
    let seqs = match state
        .db
        .get_channel_accepted_sequences(community, &rooms)
        .await
    {
        Ok(seqs) => seqs,
        Err(e) => {
            warn!(%conn_id, "user stream: room sequence query failed: {e}");
            close_stream(&state, conn_id, &handle, Some(reason::CLOSED), false).await;
            return;
        }
    };
    let mut snapshot = BTreeMap::new();
    for (room, seq) in seqs {
        if !(0..=MAX_SEQ).contains(&seq) {
            warn!(%conn_id, %room, seq, "user stream: room sequence out of range");
            close_stream(&state, conn_id, &handle, Some(reason::CLOSED), false).await;
            return;
        }
        snapshot.insert(room, seq);
    }

    // Every part, then EOSE, then the queued live frames.
    let me = match Tag::parse(["p", &pubkey_hex]) {
        Ok(tag) => tag,
        Err(e) => {
            warn!(%conn_id, "user stream: p tag: {e}");
            close_stream(&state, conn_id, &handle, Some(reason::CLOSED), false).await;
            return;
        }
    };
    for content in snapshot_parts(mode, &config.epoch, &snapshot) {
        let event = match EventBuilder::new(Kind::Custom(SNAPSHOT_KIND), content)
            .tags([me.clone()])
            .sign_with_keys(&state.relay_keypair)
        {
            Ok(event) => event,
            Err(e) => {
                warn!(%conn_id, "user stream: snapshot signing failed: {e}");
                close_stream(&state, conn_id, &handle, Some(reason::CLOSED), false).await;
                return;
            }
        };
        if !handle.is_open() || !conn.send(RelayMessage::event(&sub_id, &event)) {
            close_stream(&state, conn_id, &handle, None, false).await;
            return;
        }
    }
    if !handle.is_open() || !conn.send(RelayMessage::eose(&sub_id)) {
        close_stream(&state, conn_id, &handle, None, false).await;
        return;
    }
    if !handle.go_live(|frame| state.conn_manager.send_to_text_bytes(conn_id, frame)) {
        close_stream(&state, conn_id, &handle, None, false).await;
        return;
    }
    metrics::histogram!("kuzz_user_stream_snapshot_seconds", "mode" => mode.as_str())
        .record(started.elapsed().as_secs_f64());
    debug!(%conn_id, %sub_id, rooms = snapshot.len(), mode = mode.as_str(), "user stream live");
}

/// Route one fan-out frame to a user stream (queued until its EOSE). `false`
/// when the frame was not handed to the socket. A full pre-EOSE queue closes
/// the stream through its teardown task.
pub fn deliver(
    state: &AppState,
    conn_id: ConnId,
    handle: &StreamHandle,
    frame: Arc<Bytes>,
) -> bool {
    match handle.deliver(frame, |frame| {
        state.conn_manager.send_to_text_bytes(conn_id, frame)
    }) {
        Delivery::Sent(sent) => sent,
        Delivery::Queued => true,
        Delivery::Dropped => false,
        Delivery::Overflow => {
            handle.request_close(reason::OVERFLOW, handle.mode.is_relied_on());
            false
        }
    }
}

/// The stream's teardown task: runs a close requested from a synchronous
/// path (fan-out). A stream retired by the generic paths (CLOSE, replacement,
/// socket end) wakes it too; it then finds the id no longer owned and stops.
fn spawn_teardown(state: Arc<AppState>, conn_id: ConnId, handle: Arc<StreamHandle>) {
    tokio::spawn(async move {
        if let Some((reason, cancel)) = handle.until_closed().await {
            close_stream(&state, conn_id, &handle, Some(reason), cancel).await;
        }
    });
}

/// Retire a stream if it still owns its sub id, send `reason` (if any) and
/// cancel the connection when asked or when the CLOSED could not be queued.
pub async fn close_stream(
    state: &AppState,
    conn_id: ConnId,
    handle: &StreamHandle,
    reason: Option<&str>,
    cancel: bool,
) {
    handle.close();
    let Some(subscriptions) = state.conn_manager.subscriptions_for(conn_id) else {
        return;
    };
    let mut subs = subscriptions.lock().await;
    if subs.get(&handle.sub_id) != Some(&handle.owner) {
        return;
    }
    subs.remove(&handle.sub_id);
    if let Some(removed) = state
        .sub_registry
        .remove_subscription(conn_id, &handle.sub_id)
    {
        super::close::release_scope_topics(state, &handle.tenant, &removed.scope).await;
    }
    let mut sent = true;
    if let Some(reason) = reason {
        sent = state
            .conn_manager
            .send_to(conn_id, RelayMessage::closed(&handle.sub_id, reason));
    }
    if cancel || !sent {
        state.conn_manager.cancel_conn(conn_id);
    }
}

fn refuse(conn: &ConnectionState, sub_id: &str, reason: &str) {
    conn.send(RelayMessage::closed(sub_id, reason));
}

#[cfg(test)]
#[path = "user_stream_tests.rs"]
mod tests;
