//! Bot registration and bot tokens (plan §2.3, §3.3, §6.5).

use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use buzz_auth::token::{generate_token, TokenKind};
use buzz_auth::TokenBinding;
use buzz_core::principal::{AccessTokenKind, PrincipalId};
use buzz_db::identity::{BotRecord, ExchangeOutcome, IssuedToken, RevokeReason};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::session::{parse_profile_update, profile_response};
use super::{
    auth_error, authenticate_user, bad_request, bearer_token, forbidden, internal, json_object,
    not_found, rate_limit, rejection_response, unavailable, validate_display_name,
};
use crate::identity::{kv, publish_revocations, verify_access_token};
use crate::state::AppState;

/// Exchange replay cache lifetime (plan §3.3, §6.5).
pub(super) const EXCHANGE_REPLAY_TTL_SECS: u64 =
    buzz_core::principal::TOKEN_EXCHANGE_REPLAY_WINDOW_SECS;
/// Exchanges allowed per bot per minute.
pub(super) const EXCHANGES_PER_MINUTE: u64 = 2;

#[derive(Serialize, Deserialize)]
struct CachedExchange {
    token: String,
    expires_at: chrono::DateTime<Utc>,
}

fn parse_bot_id(raw: &str) -> Result<PrincipalId, Response> {
    PrincipalId::from_hex(raw).map_err(|_| not_found("bot not found"))
}

/// Load a live bot owned by the caller.
async fn owned_bot(
    state: &AppState,
    owner: &TokenBinding,
    raw_id: &str,
) -> Result<BotRecord, Response> {
    let bot_id = parse_bot_id(raw_id)?;
    match state.db.get_bot(&bot_id).await {
        Ok(Some(bot)) if bot.owner == owner.principal && bot.deleted_at.is_none() => Ok(bot),
        Ok(_) => Err(not_found("bot not found")),
        Err(error) => Err(internal("get_bot", &error)),
    }
}

/// `POST /auth/bots` `{display_name, host: "this_device" | "headless"}`.
pub(super) async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let body = match json_object(&body) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(display_name) = body.get("display_name").and_then(|v| v.as_str()) else {
        return bad_request("display_name is required");
    };
    if let Err(response) = validate_display_name(display_name) {
        return response;
    }
    let host_device = match body.get("host").and_then(|v| v.as_str()) {
        Some("this_device") => match binding.device_id {
            Some(device) => Some(device),
            None => return internal("create_bot", &"user token without device"),
        },
        Some("headless") => None,
        _ => return bad_request("host must be \"this_device\" or \"headless\""),
    };
    match state
        .db
        .create_bot(&binding.principal, display_name, host_device)
        .await
    {
        Ok(bot) => {
            tracing::info!(owner = %binding.principal, bot = %bot, "auth.bot_created");
            // A new bot is in no community yet; this reaches any the owner
            // pre-provisioned, and first AUTH reconciles the rest.
            crate::identity::profile::spawn_publish_everywhere(&state, bot);
            (
                StatusCode::CREATED,
                axum::Json(json!({ "bot_id": bot.to_hex() })),
            )
                .into_response()
        }
        Err(buzz_db::DbError::AccessDenied(message)) => {
            auth_error(StatusCode::FORBIDDEN, "limit_reached", &message)
        }
        Err(error) => internal("create_bot", &error),
    }
}

