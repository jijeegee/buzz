//! Live rooms across regions (global regions stage S4-4a): the home side.
//!
//! A region with subscribers in a room that lives elsewhere watches the room
//! at its home over one WebSocket per region pair
//! (`GET /internal/region/stream`, attested like every link call). The
//! entry names the room and the readers it watches it for; the home sends
//! each new event of the room with its order number and the readers allowed
//! to see it. "Allowed" is decided here, by the same gate as local fan-out
//! ([`filter_recipients_by_access`]): the home owns the room's membership,
//! visibility and blocks.
//!
//! Frames are JSON text. Entry to home:
//! - `{"op":"watch","room":<uuid>,"readers":[<hex pubkey>…]}` sets the
//!   readers of a room (sent again whenever they change).
//! - `{"op":"unwatch","room":<uuid>}`.
//!
//! Home to entry:
//! - `{"op":"watching","room":<uuid>}` once a watch is live: every event
//!   published after it is streamed. An entry reads a room's history after
//!   this, so nothing falls between the history and the stream.
//! - `{"op":"event","room":<uuid>,"seq":<n|null>,"event":{…},"to":[<hex>…]}`.
//!   `seq` is null when the event was published without its order number.
//! - `{"op":"refused","room":<uuid>,"reason":"…"}` when a watch cannot be
//!   served (unknown room, the room lives elsewhere, limits).
//!
//! The stream carries only what happens while it is open. It is closed
//! rather than left with a hole: when the entry falls behind, when this
//! region's Redis reconnects (events published meanwhile never arrived),
//! when pongs stop, and after [`MAX_LIFETIME`] so the entry is admitted
//! again with its current key. On any close the entry tells its apps to
//! resync, and they fill gaps by number.
//!
//! **Trust.** A stream is a region-level call: the home takes the readers
//! the entry names on the entry's word, as every link call takes the
//! principal the entry attests. A region key therefore vouches for every
//! user of that region, and the entry keeps applying its own credential
//! limits (channel-limited tokens) to the subscriptions it watches for.
//!
//! Only mounted with the link on (`KUZZ_REGION_LINK`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use kuzz_core::event::StoredEvent;
use kuzz_core::tenant::TenantContext;
use kuzz_pubsub::EventTopic;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::handlers::event::filter_recipients_by_access;
use crate::routing::RoomRoute;
use crate::state::AppState;

/// Path the home serves region streams on.
pub const STREAM_PATH: &str = "/internal/region/stream";

/// Rooms one stream may watch.
const MAX_ROOMS: usize = 10_000;

/// Readers one watch may name.
const MAX_READERS: usize = 5_000;

/// Readers one stream may name across its rooms.
const MAX_READERS_TOTAL: usize = 200_000;

/// Largest frame an entry may send.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// How long one frame may take to send before the entry counts as stalled.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a new watch may wait for this region's Redis subscription.
const WATCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Keep-alive ping interval, so idle links through proxies stay open and a
/// dead peer is noticed.
pub const PING_INTERVAL: Duration = Duration::from_secs(20);

/// Missed pongs after which the entry counts as gone.
const PONG_GRACE: Duration = Duration::from_secs(60);

/// How long one stream stays open; the entry then reconnects and is
/// admitted again.
pub const MAX_LIFETIME: Duration = Duration::from_secs(3600);

/// A frame from the entry region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EntryFrame {
    /// Watch a room for some readers, replacing any earlier set.
    Watch {
        /// The room.
        room: Uuid,
        /// Hex pubkeys of the readers subscribed to it at the entry.
        readers: Vec<String>,
    },
    /// Stop watching a room.
    Unwatch {
        /// The room.
        room: Uuid,
    },
}

/// A frame from the home region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HomeFrame {
    /// A watch is live.
    Watching {
        /// The room.
        room: Uuid,
    },
    /// A new event of a watched room, for the readers in `to`.
    Event {
        /// The room.
        room: Uuid,
        /// The event's order number in the room, when published with it.
        seq: Option<i64>,
        /// The event.
        event: nostr::Event,
        /// Hex pubkeys of the watching readers allowed to see it.
        to: Vec<String>,
    },
    /// A watch the home does not serve.
    Refused {
        /// The room.
        room: Uuid,
        /// Why, as a NIP-01 `CLOSED` style reason.
        reason: String,
    },
}

/// Decode hex pubkeys, sorted and without repeats; `None` if any is not a
/// 32-byte key.
fn decode_readers(readers: &[String]) -> Option<Vec<Vec<u8>>> {
    let mut decoded = readers
        .iter()
        .map(|reader| hex::decode(reader).ok().filter(|bytes| bytes.len() == 32))
        .collect::<Option<Vec<_>>>()?;
    decoded.sort_unstable();
    decoded.dedup();
    Some(decoded)
}

