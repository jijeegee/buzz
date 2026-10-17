//! Friends and silent blocks: the requester's own block list, exact person
//! lookup by username or email, and name search among people who share a
//! channel with the requester.
//!
//! Every endpoint acts as the signed-in person (user session, token or key
//! backup) inside the community bound from the request host, and identifies
//! people by their 64-hex messaging pubkey. Email addresses are matched by
//! lookup but never returned, and nothing here reveals who blocked whom to
//! anyone but the blocker.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use kuzz_core::TenantContext;
use kuzz_db::blocks::BlockOutcome;
use kuzz_db::people::{PersonLookup, PersonRecord, MAX_SHARED_PEOPLE};
use serde::Deserialize;
use serde_json::json;

use super::{
    auth_error, authenticate_user_session, bad_request, forbidden, internal, not_found, rate_limit,
    unavailable,
};
use crate::state::AppState;

/// Exact lookups one person may make per minute.
const LOOKUP_PER_MIN: u64 = 20;
/// Shared-channel searches one person may make per minute.
const SHARED_PER_MIN: u64 = 60;
/// Default page size for shared-channel search.
const DEFAULT_SHARED_LIMIT: i64 = 20;
/// Longest accepted lookup or search text.
const MAX_QUERY_LEN: usize = 320;

/// The signed-in person, their community and their messaging pubkey.
struct Requester {
    principal: kuzz_core::principal::PrincipalId,
    tenant: TenantContext,
    pubkey: Vec<u8>,
}

async fn requester(state: &AppState, headers: &HeaderMap) -> Result<Requester, Response> {
    let binding = authenticate_user_session(state, headers).await?;
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let tenant = match crate::tenant::bind_community(&state.db, raw_host).await {
        Ok(tenant) => tenant,
        Err(crate::tenant::BindError::UnmappedHost) => {
            return Err(not_found("no community is served at this host"))
        }
        Err(crate::tenant::BindError::Lookup(_)) => return Err(unavailable()),
    };
    let pubkey = match state
        .db
        .principal_messaging_pubkey(&binding.principal)
        .await
    {
        Ok(Some(pubkey)) => pubkey,
        Ok(None) => return Err(forbidden("this account has no messaging key yet")),
        Err(_) => return Err(unavailable()),
    };
    Ok(Requester {
        principal: binding.principal,
        tenant,
        pubkey,
    })
}

/// A 64-hex pubkey path segment (case-insensitive) as 32 bytes.
fn parse_pubkey(raw: &str) -> Result<Vec<u8>, Response> {
    if raw.len() != 64 {
        return Err(bad_request("pubkey must be 64 hex characters"));
    }
    hex::decode(raw).map_err(|_| bad_request("pubkey must be 64 hex characters"))
}

fn person_json(person: &PersonRecord) -> serde_json::Value {
    json!({
        "pubkey": hex::encode(&person.pubkey),
        "display_name": person.display_name,
        "avatar_url": person.avatar_url,
        "username": person.username,
    })
}

/// `GET /auth/blocks` — the requester's own block list, newest first.
pub(super) async fn list_blocks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let me = match requester(&state, &headers).await {
        Ok(me) => me,
        Err(response) => return response,
    };
    match state
        .db
        .list_blocks(me.tenant.community(), &me.pubkey)
        .await
    {
        Ok(blocks) => {
            let blocked: Vec<_> = blocks
                .iter()
                .map(|block| {
                    json!({
                        "pubkey": hex::encode(&block.blocked),
                        "created_at": block.created_at.to_rfc3339_opts(
                            chrono::SecondsFormat::Secs,
                            true,
                        ),
                    })
                })
                .collect();
            axum::Json(json!({ "blocked": blocked })).into_response()
        }
        Err(error) => internal("list_blocks", &error),
    }
}