/// `DELETE /auth/bots/{id}`.
pub(super) async fn delete_bot(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let Ok(bot_id) = parse_bot_id(&id) else {
        return not_found("bot not found");
    };
    match state.db.delete_bot(&binding.principal, &bot_id).await {
        Ok(Some(revoked)) => {
            publish_revocations(&state, &revoked, RevokeReason::BotDeleted.as_str(), None).await;
            tracing::info!(owner = %binding.principal, bot = %bot_id, "auth.bot_deleted");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => not_found("bot not found"),
        Err(error) => internal("delete_bot", &error),
    }
}

/// `PATCH /auth/bots/{id}/profile` — owner edits the bot's global profile.
pub(super) async fn update_profile(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let bot = match owned_bot(&state, &binding, &id).await {
        Ok(bot) => bot,
        Err(response) => return response,
    };
    let update = match parse_profile_update(&body, false) {
        Ok(update) => update,
        Err(response) => return response,
    };
    match state.db.update_principal_profile(&bot.id, &update).await {
        Ok(Some(record)) => {
            crate::identity::profile::spawn_publish_everywhere(&state, record.id);
            profile_response(&record)
        }
        Ok(None) => not_found("bot not found"),
        Err(error) => internal("update_bot_profile", &error),
    }
}

/// `POST /auth/bots/{id}/token` — desktop issues a fresh `bzb_` for a bot it
/// hosts; every earlier `bzb_` of the bot is revoked (`reissued`).
pub(super) async fn issue_token(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let bot = match owned_bot(&state, &binding, &id).await {
        Ok(bot) => bot,
        Err(response) => return response,
    };
    let Some(device_id) = binding
        .device_id
        .filter(|device| bot.host_device_id == Some(*device))
    else {
        return forbidden("only the hosting device can issue this bot's token");
    };
    if let Err(response) = rate_limit(
        &state,
        &format!("auth:bot_token:{}", binding.principal.to_hex()),
        60,
        10,
    )
    .await
    {
        return response;
    }
    let (token, hash) = generate_token(TokenKind::BotAccess);
    let expires_at = Utc::now()
        + chrono::Duration::from_std(state.identity.config().bot_ttl).unwrap_or_default();
    match state
        .db
        .issue_bot_token(
            &bot.id,
            device_id,
            IssuedToken {
                hash,
                expires_at: Some(expires_at),
            },
        )
        .await
    {
        Ok(revoked) => {
            publish_revocations(&state, &revoked, RevokeReason::Reissued.as_str(), None).await;
            axum::Json(json!({ "token": token.expose(), "expires_at": expires_at })).into_response()
        }
        Err(error) => internal("issue_bot_token", &error),
    }
}

/// `POST /auth/bots/{id}/revoke` — Stop: revoke the bot's `bzb_` tokens.
pub(super) async fn stop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let bot = match owned_bot(&state, &binding, &id).await {
        Ok(bot) => bot,
        Err(response) => return response,
    };
    match state
        .db
        .revoke_bot_tokens(&bot.id, &["bot"], RevokeReason::Stopped)
        .await
    {
        Ok(revoked) => {
            publish_revocations(&state, &revoked, RevokeReason::Stopped.as_str(), None).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => internal("revoke_bot_tokens", &error),
    }
}

/// `POST /auth/bots/revoke-all` — every bot token of every owned bot.
pub(super) async fn revoke_all(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    match state
        .db
        .revoke_all_owned_bot_tokens(&binding.principal)
        .await
    {
        Ok(revoked) => {
            publish_revocations(&state, &revoked, RevokeReason::RevokeAll.as_str(), None).await;
            tracing::info!(owner = %binding.principal, "auth.bots_revoke_all");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => internal("revoke_all_owned_bot_tokens", &error),
    }
}

/// `POST /auth/bots/{id}/headless-token` — shown once. Only headless bots.
pub(super) async fn issue_headless(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let bot = match owned_bot(&state, &binding, &id).await {
        Ok(bot) => bot,
        Err(response) => return response,
    };
    if bot.host_device_id.is_some() {
        return forbidden("headless tokens are only issued to headless bots");
    }
    if let Err(response) = rate_limit(
        &state,
        &format!("auth:bot_token:{}", binding.principal.to_hex()),
        60,
        10,
    )
    .await
    {
        return response;
    }
    let (token, hash) = generate_token(TokenKind::BotHeadless);
    match state
        .db
        .issue_headless_bot_token(
            &bot.id,
            IssuedToken {
                hash,
                expires_at: None,
            },
        )
        .await
    {
        Ok(()) => axum::Json(json!({
            "token": token.expose(),
            "hash_prefix": hex::encode(&hash[..6]),
        }))
        .into_response(),
        Err(error) => internal("issue_headless_bot_token", &error),
    }
}

/// `DELETE /auth/bots/{id}/headless-token/{hash_prefix}` — second step of a
/// headless token rotation.
pub(super) async fn revoke_headless(
    State(state): State<Arc<AppState>>,
    Path((id, hash_prefix)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let binding = match authenticate_user(&state, &headers).await {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let bot = match owned_bot(&state, &binding, &id).await {
        Ok(bot) => bot,
        Err(response) => return response,
    };
    if !(8..=64).contains(&hash_prefix.len()) || !hash_prefix.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return bad_request("hash_prefix must be 8-64 hex characters");
    }
    match state
        .db
        .revoke_headless_bot_token(&bot.id, &hash_prefix)
        .await
    {
        Ok(Some(hash)) => {
            publish_revocations(&state, &[hash], RevokeReason::Reissued.as_str(), None).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => not_found("no single live headless token matches"),
        Err(error) => internal("revoke_headless_bot_token", &error),
    }
}

/// Polls of the replay cache while a concurrent exchange commits.
const REPLAY_POLL_ATTEMPTS: u32 = 20;
/// Interval between replay-cache polls.
const REPLAY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// The cached response for a retried exchange of `old_hash_hex`, if any.
async fn cached_exchange(
    state: &AppState,
    old_hash_hex: &str,
) -> Result<Option<Response>, Response> {
    match kv::get(&state.redis_pool, "exchange:replay", old_hash_hex).await {
        Ok(cached) => Ok(cached
            .and_then(|json| serde_json::from_str::<CachedExchange>(&json).ok())
            .map(|cached| {
                axum::Json(json!({ "token": cached.token, "expires_at": cached.expires_at }))
                    .into_response()
            })),
        Err(error) => {
            tracing::warn!(%error, "exchange replay cache unavailable");
            Err(unavailable())
        }
    }
}

/// `POST /auth/token/exchange` — a desktop-hosted bot swaps its still-valid
/// `bzb_` for a new one.
///
/// Order (plan §3.3): (1) a retry within 10 s of a successful exchange gets
/// the same new token from the replay cache — also when it raced the commit
/// and missed the cache, by polling it briefly; (2) a token superseded more
/// than 10 s ago gets 409 `token_superseded`; (3) otherwise exchange, cache,
/// and start the
/// old token's grace period (its bound connections get a lowered deadline,
/// not an immediate close — they are expected to re-AUTH).
pub(super) async fn exchange(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return auth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "missing bearer token",
        );
    };
    if token.kind() != Some(TokenKind::BotAccess) {
        return forbidden("only desktop-hosted bot tokens can be exchanged");
    }
    let old_hash = token.hash();
    let old_hash_hex = hex::encode(old_hash);
    match cached_exchange(&state, &old_hash_hex).await {
        Ok(Some(response)) => return response,
        Ok(None) => {}
        Err(response) => return response,
    }
    let binding = match verify_access_token(&state, &token).await {
        Ok(binding) => binding,
        Err(rejection) => return rejection_response(rejection),
    };
    if binding.kind != AccessTokenKind::Bot {
        return forbidden("only desktop-hosted bot tokens can be exchanged");
    }
    if let Err(response) = rate_limit(
        &state,
        &format!("auth:exchange:{}", binding.principal.to_hex()),
        60,
        EXCHANGES_PER_MINUTE,
    )
    .await
    {
        return response;
    }
    let (new_token, new_hash) = generate_token(TokenKind::BotAccess);
    let expires_at = Utc::now()
        + chrono::Duration::from_std(state.identity.config().bot_ttl).unwrap_or_default();
    let grace =
        chrono::Duration::from_std(state.identity.config().exchange_grace).unwrap_or_default();
    let outcome = match state
        .db
        .exchange_bot_token(
            &old_hash,
            IssuedToken {
                hash: new_hash,
                expires_at: Some(expires_at),
            },
            grace,
            chrono::Duration::seconds(EXCHANGE_REPLAY_TTL_SECS as i64),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => return internal("exchange_bot_token", &error),
    };
    match outcome {
        ExchangeOutcome::Exchanged {
            bot_id,
            old_grace_until,
            revoked,
        } => {
            let cached = CachedExchange {
                token: new_token.expose().to_owned(),
                expires_at,
            };
            if let Ok(json) = serde_json::to_string(&cached) {
                if let Err(error) = kv::put_replay(
                    &state.redis_pool,
                    "exchange:replay",
                    &old_hash_hex,
                    &json,
                    EXCHANGE_REPLAY_TTL_SECS,
                )
                .await
                {
                    tracing::warn!(%error, "exchange replay cache write failed");
                }
            }
            // Stale grace-period tokens beyond the old one: close now.
            publish_revocations(&state, &revoked, RevokeReason::Exchanged.as_str(), None).await;
            // The old token: lower bound connections' deadline to the grace end.
            publish_revocations(
                &state,
                &[old_hash],
                RevokeReason::Exchanged.as_str(),
                Some(old_grace_until.timestamp()),
            )
            .await;
            tracing::info!(bot = %bot_id, "auth.token_exchanged");
            axum::Json(json!({ "token": new_token.expose(), "expires_at": expires_at }))
                .into_response()
        }
        ExchangeOutcome::RecentlyExchanged => {
            // A retry racing its own exchange (client timeout, slow commit):
            // the winner writes the replay cache right after commit, so wait
            // for it briefly before calling the token superseded.
            for _ in 0..REPLAY_POLL_ATTEMPTS {
                match cached_exchange(&state, &old_hash_hex).await {
                    Ok(Some(response)) => return response,
                    Ok(None) => tokio::time::sleep(REPLAY_POLL_INTERVAL).await,
                    Err(response) => return response,
                }
            }
            tracing::warn!("auth.exchange_unrecoverable_from_cache");
            auth_error(
                StatusCode::CONFLICT,
                "token_superseded",
                "token was already exchanged",
            )
        }
        ExchangeOutcome::Superseded => auth_error(
            StatusCode::CONFLICT,
            "token_superseded",
            "token was already exchanged",
        ),
        ExchangeOutcome::NotFound => auth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_token",
            "authentication failed",
        ),
        ExchangeOutcome::NotExchangeable => forbidden("token is not exchangeable"),
        ExchangeOutcome::Invalid => auth_error(
            StatusCode::UNAUTHORIZED,
            "token_revoked",
            "authentication failed",
        ),
    }
}
