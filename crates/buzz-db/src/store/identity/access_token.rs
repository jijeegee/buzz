//! Access-token lookup (the token → principal seam) and account disabling.

use buzz_datastore_tracing::datastore_span;
use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use buzz_core::principal::{AccessTokenKind, PrincipalId};

use super::{begin, hashes_from_rows, principal_from_bytes, token_kind_from_str, RevokeReason};
use crate::error::Result;

/// Everything needed to decide whether an access token authenticates, read
/// in one query.
#[derive(Debug, Clone)]
pub struct AccessTokenRecord {
    /// SHA-256 of the token.
    pub token_hash: [u8; 32],
    /// Authenticated principal.
    pub principal: PrincipalId,
    /// Token kind.
    pub kind: AccessTokenKind,
    /// Token expiry (`None` for headless bot tokens).
    pub expires_at: Option<DateTime<Utc>>,
    /// Set once the token was replaced by an exchange (grace period).
    pub superseded_at: Option<DateTime<Utc>>,
    /// Token revocation time.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Principal disable time.
    pub principal_disabled_at: Option<DateTime<Utc>>,
    /// Session of a user token.
    pub session_id: Option<Uuid>,
    /// Session revocation time.
    pub session_revoked_at: Option<DateTime<Utc>>,
    /// Device of a user token (via its session) or issuing device of a bot token.
    pub device_id: Option<Uuid>,
    /// Bot id of a bot token.
    pub bot_id: Option<PrincipalId>,
    /// Owner of the bot.
    pub bot_owner: Option<PrincipalId>,
    /// Bot hosting device (`None` ⇒ headless bot).
    pub bot_host_device_id: Option<Uuid>,
    /// Bot deletion time.
    pub bot_deleted_at: Option<DateTime<Utc>>,
    /// Whether the principal is an `operator` in the DB roster.
    pub is_operator: bool,
}

/// Why a stored access token does not authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTokenRejection {
    /// Expired (including an elapsed exchange grace period).
    Expired,
    /// Revoked, session revoked, or bot deleted.
    Revoked,
    /// The principal is disabled.
    PrincipalDisabled,
    /// A headless token presented for a desktop-hosted bot (plan §6.2).
    InvalidForBot,
}

impl AccessTokenRecord {
    /// Decide whether this token authenticates at `now`.
    pub fn check(&self, now: DateTime<Utc>) -> std::result::Result<(), AccessTokenRejection> {
        if self.principal_disabled_at.is_some() {
            return Err(AccessTokenRejection::PrincipalDisabled);
        }
        if self.revoked_at.is_some()
            || self.session_revoked_at.is_some()
            || self.bot_deleted_at.is_some()
        {
            return Err(AccessTokenRejection::Revoked);
        }
        if self.expires_at.is_some_and(|exp| exp <= now) {
            return Err(AccessTokenRejection::Expired);
        }
        if self.kind == AccessTokenKind::BotHeadless && self.bot_host_device_id.is_some() {
            return Err(AccessTokenRejection::InvalidForBot);
        }
        Ok(())
    }
}

pub(super) async fn lookup_access_token(
    pool: &PgPool,
    token_hash: &[u8; 32],
) -> Result<Option<AccessTokenRecord>> {
    let mut tx = begin(pool).await?;
    let row = sqlx::query(
        "SELECT t.principal_id, t.kind, t.expires_at, t.superseded_at, t.revoked_at, \
                t.session_id, t.bot_id, t.issued_by_device, \
                p.disabled_at AS principal_disabled_at, \
                s.revoked_at AS session_revoked_at, s.device_id AS session_device_id, \
                b.owner_principal_id, b.host_device_id, b.deleted_at AS bot_deleted_at, \
                EXISTS (SELECT 1 FROM relay_operators o \
                        WHERE o.pubkey = t.principal_id AND o.role = 'operator') AS is_operator \
         FROM access_tokens t \
         JOIN principals p ON p.id = t.principal_id \
         LEFT JOIN sessions s ON s.id = t.session_id \
         LEFT JOIN bots b ON b.id = t.bot_id \
         WHERE t.token_hash = $1",
    )
    .bind(token_hash.as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let principal: Vec<u8> = row.try_get("principal_id")?;
    let kind: String = row.try_get("kind")?;
    let bot_id: Option<Vec<u8>> = row.try_get("bot_id")?;
    let bot_owner: Option<Vec<u8>> = row.try_get("owner_principal_id")?;
    let session_device: Option<Uuid> = row.try_get("session_device_id")?;
    let issued_by_device: Option<Uuid> = row.try_get("issued_by_device")?;
    Ok(Some(AccessTokenRecord {
        token_hash: *token_hash,
        principal: principal_from_bytes(&principal)?,
        kind: token_kind_from_str(&kind)?,
        expires_at: row.try_get("expires_at")?,
        superseded_at: row.try_get("superseded_at")?,
        revoked_at: row.try_get("revoked_at")?,
        principal_disabled_at: row.try_get("principal_disabled_at")?,
        session_id: row.try_get("session_id")?,
        session_revoked_at: row.try_get("session_revoked_at")?,
        device_id: session_device.or(issued_by_device),
        bot_id: bot_id.as_deref().map(principal_from_bytes).transpose()?,
        bot_owner: bot_owner.as_deref().map(principal_from_bytes).transpose()?,
        bot_host_device_id: row.try_get("host_device_id")?,
        bot_deleted_at: row.try_get("bot_deleted_at")?,
        is_operator: row.try_get("is_operator")?,
    }))
}

/// Disable `principal` (self-deletion schedules a purge; an operator disable
/// does not) and revoke every session, user token and owned-bot token.
pub(super) async fn disable_principal(
    pool: &PgPool,
    principal: &PrincipalId,
    purge_after: Option<Duration>,
) -> Result<Vec<[u8; 32]>> {
    let mut tx = begin(pool).await?;
    sqlx::query(
        "UPDATE principals SET disabled_at = now(), \
                purge_after = CASE WHEN $2::double precision IS NULL THEN NULL \
                                   ELSE now() + make_interval(secs => $2) END \
         WHERE id = $1",
    )
    .bind(principal.as_bytes().as_slice())
    .bind(purge_after.map(|d| d.num_seconds() as f64))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = $2 \
         WHERE revoked_at IS NULL AND device_id IN (SELECT id FROM devices WHERE principal_id = $1)",
    )
    .bind(principal.as_bytes().as_slice())
    .bind(RevokeReason::AccountDisabled.as_str())
    .execute(&mut *tx)
    .await?;
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = $2 \
         WHERE revoked_at IS NULL AND (principal_id = $1 OR bot_id IN \
           (SELECT id FROM bots WHERE owner_principal_id = $1)) RETURNING token_hash",
    )
    .bind(principal.as_bytes().as_slice())
    .bind(RevokeReason::AccountDisabled.as_str())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    hashes_from_rows(rows)
}

impl crate::Db {
    /// Look up an access token by hash with everything its validity depends on.
    #[datastore_span(name = "lookup_access_token", system = "postgresql")]
    pub async fn lookup_access_token(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<AccessTokenRecord>> {
        lookup_access_token(&self.pool, token_hash).await
    }

    /// Disable a principal and revoke all of its (and its bots') tokens.
    /// `purge_after` schedules self-deletion; `None` is an operator disable.
    #[datastore_span(name = "disable_principal", system = "postgresql")]
    pub async fn disable_principal(
        &self,
        principal: &PrincipalId,
        purge_after: Option<Duration>,
    ) -> Result<Vec<[u8; 32]>> {
        disable_principal(&self.pool, principal, purge_after).await
    }
}
