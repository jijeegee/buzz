//! Moving a room to another region (global regions S6-3, migration 0088).
//!
//! A room lives in one region's database. To move it, the operator tool
//! freezes it in the old home ([`Db::freeze_room`]: the sequence trigger then
//! refuses its events), waits for its side effects to finish, copies its rows
//! into the new home ([`copy_room`]) and checks the copy
//! ([`Db::room_fingerprint`]). The copy keeps every row as it is: rows go in
//! with `COPY` and the session's triggers off, so sequence numbers, join
//! cutoffs and unread tallies are not stamped again, and no side effect or
//! push runs twice. The switch in the primary's directory comes after (S6-4).
//!
//! Writes that do not go through events (read positions, thread follows)
//! are not frozen; one made while the copy runs stays in the old home only.

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use kuzz_core::CommunityId;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::Db;

/// SQLSTATE the sequence trigger raises for an event in a frozen room.
pub const ROOM_MOVING_SQLSTATE: &str = "KZ001";

/// How a table's rows belong to one room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Belongs {
    /// The `channels` row itself.
    Room,
    /// A `channel_id` column.
    ChannelId,
    /// An `event_id` of one of the room's events.
    EventId,
    /// `(event_created_at, event_id)` of one of the room's events.
    EventKey,
    /// A `workflow_id` of one of the room's workflows.
    WorkflowId,
}

/// The tables holding a room's rows, in the order they are copied (parents
/// first). Everything else in a region's database is not the room's:
/// accounts and the directory are copied from the primary (S5), and work
/// queues stay with the region that runs them.
pub(crate) const ROOM_TABLES: &[(&str, Belongs)] = &[
    ("channels", Belongs::Room),
    ("events", Belongs::ChannelId),
    ("channel_members", Belongs::ChannelId),
    ("channel_metadata_order", Belongs::ChannelId),
    ("event_mentions", Belongs::ChannelId),
    ("reactions", Belongs::EventKey),
    ("thread_metadata", Belongs::ChannelId),
    ("thread_follows", Belongs::ChannelId),
    ("thread_state", Belongs::ChannelId),
    ("artifact_heads", Belongs::ChannelId),
    ("artifact_revisions", Belongs::EventId),
    ("parameterized_event_watermarks", Belongs::EventId),
    ("media_references", Belongs::ChannelId),
    ("workflows", Belongs::ChannelId),
    ("workflow_runs", Belongs::WorkflowId),
    ("workflow_approvals", Belongs::WorkflowId),
    ("scheduled_workflow_fires", Belongs::WorkflowId),
];

/// The condition selecting `table`'s rows of one room. Ids are formatted
/// from typed UUIDs, never from text.
fn room_rows(belongs: Belongs, community: Uuid, channel: Uuid) -> String {
    let events = format!(
        "SELECT id FROM events WHERE community_id = '{community}' AND channel_id = '{channel}'"
    );
    let rows = match belongs {
        Belongs::Room => format!("id = '{channel}'"),
        Belongs::ChannelId => format!("channel_id = '{channel}'"),
        Belongs::EventId => format!("event_id IN ({events})"),
        Belongs::EventKey => format!(
            "(event_created_at, event_id) IN (SELECT created_at, id FROM events \
              WHERE community_id = '{community}' AND channel_id = '{channel}')"
        ),
        Belongs::WorkflowId => format!(
            "workflow_id IN (SELECT id FROM workflows \
              WHERE community_id = '{community}' AND channel_id = '{channel}')"
        ),
    };
    format!("community_id = '{community}' AND {rows}")
}

/// Where a room stands in one region's database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomHere {
    /// Not in this database.
    Absent,
    /// Taking events.
    Open,
    /// Frozen for a move: the old home during the move, or a new home's
    /// copy that is not opened yet.
    Frozen(DateTime<Utc>),
    /// Moved to another region; kept frozen until removed.
    MovedOut(DateTime<Utc>),
}

