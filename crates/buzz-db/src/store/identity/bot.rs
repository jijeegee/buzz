//! Bot registrations and bot tokens (desktop-hosted `bzb_`, headless `bzk_`).

use buzz_datastore_tracing::datastore_span;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use buzz_core::principal::PrincipalId;

use super::{
    begin, hashes_from_rows, principal_from_bytes, IssuedToken, RevokeReason, MAX_BOTS_PER_OWNER,
};
use crate::error::{DbError, Result};

/// A `bots` row.
#[derive(Debug, Clone)]
pub struct BotRecord {
    /// Bot principal id.
    pub id: PrincipalId,
    /// Owning user principal.
    pub owner: PrincipalId,
    /// Hosting device; `None` for a headless bot.
    pub host_device_id: Option<Uuid>,
    /// Set when the bot was deleted.
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Outcome of exchanging a still-valid desktop-hosted bot token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExchangeOutcome {
    /// The new token was stored. The old one is superseded and stays usable
    /// for its grace period (until `old_grace_until`); `revoked` are other
    /// live tokens of the bot revoked to keep at most two live.
    Exchanged {
        /// Bot principal.
        bot_id: PrincipalId,
        /// When the superseded token stops being accepted.
        old_grace_until: DateTime<Utc>,
        /// Other live bot tokens revoked by the exchange.
        revoked: Vec<[u8; 32]>,
    },
    /// Unknown token.
    NotFound,
    /// Not a desktop-hosted bot token.
    NotExchangeable,
    /// Superseded by an exchange less than `replay_window` ago: a retry of
    /// that exchange, which must recover the new token from the replay cache
    /// rather than fail.
    RecentlyExchanged,
    /// Superseded by an earlier exchange outside the replay window (409
    /// `token_superseded`).
    Superseded,
    /// Revoked, expired, bot deleted or principal disabled.
    Invalid,
}

fn bot_from_row(row: &sqlx::postgres::PgRow) -> Result<BotRecord> {
    let id: Vec<u8> = row.try_get("id")?;
    let owner: Vec<u8> = row.try_get("owner_principal_id")?;
    Ok(BotRecord {
        id: principal_from_bytes(&id)?,
        owner: principal_from_bytes(&owner)?,
        host_device_id: row.try_get("host_device_id")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

/// Create a bot principal and its `bots` row in one transaction.
pub(super) async fn create_bot(
    pool: &PgPool,
    owner: &PrincipalId,
    display_name: &str,
    host_device_id: Option<Uuid>,
) -> Result<PrincipalId> {
    let mut tx = begin(pool).await?;
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM bots WHERE owner_principal_id = $1 AND deleted_at IS NULL",
    )
    .bind(owner.as_bytes().as_slice())
    .fetch_one(&mut *tx)
    .await?;
    if live >= MAX_BOTS_PER_OWNER {
        return Err(DbError::AccessDenied("bot limit reached".into()));
    }
    let bot = PrincipalId::generate();
    sqlx::query("INSERT INTO principals (id, kind, display_name) VALUES ($1, 'bot', $2)")
        .bind(bot.as_bytes().as_slice())
        .bind(display_name)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO bots (id, owner_principal_id, host_device_id) VALUES ($1, $2, $3)")
        .bind(bot.as_bytes().as_slice())
        .bind(owner.as_bytes().as_slice())
        .bind(host_device_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(bot)
}

pub(super) async fn get_bot(pool: &PgPool, bot: &PrincipalId) -> Result<Option<BotRecord>> {
    let mut tx = begin(pool).await?;
    let row = sqlx::query(
        "SELECT id, owner_principal_id, host_device_id, deleted_at FROM bots WHERE id = $1",
    )
    .bind(bot.as_bytes().as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    row.as_ref().map(bot_from_row).transpose()
}

/// Delete `bot` (owned by `owner`): mark deleted, disable its principal and
/// revoke every token. Returns `None` when `owner` does not own a live bot.
pub(super) async fn delete_bot(
    pool: &PgPool,
    owner: &PrincipalId,
    bot: &PrincipalId,
) -> Result<Option<Vec<[u8; 32]>>> {
    let mut tx = begin(pool).await?;
    let deleted = sqlx::query(
        "UPDATE bots SET deleted_at = now() \
         WHERE id = $1 AND owner_principal_id = $2 AND deleted_at IS NULL",
    )
    .bind(bot.as_bytes().as_slice())
    .bind(owner.as_bytes().as_slice())
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() != 1 {
        return Ok(None);
    }
    sqlx::query("UPDATE principals SET disabled_at = now() WHERE id = $1")
        .bind(bot.as_bytes().as_slice())
        .execute(&mut *tx)
        .await?;
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = 'bot_deleted' \
         WHERE bot_id = $1 AND revoked_at IS NULL RETURNING token_hash",
    )
    .bind(bot.as_bytes().as_slice())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(hashes_from_rows(rows)?))
}

/// Issue a desktop-hosted bot token, revoking every earlier `bzb_` of the bot
/// (`reissued`) in the same transaction.
pub(super) async fn issue_bot_token(
    pool: &PgPool,
    bot: &PrincipalId,
    device_id: Uuid,
    token: IssuedToken,
) -> Result<Vec<[u8; 32]>> {
    let mut tx = begin(pool).await?;
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = 'reissued' \
         WHERE bot_id = $1 AND kind = 'bot' AND revoked_at IS NULL RETURNING token_hash",
    )
    .bind(bot.as_bytes().as_slice())
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, principal_id, kind, bot_id, issued_by_device, expires_at) \
         VALUES ($1, $2, 'bot', $2, $3, $4)",
    )
    .bind(token.hash.as_slice())
    .bind(bot.as_bytes().as_slice())
    .bind(device_id)
    .bind(token.expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    hashes_from_rows(rows)
}

/// Issue a headless (`bzk_`) token. Only bots without a host device qualify.
pub(super) async fn issue_headless_token(
    pool: &PgPool,
    bot: &PrincipalId,
    token: IssuedToken,
) -> Result<()> {
    let mut tx = begin(pool).await?;
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, principal_id, kind, bot_id, expires_at) \
         VALUES ($1, $2, 'bot_headless', $2, $3)",
    )
    .bind(token.hash.as_slice())
    .bind(bot.as_bytes().as_slice())
    .bind(token.expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Revoke live tokens of `bot` whose kind is in `kinds`.
pub(super) async fn revoke_bot_tokens(
    pool: &PgPool,
    bot: &PrincipalId,
    kinds: &[&str],
    reason: RevokeReason,
) -> Result<Vec<[u8; 32]>> {
    let kinds: Vec<String> = kinds.iter().map(|kind| (*kind).to_owned()).collect();
    let mut tx = begin(pool).await?;
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = $3 \
         WHERE bot_id = $1 AND kind = ANY($2) AND revoked_at IS NULL RETURNING token_hash",
    )
    .bind(bot.as_bytes().as_slice())
    .bind(&kinds)
    .bind(reason.as_str())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    hashes_from_rows(rows)
}