/// Home side: `GET /internal/region/stream`. Admits the calling region's
/// attestation (region-level, no principal) and upgrades.
pub async fn stream(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let refused =
        || crate::api::api_error(StatusCode::UNAUTHORIZED, "region link refused").into_response();
    let call = match crate::region_link::admit(&state, &Method::GET, &uri, &headers, b"").await {
        Ok(call) => call,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a region stream");
            return refused();
        }
    };
    if call.claims.principal.is_some() {
        return refused();
    }
    metrics::counter!("kuzz_region_link_calls_accepted_total", "from" => call.claims.entry_region.clone()).increment(1);
    let entry = call.claims.entry_region;
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| run(state, call.tenant, entry, socket))
}

/// The rooms one stream watches, with their readers.
#[derive(Default)]
struct Watched {
    rooms: HashMap<Uuid, Vec<Vec<u8>>>,
    readers: usize,
}

/// Serve one stream until either side closes it.
async fn run(state: Arc<AppState>, tenant: TenantContext, entry: String, socket: WebSocket) {
    let (mut sink, mut source) = socket.split();
    // Subscribe before the first watch is taken, so nothing published after
    // a watch is live is missed.
    let mut events = state.pubsub.subscribe_local();
    let mut resync = state.pubsub.subscribe_resync();
    let community = tenant.community();
    let mut watched = Watched::default();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;
    let mut last_pong = Instant::now();
    let lifetime = tokio::time::sleep(MAX_LIFETIME);
    tokio::pin!(lifetime);
    metrics::gauge!("kuzz_region_streams_open", "from" => entry.clone()).increment(1.0);
    tracing::info!(from = %entry, "Region stream opened");

    let end = loop {
        let outgoing = tokio::select! {
            incoming = source.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<EntryFrame>(text.as_str()) {
                        Ok(frame) => apply(&state, &tenant, &mut watched, frame).await,
                        Err(_) => break "malformed frame",
                    }
                }
                Some(Ok(Message::Pong(_))) => {
                    last_pong = Instant::now();
                    None
                }
                Some(Ok(Message::Close(_))) | None => break "closed by entry",
                Some(Ok(_)) => None,
                Some(Err(_)) => break "read error",
            },
            received = events.recv() => match received {
                Ok(event) => deliver(&state, community, &watched, event).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => break "lagged",
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break "shutdown",
            },
            _ = resync.recv() => break "home resync",
            _ = ping.tick() => {
                if last_pong.elapsed() > PONG_GRACE {
                    break "no pong";
                }
                Some(Message::Ping(Vec::new().into()))
            }
            () = &mut lifetime => break "lifetime",
        };
        let Some(message) = outgoing else {
            continue;
        };
        match tokio::time::timeout(SEND_TIMEOUT, sink.send(message)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => break "send error",
            Err(_) => break "entry stalled",
        }
    };

    let _ = sink.close().await;
    for room in watched.rooms.keys() {
        state
            .pubsub
            .release_topic(&tenant, EventTopic::Channel(*room))
            .await;
    }
    metrics::gauge!("kuzz_region_streams_open", "from" => entry.clone()).decrement(1.0);
    metrics::counter!("kuzz_region_streams_closed_total", "from" => entry.clone(), "reason" => end)
        .increment(1);
    tracing::info!(from = %entry, reason = end, "Region stream closed");
}

