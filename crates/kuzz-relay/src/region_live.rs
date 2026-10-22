//! Live rooms across regions (global regions stages S4-4b, S4-4c): the
//! entry side.
//!
//! A REQ that names rooms of another region registers for them here like
//! for local rooms, then asks [`RegionLive::ensure`] to watch them at their
//! home. Each (community, home region) pair has one task holding one stream
//! ([`crate::region_stream`]) with every room this region watches there and
//! the readers it watches each room for. Events the home sends go to the
//! local subscriptions of the readers it allowed, with their order numbers.
//!
//! - **No gap at the start.** `ensure` waits until the home answers
//!   `watching` (at most [`ENSURE_WAIT`]); the REQ reads history after that.
//! - **No silent gap later.** When the stream drops, the task reconnects
//!   with backoff and watches again. The subscribers of a room that was
//!   watched without a live stream are sent `KUZZ_RESYNC` once the home
//!   answers `watching` again, so their apps fill the gap by number (S0).
//! - **Unwatch when nobody looks.** Every [`RECONCILE`] the task reads the
//!   subscription registry: readers who no longer subscribe are dropped,
//!   and a room nobody subscribes to is unwatched. With no room left the
//!   stream is closed.
//! - **Room-less events (S4-4c).** A room-less live REQ asks every other
//!   region's link to stream the reader's own room-less events
//!   ([`RegionLive::watch_reader`]): "you were added to a room", read mark
//!   pings, pairing. Reconcile keeps the set to readers that still have a
//!   room-less subscription. These events have no order number, so a drop
//!   can lose some; apps catch up on their next full refresh.
//!
//! Only used with the link on (`KUZZ_REGION_LINK`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use kuzz_core::event::StoredEvent;
use kuzz_core::tenant::TenantContext;
use kuzz_core::CommunityId;
use nostr::Timestamp;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use crate::region_link::{body_hash, http_base, link_keys, sign, Claims};
use crate::region_stream::{EntryFrame, HomeFrame, STREAM_PATH};
use crate::state::AppState;

/// How long a REQ waits for its rooms to be watched before reading history
/// anyway (the subscribers are resynced once the watch is live).
pub const ENSURE_WAIT: Duration = Duration::from_secs(3);

/// How often a link drops readers and rooms nobody subscribes to.
pub const RECONCILE: Duration = Duration::from_secs(10);

/// A stream up this long resets the reconnect backoff.
const STABLE: Duration = Duration::from_secs(30);

/// A link that hears nothing (not even a ping) for this long reconnects.
const SILENCE: Duration = Duration::from_secs(60);

/// Reconnect backoff bounds.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// `KUZZ_RESYNC` reason for subscribers of a room whose stream dropped.
const RESYNC_REASON: &str = "region_link";

/// Spread of the delayed closes of sockets that cannot be told to resync.
const RESYNC_LEGACY_JITTER_MS: u64 = 5_000;

/// Readers per reader watch frame (the home's per-frame limit).
const READER_CHUNK: usize = 5_000;

/// A request to a link task.
#[derive(Debug)]
struct Ensure {
    rooms: Vec<Uuid>,
    reader: Vec<u8>,
    /// Also stream the reader's room-less events.
    global: bool,
    done: oneshot::Sender<()>,
}

/// The links of this region to the homes of rooms it watches.
#[derive(Default)]
pub struct RegionLive {
    links: Mutex<HashMap<(CommunityId, String), mpsc::Sender<Ensure>>>,
}

impl RegionLive {
    /// Watch `rooms`, which live in `region`, for `reader` (a pubkey whose
    /// subscription to them is already registered). Returns once the home
    /// watches them all, or after [`ENSURE_WAIT`].
    pub async fn ensure(
        &self,
        state: &Arc<AppState>,
        tenant: &TenantContext,
        region: &str,
        rooms: Vec<Uuid>,
        reader: Vec<u8>,
    ) {
        if rooms.is_empty() {
            return;
        }
        let tx = self.link(state, tenant, region);
        let (done, wait) = oneshot::channel();
        let request = Ensure {
            rooms,
            reader,
            global: false,
            done,
        };
        let _ = tokio::time::timeout(ENSURE_WAIT, async move {
            if tx.send(request).await.is_ok() {
                let _ = wait.await;
            }
        })
        .await;
    }

