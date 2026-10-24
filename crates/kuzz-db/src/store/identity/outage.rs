//! Access tokens a region gives out while the primary is down (global
//! regions S5-6).
//!
//! A refresh normally rotates the refresh token in the primary. When the
//! primary cannot be reached, a copy region checks the refresh token against
//! its copy and records a short access token in its own
//! `outage_access_tokens` table, writing nothing global. The token check
//! ([`super::access_token`]) reads that table after `access_tokens`.

use chrono::{DateTime, Duration, Utc};
use kuzz_datastore_tracing::datastore_span;
use sqlx::{PgPool, Row as _};

use super::{begin, principal_from_bytes, IssuedToken};
use crate::error::{DbError, Result};

/// Outcome of asking for an access token while the primary is down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutageRefreshOutcome {
    /// Recorded; the token is valid until `expires_at`.
    Issued {
        /// The token's expiry: the asked one, or the refresh token's if
        /// sooner.
        expires_at: DateTime<Utc>,
    },
    /// This region's copy does not have the refresh token (it may be newer
    /// than the copy). Nothing changed.
    Unknown,
    /// The refresh token was already used. The primary decides whether that
    /// is a retry or reuse once it is back. Nothing changed.
    Used,
    /// The refresh token expired.
    Expired,
    /// The session was revoked.
    SessionRevoked,
    /// The account is disabled.
    PrincipalDisabled,
    /// `max` tokens were given out for this refresh token within the window.
    TooMany,
}

/// Rows kept after they expire, so the per-refresh limit still sees them.
const KEEP_EXPIRED: Duration = Duration::hours(24);

pub(super) async fn give_outage_access_token(
    pool: &PgPool,
    refresh_hash: &[u8; 32],
    access: &IssuedToken,
    max: i64,
    window: Duration,
) -> Result<OutageRefreshOutcome> {
    let access_expires = access
        .expires_at
        .ok_or_else(|| DbError::InvalidData("an outage token requires an expiry".into()))?;
    let mut tx = begin(pool).await?;
    let row = sqlx::query(
        "SELECT rt.session_id, rt.expires_at, rt.used_at, \
                s.revoked_at AS session_revoked_at, d.principal_id, p.disabled_at \
         FROM refresh_tokens rt \
         JOIN sessions s ON s.id = rt.session_id \
         JOIN devices d ON d.id = s.device_id \
         JOIN principals p ON p.id = d.principal_id \
         WHERE rt.token_hash = $1",
    )
    .bind(refresh_hash.as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(OutageRefreshOutcome::Unknown);
    };
    let session_id: uuid::Uuid = row.try_get("session_id")?;
    let refresh_expires: DateTime<Utc> = row.try_get("expires_at")?;
    let used_at: Option<DateTime<Utc>> = row.try_get("used_at")?;
    let session_revoked_at: Option<DateTime<Utc>> = row.try_get("session_revoked_at")?;
    let principal_bytes: Vec<u8> = row.try_get("principal_id")?;
    let disabled_at: Option<DateTime<Utc>> = row.try_get("disabled_at")?;
    let principal = principal_from_bytes(&principal_bytes)?;
    if session_revoked_at.is_some() {
        return Ok(OutageRefreshOutcome::SessionRevoked);
    }
    if disabled_at.is_some() {
        return Ok(OutageRefreshOutcome::PrincipalDisabled);
    }
    if used_at.is_some() {
        return Ok(OutageRefreshOutcome::Used);
    }
    let now = Utc::now();
    if refresh_expires <= now {
        return Ok(OutageRefreshOutcome::Expired);
    }

    // One refresh token gets at most `max` tokens per window: reuse
    // detection is off while the primary is down, so this bounds what a
    // stolen refresh token gets. The lock keeps two requests for one token
    // from both passing the count.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('kuzz_outage_refresh:' || encode($1, 'hex'), 0))")
        .bind(refresh_hash.as_slice())
        .execute(&mut *tx)
        .await?;
    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outage_access_tokens \
          WHERE refresh_hash = $1 AND created_at > now() - make_interval(secs => $2)",
    )
    .bind(refresh_hash.as_slice())
    .bind(window.num_seconds() as f64)
    .fetch_one(&mut *tx)
    .await?;
    if recent >= max {
        return Ok(OutageRefreshOutcome::TooMany);
    }
    sqlx::query(
        "DELETE FROM outage_access_tokens WHERE expires_at < now() - make_interval(secs => $1)",
    )
    .bind(KEEP_EXPIRED.num_seconds() as f64)
    .execute(&mut *tx)
    .await?;
    let expires_at = access_expires.min(refresh_expires);
    sqlx::query(
        "INSERT INTO outage_access_tokens \
                (token_hash, principal_id, session_id, refresh_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(access.hash.as_slice())
    .bind(principal.as_bytes().as_slice())
    .bind(session_id)
    .bind(refresh_hash.as_slice())
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(OutageRefreshOutcome::Issued { expires_at })
}

impl crate::Db {
    /// Record an access token for refresh token `refresh_hash` without
    /// writing any global table (the primary is down). At most `max` per
    /// refresh token within `window`.
    #[datastore_span(name = "give_outage_access_token", system = "postgresql")]
    pub async fn give_outage_access_token(
        &self,
        refresh_hash: &[u8; 32],
        access: &IssuedToken,
        max: i64,
        window: Duration,
    ) -> Result<OutageRefreshOutcome> {
        give_outage_access_token(&self.pool, refresh_hash, access, max, window).await
    }
}
