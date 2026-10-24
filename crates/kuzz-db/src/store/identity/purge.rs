//! Account purge: 30 days after a user deletes their own account, remove what
//! identifies them and keep the messages other people saw.
//!
//! The principal row stays as a disabled tombstone (`purged_at` set): its id
//! and its messaging key stay refused forever (`purged_account_keys` covers
//! key accounts whose backup row is gone), and nothing about the person is
//! left on it. Removed: external sign-ins (email, Google/Apple subject),
//! devices with their sessions and tokens, key backups, pairing requests,
//! name, photo and username, and in every community the account's own
//! profile, friend list and other personal lists, block lists and push
//! registrations. Owned agents are deleted the same way as `delete_bot` and
//! lose their names and profiles too; key-mode agent keys stay registered
//! but revoked, so they stay refused. Messages, reactions and files in rooms,
//! and other people's lists that name the account, stay.
//!
//! Only self-deletions are purged (`purge_after` set and passed); an operator
//! disable has no `purge_after`. Relay operators are skipped: removing the
//! last operator is a decision for a person, not a timer.

use kuzz_datastore_tracing::datastore_span;
use sqlx::{PgPool, Postgres, Transaction};

use kuzz_core::kind::{
    KIND_AGENT_HOST_DEVICES, KIND_AGENT_PROFILE, KIND_BOOKMARK_LIST, KIND_CONTACT_LIST,
    KIND_DEVICE_ROBOT, KIND_MUTE_LIST, KIND_PRIVATE_MANAGED_AGENT, KIND_PROFILE, KIND_PUSH_LEASE,
    KIND_READ_STATE, KIND_USER_STATUS,
};
use kuzz_core::principal::PrincipalId;

use super::{begin, lock_subject, principal_from_bytes};
use crate::error::Result;

/// Event kinds that only describe the account itself (profile, friends,
/// personal lists, app state, push registration); deleted for the account,
/// its agents and its agent keys. Nothing in a room is among them.
const PERSONAL_KINDS: [u32; 11] = [
    KIND_PROFILE,
    KIND_CONTACT_LIST,
    KIND_MUTE_LIST,
    KIND_BOOKMARK_LIST,
    KIND_AGENT_PROFILE,
    KIND_READ_STATE,
    KIND_USER_STATUS,
    KIND_PUSH_LEASE,
    KIND_PRIVATE_MANAGED_AGENT,
    KIND_AGENT_HOST_DEVICES,
    KIND_DEVICE_ROBOT,
];

/// What one purge removed (or, in a dry run, would remove).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PurgeCounts {
    /// External sign-ins (Google/Apple) removed.
    pub identities: u64,
    /// Devices removed (their sessions and refresh tokens go with them).
    pub devices: u64,
    /// Access-token rows removed.
    pub access_tokens: u64,
    /// Hosted agents deleted.
    pub bots: u64,
    /// Agent keys revoked.
    pub agent_keys: u64,
    /// Key backups removed (account and agent keys).
    pub key_backups: u64,
    /// Personal events (profile, friend list, personal lists) removed.
    pub events: u64,
    /// Per-community profile copies cleared.
    pub profiles: u64,
    /// Block, push and other per-community personal rows removed.
    pub community_rows: u64,
}

/// Outcome of trying to purge one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurgeOutcome {
    /// Personal data removed (committed, or rolled back in a dry run).
    Purged(PurgeCounts),
    /// The account is a relay operator; left for a person to remove.
    SkippedOperator,
    /// Not due any more (restored by a login, already purged, or not a
    /// self-deletion).
    NotDue,
}

/// Accounts whose self-deletion window has passed and that are not purged.
pub(super) async fn due_account_purges(pool: &PgPool, limit: i64) -> Result<Vec<PrincipalId>> {
    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT id FROM principals \
         WHERE kind = 'user' AND disabled_at IS NOT NULL AND purged_at IS NULL \
           AND purge_after IS NOT NULL AND purge_after < now() \
         ORDER BY purge_after LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter().map(|id| principal_from_bytes(id)).collect()
}

