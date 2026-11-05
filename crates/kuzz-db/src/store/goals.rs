//! Goal rows: one row per goal, one link row per linked thread or message.
//!
//! A write is one signed kind 40111 event carrying [`GoalOp`]s. Under a room
//! lock the ops apply to the rows and the event is stored in one transaction,
//! so the stored change log and the rows cannot disagree. Deleted goals are
//! really gone; there is nothing to restore. Rules that need the room's shape
//! live in [`kuzz_core::goal_store`].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use kuzz_core::goal_store::{self, GoalGraph, GoalShape};
use kuzz_core::goal_tree::{GoalOp, GoalStatus};
use kuzz_core::CommunityId;
use nostr::Event;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::event::insert_event_in_transaction;
use crate::{Db, DbError, Result};

/// Most ops one event may carry.
pub const MAX_OPS_PER_EVENT: usize = 16;

/// Content of a kind 40111 event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalOps {
    /// Applied in order; all or none.
    pub ops: Vec<GoalOp>,
}

/// What one event did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalOutcome {
    /// Parent of the last goal the event finished, if any.
    pub parent: Option<String>,
    /// Goals deleted, in deletion order.
    pub deleted: Vec<String>,
    /// Finish or remove targets that do not exist. Retries land here, and so
    /// do wrong ids, so writers warn about these.
    pub missing: Vec<String>,
}

/// Result of [`Db::apply_goal_ops`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoalWrite {
    /// Applied and stored.
    Applied(GoalOutcome),
    /// Nothing changed, so no event was stored or sent.
    NoEffect(GoalOutcome),
    /// This event was already applied.
    Duplicate,
}

/// One goal as read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalRow {
    /// Goal id.
    pub id: String,
    /// Parent id.
    pub parent: Option<String>,
    /// One-line goal.
    pub title: String,
    /// Key information.
    pub note: String,
    /// Progress state.
    pub status: GoalStatus,
    /// Sort key among siblings.
    pub order: i64,
    /// Assigned pubkeys (lowercase hex).
    pub assignees: Vec<String>,
    /// Linked thread and message ids (lowercase hex).
    pub links: Vec<String>,
    /// Last editor.
    pub updated_by: Option<String>,
    /// Unix seconds of the last edit.
    pub updated_at: i64,
    /// A linked thread has replies.
    pub is_thread: bool,
    /// No activity for the inactive window.
    pub inactive: bool,
}

fn rejected(message: impl Into<String>) -> DbError {
    DbError::GoalRejected(message.into())
}

fn checked<T>(result: std::result::Result<T, kuzz_core::goal_tree::GoalTreeError>) -> Result<T> {
    result.map_err(|e| rejected(e.to_string()))
}

fn thread_bytes(thread: &str) -> Result<Vec<u8>> {
    let thread = thread.to_ascii_lowercase();
    match hex::decode(&thread) {
        Ok(bytes) if bytes.len() == 32 => Ok(bytes),
        _ => Err(rejected(format!("invalid thread id {thread:?}"))),
    }
}

fn parse_status(value: &str) -> Result<GoalStatus> {
    GoalStatus::parse(value)
        .ok_or_else(|| DbError::InvalidData(format!("unknown goal status {value:?}")))
}

/// Parse and bound an event's content before any database work.
pub fn parse_goal_ops(content: &str) -> Result<Vec<GoalOp>> {
    let body: GoalOps =
        serde_json::from_str(content).map_err(|e| rejected(format!("invalid goal ops: {e}")))?;
    if body.ops.is_empty() {
        return Err(rejected("goal event has no ops"));
    }
    if body.ops.len() > MAX_OPS_PER_EVENT {
        return Err(rejected(format!(
            "goal event has more than {MAX_OPS_PER_EVENT} ops"
        )));
    }
    Ok(body.ops)
}

/// Row columns of the shape query, shared by the writer and the reader.
const SHAPE_SQL: &str = "SELECT g.id, g.parent_id, g.status, g.updated_at, \
        COALESCE(l.has_thread, FALSE) AS has_thread, l.last_reply_at \
     FROM goals g \
     LEFT JOIN ( \
        SELECT gl.goal_id, \
            COALESCE(bool_or(t.event_id = gl.event_id AND t.parent_event_id IS NULL \
                AND t.reply_count > 0), FALSE) AS has_thread, \
            MAX(t.last_reply_at) AS last_reply_at \
        FROM goal_links gl \
        LEFT JOIN thread_metadata t \
            ON t.community_id = gl.community_id AND t.channel_id = gl.channel_id \
            AND (t.event_id = gl.event_id OR t.root_event_id = gl.event_id) \
        WHERE gl.community_id = $1 AND gl.channel_id = $2 \
        GROUP BY gl.goal_id \
     ) l ON l.goal_id = g.id \
     WHERE g.community_id = $1 AND g.channel_id = $2";

