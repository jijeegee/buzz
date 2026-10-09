//! Observer telemetry policy for executors.
//!
//! `buzz-acp` reads the limits that apply to its agent's telemetry here so it
//! can pace and summarise frames before the relay ever has to reject them.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use buzz_auth::NipFiMode;
use nostr::PublicKey;

use crate::{
    api::{api_error, bridge, internal_error},
    nip_fi_http::admit_nip_fi_http_on_state,
    observer_quota::ObserverPolicy,
    state::AppState,
};

/// Route path of [`policy`].
pub const POLICY_PATH: &str = "/api/observer/policy";

/// `GET /api/observer/policy` — the caller's observer tier and its executor
/// and server limits. Agents get their owner's tier; anyone else their own.
pub async fn policy(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    match policy_inner(&state, &headers).await {
        Ok(policy) => Json(policy).into_response(),
        Err(response) => response,
    }
}

async fn policy_inner(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<ObserverPolicy, Response> {
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let tenant = crate::tenant::bind_community(&state.db, raw_host)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::NOT_FOUND,
                "relay: no community is configured for this host",
            )
            .into_response()
        })?;

    let url = bridge::nip98_expected_url(&state.config.relay_url, &tenant, POLICY_PATH);
    // In NIP-FI enforce/deny-protected mode a real NIP-98 event is mandatory.
    // [NIP-FI.md:594-607, FI-TRACE-HTTP-INGRESS]
    let nip_fi_active = !matches!(state.config.nip_fi.mode, NipFiMode::Off);
    let require_auth = state.config.require_auth_token || nip_fi_active;
    let admission = admit_nip_fi_http_on_state(
        state,
        headers,
        bridge::make_nip98_closure_for_admission(
            headers.clone(),
            "GET",
            url,
            None,
            require_auth,
            false,
        ),
    )?;
    let pubkey = *admission.proven_pubkey();
    let (event_id_bytes, signed_created_at) = admission.into_extra();

    bridge::enforce_http_admission(state, &tenant, &pubkey)
        .await
        .map_err(|e| e.into_response())?;
    bridge::check_nip98_replay(state, &tenant, event_id_bytes)
        .await
        .map_err(|e| e.into_response())?;
    super::relay_members::enforce_relay_membership(
        state,
        tenant.community(),
        &pubkey.to_bytes(),
        super::relay_members::extract_auth_tag_header(headers),
        signed_created_at,
    )
    .await
    .map_err(|e| e.into_response())?;

    caller_policy(state, tenant.community(), &pubkey)
        .await
        .map_err(|error| {
            internal_error(&format!("observer policy lookup: {error}")).into_response()
        })
}

/// The policy that applies to `caller`'s telemetry: its owner's tier when it
/// is an agent, its own tier otherwise.
pub(crate) async fn caller_policy(
    state: &AppState,
    community: buzz_core::CommunityId,
    caller: &PublicKey,
) -> Result<ObserverPolicy, buzz_db::DbError> {
    let owner = crate::observer_quota::agent_owner(state, community, caller)
        .await?
        .unwrap_or(*caller);
    let tier = crate::observer_quota::resolve_tier(state, community, &owner).await?;
    Ok(state.config.observer_quota.policy(tier))
}
