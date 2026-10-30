//! Whether the app may draw the active account from device storage before
//! its saved Google session is restored (docs/message-sync.md#launch-order).
//!
//! Local reads only (memory and the OS keyring), never the network: the UI
//! asks this at launch, draws the stored account at once when it is true, and
//! runs the restore (NIP-11, `/auth/refresh`, `/auth/me`) in the background.

use nostr::PublicKey;
use tauri::{AppHandle, Manager};

use super::{KeyringRefreshStore, OriginAuth, RefreshStore, TokenAuthState};
use crate::app_state::AppState;

/// True when the current community's active account has a saved session
/// that belongs to the key held here: the account bound to the saved
/// session is exactly the held key, the device is not signed out, and the
/// session has not already ended this run. Legacy token accounts (no bound
/// key) answer false and keep waiting for the restore, since their identity
/// is only known from the relay.
#[tauri::command]
pub fn saved_session_ready(app: AppHandle) -> bool {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let held = state.keys.lock().ok().map(|keys| keys.public_key());
    ready_with(&state.token_auth, &origin, held, &KeyringRefreshStore)
}

fn ready_with(
    auth: &TokenAuthState,
    origin: &str,
    held: Option<PublicKey>,
    store: &dyn RefreshStore,
) -> bool {
    let Some(held) = held else {
        return false;
    };
    let bound = match auth.signing_pubkey(origin) {
        Some(bound) => bound,
        None => match store.signing_pubkey(origin) {
            Ok(Some(bound)) => bound,
            _ => return false,
        },
    };
    if bound != held || auth.device_signed_out_with(origin, store) {
        return false;
    }
    match auth.get(origin) {
        // Ended this run: the reload that follows must not draw it again.
        Some((_, OriginAuth::NeedsLogin(_))) => false,
        Some((_, OriginAuth::Active(_) | OriginAuth::Restoring)) => true,
        None | Some((_, OriginAuth::SignedOut)) => matches!(store.load(origin), Ok(Some(_))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Store {
        token: Mutex<Option<String>>,
        bound: Option<PublicKey>,
    }

    impl RefreshStore for Store {
        fn load(&self, _origin: &str) -> Result<Option<String>, String> {
            Ok(self.token.lock().unwrap().clone())
        }
        fn store(&self, _origin: &str, token: &str) -> Result<(), String> {
            *self.token.lock().unwrap() = Some(token.into());
            Ok(())
        }
        fn delete(&self, _origin: &str) -> Result<(), String> {
            *self.token.lock().unwrap() = None;
            Ok(())
        }
        fn signing_pubkey(&self, _origin: &str) -> Result<Option<PublicKey>, String> {
            Ok(self.bound)
        }
    }

    fn key() -> PublicKey {
        nostr::Keys::generate().public_key()
    }

    #[test]
    fn a_saved_session_of_the_held_key_draws_at_once() {
        let held = key();
        let store = Store {
            token: Mutex::new(Some("r".into())),
            bound: Some(held),
        };
        let auth = TokenAuthState::default();
        assert!(ready_with(&auth, "o", Some(held), &store));
        // Restoring in the background, or already active (a reload): still.
        auth.set("o", OriginAuth::Restoring);
        assert!(ready_with(&auth, "o", Some(held), &store));
    }

    #[test]
    fn another_key_no_session_or_an_ended_session_waits() {
        let held = key();
        let auth = TokenAuthState::default();
        let other = Store {
            token: Mutex::new(Some("r".into())),
            bound: Some(key()),
        };
        assert!(
            !ready_with(&auth, "o", Some(held), &other),
            "another account"
        );
        let legacy = Store {
            token: Mutex::new(Some("r".into())),
            bound: None,
        };
        assert!(
            !ready_with(&auth, "o", Some(held), &legacy),
            "token account"
        );
        let none = Store {
            token: Mutex::new(None),
            bound: Some(held),
        };
        assert!(
            !ready_with(&auth, "o", Some(held), &none),
            "no saved session"
        );
        let saved = Store {
            token: Mutex::new(Some("r".into())),
            bound: Some(held),
        };
        assert!(!ready_with(&auth, "o", None, &saved), "no held key");
        auth.set("o", OriginAuth::NeedsLogin("refresh_reused".into()));
        assert!(
            !ready_with(&auth, "o", Some(held), &saved),
            "ended this run"
        );
    }
}
