//! Devices, login sessions and refresh-token rotation.

use buzz_datastore_tracing::datastore_span;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use buzz_core::principal::PrincipalId;

use super::{
    begin, hashes_from_rows, principal_from_bytes, revoke_session_tokens, IssuedToken,
    RevokeReason, MAX_DEVICES_PER_PRINCIPAL, REFRESH_GENERATIONS_KEPT,
};
use crate::error::{DbError, Result};

/// The device and new session of a completed login.
#[derive(Debug, Clone)]
pub struct LoginSession {
    /// Device id: new, or the reused one for a known install id.
    pub device_id: Uuid,
    /// New session id.
    pub session_id: Uuid,
    /// Access-token hashes of the reused device's earlier sessions, revoked
    /// because this login replaces them.
    pub revoked: Vec<[u8; 32]>,
}

/// A live device of a principal.
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    /// Device id.
    pub id: Uuid,
    /// User-visible device name.
    pub name: String,
    /// `desktop`, `mobile`, `web` or `cli`.
    pub platform: String,
    /// Last login or refresh on this device.
    pub last_seen_at: DateTime<Utc>,
}

/// Outcome of presenting a refresh token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Rotated: the old row is consumed, a new refresh + access were stored.
    Rotated {
        /// Principal of the session.
        principal: PrincipalId,
        /// Session id.
        session_id: Uuid,
        /// Device id.
        device_id: Uuid,
    },
    /// No such refresh token. Nothing changed.
    NotFound,
    /// The token was consumed within the reuse grace window: a network retry
    /// racing its own successful rotation. Nothing changed; the caller serves
    /// the cached rotation result instead of treating this as theft.
    RecentlyRotated,
    /// A consumed refresh token was presented again: the session and its
    /// access tokens were revoked. `revoked` are the access-token hashes.
    Reused {
        /// Revoked session id.
        session_id: Uuid,
        /// Access-token hashes revoked with the session.
        revoked: Vec<[u8; 32]>,
    },
    /// The refresh token expired.
    Expired,
    /// The session was already revoked.
    SessionRevoked,
    /// The principal is disabled.
    PrincipalDisabled,
}

/// Create (or, for an install id the principal already used, reuse) the
/// device, then session + first refresh + first access token, in one
/// transaction (one login = one atomic persist). A reused device keeps its id
/// and name, is un-revoked, and its earlier sessions are revoked: the install
/// that logs in again has lost or dropped their tokens.
pub(super) async fn complete_login(
    pool: &PgPool,
    principal: &PrincipalId,
    device_name: &str,
    platform: &str,
    install_id: Option<&str>,
    refresh: IssuedToken,
    access: IssuedToken,
) -> Result<LoginSession> {
    let refresh_expires = refresh
        .expires_at
        .ok_or_else(|| DbError::InvalidData("refresh token requires an expiry".into()))?;
    let mut tx = begin(pool).await?;
    super::lock_subject(&mut tx, principal).await?;
    let known: Option<Uuid> = match install_id {
        Some(install_id) => {
            sqlx::query_scalar(
                "SELECT id FROM devices WHERE principal_id = $1 AND install_id = $2 FOR UPDATE",
            )
            .bind(principal.as_bytes().as_slice())
            .bind(install_id)
            .fetch_optional(&mut *tx)
            .await?
        }
        None => None,
    };
    let live_devices: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM devices WHERE principal_id = $1 AND revoked_at IS NULL          AND id IS DISTINCT FROM $2",
    )
    .bind(principal.as_bytes().as_slice())
    .bind(known)
    .fetch_one(&mut *tx)
    .await?;
    if live_devices >= MAX_DEVICES_PER_PRINCIPAL {
        return Err(DbError::AccessDenied("device limit reached".into()));
    }
    let mut revoked = Vec::new();
    let device_id = match known {
        Some(device_id) => {
            sqlx::query(
                "UPDATE devices SET revoked_at = NULL, last_seen_at = now(), platform = $2                  WHERE id = $1",
            )
            .bind(device_id)
            .bind(platform)
            .execute(&mut *tx)
            .await?;
            let sessions: Vec<Uuid> = sqlx::query_scalar(
                "UPDATE sessions SET revoked_at = now(), revoked_reason = $2                  WHERE device_id = $1 AND revoked_at IS NULL RETURNING id",
            )
            .bind(device_id)
            .bind(RevokeReason::Relogin.as_str())
            .fetch_all(&mut *tx)
            .await?;
            revoked = revoke_session_tokens(&mut tx, &sessions, RevokeReason::Relogin).await?;
            device_id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO devices (principal_id, name, platform, install_id)                  VALUES ($1, $2, $3, $4) RETURNING id",
            )
            .bind(principal.as_bytes().as_slice())
            .bind(device_name)
            .bind(platform)
            .bind(install_id)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    let session_id: Uuid =
        sqlx::query_scalar("INSERT INTO sessions (device_id) VALUES ($1) RETURNING id")
            .bind(device_id)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query(
        "INSERT INTO refresh_tokens (token_hash, session_id, generation, expires_at) \
         VALUES ($1, $2, 1, $3)",
    )
    .bind(refresh.hash.as_slice())
    .bind(session_id)
    .bind(refresh_expires)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, principal_id, kind, session_id, expires_at) \
         VALUES ($1, $2, 'user', $3, $4)",
    )
    .bind(access.hash.as_slice())
    .bind(principal.as_bytes().as_slice())
    .bind(session_id)
    .bind(access.expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(LoginSession {
        device_id,
        session_id,
        revoked,
    })
}

