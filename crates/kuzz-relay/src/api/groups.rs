//! Groups HTTP API: public-group search and addresses, invite links, the
//! "who can add me" setting and invitations.
//!
//! A group is a `stream` or `forum` channel. Every route acts as the caller
//! inside the community bound from the request host, authenticated like the
//! HTTP bridge: a centralized-identity `Bearer` token, or NIP-98 (with NIP-FI
//! admission and replay protection), then the relay-membership and
//! community-ban gate. Responses are JSON with camelCase fields; errors are
//! `{"error": "<text>"}`.
//!
//! Every join (invite link, accepted invitation) goes through
//! [`crate::handlers::side_effects::add_channel_member`], so the system
//! message, kind:39002 member list and member-added notification are the same
//! as for a kind:9000 add.

// Handlers return axum `Response` errors directly (axum's design; see
// nip_fi_http.rs).
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use axum::{
    extract::{Path, RawQuery, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use base64::Engine as _;
use kuzz_auth::NipFiMode;
use kuzz_core::TenantContext;
use kuzz_db::channel::MemberRole;
use kuzz_db::groups::{GroupInviteLink, GroupSummary, SetHandleOutcome, MAX_GROUP_SEARCH};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{api_error, bridge, internal_error, parse_query_or_400};
use crate::handlers::side_effects::add_channel_member;
use crate::nip_fi_http::admit_nip_fi_http_on_state;
use crate::state::AppState;

/// Searches one person may run per minute.
const SEARCH_PER_MIN: u64 = 60;
/// Invite-link previews one person may open per minute.
const LINK_PREVIEW_PER_MIN: u64 = 30;
/// Invite-link joins one person may attempt per minute (as invite claims).
const LINK_JOIN_PER_MIN: u64 = 10;
/// Shortest and longest search text, in characters.
const SEARCH_MIN_CHARS: usize = 2;
const SEARCH_MAX_CHARS: usize = 64;
/// Random bytes in an invite code (24 URL-safe characters).
const INVITE_CODE_BYTES: usize = 18;
/// Addresses nobody may take.
const RESERVED_HANDLES: &[&str] = &[
    "admin", "kuzz", "support", "help", "settings", "api", "g", "invite",
];

type HandlerResult = Result<Response, Response>;

fn error(status: StatusCode, message: &str) -> Response {
    api_error(status, message).into_response()
}

fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "group not found")
}

fn internal(context: &str, e: &dyn std::fmt::Display) -> Response {
    internal_error(&format!("{context}: {e}")).into_response()
}

/// The authenticated caller and their community.
struct Caller {
    tenant: TenantContext,
    pubkey: Vec<u8>,
}

/// Bind the tenant, authenticate (Bearer or NIP-98), and apply the
/// relay-membership/ban gate and the per-person API quota.
async fn authenticate(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    method: &'static str,
    path: &str,
    raw_query: Option<&str>,
    body: Option<&[u8]>,
) -> Result<Caller, Response> {
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let tenant = crate::tenant::bind_community(&state.db, raw_host)
        .await
        .map_err(|_| {
            error(
                StatusCode::NOT_FOUND,
                "relay: no community is configured for this host",
            )
        })?;

    let (pubkey, auth_tag, signed_created_at) =
        match bridge::verify_bridge_bearer(state, headers, &tenant).await? {
            Some(pubkey) => (pubkey, None, None),
            None => {
                let path_with_query = match raw_query {
                    Some(q) if !q.is_empty() => format!("{path}?{q}"),
                    _ => path.to_owned(),
                };
                let url =
                    bridge::nip98_expected_url(&state.config.relay_url, &tenant, &path_with_query);
                let nip_fi_active = !matches!(state.config.nip_fi.mode, NipFiMode::Off);
                let nip_fi_enforce = matches!(state.config.nip_fi.mode, NipFiMode::Enforce);
                let admission = admit_nip_fi_http_on_state(
                    state,
                    headers,
                    bridge::make_nip98_closure_for_admission(
                        headers.clone(),
                        method,
                        url,
                        body.map(<[u8]>::to_vec),
                        state.config.require_auth_token || nip_fi_active,
                        nip_fi_enforce && body.is_some(),
                    ),
                )?;
                let pubkey = *admission.proven_pubkey();
                let (event_id_bytes, signed_created_at) = admission.into_extra();
                bridge::check_nip98_replay(state, &tenant, event_id_bytes)
                    .await
                    .map_err(IntoResponse::into_response)?;
                (
                    pubkey,
                    super::relay_members::extract_auth_tag_header(headers),
                    signed_created_at,
                )
            }
        };

    super::relay_members::enforce_relay_membership(
        state,
        tenant.community(),
        pubkey.as_bytes(),
        auth_tag,
        signed_created_at,
    )
    .await
    .map_err(IntoResponse::into_response)?;
    bridge::enforce_http_admission(state, &tenant, &pubkey)
        .await
        .map_err(IntoResponse::into_response)?;

    Ok(Caller {
        tenant,
        pubkey: pubkey.to_bytes().to_vec(),
    })
}

