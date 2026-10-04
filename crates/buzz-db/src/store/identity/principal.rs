//! Principals, external identities, the relay principal and global profiles.

use buzz_datastore_tracing::datastore_span;
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row as _};

use buzz_core::principal::{PrincipalId, PrincipalKind};

use super::{begin, is_unique_violation, principal_from_bytes, principal_kind_from_str};
use crate::error::{DbError, Result};

/// A `principals` row.
#[derive(Debug, Clone)]
pub struct PrincipalRecord {
    /// Principal id.
    pub id: PrincipalId,
    /// `user`, `bot` or `relay`.
    pub kind: PrincipalKind,
    /// Global display name.
    pub display_name: String,
    /// Global avatar URL.
    pub avatar_url: Option<String>,
    /// Optional unique `@username`.
    pub username: Option<String>,
    /// Set when the account is disabled (operator action or self-deletion).
    pub disabled_at: Option<DateTime<Utc>>,
    /// Scheduled purge time after self-deletion.
    pub purge_after: Option<DateTime<Utc>>,
}

/// Result of resolving an OIDC identity to a principal at login.
#[derive(Debug, Clone, Copy)]
pub struct LoginPrincipal {
    /// The principal the identity maps to.
    pub principal: PrincipalId,
    /// Whether this login created the account.
    pub created: bool,
    /// Whether the account is disabled by an operator (login must fail).
    pub disabled: bool,
}

/// Fields of a global profile update; `None` leaves a field unchanged.
#[derive(Debug, Clone, Default)]
pub struct ProfileUpdate {
    /// New display name.
    pub display_name: Option<String>,
    /// New avatar URL (`Some(None)` clears it).
    pub avatar_url: Option<Option<String>>,
    /// New `@username` (`Some(None)` clears it).
    pub username: Option<Option<String>>,
}

fn record_from_row(row: &sqlx::postgres::PgRow) -> Result<PrincipalRecord> {
    let id: Vec<u8> = row.try_get("id")?;
    let kind: String = row.try_get("kind")?;
    Ok(PrincipalRecord {
        id: principal_from_bytes(&id)?,
        kind: principal_kind_from_str(&kind)?,
        display_name: row.try_get("display_name")?,
        avatar_url: row.try_get("avatar_url")?,
        username: row.try_get("username")?,
        disabled_at: row.try_get("disabled_at")?,
        purge_after: row.try_get("purge_after")?,
    })
}

/// Make `key` the deployment's relay principal and return it.
///
/// The relay principal is the relay's own signing key (`RELAY_PRIVATE_KEY`),
/// so relay-authored events keep one author whether they are signed or
/// server-stamped, and every existing relay-signed event stays attributable.
/// A relay row with another id (an earlier key, or a Phase 0/1 random id) is
/// re-keyed; `relay_operators.added_by` follows it. Concurrent boots serialize
/// on an advisory lock; the partial unique index `principals_single_relay`
/// still admits only one row.
pub(super) async fn ensure_relay_principal(
    pool: &PgPool,
    key: &PrincipalId,
) -> Result<PrincipalId> {
    let mut tx = begin(pool).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('buzz:relay_principal'))")
        .execute(&mut *tx)
        .await?;
    let current: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT id FROM principals WHERE kind = 'relay'")
            .fetch_optional(&mut *tx)
            .await?;
    let key_bytes = key.as_bytes().as_slice();
    // The relay key must not already name a user or bot: re-keying onto it
    // would collide, and inserting would silently leave no relay row.
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM principals WHERE id = $1 AND kind <> 'relay')",
    )
    .bind(key_bytes)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Err(DbError::InvalidData(
            "relay key is already registered as a non-relay principal".into(),
        ));
    }
    match current {
        None => {
            sqlx::query(
                "INSERT INTO principals (id, kind, display_name) VALUES ($1, 'relay', 'relay')                  ON CONFLICT DO NOTHING",
            )
            .bind(key_bytes)
            .execute(&mut *tx)
            .await?;
        }
        Some(old) if old.as_slice() != key_bytes => {
            tracing::warn!(
                old = %hex::encode(&old),
                new = %key.to_hex(),
                "relay key changed; re-keying the relay principal"
            );
            sqlx::query("UPDATE principals SET id = $2 WHERE id = $1 AND kind = 'relay'")
                .bind(&old)
                .bind(key_bytes)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE relay_operators SET added_by = $2 WHERE added_by = $1")
                .bind(&old)
                .bind(key_bytes)
                .execute(&mut *tx)
                .await?;
        }
        Some(_) => {}
    }
    let id: Vec<u8> = sqlx::query_scalar("SELECT id FROM principals WHERE kind = 'relay'")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    principal_from_bytes(&id)
}

