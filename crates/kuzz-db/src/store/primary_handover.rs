//! Handing the primary role to another region (global regions S6-5).
//!
//! A planned handover from the old primary to a copy region, run by
//! `kuzz-admin primary hand-over`. Global writes stop for a short while:
//! 1. The old primary is guarded like a copy ([`Db::guard_global_copy`]): from
//!    then on its global tables refuse local writes.
//! 2. Every subscriber of its publication catches up to the old primary's WAL
//!    position at that moment ([`Db::slots_behind`]).
//! 3. The new primary leaves its subscription ([`Db::leave_global_copy`]),
//!    drops its guards ([`Db::unguard_global_copy`]) and publishes.
//! 4. Every other region, the old primary included, subscribes to the new
//!    primary without copying again ([`Db::subscribe_global_copy`]): nothing
//!    global changed since it caught up.
//! 5. The new primary marks itself the primary in `regions`
//!    ([`Db::mark_primary_region`]). The change reaches every region through
//!    the new subscriptions.
//!
//! The relays then restart with the new `KUZZ_PRIMARY_REGION`.

use sqlx::Row;

use super::global_copy::{COPIED_TABLES, GUARD, LIVE_TRIGGER, PUBLICATION, SUBSCRIPTION};
use crate::error::{DbError, Result};
use crate::Db;

/// This database's subscription to the global copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSubscription {
    /// It is enabled.
    pub enabled: bool,
    /// Its replication slot in the publisher's database, if it has one.
    pub slot: Option<String>,
    /// The publisher's host and port, from the connection string (no
    /// password).
    pub publisher: String,
}

/// A logical replication slot in this database that has not confirmed a
/// WAL position yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotBehind {
    /// Slot name.
    pub slot: String,
    /// Whether a subscriber is connected to it now.
    pub active: bool,
    /// Bytes of WAL it has not confirmed; `None` if it confirmed nothing yet.
    pub behind_bytes: Option<i64>,
}

/// The slot name a region's subscription uses in the primary's database:
/// one per region, so several regions can subscribe to one primary.
pub fn slot_name(region: &str) -> Result<String> {
    let valid = region
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && region.starts_with(|c: char| c.is_ascii_lowercase())
        && region.len() <= 16;
    if !valid {
        return Err(DbError::InvalidData(format!("not a region id: {region:?}")));
    }
    Ok(format!("{SUBSCRIPTION}_{}", region.replace('-', "_")))
}

/// Host and port of a libpq connection string, without the password.
pub fn publisher_of(conninfo: &str) -> String {
    let field = |name: &str| {
        conninfo
            .split_whitespace()
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .unwrap_or("")
            .trim_matches('\'')
            .to_owned()
    };
    let (host, port) = (field("host"), field("port"));
    if port.is_empty() {
        host
    } else {
        format!("{host}:{port}")
    }
}

impl Db {
    /// The WAL position now, as text (`0/16B3748`).
    pub async fn wal_position(&self) -> Result<String> {
        Ok(sqlx::query_scalar("SELECT pg_current_wal_lsn()::text")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Logical replication slots in this database that have not confirmed
    /// `lsn` yet: subscribers that have not caught up to it.
    pub async fn slots_behind(&self, lsn: &str) -> Result<Vec<SlotBehind>> {
        let rows = sqlx::query(
            "SELECT slot_name::text AS slot, active, \
                    pg_wal_lsn_diff($1::pg_lsn, confirmed_flush_lsn)::bigint AS behind \
               FROM pg_replication_slots \
              WHERE slot_type = 'logical' AND database = current_database() \
                AND (confirmed_flush_lsn IS NULL OR confirmed_flush_lsn < $1::pg_lsn) \
              ORDER BY slot_name",
        )
        .bind(lsn)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(SlotBehind {
                    slot: row.try_get("slot")?,
                    active: row.try_get("active")?,
                    behind_bytes: row.try_get("behind")?,
                })
            })
            .collect()
    }

