//! Forwarded writes (global regions stage S4-2).
//!
//! A write that reaches a region that does not serve its room is sent on to
//! the room's home region over the region link instead of being refused. The
//! entry region forwards the original event with an attestation of who it
//! admitted ([`crate::region_link`]); the home runs its ordinary ingest for
//! that principal ([`IngestAuth::Forwarded`]) and answers with the same
//! result an app connected to it would get: accepted, the room's order
//! number, or the refusal. A duplicate answers with the original number.
//!
//! The home checks the room's epoch the entry trusted. When the room has
//! moved, or its epoch changed, it answers "wrong region" with the current
//! home and the entry retries once there. Every other failure is a plain
//! retryable error: a forwarded write is applied at the home or not at all.
//!
//! Only runs with the link on (`KUZZ_REGION_LINK`); otherwise the S3 refusal
//! is returned unchanged.

use std::sync::Arc;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::Json;
use kuzz_core::tenant::TenantContext;
use nostr::Event;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::handlers::ingest::{HttpAuthMethod, IngestAuth, IngestError, IngestResult};
use crate::region_link::{CallError, EntryAuth, EntryPrincipal};
use crate::routing::RemoteHome;
use crate::state::AppState;

/// Path a home region takes forwarded writes on.
pub const EVENTS_PATH: &str = "/internal/region/events";

/// The message an app gets when the room's region could not be reached. A
/// plain `error:` so apps retry the send.
const UNREACHABLE: &str = "error: the room's region is unreachable, try again";

/// Body of a forwarded write.
#[derive(Debug, Deserialize)]
struct ForwardedWrite {
    event: Event,
}

/// Who the entry region admitted, for the attestation. `None` for logins
/// that are not forwarded: dev header logins and writes that were already
/// forwarded once.
pub(crate) fn principal_for(auth: &IngestAuth) -> Option<EntryPrincipal> {
    let (pubkey, entry_auth, scopes, channel_ids) = match auth {
        IngestAuth::Nip42 {
            pubkey,
            scopes,
            channel_ids,
            ..
        } => (pubkey, EntryAuth::Nip42, scopes, channel_ids.clone()),
        IngestAuth::Http {
            pubkey,
            scopes,
            auth_method: HttpAuthMethod::Nip98,
        } => (pubkey, EntryAuth::Nip98, scopes, None),
        IngestAuth::Token { pubkey, scopes, .. } => (pubkey, EntryAuth::Token, scopes, None),
        IngestAuth::Http {
            auth_method: HttpAuthMethod::DevPubkey,
            ..
        }
        | IngestAuth::Forwarded { .. } => return None,
    };
    Some(EntryPrincipal {
        pubkey: pubkey.to_hex(),
        auth: entry_auth,
        scopes: scopes.iter().map(|scope| scope.to_string()).collect(),
        channel_ids,
    })
}

/// Entry side: send `event` to the room's home region and return its answer
/// as this region's ingest result. Retries once when the home answers that
/// the room moved.
pub(crate) async fn forward_write(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    event: &Event,
    auth: &IngestAuth,
    mut home: RemoteHome,
) -> Result<IngestResult, IngestError> {
    let Some(principal) = principal_for(auth) else {
        return Err(IngestError::WrongRegion(home));
    };
    let body = json!({ "event": event });
    for attempt in 0..2 {
        let room = home.room.map(|channel| (channel, home.epoch));
        let answer = crate::region_link::call(
            state,
            &home.region,
            *tenant.community().as_uuid(),
            EVENTS_PATH,
            &body,
            room,
            Some(principal.clone()),
        )
        .await;
        metrics::counter!(
            "kuzz_region_forwarded_writes_total",
            "target" => home.region.clone(),
            "outcome" => match &answer {
                Ok(_) => "ok",
                Err(CallError::Status(status, _)) if status.is_client_error() => "refused",
                Err(_) => "failed",
            }
        )
        .increment(1);
        match answer {
            Ok(answer) => return result_from_answer(&answer),
            Err(CallError::Status(StatusCode::MISDIRECTED_REQUEST, answer)) if attempt == 0 => {
                if let Some(channel) = home.room {
                    state.room_router.forget(tenant.community(), channel);
                }
                match moved_home(&answer, home.room) {
                    Some(next) if next.region != state.config.region_id => home = next,
                    // Moved here, or no usable answer: the app's retry is
                    // routed afresh.
                    _ => return Err(IngestError::Internal(UNREACHABLE.into())),
                }
            }
            Err(CallError::Status(status, answer)) => return Err(refusal_from(status, &answer)),
            // The home region is not registered here: the S3 refusal, as
            // without the link.
            Err(CallError::UnknownRegion) => return Err(IngestError::WrongRegion(home)),
            Err(error) => {
                tracing::warn!(target_region = %home.region, ?error, "Forwarded write failed");
                return Err(IngestError::Internal(UNREACHABLE.into()));
            }
        }
    }
    Err(IngestError::Internal(UNREACHABLE.into()))
}