/// What a copy of a room must match: counts of the rows only events change
/// (frozen during a move) and the room's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomFingerprint {
    /// `channels.accepted_sequence`.
    pub accepted_sequence: i64,
    /// The highest `channel_sequence` of the room's events.
    pub last_sequence: i64,
    /// Rows of `events`.
    pub events: i64,
    /// Rows of `reactions`.
    pub reactions: i64,
    /// Rows of `event_mentions`.
    pub mentions: i64,
}

/// Rows copied for one table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCopied {
    /// Table name.
    pub table: &'static str,
    /// Rows written in the new home.
    pub rows: u64,
}

impl DbError {
    /// Whether the room was frozen for a move (SQLSTATE `KZ001`): the write
    /// can be tried again once the room has moved.
    pub fn is_room_moving(&self) -> bool {
        matches!(
            self,
            DbError::Sqlx(sqlx::Error::Database(db)) if db.code().as_deref() == Some(ROOM_MOVING_SQLSTATE)
        )
    }
}

impl Db {
    /// Where `channel` stands in this database.
    pub async fn room_here(&self, community: CommunityId, channel: Uuid) -> Result<RoomHere> {
        room_here(&mut *self.pool.acquire().await?, community, channel).await
    }

    /// Freeze `channel` for a move: from now on its events are refused.
    /// Waits for events being written to finish (they hold the room's row).
    /// Returns when it was frozen; freezing again keeps the first time.
    pub async fn freeze_room(
        &self,
        community: CommunityId,
        channel: Uuid,
    ) -> Result<DateTime<Utc>> {
        let since: Option<DateTime<Utc>> = sqlx::query_scalar(
            "UPDATE channels SET moving_since = COALESCE(moving_since, now()) \
              WHERE community_id = $1 AND id = $2 AND moved_out_at IS NULL \
              RETURNING moving_since",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .fetch_optional(&self.pool)
        .await?;
        since.ok_or_else(|| {
            DbError::NotFound(format!(
                "room {channel} is not in this database, or has moved out of it"
            ))
        })
    }

    /// Let `channel` take events again. A room that has moved out stays
    /// frozen. Returns whether it was frozen.
    pub async fn unfreeze_room(&self, community: CommunityId, channel: Uuid) -> Result<bool> {
        let done = sqlx::query(
            "UPDATE channels SET moving_since = NULL \
              WHERE community_id = $1 AND id = $2 AND moving_since IS NOT NULL \
                AND moved_out_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Side effects of `channel`'s events not run yet. A move waits for none.
    pub async fn room_side_effects_pending(
        &self,
        community: CommunityId,
        channel: Uuid,
    ) -> Result<i64> {
        room_side_effects_pending(&mut *self.pool.acquire().await?, community, channel).await
    }

    /// What a copy of `channel` must match ([`RoomFingerprint`]).
    pub async fn room_fingerprint(
        &self,
        community: CommunityId,
        channel: Uuid,
    ) -> Result<RoomFingerprint> {
        let row = sqlx::query(
            r#"
            SELECT (SELECT accepted_sequence FROM channels
                     WHERE community_id = $1 AND id = $2) AS accepted_sequence,
                   (SELECT COALESCE(max(channel_sequence), 0) FROM events
                     WHERE community_id = $1 AND channel_id = $2) AS last_sequence,
                   (SELECT count(*) FROM events
                     WHERE community_id = $1 AND channel_id = $2) AS events,
                   (SELECT count(*) FROM reactions r JOIN events e
                       ON e.community_id = r.community_id AND e.created_at = r.event_created_at
                      AND e.id = r.event_id
                     WHERE r.community_id = $1 AND e.channel_id = $2) AS reactions,
                   (SELECT count(*) FROM event_mentions
                     WHERE community_id = $1 AND channel_id = $2) AS mentions
            "#,
        )
        .bind(community.as_uuid())
        .bind(channel)
        .fetch_one(&self.pool)
        .await?;
        let accepted_sequence: Option<i64> = row.try_get("accepted_sequence")?;
        Ok(RoomFingerprint {
            accepted_sequence: accepted_sequence
                .ok_or_else(|| DbError::NotFound(format!("room {channel}")))?,
            last_sequence: row.try_get("last_sequence")?,
            events: row.try_get("events")?,
            reactions: row.try_get("reactions")?,
            mentions: row.try_get("mentions")?,
        })
    }

    /// Remove a copy of `channel` that was never opened (an abandoned move).
    /// Refuses a room that is open or has moved out. The caller makes sure
    /// the directory does not send the room here. Returns the rows removed.
    pub async fn delete_room_copy(&self, community: CommunityId, channel: Uuid) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        match room_here(&mut tx, community, channel).await? {
            RoomHere::Frozen(_) => {}
            RoomHere::Absent => return Ok(0),
            other => {
                return Err(DbError::InvalidData(format!(
                    "room {channel} here is {other:?}, not an unopened copy; it was not removed"
                )))
            }
        }
        triggers_off(&mut tx).await?;
        let mut removed = 0;
        for (table, belongs) in ROOM_TABLES.iter().rev() {
            let sql = format!(
                "DELETE FROM {table} WHERE {}",
                room_rows(*belongs, *community.as_uuid(), channel)
            );
            removed += sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&mut *tx)
                .await?
                .rows_affected();
        }
        tx.commit().await?;
        Ok(removed)
    }
}

async fn room_here(
    conn: &mut PgConnection,
    community: CommunityId,
    channel: Uuid,
) -> Result<RoomHere> {
    let row = sqlx::query(
        "SELECT moving_since, moved_out_at FROM channels WHERE community_id = $1 AND id = $2",
    )
    .bind(community.as_uuid())
    .bind(channel)
    .fetch_optional(conn)
    .await?;
    let Some(row) = row else {
        return Ok(RoomHere::Absent);
    };
    let moving: Option<DateTime<Utc>> = row.try_get("moving_since")?;
    let moved: Option<DateTime<Utc>> = row.try_get("moved_out_at")?;
    Ok(match (moving, moved) {
        (_, Some(at)) => RoomHere::MovedOut(at),
        (Some(since), None) => RoomHere::Frozen(since),
        (None, None) => RoomHere::Open,
    })
}

async fn room_side_effects_pending(
    conn: &mut PgConnection,
    community: CommunityId,
    channel: Uuid,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM event_side_effects s \
          WHERE s.community_id = $1 AND s.event_id IN \
                (SELECT id FROM events WHERE community_id = $1 AND channel_id = $2)",
    )
    .bind(community.as_uuid())
    .bind(channel)
    .fetch_one(conn)
    .await?)
}