/// Revoke one headless token of `bot` by hash prefix (hex, ≥ 8 chars).
/// Returns `None` when no live headless token matches exactly one row.
pub(super) async fn revoke_headless_token(
    pool: &PgPool,
    bot: &PrincipalId,
    hash_prefix: &str,
) -> Result<Option<[u8; 32]>> {
    let mut tx = begin(pool).await?;
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT token_hash FROM access_tokens \
         WHERE bot_id = $1 AND kind = 'bot_headless' AND revoked_at IS NULL \
           AND encode(token_hash, 'hex') LIKE $2 || '%' FOR UPDATE",
    )
    .bind(bot.as_bytes().as_slice())
    .bind(hash_prefix.to_ascii_lowercase())
    .fetch_all(&mut *tx)
    .await?;
    if rows.len() != 1 {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = 'reissued' \
         WHERE token_hash = $1",
    )
    .bind(&rows[0])
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(hashes_from_rows(rows)?.pop())
}

/// Revoke every bot token of every bot `owner` owns (`revoke_all`).
pub(super) async fn revoke_all_owned_bot_tokens(
    pool: &PgPool,
    owner: &PrincipalId,
) -> Result<Vec<[u8; 32]>> {
    let mut tx = begin(pool).await?;
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = 'revoke_all' \
         WHERE revoked_at IS NULL AND bot_id IN \
           (SELECT id FROM bots WHERE owner_principal_id = $1) RETURNING token_hash",
    )
    .bind(owner.as_bytes().as_slice())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    hashes_from_rows(rows)
}