/// Shared Redis fixed-window limit per person; fails closed.
async fn rate_limit(state: &AppState, caller: &Caller, scope: &str, limit: u64) -> HandlerResult {
    let key = format!(
        "groups:{scope}:{}:{}",
        caller.tenant.community(),
        hex::encode(&caller.pubkey)
    );
    match state
        .admission_rate_limiter
        .check_key(&key, 60, limit)
        .await
    {
        Ok(result) if result.allowed => Ok(().into_response()),
        Ok(result) => Err(error(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("rate limited; retry in {}s", result.reset_in_secs),
        )),
        Err(e) => {
            tracing::warn!(error = %e, "groups rate limiter unavailable; denying");
            Err(error(
                StatusCode::SERVICE_UNAVAILABLE,
                "rate limiter unavailable",
            ))
        }
    }
}

/// Parse a JSON object body.
fn json_body(body: &[u8]) -> Result<serde_json::Map<String, Value>, Response> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(error(StatusCode::BAD_REQUEST, "body must be a JSON object")),
    }
}

/// `{ id, name, description, handle, picture, channelType, memberCount,
/// isMember }`. `picture` is the group photo URL or null.
fn group_json(group: &GroupSummary) -> Value {
    json!({
        "id": group.id.to_string(),
        "name": group.name,
        "description": group.description,
        "handle": group.handle,
        "picture": group.picture,
        "channelType": group.channel_type,
        "memberCount": group.member_count,
        "isMember": group.is_member,
    })
}

/// [`group_json`] plus `visibility`.
fn group_with_visibility_json(group: &GroupSummary) -> Value {
    let mut value = group_json(group);
    value["visibility"] = json!(group.visibility);
    value
}

fn is_group_type(group: &GroupSummary) -> bool {
    matches!(group.channel_type.as_str(), "stream" | "forum")
}

fn parse_group_id(raw: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw).map_err(|_| not_found())
}

/// Normalize and validate a public address: trimmed, leading `@` dropped,
/// lowercase, `a-z0-9_`, 3..=32 characters, not reserved.
fn normalize_handle(raw: &str) -> Result<String, &'static str> {
    let handle = raw.trim().trim_start_matches('@').to_ascii_lowercase();
    let valid_chars = handle
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !(3..=32).contains(&handle.len()) || !valid_chars {
        return Err("address must be 3-32 characters: a-z, 0-9 and _");
    }
    if RESERVED_HANDLES.contains(&handle.as_str()) {
        return Err("this address is reserved");
    }
    Ok(handle)
}

