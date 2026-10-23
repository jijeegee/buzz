//! Global room registration (global regions stage S4-6).
//!
//! The primary region's `room_directory` is the one list of every room's
//! home. A region creating a room first reserves the room id there for
//! itself, then creates the room; opening a new DM reserves its participant
//! set the same way, so two regions opening the same DM at once end on one
//! room (the later one is sent to the home of the first). A region that
//! meets a room it does not know asks the primary where it lives. Without
//! the global copy it keeps the answer in its own directory; with the copy
//! (S5) the primary's directory reaches every region by itself, and asking
//! covers only the moment before it arrives. Reservations always go to the
//! primary.
//!
//! If the primary cannot be reached, creating a room or a new DM fails with
//! a retryable error (late rather than split); looking a room up falls back
//! to "not known here".

use std::sync::Arc;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::Json;
use kuzz_core::tenant::TenantContext;
use kuzz_core::CommunityId;
use kuzz_db::region::{ReservedRoom, RoomHome};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::handlers::ingest::IngestError;
use crate::routing::RemoteHome;
use crate::state::AppState;

/// Primary side: reserve a room id (or DM participant set) for the caller.
pub const RESERVE_PATH: &str = "/internal/region/reserve-room";
/// Primary side: where a room lives.
pub const LOOKUP_PATH: &str = "/internal/region/room-home";

const UNREACHABLE: &str = "error: could not reach the primary region; try again";
/// A lookup waits at most this long; routing waits on it.
const LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// What the primary said about a room this region does not know.
#[derive(Debug)]
pub enum Asked {
    /// It lives in another region.
    Elsewhere(RemoteHome),
    /// No region has it (or the link is off, or this is the primary).
    Unknown,
    /// The primary could not be asked: the caller fails retryably.
    Failed,
}

/// Where a room being created must be created.
#[derive(Debug)]
pub enum Reserved {
    /// Here, with this room id (for a DM, the id the directory holds).
    Here(Uuid),
    /// Another region already holds it.
    Elsewhere(RemoteHome),
}

#[derive(Deserialize)]
struct ReserveRequest {
    channel_id: Uuid,
    home: String,
    #[serde(default)]
    participant_hash: Option<String>,
}

#[derive(Deserialize)]
struct LookupRequest {
    channel_id: Uuid,
}

fn is_primary(state: &AppState) -> bool {
    state.config.primary_region == state.config.region_id
}

/// Keep a room's home learned from the primary in this region's directory.
/// A copy region (S5) does not: the primary's row reaches it by the copy.
async fn remember(
    state: &AppState,
    community: CommunityId,
    channel_id: Uuid,
    home: &kuzz_db::region::RoomHome,
) {
    if state.config.global_copy {
        return;
    }
    if let Err(error) = state
        .db
        .remember_room_home(community, channel_id, home)
        .await
    {
        tracing::warn!(%error, "Could not remember a room's home");
    }
}

fn home_from(answer: &Value) -> Option<RoomHome> {
    Some(RoomHome {
        region: answer.get("home_region")?.as_str()?.to_string(),
        epoch: i32::try_from(answer.get("epoch")?.as_i64()?).ok()?,
    })
}

async fn remote_home(state: &AppState, channel_id: Uuid, home: RoomHome) -> RemoteHome {
    let connect_url = match state.db.region_link(&home.region).await {
        Ok(Some((connect_url, _))) => connect_url,
        _ => String::new(),
    };
    RemoteHome {
        region: home.region,
        connect_url,
        epoch: home.epoch,
        room: Some(channel_id),
    }
}