async fn load_shape<'e, E>(executor: E, community: Uuid, channel: Uuid) -> Result<GoalGraph>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query(SHAPE_SQL)
        .bind(community)
        .bind(channel)
        .fetch_all(executor)
        .await?;
    let mut shapes = Vec::with_capacity(rows.len());
    for row in rows {
        let status: String = row.try_get("status")?;
        let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
        let last_reply_at: Option<DateTime<Utc>> = row.try_get("last_reply_at")?;
        shapes.push(GoalShape {
            id: row.try_get("id")?,
            parent: row.try_get("parent_id")?,
            status: parse_status(&status)?,
            updated_at: updated_at.timestamp(),
            has_thread: row.try_get("has_thread")?,
            last_reply_at: last_reply_at.map(|t| t.timestamp()),
        });
    }
    Ok(GoalGraph::new(shapes))
}

/// One event's work against the locked room.
struct Apply<'a, 'c> {
    tx: &'a mut Transaction<'c, Postgres>,
    community: Uuid,
    channel: Uuid,
    editor: String,
    now: DateTime<Utc>,
    graph: GoalGraph,
    outcome: GoalOutcome,
    changed: bool,
}

impl Apply<'_, '_> {
    fn stamp(&self) -> i64 {
        self.now.timestamp()
    }

    async fn reload_shape(&mut self) -> Result<()> {
        self.graph = load_shape(&mut **self.tx, self.community, self.channel).await?;
        Ok(())
    }

