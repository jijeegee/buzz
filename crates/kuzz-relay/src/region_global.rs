//! Global writes the relay makes for itself in a copy region (global regions
//! stage S5-3b, decisions E3 and E4).
//!
//! A region holding the global copy (S5-1) refuses local writes to global
//! tables. Two kinds of them are made by the relay itself while it handles a
//! room event or a sign-in, so they are not forwarded with a client's
//! request (S5-2, S5-3a):
//!
//! - **Relay-signed room-less events** (membership notices 44100/44101, the
//!   hidden-DM snapshot): [`store_relay_event`] sends the signed event to the
//!   primary (`POST /internal/region/global-event`). The primary checks it
//!   carries the relay's own signature, stores it with the same insert or
//!   replace rule and publishes it to its own subscribers. The entry region
//!   then sends it to its own subscribers at once; the copy brings the row.
//! - **Typed writes** ([`GlobalWrite`]): a user row, an agent's owner, a
//!   token profile. The entry checks its copy first and skips the call when
//!   the row is already there; otherwise one call to the primary
//!   (`POST /internal/region/global-write`). Typed operations, never SQL.
//!
//! In a region without the copy every helper writes locally, as before. If
//! the primary cannot be reached the write fails and its caller's retry
//! applies (the side-effect retry, the next AUTH).

use std::sync::Arc;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::Json;
use kuzz_core::event::StoredEvent;
use kuzz_core::principal::PrincipalId;
use kuzz_core::tenant::{CommunityId, TenantContext};
use kuzz_pubsub::EventTopic;
use nostr::Event;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::state::AppState;

/// Primary side: store a relay-signed room-less event.
pub const GLOBAL_EVENT_PATH: &str = "/internal/region/global-event";
/// Primary side: apply a typed global write.
pub const GLOBAL_WRITE_PATH: &str = "/internal/region/global-write";

/// How a relay-signed room-less event is stored, decided by its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RelayStore {
    /// A regular event: inserted once.
    Insert,
    /// A parameterized replaceable event with its own `d` tag.
    Replace(String),
}

/// The store rule for a relay-made room-less event; `None` for any other
/// kind, or a replaceable one without its `d` tag.
fn relay_store(event: &Event) -> Option<RelayStore> {
    match u32::from(event.kind.as_u16()) {
        kuzz_core::kind::KIND_MEMBER_ADDED_NOTIFICATION
        | kuzz_core::kind::KIND_MEMBER_REMOVED_NOTIFICATION => Some(RelayStore::Insert),
        kuzz_core::kind::KIND_DM_VISIBILITY => {
            kuzz_db::event::extract_d_tag(event).map(RelayStore::Replace)
        }
        _ => None,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RelayEventCall {
    event: Event,
}

/// A global write the primary applies for a copy region.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GlobalWrite {
    /// Make sure the user row exists (hex pubkey).
    EnsureUser {
        /// Hex pubkey.
        pubkey: String,
    },
    /// Record a verified NIP-OA agent→owner relationship (hex pubkeys).
    SetAgentOwner {
        /// The agent's hex pubkey.
        agent: String,
        /// The owner's hex pubkey.
        owner: String,
    },
    /// Publish a principal's token profile (kind 0) in the community.
    PublishProfile {
        /// The principal's hex pubkey.
        principal: String,
        /// Unconditionally (`true`) or only where the principal has a user
        /// row (`false`).
        participant: bool,
    },
    /// Turn off a push endpoint the push service reported gone, if the
    /// lease is still at that generation.
    DisablePushEndpoint {
        /// The lease author's hex pubkey.
        author: String,
        /// The app installation.
        installation_id: String,
        /// The lease generation the push service saw.
        generation: i64,
    },
}

/// What the primary answered for a [`GlobalWrite`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteAnswer {
    /// Something was written.
    pub created: bool,
    /// The write holds now (for [`GlobalWrite::SetAgentOwner`]: the agent
    /// is owned by that owner).
    pub holds: bool,
}

async fn call_primary(
    state: &AppState,
    community: CommunityId,
    path: &str,
    body: &Value,
) -> anyhow::Result<Value> {
    crate::region_link::call(
        state,
        &state.config.primary_region,
        community.as_uuid(),
        path,
        body,
        None,
        None,
    )
    .await
    .map_err(|error| anyhow::anyhow!("primary region call failed: {error:?}"))
}

