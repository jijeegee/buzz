//! HTTP routes that write global data are served by the primary region
//! (global regions stage S5-2, design D3.1).
//!
//! A region holding the global copy (S5-1) must not write accounts,
//! sessions, bots, operators, communities or invites itself: the copy guard
//! refuses it. Sign-in (`/auth/*`, except the per-region blocks and the
//! people reads), the admin API, the operator API and invites are therefore
//! forwarded whole to the primary over the region link: method, path,
//! headers, body and the client's address and country. The primary runs the
//! request through its own router and the answer comes back unchanged, with
//! its status, cookies and redirects.
//!
//! The client address is the one this region worked out from its own
//! trusted proxies; the primary takes it as the peer, so its per-IP limits
//! count the real client. An account created through a forwarded sign-up
//! gets this region as its home region.
//!
//! If the primary cannot be reached, these requests fail with 503 (late
//! rather than split). Chat never takes this path. The one exception is a
//! refresh while the copy has fallen behind: the region gives out an access
//! token of its own (S5-6, `api::auth::outage_refresh`).

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, OriginalUri, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower::ServiceExt as _;
use uuid::Uuid;

use crate::region_link::CallError;
use crate::state::AppState;

/// Primary side: run a forwarded HTTP request.
pub const PATH: &str = "/internal/region/http";

/// Largest forwarded request body. The envelope carries it as base64 under
/// the link's 1 MiB body limit.
const REQUEST_BODY_LIMIT: usize = 512 * 1024;
/// Largest answer the primary sends back.
const RESPONSE_BODY_LIMIT: usize = 8 * 1024 * 1024;

const UNREACHABLE: &str = "could not reach the primary region; try again";

/// Whether this request writes global data and is served by the primary.
pub fn forwards(method: &Method, path: &str) -> bool {
    if method == Method::OPTIONS {
        return false;
    }
    if let Some(rest) = path.strip_prefix("/auth/") {
        // Blocks are kept in every region (S4-5b); people reads only read.
        return !(rest == "blocks" || rest.starts_with("blocks/") || rest.starts_with("people/"));
    }
    path == "/api/admin/v1"
        || path.starts_with("/api/admin/v1/")
        || path.starts_with("/operator/")
        || matches!(
            path,
            "/api/invites" | "/api/invites/accept-policy" | "/api/invites/claim"
        )
}

/// Request headers that describe the hop, not the request. The primary sees
/// the client through `client_ip`/`country` instead of forwarding headers.
fn hop_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
            | "forwarded"
            | "x-real-ip"
    ) || name.starts_with("x-forwarded-")
        || name.starts_with("cf-")
}

/// Response headers the entry region sets itself (CORS) or recomputes.
fn entry_response_header(name: &str) -> bool {
    hop_header(name) || name == "vary" || name.starts_with("access-control-")
}

/// A request forwarded to the primary.
#[derive(Debug, Serialize, Deserialize)]
struct Forwarded {
    method: String,
    /// Path and query, as the client sent them.
    uri: String,
    headers: Vec<(String, String)>,
    /// Base64.
    body: String,
    client_ip: Option<IpAddr>,
    country: Option<String>,
}

/// The primary's answer.
#[derive(Debug, Serialize, Deserialize)]
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    /// Base64.
    body: String,
}

/// Set by the primary on a forwarded request: who the client is, as the
/// entry region saw them.
#[derive(Debug, Clone)]
pub struct ForwardedClient {
    /// The region the request came in through (attested by the link).
    pub entry_region: String,
    /// The client's country from the entry region's trusted proxy.
    pub country: Option<String>,
}

fn header_pairs(headers: &HeaderMap, skip: fn(&str) -> bool) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| !skip(name.as_str()))
        .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_string())))
        .collect()
}

fn fill_headers(target: &mut HeaderMap, pairs: &[(String, String)], skip: fn(&str) -> bool) {
    for (name, value) in pairs {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) else {
            continue;
        };
        if !skip(name.as_str()) {
            target.append(name, value);
        }
    }
}

fn plain_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Whether a refresh that could not be sent to the primary should get an
/// outage token here (S5-6): on, a refresh, and this region's copy has
/// fallen behind, so the primary has stopped sending changes too. A primary
/// that cannot be reached while the copy keeps up is not down; the app
/// retries.
fn outage_refresh_applies(state: &AppState, method: &Method, path: &str) -> bool {
    state.config.outage_tokens
        && method == Method::POST
        && path == "/auth/refresh"
        && crate::region_global_read::copy_lagging(state)
}

