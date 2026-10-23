//! Global copy (global regions stage S5-1, migration 0087).
//!
//! Global data is written only in the primary region. Postgres logical
//! replication copies it to every other region: the primary publishes
//! [`COPIED_TABLES`] and the room-less rows of `events` as [`PUBLICATION`],
//! and each other region's database subscribes to it as [`SUBSCRIPTION`].
//!
//! - The primary relay creates or updates the publication at startup
//!   ([`Db::publish_global_copy`]). `events` is published with a row filter,
//!   which Postgres allows for updates and deletes only when the filtered
//!   column is part of the replica identity: `events` and every partition
//!   get `REPLICA IDENTITY FULL` (new partitions follow their parent).
//! - A copy's relay waits until the first copy is done
//!   ([`Db::global_copy_progress`]), then installs the write guards and
//!   marks the database a copy ([`Db::guard_global_copy`]). From then on only
//!   replication writes the copied tables; anything else fails.
//! - The primary updates `global_heartbeat` every second
//!   ([`Db::beat_global_copy`]); a copy reads how old its heartbeat is
//!   ([`Db::global_copy_lag`]).
//!
//! The subscription itself (and the network path to the primary's database)
//! is set up by the operator: creating one needs rights the relay's role
//! does not normally have.
//!
//! Every table of the schema is in exactly one of [`COPIED_TABLES`] and
//! [`LOCAL_TABLES`] (`events` is in both lists' sense: room-less rows are
//! copied, room rows are local); a test keeps the lists complete, so a new
//! table has to be placed deliberately.

use kuzz_datastore_tracing::datastore_span;
use sqlx::{PgConnection, Row};

use crate::error::{DbError, Result};
use crate::Db;

/// Publication in the primary.
pub const PUBLICATION: &str = "kuzz_global";
/// Subscription in each other region.
pub const SUBSCRIPTION: &str = "kuzz_global";

/// Tables copied whole from the primary.
pub const COPIED_TABLES: &[&str] = &[
    // Regions and where rooms live.
    "regions",
    "room_directory",
    "global_heartbeat",
    // Communities and who belongs to them.
    "communities",
    "community_bans",
    "relay_members",
    "relay_invites",
    "join_policy_acceptances",
    "relay_operators",
    "pubkey_allowlist",
    "archived_identities",
    "moderation_reports",
    "moderation_actions",
    // Accounts and sign-in.
    "principals",
    "identities",
    "devices",
    "sessions",
    "refresh_tokens",
    "access_tokens",
    "bots",
    "account_key_backups",
    "api_tokens",
    // Profiles and personal data.
    "users",
    "push_leases",
    "git_repo_names",
];

/// `events` rows copied from the primary: the room-less ones (profiles,
/// contact lists, settings, agent records).
pub const COPIED_EVENTS_FILTER: &str = "channel_id IS NULL";

/// Tables every region keeps for itself. `events` is listed here for its
/// room rows; its room-less rows are copied ([`COPIED_EVENTS_FILTER`]).
pub const LOCAL_TABLES: &[&str] = &[
    // Schema bookkeeping and this database's own region.
    "_operator_global_tables",
    "local_region",
    "replica_heartbeat",
    // Rooms and their messages (the room's home region).
    "channels",
    "channel_members",
    "channel_metadata_order",
    "events",
    "event_mentions",
    "event_side_effects",
    "parameterized_event_watermarks",
    "reactions",
    "thread_follows",
    "thread_metadata",
    "thread_state",
    "artifact_heads",
    "artifact_revisions",
    "media_references",
    "media_uploads",
    "workflows",
    "workflow_runs",
    "workflow_approvals",
    "scheduled_workflow_fires",
    "subscriptions",
    // Blocks: written in every region at once (S4-5b), so each region's
    // block triggers run for its own rooms.
    "user_blocks",
    "user_block_periods",
    "user_block_versions",
    // Work queues and logs of this region's own workers.
    "region_outbox",
    "relay_admin_actions",
    "relay_admin_outbox",
    "relay_operator_audit",
    "operator_listener_outbox",
    "operator_listener_pubkeys",
    "push_match_queue",
    "push_wake_outbox",
    "push_gateway_challenges",
    "push_gateway_delegations",
    "push_gateway_delivery_auth_replays",
    "push_gateway_delivery_request_replays",
    "push_gateway_endpoint_quotas",
    "push_gateway_installations",
    "delivery_log",
    "audit_log",
    "rate_limit_violations",
    "product_feedback",
    "storage_accounting_snapshots",
    "storage_taxonomy_sweeps",
    // Community deletion runs in each region for its own data (S6 makes
    // the primary lead it).
    "community_deletion_requests",
    "community_deletion_approvals",
    "community_deletion_checkpoints",
    "community_deletion_executor_heartbeats",
    "community_deletion_manifest_keys",
    "community_serving_write_leases",
];

/// Name of the guard triggers on copied tables.
const GUARD: &str = "kuzz_global_copy_guard";