/// Whether an invite code is shaped like one we mint (URL-safe, 22..=64).
fn plausible_code(code: &str) -> bool {
    (22..=64).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn new_invite_code() -> String {
    let bytes: [u8; INVITE_CODE_BYTES] = rand::random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `https://<community host>/g/<code>` (`http` for a `ws://` relay), the
/// same host and scheme rule as relay invite links.
fn invite_url(state: &AppState, tenant: &TenantContext, code: &str) -> String {
    let scheme = if state.config.relay_url.trim_start().starts_with("wss://") {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{}/g/{code}", tenant.host())
}

fn invite_link_json(state: &AppState, tenant: &TenantContext, link: &GroupInviteLink) -> Value {
    json!({ "code": link.code, "url": invite_url(state, tenant, &link.code) })
}

async fn role_of(
    state: &AppState,
    caller: &Caller,
    channel_id: Uuid,
    pubkey: &[u8],
) -> Result<Option<MemberRole>, Response> {
    state
        .db
        .get_member_role(caller.tenant.community(), channel_id, pubkey)
        .await
        .map(|role| role.and_then(|r| r.parse().ok()))
        .map_err(|e| internal("group member role", &e))
}

async fn summary(
    state: &AppState,
    caller: &Caller,
    channel_id: Uuid,
) -> Result<Option<GroupSummary>, Response> {
    state
        .db
        .group_summary(caller.tenant.community(), &caller.pubkey, channel_id)
        .await
        .map_err(|e| internal("group summary", &e))
}

/// A live (not archived) group the caller may manage: owner/admin only.
async fn managed_group(
    state: &AppState,
    caller: &Caller,
    raw_id: &str,
) -> Result<GroupSummary, Response> {
    let id = parse_group_id(raw_id)?;
    let group = summary(state, caller, id)
        .await?
        .filter(is_group_type)
        .ok_or_else(not_found)?;
    if !group.is_member && group.visibility != "open" {
        return Err(not_found());
    }
    if group.archived_at.is_some() {
        return Err(error(StatusCode::CONFLICT, "group is archived"));
    }
    let role = role_of(state, caller, id, &caller.pubkey).await?;
    if !role.is_some_and(|r| r.is_elevated()) {
        return Err(error(
            StatusCode::FORBIDDEN,
            "only group owners and admins can do this",
        ));
    }
    Ok(group)
}

/// Whether `pubkey` is an agent (owned agent or bot principal).
async fn is_agent(state: &AppState, caller: &Caller) -> Result<bool, Response> {
    let owned = state
        .db
        .get_agent_channel_policy(caller.tenant.community(), &caller.pubkey)
        .await
        .map_err(|e| internal("agent policy", &e))?
        .is_some_and(|(_, owner)| owner.is_some());
    if owned {
        return Ok(true);
    }
    state
        .db
        .is_bot_principal(&caller.pubkey)
        .await
        .map_err(|e| internal("bot principal", &e))
}

/// A link still admits people while its creator is an active member allowed
/// to share links under the group's current invite policy.
async fn link_is_usable(
    state: &AppState,
    caller: &Caller,
    group: &GroupSummary,
    link: &GroupInviteLink,
) -> Result<bool, Response> {
    let creator = role_of(state, caller, group.id, &link.created_by).await?;
    Ok(match creator {
        None => false,
        Some(role) => group.invite_policy != "admins" || role.is_elevated(),
    })
}

/// The live group an invite code opens, or 404.
async fn group_for_code(
    state: &AppState,
    caller: &Caller,
    code: &str,
) -> Result<(GroupSummary, GroupInviteLink), Response> {
    if !plausible_code(code) {
        return Err(not_found());
    }
    let link = state
        .db
        .group_invite_link_by_code(caller.tenant.community(), code)
        .await
        .map_err(|e| internal("invite link", &e))?
        .ok_or_else(not_found)?;
    let group = summary(state, caller, link.channel_id)
        .await?
        .filter(|g| is_group_type(g) && g.archived_at.is_none())
        .ok_or_else(not_found)?;
    if !link_is_usable(state, caller, &group, &link).await? {
        return Err(not_found());
    }
    Ok((group, link))
}

/// Map a failed add to a response.
fn add_failure(e: anyhow::Error) -> Response {
    match e.downcast_ref::<kuzz_db::DbError>() {
        Some(kuzz_db::DbError::AccessDenied(message)) => error(StatusCode::FORBIDDEN, message),
        Some(kuzz_db::DbError::ChannelNotFound(_)) => not_found(),
        _ => internal("group join", &e),
    }
}

// ── S2: search and public addresses ─────────────────────────────────────────

/// Query of [`search`].
#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    limit: Option<String>,
}

/// `GET /api/groups/search?q=<text>&limit=<1..20>` →
/// `{ "groups": [group…] }`: live open groups whose name, description or
/// address contains `q`.
pub async fn search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "GET",
        "/api/groups/search",
        raw_query.as_deref(),
        None,
    )
    .await?;
    rate_limit(&state, &caller, "search", SEARCH_PER_MIN).await?;
    let query: SearchQuery =
        parse_query_or_400(raw_query.as_deref()).map_err(IntoResponse::into_response)?;
    let q = query.q.as_deref().unwrap_or("").trim();
    if !(SEARCH_MIN_CHARS..=SEARCH_MAX_CHARS).contains(&q.chars().count()) {
        return Err(error(StatusCode::BAD_REQUEST, "q must be 2-64 characters"));
    }
    let limit = match query.limit.as_deref() {
        None | Some("") => MAX_GROUP_SEARCH,
        Some(raw) => match raw.parse::<i64>() {
            Ok(n) if n >= 1 => n.min(MAX_GROUP_SEARCH),
            _ => {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "limit must be a positive integer",
                ))
            }
        },
    };
    let groups = state
        .db
        .search_open_groups(caller.tenant.community(), &caller.pubkey, q, limit)
        .await
        .map_err(|e| super::db_read_error("group search", &e).into_response())?;
    Ok(
        Json(json!({ "groups": groups.iter().map(group_json).collect::<Vec<_>>() }))
            .into_response(),
    )
}

/// `GET /api/groups/by-handle/{handle}` → the open group with this address.
pub async fn by_handle(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "GET",
        &format!("/api/groups/by-handle/{handle}"),
        None,
        None,
    )
    .await?;
    let Ok(handle) = normalize_handle(&handle) else {
        return Err(not_found());
    };
    let group = state
        .db
        .open_group_by_handle(caller.tenant.community(), &caller.pubkey, &handle)
        .await
        .map_err(|e| internal("group by handle", &e))?
        .ok_or_else(not_found)?;
    Ok(Json(group_json(&group)).into_response())
}

/// `GET /api/groups/{id}` → the group with `visibility` and `invitePolicy`,
/// for members of any group and anyone for an open group.
pub async fn get_group(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "GET",
        &format!("/api/groups/{id}"),
        None,
        None,
    )
    .await?;
    let id = parse_group_id(&id)?;
    let group = summary(&state, &caller, id)
        .await?
        .filter(|g| is_group_type(g) && (g.is_member || g.visibility == "open"))
        .ok_or_else(not_found)?;
    let mut value = group_with_visibility_json(&group);
    value["invitePolicy"] = json!(group.invite_policy);
    Ok(Json(value).into_response())
}