/// Present refresh token `old_hash`; rotate it on success.
///
/// Ordering (plan §3.6): the caller consults its 10 s replay cache *before*
/// calling this. Here a consumed row (`used_at` set) is reuse — the whole
/// session is revoked — which is distinct from an unknown token.
pub(super) async fn rotate_refresh(
    pool: &PgPool,
    old_hash: &[u8; 32],
    new_refresh: IssuedToken,
    new_access: IssuedToken,
    reuse_grace: chrono::Duration,
) -> Result<RefreshOutcome> {
    let new_refresh_expires = new_refresh
        .expires_at
        .ok_or_else(|| DbError::InvalidData("refresh token requires an expiry".into()))?;
    let mut tx = begin(pool).await?;
    let subject: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT d.principal_id FROM refresh_tokens rt JOIN sessions s ON s.id = rt.session_id JOIN devices d ON d.id = s.device_id WHERE rt.token_hash = $1")
        .bind(old_hash.as_slice()).fetch_optional(&mut *tx).await?;
    let Some(subject) = subject else {
        return Ok(RefreshOutcome::NotFound);
    };
    super::lock_subject(&mut tx, &principal_from_bytes(&subject)?).await?;
    let row = sqlx::query(
        "SELECT rt.session_id, rt.generation, rt.expires_at, rt.used_at, \
                s.revoked_at AS session_revoked_at, d.id AS device_id, d.principal_id, \
                p.disabled_at \
         FROM refresh_tokens rt \
         JOIN sessions s ON s.id = rt.session_id \
         JOIN devices d ON d.id = s.device_id \
         JOIN principals p ON p.id = d.principal_id \
         WHERE rt.token_hash = $1 \
         FOR UPDATE OF rt, s",
    )
    .bind(old_hash.as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(RefreshOutcome::NotFound);
    };
    let session_id: Uuid = row.try_get("session_id")?;
    let generation: i32 = row.try_get("generation")?;
    let expires_at: DateTime<Utc> = row.try_get("expires_at")?;
    let used_at: Option<DateTime<Utc>> = row.try_get("used_at")?;
    let session_revoked_at: Option<DateTime<Utc>> = row.try_get("session_revoked_at")?;
    let device_id: Uuid = row.try_get("device_id")?;
    let principal_bytes: Vec<u8> = row.try_get("principal_id")?;
    let disabled_at: Option<DateTime<Utc>> = row.try_get("disabled_at")?;

    if used_at.is_some_and(|used| used > Utc::now() - reuse_grace) {
        return Ok(RefreshOutcome::RecentlyRotated);
    }
    if used_at.is_some() {
        // Reuse of a consumed refresh token: assume theft, kill the session.
        sqlx::query(
            "UPDATE sessions SET revoked_at = COALESCE(revoked_at, now()), \
                    revoked_reason = COALESCE(revoked_reason, $2) WHERE id = $1",
        )
        .bind(session_id)
        .bind(RevokeReason::RefreshReused.as_str())
        .execute(&mut *tx)
        .await?;
        let revoked =
            revoke_session_tokens(&mut tx, &[session_id], RevokeReason::RefreshReused).await?;
        tx.commit().await?;
        return Ok(RefreshOutcome::Reused {
            session_id,
            revoked,
        });
    }
    if session_revoked_at.is_some() {
        return Ok(RefreshOutcome::SessionRevoked);
    }
    if disabled_at.is_some() {
        return Ok(RefreshOutcome::PrincipalDisabled);
    }
    if expires_at <= Utc::now() {
        return Ok(RefreshOutcome::Expired);
    }

    let consumed = sqlx::query(
        "UPDATE refresh_tokens SET used_at = now() WHERE token_hash = $1 AND used_at IS NULL",
    )
    .bind(old_hash.as_slice())
    .execute(&mut *tx)
    .await?;
    if consumed.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "refresh token consumed concurrently".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO refresh_tokens (token_hash, session_id, generation, expires_at) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(new_refresh.hash.as_slice())
    .bind(session_id)
    .bind(generation + 1)
    .bind(new_refresh_expires)
    .execute(&mut *tx)
    .await?;
    // Bound the rotation history per session.
    sqlx::query("DELETE FROM refresh_tokens WHERE session_id = $1 AND generation <= $2")
        .bind(session_id)
        .bind(generation + 1 - REFRESH_GENERATIONS_KEPT)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE sessions SET last_refreshed_at = now() WHERE id = $1")
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE devices SET last_seen_at = now() WHERE id = $1")
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
    let principal = principal_from_bytes(&principal_bytes)?;
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, principal_id, kind, session_id, expires_at) \
         VALUES ($1, $2, 'user', $3, $4)",
    )
    .bind(new_access.hash.as_slice())
    .bind(principal.as_bytes().as_slice())
    .bind(session_id)
    .bind(new_access.expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(RefreshOutcome::Rotated {
        principal,
        session_id,
        device_id,
    })
}

