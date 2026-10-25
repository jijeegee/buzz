//! Reconnect catch-up: every joined room is brought up to date from the
//! scope's watermark with one paged query per 128 rooms, instead of one
//! repair walk per room. The answer is stored with what it proves; the
//! watermark then moves to the catch-up's start time, but only if the socket
//! it was run for is still open (see `message_store::sync`).

use std::collections::HashSet;
use std::future::Future;

use serde::Serialize;
use serde_json::Value;
use tauri::State;

use crate::{
    app_state::AppState,
    message_store::{
        record::{unix_now, Claim},
        sync::{self, Commit, Plan},
        SYNC_KINDS,
    },
    native_websocket::WebSocketManager,
};

/// Rooms one catch-up may name (a bound, not a product limit).
const MAX_CATCH_UP_ROOMS: usize = 4096;
/// Chunk queries in flight at once.
const CATCH_UP_CONCURRENCY: usize = 4;

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CatchUpStatus {
    /// Every room not listed as a gap was caught up from `since`.
    CaughtUp,
    /// Nothing stored vouches for this server yet.
    Fresh,
    /// The watermark is too old to catch up from.
    Stale,
    /// The socket is gone or stores into another scope.
    Unavailable,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatchUpResult {
    status: CatchUpStatus,
    since: Option<i64>,
    /// Caught-up events of the dispatch rooms, relay order per chunk.
    events: Vec<Value>,
    /// Rooms the catch-up could not prove (overflow or error).
    gap_channel_ids: Vec<String>,
}

impl CatchUpResult {
    fn without_query(status: CatchUpStatus) -> Self {
        Self {
            status,
            since: None,
            events: Vec::new(),
            gap_channel_ids: Vec::new(),
        }
    }
}

fn catch_up_filter(rooms: &[String], since: i64, cursor: Option<&(i64, String)>) -> Value {
    let mut filter = serde_json::json!({
        "#h": rooms,
        "kinds": SYNC_KINDS,
        "since": since,
        "limit": sync::CATCH_UP_PAGE_LIMIT,
    });
    if let Some((until, before_id)) = cursor {
        filter["until"] = serde_json::json!(until);
        filter["before_id"] = serde_json::json!(before_id);
    }
    filter
}

/// One chunk's answer.
#[derive(Debug)]
struct ChunkResult {
    events: Vec<Value>,
    /// Every page came back and the last one was short.
    complete: bool,
    /// When the first page was asked for.
    fetched_at: i64,
}

/// Pages one chunk down to `since`, up to `CATCH_UP_MAX_PAGES`.
async fn catch_up_chunk<Q, F>(rooms: &[String], since: i64, query: Q) -> ChunkResult
where
    Q: Fn(Value) -> F,
    F: Future<Output = Result<Vec<Value>, String>>,
{
    let fetched_at = unix_now();
    let mut events = Vec::new();
    let mut cursor = None;
    for _ in 0..sync::CATCH_UP_MAX_PAGES {
        let page = match query(catch_up_filter(rooms, since, cursor.as_ref())).await {
            Ok(page) => page,
            Err(error) => {
                eprintln!("kuzz-desktop: catch-up query failed: {error}");
                return ChunkResult {
                    events,
                    complete: false,
                    fetched_at,
                };
            }
        };
        let next = sync::next_page_cursor(&page);
        events.extend(page);
        match next {
            None => {
                return ChunkResult {
                    events,
                    complete: true,
                    fetched_at,
                }
            }
            Some(next) => cursor = Some(next),
        }
    }
    ChunkResult {
        events,
        complete: false,
        fetched_at,
    }
}

fn room_of(event: &Value) -> Option<&str> {
    event["tags"].as_array()?.iter().find_map(|tag| {
        (tag.get(0)?.as_str()? == "h")
            .then(|| tag.get(1)?.as_str())
            .flatten()
    })
}

fn valid_rooms(channel_ids: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut rooms = Vec::new();
    for id in channel_ids {
        let id = uuid::Uuid::parse_str(&id)
            .map_err(|_| "invalid channel id".to_string())?
            .to_string();
        if seen.insert(id.clone()) {
            rooms.push(id);
        }
    }
    if rooms.len() > MAX_CATCH_UP_ROOMS {
        return Err(format!("at most {MAX_CATCH_UP_ROOMS} rooms"));
    }
    Ok(rooms)
}

/// What the chunk answers prove and return: events to store, claims per room
/// of a complete chunk, gap rooms otherwise, and the dispatch rooms' events.
fn assemble(
    chunks: Vec<(Vec<String>, ChunkResult)>,
    since: i64,
    dispatch: &HashSet<String>,
) -> (Commit, Vec<Value>, Vec<String>) {
    let mut commit = Commit {
        events: Vec::new(),
        claims: Vec::new(),
    };
    let (mut dispatched, mut gaps) = (Vec::new(), Vec::new());
    for (rooms, result) in chunks {
        if result.complete {
            let claim: Claim = sync::chunk_claim(since, result.fetched_at);
            commit
                .claims
                .extend(rooms.into_iter().map(|room| (room, claim.clone())));
        } else {
            gaps.extend(rooms);
        }
        for event in result.events {
            if room_of(&event).is_some_and(|room| dispatch.contains(room)) {
                dispatched.push(event.clone());
            }
            commit.events.push(event);
        }
    }
    (commit, dispatched, gaps)
}

/// Catches the given rooms up for socket `ws_id`. `dispatch_channel_ids`
/// (also caught up) are the rooms whose events the caller wants back.
#[tauri::command]
pub async fn channels_catch_up(
    ws_id: u32,
    channel_ids: Vec<String>,
    dispatch_channel_ids: Vec<String>,
    state: State<'_, AppState>,
    sockets: State<'_, WebSocketManager>,
) -> Result<CatchUpResult, String> {
    let dispatch: HashSet<String> = valid_rooms(dispatch_channel_ids)?.into_iter().collect();
    let mut rooms = valid_rooms(channel_ids)?;
    let named: HashSet<String> = rooms.iter().cloned().collect();
    rooms.extend(dispatch.iter().filter(|id| !named.contains(*id)).cloned());
    if rooms.len() > MAX_CATCH_UP_ROOMS {
        return Err(format!("at most {MAX_CATCH_UP_ROOMS} rooms"));
    }
    // No rooms known yet (the channel list has not loaded): catching up
    // nothing must not move the watermark past rooms it never asked about.
    if rooms.is_empty() {
        return Ok(CatchUpResult::without_query(CatchUpStatus::Unavailable));
    }

    let Some(tap) = sockets.tap_for(ws_id).await else {
        return Ok(CatchUpResult::without_query(CatchUpStatus::Unavailable));
    };
    // One credential and server for the whole run, and only the socket's own.
    let credential = state.user_credential()?;
    let base = crate::relay::relay_api_base_url_with_override(&state);
    let scope = tap.scope();
    if !credential
        .public_key()
        .to_hex()
        .eq_ignore_ascii_case(scope.pubkey.trim())
        || crate::auth::origin_for(&base) != crate::auth::origin_for(&scope.relay_url)
    {
        return Ok(CatchUpResult::without_query(CatchUpStatus::Unavailable));
    }

    let started_at = unix_now();
    let watermark = tap.writer().call(|conn| sync::watermark(conn)).await?;
    let since = match sync::plan(watermark, started_at) {
        Plan::From { since } => since,
        plan => {
            // Nothing to catch up from: the per-room repair covers this
            // connection, and from now on the socket moves the watermark.
            let tap_for_job = tap.clone();
            tap.writer()
                .call(move |conn| {
                    let at = tap_for_job.mark_caught_up().then_some(started_at);
                    match at {
                        Some(at) => sync::advance_watermark(conn, at),
                        None => Ok(()),
                    }
                })
                .await?;
            let status = if plan == Plan::Fresh {
                CatchUpStatus::Fresh
            } else {
                CatchUpStatus::Stale
            };
            return Ok(CatchUpResult::without_query(status));
        }
    };

    let state_ref = &state;
    let (credential_ref, base_ref) = (&credential, base.as_str());
    let query = |filter: Value| async move {
        let events = crate::relay::query_relay_at_with_keys(
            state_ref,
            base_ref,
            &[filter],
            credential_ref,
            None,
        )
        .await?;
        Ok(events
            .iter()
            .filter_map(|event| serde_json::to_value(event).ok())
            .collect::<Vec<Value>>())
    };
    use futures_util::StreamExt;
    let room_chunks: Vec<Vec<String>> = rooms
        .chunks(sync::CATCH_UP_CHUNK_ROOMS)
        .map(<[String]>::to_vec)
        .collect();
    let query = &query;
    let chunks: Vec<(Vec<String>, ChunkResult)> =
        futures_util::stream::iter(room_chunks.into_iter().map(|chunk| async move {
            let result = catch_up_chunk(&chunk, since, query).await;
            (chunk, result)
        }))
        .buffered(CATCH_UP_CONCURRENCY)
        .collect()
        .await;

    let (commit, events, gap_channel_ids) = assemble(chunks, since, &dispatch);
    let tap_for_job = tap.clone();
    tap.writer()
        .call(move |conn| {
            sync::commit(conn, commit, || {
                tap_for_job.mark_caught_up().then_some(started_at)
            })
        })
        .await?;
    Ok(CatchUpResult {
        status: CatchUpStatus::CaughtUp,
        since: Some(since),
        events,
        gap_channel_ids,
    })
}

#[cfg(test)]
#[path = "channels_catch_up_tests.rs"]
mod tests;