/// Exchange a valid, not-yet-superseded `bzb_` for a new one.
///
/// In one transaction: mark the old token superseded with a grace expiry,
/// revoke any other live `bzb_` of the bot (at most two live tokens), insert
/// the new token. A superseded token can still authenticate during its grace
/// but can never be exchanged again; one superseded less than `replay_window`
/// ago reports [`ExchangeOutcome::RecentlyExchanged`] so the caller can serve
/// the retry from its replay cache.
pub(super) async fn exchange_bot_token(
    pool: &PgPool,
    old_hash: &[u8; 32],
    new_token: IssuedToken,
    grace: chrono::Duration,
    replay_window: chrono::Duration,
) -> Result<ExchangeOutcome> {
    let mut tx = begin(pool).await?;
    let row = sqlx::query(
        "SELECT t.kind, t.bot_id, t.issued_by_device, t.expires_at, t.superseded_at, \
                t.superseded_at > now() - make_interval(secs => $2) AS recently_superseded, \
                t.revoked_at, b.deleted_at, p.disabled_at \
         FROM access_tokens t \
         JOIN bots b ON b.id = t.bot_id \
         JOIN principals p ON p.id = t.principal_id \
         WHERE t.token_hash = $1 FOR UPDATE OF t",
    )
    .bind(old_hash.as_slice())
    .bind(replay_window.num_milliseconds() as f64 / 1000.0)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(ExchangeOutcome::NotFound);
    };
    let kind: String = row.try_get("kind")?;
    if kind != "bot" {
        return Ok(ExchangeOutcome::NotExchangeable);
    }
    // `superseded_at` is stamped with the DB clock, so the replay window is
    // measured on the DB clock too (NULL when the token was never superseded).
    let superseded_at: Option<DateTime<Utc>> = row.try_get("superseded_at")?;
    let recently_superseded: Option<bool> = row.try_get("recently_superseded")?;
    if superseded_at.is_some() {
        if recently_superseded == Some(true) {
            return Ok(ExchangeOutcome::RecentlyExchanged);
        }
        return Ok(ExchangeOutcome::Superseded);
    }
    let expires_at: Option<DateTime<Utc>> = row.try_get("expires_at")?;
    let revoked_at: Option<DateTime<Utc>> = row.try_get("revoked_at")?;
    let deleted_at: Option<DateTime<Utc>> = row.try_get("deleted_at")?;
    let disabled_at: Option<DateTime<Utc>> = row.try_get("disabled_at")?;
    let now = Utc::now();
    if revoked_at.is_some()
        || deleted_at.is_some()
        || disabled_at.is_some()
        || expires_at.is_none_or(|exp| exp <= now)
    {
        return Ok(ExchangeOutcome::Invalid);
    }
    let bot_bytes: Vec<u8> = row.try_get("bot_id")?;
    let bot = principal_from_bytes(&bot_bytes)?;
    let device: Option<Uuid> = row.try_get("issued_by_device")?;

    let old_grace_until: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE access_tokens SET superseded_at = now(), \
                expires_at = LEAST(expires_at, now() + make_interval(secs => $2)) \
         WHERE token_hash = $1 RETURNING expires_at",
    )
    .bind(old_hash.as_slice())
    .bind(grace.num_milliseconds() as f64 / 1000.0)
    .fetch_one(&mut *tx)
    .await?;
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE access_tokens SET revoked_at = now(), revoked_reason = 'exchanged' \
         WHERE bot_id = $1 AND kind = 'bot' AND revoked_at IS NULL AND token_hash <> $2 \
         RETURNING token_hash",
    )
    .bind(bot.as_bytes().as_slice())
    .bind(old_hash.as_slice())
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO access_tokens (token_hash, principal_id, kind, bot_id, issued_by_device, expires_at) \
         VALUES ($1, $2, 'bot', $2, $3, $4)",
    )
    .bind(new_token.hash.as_slice())
    .bind(bot.as_bytes().as_slice())
    .bind(device)
    .bind(new_token.expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ExchangeOutcome::Exchanged {
        bot_id: bot,
        old_grace_until,
        revoked: hashes_from_rows(rows)?,
    })
}