    /// This database's subscription to the global copy, if any.
    pub async fn global_subscription(&self) -> Result<Option<GlobalSubscription>> {
        let row = sqlx::query(
            "SELECT s.subenabled, s.subslotname::text AS slot, s.subconninfo \
               FROM pg_subscription s JOIN pg_database d ON d.oid = s.subdbid \
              WHERE d.datname = current_database() AND s.subname = $1",
        )
        .bind(SUBSCRIPTION)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let conninfo: String = row.try_get("subconninfo")?;
            Ok(GlobalSubscription {
                enabled: row.try_get("subenabled")?,
                slot: row.try_get("slot")?,
                publisher: publisher_of(&conninfo),
            })
        })
        .transpose()
    }

    /// Whether this database publishes the global copy.
    pub async fn publishes_global_copy(&self) -> Result<bool> {
        Ok(
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_publication WHERE pubname = $1)")
                .bind(PUBLICATION)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// Drop this database's subscription, keeping its slot in the
    /// publisher (the publisher may be unreachable from here; remove the slot
    /// there with [`Db::drop_slot`]). Returns the slot it used, if any.
    pub async fn leave_global_copy(&self) -> Result<Option<String>> {
        let Some(subscription) = self.global_subscription().await? else {
            return Ok(None);
        };
        // Each statement runs on its own: DROP SUBSCRIPTION refuses a
        // transaction block.
        for sql in [
            format!("ALTER SUBSCRIPTION {SUBSCRIPTION} DISABLE"),
            format!("ALTER SUBSCRIPTION {SUBSCRIPTION} SET (slot_name = NONE)"),
            format!("DROP SUBSCRIPTION {SUBSCRIPTION}"),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&self.pool)
                .await?;
        }
        Ok(subscription.slot)
    }

    /// Remove a replication slot nobody uses any more. Returns whether it
    /// existed.
    pub async fn drop_slot(&self, slot: &str) -> Result<bool> {
        let active: Option<bool> =
            sqlx::query_scalar("SELECT active FROM pg_replication_slots WHERE slot_name = $1")
                .bind(slot)
                .fetch_optional(&self.pool)
                .await?;
        match active {
            None => Ok(false),
            Some(true) => Err(DbError::InvalidData(format!(
                "replication slot {slot} is still in use; nothing was removed"
            ))),
            Some(false) => {
                sqlx::query("SELECT pg_drop_replication_slot($1)")
                    .bind(slot)
                    .execute(&self.pool)
                    .await?;
                Ok(true)
            }
        }
    }

    /// Stop publishing the global copy (the old primary, once every region
    /// subscribes to the new one).
    pub async fn unpublish_global_copy(&self) -> Result<()> {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP PUBLICATION IF EXISTS {PUBLICATION}"
        )))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Promote a copy: drop the write guards and the copy's live trigger and
    /// mark the database's global tables its own. Refused while it still
    /// subscribes to a global copy.
    pub async fn unguard_global_copy(&self) -> Result<()> {
        if self.global_subscription().await?.is_some() {
            return Err(DbError::InvalidData(
                "this database still subscribes to the global copy; leave it first".into(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        for table in COPIED_TABLES {
            for name in [GUARD.to_owned(), format!("{GUARD}_truncate")] {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "DROP TRIGGER IF EXISTS {name} ON {table}"
                )))
                .execute(&mut *tx)
                .await?;
            }
        }
        for name in [
            format!("{GUARD}_insert"),
            format!("{GUARD}_update"),
            format!("{GUARD}_delete"),
            LIVE_TRIGGER.to_owned(),
        ] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP TRIGGER IF EXISTS {name} ON events"
            )))
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE local_region SET global_copy = false")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Subscribe this database to the global copy published at `conninfo`
    /// (a libpq connection string the database server itself can use),
    /// without copying the tables again: this database must already hold
    /// everything the publisher has. The slot is named after `region`.
    pub async fn subscribe_global_copy(&self, conninfo: &str, region: &str) -> Result<()> {
        let slot = slot_name(region)?;
        let quoted: String = sqlx::query_scalar("SELECT quote_literal($1)")
            .bind(conninfo)
            .fetch_one(&self.pool)
            .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE SUBSCRIPTION {SUBSCRIPTION} CONNECTION {quoted} PUBLICATION {PUBLICATION} \
             WITH (copy_data = false, slot_name = '{slot}')"
        )))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark `region` the primary in `regions`. Run in the new primary once
    /// it publishes; the change reaches every region through the copy.
    pub async fn mark_primary_region(&self, region: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM regions WHERE id = $1)")
            .bind(region)
            .fetch_one(&mut *tx)
            .await?;
        if !known {
            return Err(DbError::NotFound(format!("region {region}")));
        }
        // One primary at a time (`regions_one_primary`): clear, then set.
        sqlx::query("UPDATE regions SET is_primary = false WHERE is_primary AND id <> $1")
            .bind(region)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE regions SET is_primary = true WHERE id = $1 AND NOT is_primary")
            .bind(region)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The region `regions` marks as the primary, as this database holds it.
    pub async fn listed_primary_region(&self) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT id FROM regions WHERE is_primary")
                .fetch_optional(&self.pool)
                .await?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_region_has_its_own_slot() {
        assert_eq!(slot_name("jp").unwrap(), "kuzz_global_jp");
        assert_eq!(slot_name("us-east").unwrap(), "kuzz_global_us_east");
        assert!(slot_name("jp'; DROP").is_err());
        assert!(slot_name("1jp").is_err());
    }

    #[test]
    fn the_publisher_is_shown_without_the_password() {
        assert_eq!(
            publisher_of("host=postgres-kr port=5432 user=kuzz password=secret dbname=kuzz"),
            "postgres-kr:5432"
        );
        assert_eq!(publisher_of("host=db.example dbname=kuzz"), "db.example");
    }
}
