//! One-time import of data written before the product rename.
//!
//! The rename changed the app identifier (`xyz.block.…` → `app.kuzz.…`),
//! the keyring service and the agent config folder, so a fresh launch of the
//! renamed app would find none of its old data. Before any window or secret
//! store opens, copy each old location to its new one when the new one does
//! not exist yet. Old data is never moved or deleted, so an older build keeps
//! working and a failed copy can simply be retried on the next launch.

use std::fs;
use std::path::{Path, PathBuf};

/// The identifier this app used before the rename, if it changed.
pub(crate) fn legacy_identifier(identifier: &str) -> Option<String> {
    identifier
        .strip_prefix("app.kuzz")
        .map(|rest| format!("xyz.block.buzz{rest}")) // kuzz-rename: keep
}

/// The keyring service this app used before the rename, if it changed.
pub(crate) fn legacy_keyring_service(service: &str) -> Option<String> {
    service
        .strip_prefix("kuzz-")
        .map(|rest| format!("buzz-{rest}")) // kuzz-rename: keep
}

/// The agent config folder name this app used before the rename.
fn legacy_folder_name(name: &str) -> Option<String> {
    name.strip_prefix("kuzz-")
        .map(|rest| format!("buzz-{rest}")) // kuzz-rename: keep
}

/// Copy `legacy` to `current` when only the legacy folder exists. The copy
/// goes to a sibling temp folder first and is renamed into place, so a crash
/// mid-copy never leaves a half-filled `current` that would block a retry.
fn import_dir(legacy: &Path, current: &Path) -> Result<bool, String> {
    if current.exists() || !legacy.is_dir() {
        return Ok(false);
    }
    let mut staging_name = current.as_os_str().to_owned();
    staging_name.push(".pre-rename-import");
    let staging = PathBuf::from(staging_name);
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|e| format!("clear {}: {e}", staging.display()))?;
    }
    copy_tree(legacy, &staging)?;
    fs::rename(&staging, current)
        .map_err(|e| format!("rename {} -> {}: {e}", staging.display(), current.display()))?;
    Ok(true)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("read {}: {e}", from.display()))?;
        let kind = entry
            .file_type()
            .map_err(|e| format!("stat {}: {e}", entry.path().display()))?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)
                .map_err(|e| format!("copy {}: {e}", entry.path().display()))?;
        }
        // Symlinks (dev worktree sharing) are not carried over.
    }
    Ok(())
}

/// Folders Tauri derives from the app identifier on this platform.
fn identifier_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = [dirs::data_dir(), dirs::data_local_dir(), dirs::config_dir()]
        .into_iter()
        .flatten()
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// Run every import. Failures are logged and never block startup.
pub(crate) fn import_pre_rename_data(identifier: &str) {
    if let Some(legacy) = legacy_identifier(identifier) {
        for root in identifier_roots() {
            match import_dir(&root.join(&legacy), &root.join(identifier)) {
                Ok(true) => eprintln!("kuzz-desktop: imported {legacy} data in {}", root.display()),
                Ok(false) => {}
                Err(error) => eprintln!("kuzz-desktop: pre-rename data import failed: {error}"),
            }
        }
    }

    if let Ok(Some(config_home)) = crate::build_identity::demo_config_home() {
        let legacy = config_home
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(legacy_folder_name)
            .map(|name| config_home.with_file_name(name));
        if let Some(legacy) = legacy {
            if let Err(error) = import_dir(&legacy, &config_home) {
                eprintln!("kuzz-desktop: pre-rename agent config import failed: {error}");
            }
        }
    }

    let service = crate::app_state::keyring_service();
    if let Some(legacy) = legacy_keyring_service(service) {
        match crate::secret_store::SecretStore::shared(service).import_from_legacy_service(&legacy) {
            Ok(0) => {}
            Ok(count) => eprintln!("kuzz-desktop: imported {count} secure-storage entries"),
            Err(error) => eprintln!("kuzz-desktop: pre-rename secure-storage import failed: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_names() {
        assert_eq!(
            legacy_identifier("app.kuzz.app.demo.preview-1").as_deref(),
            Some("xyz.block.buzz.app.demo.preview-1") // kuzz-rename: keep
        );
        assert_eq!(legacy_identifier("com.example"), None);
        assert_eq!(
            legacy_keyring_service("kuzz-desktop-demo.preview-1").as_deref(),
            Some("buzz-desktop-demo.preview-1") // kuzz-rename: keep
        );
        assert_eq!(
            legacy_folder_name("kuzz-demo-preview-1").as_deref(),
            Some("buzz-demo-preview-1") // kuzz-rename: keep
        );
    }

    #[test]
    fn imports_only_when_current_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("old");
        let current = dir.path().join("new");
        fs::create_dir_all(legacy.join("agents")).unwrap();
        fs::write(legacy.join("agents/managed-agents.json"), "[]").unwrap();
        fs::write(legacy.join("accounts.json"), "{}").unwrap();

        assert!(import_dir(&legacy, &current).unwrap());
        assert_eq!(fs::read_to_string(current.join("accounts.json")).unwrap(), "{}");
        assert!(current.join("agents/managed-agents.json").is_file());
        assert!(legacy.join("accounts.json").is_file(), "legacy data stays");

        fs::write(current.join("accounts.json"), "{\"new\":1}").unwrap();
        assert!(!import_dir(&legacy, &current).unwrap());
        assert_eq!(
            fs::read_to_string(current.join("accounts.json")).unwrap(),
            "{\"new\":1}"
        );
    }
}
