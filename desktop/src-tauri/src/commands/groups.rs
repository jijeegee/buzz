//! Groups HTTP routes on the active community's server: search, public
//! address, invite links, who-can-add-me and invitations (relay PR #98).
//!
//! One narrow command forwards an allow-listed `/api/...` path with the same
//! NIP-98 / bearer auth as every other relay HTTP call. Errors come back as
//! `HTTP <status>: <server text>` so the UI can branch on the status (404,
//! 409, 400, 429) without parsing URLs.

use std::time::Duration;

use reqwest::Method;
use serde_json::Value;
use tauri::State;

use crate::{
    app_state::AppState,
    relay::{build_nip98_auth_header, classify_request_error, relay_api_base_url_with_override},
};

const GROUPS_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Path prefixes this command may reach. Everything else is refused before
/// a request is built.
const ALLOWED_PREFIXES: [&str; 3] = [
    "/api/groups/",
    "/api/group-invitations",
    "/api/me/group-add-policy",
];

fn checked_method(method: &str) -> Result<Method, String> {
    match method {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        "PUT" => Ok(Method::PUT),
        other => Err(format!("unsupported groups method: {other}")),
    }
}

/// Allow-listed, single-origin relative path (query allowed, no fragment,
/// no traversal, no scheme/host smuggling).
fn checked_path(path: &str) -> Result<&str, String> {
    let (route, _query) = path.split_once('?').unwrap_or((path, ""));
    let allowed = ALLOWED_PREFIXES
        .iter()
        .any(|prefix| route.starts_with(prefix));
    if !allowed
        || route.contains("..")
        || route.contains("//")
        || path.contains('#')
        || path.contains('\\')
        || path.chars().any(char::is_whitespace)
    {
        return Err("unsupported groups path".to_string());
    }
    Ok(path)
}

/// Turn a non-2xx groups response into `HTTP <status>: <error>`.
///
/// Deliberately not `relay_error_message`: a groups 429 (search is limited
/// per minute) must not arm the app-wide relay back-off gate.
async fn groups_error(response: reqwest::Response) -> String {
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    let text = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    if text.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {text}")
    }
}

#[tauri::command]
pub async fn groups_request(
    method: String,
    path: String,
    body: Option<Value>,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let method = checked_method(&method)?;
    let path = checked_path(&path)?;
    crate::relay_admission::wait_for_rate_limit().await;

    let url = format!(
        "{}{}",
        relay_api_base_url_with_override(&state).trim_end_matches('/'),
        path
    );
    let body_bytes = match &body {
        Some(value) => serde_json::to_vec(value).map_err(|error| error.to_string())?,
        None => Vec::new(),
    };
    let auth = build_nip98_auth_header(&method, &url, &body_bytes, &state)?;
    let mut request = state
        .http_client
        .request(method, &url)
        .header("Authorization", auth)
        .timeout(GROUPS_REQUEST_TIMEOUT);
    if body.is_some() {
        request = request
            .header("Content-Type", "application/json")
            .body(body_bytes);
    }
    let response = request
        .send()
        .await
        .map_err(|error| classify_request_error(&error))?;
    if !response.status().is_success() {
        return Err(groups_error(response).await);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| classify_request_error(&error))?;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        eprintln!("kuzz-desktop: groups route returned malformed JSON: {error}");
        "Something went wrong. Try again.".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_only_groups_routes() {
        for path in [
            "/api/groups/search?q=ab&limit=20",
            "/api/groups/by-handle/team_a",
            "/api/groups/invite/abc_DEF-123",
            "/api/groups/2b1c/invite-link/reset",
            "/api/group-invitations",
            "/api/group-invitations/5/accept",
            "/api/me/group-add-policy",
        ] {
            assert!(checked_path(path).is_ok(), "{path}");
        }
        for path in [
            "/api/admin/v1/members",
            "/api/groups/../admin",
            "/api/groups//x",
            "https://evil.example/api/groups/x",
            "/api/groups/x#frag",
            "/api/groups/x y",
            "/auth/people/lookup",
        ] {
            assert!(checked_path(path).is_err(), "{path}");
        }
    }

    #[test]
    fn allows_only_read_and_write_methods() {
        assert!(checked_method("GET").is_ok());
        assert!(checked_method("PUT").is_ok());
        assert!(checked_method("POST").is_ok());
        assert!(checked_method("DELETE").is_err());
    }
}
