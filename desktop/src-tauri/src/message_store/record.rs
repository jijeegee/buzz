//! Fetch writes: every `/query` read the app makes for a room or thread is
//! stored, and what the read proves is claimed. A claim comes only from the
//! request's own bounds and the relay's bounds overlay, never from which
//! events happened to arrive.

use rusqlite::Connection;
use serde_json::Value;
use tauri::AppHandle;

pub(crate) use super::coverage::Claim;
use super::{
    coverage::{cmp_pos, Cursor, Upper, TIME_FLOOR_ID},
    ingest::{self, Class},
    is_hex64, writer_for, ScopeWriter, StoreScope,
};
use crate::app_state::AppState;

/// Relay-signed overlay closing every room window.
const KIND_WINDOW_BOUNDS: u64 = 39006;

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Where one read's results go: the scope of the credential and relay that
/// served it, read once together, and the time the read started.
pub(crate) struct Recorder {
    writer: Option<ScopeWriter>,
    pub(crate) fetched_at: i64,
}

/// [`crate::relay::query_relay`], pinned to one credential and relay so the
/// response is stored in the scope it came from even if the account changes
/// while the request is in flight.
pub(crate) async fn query(
    app: &AppHandle,
    state: &AppState,
    filters: &[Value],
) -> Result<(Vec<nostr::Event>, Recorder), String> {
    let credential = state.user_credential()?;
    let base = crate::relay::relay_api_base_url_with_override(state);
    let fetched_at = unix_now();
    let events =
        crate::relay::query_relay_at_with_keys(state, &base, filters, &credential, None).await?;
    let scope = StoreScope {
        pubkey: credential.public_key().to_hex(),
        relay_url: base,
    };
    let writer = writer_for(app, &scope);
    Ok((events, Recorder { writer, fetched_at }))
}

impl Recorder {
    fn submit<F>(&self, f: F)
    where
        F: FnOnce(&mut Connection) -> Result<(), String> + Send + 'static,
    {
        if let Some(writer) = &self.writer {
            writer.submit(f);
        }
    }

    /// One room window page (`top`), fetched below `cursor` or at the head.
    pub(crate) fn window(&self, channel_id: &str, events: &[Value], cursor: Option<(i64, &str)>) {
        let claim = window_claim(events, channel_id, cursor, self.fetched_at);
        let (channel_id, events) = (channel_id.to_string(), events.to_vec());
        self.submit(move |conn| {
            record_room(conn, &channel_id, &events, Class::Top, claim.as_ref())
        });
    }

    /// One reconnect-repair page (`all`).
    pub(crate) fn repair(&self, channel_id: &str, events: &[Value], request: RepairRequest<'_>) {
        let claim = repair_claim(events, &request, self.fetched_at);
        let (channel_id, events) = (channel_id.to_string(), events.to_vec());
        self.submit(move |conn| {
            record_room(conn, &channel_id, &events, Class::All, claim.as_ref())
        });
    }

    /// Events read by id (a thread root, a linked message): stored, never
    /// claimed, so an old thread's root is at hand like a recent one's.
    pub(crate) fn events(&self, events: &[Value]) {
        let events = events.to_vec();
        self.submit(move |conn| ingest::insert_events(conn, None, &events, unix_now()).map(|_| ()));
    }

    /// One thread page; `claim` from [`bounds_claim`].
    pub(crate) fn thread(
        &self,
        channel_id: &str,
        root_id: &str,
        events: &[Value],
        claim: Option<Claim>,
    ) {
        let (channel_id, events) = (channel_id.to_string(), events.to_vec());
        let root_id = root_id.to_ascii_lowercase();
        self.submit(move |conn| {
            ingest::insert_events(conn, Some(&channel_id), &events, unix_now())?;
            match claim {
                Some(claim) if is_hex64(&root_id) => ingest::claim_thread(conn, &root_id, &claim),
                _ => Ok(()),
            }
        });
    }
}

pub(crate) fn record_room(
    conn: &mut Connection,
    channel_id: &str,
    events: &[Value],
    class: Class,
    claim: Option<&Claim>,
) -> Result<(), String> {
    ingest::insert_events(conn, Some(channel_id), events, unix_now())?;
    match claim {
        Some(claim) => ingest::claim_room(conn, channel_id, class, claim),
        None => Ok(()),
    }
}

fn cursor(created_at: i64, event_id: &str) -> Option<Cursor> {
    is_hex64(event_id).then(|| Cursor {
        created_at,
        event_id: event_id.to_ascii_lowercase(),
    })
}