/// Purge one account in a single transaction. With `dry_run` every step runs
/// and is rolled back, so the counts are exactly what a real run removes.
pub(super) async fn purge_account(
    pool: &PgPool,
    account: &PrincipalId,
    dry_run: bool,
) -> Result<PurgeOutcome> {
    let mut tx = begin(pool).await?;
    // Same lock order as disabling: roster, then the account.
    crate::relay_operators::acquire_roster_lock(&mut tx).await?;
    lock_subject(&mut tx, account).await?;
    let id = account.as_bytes().as_slice();

    let due: Option<String> = sqlx::query_scalar(
        "SELECT identity_mode FROM principals \
         WHERE id = $1 AND kind = 'user' AND disabled_at IS NOT NULL AND purged_at IS NULL \
           AND purge_after IS NOT NULL AND purge_after < now()",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(identity_mode) = due else {
        return Ok(PurgeOutcome::NotDue);
    };
    let operator: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM relay_operators WHERE pubkey = $1)")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if operator {
        return Ok(PurgeOutcome::SkippedOperator);
    }

    let mut counts = PurgeCounts::default();

    // The key that signed the account's messages: the backup key for key
    // accounts, the principal id itself for token accounts.
    let messaging_key: Vec<u8> = if identity_mode == "key_backup" {
        sqlx::query_scalar("SELECT pubkey FROM account_key_backups WHERE account_id = $1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .unwrap_or_else(|| id.to_vec())
    } else {
        id.to_vec()
    };

    // Hosted agents: deleted like `delete_bot`, and nameless.
    let bots: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE bots SET deleted_at = COALESCE(deleted_at, now()) \
         WHERE owner_principal_id = $1 RETURNING id",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    counts.bots = bots.len() as u64;
    sqlx::query(
        "UPDATE principals SET disabled_at = COALESCE(disabled_at, now()), display_name = '', \
                avatar_url = NULL, username = NULL, locale = NULL, discoverable = false \
         WHERE id = ANY($1)",
    )
    .bind(&bots)
    .execute(&mut *tx)
    .await?;

    // Key-mode agents stay registered but revoked, so their keys stay refused.
    let agent_keys: Vec<Vec<u8>> = sqlx::query_scalar(
        "UPDATE key_registry SET revoked_at = COALESCE(revoked_at, now()) \
         WHERE principal_id = $1 RETURNING pubkey",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    counts.agent_keys = agent_keys.len() as u64;

    counts.key_backups += exec(
        &mut tx,
        "DELETE FROM agent_key_backups WHERE owner_id = $1",
        id,
    )
    .await?;
    exec(
        &mut tx,
        "DELETE FROM pairing_requests WHERE principal_id = $1",
        id,
    )
    .await?;
    counts.access_tokens = sqlx::query(
        "DELETE FROM access_tokens WHERE principal_id = $1 OR principal_id = ANY($2) \
            OR bot_id = ANY($2)",
    )
    .bind(id)
    .bind(&bots)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    counts.devices = exec(&mut tx, "DELETE FROM devices WHERE principal_id = $1", id).await?;

    // External sign-ins, and any Apple revoke token still waiting for them.
    sqlx::query(
        "DELETE FROM provider_revoke_tokens r USING identities i \
         WHERE i.principal_id = $1 AND r.provider = i.provider AND r.subject = i.subject",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    counts.identities = exec(
        &mut tx,
        "DELETE FROM identities WHERE principal_id = $1",
        id,
    )
    .await?;

    sqlx::query("INSERT INTO purged_account_keys (pubkey) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(&messaging_key)
        .execute(&mut *tx)
        .await?;
    counts.key_backups += exec(
        &mut tx,
        "DELETE FROM account_key_backups WHERE account_id = $1",
        id,
    )
    .await?;

    sqlx::query(
        "UPDATE principals SET display_name = '', avatar_url = NULL, username = NULL, \
                locale = NULL, discoverable = false, purged_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;

    // Every community: the account's and its agents' own profile and lists.
    let mut keys = vec![messaging_key.clone()];
    keys.extend(bots.iter().cloned());
    keys.extend(agent_keys.iter().cloned());
    let kinds: Vec<i32> = PERSONAL_KINDS.iter().map(|&kind| kind as i32).collect();
    counts.events = sqlx::query("DELETE FROM events WHERE pubkey = ANY($1) AND kind = ANY($2)")
        .bind(&keys)
        .bind(&kinds)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    counts.profiles = sqlx::query(
        "UPDATE users SET display_name = NULL, avatar_url = NULL, about = NULL, \
                nip05_handle = NULL, metadata_event_id = NULL, updated_at = now() \
         WHERE pubkey = ANY($1)",
    )
    .bind(&keys)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    for statement in [
        "DELETE FROM user_blocks WHERE blocker = ANY($1)",
        "DELETE FROM user_block_periods WHERE blocker = ANY($1)",
        "DELETE FROM push_wake_outbox WHERE author = ANY($1)",
        "DELETE FROM push_leases WHERE author = ANY($1)",
        "DELETE FROM group_add_settings WHERE pubkey = ANY($1)",
        "DELETE FROM thread_follows WHERE pubkey = ANY($1)",
    ] {
        counts.community_rows += sqlx::query(statement)
            .bind(&keys)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }

    if dry_run {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(PurgeOutcome::Purged(counts))
}

async fn exec(tx: &mut Transaction<'_, Postgres>, sql: &'static str, id: &[u8]) -> Result<u64> {
    Ok(sqlx::query(sql)
        .bind(id)
        .execute(&mut **tx)
        .await?
        .rows_affected())
}

impl crate::Db {
    /// Up to `limit` accounts whose self-deletion window has passed.
    #[datastore_span(name = "due_account_purges", system = "postgresql")]
    pub async fn due_account_purges(&self, limit: i64) -> Result<Vec<PrincipalId>> {
        due_account_purges(&self.pool, limit).await
    }

    /// Purge one self-deleted account whose window has passed (see module
    /// docs). `dry_run` rolls everything back and reports what it would do.
    #[datastore_span(name = "purge_account", system = "postgresql")]
    pub async fn purge_account(
        &self,
        account: &PrincipalId,
        dry_run: bool,
    ) -> Result<PurgeOutcome> {
        purge_account(&self.pool, account, dry_run).await
    }
}