/// `PUT /api/groups/{id}/handle` `{ "handle": "<address>" | null }` → the
/// group. Owner/admin of an open group; 409 when taken.
pub async fn put_handle(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "PUT",
        &format!("/api/groups/{id}/handle"),
        None,
        Some(&body),
    )
    .await?;
    let group = managed_group(&state, &caller, &id).await?;
    let body = json_body(&body)?;
    let handle = match body.get("handle") {
        Some(Value::Null) => None,
        Some(Value::String(raw)) => {
            Some(normalize_handle(raw).map_err(|m| error(StatusCode::BAD_REQUEST, m))?)
        }
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "handle must be a string or null",
            ))
        }
    };
    if handle.is_some() && group.visibility != "open" {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "only public groups can have an address",
        ));
    }
    match state
        .db
        .set_group_handle(caller.tenant.community(), group.id, handle.as_deref())
        .await
        .map_err(|e| internal("set group handle", &e))?
    {
        SetHandleOutcome::Set => {}
        SetHandleOutcome::Taken => {
            return Err(error(StatusCode::CONFLICT, "this address is taken"))
        }
        SetHandleOutcome::NotFound => return Err(not_found()),
    }
    let group = summary(&state, &caller, group.id)
        .await?
        .ok_or_else(not_found)?;
    Ok(Json(group_json(&group)).into_response())
}

// ── S3: invite links ─────────────────────────────────────────────────────────

/// `GET /api/groups/{id}/invite-link` → `{ code, url }`, created on first
/// call. Active members; owners/admins only when the invite policy is
/// `admins`.
pub async fn get_invite_link(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "GET",
        &format!("/api/groups/{id}/invite-link"),
        None,
        None,
    )
    .await?;
    let id = parse_group_id(&id)?;
    let group = summary(&state, &caller, id)
        .await?
        .filter(is_group_type)
        .ok_or_else(not_found)?;
    let role = role_of(&state, &caller, id, &caller.pubkey).await?;
    let Some(role) = role else {
        return Err(if group.visibility == "open" {
            error(
                StatusCode::FORBIDDEN,
                "only members can share the invite link",
            )
        } else {
            not_found()
        });
    };
    if group.invite_policy == "admins" && !role.is_elevated() {
        return Err(error(
            StatusCode::FORBIDDEN,
            "only group owners and admins can share the invite link",
        ));
    }
    if group.archived_at.is_some() {
        return Err(error(StatusCode::CONFLICT, "group is archived"));
    }
    let community = caller.tenant.community();
    let active = state
        .db
        .active_group_invite_link(community, id)
        .await
        .map_err(|e| internal("invite link", &e))?;
    if let Some(link) = &active {
        if link_is_usable(&state, &caller, &group, link).await? {
            return Ok(Json(invite_link_json(&state, &caller.tenant, link)).into_response());
        }
    }
    // None yet, or its creator can no longer share: replace it.
    let link = state
        .db
        .replace_group_invite_link(
            community,
            id,
            active.as_ref().map(|l| l.code.as_str()),
            &new_invite_code(),
            &caller.pubkey,
        )
        .await
        .map_err(|e| internal("create invite link", &e))?;
    Ok(Json(invite_link_json(&state, &caller.tenant, &link)).into_response())
}

/// `POST /api/groups/{id}/invite-link/reset` → the new `{ code, url }`; the
/// old code stops working. Owner/admin only.
pub async fn reset_invite_link(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "POST",
        &format!("/api/groups/{id}/invite-link/reset"),
        None,
        Some(&body),
    )
    .await?;
    let group = managed_group(&state, &caller, &id).await?;
    let community = caller.tenant.community();
    let active = state
        .db
        .active_group_invite_link(community, group.id)
        .await
        .map_err(|e| internal("invite link", &e))?;
    let link = state
        .db
        .replace_group_invite_link(
            community,
            group.id,
            active.as_ref().map(|l| l.code.as_str()),
            &new_invite_code(),
            &caller.pubkey,
        )
        .await
        .map_err(|e| internal("reset invite link", &e))?;
    Ok(Json(invite_link_json(&state, &caller.tenant, &link)).into_response())
}

/// `GET /api/groups/invite/{code}` → `{ id, name, description, picture,
/// channelType, memberCount, visibility, isMember }` (open and private groups).
pub async fn preview_invite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "GET",
        &format!("/api/groups/invite/{code}"),
        None,
        None,
    )
    .await?;
    rate_limit(&state, &caller, "link:preview", LINK_PREVIEW_PER_MIN).await?;
    let (group, _) = group_for_code(&state, &caller, &code).await?;
    let mut value = group_with_visibility_json(&group);
    if let Some(map) = value.as_object_mut() {
        map.remove("handle");
    }
    Ok(Json(value).into_response())
}

/// `POST /api/groups/invite/{code}/join` → `{ channelId, joined }`; `joined`
/// is false when the caller already was a member. Bypasses "who can add me"
/// (the caller chose to join); refuses agents and people an owner/admin
/// removed from the group.
pub async fn join_invite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(code): Path<String>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "POST",
        &format!("/api/groups/invite/{code}/join"),
        None,
        Some(&body),
    )
    .await?;
    rate_limit(&state, &caller, "link:join", LINK_JOIN_PER_MIN).await?;
    let (group, link) = group_for_code(&state, &caller, &code).await?;
    if group.is_member {
        return Ok(
            Json(json!({ "channelId": group.id.to_string(), "joined": false })).into_response(),
        );
    }
    if is_agent(&state, &caller).await? {
        return Err(error(
            StatusCode::FORBIDDEN,
            "agents join groups when their owner adds them",
        ));
    }
    if state
        .db
        .removed_from_group_by_other(caller.tenant.community(), group.id, &caller.pubkey)
        .await
        .map_err(|e| internal("removed check", &e))?
    {
        return Err(error(
            StatusCode::FORBIDDEN,
            "you were removed from this group; ask a member to add you",
        ));
    }
    // Private groups need an active inviter for add_member: the link's
    // creator, whom `group_for_code` just confirmed.
    let invited_by = (group.visibility == "private").then_some(link.created_by.as_slice());
    add_channel_member(
        &caller.tenant,
        &state,
        group.id,
        &caller.pubkey,
        MemberRole::Member,
        invited_by,
        &caller.pubkey,
    )
    .await
    .map_err(add_failure)?;
    Ok(Json(json!({ "channelId": group.id.to_string(), "joined": true })).into_response())
}

