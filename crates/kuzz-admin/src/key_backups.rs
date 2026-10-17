//! One-shot rewrite of custodial key backups stored before the protocol rename.

use anyhow::{anyhow, Result};
use kuzz_auth::key_backup::{BackupMasterKey, EncryptedKeyBackup};
use kuzz_db::identity::KeyBackupRecord;
use serde::Serialize;

#[derive(Serialize)]
struct Report {
    rewritten: u64,
    already_current: u64,
}

/// `reencrypt-key-backups`: decrypt every version 1 row with the pre-rename
/// AAD and store it again as the current version, all in one transaction.
/// Rows already at the current version are skipped, so re-running is safe.
pub(crate) async fn run() -> Result<i32> {
    let master = master_key_from_env(|name| std::env::var(name).ok())?;
    let db = crate::connect_db().await?;
    let report = db
        .reencrypt_legacy_key_backups(|account, record| reencrypt(&master, account, record))
        .await?;
    println!(
        "{}",
        serde_json::to_string(&Report {
            rewritten: report.rewritten,
            already_current: report.already_current,
        })?
    );
    Ok(0)
}

/// The relay's own master-key settings, so the command uses the same key.
fn master_key_from_env(lookup: impl Fn(&str) -> Option<String>) -> Result<BackupMasterKey> {
    let raw = lookup("AUTH_KEY_BACKUP_MASTER_KEY")
        .ok_or_else(|| anyhow!("AUTH_KEY_BACKUP_MASTER_KEY is not set"))?;
    let key_id = lookup("AUTH_KEY_BACKUP_KEY_ID")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("AUTH_KEY_BACKUP_KEY_ID is not set"))?;
    BackupMasterKey::from_hex(key_id.trim(), raw.trim())
        .map_err(|_| anyhow!("invalid key backup master key or key id"))
}

fn reencrypt(
    master: &BackupMasterKey,
    account: &[u8; 32],
    record: &KeyBackupRecord,
) -> std::result::Result<KeyBackupRecord, String> {
    let envelope = EncryptedKeyBackup {
        pubkey: record.pubkey,
        version: record.version,
        key_id: record.key_id.clone(),
        nonce: record.nonce.clone(),
        ciphertext: record.ciphertext.clone(),
    };
    let current = master
        .reencrypt_legacy_v1(account, &envelope)
        .map_err(|err| err.to_string())?;
    Ok(KeyBackupRecord {
        pubkey: current.pubkey,
        version: current.version,
        key_id: current.key_id,
        nonce: current.nonce,
        ciphertext: current.ciphertext,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_key_requires_both_relay_settings() {
        let key = "19".repeat(32);
        assert!(master_key_from_env(|_| None).is_err());
        assert!(master_key_from_env(|name| {
            (name == "AUTH_KEY_BACKUP_MASTER_KEY").then(|| key.clone())
        })
        .is_err());
        assert!(master_key_from_env(|name| match name {
            "AUTH_KEY_BACKUP_MASTER_KEY" => Some(key.clone()),
            "AUTH_KEY_BACKUP_KEY_ID" => Some("prod-v1".into()),
            _ => None,
        })
        .is_ok());
    }

    #[test]
    fn current_rows_are_refused_rather_than_rewritten_twice() {
        let master = BackupMasterKey::from_hex("prod-v1", &"19".repeat(32)).unwrap();
        let keys = nostr::Keys::generate();
        let account = [7; 32];
        let current = master
            .encrypt(&account, &keys.secret_key().to_secret_hex())
            .unwrap();
        let record = KeyBackupRecord {
            pubkey: current.pubkey,
            version: current.version,
            key_id: current.key_id,
            nonce: current.nonce,
            ciphertext: current.ciphertext,
        };
        assert!(reencrypt(&master, &account, &record).is_err());
    }
}
