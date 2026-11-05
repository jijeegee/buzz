//! Where each account keeps its agent settings on this computer.
//!
//! Harness, model, provider and API keys belong to an account, not to the
//! computer: every account's agents keep running whichever account is active,
//! so an agent reads its owner's settings, and the screens read the active
//! account's. They live in `<app-data>/agents/accounts/<owner-hex>/`, one
//! folder per account, so a save cannot reach another account's values. Sign
//! out keeps the folder: signing back in finds the account already set up.

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use super::managed_agents_base_dir;

const ACCOUNTS_DIR: &str = "accounts";
const AGENT_SETUP_PENDING_FILE: &str = "agent-setup-pending";

/// An account's settings folder under `base`, for a 64-character hex owner.
fn settings_dir_in(base: &Path, owner_hex: &str) -> Result<PathBuf, String> {
    if owner_hex.len() != 64 || !owner_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid account".to_string());
    }
    let dir = base.join(ACCOUNTS_DIR).join(owner_hex.to_ascii_lowercase());
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// `owner_hex`'s settings folder, created if missing.
pub(crate) fn account_settings_dir<R: tauri::Runtime>(
    app: &AppHandle<R>,
    owner_hex: &str,
) -> Result<PathBuf, String> {
    settings_dir_in(&managed_agents_base_dir(app)?, owner_hex)
}

/// The account whose settings the screens read and save. `Err` while a token
/// account cannot be used yet: the fallback key is not that account, and its
/// folder must not take the account's values.
pub(crate) fn active_settings_owner<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<String, String> {
    use tauri::Manager;
    let state = app.state::<crate::app_state::AppState>();
    if let crate::auth::CredentialMode::Blocked(reason) = state.current_credential_mode() {
        return Err(reason);
    }
    state.current_identity_pubkey().map(|key| key.to_hex())
}

fn require_owner(active: Result<String, String>, owner: &str) -> Result<(), String> {
    if active?.eq_ignore_ascii_case(owner) {
        Ok(())
    } else {
        Err("The account changed. Reopen this screen and try again.".to_string())
    }
}

/// A save from a screen names the account that screen showed. It is refused
/// once another account is active: the values were chosen for `owner`, and the
/// write goes to `owner`'s folder whatever happens after this check.
pub(crate) fn require_settings_owner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    owner: &str,
) -> Result<(), String> {
    require_owner(active_settings_owner(app), owner)
}

/// A sign-in on this computer: the next start checks whether this account's
/// agents are set up here. Written before the sign-in can reload the window.
pub(crate) fn mark_agent_setup_pending<R: tauri::Runtime>(
    app: &AppHandle<R>,
    owner_hex: &str,
) -> Result<(), String> {
    let path = account_settings_dir(app, owner_hex)?.join(AGENT_SETUP_PENDING_FILE);
    std::fs::write(&path, b"").map_err(|e| format!("failed to write {}: {e}", path.display()))
}

pub(crate) fn agent_setup_pending<R: tauri::Runtime>(
    app: &AppHandle<R>,
    owner_hex: &str,
) -> Result<bool, String> {
    Ok(account_settings_dir(app, owner_hex)?
        .join(AGENT_SETUP_PENDING_FILE)
        .exists())
}

pub(crate) fn clear_agent_setup_pending<R: tauri::Runtime>(
    app: &AppHandle<R>,
    owner_hex: &str,
) -> Result<(), String> {
    let path = account_settings_dir(app, owner_hex)?.join(AGENT_SETUP_PENDING_FILE);
    match std::fs::remove_file(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("failed to remove {}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "AB00000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn a_save_is_refused_unless_its_account_is_active() {
        let lower = OWNER.to_ascii_lowercase();
        assert!(require_owner(Ok(lower), OWNER).is_ok());
        assert!(require_owner(Ok("b".repeat(64)), OWNER).is_err());
        assert!(require_owner(Err("blocked".to_string()), OWNER).is_err());
    }

    #[test]
    fn each_owner_gets_one_lowercase_folder() {
        let base = tempfile::tempdir().unwrap();
        let upper = settings_dir_in(base.path(), OWNER).unwrap();
        let lower = settings_dir_in(base.path(), &OWNER.to_ascii_lowercase()).unwrap();
        assert_eq!(upper, lower);
        assert!(upper.is_dir());
    }

    #[test]
    fn anything_but_a_hex_key_cannot_name_a_folder() {
        let base = tempfile::tempdir().unwrap();
        for owner in ["", "..", "../../etc", &"g".repeat(64), &"a".repeat(63)] {
            assert!(settings_dir_in(base.path(), owner).is_err(), "{owner}");
        }
    }
}