    /// Stream `reader`'s room-less events from every other region (a pubkey
    /// whose room-less subscription is already registered). Does not wait.
    pub async fn watch_reader(
        &self,
        state: &Arc<AppState>,
        tenant: &TenantContext,
        reader: Vec<u8>,
    ) {
        let regions = match state.room_router.other_regions(&state.db).await {
            Ok(regions) => regions,
            Err(error) => {
                tracing::warn!(%error, "Region list unavailable; room-less events stay local");
                return;
            }
        };
        for region in regions {
            let (done, _) = oneshot::channel();
            let request = Ensure {
                rooms: Vec::new(),
                reader: reader.clone(),
                global: true,
                done,
            };
            if self.link(state, tenant, &region).try_send(request).is_err() {
                metrics::counter!("kuzz_region_live_reader_watch_dropped_total").increment(1);
            }
        }
    }

    fn link(
        &self,
        state: &Arc<AppState>,
        tenant: &TenantContext,
        region: &str,
    ) -> mpsc::Sender<Ensure> {
        let mut links = self.links.lock().unwrap_or_else(|e| e.into_inner());
        let key = (tenant.community(), region.to_string());
        if let Some(tx) = links.get(&key).filter(|tx| !tx.is_closed()) {
            return tx.clone();
        }
        let (tx, rx) = mpsc::channel(1024);
        tokio::spawn(run_link(
            Arc::clone(state),
            tenant.clone(),
            region.to_string(),
            rx,
        ));
        links.insert(key, tx.clone());
        tx
    }
}

/// What one link watches.
#[derive(Default)]
struct Rooms {
    /// Readers per room, as last sent to the home.
    readers: HashMap<Uuid, BTreeSet<Vec<u8>>>,
    /// Watches sent and not yet answered, per room. The home answers in
    /// order, so a room is live with its latest readers only at zero.
    inflight: HashMap<Uuid, usize>,
    /// Rooms the home answered `watching` for on the current stream.
    live: HashSet<Uuid>,
    /// Rooms to resync once they are live again.
    resync: HashSet<Uuid>,
    /// Rooms to watch again at the next reconcile (a temporary refusal).
    retry: HashSet<Uuid>,
    /// Callers waiting for a room to be live.
    waiters: HashMap<Uuid, Vec<Arc<Waiter>>>,
    /// Readers whose room-less events are streamed, as last sent.
    global: BTreeSet<Vec<u8>>,
}

/// One `ensure` call, done once each of its rooms is live or refused.
struct Waiter {
    left: Mutex<usize>,
    done: Mutex<Option<oneshot::Sender<()>>>,
}

impl Waiter {
    fn new(rooms: usize, done: oneshot::Sender<()>) -> Arc<Self> {
        Arc::new(Self {
            left: Mutex::new(rooms),
            done: Mutex::new(Some(done)),
        })
    }

    /// Whether the caller stopped waiting ([`ENSURE_WAIT`]) and read history
    /// without the watch.
    fn gave_up(&self) -> bool {
        self.done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(oneshot::Sender::is_closed)
    }

    fn room_done(&self) {
        let mut left = self.left.lock().unwrap_or_else(|e| e.into_inner());
        *left = left.saturating_sub(1);
        if *left == 0 {
            self.finish();
        }
    }

    fn finish(&self) {
        if let Some(done) = self.done.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = done.send(());
        }
    }
}

/// Whether a refusal may pass on its own (the home says "try again").
fn temporary(reason: &str) -> bool {
    reason.starts_with("error:")
}

/// Reader watch frames for `readers`, in chunks the home accepts.
fn reader_frames<'a>(readers: impl Iterator<Item = &'a Vec<u8>>, watch: bool) -> Vec<EntryFrame> {
    let readers: Vec<String> = readers.map(hex::encode).collect();
    readers
        .chunks(READER_CHUNK)
        .map(|chunk| {
            let readers = chunk.to_vec();
            if watch {
                EntryFrame::WatchReaders { readers }
            } else {
                EntryFrame::UnwatchReaders { readers }
            }
        })
        .collect()
}