/// Resolve `(provider, subject)` to a principal, creating the account on
/// first login. A self-deleted account inside its purge window is restored.
pub(super) async fn login_identity(
    pool: &PgPool,
    provider: &str,
    subject: &str,
    email: Option<&str>,
    display_name: &str,
    avatar_url: Option<&str>,
) -> Result<LoginPrincipal> {
    // Two attempts: a concurrent first login for the same subject makes our
    // identity insert a no-op; the retry then finds the winner's principal.
    for _ in 0..2 {
        let mut tx = begin(pool).await?;
        let existing = sqlx::query(
            "SELECT i.principal_id, p.disabled_at, p.purge_after FROM identities i \
             JOIN principals p ON p.id = i.principal_id \
             WHERE i.provider = $1 AND i.subject = $2 FOR UPDATE OF i, p",
        )
        .bind(provider)
        .bind(subject)
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(row) = existing {
            let id: Vec<u8> = row.try_get("principal_id")?;
            let disabled_at: Option<DateTime<Utc>> = row.try_get("disabled_at")?;
            let purge_after: Option<DateTime<Utc>> = row.try_get("purge_after")?;
            sqlx::query(
                "UPDATE identities SET last_login_at = now(), email = COALESCE($3, email) \
                 WHERE provider = $1 AND subject = $2",
            )
            .bind(provider)
            .bind(subject)
            .bind(email)
            .execute(&mut *tx)
            .await?;
            // Self-deletion (purge scheduled) is undone by logging in again
            // inside the purge window; an operator disable is not.
            let self_deleted = disabled_at.is_some() && purge_after.is_some();
            if self_deleted {
                sqlx::query(
                    "UPDATE principals SET disabled_at = NULL, purge_after = NULL WHERE id = $1",
                )
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            return Ok(LoginPrincipal {
                principal: principal_from_bytes(&id)?,
                created: false,
                disabled: disabled_at.is_some() && !self_deleted,
            });
        }

        let principal = PrincipalId::generate();
        sqlx::query(
            "INSERT INTO principals (id, kind, display_name, avatar_url) VALUES ($1, 'user', $2, $3)",
        )
        .bind(principal.as_bytes().as_slice())
        .bind(display_name)
        .bind(avatar_url)
        .execute(&mut *tx)
        .await?;
        let inserted: Option<Vec<u8>> = sqlx::query_scalar(
            "INSERT INTO identities (provider, subject, principal_id, email, last_login_at) \
             VALUES ($1, $2, $3, $4, now()) ON CONFLICT (provider, subject) DO NOTHING \
             RETURNING principal_id",
        )
        .bind(provider)
        .bind(subject)
        .bind(principal.as_bytes().as_slice())
        .bind(email)
        .fetch_optional(&mut *tx)
        .await?;
        if inserted.is_some() {
            tx.commit().await?;
            return Ok(LoginPrincipal {
                principal,
                created: true,
                disabled: false,
            });
        }
        // Lost the race: discard our provisional principal and re-read.
        tx.rollback().await?;
    }
    Err(DbError::InvalidData(
        "identity login did not converge after a concurrent first login".into(),
    ))
}

pub(super) async fn get_principal(
    pool: &PgPool,
    id: &PrincipalId,
) -> Result<Option<PrincipalRecord>> {
    let mut tx = begin(pool).await?;
    let row = sqlx::query(
        "SELECT id, kind, display_name, avatar_url, username, disabled_at, purge_after          FROM principals WHERE id = $1",
    )
    .bind(id.as_bytes().as_slice())
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    row.as_ref().map(record_from_row).transpose()
}

/// Apply a global profile update and project it into every active
/// community's `users` row in the same transaction.
pub(super) async fn update_profile(
    pool: &PgPool,
    id: &PrincipalId,
    update: &ProfileUpdate,
) -> Result<Option<PrincipalRecord>> {
    let mut tx = begin(pool).await?;
    let result = sqlx::query(
        "UPDATE principals SET \
           display_name = COALESCE($2, display_name), \
           avatar_url = CASE WHEN $3 THEN $4 ELSE avatar_url END, \
           username = CASE WHEN $5 THEN $6 ELSE username END \
         WHERE id = $1 AND disabled_at IS NULL \
         RETURNING id, kind, display_name, avatar_url, username, disabled_at, purge_after",
    )
    .bind(id.as_bytes().as_slice())
    .bind(update.display_name.as_deref())
    .bind(update.avatar_url.is_some())
    .bind(update.avatar_url.clone().flatten())
    .bind(update.username.is_some())
    .bind(update.username.clone().flatten())
    .fetch_optional(&mut *tx)
    .await;
    let row = match result {
        Ok(row) => row,
        Err(error) if is_unique_violation(&error) => {
            return Err(DbError::AccessDenied("username is already taken".into()))
        }
        Err(error) => return Err(error.into()),
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let record = record_from_row(&row)?;
    sqlx::query(
        "UPDATE users SET display_name = $2, avatar_url = $3, updated_at = now() \
         WHERE pubkey = $1 AND community_id IN \
           (SELECT id FROM communities WHERE deletion_state = 'active')",
    )
    .bind(id.as_bytes().as_slice())
    .bind(&record.display_name)
    .bind(record.avatar_url.as_deref())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(record))
}

/// An active community whose `users` projection holds a principal.
#[derive(Debug, Clone)]
pub struct ProfileCommunity {
    /// The community.
    pub community: buzz_core::CommunityId,
    /// Its host (for tenant labelling only).
    pub host: String,
}

/// The newest live kind:0 a principal has in one community.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEventState {
    /// `created_at` (unix seconds).
    pub created_at: i64,
    /// Event content (profile JSON).
    pub content: String,
}