/// How far a copy's subscription has come.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CopyProgress {
    /// The subscription exists in this database.
    pub subscribed: bool,
    /// It is enabled.
    pub enabled: bool,
    /// Copied tables (counting `events` once) the subscription has finished
    /// copying and now streams.
    pub ready: usize,
    /// Copied tables the subscription does not include yet: the operator
    /// has to refresh it (`ALTER SUBSCRIPTION … REFRESH PUBLICATION`).
    pub missing: Vec<String>,
}

impl CopyProgress {
    /// Every copied table has had its first copy and is streaming.
    pub fn is_ready(&self) -> bool {
        self.subscribed && self.enabled && self.missing.is_empty() && self.ready == expected_count()
    }
}

/// Copied tables plus `events`.
fn expected_count() -> usize {
    COPIED_TABLES.len() + 1
}

/// The publication's table list: every copied table and the filtered events.
fn publication_tables() -> String {
    let mut list = COPIED_TABLES.join(", ");
    list.push_str(&format!(", events WHERE ({COPIED_EVENTS_FILTER})"));
    list
}

impl Db {
    /// Whether this database's global tables are a copy (`local_region`).
    #[datastore_span(name = "is_global_copy", system = "postgresql")]
    pub async fn is_global_copy(&self) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT kuzz_is_global_copy()")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Primary: publish the global tables. Gives `events` and its partitions
    /// the replica identity the row filter needs, then creates the
    /// publication or brings its table list up to date. Refuses on a
    /// database marked as a copy: a copy is promoted by the operator first.
    #[datastore_span(name = "publish_global_copy", system = "postgresql")]
    pub async fn publish_global_copy(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        publish_global_copy_on(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Copy: how far the subscription has come.
    #[datastore_span(name = "global_copy_progress", system = "postgresql")]
    pub async fn global_copy_progress(&self) -> Result<CopyProgress> {
        let subscription = sqlx::query(
            "SELECT s.oid, s.subenabled FROM pg_subscription s \
               JOIN pg_database d ON d.oid = s.subdbid \
              WHERE d.datname = current_database() AND s.subname = $1",
        )
        .bind(SUBSCRIPTION)
        .fetch_optional(&self.pool)
        .await?;
        let Some(subscription) = subscription else {
            return Ok(CopyProgress::default());
        };
        let oid: sqlx::postgres::types::Oid = subscription.try_get("oid")?;
        let states: Vec<(String, String)> = sqlx::query_as(
            "SELECT c.relname::text, r.srsubstate::text FROM pg_subscription_rel r \
               JOIN pg_class c ON c.oid = r.srrelid \
              WHERE r.srsubid = $1",
        )
        .bind(oid)
        .fetch_all(&self.pool)
        .await?;
        let state_of = |table: &str| {
            states
                .iter()
                .find(|(name, _)| name == table)
                .map(|(_, state)| state.as_str())
        };
        let mut progress = CopyProgress {
            subscribed: true,
            enabled: subscription.try_get("subenabled")?,
            ..CopyProgress::default()
        };
        for table in COPIED_TABLES.iter().copied().chain(["events"]) {
            match state_of(table) {
                // `r`: copied and streaming.
                Some("r") => progress.ready += 1,
                Some(_) => {}
                None => progress.missing.push(table.to_owned()),
            }
        }
        Ok(progress)
    }

    /// Copy: refuse local writes to the copied tables and mark this
    /// database a copy. Call once the first copy is done; idempotent.
    #[datastore_span(name = "guard_global_copy", system = "postgresql")]
    pub async fn guard_global_copy(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        guard_global_copy_on(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Primary: record that the copy source is alive now.
    #[datastore_span(name = "beat_global_copy", system = "postgresql")]
    pub async fn beat_global_copy(&self) -> Result<()> {
        sqlx::query(
            "INSERT INTO global_heartbeat (beat_at) VALUES (clock_timestamp()) \
             ON CONFLICT (singleton) DO UPDATE SET beat_at = EXCLUDED.beat_at",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Copy: seconds since the newest primary heartbeat this copy holds, or
    /// `None` before the first one arrived.
    #[datastore_span(name = "global_copy_lag", system = "postgresql")]
    pub async fn global_copy_lag(&self) -> Result<Option<f64>> {
        Ok(sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM clock_timestamp() - beat_at)::float8 FROM global_heartbeat",
        )
        .fetch_optional(&self.pool)
        .await?)
    }
}

pub(crate) async fn publish_global_copy_on(connection: &mut PgConnection) -> Result<()> {
    let is_copy: bool = sqlx::query_scalar("SELECT kuzz_is_global_copy()")
        .fetch_one(&mut *connection)
        .await?;
    if is_copy {
        return Err(DbError::InvalidData(
            "this database is a copy of the global tables; promote it before running it as the primary"
                .into(),
        ));
    }
    let lacking: Vec<String> = sqlx::query_scalar(
        "SELECT t.relid::regclass::text FROM pg_partition_tree('events') t \
           JOIN pg_class c ON c.oid = t.relid \
          WHERE c.relkind IN ('r', 'p') AND c.relreplident <> 'f'",
    )
    .fetch_all(&mut *connection)
    .await?;
    for table in lacking {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER TABLE {table} REPLICA IDENTITY FULL"
        )))
        .execute(&mut *connection)
        .await?;
    }
    let published: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT tablename::text, rowfilter FROM pg_publication_tables WHERE pubname = $1",
    )
    .bind(PUBLICATION)
    .fetch_all(&mut *connection)
    .await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_publication WHERE pubname = $1)")
            .bind(PUBLICATION)
            .fetch_one(&mut *connection)
            .await?;
    let up_to_date = published.len() == expected_count()
        && COPIED_TABLES.iter().all(|table| {
            published
                .iter()
                .any(|(name, filter)| name == table && filter.is_none())
        })
        && published
            .iter()
            .any(|(name, filter)| name == "events" && filter.is_some());
    if !exists {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE PUBLICATION {PUBLICATION} FOR TABLE {} \
             WITH (publish_via_partition_root = true)",
            publication_tables()
        )))
        .execute(&mut *connection)
        .await?;
    } else if !up_to_date {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER PUBLICATION {PUBLICATION} SET TABLE {}",
            publication_tables()
        )))
        .execute(&mut *connection)
        .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "ALTER PUBLICATION {PUBLICATION} SET (publish_via_partition_root = true)"
        )))
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// Whether this database is a copy, on a connection; `false` before
/// migration 0087 (startup probes also run against older schemas in tests).
pub(crate) async fn is_global_copy_on(connection: &mut PgConnection) -> Result<bool> {
    let known: bool =
        sqlx::query_scalar("SELECT to_regprocedure('kuzz_is_global_copy()') IS NOT NULL")
            .fetch_one(&mut *connection)
            .await?;
    if !known {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT kuzz_is_global_copy()")
        .fetch_one(&mut *connection)
        .await?)
}

