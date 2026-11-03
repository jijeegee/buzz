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
//! Registration order (plan V2 §3.2): claim the sub id; register the stream
//! with no rooms; query A (member rooms on the writer, intersected with a
//! token's channel scope) plus the membership changes held meanwhile; set the
//! rooms and retain their topics; query B (each room's `accepted_sequence`);
//! send the parts, then EOSE, then flush the queue.
//!
//! Membership afterwards (U1b): joins and removals on this pod apply through
//! hooks at the change ([`recheck_room`], [`remove_room`],
//! [`remove_deleted_room`]); changes on other pods arrive as cache drops
//! ([`after_cache_invalidation`]). Every apply is compare-and-apply on the
//! key's origin generation ([`crate::membership_cache_gen`]): a result read
//! before a later change is never applied over it. Membership never closes a
//! stream; a removed room just stops being carried (no CLOSED).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use kuzz_core::CommunityId;
use kuzz_pubsub::EventTopic;
use nostr::{EventBuilder, Filter, Kind, SingleLetterTag, Tag};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::connection::ConnectionState;
use crate::protocol::RelayMessage;
use crate::state::{AppState, RawSend};
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

/// A `shadow` stream frame is not queued while the send buffer is more than
/// this full; the stream closes instead (`closed{shadow_full}`).
pub const SHADOW_FILL_PERCENT: usize = 50;
/// On a connection holding a `live`/`gated` stream, a history frame that would
/// fill the send buffer past this is refused (`error: busy`).
pub const HISTORY_FILL_PERCENT: usize = 80;

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

    // Register with no rooms before query A, so every membership change from
    // here on reaches the stream: held until query A's result is in, then
    // replayed on top of it.
    let handle = Arc::new(
        StreamHandle::new(
            sub_id.clone(),
            owner,
            mode,
            conn.tenant.clone(),
            queue_cap(state.config.send_buffer_size),
        )
        .limited_to(token_channel_ids),
    );
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
            Vec::new(),
        );
        if let Some(replaced) = replaced {
            super::close::release_scope_topics(&state, &conn.tenant, &replaced.scope).await;
        }
    }
    spawn_teardown(Arc::clone(&state), conn_id, Arc::clone(&handle));

    // Query A: member rooms, on the writer.
    let member_rooms = match state
        .db
        .get_member_channel_ids(community, &pubkey_bytes)
        .await
    {
        Ok(rooms) => rooms,
        Err(e) => {
            warn!(%conn_id, "user stream: member rooms query failed: {e}");
            close_stream(&state, conn_id, &handle, Some(reason::CLOSED), false).await;
            return;
        }
    };

    match set_stream_rooms(&state, &conn, &handle, member_rooms).await {
        SetRooms::Set => {}
        SetRooms::Gone => return,
        SetRooms::TooMany => {
            close_stream(
                &state,
                conn_id,
                &handle,
                Some(reason::TOO_MANY_ROOMS),
                false,
            )
            .await;
            return;
        }
    }

    // Query B: each carried room's accepted sequence, after registration, so
    // a frame committed after this read is either queued or already counted.
    let rooms = state.sub_registry.stream_rooms(conn_id);
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
        if !handle.is_open() {
            close_stream(&state, conn_id, &handle, None, false).await;
            return;
        }
        if !conn.send(RelayMessage::event(&sub_id, &event)) {
            snapshot_send_failed(&state, conn_id, &handle).await;
            return;
        }
    }
    if !handle.is_open() {
        close_stream(&state, conn_id, &handle, None, false).await;
        return;
    }
    if !conn.send(RelayMessage::eose(&sub_id)) {
        snapshot_send_failed(&state, conn_id, &handle).await;
        return;
    }
    let mut sent = RawSend::Sent;
    if !handle.go_live(|frame| {
        sent = send_frame(&state, conn_id, mode, frame);
        sent == RawSend::Sent
    }) {
        // Closed meanwhile or the socket is gone: nothing to say. A frame the
        // buffer could not take: overflow by the mode's rule.
        let full = sent == RawSend::Full;
        let overflow = full.then_some(reason::OVERFLOW);
        close_stream(
            &state,
            conn_id,
            &handle,
            overflow,
            full && mode.is_relied_on(),
        )
        .await;
        return;
    }
    metrics::histogram!("kuzz_user_stream_snapshot_seconds", "mode" => mode.as_str())
        .record(started.elapsed().as_secs_f64());
    debug!(%conn_id, %sub_id, rooms = snapshot.len(), mode = mode.as_str(), "user stream live");
}