impl Rooms {
    /// Nothing to watch: no room and no reader.
    fn idle(&self) -> bool {
        self.readers.is_empty() && self.global.is_empty()
    }

    /// Frames that watch every reader's room-less events (a new stream).
    fn watch_all_readers(&self) -> Vec<EntryFrame> {
        reader_frames(self.global.iter(), true)
    }

    /// The watch frame for `room` with its current readers, counted as sent.
    fn watch(&mut self, room: Uuid) -> Option<EntryFrame> {
        let readers = self.readers.get(&room)?;
        let frame = watch_frame(room, readers);
        *self.inflight.entry(room).or_default() += 1;
        self.live.remove(&room);
        Some(frame)
    }

    /// Forget a room; returns the unwatch frame.
    fn forget(&mut self, room: Uuid) -> EntryFrame {
        self.readers.remove(&room);
        self.inflight.remove(&room);
        self.live.remove(&room);
        self.resync.remove(&room);
        self.retry.remove(&room);
        for waiter in self.waiters.remove(&room).unwrap_or_default() {
            waiter.room_done();
        }
        EntryFrame::Unwatch { room }
    }

    /// Add a reader; returns the watch frames to send when `connected`.
    fn add(&mut self, request: Ensure, connected: bool) -> Vec<EntryFrame> {
        let mut frames = Vec::new();
        if request.global && self.global.insert(request.reader.clone()) && connected {
            frames.extend(reader_frames(std::iter::once(&request.reader), true));
        }
        // A caller that already gave up reads history without the watch.
        let unwatched = !connected || request.done.is_closed();
        let mut changed = Vec::new();
        let mut pending = Vec::new();
        for room in request.rooms {
            let readers = self.readers.entry(room).or_default();
            let new_reader = readers.insert(request.reader.clone());
            let unanswered = !self.live.contains(&room) && !self.inflight.contains_key(&room);
            if new_reader || unanswered {
                changed.push(room);
            }
            if unwatched {
                self.resync.insert(room);
            }
            if new_reader || !self.live.contains(&room) {
                pending.push(room);
            }
        }
        let waiter = Waiter::new(pending.len(), request.done);
        if pending.is_empty() || !connected {
            waiter.finish();
        } else {
            for room in &pending {
                self.waiters
                    .entry(*room)
                    .or_default()
                    .push(Arc::clone(&waiter));
            }
        }
        if !connected {
            return Vec::new();
        }
        frames.extend(changed.into_iter().filter_map(|room| self.watch(room)));
        frames
    }

    /// The home answered `watching` for `room`. Returns whether its
    /// subscribers must resync now.
    fn answered(&mut self, room: Uuid) -> bool {
        let Some(count) = self.inflight.get_mut(&room) else {
            return false;
        };
        *count -= 1;
        if *count > 0 {
            return false;
        }
        self.inflight.remove(&room);
        if !self.readers.contains_key(&room) {
            return false;
        }
        self.live.insert(room);
        let mut resync = self.resync.remove(&room);
        for waiter in self.waiters.remove(&room).unwrap_or_default() {
            resync |= waiter.gave_up();
            waiter.room_done();
        }
        resync
    }

    /// The home refused a watch of `room`. A temporary refusal is retried at
    /// the next reconcile and the room resyncs once live; any other forgets
    /// the room. Returns frames to send.
    fn refused(&mut self, room: Uuid, reason: &str) -> Vec<EntryFrame> {
        let Some(count) = self.inflight.get_mut(&room) else {
            return Vec::new();
        };
        *count -= 1;
        if *count > 0 {
            // A newer watch of the room is still to be answered.
            return Vec::new();
        }
        self.inflight.remove(&room);
        if !self.readers.contains_key(&room) {
            return Vec::new();
        }
        if temporary(reason) {
            self.retry.insert(room);
            self.resync.insert(room);
            for waiter in self.waiters.remove(&room).unwrap_or_default() {
                waiter.room_done();
            }
            return Vec::new();
        }
        vec![self.forget(room)]
    }

    /// The stream dropped: nothing is live, every room resyncs once it is,
    /// and nobody waits any longer.
    fn dropped(&mut self) {
        self.live.clear();
        self.inflight.clear();
        self.retry.clear();
        self.resync.extend(self.readers.keys().copied());
        for (_, waiters) in self.waiters.drain() {
            for waiter in waiters {
                waiter.finish();
            }
        }
    }