fn parse_invite_policy(body: &[u8]) -> Result<&'static str, Response> {
    match json_body(body)?.get("policy").and_then(Value::as_str) {
        Some("members") => Ok("members"),
        Some("admins") => Ok("admins"),
        _ => Err(error(
            StatusCode::BAD_REQUEST,
            "policy must be \"members\" or \"admins\"",
        )),
    }
}

/// `GET /api/groups/{id}/invite-policy` → `{ policy }` (members of the group).
pub async fn get_invite_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "GET",
        &format!("/api/groups/{id}/invite-policy"),
        None,
        None,
    )
    .await?;
    let id = parse_group_id(&id)?;
    let group = summary(&state, &caller, id)
        .await?
        .filter(|g| is_group_type(g) && (g.is_member || g.visibility == "open"))
        .ok_or_else(not_found)?;
    Ok(Json(json!({ "policy": group.invite_policy })).into_response())
}

/// `PUT /api/groups/{id}/invite-policy` `{ "policy": "members" | "admins" }`
/// → `{ policy }`. Owner/admin only.
pub async fn put_invite_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "PUT",
        &format!("/api/groups/{id}/invite-policy"),
        None,
        Some(&body),
    )
    .await?;
    let group = managed_group(&state, &caller, &id).await?;
    let policy = parse_invite_policy(&body)?;
    if !state
        .db
        .set_group_invite_policy(caller.tenant.community(), group.id, policy)
        .await
        .map_err(|e| internal("set invite policy", &e))?
    {
        return Err(not_found());
    }
    Ok(Json(json!({ "policy": policy })).into_response())
}

// ── S3: who can add me + invitations ────────────────────────────────────────

/// `GET /api/me/group-add-policy` → `{ policy }` (`friends` by default).
pub async fn get_add_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "GET",
        "/api/me/group-add-policy",
        None,
        None,
    )
    .await?;
    let policy = state
        .db
        .group_add_policy(caller.tenant.community(), &caller.pubkey)
        .await
        .map_err(|e| internal("group add policy", &e))?;
    Ok(Json(json!({ "policy": policy })).into_response())
}

/// `PUT /api/me/group-add-policy` `{ "policy": "friends" | "everyone" }` →
/// `{ policy }`.
pub async fn put_add_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "PUT",
        "/api/me/group-add-policy",
        None,
        Some(&body),
    )
    .await?;
    let policy = match json_body(&body)?.get("policy").and_then(Value::as_str) {
        Some("friends") => "friends",
        Some("everyone") => "everyone",
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "policy must be \"friends\" or \"everyone\"",
            ))
        }
    };
    state
        .db
        .set_group_add_policy(caller.tenant.community(), &caller.pubkey, policy)
        .await
        .map_err(|e| internal("set group add policy", &e))?;
    Ok(Json(json!({ "policy": policy })).into_response())
}

/// `GET /api/group-invitations` → `[{ id, channel: { id, name, description,
/// channelType, memberCount, visibility }, inviter, createdAt }]`, newest
/// first. `createdAt` is RFC 3339 UTC.
pub async fn list_invitations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "GET",
        "/api/group-invitations",
        None,
        None,
    )
    .await?;
    let invitations = state
        .db
        .pending_group_invitations(caller.tenant.community(), &caller.pubkey)
        .await
        .map_err(|e| super::db_read_error("group invitations", &e).into_response())?;
    let items: Vec<Value> = invitations
        .iter()
        .map(|invitation| {
            let channel = &invitation.channel;
            json!({
                "id": invitation.id.to_string(),
                "channel": {
                    "id": channel.id.to_string(),
                    "name": channel.name,
                    "description": channel.description,
                    "picture": channel.picture,
                    "channelType": channel.channel_type,
                    "memberCount": channel.member_count,
                    "visibility": channel.visibility,
                },
                "inviter": hex::encode(&invitation.inviter),
                "createdAt": invitation
                    .created_at
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            })
        })
        .collect();
    Ok(Json(Value::Array(items)).into_response())
}