/// `PUT /auth/blocks/{pubkey}` — block (idempotent). Self → 400.
pub(super) async fn put_block(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let me = match requester(&state, &headers).await {
        Ok(me) => me,
        Err(response) => return response,
    };
    let target = match parse_pubkey(&raw) {
        Ok(target) => target,
        Err(response) => return response,
    };
    if target == me.pubkey {
        return bad_request("you cannot block yourself");
    }
    match state
        .db
        .block_user(me.tenant.community(), &me.pubkey, &target)
        .await
    {
        Ok(BlockOutcome::Added | BlockOutcome::AlreadyBlocked) => {
            state.invalidate_block(&me.tenant, &me.pubkey, &target);
            tracing::info!(principal = %me.principal, "auth.block");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(BlockOutcome::LimitReached) => auth_error(
            StatusCode::CONFLICT,
            "block_limit",
            &format!(
                "you can block at most {} people",
                kuzz_db::blocks::MAX_BLOCKS_PER_USER
            ),
        ),
        Err(error) => internal("block_user", &error),
    }
}

/// `DELETE /auth/blocks/{pubkey}` — unblock (idempotent).
pub(super) async fn delete_block(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let me = match requester(&state, &headers).await {
        Ok(me) => me,
        Err(response) => return response,
    };
    let target = match parse_pubkey(&raw) {
        Ok(target) => target,
        Err(response) => return response,
    };
    match state
        .db
        .unblock_user(me.tenant.community(), &me.pubkey, &target)
        .await
    {
        Ok(_) => {
            state.invalidate_block(&me.tenant, &me.pubkey, &target);
            tracing::info!(principal = %me.principal, "auth.unblock");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => internal("unblock_user", &error),
    }
}

/// `?q=` (and `?limit=` for shared search).
#[derive(Debug, Deserialize)]
pub(super) struct PeopleQuery {
    q: Option<String>,
    limit: Option<String>,
}

fn query_text(params: &PeopleQuery) -> Result<&str, Response> {
    let q = params.q.as_deref().unwrap_or("").trim();
    if q.is_empty() {
        return Err(bad_request("q is required"));
    }
    if q.len() > MAX_QUERY_LEN {
        return Err(bad_request("q is too long"));
    }
    Ok(q)
}

/// The lookup a trimmed `q` asks for: an email when it contains `@`
/// anywhere but the start, otherwise a username without its leading `@`.
fn lookup_for(q: &str) -> Option<PersonLookup<'_>> {
    if q.trim_start_matches('@').contains('@') {
        return Some(PersonLookup::Email(q));
    }
    let name = q.trim_start_matches('@');
    (!name.is_empty()).then_some(PersonLookup::Username(name))
}

/// `GET /auth/people/lookup?q=` — one discoverable person by exact username
/// or email. 404 for no match, discovery off, or a bot.
pub(super) async fn lookup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<PeopleQuery>,
) -> Response {
    let me = match requester(&state, &headers).await {
        Ok(me) => me,
        Err(response) => return response,
    };
    if let Err(response) = rate_limit(
        &state,
        &format!("auth:people:lookup:{}", me.principal),
        60,
        LOOKUP_PER_MIN,
    )
    .await
    {
        return response;
    }
    let q = match query_text(&params) {
        Ok(q) => q,
        Err(response) => return response,
    };
    let Some(lookup) = lookup_for(q) else {
        return not_found("no one found");
    };
    match state.db.lookup_person(me.tenant.community(), lookup).await {
        Ok(Some(person)) => axum::Json(person_json(&person)).into_response(),
        Ok(None) => not_found("no one found"),
        Err(error) => internal("lookup_person", &error),
    }
}

/// `GET /auth/people/shared?q=<prefix>&limit=` — people in the requester's
/// channels whose display name or username starts with `q`.
pub(super) async fn shared(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<PeopleQuery>,
) -> Response {
    let me = match requester(&state, &headers).await {
        Ok(me) => me,
        Err(response) => return response,
    };
    if let Err(response) = rate_limit(
        &state,
        &format!("auth:people:shared:{}", me.principal),
        60,
        SHARED_PER_MIN,
    )
    .await
    {
        return response;
    }
    let q = match query_text(&params) {
        Ok(q) => q,
        Err(response) => return response,
    };
    let limit = match params.limit.as_deref() {
        None => DEFAULT_SHARED_LIMIT,
        Some(raw) => match raw.parse::<i64>() {
            Ok(limit) if limit >= 1 => limit.min(MAX_SHARED_PEOPLE),
            _ => return bad_request("limit must be a positive number"),
        },
    };
    match state
        .db
        .shared_channel_people(me.tenant.community(), &me.pubkey, q, limit)
        .await
    {
        Ok(people) => {
            let people: Vec<_> = people.iter().map(person_json).collect();
            axum::Json(json!({ "people": people })).into_response()
        }
        Err(error) => internal("shared_channel_people", &error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_text_picks_email_or_username() {
        assert_eq!(
            lookup_for("ada@example.com"),
            Some(PersonLookup::Email("ada@example.com"))
        );
        assert_eq!(lookup_for("@ada"), Some(PersonLookup::Username("ada")));
        assert_eq!(lookup_for("ada"), Some(PersonLookup::Username("ada")));
        assert_eq!(lookup_for("@"), None);
    }

    #[test]
    fn pubkey_paths_need_64_hex() {
        assert!(parse_pubkey(&"ab".repeat(32)).is_ok());
        assert!(parse_pubkey(&"AB".repeat(32)).is_ok());
        assert!(parse_pubkey("abc").is_err());
        assert!(parse_pubkey(&"zz".repeat(32)).is_err());
    }
}