/// A request signed with an access token this region gave out while the
/// primary was down (S5-6) is not forwarded: the primary does not know the
/// token and would answer `invalid_token`, which apps take as signed out.
/// 503 makes the app retry, and its next refresh reaches the primary.
async fn outage_token_held(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = kuzz_auth::TokenSecret::new(token.trim().to_owned());
    if token.kind() != Some(kuzz_auth::TokenKind::UserAccess) {
        return None;
    }
    let hash = token.hash();
    let unavailable = |error: &dyn std::fmt::Display| {
        tracing::warn!(%error, "outage token lookup failed");
        Some(plain_error(StatusCode::SERVICE_UNAVAILABLE, UNREACHABLE))
    };
    match state.db.is_outage_access_token(&hash).await {
        Ok(false) => return None,
        Ok(true) => {}
        Err(error) => return unavailable(&error),
    }
    // An outage token that no longer works gets the answer any dead token
    // gets; only a live one is told to retry.
    let record = match state.db.lookup_access_token(&hash).await {
        Ok(Some(record)) => record,
        Ok(None) => return Some(token_refused("invalid_token", "authentication failed")),
        Err(error) => return unavailable(&error),
    };
    use kuzz_db::identity::AccessTokenRejection as Rejection;
    Some(match record.check(chrono::Utc::now()) {
        Ok(()) => {
            metrics::counter!("kuzz_outage_tokens_held_total").increment(1);
            plain_error(StatusCode::SERVICE_UNAVAILABLE, UNREACHABLE)
        }
        Err(Rejection::Expired) => token_refused("token_expired", "token expired"),
        Err(Rejection::Revoked) => token_refused("token_revoked", "session revoked"),
        Err(Rejection::PrincipalDisabled) => {
            token_refused("principal_disabled", "account disabled")
        }
        Err(Rejection::InvalidForBot) => token_refused("invalid_token", "authentication failed"),
    })
}

/// A 401 in the auth API's shape.
fn token_refused(code: &str, message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": message, "code": code })),
    )
        .into_response()
}

/// Entry side middleware: in a region holding the global copy, send a
/// global write to the primary and return its answer.
pub async fn forward(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    if !state.config.global_copy || !forwards(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    if let Some(response) = outage_token_held(&state, request.headers()).await {
        return response;
    }
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, REQUEST_BODY_LIMIT).await else {
        return plain_error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large");
    };
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    let proxies = &state.identity.config().trusted_proxies;
    let forwarded = Forwarded {
        method: parts.method.to_string(),
        uri: parts
            .uri
            .path_and_query()
            .map_or_else(|| parts.uri.path().to_string(), |pq| pq.to_string()),
        headers: header_pairs(&parts.headers, hop_header),
        body: BASE64.encode(&body),
        client_ip: proxies.client_ip(peer, &parts.headers),
        country: proxies.country(peer, &parts.headers),
    };
    let Ok(forwarded) = serde_json::to_value(&forwarded) else {
        return plain_error(StatusCode::INTERNAL_SERVER_ERROR, "internal server error");
    };
    let answer = match crate::region_link::call(
        &state,
        &state.config.primary_region,
        Uuid::nil(),
        PATH,
        &forwarded,
        None,
        None,
    )
    .await
    {
        Ok(answer) => answer,
        Err(error) => {
            let reason = match &error {
                CallError::Off => "off",
                CallError::UnknownRegion => "unknown_region",
                CallError::Unreachable(_) | CallError::NotSent(_) => "unreachable",
                CallError::Status(..) => "status",
            };
            metrics::counter!("kuzz_region_http_forward_failures_total", "reason" => reason)
                .increment(1);
            tracing::warn!(reason, "Could not forward a global write to the primary");
            // Only when the primary never got the request: after a timeout or
            // an error answer it may have rotated the refresh token already,
            // and handing back the old one would trip its reuse check later.
            let not_sent = matches!(error, CallError::NotSent(_));
            if not_sent && outage_refresh_applies(&state, &parts.method, parts.uri.path()) {
                return crate::api::auth::outage_refresh(&state, &parts.headers, &body).await;
            }
            return plain_error(StatusCode::SERVICE_UNAVAILABLE, UNREACHABLE);
        }
    };
    let Ok(answer) = serde_json::from_value::<Answer>(answer) else {
        return plain_error(StatusCode::BAD_GATEWAY, UNREACHABLE);
    };
    let (Ok(status), Ok(body)) = (
        StatusCode::from_u16(answer.status),
        BASE64.decode(answer.body.as_bytes()),
    ) else {
        return plain_error(StatusCode::BAD_GATEWAY, UNREACHABLE);
    };
    metrics::counter!("kuzz_region_http_forwarded_total").increment(1);
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    fill_headers(
        response.headers_mut(),
        &answer.headers,
        entry_response_header,
    );
    response
}