    async fn next_order(&mut self) -> Result<i64> {
        let max: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(order_key) FROM goals WHERE community_id = $1 AND channel_id = $2",
        )
        .bind(self.community)
        .bind(self.channel)
        .fetch_one(&mut **self.tx)
        .await?;
        Ok(max.map_or(0, |m| m.saturating_add(1)))
    }

    async fn insert(
        &mut self,
        id: &str,
        parent: Option<&str>,
        title: &str,
        note: &str,
        assignees: &[String],
    ) -> Result<()> {
        let order = self.next_order().await?;
        sqlx::query(
            "INSERT INTO goals (community_id, channel_id, id, parent_id, title, note, \
                order_key, assignees, updated_by, updated_at, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(id)
        .bind(parent)
        .bind(title)
        .bind(note)
        .bind(order)
        .bind(assignees)
        .bind(&self.editor)
        .bind(self.now)
        .execute(&mut **self.tx)
        .await?;
        self.graph.insert(GoalShape {
            id: id.to_owned(),
            parent: parent.map(str::to_owned),
            status: GoalStatus::Open,
            updated_at: self.stamp(),
            has_thread: false,
            last_reply_at: None,
        });
        self.changed = true;
        Ok(())
    }

    /// Mark rows as edited by this event.
    async fn touch(&mut self, ids: &[String]) -> Result<()> {
        sqlx::query(
            "UPDATE goals SET updated_by = $4, updated_at = $5 \
             WHERE community_id = $1 AND channel_id = $2 AND id = ANY($3)",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(ids)
        .bind(&self.editor)
        .bind(self.now)
        .execute(&mut **self.tx)
        .await?;
        let stamp = self.stamp();
        for id in ids {
            self.graph.touch(id, stamp);
        }
        self.changed = true;
        Ok(())
    }

    /// Delete rows. The nearest surviving parent takes the edit time, so
    /// finishing small work keeps the plan above it active.
    async fn delete(&mut self, ids: Vec<String>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut survivors = Vec::new();
        for id in &ids {
            let mut cursor = self.graph.get(id).and_then(|n| n.parent.clone());
            while let Some(parent) = cursor {
                if !ids.contains(&parent) {
                    survivors.push(parent);
                    break;
                }
                cursor = self.graph.get(&parent).and_then(|n| n.parent.clone());
            }
        }
        survivors.sort();
        survivors.dedup();
        sqlx::query(
            "DELETE FROM goals WHERE community_id = $1 AND channel_id = $2 AND id = ANY($3)",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(&ids)
        .execute(&mut **self.tx)
        .await?;
        self.graph.remove(&ids);
        if !survivors.is_empty() {
            self.touch(&survivors).await?;
        }
        self.outcome.deleted.extend(ids);
        self.changed = true;
        Ok(())
    }

    async fn write_status(&mut self, id: &str, status: GoalStatus) -> Result<()> {
        sqlx::query(
            "UPDATE goals SET status = $4 \
             WHERE community_id = $1 AND channel_id = $2 AND id = $3",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(id)
        .bind(status.as_str())
        .execute(&mut **self.tx)
        .await?;
        self.graph.set_status(id, status);
        Ok(())
    }

    async fn set_root(&mut self, id: &str, title: &str, note: Option<&str>) -> Result<()> {
        checked(goal_store::validate_fields(Some(title), note, &[]))?;
        let Some(root) = self.graph.root().map(str::to_owned) else {
            checked(goal_store::validate_id(id))?;
            return self.insert(id, None, title, note.unwrap_or(""), &[]).await;
        };
        sqlx::query(
            "UPDATE goals SET title = $4, note = COALESCE($5, note) \
             WHERE community_id = $1 AND channel_id = $2 AND id = $3",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(&root)
        .bind(title)
        .bind(note)
        .execute(&mut **self.tx)
        .await?;
        self.touch(&[root]).await
    }

    async fn add(
        &mut self,
        id: &str,
        parent: &str,
        title: &str,
        note: Option<&str>,
        assignees: &[String],
    ) -> Result<()> {
        checked(goal_store::validate_id(id))?;
        checked(goal_store::validate_fields(Some(title), note, assignees))?;
        if self.graph.get(id).is_some() {
            let same: Option<(Option<String>, String)> = sqlx::query_as(
                "SELECT parent_id, title FROM goals \
                 WHERE community_id = $1 AND channel_id = $2 AND id = $3",
            )
            .bind(self.community)
            .bind(self.channel)
            .bind(id)
            .fetch_optional(&mut **self.tx)
            .await?;
            // A retry of an Add that landed; a Token retry gets a new event id.
            return match same {
                Some((Some(p), t)) if p == parent && t == title => Ok(()),
                _ => Err(rejected(format!("duplicate goal id {id}"))),
            };
        }
        if self.graph.get(parent).is_none() {
            return Err(rejected(format!("goal not found: {parent}")));
        }
        if self.graph.layer(parent) + 1 > kuzz_core::goal_tree::MAX_DEPTH {
            return Err(rejected("goal would be too deep"));
        }
        let assignees: Vec<String> = assignees.iter().map(|a| a.to_ascii_lowercase()).collect();
        self.insert(id, Some(parent), title, note.unwrap_or(""), &assignees)
            .await
    }

    async fn update(
        &mut self,
        id: &str,
        title: Option<&str>,
        note: Option<&str>,
        status: Option<GoalStatus>,
        add_assignees: &[String],
        remove_assignees: &[String],
    ) -> Result<()> {
        let finishing = status.is_some_and(goal_store::finished);
        let Some(current) = self.graph.get(id).map(|n| n.status) else {
            if finishing {
                self.outcome.missing.push(id.to_owned());
                return Ok(());
            }
            return Err(rejected(format!("goal not found: {id}")));
        };
        checked(goal_store::validate_fields(title, note, add_assignees))?;
        let add: Vec<String> = add_assignees
            .iter()
            .map(|a| a.to_ascii_lowercase())
            .collect();
        let remove: Vec<String> = remove_assignees
            .iter()
            .map(|a| a.to_ascii_lowercase())
            .collect();
        let assignees: Vec<String> = sqlx::query_scalar(
            "UPDATE goals SET title = COALESCE($4, title), note = COALESCE($5, note), \
                assignees = ARRAY( \
                    SELECT a FROM unnest(assignees || $6::text[]) WITH ORDINALITY AS u(a, n) \
                    WHERE a <> ALL($7::text[]) GROUP BY a ORDER BY MIN(n)) \
             WHERE community_id = $1 AND channel_id = $2 AND id = $3 \
             RETURNING assignees",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(id)
        .bind(title)
        .bind(note)
        .bind(&add)
        .bind(&remove)
        .fetch_one(&mut **self.tx)
        .await?;
        if assignees.len() > kuzz_core::goal_tree::MAX_ASSIGNEES {
            return Err(rejected("goal has too many assignees"));
        }
        if let Some(status) = status {
            self.write_status(id, status).await?;
        }
        self.touch(&[id.to_owned()]).await?;
        if finishing && !goal_store::finished(current) {
            self.after_finish(id).await?;
        }
        Ok(())
    }

    async fn after_finish(&mut self, id: &str) -> Result<()> {
        self.outcome.parent = self.graph.get(id).and_then(|n| n.parent.clone());
        let doomed = self.graph.deletions_after_finish(id);
        self.delete(doomed).await
    }

    async fn move_goal(&mut self, id: &str, parent: &str, order: Option<i64>) -> Result<()> {
        let Some(node) = self.graph.get(id) else {
            return Err(rejected(format!("goal not found: {id}")));
        };
        if node.parent.is_none() {
            return Err(rejected("the layer 1 goal cannot be moved"));
        }
        if self.graph.get(parent).is_none() {
            return Err(rejected(format!("goal not found: {parent}")));
        }
        checked(goal_store::validate_move(&self.graph, id, parent))?;
        let order = match order {
            Some(order) => order,
            None => self.next_order().await?,
        };
        sqlx::query(
            "UPDATE goals SET parent_id = $4, order_key = $5 \
             WHERE community_id = $1 AND channel_id = $2 AND id = $3",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(id)
        .bind(parent)
        .bind(order)
        .execute(&mut **self.tx)
        .await?;
        self.graph.set_parent(id, parent);
        self.touch(&[id.to_owned()]).await
    }

    async fn remove(&mut self, id: &str, recursive: bool) -> Result<()> {
        if self.graph.get(id).is_none() {
            self.outcome.missing.push(id.to_owned());
            return Ok(());
        }
        let doomed = self.graph.subtree(id);
        if doomed.len() > 1 && !recursive {
            return Err(rejected(format!(
                "goal {id} has {} sub-goals; remove them too with recursive",
                doomed.len() - 1
            )));
        }
        self.delete(doomed).await
    }

    async fn link(&mut self, id: &str, thread: &str) -> Result<()> {
        let Some(node) = self.graph.get(id) else {
            return Err(rejected(format!("goal not found: {id}")));
        };
        if node.parent.is_none() {
            return Err(rejected("the layer 1 goal cannot be linked"));
        }
        let status = node.status;
        let event_id = thread_bytes(thread)?;
        sqlx::query(
            "INSERT INTO goal_links (community_id, channel_id, event_id, goal_id) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (community_id, channel_id, event_id) \
             DO UPDATE SET goal_id = EXCLUDED.goal_id",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(&event_id)
        .bind(id)
        .execute(&mut **self.tx)
        .await?;
        // Work has started on a goal once a thread is linked to it.
        if status == GoalStatus::Open {
            self.write_status(id, GoalStatus::InProgress).await?;
        }
        self.touch(&[id.to_owned()]).await?;
        self.reload_shape().await
    }

    async fn unlink(&mut self, thread: &str) -> Result<()> {
        let event_id = thread_bytes(thread)?;
        let goal: Option<String> = sqlx::query_scalar(
            "DELETE FROM goal_links \
             WHERE community_id = $1 AND channel_id = $2 AND event_id = $3 \
             RETURNING goal_id",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(&event_id)
        .fetch_optional(&mut **self.tx)
        .await?;
        if let Some(goal) = goal {
            self.touch(&[goal]).await?;
            self.reload_shape().await?;
        }
        Ok(())
    }

    /// Give up every inactive goal, deepest first so each deletion sees its
    /// children already settled.
    async fn drop_inactive(&mut self) -> Result<()> {
        let mut ids: Vec<(usize, String)> = self
            .graph
            .inactive(self.stamp())
            .into_iter()
            .map(|id| (self.graph.layer(id), id.to_owned()))
            .collect();
        ids.sort_by(|a, b| b.cmp(a));
        for (_, id) in &ids {
            self.write_status(id, GoalStatus::Dropped).await?;
        }
        if !ids.is_empty() {
            let touched: Vec<String> = ids.iter().map(|(_, id)| id.clone()).collect();
            self.touch(&touched).await?;
        }
        for (_, id) in ids {
            if self.graph.get(&id).is_some() {
                self.after_finish(&id).await?;
            }
        }
        // A bulk action names no single parent.
        self.outcome.parent = None;
        Ok(())
    }

    async fn apply(&mut self, op: &GoalOp) -> Result<()> {
        match op {
            GoalOp::SetRoot { id, title, note } => self.set_root(id, title, note.as_deref()).await,
            GoalOp::Add {
                id,
                parent,
                title,
                note,
                assignees,
            } => {
                self.add(id, parent, title, note.as_deref(), assignees)
                    .await
            }
            GoalOp::Update {
                id,
                title,
                note,
                status,
                add_assignees,
                remove_assignees,
            } => {
                self.update(
                    id,
                    title.as_deref(),
                    note.as_deref(),
                    *status,
                    add_assignees,
                    remove_assignees,
                )
                .await
            }
            GoalOp::Move { id, parent, order } => self.move_goal(id, parent, *order).await,
            GoalOp::Remove { id, recursive } => self.remove(id, *recursive).await,
            GoalOp::Link { id, thread } => self.link(id, thread).await,
            GoalOp::Unlink { thread } => self.unlink(thread).await,
            GoalOp::DropInactive => self.drop_inactive().await,
        }
    }
}

impl Db {
    /// Apply one kind 40111 event and store it, all or nothing. A rejected
    /// event leaves no trace. Replays of a stored event change nothing.
    pub async fn apply_goal_ops(
        &self,
        community: CommunityId,
        channel: Uuid,
        event: &Event,
        ops: &[GoalOp],
    ) -> Result<GoalWrite> {
        let mut tx = self.begin_event_write_transaction().await?;
        sqlx::query("SET LOCAL statement_timeout='5s'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("goals:{}:{channel}", community.as_uuid()))
            .execute(&mut *tx)
            .await?;
        let created_at = DateTime::from_timestamp(event.created_at.as_secs() as i64, 0)
            .ok_or(DbError::InvalidTimestamp(event.created_at.as_secs() as i64))?;
        let duplicate: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events \
             WHERE community_id = $1 AND created_at = $2 AND id = $3)",
        )
        .bind(community.as_uuid())
        .bind(created_at)
        .bind(event.id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await?;
        if duplicate {
            return Ok(GoalWrite::Duplicate);
        }
        let graph = load_shape(&mut *tx, *community.as_uuid(), channel).await?;
        let mut apply = Apply {
            tx: &mut tx,
            community: *community.as_uuid(),
            channel,
            editor: event.pubkey.to_hex(),
            now: Utc::now(),
            graph,
            outcome: GoalOutcome::default(),
            changed: false,
        };
        for op in ops {
            apply.apply(op).await?;
        }
        let outcome = std::mem::take(&mut apply.outcome);
        if !apply.changed {
            return Ok(GoalWrite::NoEffect(outcome));
        }
        insert_event_in_transaction(&mut tx, community, event, Some(channel)).await?;
        tx.commit().await?;
        Ok(GoalWrite::Applied(outcome))
    }

    /// Every goal of a conversation with its computed state. Links are
    /// lowercase hex. Order: siblings by sort key, ties by id.
    pub async fn list_goals(&self, community: CommunityId, channel: Uuid) -> Result<Vec<GoalRow>> {
        let community_id = *community.as_uuid();
        let graph = load_shape(&self.pool, community_id, channel).await?;
        let inactive = graph.inactive(Utc::now().timestamp());
        let mut links: HashMap<String, Vec<String>> = HashMap::new();
        for row in sqlx::query(
            "SELECT goal_id, event_id FROM goal_links \
             WHERE community_id = $1 AND channel_id = $2 ORDER BY event_id",
        )
        .bind(community_id)
        .bind(channel)
        .fetch_all(&self.pool)
        .await?
        {
            let event_id: Vec<u8> = row.try_get("event_id")?;
            links
                .entry(row.try_get("goal_id")?)
                .or_default()
                .push(hex::encode(event_id));
        }
        let rows = sqlx::query(
            "SELECT id, parent_id, title, note, status, order_key, assignees, updated_by, \
                updated_at \
             FROM goals WHERE community_id = $1 AND channel_id = $2 ORDER BY order_key, id",
        )
        .bind(community_id)
        .bind(channel)
        .fetch_all(&self.pool)
        .await?;
        let mut goals = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("id")?;
            let status: String = row.try_get("status")?;
            let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
            goals.push(GoalRow {
                parent: row.try_get("parent_id")?,
                title: row.try_get("title")?,
                note: row.try_get("note")?,
                status: parse_status(&status)?,
                order: row.try_get("order_key")?,
                assignees: row.try_get("assignees")?,
                links: links.remove(&id).unwrap_or_default(),
                updated_by: row.try_get("updated_by")?,
                updated_at: updated_at.timestamp(),
                // A goal added after the shape read is simply not flagged yet.
                is_thread: graph.get(&id).is_some_and(|n| n.has_thread),
                inactive: inactive.contains(id.as_str()),
                id,
            });
        }
        Ok(goals)
    }
}