/// Store a relay-signed room-less event: locally, or at the primary in a
/// copy region. Returns the stored event when it was new (the caller fans
/// it out to this region's subscribers) and `None` for a duplicate or a
/// superseded replacement.
pub async fn store_relay_event(
    tenant: &TenantContext,
    state: &AppState,
    event: &Event,
) -> anyhow::Result<Option<StoredEvent>> {
    if !state.config.global_copy {
        let store = relay_store(event)
            .ok_or_else(|| anyhow::anyhow!("not a relay-made room-less event"))?;
        let (stored, inserted) = store_here(state, tenant, event, &store).await?;
        return Ok(inserted.then_some(stored));
    }
    let body = serde_json::to_value(RelayEventCall {
        event: event.clone(),
    })?;
    let answer = call_primary(state, tenant.community(), GLOBAL_EVENT_PATH, &body).await?;
    let inserted = answer
        .get("inserted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(inserted.then(|| StoredEvent::new(event.clone(), None)))
}

async fn store_here(
    state: &AppState,
    tenant: &TenantContext,
    event: &Event,
    store: &RelayStore,
) -> anyhow::Result<(StoredEvent, bool)> {
    Ok(match store {
        RelayStore::Insert => {
            state
                .db
                .insert_event(tenant.community(), event, None)
                .await?
        }
        RelayStore::Replace(d_tag) => {
            state
                .db
                .replace_parameterized_event(tenant.community(), event, d_tag, None)
                .await?
        }
    })
}

/// Make sure `pubkey`'s user row exists. In a copy region an existing row
/// costs no call. Returns whether a row was created here (the primary counts
/// its own).
pub async fn ensure_user(
    tenant: &TenantContext,
    state: &AppState,
    pubkey: &[u8],
) -> anyhow::Result<bool> {
    if !state.config.global_copy {
        return Ok(state.db.ensure_user(tenant.community(), pubkey).await?);
    }
    if state
        .db
        .get_user(tenant.community(), pubkey)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    let write = GlobalWrite::EnsureUser {
        pubkey: hex::encode(pubkey),
    };
    write_at_primary(state, tenant.community(), &write).await?;
    Ok(false)
}

/// Apply `write` at the primary (copy regions only).
pub async fn write_at_primary(
    state: &AppState,
    community: CommunityId,
    write: &GlobalWrite,
) -> anyhow::Result<WriteAnswer> {
    let answer = call_primary(
        state,
        community,
        GLOBAL_WRITE_PATH,
        &serde_json::to_value(write)?,
    )
    .await?;
    Ok(serde_json::from_value(answer)?)
}

/// An error answer: `{"error": <kind>, "message": <text>}`.
fn answer_error(status: StatusCode, kind: &str, message: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": kind, "message": message })))
}

/// Admit a call: the link attestation, then that this is the primary
/// holding the global tables (a copy forwarding to a copy would loop).
async fn admit(
    state: &AppState,
    uri: &axum::http::Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<crate::region_link::VerifiedCall, (StatusCode, Json<Value>)> {
    let call = match crate::region_link::admit(state, &Method::POST, uri, headers, body).await {
        Ok(call) => call,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a global write");
            return Err(answer_error(
                StatusCode::UNAUTHORIZED,
                "refused",
                "region link refused",
            ));
        }
    };
    if state.config.global_copy || state.config.region_id != state.config.primary_region {
        return Err(answer_error(
            StatusCode::CONFLICT,
            "not_primary",
            "not the primary region",
        ));
    }
    Ok(call)
}

/// Primary side: `POST /internal/region/global-event`. Stores a relay-signed
/// room-less event and publishes it to this region's subscribers.
pub async fn global_event(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let call = match admit(&state, &uri, &headers, &body).await {
        Ok(call) => call,
        Err(answer) => return answer,
    };
    let Ok(request) = serde_json::from_slice::<RelayEventCall>(&body) else {
        return answer_error(StatusCode::BAD_REQUEST, "malformed", "malformed event");
    };
    let event = request.event;
    // Only the relay's own room-less events: one relay key serves every
    // region (S1), so its signature proves a region made it. The kind
    // decides how it is stored.
    let store = relay_store(&event);
    let Some(store) = store
        .filter(|_| event.pubkey == state.relay_keypair.public_key() && event.verify().is_ok())
    else {
        return answer_error(StatusCode::FORBIDDEN, "refused", "not a relay event");
    };
    let tenant = call.tenant;
    match store_here(&state, &tenant, &event, &store).await {
        Ok((stored, inserted)) => {
            if inserted {
                if let Err(error) = state
                    .pubsub
                    .publish_event(&tenant, EventTopic::Global, &stored.event)
                    .await
                {
                    tracing::warn!(%error, "Global event publish failed");
                }
            }
            (StatusCode::OK, Json(json!({ "inserted": inserted })))
        }
        Err(error) => {
            tracing::warn!(%error, "Storing a region's global event failed");
            answer_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
        }
    }
}

/// Primary side: `POST /internal/region/global-write`. Applies a typed
/// global write.
pub async fn global_write(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, Json<Value>) {
    let call = match admit(&state, &uri, &headers, &body).await {
        Ok(call) => call,
        Err(answer) => return answer,
    };
    let Ok(write) = serde_json::from_slice::<GlobalWrite>(&body) else {
        return answer_error(StatusCode::BAD_REQUEST, "malformed", "malformed write");
    };
    match apply(&state, &call.tenant, write).await {
        Ok(Some(answer)) => (StatusCode::OK, Json(json!(answer))),
        Ok(None) => answer_error(StatusCode::BAD_REQUEST, "malformed", "malformed write"),
        Err(error) => {
            tracing::warn!(%error, "A region's global write failed");
            answer_error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "try again")
        }
    }
}