/// `POST /api/group-invitations/{id}/accept` → `{ channelId, joined }`.
/// 404 when not the caller's pending, unexpired invitation; 409 (and the
/// invitation expires) when the group is gone or archived or the inviter may
/// no longer add people.
pub async fn accept_invitation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "POST",
        &format!("/api/group-invitations/{id}/accept"),
        None,
        Some(&body),
    )
    .await?;
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let community = caller.tenant.community();
    let invitation = state
        .db
        .pending_group_invitation(community, id, &caller.pubkey)
        .await
        .map_err(|e| internal("group invitation", &e))?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "invitation not found"))?;
    let group = summary(&state, &caller, invitation.channel_id)
        .await?
        .filter(|g| is_group_type(g) && g.archived_at.is_none());
    let inviter_role = match &group {
        Some(_) => role_of(&state, &caller, invitation.channel_id, &invitation.inviter).await?,
        None => None,
    };
    let still_valid = match (&group, inviter_role) {
        (Some(group), Some(role)) => group.invite_policy != "admins" || role.is_elevated(),
        _ => false,
    };
    let Some(group) = group.filter(|_| still_valid) else {
        if let Err(e) = state
            .db
            .respond_group_invitation(community, id, &caller.pubkey, "expired")
            .await
        {
            tracing::warn!(error = %e, "expiring group invitation failed");
        }
        return Err(error(
            StatusCode::CONFLICT,
            "this invitation is no longer valid",
        ));
    };
    if group.is_member {
        state
            .db
            .respond_group_invitation(community, id, &caller.pubkey, "accepted")
            .await
            .map_err(|e| internal("accept group invitation", &e))?;
        return Ok(
            Json(json!({ "channelId": group.id.to_string(), "joined": false })).into_response(),
        );
    }
    add_channel_member(
        &caller.tenant,
        &state,
        group.id,
        &caller.pubkey,
        MemberRole::Member,
        Some(&invitation.inviter),
        &invitation.inviter,
    )
    .await
    .map_err(add_failure)?;
    Ok(Json(json!({ "channelId": group.id.to_string(), "joined": true })).into_response())
}

/// `POST /api/group-invitations/{id}/decline` → `{ status: "declined" }`.
/// The inviter is not told.
pub async fn decline_invitation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let caller = authenticate(
        &state,
        &headers,
        "POST",
        &format!("/api/group-invitations/{id}/decline"),
        None,
        Some(&body),
    )
    .await?;
    let id = Uuid::parse_str(&id).map_err(|_| not_found())?;
    let community = caller.tenant.community();
    let pending = state
        .db
        .pending_group_invitation(community, id, &caller.pubkey)
        .await
        .map_err(|e| internal("group invitation", &e))?;
    if pending.is_none()
        || !state
            .db
            .respond_group_invitation(community, id, &caller.pubkey, "declined")
            .await
            .map_err(|e| internal("decline group invitation", &e))?
    {
        return Err(error(StatusCode::NOT_FOUND, "invitation not found"));
    }
    Ok(Json(json!({ "status": "declined" })).into_response())
}

// ── Public landing page: /g/<code> and /g/@<handle> ─────────────────────────

/// Deep-link scheme the desktop (tauri deep-link) and mobile (Android intent
/// filter, iOS URL type) apps register.
const APP_SCHEME: &str = "kuzz";
/// Landing pages one client IP may open per minute (as the preview API).
const LANDING_PER_IP_PER_MIN: u64 = LINK_PREVIEW_PER_MIN;
/// Landing pages per minute for all clients together when the IP is unknown.
const LANDING_GLOBAL_PER_MIN: u64 = 600;
/// Longest description shown on the page, in characters.
const LANDING_DESCRIPTION_CHARS: usize = 280;

/// Escape text for HTML element content and double-quoted attributes.
fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// What a landing page shows.
struct Landing<'a> {
    name: &'a str,
    member_count: i64,
    /// `None` for private groups (name and member count only).
    description: Option<&'a str>,
    /// `kuzz://join/<code>` or `kuzz://join/@<handle>`.
    deep_link: String,
    /// Same-origin path of the group photo (`/g/<target>/picture`), if set.
    picture_src: Option<String>,
}

const LANDING_STYLE: &str =
    "body{margin:0;font-family:system-ui,-apple-system,'Segoe UI',sans-serif;\
background:#f6f6f4;color:#1d1d1b;display:flex;min-height:100vh;align-items:center;\
justify-content:center}main{background:#fff;border-radius:16px;padding:32px;max-width:420px;\
width:calc(100% - 48px);box-shadow:0 2px 12px rgba(0,0,0,.08);text-align:center}\
h1{font-size:22px;margin:0 0 8px;overflow-wrap:anywhere}.count{color:#6b6b66;margin:0 0 16px}\
.desc{margin:0 0 24px;overflow-wrap:anywhere;white-space:pre-line}\
.open{display:inline-block;background:#0047a0;color:#fff;text-decoration:none;\
padding:12px 28px;border-radius:999px;font-weight:600}.hint{color:#6b6b66;font-size:14px;\
margin:20px 0 0}.photo{width:96px;height:96px;border-radius:50%;object-fit:cover;\
margin:0 0 16px}";

fn landing_document(title: &str, main: &str) -> String {
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex,nofollow\">\
         <title>{title}</title><style>{LANDING_STYLE}</style></head>\
         <body><main>{main}</main></body></html>",
        title = escape_html(title),
    )
}

