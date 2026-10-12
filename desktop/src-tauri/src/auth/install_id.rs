//! This app install's stable device id, sent with every Google sign-in so the
//! relay reuses the same device (its name, robot and hosted-agent records)
//! when this install signs in again instead of listing a new one.
//!
//! A random UUID kept in the app data dir: one per install, so two Buzz apps
//! on one computer are two devices, and no hardware identifier is read.

use std::path::Path;

use tauri::Manager;

const FILE_NAME: &str = "install-id";

/// The id stored in `dir`, created on first use. `None` when it cannot be
/// read or written; sign-in then proceeds without one (a new device).
pub(crate) fn load_or_create_in(dir: &Path) -> Option<String> {
    let path = dir.join(FILE_NAME);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if let Ok(id) = uuid::Uuid::parse_str(existing.trim()) {
            return Some(id.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&path, &id).ok()?;
    Some(id)
}

/// [`load_or_create_in`] for this app's data dir.
pub(crate) fn load_or_create<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<String> {
    let dir = app.path().app_data_dir().ok()?;
    load_or_create_in(&dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_id_is_created_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_in(dir.path()).unwrap();
        assert!(uuid::Uuid::parse_str(&first).is_ok());
        assert_eq!(
            load_or_create_in(dir.path()).as_deref(),
            Some(first.as_str())
        );
    }

    #[test]
    fn unreadable_install_id_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "garbage").unwrap();
        let id = load_or_create_in(dir.path()).unwrap();
        assert!(uuid::Uuid::parse_str(&id).is_ok());
        assert_eq!(load_or_create_in(dir.path()), Some(id));
    }
}