    /// Apply who subscribes now (`subscribers`: readers per room), and watch
    /// rooms to retry again. Returns the frames to send when `connected`.
    fn reconcile(
        &mut self,
        subscribers: HashMap<Uuid, BTreeSet<Vec<u8>>>,
        global: &BTreeSet<Vec<u8>>,
        connected: bool,
    ) -> Vec<EntryFrame> {
        // Room-less readers follow the registry both ways: those gone are
        // unwatched, and any a dropped request missed are watched.
        let gone: Vec<Vec<u8>> = self.global.difference(global).cloned().collect();
        let new: Vec<Vec<u8>> = global.difference(&self.global).cloned().collect();
        let mut frames = reader_frames(gone.iter(), false);
        frames.extend(reader_frames(new.iter(), true));
        self.global = global.clone();
        let rooms: Vec<Uuid> = self.readers.keys().copied().collect();
        for room in rooms {
            if self.waiters.contains_key(&room) {
                continue;
            }
            let kept: BTreeSet<Vec<u8>> = match subscribers.get(&room) {
                Some(now) => self.readers[&room].intersection(now).cloned().collect(),
                None => BTreeSet::new(),
            };
            if kept.is_empty() {
                frames.push(self.forget(room));
            } else if kept.len() != self.readers[&room].len() || self.retry.remove(&room) {
                self.readers.insert(room, kept);
                frames.extend(self.watch(room));
            }
        }
        if !connected {
            self.inflight.clear();
            return Vec::new();
        }
        frames
    }
}

fn watch_frame(room: Uuid, readers: &BTreeSet<Vec<u8>>) -> EntryFrame {
    EntryFrame::Watch {
        room,
        readers: readers.iter().map(hex::encode).collect(),
    }
}

fn text(frame: &EntryFrame) -> Option<Message> {
    serde_json::to_string(frame)
        .ok()
        .map(|text| Message::Text(text.into()))
}

/// Open a stream to `region` for `tenant`'s community.
async fn connect(
    state: &AppState,
    tenant: &TenantContext,
    region: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let keys = link_keys(state).ok_or("the region link is off")?;
    let (connect_url, _) = state
        .db
        .region_link(region)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("unknown region")?;
    let claims = Claims {
        v: 1,
        entry_region: state.config.region_id.clone(),
        target_region: region.to_string(),
        community_id: *tenant.community().as_uuid(),
        method: "GET".to_string(),
        path: STREAM_PATH.to_string(),
        body_sha256: body_hash(b""),
        channel_id: None,
        epoch: None,
        principal: None,
        nonce: Uuid::new_v4().simple().to_string(),
    };
    let auth = sign(&keys, &claims, Timestamp::now())?;
    let base = http_base(&connect_url);
    let url = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}{STREAM_PATH}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}{STREAM_PATH}")
    } else {
        return Err("bad region address".to_string());
    };
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        auth.parse().map_err(|_| "bad attestation header")?,
    );
    let (socket, _) = tokio::time::timeout(
        crate::region_link::CALL_TIMEOUT,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| "timed out".to_string())?
    .map_err(|e| e.to_string())?;
    Ok(socket)
}

/// Readers with a room-less subscription here now.
fn global_subscribers(state: &AppState, community: CommunityId) -> BTreeSet<Vec<u8>> {
    state
        .sub_registry
        .global_subscriber_conns(community)
        .into_iter()
        .filter_map(|conn| state.conn_manager.pubkey_for_conn(conn))
        .collect()
}

/// Readers subscribed now to each of `rooms`.
fn subscribers(
    state: &AppState,
    community: CommunityId,
    rooms: &HashSet<Uuid>,
) -> HashMap<Uuid, BTreeSet<Vec<u8>>> {
    state
        .sub_registry
        .channel_subscriber_conns_many(community, rooms)
        .into_iter()
        .map(|(room, conns)| {
            let readers = conns
                .into_iter()
                .filter_map(|conn| state.conn_manager.pubkey_for_conn(conn))
                .collect();
            (room, readers)
        })
        .collect()
}

