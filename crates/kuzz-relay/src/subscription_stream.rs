//! Per-connection state of a user stream (`{"#h":["$joined"]}` REQ).
//!
//! The stream is an ordinary scoped entry of the
//! [`SubscriptionRegistry`](crate::subscription::SubscriptionRegistry): its
//! filter is the frozen kinds and its scope the member rooms, so fan-out,
//! topic retention and teardown are the generic ones. What is special lives
//! here: the snapshot phase (live frames queue until every part and the EOSE
//! are out), the connection's mode, and the owner token the stream was
//! claimed with.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use kuzz_core::tenant::TenantContext;

use crate::subscription::SubId;
use crate::user_stream_config::StreamMode;

/// Delivery state of one stream.
#[derive(Debug)]
enum Phase {
    /// Snapshot parts are being sent; live frames wait here (bounded).
    Snapshot(Vec<Arc<Bytes>>),
    /// EOSE is out and the queue flushed; frames go straight to the socket.
    Live,
    /// Closed (overflow, error or replaced); frames are dropped.
    Closed,
}

/// What [`StreamHandle::deliver`] did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Handed to the socket; `false` when the send failed.
    Sent(bool),
    /// Held until the snapshot EOSE.
    Queued,
    /// The pre-EOSE queue is full: the stream is now closed and the caller
    /// must send `error: user-stream overflow`.
    Overflow,
    /// The stream is closed; the frame is dropped.
    Dropped,
}

/// One connection's user stream.
#[derive(Debug)]
pub struct StreamHandle {
    /// Client sub id of the stream REQ.
    pub sub_id: SubId,
    /// Lifecycle owner token the stream was claimed with.
    pub owner: u64,
    /// Mode for this connection (snapshot part 1 `mode`).
    pub mode: StreamMode,
    /// Tenant of the connection (topic release on teardown).
    pub tenant: TenantContext,
    /// A token's channel scope: rooms outside it are never carried.
    allowed_rooms: Option<Vec<uuid::Uuid>>,
    queue_cap: usize,
    phase: Mutex<Phase>,
    /// CLOSED reason and whether to cancel the socket, set by a close that
    /// cannot run the async teardown itself (fan-out is synchronous).
    close_request: Mutex<Option<(&'static str, bool)>>,
    /// Woken by every close; the stream's teardown task waits on it.
    closed: tokio::sync::Notify,
    /// Membership changes that arrived before the stream's rooms were read
    /// (query A): `(room, carried)` in arrival order, replayed on top of
    /// query A's result. `None` once the rooms are set. Touched only under
    /// the connection's subscription lock.
    room_ops: Mutex<Option<Vec<(uuid::Uuid, bool)>>>,
}

impl StreamHandle {
    /// A stream in its snapshot phase, queueing at most `queue_cap` live frames.
    pub fn new(
        sub_id: SubId,
        owner: u64,
        mode: StreamMode,
        tenant: TenantContext,
        queue_cap: usize,
    ) -> Self {
        Self {
            sub_id,
            owner,
            mode,
            tenant,
            allowed_rooms: None,
            queue_cap,
            phase: Mutex::new(Phase::Snapshot(Vec::new())),
            close_request: Mutex::new(None),
            closed: tokio::sync::Notify::new(),
            room_ops: Mutex::new(Some(Vec::new())),
        }
    }

    /// Limit the stream to a token's channel scope.
    pub fn limited_to(mut self, rooms: Option<Vec<uuid::Uuid>>) -> Self {
        self.allowed_rooms = rooms;
        self
    }

    /// Whether the stream may carry `room` (inside the token's scope, if any).
    pub fn allows(&self, room: uuid::Uuid) -> bool {
        self.allowed_rooms
            .as_ref()
            .is_none_or(|rooms| rooms.contains(&room))
    }

    /// Hold a membership change until the rooms are set; `false` when they
    /// already are (apply it to the registry instead).
    pub fn hold_room_op(&self, room: uuid::Uuid, carried: bool) -> bool {
        match &mut *self
            .room_ops
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
        {
            Some(ops) => {
                ops.push((room, carried));
                true
            }
            None => false,
        }
    }

    /// The changes held since registration, in order; later ones apply
    /// directly. Empty when called twice.
    pub fn take_room_ops(&self) -> Vec<(uuid::Uuid, bool)> {
        self.room_ops
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
            .unwrap_or_default()
    }

