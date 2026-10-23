//! Moving a room to another region (global regions S6-3, migration 0088).
//!
//! A room lives in one region's database. To move it, the operator tool
//! freezes it in the old home ([`Db::freeze_room`]: the sequence trigger then
//! refuses its events, except the relay's own room messages and group
//! snapshots while the room's side effects drain), waits for its side effects
//! to finish, copies its rows into the new home ([`copy_room`], which checks
//! every table's rows against the old home's) and checks the copy again
//! ([`Db::room_fingerprint`]). The copy keeps every row as it is: rows go in
//! with `COPY` and the session's triggers off, so sequence numbers, join
//! cutoffs and unread tallies are not stamped again, and no side effect or
//! push runs twice. The copy is marked (`moved_in_at`) until it is opened;
//! only a marked copy is ever removed by an abandoned move.
//!
//! Then (S6-4) the primary's directory homes the room in the new region
//! ([`Db::switch_room_home`]), the copy is opened ([`Db::unfreeze_room`]) and
//! the old home's room is marked moved out ([`Db::mark_room_moved_out`]),
//! read-only until it is removed ([`Db::forget_moved_room`]). Every step that
//! could split a room checks the directory as the database holds it: a room
//! the directory homes elsewhere is never opened, and one it homes here is
//! never removed.
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
    /// Takes the room's event lock, so events being written finish first.
    /// Returns when it was frozen; freezing again keeps the first time.
    pub async fn freeze_room(
        &self,
        community: CommunityId,
        channel: Uuid,
    ) -> Result<DateTime<Utc>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended(\
                 'kuzz_channel_membership:' || $1::text || ':' || $2::text, 0))",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .execute(&mut *tx)
        .await?;
        let since: Option<DateTime<Utc>> = sqlx::query_scalar(
            "UPDATE channels SET moving_since = COALESCE(moving_since, now()) \
              WHERE community_id = $1 AND id = $2 AND moved_out_at IS NULL \
              RETURNING moving_since",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        since.ok_or_else(|| {
            DbError::NotFound(format!(
                "room {channel} is not in this database, or has moved out of it"
            ))
        })
    }

    /// Let `channel` take events again: an abandoned move in the old home,
    /// or the copy in the new home once the directory homes it there. A room
    /// that has moved out stays frozen, and so does one the directory (this
    /// database's copy of it) homes in another region. Returns whether it was
    /// frozen.
    pub async fn unfreeze_room(&self, community: CommunityId, channel: Uuid) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let elsewhere: Option<String> = sqlx::query_scalar(
            "SELECT d.home_region FROM room_directory d JOIN local_region l \
                ON l.region_id <> d.home_region \
              WHERE d.community_id = $1 AND d.channel_id = $2",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(home) = elsewhere {
            return Err(DbError::InvalidData(format!(
                "the directory homes room {channel} in region {home}; it stays frozen here"
            )));
        }
        let done = sqlx::query(
            "UPDATE channels SET moving_since = NULL, moved_in_at = NULL \
              WHERE community_id = $1 AND id = $2 AND moving_since IS NOT NULL \
                AND moved_out_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
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
    /// Only a copy [`copy_room`] wrote and nothing opened since is removed:
    /// never the old home (frozen during a move too), and never a room the
    /// directory homes in this database's region. Returns the rows removed.
    pub async fn delete_room_copy(&self, community: CommunityId, channel: Uuid) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let copy: Option<bool> = sqlx::query_scalar(
            "SELECT moved_in_at IS NOT NULL AND moving_since IS NOT NULL \
                    AND moved_out_at IS NULL \
               FROM channels WHERE community_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .fetch_optional(&mut *tx)
        .await?;
        match copy {
            None => return Ok(0),
            Some(true) => {}
            Some(false) => {
                let here = room_here(&mut tx, community, channel).await?;
                return Err(DbError::InvalidData(format!(
                    "room {channel} here is {here:?}, not an unopened copy; it was not removed"
                )));
            }
        }
        refuse_if_homed_here(&mut tx, community, channel).await?;
        let removed = delete_room_rows(&mut tx, community, channel).await?;
        tx.commit().await?;
        Ok(removed)
    }

    /// Mark the old home's frozen room as moved out once the directory homes
    /// it elsewhere (S6-4): it stays frozen and read-only, and its workflows
    /// stop running here. Refused while the directory (this database's copy
    /// of it) still homes the room here. Marking again changes nothing.
    pub async fn mark_room_moved_out(&self, community: CommunityId, channel: Uuid) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        match room_here(&mut tx, community, channel).await? {
            RoomHere::Frozen(_) => {}
            RoomHere::MovedOut(_) => return Ok(()),
            other => {
                return Err(DbError::InvalidData(format!(
                    "room {channel} here is {other:?}, not frozen for a move"
                )))
            }
        }
        refuse_if_homed_here(&mut tx, community, channel).await?;
        sqlx::query(
            "UPDATE channels SET moved_out_at = now() \
              WHERE community_id = $1 AND id = $2 AND moved_out_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE workflows SET enabled = false WHERE community_id = $1 AND channel_id = $2",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Remove the rows of a room that moved out of this database at least
    /// `kept_days` ago (S6-4). Refused for any other room, and while the
    /// directory homes it here. Returns the rows removed.
    pub async fn forget_moved_room(
        &self,
        community: CommunityId,
        channel: Uuid,
        kept_days: u32,
    ) -> Result<u64> {
        let kept_for = chrono::Duration::days(i64::from(kept_days));
        let mut tx = self.pool.begin().await?;
        match room_here(&mut tx, community, channel).await? {
            RoomHere::MovedOut(at) if at <= Utc::now() - kept_for => {}
            RoomHere::MovedOut(at) => {
                return Err(DbError::InvalidData(format!(
                    "room {channel} moved out at {at}; it is kept until {}",
                    at + kept_for
                )))
            }
            RoomHere::Absent => return Ok(0),
            other => {
                return Err(DbError::InvalidData(format!(
                    "room {channel} here is {other:?}, not moved out; it was not removed"
                )))
            }
        }
        refuse_if_homed_here(&mut tx, community, channel).await?;
        let removed = delete_room_rows(&mut tx, community, channel).await?;
        tx.commit().await?;
        Ok(removed)
    }
}

/// Refuse when the directory, as this database holds it, homes `channel` in
/// this database's own region. A database no region has claimed has no home
/// to protect.
async fn refuse_if_homed_here(
    conn: &mut PgConnection,
    community: CommunityId,
    channel: Uuid,
) -> Result<()> {
    let here: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM room_directory d JOIN local_region l \
                           ON l.region_id = d.home_region \
                        WHERE d.community_id = $1 AND d.channel_id = $2)",
    )
    .bind(community.as_uuid())
    .bind(channel)
    .fetch_one(conn)
    .await?;
    if here {
        return Err(DbError::InvalidData(format!(
            "the directory homes room {channel} in this database's region; nothing was changed"
        )));
    }
    Ok(())
}

