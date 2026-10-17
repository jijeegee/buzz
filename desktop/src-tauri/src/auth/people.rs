//! Friends, blocks and findability for the signed-in account (`/auth/blocks`,
//! `/auth/people/*`, and the Kuzz ID and findability fields of
//! `/auth/me` / `PATCH /auth/profile`).
//!
//! Like every account command, each call acts on the **current** community
//! and captures its origin once.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use super::api::{self, ApiError};
use super::commands::fresh_session;
use crate::app_state::AppState;

/// Shown when the server rate-limits lookups or searches.
const TOO_MANY_TRIES: &str = "Too many tries. Wait a minute and try again.";
/// Shown when the chosen Kuzz ID belongs to someone else.
const ID_TAKEN: &str = "That ID is taken";
/// Shown when a Kuzz ID does not fit the rules.
const ID_RULES: &str = "Use 3 to 32 lowercase letters, numbers or _";

/// The account's Kuzz ID and whether others can find it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeopleSettings {
    pub kuzz_id: Option<String>,
    pub discoverable: bool,
}

/// Plain-words error for the people endpoints.
fn people_error(error: ApiError) -> String {
    match error.status {
        Some(429) => TOO_MANY_TRIES.to_owned(),
        // Only the Kuzz ID can conflict.
        Some(409) => ID_TAKEN.to_owned(),
        _ => error.message,
    }
}

/// A typed Kuzz ID, normalised: trimmed, without a leading `@`, lowercased.
/// Empty clears the ID (`None`); otherwise 3-32 of `[a-z0-9_]`.
pub(crate) fn normalize_kuzz_id(input: &str) -> Result<Option<String>, String> {
    let id = input.trim();
    let id = id.strip_prefix('@').unwrap_or(id).to_ascii_lowercase();
    if id.is_empty() {
        return Ok(None);
    }
    let valid = (3..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if valid {
        Ok(Some(id))
    } else {
        Err(ID_RULES.to_owned())
    }
}

/// The account's Kuzz ID and findability (`GET /auth/me`).
#[tauri::command]
pub async fn get_people_settings(app: AppHandle) -> Result<PeopleSettings, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    let me = api::me(&state.http_client, &origin, &session.access)
        .await
        .map_err(people_error)?;
    Ok(PeopleSettings {
        kuzz_id: me.username,
        discoverable: me.discoverable.unwrap_or(true),
    })
}

/// Set (or clear, when empty) the Kuzz ID; returns the saved ID.
#[tauri::command]
pub async fn set_kuzz_id(app: AppHandle, kuzz_id: String) -> Result<Option<String>, String> {
    let id = normalize_kuzz_id(&kuzz_id)?;
    patch_profile(&app, serde_json::json!({ "username": id })).await?;
    Ok(id)
}

/// "Let people find me by ID or email".
#[tauri::command]
pub async fn set_discoverable(app: AppHandle, discoverable: bool) -> Result<(), String> {
    patch_profile(&app, serde_json::json!({ "discoverable": discoverable })).await
}

async fn patch_profile(app: &AppHandle, body: serde_json::Value) -> Result<(), String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::update_profile(&state.http_client, &origin, &session.access, body)
        .await
        .map_err(people_error)
}

/// The people this account blocked.
#[tauri::command]
pub async fn list_blocked_people(app: AppHandle) -> Result<Vec<api::BlockedPerson>, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::list_blocks(&state.http_client, &origin, &session.access)
        .await
        .map_err(people_error)
}

/// Block (`blocked`) or unblock one person.
#[tauri::command]
pub async fn set_person_blocked(
    app: AppHandle,
    pubkey: String,
    blocked: bool,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::set_block(
        &state.http_client,
        &origin,
        &session.access,
        &pubkey,
        blocked,
    )
    .await
    .map_err(people_error)
}

/// The one person whose Kuzz ID or email is exactly `query`, or `None`.
#[tauri::command]
pub async fn lookup_person(
    app: AppHandle,
    query: String,
) -> Result<Option<api::PersonInfo>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(None);
    }
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::lookup_person(&state.http_client, &origin, &session.access, query)
        .await
        .map_err(people_error)
}

/// People in my channels whose name starts with `query`.
#[tauri::command]
pub async fn search_shared_people(
    app: AppHandle,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<api::PersonInfo>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = fresh_session(&state, &origin).await?;
    api::shared_people(
        &state.http_client,
        &origin,
        &session.access,
        query,
        limit.unwrap_or(api::SHARED_PEOPLE_MAX),
    )
    .await
    .map_err(people_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kuzz_ids_are_normalised_and_checked() {
        assert_eq!(
            normalize_kuzz_id("  @Jiho_1 ").unwrap(),
            Some("jiho_1".into())
        );
        assert_eq!(normalize_kuzz_id("   ").unwrap(), None);
        assert_eq!(normalize_kuzz_id("ab").unwrap_err(), ID_RULES);
        assert_eq!(normalize_kuzz_id(&"a".repeat(33)).unwrap_err(), ID_RULES);
        assert_eq!(normalize_kuzz_id("ji-ho").unwrap_err(), ID_RULES);
        assert_eq!(normalize_kuzz_id("지호abc").unwrap_err(), ID_RULES);
    }

    #[test]
    fn people_errors_read_in_plain_words() {
        let error = |status: u16, code: Option<&str>| ApiError {
            status: Some(status),
            code: code.map(str::to_owned),
            message: "relay text".into(),
        };
        assert_eq!(people_error(error(429, None)), TOO_MANY_TRIES);
        assert_eq!(people_error(error(409, Some("username_taken"))), ID_TAKEN);
        assert_eq!(people_error(error(400, None)), "relay text");
    }
}
