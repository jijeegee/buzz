//! Immutable custodial bindings. Account ids authorize storage, never messages.
use buzz_core::principal::PrincipalId;
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
        row.map(|row| {
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
        })
        .transpose()
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
        if backup.version != 1
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