/// Tell the subscribers of `room` to fetch what they missed.
async fn resync_room(state: Arc<AppState>, community: CommunityId, room: Uuid) {
    let rooms = HashSet::from([room]);
    let conns: Vec<Uuid> = state
        .sub_registry
        .channel_subscriber_conns_many(community, &rooms)
        .remove(&room)
        .map(|conns| conns.into_iter().collect())
        .unwrap_or_default();
    if conns.is_empty() {
        return;
    }
    let (signalled, closed) = state
        .conn_manager
        .resync_conns(&conns, RESYNC_REASON, RESYNC_LEGACY_JITTER_MS)
        .await;
    metrics::counter!("kuzz_region_live_resyncs_total").increment(1);
    tracing::info!(%room, signalled, closed, "Region room live again; subscribers resynced");
}

/// Deliver a room-less event the home sent to the local subscribers of the
/// readers it allowed.
fn deliver_global(state: &AppState, community: CommunityId, event: nostr::Event, to: &[String]) {
    let allowed: HashSet<Vec<u8>> = to.iter().filter_map(|hex| hex::decode(hex).ok()).collect();
    let stored = StoredEvent::new(event, None);
    let matches: Vec<_> = state
        .sub_registry
        .fan_out_scoped(community, &stored)
        .into_iter()
        .filter(|(conn, _)| {
            state.conn_manager.community_for_conn(*conn) == Some(community)
                && state
                    .conn_manager
                    .pubkey_for_conn(*conn)
                    .is_some_and(|pk| allowed.contains(&pk))
        })
        .collect();
    metrics::counter!("kuzz_region_live_global_events_total").increment(1);
    let dropped = crate::handlers::event::send_to_matches(state, &matches, &stored);
    if dropped > 0 {
        tracing::warn!(dropped, "region live fan-out: connection(s) dropped");
    }
}

/// Deliver an event the home sent to the local subscribers of the readers
/// it allowed.
fn deliver(
    state: &AppState,
    community: CommunityId,
    room: Uuid,
    seq: Option<i64>,
    event: nostr::Event,
    to: &[String],
) {
    let allowed: HashSet<Vec<u8>> = to.iter().filter_map(|hex| hex::decode(hex).ok()).collect();
    let mut stored = StoredEvent::new(event, Some(room));
    stored.channel_sequence = seq;
    let matches: Vec<_> = state
        .sub_registry
        .fan_out_scoped(community, &stored)
        .into_iter()
        .filter(|(conn, _)| {
            state.conn_manager.community_for_conn(*conn) == Some(community)
                && state
                    .conn_manager
                    .pubkey_for_conn(*conn)
                    .is_some_and(|pk| allowed.contains(&pk))
        })
        .collect();
    metrics::counter!("kuzz_region_live_events_total").increment(1);
    let dropped = crate::handlers::event::send_to_matches(state, &matches, &stored);
    if dropped > 0 {
        tracing::warn!(dropped, "region live fan-out: connection(s) dropped");
    }
}