/// Revoke `device_id` (owned by `principal`): its sessions, their access
/// tokens, and the desktop-hosted bot tokens it issued. Returns `None` when the
/// device does not belong to `principal`.
pub(super) async fn revoke_device(
    pool: &PgPool,
    principal: &PrincipalId,
    device_id: Uuid,
    reason: RevokeReason,
) -> Result<Option<Vec<[u8; 32]>>> {
    let mut tx = begin(pool).await?;
    super::lock_subject(&mut tx, principal).await?;
    let owned: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM devices WHERE id = $1 AND principal_id = $2 FOR UPDATE")
            .bind(device_id)
            .bind(principal.as_bytes().as_slice())
            .fetch_optional(&mut *tx)
            .await?;
    if owned.is_none() {
        return Ok(None);
    }
    sqlx::query("UPDATE devices SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1")
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
    let sessions: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = $2 \
         WHERE device_id = $1 AND revoked_at IS NULL RETURNING id",
    )
    .bind(device_id)
    .bind(reason.as_str())
    .fetch_all(&mut *tx)
    .await?;
    let mut revoked = revoke_session_tokens(&mut tx, &sessions, reason).await?;
    let bot_rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = 'device_revoked' \
         WHERE kind = 'bot' AND revoked_at IS NULL AND bot_id IN \
           (SELECT id FROM bots WHERE host_device_id = $1) \
         RETURNING token_hash",
    )
    .bind(device_id)
    .fetch_all(&mut *tx)
    .await?;
    revoked.extend(hashes_from_rows(bot_rows)?);
    tx.commit().await?;
    Ok(Some(revoked))
}

/// Revoke every human session of `principal` except `keep_session`. Bot
/// tokens are untouched (Telegram "terminate other sessions").
pub(super) async fn revoke_other_sessions(
    pool: &PgPool,
    principal: &PrincipalId,
    keep_session: Uuid,
) -> Result<Vec<[u8; 32]>> {
    let mut tx = begin(pool).await?;
    super::lock_subject(&mut tx, principal).await?;
    let sessions: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE sessions SET revoked_at = now(), revoked_reason = 'revoke_all' \
         WHERE revoked_at IS NULL AND id <> $2 AND device_id IN \
           (SELECT id FROM devices WHERE principal_id = $1) RETURNING id",
    )
    .bind(principal.as_bytes().as_slice())
    .bind(keep_session)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE devices SET revoked_at = now() WHERE principal_id = $1 AND revoked_at IS NULL \
         AND id <> (SELECT device_id FROM sessions WHERE id = $2)",
    )
    .bind(principal.as_bytes().as_slice())
    .bind(keep_session)
    .execute(&mut *tx)
    .await?;
    let revoked = revoke_session_tokens(&mut tx, &sessions, RevokeReason::RevokeAll).await?;
    tx.commit().await?;
    Ok(revoked)
}