    fn phase(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Route one live frame: queue it during the snapshot, send it once live.
    /// `send` must not block (it is called under the phase lock, which keeps
    /// queued frames ahead of later ones).
    pub fn deliver(&self, frame: Arc<Bytes>, send: impl FnOnce(Arc<Bytes>) -> bool) -> Delivery {
        let mut phase = self.phase();
        match &mut *phase {
            Phase::Live => Delivery::Sent(send(frame)),
            Phase::Closed => Delivery::Dropped,
            Phase::Snapshot(queue) => {
                if queue.len() >= self.queue_cap {
                    *phase = Phase::Closed;
                    Delivery::Overflow
                } else {
                    queue.push(frame);
                    Delivery::Queued
                }
            }
        }
    }

    /// After the EOSE: send the queued frames in order and go live. `false`
    /// when the stream was closed meanwhile (nothing sent) or a send failed
    /// (the stream is closed).
    #[must_use]
    pub fn go_live(&self, mut send: impl FnMut(Arc<Bytes>) -> bool) -> bool {
        let mut phase = self.phase();
        let Phase::Snapshot(queue) = std::mem::replace(&mut *phase, Phase::Live) else {
            *phase = Phase::Closed;
            return false;
        };
        for frame in queue {
            if !send(frame) {
                *phase = Phase::Closed;
                return false;
            }
        }
        true
    }

    /// Whether the stream is still open (snapshot or live).
    pub fn is_open(&self) -> bool {
        !matches!(*self.phase(), Phase::Closed)
    }

    /// Close the stream; later frames are dropped. `true` if it was open.
    pub fn close(&self) -> bool {
        let was_open = {
            let mut phase = self.phase();
            !matches!(std::mem::replace(&mut *phase, Phase::Closed), Phase::Closed)
        };
        self.closed.notify_one();
        was_open
    }

    /// Close from a synchronous path and ask the teardown task to retire the
    /// stream, send `reason`, and cancel the socket when `cancel`.
    pub fn request_close(&self, reason: &'static str, cancel: bool) {
        self.close_request
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get_or_insert((reason, cancel));
        self.close();
    }

    /// Wait until the stream is closed; returns the requested CLOSED, if any.
    pub async fn until_closed(&self) -> Option<(&'static str, bool)> {
        while self.is_open() {
            self.closed.notified().await;
        }
        *self
            .close_request
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Frames waiting for the EOSE (tests and logs).
    pub fn queued(&self) -> usize {
        match &*self.phase() {
            Phase::Snapshot(queue) => queue.len(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(cap: usize) -> StreamHandle {
        StreamHandle::new(
            "s".to_string(),
            1,
            StreamMode::Live,
            TenantContext::resolved(
                kuzz_core::CommunityId::from_uuid(uuid::Uuid::new_v4()),
                "t.example".to_string(),
            ),
            cap,
        )
    }

    fn frame(n: u8) -> Arc<Bytes> {
        Arc::new(Bytes::from(vec![n]))
    }

    #[test]
    fn frames_queue_until_live_then_flush_in_order() {
        let h = handle(3);
        assert_eq!(h.deliver(frame(1), |_| unreachable!()), Delivery::Queued);
        assert_eq!(h.deliver(frame(2), |_| unreachable!()), Delivery::Queued);
        let mut sent = Vec::new();
        assert!(h.go_live(|f| {
            sent.push(f[0]);
            true
        }));
        assert_eq!(
            h.deliver(frame(3), |f| {
                sent.push(f[0]);
                true
            }),
            Delivery::Sent(true)
        );
        assert_eq!(sent, [1, 2, 3]);
    }

    #[test]
    fn a_full_queue_closes_the_stream() {
        let h = handle(2);
        h.deliver(frame(1), |_| true);
        h.deliver(frame(2), |_| true);
        assert_eq!(h.deliver(frame(3), |_| true), Delivery::Overflow);
        h.request_close("r", true);
        assert!(!h.is_open());
        assert_eq!(h.deliver(frame(4), |_| unreachable!()), Delivery::Dropped);
        assert!(
            !h.go_live(|_| unreachable!()),
            "no EOSE flush after overflow"
        );
    }

    #[test]
    fn room_ops_are_held_until_taken() {
        let h = handle(2);
        let (a, b) = (uuid::Uuid::from_u128(1), uuid::Uuid::from_u128(2));
        assert!(h.hold_room_op(a, true));
        assert!(h.hold_room_op(b, false));
        assert_eq!(h.take_room_ops(), [(a, true), (b, false)]);
        assert!(!h.hold_room_op(a, false), "applied directly once taken");
        assert!(h.take_room_ops().is_empty());
    }

    #[test]
    fn close_is_reported_once() {
        let h = handle(2);
        assert!(h.close());
        assert!(!h.close());
    }
}
