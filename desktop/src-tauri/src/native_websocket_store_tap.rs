//! Live socket tap: room events a scoped socket receives go to that scope's
//! message store before the webview sees them.
//!
//! Only the main relay session names a scope when it connects
//! (`relayClientSession.ts`); read-only account watchers do not, so nothing
//! they receive is stored. Events are insert-only. Coverage comes from live
//! segments: a subscription whose filter asks for every synced kind of some
//! rooms, and nothing narrower, proves those rooms from its `since` (or from
//! when it was sent, if its stored replay was cut by its limit) once it
//! reaches EOSE, until it is closed or the socket exits.
//!
//! The tap also moves the scope's catch-up watermark (see
//! `message_store::sync`), but only once a catch-up has run for this socket:
//! a REQ it answered with EOSE vouches for everything sent before the REQ,
//! and an event a live segment delivered for everything before it.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, PoisonError},
};

use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};

use crate::message_store::{
    live::{self, LiveOp, Segments},
    record::unix_now,
    ScopeWriter, StoreScope, SYNC_KINDS,
};

const KIND_THREAD_SUMMARY: u64 = 39005;
/// Filter keys a live-segment subscription may carry. Anything else (authors,
/// `#e`, `until`, ids, search) narrows it below "every synced event".
const SEGMENT_FILTER_KEYS: [&str; 4] = ["kinds", "#h", "since", "limit"];

/// The scope a `connect` config names (`{storeScope: {pubkey, relayUrl}}`),
/// when it is for the socket's own server. Pure.
pub(crate) fn requested_scope(config: Option<&Value>, url: &str) -> Option<StoreScope> {
    let scope: StoreScope = serde_json::from_value(config?.get("storeScope")?.clone()).ok()?;
    (crate::auth::origin_for(&scope.relay_url) == crate::auth::origin_for(url)).then_some(scope)
}

/// The writer for a socket being opened: only when the config names a scope
/// for this server and the signed-in account.
pub(crate) fn writer_for_connect<R: Runtime>(
    app: &AppHandle<R>,
    url: &str,
    config: Option<&Value>,
) -> Option<(StoreScope, ScopeWriter)> {
    let scope = requested_scope(config, url)?;
    let state = app.try_state::<crate::app_state::AppState>()?;
    let signed_in = state.user_credential().ok()?.public_key().to_hex();
    if !signed_in.eq_ignore_ascii_case(scope.pubkey.trim()) {
        eprintln!("kuzz-desktop: message store: socket scope is not the signed-in account");
        return None;
    }
    let writer = crate::message_store::writer_for(app, &scope)?;
    Some((scope, writer))
}

/// Outstanding REQs remembered for their round trip (bounded).
const MAX_ROUND_TRIPS: usize = 4096;

/// A REQ that may become a live segment once it reaches EOSE.
struct PendingReq {
    channel_ids: Vec<String>,
    since: Option<i64>,
    limit: Option<u64>,
    sent_at: i64,
    replayed: u64,
}

#[derive(Default)]
struct TapState {
    ops: Vec<LiveOp>,
    pending: HashMap<String, PendingReq>,
    live: HashSet<String>,
    /// When each outstanding REQ was sent, by subscription id.
    round_trips: HashMap<String, i64>,
    /// A catch-up ran for this socket: its frames may move the watermark.
    caught_up: bool,
    /// Newest watermark vouched for since the last flush.
    watermark: Option<i64>,
    last_frame_at: Option<i64>,
    finished: bool,
}