pub(super) async fn list_devices(
    pool: &PgPool,
    principal: &PrincipalId,
) -> Result<Vec<DeviceRecord>> {
    let mut tx = begin(pool).await?;
    let rows = sqlx::query(
        "SELECT id, name, platform, last_seen_at FROM devices \
         WHERE principal_id = $1 AND revoked_at IS NULL ORDER BY created_at",
    )
    .bind(principal.as_bytes().as_slice())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    rows.iter()
        .map(|row| {
            Ok(DeviceRecord {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                platform: row.try_get("platform")?,
                last_seen_at: row.try_get("last_seen_at")?,
            })
        })
        .collect()
}

/// Rename live `device_id` (owned by `principal`). Returns `None` when the
/// device does not belong to `principal` or is revoked.
pub(super) async fn rename_device(
    pool: &PgPool,
    principal: &PrincipalId,
    device_id: Uuid,
    name: &str,
) -> Result<Option<DeviceRecord>> {
    let mut tx = begin(pool).await?;
    let row = sqlx::query(
        "UPDATE devices SET name = $3 \
         WHERE id = $1 AND principal_id = $2 AND revoked_at IS NULL \
         RETURNING id, name, platform, last_seen_at",
    )
    .bind(device_id)
    .bind(principal.as_bytes().as_slice())
    .bind(name)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    row.map(|row| {
        Ok(DeviceRecord {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            platform: row.try_get("platform")?,
            last_seen_at: row.try_get("last_seen_at")?,
        })
    })
    .transpose()
}

impl crate::Db {
    /// Persist a completed login: device (reused for a known `install_id`),
    /// session, refresh and access token.
    #[datastore_span(name = "complete_login", system = "postgresql")]
    pub async fn complete_login(
        &self,
        principal: &PrincipalId,
        device_name: &str,
        platform: &str,
        install_id: Option<&str>,
        refresh: IssuedToken,
        access: IssuedToken,
    ) -> Result<LoginSession> {
        complete_login(
            &self.pool,
            principal,
            device_name,
            platform,
            install_id,
            refresh,
            access,
        )
        .await
    }

    /// Rotate a refresh token (or detect its reuse). A token consumed less
    /// than `reuse_grace` ago yields [`RefreshOutcome::RecentlyRotated`]
    /// instead of revoking the session.
    #[datastore_span(name = "rotate_refresh_token", system = "postgresql")]
    pub async fn rotate_refresh_token(
        &self,
        old_hash: &[u8; 32],
        new_refresh: IssuedToken,
        new_access: IssuedToken,
        reuse_grace: chrono::Duration,
    ) -> Result<RefreshOutcome> {
        rotate_refresh(&self.pool, old_hash, new_refresh, new_access, reuse_grace).await
    }

    /// Revoke a device of `principal`, its sessions and hosted bot tokens.
    #[datastore_span(name = "revoke_device", system = "postgresql")]
    pub async fn revoke_device(
        &self,
        principal: &PrincipalId,
        device_id: Uuid,
        reason: super::RevokeReason,
    ) -> Result<Option<Vec<[u8; 32]>>> {
        revoke_device(&self.pool, principal, device_id, reason).await
    }

    /// Revoke all human sessions of `principal` except `keep_session`.
    #[datastore_span(name = "revoke_other_sessions", system = "postgresql")]
    pub async fn revoke_other_sessions(
        &self,
        principal: &PrincipalId,
        keep_session: Uuid,
    ) -> Result<Vec<[u8; 32]>> {
        revoke_other_sessions(&self.pool, principal, keep_session).await
    }

    /// List live devices of `principal`.
    #[datastore_span(name = "list_devices", system = "postgresql")]
    pub async fn list_devices(&self, principal: &PrincipalId) -> Result<Vec<DeviceRecord>> {
        list_devices(&self.pool, principal).await
    }

    /// Rename a live device of `principal`.
    #[datastore_span(name = "rename_device", system = "postgresql")]
    pub async fn rename_device(
        &self,
        principal: &PrincipalId,
        device_id: Uuid,
        name: &str,
    ) -> Result<Option<DeviceRecord>> {
        rename_device(&self.pool, principal, device_id, name).await
    }
}