/// Turn this transaction's triggers off (rules, foreign keys and guards
/// included), so rows keep their stamped values. Needs a superuser.
async fn triggers_off(conn: &mut PgConnection) -> Result<()> {
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(conn)
        .await
        .map_err(|error| {
            DbError::AccessDenied(format!(
                "moving a room needs a superuser in the new home's database \
                 (session_replication_role): {error}"
            ))
        })?;
    Ok(())
}

/// The newest migration applied in a database.
async fn schema_version(conn: &mut PgConnection) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE(max(version), 0) FROM _sqlx_migrations WHERE success")
            .fetch_one(conn)
            .await?,
    )
}

/// Columns of `table` a copy writes: every stored column, in the target's
/// order.
async fn copy_columns(conn: &mut PgConnection, table: &str) -> Result<String> {
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT quote_ident(column_name::text) FROM information_schema.columns \
          WHERE table_schema = current_schema() AND table_name = $1 AND is_generated = 'NEVER' \
          ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(conn)
    .await?;
    if columns.is_empty() {
        return Err(DbError::InvalidData(format!("table {table} not found")));
    }
    Ok(columns.join(", "))
}

/// Copy the frozen room `channel` from its old home `from` into the new home
/// `to`, all in one transaction there, from one snapshot of `from`.
///
/// Refuses unless both databases are on the same migration, the room is
/// frozen in `from` with no side effect left, and `to` does not hold it.
pub async fn copy_room(
    from: &Db,
    to: &Db,
    community: CommunityId,
    channel: Uuid,
) -> Result<Vec<TableCopied>> {
    let mut source = from.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *source)
        .await?;
    let mut target = to.pool.begin().await?;

    let (source_version, target_version) = (
        schema_version(&mut source).await?,
        schema_version(&mut target).await?,
    );
    if source_version != target_version {
        return Err(DbError::InvalidData(format!(
            "the old home is at migration {source_version} and the new home at \
             {target_version}; migrate both to the same version first"
        )));
    }
    match room_here(&mut source, community, channel).await? {
        RoomHere::Frozen(_) => {}
        other => {
            return Err(DbError::InvalidData(format!(
                "room {channel} in the old home is {other:?}; freeze it first"
            )))
        }
    }
    let pending = room_side_effects_pending(&mut source, community, channel).await?;
    if pending > 0 {
        return Err(DbError::InvalidData(format!(
            "room {channel} still has {pending} side effects to run; wait and try again"
        )));
    }
    match room_here(&mut target, community, channel).await? {
        RoomHere::Absent => {}
        other => {
            return Err(DbError::InvalidData(format!(
                "the new home already holds room {channel} ({other:?})"
            )))
        }
    }
    triggers_off(&mut target).await?;

    let mut copied = Vec::with_capacity(ROOM_TABLES.len());
    for (table, belongs) in ROOM_TABLES {
        let columns = copy_columns(&mut target, table).await?;
        let rows = room_rows(*belongs, *community.as_uuid(), channel);
        let expected: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM {table} WHERE {rows}"
        )))
        .fetch_one(&mut *source)
        .await?;
        let mut sink = target
            .copy_in_raw(&format!(
                "COPY {table} ({columns}) FROM STDIN (FORMAT binary)"
            ))
            .await?;
        {
            let mut out = source
                .copy_out_raw(&format!(
                    "COPY (SELECT {columns} FROM {table} WHERE {rows}) TO STDOUT (FORMAT binary)"
                ))
                .await?;
            while let Some(chunk) = out.next().await {
                sink.send(chunk?).await?;
            }
        }
        let written = sink.finish().await?;
        if i64::try_from(written).ok() != Some(expected) {
            return Err(DbError::InvalidData(format!(
                "{table}: {expected} rows in the old home, {written} written; nothing was kept"
            )));
        }
        copied.push(TableCopied {
            table,
            rows: written,
        });
    }
    target.commit().await?;
    source.rollback().await?;
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_rows_are_selected_by_typed_ids() {
        let community = Uuid::from_u128(1);
        let channel = Uuid::from_u128(2);
        assert_eq!(
            room_rows(Belongs::Room, community, channel),
            "community_id = '00000000-0000-0000-0000-000000000001' \
             AND id = '00000000-0000-0000-0000-000000000002'"
        );
        assert!(room_rows(Belongs::EventKey, community, channel)
            .contains("(event_created_at, event_id) IN (SELECT created_at, id FROM events"));
    }

    #[test]
    fn parents_are_copied_before_children() {
        let at = |name: &str| {
            ROOM_TABLES
                .iter()
                .position(|(table, _)| *table == name)
                .unwrap()
        };
        assert_eq!(at("channels"), 0);
        assert!(at("events") < at("reactions"));
        assert!(at("workflows") < at("workflow_runs"));
        assert!(at("workflow_runs") < at("workflow_approvals"));
        assert!(at("workflow_runs") < at("scheduled_workflow_fires"));
    }
}