/// A snapshot part or the EOSE did not fit the send buffer (review S5): close
/// with the overflow reason so the client learns the mode failed. Before EOSE
/// the client still keeps its chunks, so the socket is not cancelled; the
/// CLOSED goes on the data channel, else the control channel, else the
/// connection is cancelled. On a gone socket it is a no-op.
async fn snapshot_send_failed(state: &AppState, conn_id: ConnId, handle: &StreamHandle) {
    close_stream(state, conn_id, handle, Some(reason::OVERFLOW), false).await;
}

/// Outcome of [`set_stream_rooms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetRooms {
    /// The rooms are set and their topics retained.
    Set,
    /// The stream no longer owns its id (replaced, closed or socket gone).
    Gone,
    /// More than [`MAX_ROOMS`]; nothing was set.
    TooMany,
}

/// Set a registered stream's rooms: query A's `member_rooms` (inside the
/// token's scope), then the membership changes held since registration, in
/// order. From here on changes apply to the registry directly.
async fn set_stream_rooms(
    state: &AppState,
    conn: &ConnectionState,
    handle: &StreamHandle,
    member_rooms: Vec<Uuid>,
) -> SetRooms {
    let subs = conn.subscriptions.lock().await;
    if subs.get(&handle.sub_id) != Some(&handle.owner) || !handle.is_open() {
        return SetRooms::Gone;
    }
    let mut rooms: BTreeSet<Uuid> = member_rooms
        .into_iter()
        .filter(|room| handle.allows(*room))
        .collect();
    for (room, carried) in handle.take_room_ops() {
        if !carried {
            rooms.remove(&room);
        } else if handle.allows(room) {
            rooms.insert(room);
        }
    }
    if rooms.len() > MAX_ROOMS {
        return SetRooms::TooMany;
    }
    for &room in &rooms {
        if state
            .sub_registry
            .stream_set_room(conn.conn_id, &handle.sub_id, room, true)
        {
            state
                .pubsub
                .retain_topic(&conn.tenant, EventTopic::Channel(room))
                .await;
        }
    }
    SetRooms::Set
}

/// Route one fan-out frame to a user stream (queued until its EOSE). `false`
/// when the frame was not handed to the socket.
///
/// A frame the stream cannot take closes it with `error: user-stream
/// overflow` through its teardown task (plan V4.2 section 2.4): a full
/// pre-EOSE queue, a `shadow` frame over [`SHADOW_FILL_PERCENT`] fill (the
/// sub only; chunks keep the rest of the buffer), or a `live`/`gated` frame
/// on a full buffer (CLOSED then the socket is cancelled). Stream frames
/// never touch the slow-client grace counter.
pub fn deliver(
    state: &AppState,
    conn_id: ConnId,
    handle: &StreamHandle,
    frame: Arc<Bytes>,
) -> bool {
    let mut sent = RawSend::Sent;
    match handle.deliver(frame, |frame| {
        sent = send_frame(state, conn_id, handle.mode, frame);
        sent == RawSend::Sent
    }) {
        Delivery::Sent(true) | Delivery::Queued => true,
        Delivery::Sent(false) => {
            if sent == RawSend::Full {
                handle.request_close(reason::OVERFLOW, handle.mode.is_relied_on());
            }
            false
        }
        Delivery::Dropped => false,
        Delivery::Overflow => {
            handle.request_close(reason::OVERFLOW, handle.mode.is_relied_on());
            false
        }
    }
}

/// Hand one stream frame to the socket under the mode's rule: `shadow` takes
/// no frame over [`SHADOW_FILL_PERCENT`] fill (reported as `Full`).
fn send_frame(state: &AppState, conn_id: ConnId, mode: StreamMode, frame: Arc<Bytes>) -> RawSend {
    if !mode.is_relied_on()
        && state
            .conn_manager
            .send_fill_exceeds(conn_id, SHADOW_FILL_PERCENT, 0)
    {
        return RawSend::Full;
    }
    state.conn_manager.try_send_raw(conn_id, frame)
}