/// Delete every row of `channel`, children first, with triggers off so no
/// cascade or directory trigger runs.
async fn delete_room_rows(
    conn: &mut PgConnection,
    community: CommunityId,
    channel: Uuid,
) -> Result<u64> {
    triggers_off(conn).await?;
    let mut removed = 0;
    for (table, belongs) in ROOM_TABLES.iter().rev() {
        let sql = format!(
            "DELETE FROM {table} WHERE {}",
            room_rows(*belongs, *community.as_uuid(), channel)
        );
        removed += sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut *conn)
            .await?
            .rows_affected();
    }
    Ok(removed)
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

/// Render values the same way in both databases, whatever their settings,
/// so their checksums compare.
async fn same_text(conn: &mut PgConnection) -> Result<()> {
    for setting in [
        "SET LOCAL TimeZone = 'UTC'",
        "SET LOCAL DateStyle = 'ISO, YMD'",
        "SET LOCAL IntervalStyle = 'postgres'",
        "SET LOCAL extra_float_digits = 3",
        "SET LOCAL bytea_output = 'hex'",
    ] {
        sqlx::query(setting).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Rows of `table` selected by `rows`, and a checksum of their values.
async fn table_checksum(
    conn: &mut PgConnection,
    table: &str,
    columns: &str,
    rows: &str,
) -> Result<(i64, String)> {
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) AS n, COALESCE(md5(string_agg(h, '' ORDER BY h)), '') AS sum \
           FROM (SELECT md5(ROW({columns})::text) AS h FROM {table} WHERE {rows}) r"
    )))
    .fetch_one(conn)
    .await?;
    Ok((row.try_get("n")?, row.try_get("sum")?))
}

/// Copy the frozen room `channel` from its old home `from` into the new home
/// `to`, all in one transaction there, from one snapshot of `from`. Every
/// table's copied rows must match the old home's, value for value, or
/// nothing is kept. The copy stays frozen and marked as a move's copy.
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
    // With triggers off the community's write fence does not run: check it
    // once here instead.
    sqlx::query("SELECT assert_community_write_allowed($1)")
        .bind(community.as_uuid())
        .execute(&mut *target)
        .await?;
    triggers_off(&mut target).await?;
    for conn in [&mut *source, &mut *target] {
        same_text(conn).await?;
    }

    let mut copied = Vec::with_capacity(ROOM_TABLES.len());
    for (table, belongs) in ROOM_TABLES {
        let columns = copy_columns(&mut target, table).await?;
        let rows = room_rows(*belongs, *community.as_uuid(), channel);
        let expected = table_checksum(&mut source, table, &columns, &rows).await?;
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
        let landed = table_checksum(&mut target, table, &columns, &rows).await?;
        if landed != expected || i64::try_from(written).ok() != Some(expected.0) {
            return Err(DbError::InvalidData(format!(
                "{table}: the copy does not match the old home ({} rows there, {written} \
                 written, {} here); nothing was kept",
                expected.0, landed.0
            )));
        }
        copied.push(TableCopied {
            table,
            rows: written,
        });
    }
    sqlx::query("UPDATE channels SET moved_in_at = now() WHERE community_id = $1 AND id = $2")
        .bind(community.as_uuid())
        .bind(channel)
        .execute(&mut *target)
        .await?;
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
