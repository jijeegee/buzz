//! Moving this install from a software device key to the TPM or Secure
//! Enclave without signing anyone out (relay `POST /auth/device/rotate`).
//!
//! All accounts and agents of an install share one device key, and the relay
//! binds it per device row (one row per account). So the move is done for
//! every signed-in account at once, at launch:
//!
//! 1. Wait until every account listed as signed in has its session back.
//! 2. Hold every one of those sessions' refresh locks, so no refresh is
//!    proven by a key the relay just dropped.
//! 3. Ask the relay to move each account's device row, proven by the old key
//!    (the request) and the new one (the body). The row keeps its sessions
//!    and the agents hosted on it.
//! 4. Once every row moved, record the hardware key as the key in use and
//!    sign with it, here and in the app signer.
//!
//! If the first move fails nothing changed: the software key stays and the
//! next launch tries again. A move that fails after another succeeded is
//! retried; if it still fails the install moves on anyway (most rows are on
//! the new key) and that account asks to sign in again. A row that an
//! earlier, interrupted run already moved answers the new key, which counts
//! as moved.

use std::sync::Arc;
use std::time::Duration;

use nostr::PublicKey;

use super::accounts::AccountState;
use super::api::ApiError;
use super::{OriginAuth, TokenAuthState, UserSession};
use crate::device_key::DeviceKey;

/// How often (and how many times) launch checks whether the sessions are back.
const READY_POLL: Duration = Duration::from_secs(15);
const READY_TRIES: u32 = 12;
/// Waits before retrying a move that failed after another succeeded.
const RETRY_WAITS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];
/// A session whose access token expires sooner is not used for a move.
const MIN_ACCESS_LEFT_SECS: i64 = 120;

/// Launch: move to the hardware key when the install still uses a software
/// one and this machine has usable hardware. Runs once, in the background.
pub(crate) fn spawn<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(Some(hardware)) =
            tauri::async_runtime::spawn_blocking(crate::device_key::hardware_upgrade).await
        else {
            return;
        };
        let Some(software) = crate::device_key::current() else {
            return;
        };
        for _ in 0..READY_TRIES {
            match attempt(&app, &software, &hardware).await {
                Attempt::NotReady => tokio::time::sleep(READY_POLL).await,
                Attempt::Finished => return,
            }
        }
        eprintln!("kuzz-desktop: device key: sessions not back; staying on the software key");
    });
}

enum Attempt {
    /// A listed session is still coming back; try again shortly.
    NotReady,
    /// Moved, or stopped for this launch.
    Finished,
}