/// Reserve `channel_id` for this region before creating it here; for a DM
/// pass its `participant_hash`. With the link off or one region it is a
/// no-op.
pub async fn reserve(
    state: &AppState,
    tenant: &TenantContext,
    channel_id: Uuid,
    participant_hash: Option<&[u8]>,
) -> Result<Reserved, IngestError> {
    if !state.config.region_link {
        return Ok(Reserved::Here(channel_id));
    }
    let others = state
        .room_router
        .other_regions(&state.db)
        .await
        .map_err(|error| IngestError::Internal(format!("error: {error}")))?;
    if others.is_empty() {
        return Ok(Reserved::Here(channel_id));
    }
    let community = tenant.community();
    let local = state.config.region_id.as_str();
    let reserved = if is_primary(state) {
        state
            .db
            .reserve_room_home(community, channel_id, local, participant_hash)
            .await
            .map_err(|error| match error {
                kuzz_db::DbError::InvalidData(message) => {
                    IngestError::Rejected(format!("invalid: {message}"))
                }
                error => IngestError::Internal(format!("error: {error}")),
            })?
    } else {
        let body = json!({
            "channel_id": channel_id,
            "home": local,
            "participant_hash": participant_hash.map(hex::encode),
        });
        let answer = crate::region_link::call(
            state,
            &state.config.primary_region,
            *community.as_uuid(),
            RESERVE_PATH,
            &body,
            None,
            None,
        )
        .await;
        metrics::counter!(
            "kuzz_region_room_reservations_total",
            "outcome" => if answer.is_ok() { "ok" } else { "failed" }
        )
        .increment(1);
        let answer = match answer {
            Ok(answer) => answer,
            Err(crate::region_link::CallError::Status(status, answer))
                if status == StatusCode::BAD_REQUEST =>
            {
                let message = answer["message"].as_str().unwrap_or("invalid: room id");
                return Err(IngestError::Rejected(message.to_string()));
            }
            Err(error) => {
                tracing::warn!(?error, "Could not reserve a room at the primary region");
                return Err(IngestError::Internal(UNREACHABLE.into()));
            }
        };
        let (Some(channel_id), Some(home)) = (
            answer["channel_id"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok()),
            home_from(&answer),
        ) else {
            return Err(IngestError::Internal(UNREACHABLE.into()));
        };
        ReservedRoom { channel_id, home }
    };
    state.room_router.forget(community, channel_id);
    state.room_router.forget(community, reserved.channel_id);
    if reserved.home.region == local {
        return Ok(Reserved::Here(reserved.channel_id));
    }
    // Keep the answer here so later operations route without asking.
    remember(state, community, reserved.channel_id, &reserved.home).await;
    Ok(Reserved::Elsewhere(
        remote_home(state, reserved.channel_id, reserved.home).await,
    ))
}

/// A room this region's directory does not know: ask the primary where it
/// lives and remember it. After a failed lookup the primary is not asked
/// for a few seconds, and only a bounded number of lookups run at once, so
/// unknown or random room ids cannot pile up calls.
pub async fn ask_primary(state: &AppState, community: CommunityId, channel_id: Uuid) -> Asked {
    if !state.config.region_link || is_primary(state) {
        return Asked::Unknown;
    }
    if state.room_router.recently_unknown(community, channel_id) {
        return Asked::Unknown;
    }
    let Some(_slot) = state.room_router.primary_lookup_slot() else {
        metrics::counter!("kuzz_region_room_lookups_total", "outcome" => "skipped").increment(1);
        return Asked::Failed;
    };
    let answer = tokio::time::timeout(
        LOOKUP_TIMEOUT,
        crate::region_link::call(
            state,
            &state.config.primary_region,
            *community.as_uuid(),
            LOOKUP_PATH,
            &json!({ "channel_id": channel_id }),
            None,
            None,
        ),
    )
    .await;
    let answer = match answer {
        Ok(Ok(answer)) => answer,
        Ok(Err(error)) => {
            tracing::warn!(?error, "Could not ask the primary region for a room's home");
            state.room_router.note_primary_down();
            metrics::counter!("kuzz_region_room_lookups_total", "outcome" => "failed").increment(1);
            return Asked::Failed;
        }
        Err(_) => {
            tracing::warn!("Asking the primary region for a room's home timed out");
            state.room_router.note_primary_down();
            metrics::counter!("kuzz_region_room_lookups_total", "outcome" => "failed").increment(1);
            return Asked::Failed;
        }
    };
    metrics::counter!("kuzz_region_room_lookups_total", "outcome" => "ok").increment(1);
    let Some(home) = home_from(&answer) else {
        state.room_router.note_unknown(community, channel_id);
        return Asked::Unknown;
    };
    if home.region == state.config.region_id {
        // The primary says it is ours, but it is not here (yet).
        state.room_router.note_unknown(community, channel_id);
        return Asked::Unknown;
    }
    remember(state, community, channel_id, &home).await;
    state.room_router.forget(community, channel_id);
    Asked::Elsewhere(remote_home(state, channel_id, home).await)
}

