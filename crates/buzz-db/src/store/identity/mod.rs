//! Centralized-identity persistence: principals, external identities, devices,
//! sessions, refresh-token rotation, bots and opaque access tokens.
//!
//! Backs migrations 0056–0058. Every user action that touches more than one
//! row is a single transaction (Rule 5): login completion, refresh rotation,
//! bot creation, token exchange, device revocation and profile updates. Every
//! revocation returns the hashes it revoked so the caller can publish the
//! cross-instance close *after* commit.
//!
//! Tokens are only ever handled as SHA-256 hashes here.

mod access_token;
mod bot;
mod principal;
mod session;

#[cfg(test)]
mod postgres_tests;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use buzz_core::principal::{AccessTokenKind, PrincipalId, PrincipalKind};

use crate::error::{DbError, Result};

pub use access_token::{AccessTokenRecord, AccessTokenRejection};
pub use bot::{BotRecord, ExchangeOutcome};
pub use principal::{LoginPrincipal, PrincipalRecord, ProfileUpdate};
pub use session::{DeviceRecord, LoginSession, RefreshOutcome};

/// Maximum live (non-revoked) devices per principal.
pub const MAX_DEVICES_PER_PRINCIPAL: i64 = 50;
/// Maximum live (non-deleted) bots per owner.
pub const MAX_BOTS_PER_OWNER: i64 = 100;
/// Refresh-token rotation rows kept per session.
pub const REFRESH_GENERATIONS_KEPT: i32 = 100;

/// A newly issued token's hash and expiry, as handed to the store.
#[derive(Debug, Clone, Copy)]
pub struct IssuedToken {
    /// SHA-256 of the token plaintext.
    pub hash: [u8; 32],
    /// Expiry; `None` only for headless bot tokens.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Reason recorded on revoked sessions and tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeReason {
    /// Replaced by an exchange (grace expired).
    Exchanged,
    /// Desktop re-issued the bot token.
    Reissued,
    /// The hosted bot was stopped.
    Stopped,
    /// The user logged out on this device.
    Logout,
    /// The device was remotely logged out.
    DeviceRevoked,
    /// The bot was deleted.
    BotDeleted,
    /// The account was disabled or deleted.
    AccountDisabled,
    /// "Revoke all bot tokens" / "log out other sessions".
    RevokeAll,
    /// A consumed refresh token was replayed.
    RefreshReused,
}

impl RevokeReason {
    /// Column value.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Exchanged => "exchanged",
            Self::Reissued => "reissued",
            Self::Stopped => "stopped",
            Self::Logout => "logout",
            Self::DeviceRevoked => "device_revoked",
            Self::BotDeleted => "bot_deleted",
            Self::AccountDisabled => "account_disabled",
            Self::RevokeAll => "revoke_all",
            Self::RefreshReused => "refresh_reused",
        }
    }
}

async fn begin(pool: &PgPool) -> Result<Transaction<'static, Postgres>> {
    let connection = crate::observability::acquire_writer(
        pool,
        crate::observability::WriterOperation::Authentication,
    )
    .await?;
    Ok(sqlx::Transaction::begin(connection, None).await?)
}

fn principal_from_bytes(bytes: &[u8]) -> Result<PrincipalId> {
    PrincipalId::from_slice(bytes)
        .map_err(|e| DbError::InvalidData(format!("stored principal id: {e}")))
}

fn hashes_from_rows(rows: Vec<Vec<u8>>) -> Result<Vec<[u8; 32]>> {
    rows.into_iter()
        .map(|row| {
            <[u8; 32]>::try_from(row.as_slice())
                .map_err(|_| DbError::InvalidData("stored token hash is not 32 bytes".into()))
        })
        .collect()
}

fn token_kind_from_str(value: &str) -> Result<AccessTokenKind> {
    AccessTokenKind::parse(value)
        .ok_or_else(|| DbError::InvalidData(format!("unknown access token kind {value}")))
}

fn principal_kind_from_str(value: &str) -> Result<PrincipalKind> {
    PrincipalKind::parse(value)
        .ok_or_else(|| DbError::InvalidData(format!("unknown principal kind {value}")))
}

/// Whether `error` is a unique-constraint violation (SQLSTATE 23505).
fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

/// Revoke every live access token of `session_ids`, returning their hashes.
async fn revoke_session_tokens(
    tx: &mut Transaction<'static, Postgres>,
    session_ids: &[Uuid],
    reason: RevokeReason,
) -> Result<Vec<[u8; 32]>> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = $2 \
         WHERE session_id = ANY($1) AND revoked_at IS NULL RETURNING token_hash",
    )
    .bind(session_ids)
    .bind(reason.as_str())
    .fetch_all(&mut **tx)
    .await?;
    hashes_from_rows(rows)
}