/// Upper bound on communities one profile change fans out to.
pub const MAX_PROFILE_COMMUNITIES: i64 = 1000;

/// An attributed connection for the profile-publish reads.
async fn profile_reader(pool: &PgPool) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>> {
    crate::observability::acquire_writer(
        pool,
        crate::observability::WriterOperation::Authentication,
    )
    .await
    .map_err(Into::into)
}

async fn profile_communities(pool: &PgPool, id: &PrincipalId) -> Result<Vec<ProfileCommunity>> {
    let rows = sqlx::query(
        "SELECT u.community_id, c.host FROM users u          JOIN communities c ON c.id = u.community_id          WHERE u.pubkey = $1 AND c.archived_at IS NULL AND c.deleted_at IS NULL            AND c.deletion_state = 'active'          ORDER BY u.community_id LIMIT $2",
    )
    .bind(id.as_bytes().as_slice())
    .bind(MAX_PROFILE_COMMUNITIES)
    .fetch_all(&mut *profile_reader(pool).await?)
    .await?;
    rows.iter()
        .map(|row| {
            let community: uuid::Uuid = row.try_get("community_id")?;
            Ok(ProfileCommunity {
                community: buzz_core::CommunityId::from_uuid(community),
                host: row.try_get("host")?,
            })
        })
        .collect()
}

/// Everything a kind:0 publish decides on, read under its lock.
#[derive(Debug, Clone)]
pub struct ProfilePublishSnapshot {
    /// The principal row (fresh: read after the lock was granted).
    pub record: Option<PrincipalRecord>,
    /// The community's newest live kind:0 of the principal.
    pub latest: Option<ProfileEventState>,
    /// Whether the community's `users` projection holds the principal.
    pub has_user_row: bool,
}

/// A held per-(community, principal) kind:0 publish lock: a transaction
/// holding `pg_advisory_xact_lock`. [`Self::release`] commits; dropping it
/// rolls back, which releases the lock as well.
pub struct ProfilePublishLock {
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
}

impl std::fmt::Debug for ProfilePublishLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProfilePublishLock")
    }
}

impl ProfilePublishLock {
    /// Release the lock.
    pub async fn release(self) -> Result<()> {
        self.tx.commit().await?;
        Ok(())
    }
}