fn render_landing(landing: &Landing<'_>) -> String {
    let members = if landing.member_count == 1 {
        "1 member".to_owned()
    } else {
        format!("{} members", landing.member_count)
    };
    let description = landing
        .description
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| {
            let short: String = d.chars().take(LANDING_DESCRIPTION_CHARS).collect();
            let ellipsis = if d.chars().count() > LANDING_DESCRIPTION_CHARS {
                "\u{2026}"
            } else {
                ""
            };
            format!("<p class=\"desc\">{}{ellipsis}</p>", escape_html(&short))
        })
        .unwrap_or_default();
    let photo = landing
        .picture_src
        .as_deref()
        .map(|src| {
            format!(
                "<img class=\"photo\" src=\"{}\" alt=\"\">",
                escape_html(src)
            )
        })
        .unwrap_or_default();
    let main = format!(
        "{photo}<h1>{name}</h1><p class=\"count\">{members}</p>{description}\
         <a class=\"open\" href=\"{link}\">Open in Kuzz</a>\
         <p class=\"hint\">Don&#39;t have Kuzz yet? Ask the person who shared this link.</p>",
        name = escape_html(landing.name),
        link = escape_html(&landing.deep_link),
    );
    landing_document(&format!("{} on Kuzz", landing.name), &main)
}

fn render_landing_not_found() -> String {
    landing_document(
        "Link not found",
        "<h1>This link doesn&#39;t work</h1>\
         <p class=\"hint\">It may have been reset or the group may no longer exist. \
         Ask the person who shared it for a new link.</p>",
    )
}

fn html_response(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'; img-src 'self'; base-uri 'none'; \
                 form-action 'none'; frame-ancestors 'none'",
            ),
            (
                axum::http::header::HeaderName::from_static("x-robots-tag"),
                "noindex, nofollow",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
            (axum::http::header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response()
}

/// `GET /g/{code}` and `GET /g/@{handle}` (no auth): a small self-contained
/// HTML page for a shared group link with an "Open in Kuzz" button
/// (`kuzz://join/<code>` / `kuzz://join/@<handle>`). Open groups show name,
/// member count and a short description; private groups (code only) show
/// name and member count. Unknown, revoked, deleted or archived → 404 page.
pub async fn landing(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Path(target): Path<String>,
) -> Response {
    let not_found = || html_response(StatusCode::NOT_FOUND, render_landing_not_found());
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return not_found();
    };
    if super::auth::rate_limit_client(
        &state,
        &extensions,
        &headers,
        &format!("groups:landing:{}", tenant.community()),
        LANDING_PER_IP_PER_MIN,
        LANDING_GLOBAL_PER_MIN,
    )
    .await
    .is_err()
    {
        return html_response(
            StatusCode::TOO_MANY_REQUESTS,
            landing_document(
                "Try again soon",
                "<h1>Too many requests</h1><p class=\"hint\">Try again in a minute.</p>",
            ),
        );
    }
    // An anonymous viewer: the all-zero key is never a member.
    let viewer = Caller {
        tenant,
        pubkey: vec![0u8; 32],
    };
    let page = if let Some(raw) = target.strip_prefix('@') {
        let Ok(handle) = normalize_handle(raw) else {
            return not_found();
        };
        match state
            .db
            .open_group_by_handle(viewer.tenant.community(), &viewer.pubkey, &handle)
            .await
        {
            Ok(Some(group)) => render_landing(&Landing {
                name: &group.name,
                member_count: group.member_count,
                description: group.description.as_deref(),
                deep_link: format!("{APP_SCHEME}://join/@{handle}"),
                picture_src: group
                    .picture
                    .as_ref()
                    .map(|_| format!("/g/@{handle}/picture")),
            }),
            Ok(None) => return not_found(),
            Err(e) => {
                tracing::warn!(error = %e, "group landing by handle failed");
                return internal("group landing", &e);
            }
        }
    } else {
        match group_for_code(&state, &viewer, &target).await {
            Ok((group, link)) => render_landing(&Landing {
                name: &group.name,
                member_count: group.member_count,
                description: if group.visibility == "open" {
                    group.description.as_deref()
                } else {
                    None
                },
                deep_link: format!("{APP_SCHEME}://join/{}", link.code),
                picture_src: group
                    .picture
                    .as_ref()
                    .map(|_| format!("/g/{}/picture", link.code)),
            }),
            Err(response) if response.status() == StatusCode::NOT_FOUND => return not_found(),
            Err(response) => return response,
        }
    };
    html_response(StatusCode::OK, page)
}

/// The `<sha256>[.ext]` file name of a group photo URL on this relay's /media.
fn picture_media_name(picture: &str) -> Option<&str> {
    let (_, name) = picture.rsplit_once("/media/")?;
    let (hash, ext) = match name.split_once('.') {
        Some((hash, ext)) => (hash, Some(ext)),
        None => (name, None),
    };
    let hash_ok = hash.len() == 64 && hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
    let ext_ok = ext.is_none_or(crate::api::media::is_safe_ext);
    (hash_ok && ext_ok).then_some(name)
}