/// The upper end of a page: below the request cursor, else the fetch time.
fn upper(request_cursor: Option<(i64, &str)>, fetched_at: i64) -> Option<Upper> {
    match request_cursor {
        Some((created_at, event_id)) => cursor(created_at, event_id).map(Upper::Before),
        None => Some(Upper::At(fetched_at)),
    }
}

/// What a window or thread page proves, from its bounds overlay content
/// (`{has_more, next_cursor}`). `next_cursor` is the oldest position the relay
/// scanned, so the page covers it and everything up to the upper end.
pub(crate) fn bounds_claim(
    bounds: &Value,
    request_cursor: Option<(i64, &str)>,
    fetched_at: i64,
) -> Option<Claim> {
    // Only older-direction pages are requested; a newer page proves nothing here.
    if bounds.get("direction").is_some() {
        return None;
    }
    let has_more = bounds.get("has_more")?.as_bool()?;
    let lo = if has_more {
        let next = bounds.get("next_cursor")?;
        Some(cursor(
            next.get("created_at")?.as_i64()?,
            next.get("id")?.as_str()?,
        )?)
    } else {
        None
    };
    Some(Claim {
        lo,
        lo_is_start: !has_more,
        hi: upper(request_cursor, fetched_at)?,
    })
}

fn has_tag(event: &Value, name: &str, value: &str) -> bool {
    event["tags"].as_array().is_some_and(|tags| {
        tags.iter().any(|tag| {
            tag.get(0).and_then(Value::as_str) == Some(name)
                && tag.get(1).and_then(Value::as_str) == Some(value)
        })
    })
}

/// A window page's claim, from its one `39006` overlay. None without it.
pub(crate) fn window_claim(
    events: &[Value],
    channel_id: &str,
    request_cursor: Option<(i64, &str)>,
    fetched_at: i64,
) -> Option<Claim> {
    let bounds = events.iter().rev().find(|event| {
        event["kind"].as_u64() == Some(KIND_WINDOW_BOUNDS) && has_tag(event, "h", channel_id)
    })?;
    let content: Value = serde_json::from_str(bounds["content"].as_str()?).ok()?;
    bounds_claim(
        &content,
        request_cursor,
        answered_at(fetched_at, bounds["created_at"].as_i64()),
    )
}

/// The upper end a head fetch may claim: the local fetch time, but never past
/// the relay's own clock when it signed the page's bounds. A device clock that
/// runs ahead must not claim events the relay had not yet seen, which a later
/// comparison against message times (sender clocks) would then trust.
pub(crate) fn answered_at(fetched_at: i64, bounds_created_at: Option<i64>) -> i64 {
    bounds_created_at.map_or(fetched_at, |relay_at| fetched_at.min(relay_at))
}

/// The bounds of one reconnect-repair request.
pub(crate) struct RepairRequest<'a> {
    pub(crate) since: u64,
    pub(crate) limit: u32,
    pub(crate) until: Option<u64>,
    pub(crate) before_id: Option<&'a str>,
}

/// What one repair page proves (`all`). Results are newest first and the
/// limit cuts the oldest, so a short page is complete down to `since`, and a
/// full page is complete down to its oldest event (the next page's cursor,
/// which it joins).
pub(crate) fn repair_claim(
    events: &[Value],
    request: &RepairRequest<'_>,
    fetched_at: i64,
) -> Option<Claim> {
    let since = i64::try_from(request.since).ok()?;
    let hi = match (request.until, request.before_id) {
        (Some(until), Some(before_id)) => Upper::Before(cursor(until as i64, before_id)?),
        (Some(until), None) => Upper::At((until as i64).min(fetched_at)),
        (None, _) => Upper::At(fetched_at),
    };
    if events.len() < request.limit as usize {
        return Some(Claim {
            lo: Some(Cursor {
                created_at: since,
                event_id: TIME_FLOOR_ID.to_string(),
            }),
            lo_is_start: since == 0,
            hi,
        });
    }
    let mut oldest: Option<Cursor> = None;
    for event in events {
        let found = cursor(event["created_at"].as_i64()?, event["id"].as_str()?)?;
        let older = oldest.as_ref().is_none_or(|current| {
            cmp_pos(
                found.created_at,
                &found.event_id,
                current.created_at,
                &current.event_id,
            )
            .is_lt()
        });
        if older {
            oldest = Some(found);
        }
    }
    Some(Claim {
        lo: Some(oldest?),
        lo_is_start: false,
        hi,
    })
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