/// Fail closed (plan V4.2 section 2.4): the access gate could not decide
/// whether an event reaches `recipients`, so it reaches none of them. The
/// user streams among them close with `error: user-stream closed` (a
/// `live`/`gated` socket is cancelled after it), since a stream must never
/// miss a frame silently. Other subscriptions are unchanged.
pub fn fail_closed<'a>(
    state: &AppState,
    recipients: impl IntoIterator<Item = &'a (ConnId, crate::subscription::SubId)>,
) {
    if !state.sub_registry.has_streams() {
        return;
    }
    for (conn_id, sub_id) in recipients {
        if let Some(handle) = state.sub_registry.stream_for(*conn_id, sub_id) {
            warn!(%conn_id, "user stream: fan-out access check failed; closing the stream");
            handle.request_close(reason::CLOSED, handle.mode.is_relied_on());
        }
    }
}

/// History headroom (plan V4.2 section 2.4): on a connection holding an open
/// `live`/`gated` stream, `true` when one more history frame would fill the
/// send buffer past [`HISTORY_FILL_PERCENT`]. The caller ends the REQ with
/// `CLOSED "error: busy"` at the threshold (no grace bump). Other
/// connections are unchanged.
pub fn history_busy(state: &AppState, conn: &ConnectionState) -> bool {
    conn.send_fill_exceeds(HISTORY_FILL_PERCENT)
        && state.sub_registry.has_streams()
        && state
            .sub_registry
            .stream_of(conn.conn_id)
            .is_some_and(|handle| handle.mode.is_relied_on() && handle.is_open())
}

/// Count one history REQ refused with `error: busy`.
pub fn note_history_busy() {
    metrics::counter!("kuzz_user_stream_history_busy_total").increment(1);
}

/// Multi-node fan-out lagged (live events were lost): close every user
/// stream on this pod, in all communities, with `error: user-stream lagged`
/// on the control channel, then cancel its socket. Clients reconnect with
/// jitter and catch up. Returns the number of streams closed.
pub fn close_all_lagged(state: &AppState) -> usize {
    let streams = state.sub_registry.streams_in(None);
    for (_, handle) in &streams {
        handle.request_close(reason::LAGGED, true);
    }
    if !streams.is_empty() {
        warn!(
            streams = streams.len(),
            "user stream: fan-out lagged; closed every stream on this pod"
        );
    }
    streams.len()
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
///
/// Which queue carries the CLOSED (plan V4.2 section 2.4): with `cancel` it
/// goes on the control channel (drained before the Close), then the socket
/// is cancelled; without, it is a threshold CLOSED: the data channel without
/// a grace bump (after the frames already queued for the sub), else the
/// control channel, else the socket is cancelled.
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
    let Some(reason) = reason else {
        if cancel {
            state.conn_manager.cancel_conn(conn_id);
        }
        return;
    };
    metrics::counter!("kuzz_user_stream_closed_total", "reason" => closed_label(reason, handle.mode))
        .increment(1);
    let frame = RelayMessage::closed(&handle.sub_id, reason);
    if cancel {
        state.conn_manager.send_closed_and_cancel(conn_id, frame);
    } else {
        state.conn_manager.send_threshold_closed(conn_id, frame);
    }
}

/// `closed{key}` label of a stream CLOSED (the client's keys; overflow splits
/// by mode).
fn closed_label(reason: &str, mode: StreamMode) -> &'static str {
    match reason {
        reason::OVERFLOW if mode.is_relied_on() => "full",
        reason::OVERFLOW => "shadow_full",
        reason::LAGGED => "lagged",
        reason::TOO_MANY_ROOMS => "too_many_rooms",
        reason::CLOSED => "closed",
        _ => "other",
    }
}

/// Most compare-and-apply rounds of one membership re-check before the
/// streams are closed instead (`error: user-stream closed`).
pub const MAX_RECHECKS: usize = 3;

/// The account's streams on this pod.
fn streams_of(
    state: &AppState,
    community: CommunityId,
    pubkey: &[u8],
) -> Vec<(ConnId, Arc<StreamHandle>)> {
    if !state.sub_registry.has_streams() {
        return Vec::new();
    }
    state
        .conn_manager
        .connection_ids_for_pubkey_in_community(community, pubkey)
        .into_iter()
        .filter_map(|conn_id| {
            state
                .sub_registry
                .stream_of(conn_id)
                .map(|handle| (conn_id, handle))
        })
        .collect()
}

/// What one membership apply did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Applied {
    /// Applied, held for the replay, or nothing to do (stream gone).
    Done,
    /// The key changed since the result was read: read again.
    Stale,
}