/// One link: connect while there is something to watch, reconnect on drops.
async fn run_link(
    state: Arc<AppState>,
    tenant: TenantContext,
    region: String,
    mut requests: mpsc::Receiver<Ensure>,
) {
    let community = tenant.community();
    let mut rooms = Rooms::default();
    let mut backoff = BACKOFF_MIN;
    loop {
        if rooms.idle() {
            let Some(request) = requests.recv().await else {
                return;
            };
            // The caller waits for the watch and reads history after it.
            rooms.add(request, true);
        }
        let mut socket = match connect(&state, &tenant, &region).await {
            Ok(socket) => socket,
            Err(error) => {
                metrics::counter!("kuzz_region_live_connect_failures_total", "target" => region.clone())
                    .increment(1);
                tracing::warn!(target_region = %region, %error, "Region stream connect failed");
                rooms.dropped();
                let retry = tokio::time::sleep(backoff);
                tokio::pin!(retry);
                loop {
                    tokio::select! {
                        () = &mut retry => break,
                        request = requests.recv() => match request {
                            Some(request) => {
                                rooms.add(request, false);
                            }
                            None => return,
                        },
                    }
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
                let watched: HashSet<Uuid> = rooms.readers.keys().copied().collect();
                let now = subscribers(&state, community, &watched);
                rooms.reconcile(now, &global_subscribers(&state, community), false);
                continue;
            }
        };
        tracing::info!(target_region = %region, rooms = rooms.readers.len(), "Region stream connected");
        let opened = Instant::now();
        let end = serve(&state, community, &mut rooms, &mut requests, &mut socket).await;
        let _ = socket.close(None).await;
        rooms.dropped();
        metrics::counter!("kuzz_region_live_drops_total", "target" => region.clone(), "reason" => end)
            .increment(1);
        tracing::info!(target_region = %region, reason = end, "Region stream ended");
        if end == "requests closed" {
            return;
        }
        if rooms.idle() {
            // Nothing is left to resync.
            rooms.resync.clear();
        }
        // A stream that ends soon after it opened backs off like a failed
        // connect, so a home that keeps closing it is not hammered.
        if opened.elapsed() >= STABLE {
            backoff = BACKOFF_MIN;
        } else if end != "idle" {
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }
}

/// Run one connected stream until it ends; returns why.
async fn serve<S>(
    state: &Arc<AppState>,
    community: CommunityId,
    rooms: &mut Rooms,
    requests: &mut mpsc::Receiver<Ensure>,
    socket: &mut S,
) -> &'static str
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + futures_util::Sink<Message>
        + Unpin,
{
    // A new stream: nothing sent on it is unanswered yet.
    rooms.inflight.clear();
    let watched: Vec<Uuid> = rooms.readers.keys().copied().collect();
    for room in watched {
        let Some(frame) = rooms.watch(room).as_ref().and_then(text) else {
            continue;
        };
        if socket.send(frame).await.is_err() {
            return "send error";
        }
    }
    for frame in rooms.watch_all_readers() {
        let Some(frame) = text(&frame) else {
            continue;
        };
        if socket.send(frame).await.is_err() {
            return "send error";
        }
    }
    let mut reconcile = tokio::time::interval(RECONCILE);
    reconcile.tick().await;
    let mut heard = Instant::now();
    loop {
        let outgoing: Vec<EntryFrame> = tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else {
                    return "requests closed";
                };
                rooms.add(request, true)
            }
            incoming = socket.next() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(_)) => return "read error",
                    None => return "closed by home",
                };
                heard = Instant::now();
                let Message::Text(text) = message else {
                    if matches!(message, Message::Close(_)) {
                        return "closed by home";
                    }
                    continue;
                };
                match serde_json::from_str::<HomeFrame>(text.as_str()) {
                    Ok(HomeFrame::Watching { room }) => {
                        if rooms.answered(room) {
                            // Off the link task: legacy sockets close after a delay.
                            tokio::spawn(resync_room(Arc::clone(state), community, room));
                        }
                        Vec::new()
                    }
                    Ok(HomeFrame::Event { room, seq, event, to }) => {
                        if rooms.readers.contains_key(&room) {
                            deliver(state, community, room, seq, event, &to);
                        }
                        Vec::new()
                    }
                    Ok(HomeFrame::Refused { room, reason }) => {
                        metrics::counter!("kuzz_region_live_refusals_total").increment(1);
                        tracing::warn!(%room, %reason, "Home refused a room watch");
                        rooms.refused(room, &reason)
                    }
                    Ok(HomeFrame::GlobalEvent { event, to }) => {
                        deliver_global(state, community, event, &to);
                        Vec::new()
                    }
                    // A frame this version does not know (a newer home).
                    Err(error) => {
                        tracing::warn!(%error, "Ignoring an unknown region stream frame");
                        Vec::new()
                    }
                }
            }
            _ = reconcile.tick() => {
                if heard.elapsed() > SILENCE {
                    return "home silent";
                }
                let watched: HashSet<Uuid> = rooms.readers.keys().copied().collect();
                let frames = rooms.reconcile(
                    subscribers(state, community, &watched),
                    &global_subscribers(state, community),
                    true,
                );
                if rooms.idle() {
                    for frame in &frames {
                        if let Some(message) = text(frame) {
                            let _ = socket.send(message).await;
                        }
                    }
                    return "idle";
                }
                frames
            }
        };
        for frame in outgoing {
            let Some(message) = text(&frame) else {
                continue;
            };
            let sent = tokio::time::timeout(Duration::from_secs(10), socket.send(message)).await;
            if !matches!(sent, Ok(Ok(()))) {
                return "send error";
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ensure(rooms: &[Uuid], reader: u8) -> (Ensure, oneshot::Receiver<()>) {
        let (done, wait) = oneshot::channel();
        (
            Ensure {
                rooms: rooms.to_vec(),
                reader: vec![reader; 32],
                global: false,
                done,
            },
            wait,
        )
    }

    #[test]
    fn a_new_reader_waits_for_the_watch() {
        let room = Uuid::new_v4();
        let mut rooms = Rooms::default();
        let (request, mut wait) = ensure(&[room], 1);
        assert_eq!(rooms.add(request, true).len(), 1);
        assert!(wait.try_recv().is_err(), "waits for watching");
        assert!(!rooms.answered(room));
        assert!(wait.try_recv().is_ok());

        // The same reader again: already live, done at once.
        let (request, mut wait) = ensure(&[room], 1);
        assert!(rooms.add(request, true).is_empty());
        assert!(wait.try_recv().is_ok());

        // Another reader: the room is watched again with both.
        let (request, mut wait) = ensure(&[room], 2);
        assert_eq!(rooms.add(request, true).len(), 1);
        assert_eq!(rooms.readers[&room].len(), 2);
        assert!(wait.try_recv().is_err());
    }

    #[test]
    fn a_waiter_for_several_rooms_waits_for_all() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut rooms = Rooms::default();
        let (request, mut wait) = ensure(&[a, b], 1);
        rooms.add(request, true);
        rooms.answered(a);
        assert!(wait.try_recv().is_err());
        rooms.answered(b);
        assert!(wait.try_recv().is_ok());
    }

    #[test]
    fn a_drop_resyncs_every_room_and_frees_waiters() {
        let room = Uuid::new_v4();
        let mut rooms = Rooms::default();
        let (request, mut wait) = ensure(&[room], 1);
        rooms.add(request, true);
        rooms.live.insert(room);
        rooms.dropped();
        assert!(wait.try_recv().is_ok());
        assert!(rooms.live.is_empty());
        assert!(rooms.resync.contains(&room));

        // Added without a stream: done at once, resynced later.
        let other = Uuid::new_v4();
        let (request, mut wait) = ensure(&[other], 1);
        rooms.add(request, false);
        assert!(wait.try_recv().is_ok());
        assert!(rooms.resync.contains(&other));
    }

    #[test]
    fn reconcile_drops_readers_and_rooms_nobody_subscribes_to() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut rooms = Rooms::default();
        for reader in [1, 2] {
            let (request, _) = ensure(&[a, b], reader);
            rooms.add(request, false);
        }
        let now = HashMap::from([(a, BTreeSet::from([vec![1u8; 32]]))]);
        let frames = rooms.reconcile(now, &BTreeSet::new(), true);
        assert_eq!(frames.len(), 2);
        assert!(frames.contains(&EntryFrame::Unwatch { room: b }));
        assert!(frames.contains(&EntryFrame::Watch {
            room: a,
            readers: vec!["01".repeat(32)],
        }));
        assert_eq!(rooms.readers.len(), 1);
        // Nothing changed: nothing is sent.
        let now = HashMap::from([(a, BTreeSet::from([vec![1u8; 32]]))]);
        assert!(rooms.reconcile(now, &BTreeSet::new(), true).is_empty());
    }

    #[test]
    fn reconcile_waits_for_rooms_being_watched() {
        let room = Uuid::new_v4();
        let mut rooms = Rooms::default();
        let (request, _wait) = ensure(&[room], 1);
        rooms.add(request, true);
        assert!(rooms
            .reconcile(HashMap::new(), &BTreeSet::new(), true)
            .is_empty());
        assert!(rooms.readers.contains_key(&room));
    }

    #[test]
    fn only_the_last_answer_makes_a_room_live() {
        let room = Uuid::new_v4();
        let mut rooms = Rooms::default();
        let (request, mut first) = ensure(&[room], 1);
        rooms.add(request, true);
        let (request, mut second) = ensure(&[room], 2);
        rooms.add(request, true);
        // The answer to the first watch, without reader 2.
        rooms.answered(room);
        assert!(first.try_recv().is_err());
        assert!(second.try_recv().is_err());
        assert!(!rooms.live.contains(&room));
        rooms.answered(room);
        assert!(first.try_recv().is_ok());
        assert!(second.try_recv().is_ok());
        assert!(rooms.live.contains(&room));
        // An answer nobody asked for changes nothing.
        assert!(!rooms.answered(room));
    }

    #[test]
    fn a_caller_that_gave_up_gets_a_resync() {
        let room = Uuid::new_v4();
        let mut rooms = Rooms::default();
        let (request, wait) = ensure(&[room], 1);
        rooms.add(request, true);
        drop(wait);
        assert!(rooms.answered(room), "history was read without the watch");

        // Gave up before the request was taken from the queue.
        let other = Uuid::new_v4();
        let (request, wait) = ensure(&[other], 1);
        drop(wait);
        rooms.add(request, true);
        assert!(rooms.answered(other));
    }

    #[test]
    fn a_temporary_refusal_is_retried_and_others_forget_the_room() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut rooms = Rooms::default();
        let (request, mut wait) = ensure(&[a, b], 1);
        rooms.add(request, true);
        assert!(rooms.refused(a, "error: subscription not ready").is_empty());
        assert_eq!(
            rooms.refused(b, "restricted: not homed here"),
            vec![EntryFrame::Unwatch { room: b }]
        );
        assert!(wait.try_recv().is_ok());
        assert!(rooms.readers.contains_key(&a));
        assert!(!rooms.readers.contains_key(&b));

        // The next reconcile watches `a` again; live, it resyncs.
        let now = HashMap::from([(a, BTreeSet::from([vec![1u8; 32]]))]);
        assert_eq!(rooms.reconcile(now, &BTreeSet::new(), true).len(), 1);
        assert!(rooms.answered(a));
    }

    #[test]
    fn a_room_not_live_is_watched_again_for_a_known_reader() {
        let room = Uuid::new_v4();
        let mut rooms = Rooms::default();
        let (request, _) = ensure(&[room], 1);
        rooms.add(request, false);
        // Same reader on a new stream, with no watch in flight.
        rooms.inflight.clear();
        let (request, _) = ensure(&[room], 1);
        assert_eq!(rooms.add(request, true).len(), 1);
    }

    #[test]
    fn room_less_readers_are_watched_and_dropped_when_gone() {
        let mut rooms = Rooms::default();
        let (done, _) = oneshot::channel();
        let request = Ensure {
            rooms: Vec::new(),
            reader: vec![1; 32],
            global: true,
            done,
        };
        assert_eq!(
            rooms.add(request, true),
            vec![EntryFrame::WatchReaders {
                readers: vec!["01".repeat(32)]
            }]
        );
        assert!(!rooms.idle());
        assert_eq!(rooms.watch_all_readers().len(), 1);
        // Still subscribed: nothing changes.
        let still = BTreeSet::from([vec![1u8; 32]]);
        assert!(rooms.reconcile(HashMap::new(), &still, true).is_empty());
        // A reader whose request was dropped is watched at reconcile.
        let more = BTreeSet::from([vec![1u8; 32], vec![2u8; 32]]);
        assert_eq!(
            rooms.reconcile(HashMap::new(), &more, true),
            vec![EntryFrame::WatchReaders {
                readers: vec!["02".repeat(32)]
            }]
        );
        assert!(rooms.reconcile(HashMap::new(), &still, true).len() == 1);
        // Gone: unwatched, and the link is idle.
        assert_eq!(
            rooms.reconcile(HashMap::new(), &BTreeSet::new(), true),
            vec![EntryFrame::UnwatchReaders {
                readers: vec!["01".repeat(32)]
            }]
        );
        assert!(rooms.idle());
    }

    #[test]
    fn reader_frames_are_chunked() {
        let readers: Vec<Vec<u8>> = (0..READER_CHUNK + 1)
            .map(|i| {
                let mut key = vec![0u8; 32];
                key[..8].copy_from_slice(&(i as u64).to_be_bytes());
                key
            })
            .collect();
        assert_eq!(reader_frames(readers.iter(), true).len(), 2);
    }
}