/// Advisory-lock key for one (community, principal) kind:0 stream.
fn profile_lock_key(community: buzz_core::CommunityId, id: &PrincipalId) -> i64 {
    use sha2::{Digest as _, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"buzz:profile-publish:");
    hasher.update(community.as_uuid().as_bytes());
    hasher.update(id.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(bytes)
}

async fn lock_profile_publish(
    pool: &PgPool,
    community: buzz_core::CommunityId,
    id: &PrincipalId,
) -> Result<(ProfilePublishLock, ProfilePublishSnapshot)> {
    let mut tx = begin(pool).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(profile_lock_key(community, id))
        .execute(&mut *tx)
        .await?;
    let record = sqlx::query(
        "SELECT id, kind, display_name, avatar_url, username, disabled_at, purge_after \
         FROM principals WHERE id = $1",
    )
    .bind(id.as_bytes().as_slice())
    .fetch_optional(&mut *tx)
    .await?
    .as_ref()
    .map(record_from_row)
    .transpose()?;
    let latest = sqlx::query(
        "SELECT created_at, content FROM events \
         WHERE community_id = $1 AND kind = 0 AND pubkey = $2 \
           AND channel_id IS NULL AND deleted_at IS NULL \
         ORDER BY created_at DESC, id ASC LIMIT 1",
    )
    .bind(community.as_uuid())
    .bind(id.as_bytes().as_slice())
    .fetch_optional(&mut *tx)
    .await?
    .map(|row| -> Result<ProfileEventState> {
        let created_at: DateTime<Utc> = row.try_get("created_at")?;
        Ok(ProfileEventState {
            created_at: created_at.timestamp(),
            content: row.try_get("content")?,
        })
    })
    .transpose()?;
    let has_user_row: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE community_id = $1 AND pubkey = $2)",
    )
    .bind(community.as_uuid())
    .bind(id.as_bytes().as_slice())
    .fetch_one(&mut *tx)
    .await?;
    Ok((
        ProfilePublishLock { tx },
        ProfilePublishSnapshot {
            record,
            latest,
            has_user_row,
        },
    ))
}

impl crate::Db {
    /// Make `key` (the relay signing key) the relay principal; see
    /// [`ensure_relay_principal`].
    #[datastore_span(name = "ensure_relay_principal", system = "postgresql")]
    pub async fn ensure_relay_principal(&self, key: &PrincipalId) -> Result<PrincipalId> {
        ensure_relay_principal(&self.pool, key).await
    }

    /// Resolve an OIDC identity to a principal, creating it on first login.
    #[datastore_span(name = "login_identity", system = "postgresql")]
    pub async fn login_identity(
        &self,
        provider: &str,
        subject: &str,
        email: Option<&str>,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> Result<LoginPrincipal> {
        login_identity(
            &self.pool,
            provider,
            subject,
            email,
            display_name,
            avatar_url,
        )
        .await
    }

    /// Fetch a principal row.
    #[datastore_span(name = "get_principal", system = "postgresql")]
    pub async fn get_principal(&self, id: &PrincipalId) -> Result<Option<PrincipalRecord>> {
        get_principal(&self.pool, id).await
    }

    /// Update a principal's global profile and its `users` projection.
    /// Returns `None` when the principal is missing or disabled.
    #[datastore_span(name = "update_principal_profile", system = "postgresql")]
    pub async fn update_principal_profile(
        &self,
        id: &PrincipalId,
        update: &ProfileUpdate,
    ) -> Result<Option<PrincipalRecord>> {
        update_profile(&self.pool, id, update).await
    }

    /// Active communities whose `users` projection holds `id` (at most
    /// [`MAX_PROFILE_COMMUNITIES`]): where its kind:0 must be published.
    #[datastore_span(name = "principal_profile_communities", system = "postgresql")]
    pub async fn principal_profile_communities(
        &self,
        id: &PrincipalId,
    ) -> Result<Vec<ProfileCommunity>> {
        profile_communities(&self.pool, id).await
    }

    /// Take the per-(community, principal) kind:0 publish lock and read,
    /// under it, the principal row, the newest kind:0 and whether a `users`
    /// row exists. Concurrent publishers for the same pair serialize here, so
    /// the one that runs last always reads the newest profile.
    #[datastore_span(name = "lock_principal_profile_publish", system = "postgresql")]
    pub async fn lock_principal_profile_publish(
        &self,
        community: buzz_core::CommunityId,
        id: &PrincipalId,
    ) -> Result<(ProfilePublishLock, ProfilePublishSnapshot)> {
        lock_profile_publish(&self.pool, community, id).await
    }
}