pub(crate) struct StoreTap {
    ws_id: i64,
    scope: StoreScope,
    writer: ScopeWriter,
    segments: Arc<Mutex<Segments>>,
    state: Mutex<TapState>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `["TYPE", ...]` parsed, only for the frame types the tap reads.
fn parse(text: &str, types: &[&str]) -> Option<(String, Vec<Value>)> {
    let head = text.trim_start().strip_prefix('[')?.trim_start();
    if !types
        .iter()
        .any(|kind| head.starts_with(&format!("\"{kind}\"")))
    {
        return None;
    }
    let Value::Array(mut parts) = serde_json::from_str(text).ok()? else {
        return None;
    };
    if parts.is_empty() {
        return None;
    }
    let Value::String(kind) = parts.remove(0) else {
        return None;
    };
    Some((kind, parts))
}

/// Whether the store keeps `event`: a synced kind or thread summary with a room tag.
fn is_room_event(event: &Value) -> bool {
    let synced = event["kind"].as_u64().is_some_and(|kind| {
        kind == KIND_THREAD_SUMMARY || SYNC_KINDS.iter().any(|k| u64::from(*k) == kind)
    });
    synced
        && event["tags"].as_array().is_some_and(|tags| {
            tags.iter().any(|tag| {
                tag.get(0).and_then(Value::as_str) == Some("h")
                    && tag
                        .get(1)
                        .and_then(Value::as_str)
                        .is_some_and(|room| !room.is_empty())
            })
        })
}

/// A filter that delivers every synced event of its rooms from its `since`.
fn segment_request(filters: &[Value], sent_at: i64) -> Option<PendingReq> {
    let [Value::Object(filter)] = filters else {
        return None;
    };
    if filter
        .keys()
        .any(|key| !SEGMENT_FILTER_KEYS.contains(&key.as_str()))
    {
        return None;
    }
    let kinds: HashSet<u64> = filter
        .get("kinds")?
        .as_array()?
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    if !SYNC_KINDS
        .iter()
        .all(|kind| kinds.contains(&u64::from(*kind)))
    {
        return None;
    }
    let channel_ids: Vec<String> = filter
        .get("#h")?
        .as_array()?
        .iter()
        .map(|room| room.as_str().map(str::to_string))
        .collect::<Option<_>>()?;
    if channel_ids.is_empty() {
        return None;
    }
    Some(PendingReq {
        channel_ids,
        since: filter.get("since").and_then(Value::as_i64),
        limit: filter.get("limit").and_then(Value::as_u64),
        sent_at,
        replayed: 0,
    })
}

impl TapState {
    fn vouch(&mut self, at: i64) {
        if self.caught_up {
            self.watermark = Some(self.watermark.map_or(at, |current| current.max(at)));
        }
    }

    fn end(&mut self, sub_id: &str) {
        self.pending.remove(sub_id);
        self.round_trips.remove(sub_id);
        if self.live.remove(sub_id) {
            let at = self.last_frame_at.unwrap_or_else(unix_now);
            self.ops.push(LiveOp::End {
                sub_id: sub_id.to_string(),
                at,
            });
        }
    }

    fn eose(&mut self, sub_id: &str) {
        // The relay answered a REQ on this socket: everything sent before
        // the REQ was delivered.
        if let Some(sent_at) = self.round_trips.remove(sub_id) {
            self.vouch(sent_at);
        }
        let Some(request) = self.pending.remove(sub_id) else {
            return;
        };
        let floor = request.since.unwrap_or(i64::MIN);
        // A replay cut by its limit proves nothing before the REQ was sent.
        let complete =
            request.since.is_some() && request.limit.is_some_and(|limit| request.replayed < limit);
        let since = if complete {
            floor
        } else {
            request.sent_at.max(floor)
        };
        self.live.insert(sub_id.to_string());
        self.ops.push(LiveOp::Begin {
            sub_id: sub_id.to_string(),
            channel_ids: request.channel_ids,
            since,
        });
    }
}

impl StoreTap {
    pub(crate) fn new(ws_id: u32, scope: StoreScope, writer: ScopeWriter) -> Self {
        Self {
            ws_id: i64::from(ws_id),
            scope,
            writer,
            segments: Arc::default(),
            state: Mutex::default(),
        }
    }