/// One signed-in session, with its refresh lock held.
struct Held {
    origin: String,
    account: String,
    session: UserSession,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

async fn attempt<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    software: &DeviceKey,
    hardware: &DeviceKey,
) -> Attempt {
    use tauri::Manager;
    let state = app.state::<crate::app_state::AppState>();
    let Ok(data_dir) = app.path().app_data_dir() else {
        return Attempt::Finished;
    };
    let registry = match super::accounts::load_registry(&data_dir) {
        Ok(registry) => registry.unwrap_or_default(),
        Err(error) => {
            eprintln!("kuzz-desktop: device key: account list unreadable: {error}");
            return Attempt::Finished;
        }
    };
    let active = registry.active_pubkey();
    let mut wanted: Vec<(String, Option<PublicKey>)> = registry
        .accounts
        .iter()
        .filter(|a| a.state == AccountState::SignedIn)
        .filter_map(|a| {
            let pubkey = PublicKey::from_hex(&a.pubkey).ok()?;
            // The active account's session lives in the app's own state.
            let background = (Some(pubkey) != active).then_some(pubkey);
            Some((a.origin.clone(), background))
        })
        .collect();
    // Sessions of the active account the list does not name (older installs).
    for origin in state.token_auth.known_origins() {
        if !wanted.iter().any(|(o, b)| *o == origin && b.is_none()) {
            if let Some((_, OriginAuth::Active(_) | OriginAuth::Restoring)) =
                state.token_auth.get(&origin)
            {
                wanted.push((origin, None));
            }
        }
    }
    // Background sessions' locks before the app's own, the order an account
    // switch takes them in (`accounts::hand_over_origins`).
    wanted.sort_by_key(|(origin, background)| (background.is_none(), origin.clone()));
    let mut seen = std::collections::HashSet::new();
    wanted.retain(|(origin, background)| {
        seen.insert((origin.clone(), background.map(|k| k.to_hex())))
    });
    if wanted.is_empty() {
        // Nobody signed in: the next launch after a sign-in moves the key.
        return Attempt::Finished;
    }

    let mut held = Vec::with_capacity(wanted.len());
    for (origin, background) in wanted {
        let auth: Option<Arc<TokenAuthState>> = match background {
            None => Some(Arc::clone(&state.token_auth)),
            Some(pubkey) => state
                .token_auth
                .inactive
                .on_origin(&origin)
                .into_iter()
                .find(|(account, _)| *account == pubkey)
                .map(|(_, auth)| auth),
        };
        let Some(auth) = auth else {
            return Attempt::NotReady;
        };
        let guard = auth.refresh_lock(&origin).lock_owned().await;
        let session = match auth.get(&origin) {
            Some((_, OriginAuth::Active(session))) => session,
            _ => return Attempt::NotReady,
        };
        if session.device_id.is_none()
            || session.access_expires_at < unix_now() + MIN_ACCESS_LEFT_SECS
        {
            return Attempt::NotReady;
        }
        held.push(Held {
            origin,
            account: background
                .map_or_else(|| "active".into(), |pubkey| pubkey.to_hex()[..8].into()),
            session,
            _guard: guard,
        });
    }

    let mut moved = 0usize;
    let mut stranded = Vec::new();
    for entry in &held {
        let mut result = move_one(&state.http_client, entry, software, hardware).await;
        if moved > 0 {
            for wait in RETRY_WAITS {
                if result.is_ok() {
                    break;
                }
                tokio::time::sleep(wait).await;
                result = move_one(&state.http_client, entry, software, hardware).await;
            }
        }
        match result {
            Ok(()) => moved += 1,
            Err(error) if moved == 0 => {
                eprintln!(
                    "kuzz-desktop: device key: not moved to hardware ({} on {}: {}); \
                     staying on the software key",
                    entry.account,
                    entry.origin,
                    error.code.as_deref().unwrap_or(&error.message)
                );
                return Attempt::Finished;
            }
            Err(error) => stranded.push(format!(
                "{} on {} ({})",
                entry.account,
                entry.origin,
                error.code.as_deref().unwrap_or(&error.message)
            )),
        }
    }

    let hardware = hardware.clone();
    let adopted =
        tauri::async_runtime::spawn_blocking(move || crate::device_key::adopt_hardware(&hardware))
            .await
            .map_err(|error| error.to_string())
            .and_then(|saved| saved);
    drop(held);
    match adopted {
        Ok(()) => eprintln!("kuzz-desktop: device key: moved to hardware for {moved} sign-in(s)"),
        Err(error) => eprintln!(
            "kuzz-desktop: warning: device key: moved to hardware, but not saved: {error}"
        ),
    }
    if !stranded.is_empty() {
        eprintln!(
            "kuzz-desktop: warning: device key: these sign-ins stayed on the old key and \
             will ask to sign in again: {}",
            stranded.join(", ")
        );
    }
    Attempt::Finished
}

/// Move one account's device row to `hardware`. A row an interrupted run
/// already moved refuses the old key (401); it counts as moved when the
/// relay confirms the new key is its key.
async fn move_one(
    client: &reqwest::Client,
    entry: &Held,
    software: &DeviceKey,
    hardware: &DeviceKey,
) -> Result<(), ApiError> {
    let device_id = entry.session.device_id.as_deref().unwrap_or_default();
    let access = entry.session.access.as_str();
    let error = match super::api::rotate_device_key(
        client,
        &entry.origin,
        access,
        device_id,
        software,
        hardware,
    )
    .await
    {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    if error.status != Some(401) {
        return Err(error);
    }
    match super::api::rotate_device_key(
        client,
        &entry.origin,
        access,
        device_id,
        hardware,
        hardware,
    )
    .await
    {
        Err(probe) if already_moved(&probe) => Ok(()),
        _ => Err(error),
    }
}

/// Whether a rotation to the key that proved the request was refused because
/// that key is already the row's key. Pure.
fn already_moved(error: &ApiError) -> bool {
    error.status == Some(409) && error.code.as_deref() == Some("device_key_unchanged")
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(status: u16, code: &str) -> ApiError {
        ApiError {
            status: Some(status),
            code: Some(code.into()),
            message: String::new(),
        }
    }

    #[test]
    fn only_an_unchanged_key_answer_counts_as_already_moved() {
        assert!(already_moved(&err(409, "device_key_unchanged")));
        for other in [
            err(409, "device_key_changed"),
            err(409, "device_key_in_use"),
            err(401, "device_proof_invalid"),
            err(403, "device_proof_invalid"),
        ] {
            assert!(!already_moved(&other));
        }
    }
}
