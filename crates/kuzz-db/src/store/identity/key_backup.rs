//! Immutable custodial bindings. Account ids authorize storage, never messages.
use kuzz_core::principal::PrincipalId;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use super::{begin, lock_subject, LoginPrincipal};
use crate::error::Result;
use crate::{Db, DbError};

/// Ciphertext-only database representation of a signing-key backup.
#[derive(Clone)]
pub struct KeyBackupRecord {
    /// The immutable Nostr public key, unique across backup accounts.
    pub pubkey: [u8; 32],
    /// AEAD/AAD envelope version.
    pub version: u16,
    /// Nonsecret master-key label.
    pub key_id: String,
    /// Random 96-bit nonce.
    pub nonce: Vec<u8>,
    /// 32 encrypted secret bytes plus 16-byte authentication tag.
    pub ciphertext: Vec<u8>,
}

/// Envelope version new backups must carry (kuzz AAD domain).
pub const KEY_BACKUP_VERSION: u16 = 2;
/// Pre-rename envelope version, rewritten by `kuzz-admin reencrypt-key-backups`.
pub const LEGACY_KEY_BACKUP_VERSION: u16 = 1;

/// Counts reported by [`Db::reencrypt_legacy_key_backups`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyBackupReencryption {
    /// Version 1 rows rewritten as the current version.
    pub rewritten: u64,
    /// Rows already at the current version, left untouched.
    pub already_current: u64,
}

/// Atomic initialization result; no outcome overwrites an existing binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializeKeyBackup {
    /// New binding and ciphertext committed together.
    Created,
    /// This account already has a binding; restore instead.
    BackupExists,
    /// Another account already owns this public-key binding.
    PubkeyInUse,
    /// Login is too old or has been revoked; repeat OIDC.
    SessionNotFresh,
    /// Account is missing, disabled or belongs to the old token mode.
    AccountConflict,
}

async fn session_fresh(
    tx: &mut Transaction<'_, Postgres>,
    account: &PrincipalId,
    session: Uuid,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sessions s \
         JOIN devices d ON d.id = s.device_id JOIN principals p ON p.id = d.principal_id \
         WHERE s.id = $1 AND d.principal_id = $2 AND p.identity_mode = 'key_backup' \
           AND p.disabled_at IS NULL AND s.revoked_at IS NULL AND d.revoked_at IS NULL \
           AND s.created_at >= now() - interval '5 minutes' AND s.created_at <= now())",
    )
    .bind(session)
    .bind(account.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await?)
}