/// A community for a startup probe that runs in a transaction it rolls
/// back. The primary (or a single region) makes a scratch community; a copy
/// may not write `communities`, so it borrows a copied one. `None` when a
/// copy holds no community yet.
pub(crate) async fn probe_community_on(
    connection: &mut PgConnection,
    host_prefix: &str,
) -> Result<Option<uuid::Uuid>> {
    if is_global_copy_on(connection).await? {
        return Ok(sqlx::query_scalar(
            "SELECT id FROM communities ORDER BY created_at, id LIMIT 1",
        )
        .fetch_optional(&mut *connection)
        .await?);
    }
    let community = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(community)
        .bind(format!("{host_prefix}-{}.invalid", community.simple()))
        .execute(&mut *connection)
        .await?;
    Ok(Some(community))
}

pub(crate) async fn guard_global_copy_on(connection: &mut PgConnection) -> Result<()> {
    for table in COPIED_TABLES {
        for sql in [
            format!(
                "CREATE OR REPLACE TRIGGER {GUARD} BEFORE INSERT OR UPDATE OR DELETE ON {table} \
                 FOR EACH ROW EXECUTE FUNCTION refuse_global_copy_write()"
            ),
            format!(
                "CREATE OR REPLACE TRIGGER {GUARD}_truncate BEFORE TRUNCATE ON {table} \
                 FOR EACH STATEMENT EXECUTE FUNCTION refuse_global_copy_write()"
            ),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&mut *connection)
                .await?;
        }
    }
    for sql in [
        format!(
            "CREATE OR REPLACE TRIGGER {GUARD}_insert BEFORE INSERT ON events \
             FOR EACH ROW WHEN (NEW.channel_id IS NULL) EXECUTE FUNCTION refuse_global_copy_write()"
        ),
        format!(
            "CREATE OR REPLACE TRIGGER {GUARD}_update BEFORE UPDATE ON events \
             FOR EACH ROW WHEN (OLD.channel_id IS NULL OR NEW.channel_id IS NULL) \
             EXECUTE FUNCTION refuse_global_copy_write()"
        ),
        format!(
            "CREATE OR REPLACE TRIGGER {GUARD}_delete BEFORE DELETE ON events \
             FOR EACH ROW WHEN (OLD.channel_id IS NULL) EXECUTE FUNCTION refuse_global_copy_write()"
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut *connection)
            .await?;
    }
    let marked = sqlx::query("UPDATE local_region SET global_copy = true")
        .execute(&mut *connection)
        .await?;
    if marked.rows_affected() == 0 {
        return Err(DbError::InvalidData(
            "claim this database for its region before marking it a copy".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_table_is_both_copied_and_local() {
        for table in COPIED_TABLES {
            assert!(!LOCAL_TABLES.contains(table), "{table} is in both lists");
        }
        let mut all: Vec<_> = COPIED_TABLES.iter().chain(LOCAL_TABLES).collect();
        let count = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), count, "a table is listed twice");
    }

    #[test]
    fn events_are_published_with_their_filter() {
        let list = publication_tables();
        assert!(
            list.ends_with("events WHERE (channel_id IS NULL)"),
            "{list}"
        );
        assert!(list.starts_with("regions, room_directory, global_heartbeat"));
    }
}