/// Carry (`carried`) or drop `room` on one stream, under the connection's
/// lifecycle lock, if `current()` still holds there. Before the stream's
/// rooms are set the change is held and replayed on top of query A.
async fn apply_room(
    state: &AppState,
    conn_id: ConnId,
    handle: &StreamHandle,
    room: Uuid,
    carried: bool,
    current: impl Fn() -> bool,
) -> Applied {
    let Some(subscriptions) = state.conn_manager.subscriptions_for(conn_id) else {
        return Applied::Done;
    };
    let subs = subscriptions.lock().await;
    if subs.get(&handle.sub_id) != Some(&handle.owner) || !handle.is_open() {
        return Applied::Done;
    }
    if !current() {
        return Applied::Stale;
    }
    if carried && !handle.allows(room) {
        return Applied::Done;
    }
    if handle.hold_room_op(room, carried) {
        return Applied::Done;
    }
    if state
        .sub_registry
        .stream_set_room(conn_id, &handle.sub_id, room, carried)
    {
        let topic = EventTopic::Channel(room);
        if carried {
            state.pubsub.retain_topic(&handle.tenant, topic).await;
        } else {
            state.pubsub.release_topic(&handle.tenant, topic).await;
        }
        metrics::counter!(
            "kuzz_user_stream_room_changes_total",
            "change" => if carried { "add" } else { "remove" }
        )
        .increment(1);
    }
    Applied::Done
}

/// Membership re-check of one (room, account): read on the writer whether
/// the account is a member of `room` and carry or drop it on each of their
/// streams on this pod. Compare-and-apply: the result is applied only while
/// the key's origin generation is the one taken before the read; after a
/// later change it is read again. After [`MAX_RECHECKS`] lost races, or on a
/// database error, the streams close (`error: user-stream closed`).
///
/// Joins call it before their 44100 goes out, so the joiner's next room
/// event already rides the stream.
pub async fn recheck_room(state: &AppState, community: CommunityId, room: Uuid, pubkey: &[u8]) {
    for _ in 0..MAX_RECHECKS {
        let streams = streams_of(state, community, pubkey);
        if streams.is_empty() {
            return;
        }
        let generation = state
            .membership_gen
            .origin_generation(community, room, pubkey);
        let member = match state.db.is_member(community, room, pubkey).await {
            Ok(member) => member,
            Err(e) => {
                warn!(%room, "user stream: membership re-check failed: {e}");
                close_streams(state, &streams, reason::CLOSED).await;
                return;
            }
        };
        let current = || {
            state
                .membership_gen
                .origin_generation(community, room, pubkey)
                == generation
        };
        let mut stale = false;
        for (conn_id, handle) in &streams {
            if apply_room(state, *conn_id, handle, room, member, current).await == Applied::Stale {
                stale = true;
                break;
            }
        }
        if !stale {
            return;
        }
    }
    warn!(%room, "user stream: membership re-check kept losing races; closing the streams");
    let streams = streams_of(state, community, pubkey);
    close_streams(state, &streams, reason::CLOSED).await;
}

/// The account left or was removed from `room` by the change whose own
/// generation is `generation` ([`AppState::invalidate_membership`]'s
/// result): drop the room from their streams on this pod, without a CLOSED.
/// When a later change of the key happened meanwhile, re-check from the
/// database instead, in a task: the kick path calls this under the membership
/// fence, which holds a writer connection.
pub async fn remove_room(
    state: &Arc<AppState>,
    community: CommunityId,
    room: Uuid,
    pubkey: &[u8],
    generation: u64,
) {
    let current = || {
        state
            .membership_gen
            .origin_generation(community, room, pubkey)
            == generation
    };
    for (conn_id, handle) in streams_of(state, community, pubkey) {
        if apply_room(state, conn_id, &handle, room, false, current).await == Applied::Stale {
            let state = Arc::clone(state);
            let pubkey = pubkey.to_vec();
            tokio::spawn(async move { recheck_room(&state, community, room, &pubkey).await });
            return;
        }
    }
}

/// `room` was deleted: drop it from every stream in the community on this pod
/// (a deleted room never comes back, so nothing to compare).
pub async fn remove_deleted_room(state: &AppState, community: CommunityId, room: Uuid) {
    for (conn_id, handle) in state.sub_registry.streams_in(Some(community)) {
        apply_room(state, conn_id, &handle, room, false, || true).await;
    }
}