impl crate::Db {
    /// Register a bot owned by `owner` (`host_device_id = None` ⇒ headless).
    #[datastore_span(name = "create_bot", system = "postgresql")]
    pub async fn create_bot(
        &self,
        owner: &PrincipalId,
        display_name: &str,
        host_device_id: Option<Uuid>,
    ) -> Result<PrincipalId> {
        create_bot(&self.pool, owner, display_name, host_device_id).await
    }

    /// Fetch a bot registration.
    #[datastore_span(name = "get_bot", system = "postgresql")]
    pub async fn get_bot(&self, bot: &PrincipalId) -> Result<Option<BotRecord>> {
        get_bot(&self.pool, bot).await
    }

    /// Delete a bot owned by `owner` and revoke all of its tokens.
    #[datastore_span(name = "delete_bot", system = "postgresql")]
    pub async fn delete_bot(
        &self,
        owner: &PrincipalId,
        bot: &PrincipalId,
    ) -> Result<Option<Vec<[u8; 32]>>> {
        delete_bot(&self.pool, owner, bot).await
    }

    /// Issue a desktop-hosted bot token (revokes earlier ones as `reissued`).
    #[datastore_span(name = "issue_bot_token", system = "postgresql")]
    pub async fn issue_bot_token(
        &self,
        bot: &PrincipalId,
        device_id: Uuid,
        token: IssuedToken,
    ) -> Result<Vec<[u8; 32]>> {
        issue_bot_token(&self.pool, bot, device_id, token).await
    }

    /// Issue a headless bot token.
    #[datastore_span(name = "issue_headless_bot_token", system = "postgresql")]
    pub async fn issue_headless_bot_token(
        &self,
        bot: &PrincipalId,
        token: IssuedToken,
    ) -> Result<()> {
        issue_headless_token(&self.pool, bot, token).await
    }

    /// Revoke live tokens of `bot` of the given kinds.
    #[datastore_span(name = "revoke_bot_tokens", system = "postgresql")]
    pub async fn revoke_bot_tokens(
        &self,
        bot: &PrincipalId,
        kinds: &[&str],
        reason: RevokeReason,
    ) -> Result<Vec<[u8; 32]>> {
        revoke_bot_tokens(&self.pool, bot, kinds, reason).await
    }

    /// Revoke one headless token of `bot` by hash prefix.
    #[datastore_span(name = "revoke_headless_bot_token", system = "postgresql")]
    pub async fn revoke_headless_bot_token(
        &self,
        bot: &PrincipalId,
        hash_prefix: &str,
    ) -> Result<Option<[u8; 32]>> {
        revoke_headless_token(&self.pool, bot, hash_prefix).await
    }

    /// Revoke every bot token of every bot owned by `owner`.
    #[datastore_span(name = "revoke_all_owned_bot_tokens", system = "postgresql")]
    pub async fn revoke_all_owned_bot_tokens(&self, owner: &PrincipalId) -> Result<Vec<[u8; 32]>> {
        revoke_all_owned_bot_tokens(&self.pool, owner).await
    }

    /// Exchange a valid desktop-hosted bot token for a new one. A token
    /// superseded less than `replay_window` ago yields
    /// [`ExchangeOutcome::RecentlyExchanged`].
    #[datastore_span(name = "exchange_bot_token", system = "postgresql")]
    pub async fn exchange_bot_token(
        &self,
        old_hash: &[u8; 32],
        new_token: IssuedToken,
        grace: chrono::Duration,
        replay_window: chrono::Duration,
    ) -> Result<ExchangeOutcome> {
        exchange_bot_token(&self.pool, old_hash, new_token, grace, replay_window).await
    }
}