/// Apply a typed write here. `Ok(None)`: malformed.
async fn apply(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    write: GlobalWrite,
) -> anyhow::Result<Option<WriteAnswer>> {
    let pubkey = |hex: &str| nostr::PublicKey::from_hex(hex).ok();
    Ok(Some(match write {
        GlobalWrite::EnsureUser { pubkey: user } => {
            let Some(user) = pubkey(&user) else {
                return Ok(None);
            };
            let created = state
                .db
                .ensure_user(tenant.community(), user.as_bytes())
                .await?;
            if created {
                metrics::counter!(
                    "kuzz_users_created_total",
                    "community" => tenant.host().to_owned()
                )
                .increment(1);
            }
            WriteAnswer {
                created,
                holds: true,
            }
        }
        GlobalWrite::SetAgentOwner { agent, owner } => {
            let (Some(agent), Some(owner)) = (pubkey(&agent), pubkey(&owner)) else {
                return Ok(None);
            };
            let owned_before = state
                .db
                .is_agent_owner(tenant.community(), agent.as_bytes(), owner.as_bytes())
                .await?;
            let holds =
                crate::api::relay_members::materialize_nip_oa_owner(state, tenant, &agent, &owner)
                    .await;
            WriteAnswer {
                created: holds && !owned_before,
                holds,
            }
        }
        GlobalWrite::PublishProfile {
            principal,
            participant,
        } => {
            let Ok(principal) = PrincipalId::from_hex(&principal) else {
                return Ok(None);
            };
            let scope = if participant {
                crate::identity::profile::Scope::Participant
            } else {
                crate::identity::profile::Scope::Member
            };
            let created = crate::identity::profile::publish_profile_in_community(
                state, tenant, &principal, scope,
            )
            .await?;
            WriteAnswer {
                created,
                holds: true,
            }
        }
        GlobalWrite::DisablePushEndpoint {
            author,
            installation_id,
            generation,
        } => {
            let Some(author) = pubkey(&author) else {
                return Ok(None);
            };
            let created = state
                .db
                .disable_push_endpoint(
                    tenant.community(),
                    author.as_bytes(),
                    &installation_id,
                    generation,
                )
                .await?;
            WriteAnswer {
                created,
                holds: true,
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_travel_as_tagged_operations() {
        let write = GlobalWrite::SetAgentOwner {
            agent: "aa".into(),
            owner: "bb".into(),
        };
        let value = serde_json::to_value(&write).unwrap();
        assert_eq!(
            value,
            json!({"op": "set_agent_owner", "agent": "aa", "owner": "bb"})
        );
        assert_eq!(serde_json::from_value::<GlobalWrite>(value).unwrap(), write);
    }

    #[test]
    fn a_relay_event_is_stored_by_its_kind() {
        let keys = nostr::Keys::generate();
        let notice = nostr::EventBuilder::new(nostr::Kind::Custom(44100), "")
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(relay_store(&notice), Some(RelayStore::Insert));
        let snapshot = nostr::EventBuilder::new(nostr::Kind::Custom(30622), "")
            .tags([nostr::Tag::parse(["d", "viewer"]).unwrap()])
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(
            relay_store(&snapshot),
            Some(RelayStore::Replace("viewer".into()))
        );
        let no_d = nostr::EventBuilder::new(nostr::Kind::Custom(30622), "")
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(relay_store(&no_d), None);
        let profile = nostr::EventBuilder::new(nostr::Kind::Metadata, "{}")
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(relay_store(&profile), None);
    }
}