/// Re-read the rooms of every stream in `community` on this pod (query A
/// again) and apply the difference: a carried room that query A no longer
/// returns is dropped while its key is unchanged since before the read
/// (otherwise re-checked); a room query A returns that the stream lacks is
/// re-checked one by one (rare: something was missed).
async fn rerun_rooms(state: &AppState, community: CommunityId) {
    for (conn_id, handle) in state.sub_registry.streams_in(Some(community)) {
        let Some(pubkey) = state.conn_manager.pubkey_for_conn(conn_id) else {
            continue;
        };
        let carried: BTreeSet<Uuid> = state
            .sub_registry
            .stream_rooms(conn_id)
            .into_iter()
            .collect();
        let before: BTreeMap<Uuid, u64> = carried
            .iter()
            .map(|&room| {
                (
                    room,
                    state
                        .membership_gen
                        .origin_generation(community, room, &pubkey),
                )
            })
            .collect();
        let member: BTreeSet<Uuid> = match state.db.get_member_channel_ids(community, &pubkey).await
        {
            Ok(rooms) => rooms.into_iter().filter(|r| handle.allows(*r)).collect(),
            Err(e) => {
                warn!(%conn_id, "user stream: member rooms re-run failed: {e}");
                let cancel = handle.mode.is_relied_on();
                close_stream(state, conn_id, &handle, Some(reason::CLOSED), cancel).await;
                continue;
            }
        };
        for (&room, &generation) in before.iter().filter(|(room, _)| !member.contains(room)) {
            let current = || {
                state
                    .membership_gen
                    .origin_generation(community, room, &pubkey)
                    == generation
            };
            if apply_room(state, conn_id, &handle, room, false, current).await == Applied::Stale {
                recheck_room(state, community, room, &pubkey).await;
            }
        }
        for &room in member.difference(&carried) {
            recheck_room(state, community, room, &pubkey).await;
        }
    }
}

/// [`rerun_rooms`] for one community in a task, coalesced: while a re-run of
/// the community is in flight, more requests make it run once more after.
pub fn spawn_rerun_rooms(state: &Arc<AppState>, community: CommunityId) {
    if !state.sub_registry.begin_stream_rerun(community) {
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        loop {
            rerun_rooms(&state, community).await;
            if !state.sub_registry.finish_stream_rerun(community) {
                break;
            }
        }
    });
}

/// The stream side of a cache drop received over Redis (after
/// [`AppState::apply_cache_invalidation`]): a membership change on another
/// pod is re-checked for the account's streams here; a channel delete or an
/// accessible-all drop re-runs query A for the community's streams.
pub fn after_cache_invalidation(
    state: &Arc<AppState>,
    community: CommunityId,
    invalidation: &kuzz_pubsub::cache_invalidation::CacheInvalidation,
) {
    if !state.sub_registry.has_streams() {
        return;
    }
    match invalidation {
        kuzz_pubsub::cache_invalidation::CacheInvalidation::Membership {
            channel_id,
            pubkey,
            origin,
        } => {
            // This pod's own change was applied by its hook already.
            if *origin == Some(state.membership_gen.pod_id())
                || streams_of(state, community, pubkey).is_empty()
            {
                return;
            }
            let state = Arc::clone(state);
            let (room, pubkey) = (*channel_id, pubkey.clone());
            tokio::spawn(async move { recheck_room(&state, community, room, &pubkey).await });
        }
        kuzz_pubsub::cache_invalidation::CacheInvalidation::ChannelDeleted
        | kuzz_pubsub::cache_invalidation::CacheInvalidation::AccessibleAll => {
            spawn_rerun_rooms(state, community)
        }
        _ => {}
    }
}

/// Cache drops were lost (the consumer lagged): re-run query A for every
/// stream on this pod.
pub fn after_cache_invalidation_lag(state: &Arc<AppState>) {
    let communities: BTreeSet<CommunityId> = state
        .sub_registry
        .streams_in(None)
        .into_iter()
        .map(|(_, handle)| handle.tenant.community())
        .collect();
    for community in communities {
        spawn_rerun_rooms(state, community);
    }
}

/// Close streams with `reason`; a `live`/`gated` socket is cancelled after it.
async fn close_streams(state: &AppState, streams: &[(ConnId, Arc<StreamHandle>)], reason: &str) {
    for (conn_id, handle) in streams {
        let cancel = handle.mode.is_relied_on();
        close_stream(state, *conn_id, handle, Some(reason), cancel).await;
    }
}

fn refuse(conn: &ConnectionState, sub_id: &str, reason: &str) {
    conn.send(RelayMessage::closed(sub_id, reason));
}

#[cfg(test)]
#[path = "user_stream_tests.rs"]
mod tests;