fn refused() -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "refused", "message": "region link refused" })),
    )
}

fn answer_error(status: StatusCode, error: &str, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": error, "message": message })))
}

/// `POST /internal/region/reserve-room` (primary): reserve a room id, or a
/// DM's participant set, for the calling region. A region reserves only for
/// itself.
pub async fn reserve_room(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let call = match crate::region_link::admit(&state, &Method::POST, &uri, &headers, &body).await {
        Ok(call) => call,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a room reservation");
            return refused();
        }
    };
    if !is_primary(&state) {
        return answer_error(
            StatusCode::CONFLICT,
            "not_primary",
            "not the primary region",
        );
    }
    let Ok(request) = serde_json::from_slice::<ReserveRequest>(&body) else {
        return answer_error(
            StatusCode::BAD_REQUEST,
            "malformed",
            "invalid: malformed reservation",
        );
    };
    if request.home != call.claims.entry_region {
        return answer_error(
            StatusCode::BAD_REQUEST,
            "malformed",
            "invalid: a region reserves only for itself",
        );
    }
    let hash = match request.participant_hash.as_deref().map(hex::decode) {
        None => None,
        Some(Ok(hash)) if hash.len() == 32 => Some(hash),
        Some(_) => {
            return answer_error(
                StatusCode::BAD_REQUEST,
                "malformed",
                "invalid: participant hash",
            )
        }
    };
    let community = call.tenant.community();
    match state
        .db
        .reserve_room_home(
            community,
            request.channel_id,
            &request.home,
            hash.as_deref(),
        )
        .await
    {
        Ok(reserved) => {
            state.room_router.forget(community, request.channel_id);
            state.room_router.forget(community, reserved.channel_id);
            (
                StatusCode::OK,
                Json(json!({
                    "channel_id": reserved.channel_id,
                    "home_region": reserved.home.region,
                    "epoch": reserved.home.epoch,
                })),
            )
        }
        Err(kuzz_db::DbError::InvalidData(message)) => answer_error(
            StatusCode::BAD_REQUEST,
            "rejected",
            &format!("invalid: {message}"),
        ),
        Err(error) => {
            tracing::warn!(%error, "Room reservation failed");
            answer_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
        }
    }
}

/// `POST /internal/region/room-home` (primary): where a room lives;
/// `home_region` is null when the directory has no row for it.
pub async fn room_home(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let call = match crate::region_link::admit(&state, &Method::POST, &uri, &headers, &body).await {
        Ok(call) => call,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a room lookup");
            return refused();
        }
    };
    let Ok(request) = serde_json::from_slice::<LookupRequest>(&body) else {
        return answer_error(
            StatusCode::BAD_REQUEST,
            "malformed",
            "invalid: malformed lookup",
        );
    };
    match state
        .db
        .room_home(call.tenant.community(), request.channel_id)
        .await
    {
        Ok(Some(home)) => (
            StatusCode::OK,
            Json(json!({ "home_region": home.region, "epoch": home.epoch })),
        ),
        Ok(None) => (StatusCode::OK, Json(json!({ "home_region": null }))),
        Err(error) => {
            tracing::warn!(%error, "Room lookup failed");
            answer_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
        }
    }
}
