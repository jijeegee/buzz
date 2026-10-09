//! Restoring a stored Google session for a community (keyring refresh token →
//! access token) and starting its background refresh loop.
//!
//! Called lazily: by the status command the UI runs on community init, and by
//! the agent start path, which must never decide "key mode" for a community
//! the user is signed in to merely because the UI has not asked yet.

use std::sync::Arc;
use std::time::Duration;

use tauri::{Emitter, Manager};

use super::refresh::{self, RefreshFailure};
use super::{KeyringRefreshStore, TOKEN_AUTH_CHANGED_EVENT};
use crate::app_state::AppState;

/// How long a caller waits for a restore. Longer than the restore's own two
/// requests (`/auth/refresh` + `/auth/me`), and the restore itself runs
/// detached, so giving up waiting never cancels a refresh-token rotation
/// mid-flight (which would drop the rotated token).
pub(crate) const RESTORE_TIMEOUT: Duration =
    Duration::from_secs(2 * super::api::AUTH_REQUEST_TIMEOUT.as_secs() + 5);

/// Emit the token-auth-changed event for `origin`.
pub(crate) fn emit_changed<R: tauri::Runtime>(app: &tauri::AppHandle<R>, origin: &str) {
    let _ = app.emit(TOKEN_AUTH_CHANGED_EVENT, origin.to_owned());
    // A session that just became active (restore or sign-in) knows this
    // device: tell the owner which agents it hosts. Unchanged lists are skipped.
    crate::device_robot::spawn_publish_agent_host_devices(app);
}

/// Start the background refresh loop for `origin`'s login `generation`.
pub(crate) fn spawn_refresh_loop<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    origin: &str,
    generation: u64,
) {
    let state = app.state::<AppState>();
    let auth = Arc::clone(&state.token_auth);
    let client = state.http_client.clone();
    let notify_app = app.clone();
    tauri::async_runtime::spawn(refresh::run_refresh_loop(
        auth,
        client,
        origin.to_owned(),
        generation,
        Arc::new(KeyringRefreshStore),
        move |origin| emit_changed(&notify_app, origin),
    ));
}

fn finish<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    origin: &str,
    generation: u64,
    attempt: &Result<(), RefreshFailure>,
) {
    // Active, or still restoring after a transient failure: the loop keeps it
    // fresh or keeps retrying. A terminal failure already moved the origin to
    // "sign in again".
    if !matches!(attempt, Err(RefreshFailure::Terminal(_))) {
        spawn_refresh_loop(app, origin, generation);
    }
    emit_changed(app, origin);
}

/// Start (or join) the single-flight restore of `origin` on the app runtime.
/// The task runs to completion even when every waiter gives up.
fn spawn_restore<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    origin: &str,
) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    let origin = origin.to_owned();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if let Some((generation, attempt)) = refresh::restore_session(
            &state.token_auth,
            &state.http_client,
            &origin,
            &KeyringRefreshStore,
        )
        .await
        {
            finish(&app, &origin, generation, &attempt);
        }
    })
}

/// Restore `origin`'s stored session, if any, and wait (bounded) for it or for
/// a restore already in flight, so the caller re-reads a settled state.
pub(crate) async fn restore_if_stored<R: tauri::Runtime>(app: &tauri::AppHandle<R>, origin: &str) {
    let _ = tokio::time::timeout(RESTORE_TIMEOUT, spawn_restore(app, origin)).await;
}

/// [`restore_if_stored`] for sync agent start paths (blocks the caller on an
/// isolated thread).
pub(crate) fn restore_if_stored_blocking<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    origin: &str,
) {
    let handle = spawn_restore(app, origin);
    let _ = super::bots::block_on_isolated(async move {
        let _ = tokio::time::timeout(RESTORE_TIMEOUT, handle).await;
    });
}
