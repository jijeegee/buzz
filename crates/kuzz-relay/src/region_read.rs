//! Reads of rooms another region serves (global regions stage S4-3).
//!
//! A read that names only rooms of one other region (`REQ`/`COUNT` with `#h`,
//! HTTP `/query` and `/count`, including thread windows and `after_seq`
//! pages) is answered by that region over the region link: the entry sends
//! the reader's filters with an attestation of who it admitted, the home runs
//! its ordinary read path for that reader and returns the result unchanged.
//! Every region of one service signs with the same relay key, so relay-signed
//! events in the answer (room metadata, `39006` page bounds) verify as this
//! relay's own.
//!
//! A WS `REQ` gets the stored events and `EOSE`; live events of a remote room
//! follow with the region stream (S4-4). Reads that mix regions are still
//! refused with "wrong region" until they are merged.
//!
//! Only runs with the link on (`KUZZ_REGION_LINK`).

use std::sync::Arc;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::Json;
use kuzz_core::tenant::TenantContext;
use kuzz_core::CommunityId;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::region_link::{CallError, EntryAuth, EntryPrincipal};
use crate::state::AppState;

/// Path a home region answers proxied reads on.
pub const QUERY_PATH: &str = "/internal/region/query";

/// The message a reader gets when the room's region could not be reached.
pub const UNREACHABLE: &str = "error: the room's region is unreachable, try again";

/// Which read the entry asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    /// `/query` (events, thread windows, `after_seq` pages).
    Query,
    /// `/count`.
    Count,
}

/// Body of a proxied read.
#[derive(Debug, Deserialize)]
struct RemoteRead {
    mode: ReadMode,
    filters: Vec<Value>,
}

/// The one other region that serves every room in `channels`, when the link
/// is on. `None` when the link is off, nothing is named, a room is served
/// here (or is unknown here), or the rooms span several regions.
pub(crate) async fn single_remote_home(
    state: &AppState,
    community: CommunityId,
    channels: &[Uuid],
) -> Result<Option<String>, kuzz_db::DbError> {
    if !state.config.region_link || channels.is_empty() {
        return Ok(None);
    }
    let split = state
        .room_router
        .split(&state.db, community, channels.to_vec())
        .await?;
    if !split.here.is_empty() || split.elsewhere.len() != 1 {
        return Ok(None);
    }
    Ok(split.elsewhere.into_keys().next())
}

/// The reader as the entry admitted them, with the scopes their credential
/// carries (an empty list is an unrestricted login).
pub(crate) fn reader(
    pubkey: &nostr::PublicKey,
    auth: EntryAuth,
    scopes: &[kuzz_auth::Scope],
    channel_ids: Option<Vec<Uuid>>,
) -> EntryPrincipal {
    let scopes = if scopes.is_empty() {
        kuzz_auth::Scope::all_known()
    } else {
        scopes.to_vec()
    };
    EntryPrincipal {
        pubkey: pubkey.to_hex(),
        auth,
        scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
        channel_ids,
    }
}

/// The rooms every filter names, or `None` when a filter names none (such a
/// read spans every room the reader can open, in every region).
pub(crate) fn rooms_of_every_filter(sets: &[Option<Vec<Uuid>>]) -> Option<Vec<Uuid>> {
    let mut rooms = Vec::new();
    for set in sets {
        match set {
            Some(set) if !set.is_empty() => rooms.extend_from_slice(set),
            _ => return None,
        }
    }
    (!rooms.is_empty()).then_some(rooms)
}

/// Entry side: ask `region` to run `filters` for `reader`. Returns the
/// home's answer (an event array for [`ReadMode::Query`], `{"count": n}` for
/// [`ReadMode::Count`]) or its refusal as an HTTP error.
pub(crate) async fn read_at_home(
    state: &AppState,
    tenant: &TenantContext,
    region: &str,
    mode: ReadMode,
    filters: Vec<Value>,
    reader: EntryPrincipal,
) -> Result<Value, (StatusCode, Json<Value>)> {
    let body = json!({ "mode": mode, "filters": filters });
    let answer = crate::region_link::call(
        state,
        region,
        *tenant.community().as_uuid(),
        QUERY_PATH,
        &body,
        None,
        Some(reader),
    )
    .await;
    metrics::counter!(
        "kuzz_region_proxied_reads_total",
        "target" => region.to_string(),
        "outcome" => if answer.is_ok() { "ok" } else { "failed" }
    )
    .increment(1);
    match answer {
        Ok(answer) => Ok(answer),
        // The home's own refusal (not a member, bad filter, moved room) is
        // the reader's answer. A refused link (401) is not the reader's fault.
        Err(CallError::Status(status, answer))
            if status != StatusCode::UNAUTHORIZED && answer.get("error").is_some() =>
        {
            Err((status, Json(answer)))
        }
        Err(error) => {
            tracing::warn!(target_region = %region, ?error, "Proxied read failed");
            Err(crate::api::api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                UNREACHABLE,
            ))
        }
    }
}