impl Db {
    /// Resolve a Google subject for backup custody, refusing token-account merges.
    pub async fn login_key_identity(
        &self,
        provider: &str,
        subject: &str,
        email: Option<&str>,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<LoginPrincipal> {
        super::principal::login_identity_mode(
            &self.pool,
            provider,
            subject,
            email,
            display_name,
            avatar_url,
            super::principal::IdentityLoginMode::KeyBackup,
        )
        .await
    }

    /// Sign in to an existing key-backup account without ever creating one:
    /// an unknown identity fails with `AccessDenied("signup_required")`.
    pub async fn login_existing_key_identity(
        &self,
        provider: &str,
        subject: &str,
        email: Option<&str>,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<LoginPrincipal> {
        super::principal::login_identity_mode(
            &self.pool,
            provider,
            subject,
            email,
            display_name,
            avatar_url,
            super::principal::IdentityLoginMode::ExistingKeyBackup,
        )
        .await
    }

    /// Preserve existing token-account logins without creating a new unrelated
    /// messaging principal on a custody-enabled deployment.
    pub async fn login_existing_token_identity(
        &self,
        provider: &str,
        subject: &str,
        email: Option<&str>,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<LoginPrincipal> {
        super::principal::login_identity_mode(
            &self.pool,
            provider,
            subject,
            email,
            display_name,
            avatar_url,
            super::principal::IdentityLoginMode::ExistingToken,
        )
        .await
    }

    /// Whether this account is reserved for custodial backup sessions.
    pub async fn account_key_mode(&self, account: &PrincipalId) -> Result<bool> {
        let mut tx = begin(&self.pool).await?;
        let mode: Option<String> =
            sqlx::query_scalar("SELECT identity_mode FROM principals WHERE id = $1")
                .bind(account.as_bytes().as_slice())
                .fetch_optional(&mut *tx)
                .await?;
        tx.commit().await?;
        match mode.as_deref() {
            Some("key_backup") => Ok(true),
            Some("token") => Ok(false),
            _ => Err(DbError::AccessDenied("unknown account mode".into())),
        }
    }

    /// Check OIDC login recency using database time, never token-refresh time.
    pub async fn key_backup_session_fresh(
        &self,
        account: &PrincipalId,
        session: Uuid,
    ) -> Result<bool> {
        let mut tx = begin(&self.pool).await?;
        let fresh = session_fresh(&mut tx, account, session).await?;
        tx.commit().await?;
        Ok(fresh)
    }

    /// Read exactly this account's immutable backup. Absence and DB failure differ.
    pub async fn get_key_backup(&self, account: &PrincipalId) -> Result<Option<KeyBackupRecord>> {
        let mut tx = begin(&self.pool).await?;
        let row = sqlx::query(
            "SELECT pubkey, version, key_id, nonce, ciphertext FROM account_key_backups WHERE account_id = $1"
        ).bind(account.as_bytes().as_slice()).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        row.as_ref().map(decode_record).transpose()
    }

    /// Rewrite every version 1 backup through `reencrypt` in one transaction.
    ///
    /// `reencrypt` receives the account id and the stored envelope and must
    /// return the replacement envelope; it may not change the public key or
    /// master-key label. Rows already at [`KEY_BACKUP_VERSION`] are skipped, so
    /// running this again is a no-op. Any failure rolls back every row.
    pub async fn reencrypt_legacy_key_backups<F>(
        &self,
        mut reencrypt: F,
    ) -> Result<KeyBackupReencryption>
    where
        F: FnMut(&[u8; 32], &KeyBackupRecord) -> std::result::Result<KeyBackupRecord, String>,
    {
        let mut tx = begin(&self.pool).await?;
        let rows = sqlx::query(
            "SELECT account_id, pubkey, version, key_id, nonce, ciphertext \
             FROM account_key_backups WHERE version = $1 ORDER BY account_id FOR UPDATE",
        )
        .bind(LEGACY_KEY_BACKUP_VERSION as i16)
        .fetch_all(&mut *tx)
        .await?;
        let already_current: i64 =
            sqlx::query_scalar("SELECT count(*) FROM account_key_backups WHERE version = $1")
                .bind(KEY_BACKUP_VERSION as i16)
                .fetch_one(&mut *tx)
                .await?;
        let mut rewritten = 0u64;
        for row in &rows {
            let account_bytes: Vec<u8> = row.try_get("account_id")?;
            let account: [u8; 32] = account_bytes
                .as_slice()
                .try_into()
                .map_err(|_| DbError::InvalidData("invalid backup account id".into()))?;
            let current = decode_record(row)?;
            let replacement = reencrypt(&account, &current).map_err(|err| {
                DbError::InvalidData(format!(
                    "key backup for account {} could not be re-encrypted: {err}",
                    hex::encode(account)
                ))
            })?;
            if replacement.version != KEY_BACKUP_VERSION
                || replacement.pubkey != current.pubkey
                || replacement.key_id != current.key_id
                || replacement.nonce.len() != 12
                || replacement.ciphertext.len() != 48
            {
                return Err(DbError::InvalidData(format!(
                    "re-encrypted key backup for account {} is not a valid version {KEY_BACKUP_VERSION} envelope",
                    hex::encode(account)
                )));
            }
            let updated = sqlx::query(
                "UPDATE account_key_backups SET version = $2, nonce = $3, ciphertext = $4 \
                 WHERE account_id = $1 AND version = $5",
            )
            .bind(account.as_slice())
            .bind(replacement.version as i16)
            .bind(&replacement.nonce)
            .bind(&replacement.ciphertext)
            .bind(LEGACY_KEY_BACKUP_VERSION as i16)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if updated != 1 {
                return Err(DbError::InvalidData(format!(
                    "key backup for account {} changed during re-encryption",
                    hex::encode(account)
                )));
            }
            rewritten += 1;
        }
        tx.commit().await?;
        Ok(KeyBackupReencryption {
            rewritten,
            already_current: u64::try_from(already_current).unwrap_or_default(),
        })
    }

    /// Commit one new immutable binding. Account locks and unique constraints
    /// settle concurrent devices without replacement or cross-account merging.
    /// Session recency/revocation is rechecked inside the same transaction.
    pub async fn initialize_key_backup(
        &self,
        account: &PrincipalId,
        session: Uuid,
        backup: &KeyBackupRecord,
    ) -> Result<InitializeKeyBackup> {
        if backup.version != KEY_BACKUP_VERSION
            || backup.nonce.len() != 12
            || backup.ciphertext.len() != 48
            || backup.key_id.is_empty()
            || backup.key_id.len() > 64
            || !backup
                .key_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || nostr::PublicKey::from_slice(&backup.pubkey)
                .and_then(|p| p.xonly())
                .is_err()
        {
            return Err(DbError::InvalidData("invalid key backup envelope".into()));
        }
        let mut tx = begin(&self.pool).await?;
        lock_subject(&mut tx, account).await?;
        let eligible: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM principals WHERE id = $1 AND kind = 'user' \
             AND identity_mode = 'key_backup' AND disabled_at IS NULL)",
        )
        .bind(account.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await?;
        if !eligible {
            return Ok(InitializeKeyBackup::AccountConflict);
        }
        if !session_fresh(&mut tx, account, session).await? {
            return Ok(InitializeKeyBackup::SessionNotFresh);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM account_key_backups WHERE account_id = $1)",
        )
        .bind(account.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            return Ok(InitializeKeyBackup::BackupExists);
        }
        // A purged account's key stays refused, even with its backup gone.
        let purged: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM purged_account_keys WHERE pubkey = $1)",
        )
        .bind(backup.pubkey.as_slice())
        .fetch_one(&mut *tx)
        .await?;
        if purged {
            return Ok(InitializeKeyBackup::PubkeyInUse);
        }
        let inserted: Option<Vec<u8>> = sqlx::query_scalar(
            "INSERT INTO account_key_backups (account_id, pubkey, version, key_id, nonce, ciphertext) \
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING RETURNING account_id"
        ).bind(account.as_bytes().as_slice()).bind(backup.pubkey.as_slice()).bind(backup.version as i16)
            .bind(&backup.key_id).bind(&backup.nonce).bind(&backup.ciphertext).fetch_optional(&mut *tx).await?;
        tx.commit().await?;
        Ok(if inserted.is_some() {
            InitializeKeyBackup::Created
        } else {
            InitializeKeyBackup::PubkeyInUse
        })
    }
}

fn decode_record(row: &sqlx::postgres::PgRow) -> Result<KeyBackupRecord> {
    let bytes: Vec<u8> = row.try_get("pubkey")?;
    let version: i16 = row.try_get("version")?;
    Ok(KeyBackupRecord {
        pubkey: bytes
            .try_into()
            .map_err(|_| DbError::InvalidData("invalid backup pubkey".into()))?,
        version: u16::try_from(version)
            .map_err(|_| DbError::InvalidData("invalid backup version".into()))?,
        key_id: row.try_get("key_id")?,
        nonce: row.try_get("nonce")?,
        ciphertext: row.try_get("ciphertext")?,
    })
}
