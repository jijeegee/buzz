//! `/auth/operators` — deployment staff roster over the **existing**
//! `relay_operators` store (plan B5): writes go through the audited
//! `upsert`/`remove` paths; the caller must resolve to an effective Operator
//! with the admin API's precedence (config, owner fallback, DB roster).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use buzz_auth::TokenBinding;
use buzz_core::principal::{AccessTokenKind, PrincipalId};
use serde_json::json;

use super::{
    auth_error, authenticate, bad_request, forbidden, internal, json_object, not_found, unavailable,
};
use crate::api::admin::is_effective_operator;
use crate::state::AppState;

async fn require_operator(state: &AppState, headers: &HeaderMap) -> Result<TokenBinding, Response> {
    let binding = authenticate(state, headers).await?;
    if binding.kind != AccessTokenKind::User {
        return Err(forbidden("operator access requires a user session"));
    }
    match is_effective_operator(state, *binding.principal.as_bytes()).await {
        Ok(true) => Ok(binding),
        Ok(false) => Err(forbidden("operator role required")),
        Err(()) => Err(unavailable()),
    }
}

fn parse_target(raw: &str) -> Result<PrincipalId, Response> {
    PrincipalId::from_hex(raw).map_err(|_| not_found("principal not found"))
}

/// `GET /auth/operators`.
pub(super) async fn list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_operator(&state, &headers).await {
        return response;
    }
    match state.db.list_relay_operators().await {
        Ok(rows) => axum::Json(
            rows.into_iter()
                .map(|row| {
                    json!({
                        "principal_id": hex::encode(&row.pubkey),
                        "role": row.role,
                        "added_by": hex::encode(&row.added_by),
                        "created_at": row.created_at,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => internal("list_relay_operators", &error),
    }
}

/// `PUT /auth/operators/{principal}` `{role: "operator" | "moderator"}`.
pub(super) async fn grant(
    State(state): State<Arc<AppState>>,
    Path(target): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let caller = match require_operator(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let target = match parse_target(&target) {
        Ok(target) => target,
        Err(response) => return response,
    };
    let body = match json_object(&body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(role) = body
        .get("role")
        .and_then(|v| v.as_str())
        .filter(|role| matches!(*role, "operator" | "moderator"))
    else {
        return bad_request("role must be \"operator\" or \"moderator\"");
    };
    match state.db.get_principal(&target).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("principal not found"),
        Err(error) => return internal("get_principal", &error),
    }
    match state
        .db
        .upsert_identity_operator(target.as_bytes(), role, caller.principal.as_bytes())
        .await
    {
        Ok(()) => {
            tracing::info!(target = %target, role, "auth.operator_granted");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(buzz_db::DbError::LastOperator) => auth_error(
            StatusCode::CONFLICT,
            "last_operator",
            "operation would remove the last relay operator",
        ),
        Err(buzz_db::DbError::AccessDenied(message)) => bad_request(&message),
        Err(error) => internal("upsert_identity_operator", &error),
    }
}

/// `DELETE /auth/operators/{principal}`.
pub(super) async fn revoke(
    State(state): State<Arc<AppState>>,
    Path(target): Path<String>,
    headers: HeaderMap,
) -> Response {
    let caller = match require_operator(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let target = match parse_target(&target) {
        Ok(target) => target,
        Err(response) => return response,
    };
    match state
        .db
        .remove_identity_operator(target.as_bytes(), caller.principal.as_bytes())
        .await
    {
        Ok(true) => {
            tracing::info!(target = %target, "auth.operator_revoked");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => not_found("not an operator or moderator"),
        Err(buzz_db::DbError::LastOperator) => auth_error(
            StatusCode::CONFLICT,
            "last_operator",
            "operation would remove the last relay operator",
        ),
        Err(error) => internal("remove_relay_operator", &error),
    }
}