    /// Every frame the relay sent, before it is queued for the webview.
    pub(crate) fn incoming(&self, text: Option<&str>) {
        let mut state = lock(&self.state);
        if state.finished {
            return;
        }
        state.last_frame_at = Some(unix_now());
        let Some((kind, parts)) = text.and_then(|text| parse(text, &["EVENT", "EOSE", "CLOSED"]))
        else {
            return;
        };
        let Some(sub_id) = parts.first().and_then(Value::as_str) else {
            return;
        };
        match kind.as_str() {
            "EVENT" => {
                if let Some(request) = state.pending.get_mut(sub_id) {
                    request.replayed += 1;
                }
                let delivered_live = state.live.contains(sub_id);
                if let Some(event) = parts.get(1).filter(|event| is_room_event(event)) {
                    // A live event vouches for what came before it; a future
                    // timestamp vouches for no more than now.
                    if let (true, Some(created_at), Some(now)) = (
                        delivered_live,
                        event["created_at"].as_i64(),
                        state.last_frame_at,
                    ) {
                        state.vouch(created_at.min(now));
                    }
                    state.ops.push(LiveOp::Event(event.clone()));
                }
            }
            "EOSE" => {
                let sub_id = sub_id.to_string();
                state.eose(&sub_id);
            }
            _ => {
                let sub_id = sub_id.to_string();
                state.end(&sub_id);
            }
        }
    }

    /// Every text frame the webview sends on this socket.
    pub(crate) fn outgoing(&self, text: &str) {
        let Some((kind, parts)) = parse(text, &["REQ", "CLOSE"]) else {
            return;
        };
        let Some(sub_id) = parts.first().and_then(Value::as_str).map(str::to_string) else {
            return;
        };
        {
            let mut state = lock(&self.state);
            if state.finished {
                return;
            }
            // A REQ reusing an id replaces that subscription.
            state.end(&sub_id);
            if kind == "REQ" {
                let sent_at = unix_now();
                if state.round_trips.len() < MAX_ROUND_TRIPS {
                    state.round_trips.insert(sub_id.clone(), sent_at);
                }
                if let Some(request) = segment_request(&parts[1..], sent_at) {
                    state.pending.insert(sub_id, request);
                }
            }
        }
        self.flush();
    }

    /// Hands queued work to the store. Called before every webview flush, so
    /// the webview never sees an event the store has not been given.
    pub(crate) fn flush(&self) {
        let mut state = lock(&self.state);
        if let Some(at) = state.watermark.take() {
            state.ops.push(LiveOp::Watermark(at));
        }
        if state.ops.is_empty() {
            return;
        }
        let mut ops = std::mem::take(&mut state.ops);
        if let Some(at) = state.last_frame_at {
            ops.push(LiveOp::Frame(at));
        }
        drop(state);
        let (ws_id, segments) = (self.ws_id, Arc::clone(&self.segments));
        self.writer
            .submit(move |conn| live::apply(conn, ws_id, &mut lock(&segments), ops));
    }

    /// The scope this socket stores into.
    pub(crate) fn scope(&self) -> &StoreScope {
        &self.scope
    }

    pub(crate) fn writer(&self) -> &ScopeWriter {
        &self.writer
    }

    /// A catch-up for this socket committed: from now on its round trips and
    /// live events move the watermark. False once the socket has exited (a
    /// dead socket vouches for nothing).
    pub(crate) fn mark_caught_up(&self) -> bool {
        let mut state = lock(&self.state);
        if state.finished {
            return false;
        }
        state.caught_up = true;
        true
    }

    /// The socket exited: its segments close at its last frame.
    pub(crate) fn finish(&self) {
        {
            let mut state = lock(&self.state);
            if state.finished {
                return;
            }
            state.finished = true;
            state.pending.clear();
            state.live.clear();
            state.round_trips.clear();
            if let Some(at) = state.last_frame_at {
                state.ops.push(LiveOp::EndAll { at });
            }
        }
        self.flush();
    }
}

#[cfg(test)]
#[path = "native_websocket_store_tap_tests.rs"]
mod tests;