/// `GET /g/{code}/picture` and `GET /g/@{handle}/picture` (no auth): the photo
/// of the group the landing page shows, so the page can display it without a
/// signed-in media read. Same lookup and rate limit as [`landing`]; 404 when
/// the group is not shown there or has no photo.
pub async fn landing_picture(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    extensions: axum::http::Extensions,
    Path(target): Path<String>,
) -> Response {
    let not_found = || StatusCode::NOT_FOUND.into_response();
    let raw_host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Ok(tenant) = crate::tenant::bind_community(&state.db, raw_host).await else {
        return not_found();
    };
    if super::auth::rate_limit_client(
        &state,
        &extensions,
        &headers,
        &format!("groups:landing:{}", tenant.community()),
        LANDING_PER_IP_PER_MIN,
        LANDING_GLOBAL_PER_MIN,
    )
    .await
    .is_err()
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let viewer = Caller {
        tenant,
        pubkey: vec![0u8; 32],
    };
    let group = if let Some(raw) = target.strip_prefix('@') {
        let Ok(handle) = normalize_handle(raw) else {
            return not_found();
        };
        match state
            .db
            .open_group_by_handle(viewer.tenant.community(), &viewer.pubkey, &handle)
            .await
        {
            Ok(Some(group)) => group,
            Ok(None) => return not_found(),
            Err(e) => return internal("group landing picture", &e),
        }
    } else {
        match group_for_code(&state, &viewer, &target).await {
            Ok((group, _)) => group,
            Err(response) => return response,
        }
    };
    let Some(name) = group.picture.as_deref().and_then(picture_media_name) else {
        return not_found();
    };
    match crate::api::media::serve_blob_for_tenant(&state, &viewer.tenant, name, &headers).await {
        Ok(response) => response,
        Err(_) => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        escape_html, new_invite_code, normalize_handle, picture_media_name, plausible_code,
        render_landing, Landing,
    };

    #[test]
    fn landing_escapes_user_text_and_hides_private_descriptions() {
        assert_eq!(
            escape_html("<b>&\"x'</b>"),
            "&lt;b&gt;&amp;&quot;x&#39;&lt;/b&gt;"
        );
        let page = render_landing(&Landing {
            name: "<script>alert(1)</script>",
            member_count: 1,
            description: Some("a \"quoted\" <img src=x> & more"),
            deep_link: "kuzz://join/abc\"onclick=\"x".to_owned(),
            picture_src: None,
        });
        assert!(!page.contains("<script>alert"), "{page}");
        assert!(!page.contains("<img"), "{page}");
        assert!(page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(page.contains("1 member<"));
        assert!(page.contains("href=\"kuzz://join/abc&quot;onclick=&quot;x\""));
        assert!(page.contains("noindex"));
        assert!(page.contains("Don&#39;t have Kuzz yet? Ask the person who shared this link."));
        assert!(
            !page.contains("http://") && !page.contains("https://"),
            "no external assets"
        );
        let private = render_landing(&Landing {
            name: "Secret",
            member_count: 3,
            description: None,
            deep_link: "kuzz://join/code".to_owned(),
            picture_src: None,
        });
        assert!(private.contains("3 members"));
        assert!(!private.contains("class=\"desc\""));
        assert!(!private.contains("<img"));
    }

    #[test]
    fn landing_shows_group_photo_from_same_origin() {
        let page = render_landing(&Landing {
            name: "Crew",
            member_count: 2,
            description: None,
            deep_link: "kuzz://join/code".to_owned(),
            picture_src: Some("/g/code\"x/picture".to_owned()),
        });
        assert!(
            page.contains("<img class=\"photo\" src=\"/g/code&quot;x/picture\" alt=\"\">"),
            "{page}"
        );
        assert!(!page.contains("http://") && !page.contains("https://"));
    }

    #[test]
    fn picture_media_name_takes_the_blob_file_name() {
        let hash = "ab".repeat(32);
        assert_eq!(
            picture_media_name(&format!("https://relay.example/media/{hash}.jpg")),
            Some(format!("{hash}.jpg").as_str())
        );
        assert_eq!(
            picture_media_name(&format!("https://relay.example/media/{hash}")),
            Some(hash.as_str())
        );
        assert_eq!(
            picture_media_name("https://relay.example/media/../etc"),
            None
        );
        assert_eq!(
            picture_media_name(&format!("https://relay.example/media/{hash}.JPG")),
            None
        );
        assert_eq!(picture_media_name("https://relay.example/x.jpg"), None);
    }

    #[test]
    fn handles_are_normalized_and_validated() {
        assert_eq!(normalize_handle(" @Design_Team ").unwrap(), "design_team");
        assert_eq!(normalize_handle("abc").unwrap(), "abc");
        assert!(normalize_handle("ab").is_err());
        assert!(normalize_handle(&"a".repeat(33)).is_err());
        assert!(normalize_handle("has space").is_err());
        assert!(normalize_handle("dash-name").is_err());
        assert!(normalize_handle("한글이름").is_err());
        for reserved in [
            "admin", "KUZZ", "support", "help", "settings", "api", "invite",
        ] {
            assert_eq!(
                normalize_handle(reserved),
                Err("this address is reserved"),
                "{reserved}"
            );
        }
    }

    #[test]
    fn invite_codes_are_long_random_and_url_safe() {
        let a = new_invite_code();
        let b = new_invite_code();
        assert_eq!(a.len(), 24);
        assert_ne!(a, b);
        assert!(plausible_code(&a));
        assert!(!plausible_code("short"));
        assert!(!plausible_code("has/slash-has/slash-has/slash"));
    }
}
