//! What the live socket tap (`native_websocket_store_tap.rs`) asks of one
//! scope, applied in order on the scope's thread.

use std::collections::HashMap;

use rusqlite::Connection;
use serde_json::Value;

use super::{ingest, record::unix_now, sync};

#[derive(Debug, PartialEq)]
pub(crate) enum LiveOp {
    /// Insert-only; never a coverage claim.
    Event(Value),
    /// A subscription reached EOSE: its rooms are live from `since`.
    Begin {
        sub_id: String,
        channel_ids: Vec<String>,
        since: i64,
    },
    /// A subscription ended (CLOSE, CLOSED or replaced) at `at`.
    End { sub_id: String, at: i64 },
    /// The socket exited: close every segment it opened at `at`.
    EndAll { at: i64 },
    /// Newest frame time, so a crash closes open segments no later than it.
    Frame(i64),
    /// The socket (caught up) vouches for everything before this time.
    Watermark(i64),
}

/// Open segments of one socket, by subscription id. Lives as long as the
/// socket and is touched only on the scope's thread.
pub(crate) type Segments = HashMap<String, i64>;

pub(crate) fn apply(
    conn: &mut Connection,
    ws_id: i64,
    segments: &mut Segments,
    ops: Vec<LiveOp>,
) -> Result<(), String> {
    let mut events = Vec::new();
    let flush = |conn: &mut Connection, events: &mut Vec<Value>| -> Result<(), String> {
        if !events.is_empty() {
            ingest::insert_events(conn, None, events, unix_now())?;
            events.clear();
        }
        Ok(())
    };
    for op in ops {
        if let LiveOp::Event(event) = op {
            events.push(event);
            continue;
        }
        flush(conn, &mut events)?;
        match op {
            LiveOp::Event(_) => unreachable!("handled above"),
            LiveOp::Begin {
                sub_id,
                channel_ids,
                since,
            } => {
                let segment = ingest::begin_live(conn, ws_id, &channel_ids, since)?;
                if let Some(replaced) = segments.insert(sub_id, segment) {
                    ingest::close_segment(conn, replaced, since)?;
                }
            }
            LiveOp::End { sub_id, at } => {
                if let Some(segment) = segments.remove(&sub_id) {
                    ingest::close_segment(conn, segment, at)?;
                }
            }
            LiveOp::EndAll { at } => {
                segments.clear();
                ingest::close_live(conn, Some(ws_id), at)?;
            }
            LiveOp::Frame(at) => ingest::note_frame(conn, at)?,
            LiveOp::Watermark(at) => sync::advance_watermark(conn, at)?,
        }
    }
    flush(conn, &mut events)
}