/// The refusal text of an HTTP error answer, for a WS `CLOSED`.
pub(crate) fn closed_message(error: &(StatusCode, Json<Value>)) -> String {
    let body = &error.1 .0;
    if body.get("error").and_then(Value::as_str) == Some("wrong_region") {
        let region = body.get("region").and_then(Value::as_str).unwrap_or("");
        let url = body
            .get("connect_url")
            .and_then(Value::as_str)
            .unwrap_or("");
        return format!(
            "{} room lives in {region} {url}",
            crate::routing::WRONG_REGION_PREFIX
        )
        .trim_end()
        .to_string();
    }
    let message = body
        .get("error")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(UNREACHABLE);
    if message.contains(':') {
        message.to_string()
    } else {
        format!("error: {message}")
    }
}

/// A WS filter as `/query` JSON: the filter plus its `before_id` and
/// `after_seq` cursors, which `nostr::Filter` does not carry.
pub(crate) fn raw_filter(
    filter: &nostr::Filter,
    before_id: Option<&[u8]>,
    after_seq: Option<i64>,
) -> Value {
    let mut raw = serde_json::to_value(filter).unwrap_or_else(|_| json!({}));
    if let Some(object) = raw.as_object_mut() {
        if let Some(before_id) = before_id {
            object.insert("before_id".into(), hex::encode(before_id).into());
        }
        if let Some(after_seq) = after_seq {
            object.insert("after_seq".into(), after_seq.into());
        }
    }
    raw
}

/// Home side: `POST /internal/region/query`. Admits the calling region's
/// attestation and runs the read for the attested reader, without
/// forwarding it again.
pub async fn query(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let refused = || {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "refused", "message": "region link refused" })),
        )
    };
    let call = match crate::region_link::admit(&state, &Method::POST, &uri, &headers, &body).await {
        Ok(call) => call,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a proxied read");
            return refused();
        }
    };
    let Some(principal) = call.claims.principal.clone() else {
        return refused();
    };
    let (Ok(read), Ok(pubkey)) = (
        serde_json::from_slice::<RemoteRead>(&body),
        nostr::PublicKey::from_hex(&principal.pubkey),
    ) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid: malformed read" })),
        );
    };
    // A channel-limited credential reads only its channels.
    if let Some(allowed) = principal.channel_ids.as_deref() {
        let named = named_channels(&read.filters);
        let every_filter_named = read
            .filters
            .iter()
            .all(|filter| !named_channels(std::slice::from_ref(filter)).is_empty());
        if !every_filter_named || named.iter().any(|channel| !allowed.contains(channel)) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "restricted: channel not allowed for this token" })),
            );
        }
    }
    metrics::counter!("kuzz_region_link_calls_accepted_total", "from" => call.claims.entry_region.clone()).increment(1);
    let filters = Value::Array(read.filters).to_string();
    let result = crate::api::bridge::read_for_region(
        &state,
        &call.tenant,
        filters.as_bytes(),
        pubkey,
        read.mode,
    )
    .await;
    match result {
        Ok(Json(answer)) => (StatusCode::OK, Json(answer)),
        Err(error) => error,
    }
}

/// Rooms the raw filters name in `#h`.
fn named_channels(filters: &[Value]) -> Vec<Uuid> {
    filters
        .iter()
        .filter_map(|filter| filter.get("#h").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_cursors_travel_with_the_filter() {
        let room = Uuid::new_v4();
        let filter = nostr::Filter::new()
            .custom_tag(
                nostr::SingleLetterTag::lowercase(nostr::Alphabet::H),
                room.to_string(),
            )
            .until(nostr::Timestamp::from(100))
            .limit(5);
        let raw = raw_filter(&filter, Some(&[0xab; 32]), Some(7));
        assert_eq!(raw["#h"], json!([room.to_string()]));
        assert_eq!(raw["until"], 100);
        assert_eq!(raw["before_id"], "ab".repeat(32));
        assert_eq!(raw["after_seq"], 7);
        assert_eq!(named_channels(&[raw]), vec![room]);
        let plain = raw_filter(&filter, None, None);
        assert!(plain.get("before_id").is_none() && plain.get("after_seq").is_none());
    }

    #[test]
    fn a_filter_without_rooms_keeps_the_read_local() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(
            rooms_of_every_filter(&[Some(vec![a]), Some(vec![b])]),
            Some(vec![a, b])
        );
        assert_eq!(rooms_of_every_filter(&[Some(vec![a]), None]), None);
        assert_eq!(rooms_of_every_filter(&[Some(vec![a]), Some(vec![])]), None);
        assert_eq!(rooms_of_every_filter(&[]), None);
    }

    #[test]
    fn home_refusals_read_as_ws_closed_messages() {
        let wrong = (
            StatusCode::MISDIRECTED_REQUEST,
            Json(
                json!({"error": "wrong_region", "region": "sg", "connect_url": "wss://sg.example"}),
            ),
        );
        assert_eq!(
            closed_message(&wrong),
            "wrong-region: room lives in sg wss://sg.example"
        );
        let denied = (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "restricted: not a member"})),
        );
        assert_eq!(closed_message(&denied), "restricted: not a member");
        let bare = (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "bad filter"})),
        );
        assert_eq!(closed_message(&bare), "error: bad filter");
    }
}