/// The current home from a home region's "wrong region" answer.
fn moved_home(answer: &Value, room: Option<uuid::Uuid>) -> Option<RemoteHome> {
    Some(RemoteHome {
        region: answer.get("region")?.as_str()?.to_string(),
        connect_url: answer
            .get("connect_url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        epoch: i32::try_from(answer.get("epoch")?.as_i64()?).ok()?,
        room,
    })
}

/// The home's accepted (or duplicate) answer as an ingest result.
fn result_from_answer(answer: &Value) -> Result<IngestResult, IngestError> {
    let event_id = answer.get("event_id").and_then(Value::as_str);
    let accepted = answer.get("accepted").and_then(Value::as_bool);
    let (Some(event_id), Some(accepted)) = (event_id, accepted) else {
        return Err(IngestError::Internal(UNREACHABLE.into()));
    };
    Ok(IngestResult {
        event_id: event_id.to_string(),
        accepted,
        message: answer
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        channel_sequence: answer.get("channel_sequence").and_then(Value::as_i64),
    })
}

/// The home's refusal as the same ingest error it raised there.
fn refusal_from(status: StatusCode, answer: &Value) -> IngestError {
    let message = answer
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match status {
        StatusCode::BAD_REQUEST if !message.is_empty() => IngestError::Rejected(message),
        StatusCode::CONFLICT if !message.is_empty() => IngestError::CanvasConflict(message),
        StatusCode::FORBIDDEN if !message.is_empty() => IngestError::AuthFailed(message),
        StatusCode::PAYLOAD_TOO_LARGE => {
            IngestError::Rejected("invalid: event too large to forward".into())
        }
        _ => IngestError::Internal(UNREACHABLE.into()),
    }
}

/// An error answer: `{"error": <kind>, "message": <text>}`.
fn answer_error(status: StatusCode, kind: &str, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": kind, "message": message })))
}

/// The "wrong region" answer with the room's current home.
fn answer_wrong_region(home: &RemoteHome) -> (StatusCode, Json<Value>) {
    (
        StatusCode::MISDIRECTED_REQUEST,
        Json(json!({
            "error": "wrong_region",
            "region": home.region,
            "connect_url": home.connect_url,
            "epoch": home.epoch,
        })),
    )
}

/// Home side: `POST /internal/region/events`. Admits the calling region's
/// attestation, checks the room's epoch, and ingests the event for the
/// attested principal.
pub async fn events(
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
            tracing::warn!(reason = refusal.label(), "Refused a forwarded write");
            return answer_error(StatusCode::UNAUTHORIZED, "refused", "region link refused");
        }
    };
    let Some(principal) = call.claims.principal.clone() else {
        return answer_error(StatusCode::UNAUTHORIZED, "refused", "region link refused");
    };
    let Ok(write) = serde_json::from_slice::<ForwardedWrite>(&body) else {
        return answer_error(
            StatusCode::BAD_REQUEST,
            "malformed",
            "invalid: malformed write",
        );
    };
    let Ok(pubkey) = nostr::PublicKey::from_hex(&principal.pubkey) else {
        return answer_error(
            StatusCode::BAD_REQUEST,
            "malformed",
            "invalid: malformed principal",
        );
    };

    // The entry routed by the room's home and epoch it had cached; a room
    // that moved since is answered with where it lives now.
    if let (Some(channel), Some(epoch)) = (call.claims.channel_id, call.claims.epoch) {
        match state.db.room_home(call.tenant.community(), channel).await {
            Ok(Some(home)) if home.region != state.config.region_id || home.epoch != epoch => {
                let connect_url = match state.db.region_link(&home.region).await {
                    Ok(Some((connect_url, _))) => connect_url,
                    _ => String::new(),
                };
                return answer_wrong_region(&RemoteHome {
                    region: home.region,
                    connect_url,
                    epoch: home.epoch,
                    room: Some(channel),
                });
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "Room lookup for a forwarded write failed");
                return answer_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again");
            }
        }
    }

    let auth = IngestAuth::Forwarded {
        pubkey,
        scopes: kuzz_auth::scope::parse_scopes(&principal.scopes),
        channel_ids: principal.channel_ids,
        entry_region: call.claims.entry_region.clone(),
        entry_auth: principal.auth,
    };
    metrics::counter!("kuzz_region_link_calls_accepted_total", "from" => call.claims.entry_region.clone()).increment(1);
    match crate::handlers::ingest::ingest_event_here(&state, &call.tenant, write.event, auth).await
    {
        Ok(result) => {
            let mut answer = json!({
                "event_id": result.event_id,
                "accepted": result.accepted,
                "message": result.message,
            });
            if let Some(sequence) = result.channel_sequence {
                answer["channel_sequence"] = sequence.into();
            }
            (StatusCode::OK, Json(answer))
        }
        Err(IngestError::Rejected(message)) => {
            answer_error(StatusCode::BAD_REQUEST, "rejected", &message)
        }
        Err(IngestError::CanvasConflict(message)) => {
            answer_error(StatusCode::CONFLICT, "canvas_conflict", &message)
        }
        Err(IngestError::AuthFailed(message)) => {
            answer_error(StatusCode::FORBIDDEN, "auth_failed", &message)
        }
        Err(IngestError::Internal(message)) => {
            tracing::warn!(%message, "Forwarded write failed at its home");
            answer_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
        }
        Err(IngestError::WrongRegion(home)) => answer_wrong_region(&home),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> nostr::Keys {
        nostr::Keys::generate()
    }

    #[test]
    fn dev_logins_and_forwarded_writes_are_not_forwarded() {
        let pubkey = keys().public_key();
        let dev = IngestAuth::Http {
            pubkey,
            scopes: vec![],
            auth_method: HttpAuthMethod::DevPubkey,
        };
        assert!(principal_for(&dev).is_none());
        let forwarded = IngestAuth::Forwarded {
            pubkey,
            scopes: vec![],
            channel_ids: None,
            entry_region: "jp".into(),
            entry_auth: EntryAuth::Nip42,
        };
        assert!(principal_for(&forwarded).is_none());
    }

    #[test]
    fn principals_carry_their_login_and_limits() {
        let pubkey = keys().public_key();
        let room = uuid::Uuid::new_v4();
        let ws = IngestAuth::Nip42 {
            pubkey,
            scopes: vec![kuzz_auth::Scope::MessagesWrite],
            channel_ids: Some(vec![room]),
            conn_id: uuid::Uuid::new_v4(),
        };
        let principal = principal_for(&ws).expect("forwardable");
        assert_eq!(principal.pubkey, pubkey.to_hex());
        assert_eq!(principal.auth, EntryAuth::Nip42);
        assert_eq!(principal.channel_ids, Some(vec![room]));
        assert_eq!(
            kuzz_auth::scope::parse_scopes(&principal.scopes),
            vec![kuzz_auth::Scope::MessagesWrite]
        );
        let token = IngestAuth::Token {
            pubkey,
            scopes: vec![],
            conn_id: None,
        };
        assert_eq!(
            principal_for(&token).expect("forwardable").auth,
            EntryAuth::Token
        );
    }

    #[test]
    fn home_answers_map_back_to_the_same_result() {
        let ok = result_from_answer(&json!({
            "event_id": "ab", "accepted": true, "message": "duplicate: already have this event",
            "channel_sequence": 7,
        }))
        .expect("accepted");
        assert_eq!(ok.channel_sequence, Some(7));
        assert!(ok.accepted);
        assert!(result_from_answer(&json!({"accepted": true})).is_err());

        let answer = json!({"error": "rejected", "message": "invalid: not a member"});
        assert!(matches!(
            refusal_from(StatusCode::BAD_REQUEST, &answer),
            IngestError::Rejected(m) if m == "invalid: not a member"
        ));
        assert!(matches!(
            refusal_from(StatusCode::FORBIDDEN, &answer),
            IngestError::AuthFailed(_)
        ));
        assert!(matches!(
            refusal_from(StatusCode::CONFLICT, &answer),
            IngestError::CanvasConflict(_)
        ));
        // A refusal of the link itself (401) or a server error is retryable.
        assert!(matches!(
            refusal_from(StatusCode::UNAUTHORIZED, &answer),
            IngestError::Internal(_)
        ));
        assert!(matches!(
            refusal_from(StatusCode::BAD_REQUEST, &json!({})),
            IngestError::Internal(_)
        ));
        // Too large for the link is final, not "try again".
        assert!(matches!(
            refusal_from(StatusCode::PAYLOAD_TOO_LARGE, &Value::Null),
            IngestError::Rejected(_)
        ));
    }

    #[test]
    fn a_moved_room_names_its_new_home() {
        let room = uuid::Uuid::new_v4();
        let (status, Json(answer)) = answer_wrong_region(&RemoteHome {
            region: "sg".into(),
            connect_url: "wss://sg.example".into(),
            epoch: 3,
            room: Some(room),
        });
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
        let next = moved_home(&answer, Some(room)).expect("home");
        assert_eq!(next.region, "sg");
        assert_eq!(next.epoch, 3);
        assert_eq!(next.room, Some(room));
        assert!(moved_home(&json!({"region": "sg"}), Some(room)).is_none());
    }
}