/// Apply an entry frame; returns the answer to send, if any.
async fn apply(
    state: &AppState,
    tenant: &TenantContext,
    watched: &mut Watched,
    frame: EntryFrame,
) -> Option<Message> {
    let refuse = |room: Uuid, reason: &str| {
        encode(&HomeFrame::Refused {
            room,
            reason: reason.to_string(),
        })
    };
    match frame {
        EntryFrame::Watch { room, readers } => {
            if readers.len() > MAX_READERS {
                return refuse(room, "rate-limited: too many readers");
            }
            let Some(readers) = decode_readers(&readers) else {
                return refuse(room, "invalid: malformed reader");
            };
            let before = watched.rooms.get(&room).map_or(0, Vec::len);
            if watched.readers - before + readers.len() > MAX_READERS_TOTAL {
                return refuse(room, "rate-limited: too many readers");
            }
            // Checked on every watch, so a room that moved is refused.
            match state
                .room_router
                .route(&state.db, tenant.community(), room)
                .await
            {
                Ok(RoomRoute::Here) => {}
                Ok(RoomRoute::Elsewhere(home)) => {
                    unwatch(state, tenant, watched, room).await;
                    return refuse(room, &home.ws_message());
                }
                Err(_) => return refuse(room, "error: room lookup failed"),
            }
            if !watched.rooms.contains_key(&room) {
                if watched.rooms.len() >= MAX_ROOMS {
                    return refuse(room, "rate-limited: too many rooms");
                }
                if state
                    .db
                    .get_channel(tenant.community(), room)
                    .await
                    .is_err()
                {
                    return refuse(room, "invalid: unknown room");
                }
                state
                    .pubsub
                    .retain_topic(tenant, EventTopic::Channel(room))
                    .await;
                let live = tokio::time::timeout(WATCH_TIMEOUT, state.pubsub.flush_subscriptions())
                    .await
                    .unwrap_or(false);
                if !live {
                    state
                        .pubsub
                        .release_topic(tenant, EventTopic::Channel(room))
                        .await;
                    return refuse(room, "error: room stream not ready, try again");
                }
            }
            watched.readers = watched.readers - before + readers.len();
            watched.rooms.insert(room, readers);
            encode(&HomeFrame::Watching { room })
        }
        EntryFrame::Unwatch { room } => {
            unwatch(state, tenant, watched, room).await;
            None
        }
    }
}

async fn unwatch(state: &AppState, tenant: &TenantContext, watched: &mut Watched, room: Uuid) {
    if let Some(readers) = watched.rooms.remove(&room) {
        watched.readers -= readers.len();
        state
            .pubsub
            .release_topic(tenant, EventTopic::Channel(room))
            .await;
    }
}

/// The frame for a published event of a watched room, or `None` when the
/// room is not watched or no watching reader may see it.
async fn deliver(
    state: &AppState,
    community: kuzz_core::CommunityId,
    watched: &Watched,
    published: kuzz_pubsub::ChannelEvent,
) -> Option<Message> {
    if published.community_id != community {
        return None;
    }
    let EventTopic::Channel(room) = published.topic else {
        return None;
    };
    let readers = watched.rooms.get(&room)?;
    let mut stored = StoredEvent::new(published.event, Some(room));
    stored.channel_sequence = published.channel_sequence;
    let recipients = readers
        .iter()
        .map(|reader| (reader.clone(), Some(reader.clone())))
        .collect();
    let allowed = filter_recipients_by_access(state, community, &stored, recipients, None).await;
    if allowed.is_empty() {
        return None;
    }
    metrics::counter!("kuzz_region_stream_events_sent_total").increment(1);
    encode(&HomeFrame::Event {
        room,
        seq: stored.channel_sequence,
        event: stored.event,
        to: allowed.iter().map(hex::encode).collect(),
    })
}

fn encode(frame: &HomeFrame) -> Option<Message> {
    serde_json::to_string(frame)
        .ok()
        .map(|text| Message::Text(text.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_frames_parse() {
        let room = Uuid::new_v4();
        let watch: EntryFrame = serde_json::from_value(serde_json::json!({
            "op": "watch", "room": room, "readers": ["ab".repeat(32)]
        }))
        .unwrap();
        assert_eq!(
            watch,
            EntryFrame::Watch {
                room,
                readers: vec!["ab".repeat(32)]
            }
        );
        let unwatch: EntryFrame =
            serde_json::from_value(serde_json::json!({"op": "unwatch", "room": room})).unwrap();
        assert_eq!(unwatch, EntryFrame::Unwatch { room });
        assert!(serde_json::from_value::<EntryFrame>(serde_json::json!({"op": "drop"})).is_err());
    }

    #[test]
    fn readers_must_be_keys_and_are_deduplicated() {
        assert_eq!(
            decode_readers(&["02".repeat(32), "01".repeat(32), "02".repeat(32)]),
            Some(vec![vec![1u8; 32], vec![2u8; 32]])
        );
        assert_eq!(decode_readers(&["01".repeat(31)]), None);
        assert_eq!(decode_readers(&["zz".repeat(32)]), None);
        assert_eq!(decode_readers(&[]), Some(Vec::new()));
    }

    #[test]
    fn home_frame_shapes() {
        let room = Uuid::new_v4();
        for (frame, op) in [
            (
                HomeFrame::Refused {
                    room,
                    reason: "x".into(),
                },
                "refused",
            ),
            (HomeFrame::Watching { room }, "watching"),
        ] {
            let Some(Message::Text(text)) = encode(&frame) else {
                panic!("no frame");
            };
            let value: serde_json::Value = serde_json::from_str(text.as_str()).unwrap();
            assert_eq!(value["op"], op);
            assert_eq!(value["room"], room.to_string());
        }
    }
}