/// Primary side: `POST /internal/region/http`. Admits the calling region,
/// runs the forwarded request through this region's router and answers with
/// its status, headers and body.
pub async fn serve(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let claims = match crate::region_link::admit_deployment(
        &state,
        &Method::POST,
        &uri,
        &headers,
        &body,
    )
    .await
    {
        Ok(claims) => claims,
        Err(refusal) => {
            metrics::counter!("kuzz_region_link_calls_refused_total", "reason" => refusal.label())
                .increment(1);
            tracing::warn!(reason = refusal.label(), "Refused a forwarded HTTP request");
            return plain_error(StatusCode::UNAUTHORIZED, "region link refused");
        }
    };
    // Only the primary serves these: a copy forwarding to a copy would loop.
    if state.config.global_copy {
        return plain_error(StatusCode::CONFLICT, "not the primary region");
    }
    let Ok(forwarded) = serde_json::from_slice::<Forwarded>(&body) else {
        return plain_error(StatusCode::BAD_REQUEST, "malformed forwarded request");
    };
    let (Ok(method), Ok(target), Ok(request_body)) = (
        Method::from_bytes(forwarded.method.as_bytes()),
        forwarded.uri.parse::<Uri>(),
        BASE64.decode(forwarded.body.as_bytes()),
    ) else {
        return plain_error(StatusCode::BAD_REQUEST, "malformed forwarded request");
    };
    // The link carries global writes only, never an internal route.
    if target.scheme().is_some() || !forwards(&method, target.path()) {
        return plain_error(StatusCode::FORBIDDEN, "route is not forwarded");
    }

    let mut request = Request::new(Body::from(request_body));
    *request.method_mut() = method;
    *request.uri_mut() = target;
    fill_headers(request.headers_mut(), &forwarded.headers, hop_header);
    if let Some(ip) = forwarded.client_ip {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(ip, 0)));
    }
    request.extensions_mut().insert(ForwardedClient {
        entry_region: claims.entry_region,
        country: forwarded
            .country
            .filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())),
    });

    // Forwarded requests are rare (sign-in, admin, invites); building the
    // router per request keeps no second copy of the state alive.
    let response = match crate::router::build_router(Arc::clone(&state))
        .oneshot(request)
        .await
    {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let (parts, body) = response.into_parts();
    let Ok(body) = axum::body::to_bytes(body, RESPONSE_BODY_LIMIT).await else {
        return plain_error(StatusCode::BAD_GATEWAY, "answer too large to forward");
    };
    Json(Answer {
        status: parts.status.as_u16(),
        headers: header_pairs(&parts.headers, hop_header),
        body: BASE64.encode(&body),
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_writes_are_forwarded_and_chat_is_not() {
        for (method, path) in [
            (Method::GET, "/auth/oidc/google/start"),
            (Method::GET, "/auth/oidc/google/callback"),
            (Method::POST, "/auth/oidc/complete"),
            (Method::POST, "/auth/refresh"),
            (Method::GET, "/auth/me"),
            (Method::POST, "/auth/bots"),
            (Method::DELETE, "/auth/account"),
            (Method::GET, "/api/admin/v1/reports"),
            (Method::POST, "/operator/communities"),
            (Method::POST, "/api/invites"),
            (Method::POST, "/api/invites/claim"),
            (Method::POST, "/api/invites/accept-policy"),
        ] {
            assert!(forwards(&method, path), "{method} {path}");
        }
        for (method, path) in [
            (Method::OPTIONS, "/auth/refresh"),
            (Method::GET, "/auth/blocks"),
            (Method::PUT, "/auth/blocks/ab"),
            (Method::GET, "/auth/people/lookup"),
            (Method::GET, "/"),
            (Method::POST, "/events"),
            (Method::POST, "/query"),
            (Method::GET, "/api/join-policy"),
            (Method::PUT, "/media/upload"),
            (Method::POST, "/internal/region/http"),
            (Method::POST, "/internal/region/events"),
            (Method::GET, "/authx"),
        ] {
            assert!(!forwards(&method, path), "{method} {path}");
        }
    }

    #[test]
    fn hop_and_forwarding_headers_are_not_carried() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("authorization", "Bearer t"),
            ("cookie", "a=1"),
            ("host", "kuzz.example"),
            ("x-forwarded-for", "1.2.3.4"),
            ("forwarded", "for=1.2.3.4"),
            ("cf-ipcountry", "KR"),
            ("cf-connecting-ip", "1.2.3.4"),
            ("x-real-ip", "1.2.3.4"),
            ("content-length", "3"),
        ] {
            headers.append(name, HeaderValue::from_static(value));
        }
        let mut kept: Vec<String> = header_pairs(&headers, hop_header)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        kept.sort();
        assert_eq!(kept, ["authorization", "cookie", "host"]);
    }

    #[test]
    fn answers_keep_every_cookie_and_drop_cors() {
        let pairs = vec![
            ("set-cookie".to_string(), "a=1".to_string()),
            ("set-cookie".to_string(), "b=2".to_string()),
            ("location".to_string(), "kuzz://done".to_string()),
            ("access-control-allow-origin".to_string(), "*".to_string()),
            ("vary".to_string(), "origin".to_string()),
        ];
        let mut headers = HeaderMap::new();
        fill_headers(&mut headers, &pairs, entry_response_header);
        assert_eq!(headers.get_all("set-cookie").iter().count(), 2);
        assert_eq!(headers["location"], "kuzz://done");
        assert!(headers.get("access-control-allow-origin").is_none());
        assert!(headers.get("vary").is_none());
    }
}
