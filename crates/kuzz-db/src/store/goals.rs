//! Goal rows: one row per goal, one link row per linked thread or message.
//!
//! A write is one signed kind 40111 event carrying [`GoalOp`]s. Under a room
//! lock the ops apply to the rows and the event is stored in one transaction,
//! so the stored change log and the rows cannot disagree. Deleted goals are
//! really gone; there is nothing to restore. Rules that need the room's shape
//! live in [`kuzz_core::goal_store`].

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use kuzz_core::event::StoredEvent;
use kuzz_core::goal_store::{self, GoalGraph, GoalShape, GoalView};
use kuzz_core::goal_tree::{GoalOp, GoalStatus};
use kuzz_core::CommunityId;
use nostr::Event;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::event::insert_event_received_in_transaction;
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
    /// Nearest surviving parent of the last goal the event finished, if any.
    pub parent: Option<String>,
    /// Goals deleted, in deletion order.
    pub deleted: Vec<String>,
    /// Finish or remove targets that do not exist. Retries land here, and so
    /// do wrong ids, so writers warn about these.
    pub missing: Vec<String>,
}

/// Result of [`Db::apply_goal_ops`].
#[derive(Clone, Debug)]
pub enum GoalWrite {
    /// Applied and stored; the event is ready to send to subscribers.
    Applied(Box<StoredEvent>, GoalOutcome),
    /// Nothing changed, so no event was stored or sent.
    NoEffect(GoalOutcome),
    /// This event was already applied.
    Duplicate,
}

/// Most rows in one page of a goal list, before their parents are added.
pub const MAX_GOAL_PAGE: usize = 200;
/// Most threads in one focus read.
pub const MAX_GOAL_THREADS: usize = 32;

/// Which goals to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoalSelection {
    /// One list, in pages. `after` is the `next` of the page before.
    View {
        /// The list.
        view: GoalView,
        /// Where the last page ended.
        after: Option<GoalCursor>,
    },
    /// What these threads work on.
    Threads(Vec<[u8; 32]>),
}

/// A position in a goal list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalCursor {
    /// Sort key of the last row.
    pub order: i64,
    /// Id of the last row.
    pub id: String,
}

/// Goals in each list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct GoalCounts {
    /// Unfinished with recent activity.
    pub active: usize,
    /// Unfinished and quiet.
    pub inactive: usize,
    /// Done or dropped.
    pub done: usize,
}

/// The rows of one read, each with the goals above it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalPage {
    /// Selected rows and their parents, in list order.
    pub rows: Vec<GoalPageRow>,
    /// List sizes for the whole conversation.
    pub counts: GoalCounts,
    /// For a thread read: each thread with its goal, if it has one.
    pub threads: Vec<([u8; 32], Option<String>)>,
    /// Pass back to read the next page.
    pub next: Option<GoalCursor>,
}

/// One row of a read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalPageRow {
    /// The goal.
    pub goal: GoalRow,
    /// Selected, as opposed to being here as a parent of one that was.
    pub matched: bool,
    /// Done and total goals below that count; see [`GoalGraph::progress`].
    pub progress: (usize, usize),
    /// Unfinished goals directly below.
    pub open_children: usize,
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
const SHAPE_SQL: &str = "SELECT g.id, g.parent_id, g.status, g.order_key, g.updated_at, \
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
            order: row.try_get("order_key")?,
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
            order,
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
        let doomed: HashSet<&String> = ids.iter().collect();
        // The report reads this goal back, so it must be one that stays.
        let mut parent = self.outcome.parent.take();
        while let Some(gone) = parent.as_ref().filter(|p| doomed.contains(*p)) {
            parent = self.graph.get(gone).and_then(|n| n.parent.clone());
        }
        self.outcome.parent = parent;
        let mut survivors: Vec<String> = ids
            .iter()
            .filter_map(|id| self.graph.get(id)?.parent.clone())
            .filter(|parent| !doomed.contains(parent))
            .collect();
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

    async fn write_status(&mut self, ids: &[String], status: GoalStatus) -> Result<()> {
        sqlx::query(
            "UPDATE goals SET status = $4 \
             WHERE community_id = $1 AND channel_id = $2 AND id = ANY($3)",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(ids)
        .bind(status.as_str())
        .execute(&mut **self.tx)
        .await?;
        for id in ids {
            self.graph.set_status(id, status);
        }
        Ok(())
    }

    async fn stored_text(&mut self, id: &str) -> Result<(String, String, Vec<String>, i64)> {
        Ok(sqlx::query_as(
            "SELECT title, note, assignees, order_key FROM goals \
             WHERE community_id = $1 AND channel_id = $2 AND id = $3",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(id)
        .fetch_one(&mut **self.tx)
        .await?)
    }

    async fn set_root(&mut self, id: &str, title: &str, note: Option<&str>) -> Result<()> {
        checked(goal_store::validate_fields(Some(title), note, &[]))?;
        let Some(root) = self.graph.root().map(str::to_owned) else {
            checked(goal_store::validate_id(id))?;
            return self.insert(id, None, title, note.unwrap_or(""), &[]).await;
        };
        let (old_title, old_note, ..) = self.stored_text(&root).await?;
        if title == old_title && note.is_none_or(|n| n == old_note) {
            return Ok(());
        }
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
        if let Some(existing) = self.graph.get(id) {
            let same_parent = existing.parent.as_deref() == Some(parent);
            let (old_title, ..) = self.stored_text(id).await?;
            // A retry of an Add that landed; a Token retry gets a new event id.
            return if same_parent && old_title == title {
                Ok(())
            } else {
                Err(rejected(format!("duplicate goal id {id}")))
            };
        }
        if self.graph.get(parent).is_none() {
            return Err(rejected(format!("goal not found: {parent}")));
        }
        if self.graph.layer(parent) + 1 > kuzz_core::goal_tree::MAX_DEPTH {
            return Err(rejected("goal would be too deep"));
        }
        let assignees = goal_store::merge_assignees(&[], assignees, &[]);
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
        let (old_title, old_note, old_assignees, _) = self.stored_text(id).await?;
        let assignees =
            goal_store::merge_assignees(&old_assignees, add_assignees, remove_assignees);
        if assignees.len() > kuzz_core::goal_tree::MAX_ASSIGNEES {
            return Err(rejected("goal has too many assignees"));
        }
        let unchanged = title.is_none_or(|t| t == old_title)
            && note.is_none_or(|n| n == old_note)
            && status.is_none_or(|s| s == current)
            && assignees == old_assignees;
        if unchanged {
            return Ok(());
        }
        sqlx::query(
            "UPDATE goals SET title = COALESCE($4, title), note = COALESCE($5, note), \
                assignees = $6 \
             WHERE community_id = $1 AND channel_id = $2 AND id = $3",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(id)
        .bind(title)
        .bind(note)
        .bind(&assignees)
        .execute(&mut **self.tx)
        .await?;
        let id = [id.to_owned()];
        if let Some(status) = status {
            self.write_status(&id, status).await?;
        }
        self.touch(&id).await?;
        if finishing && !goal_store::finished(current) {
            self.after_finish(&id[0]).await?;
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
        let same_parent = node.parent.as_deref() == Some(parent);
        if self.graph.get(parent).is_none() {
            return Err(rejected(format!("goal not found: {parent}")));
        }
        if same_parent {
            let (.., old_order) = self.stored_text(id).await?;
            // With no order the goal goes last, so one already last stays put.
            let last: Option<i64> = sqlx::query_scalar(
                "SELECT MAX(order_key) FROM goals \
                 WHERE community_id = $1 AND channel_id = $2 AND parent_id = $3",
            )
            .bind(self.community)
            .bind(self.channel)
            .bind(parent)
            .fetch_one(&mut **self.tx)
            .await?;
            if order.or(last) == Some(old_order) {
                return Ok(());
            }
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
        let linked: Option<String> = sqlx::query_scalar(
            "SELECT goal_id FROM goal_links \
             WHERE community_id = $1 AND channel_id = $2 AND event_id = $3",
        )
        .bind(self.community)
        .bind(self.channel)
        .bind(&event_id)
        .fetch_optional(&mut **self.tx)
        .await?;
        if linked.as_deref() == Some(id) && status != GoalStatus::Open {
            return Ok(());
        }
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
        let id = [id.to_owned()];
        // Work has started on a goal once a thread is linked to it.
        if status == GoalStatus::Open {
            self.write_status(&id, GoalStatus::InProgress).await?;
        }
        self.touch(&id).await?;
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
    /// children already settled. Settled on a copy, so one DELETE covers it
    /// all and the room lock is not held row by row.
    async fn drop_inactive(&mut self) -> Result<()> {
        let mut ids: Vec<(usize, String)> = self
            .graph
            .inactive(self.stamp())
            .into_iter()
            .map(|id| (self.graph.layer(id), id.to_owned()))
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        ids.sort_by(|a, b| b.cmp(a));
        let ids: Vec<String> = ids.into_iter().map(|(_, id)| id).collect();
        self.write_status(&ids, GoalStatus::Dropped).await?;
        self.touch(&ids).await?;
        let mut settled = self.graph.clone();
        let mut doomed = Vec::new();
        for id in &ids {
            let gone = settled.deletions_after_finish(id);
            settled.remove(&gone);
            doomed.extend(gone);
        }
        self.delete(doomed).await
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
        let received_at = Utc::now();
        let mut apply = Apply {
            tx: &mut tx,
            community: *community.as_uuid(),
            channel,
            editor: event.pubkey.to_hex(),
            now: received_at,
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
        let (stored, _) = insert_event_received_in_transaction(
            &mut tx,
            community,
            event,
            Some(channel),
            received_at,
        )
        .await?;
        tx.commit().await?;
        Ok(GoalWrite::Applied(Box::new(stored), outcome))
    }

    /// Read one list or one set of threads' goals. Rows carry everything a
    /// client shows, computed here so clients never rebuild the rules. The
    /// shape is read first and only the chosen rows' text after it, so a list
    /// costs the same in a room that keeps many old goals.
    pub async fn read_goals(
        &self,
        community: CommunityId,
        channel: Uuid,
        selection: &GoalSelection,
    ) -> Result<GoalPage> {
        let community_id = *community.as_uuid();
        let graph = load_shape(&self.pool, community_id, channel).await?;
        let now = Utc::now().timestamp();
        let views = graph.views(now);
        let mut counts = GoalCounts::default();
        for view in views.values() {
            match view {
                GoalView::Active => counts.active += 1,
                GoalView::Inactive => counts.inactive += 1,
                GoalView::Done => counts.done += 1,
            }
        }
        let mut threads = Vec::new();
        let mut next = None;
        let matched: Vec<String> = match selection {
            GoalSelection::View { view, after } => {
                let rows: Vec<&GoalShape> = graph
                    .sorted()
                    .into_iter()
                    .filter(|n| views.get(n.id.as_str()) == Some(view))
                    .filter(|n| {
                        after
                            .as_ref()
                            .is_none_or(|a| (n.order, n.id.as_str()) > (a.order, a.id.as_str()))
                    })
                    .collect();
                if let Some(last) = rows.get(MAX_GOAL_PAGE.saturating_sub(1)) {
                    if rows.len() > MAX_GOAL_PAGE {
                        next = Some(GoalCursor {
                            order: last.order,
                            id: last.id.clone(),
                        });
                    }
                }
                rows.into_iter()
                    .take(MAX_GOAL_PAGE)
                    .map(|n| n.id.clone())
                    .collect()
            }
            GoalSelection::Threads(wanted) => {
                let ids: Vec<&[u8]> = wanted.iter().map(<[u8; 32]>::as_slice).collect();
                let linked: HashMap<Vec<u8>, String> = sqlx::query_as(
                    "SELECT event_id, goal_id FROM goal_links \
                     WHERE community_id = $1 AND channel_id = $2 AND event_id = ANY($3)",
                )
                .bind(community_id)
                .bind(channel)
                .bind(&ids)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect();
                let mut focus = Vec::new();
                for thread in wanted {
                    let goal = linked.get(thread.as_slice()).cloned();
                    if let Some(goal) = &goal {
                        focus.extend(graph.focus(goal, now));
                    }
                    threads.push((*thread, goal));
                }
                focus
            }
        };
        let mut wanted: HashSet<String> = HashSet::with_capacity(matched.len());
        for id in &matched {
            if wanted.insert(id.clone()) {
                wanted.extend(graph.ancestors(id));
            }
        }
        let matched: HashSet<String> = matched.into_iter().collect();
        let ids: Vec<String> = wanted.into_iter().collect();
        let progress = graph.progress(now);
        let rows = self
            .fetch_goal_rows(community_id, channel, &graph, now, Some(&ids))
            .await?
            .into_iter()
            .map(|goal| GoalPageRow {
                matched: matched.contains(&goal.id),
                progress: progress.get(goal.id.as_str()).copied().unwrap_or_default(),
                open_children: graph.open_children(&goal.id),
                goal,
            })
            .collect();
        Ok(GoalPage {
            rows,
            counts,
            threads,
            next,
        })
    }

    /// Every goal of a conversation with its computed state. Links are
    /// lowercase hex. Order: siblings by sort key, ties by id.
    pub async fn list_goals(&self, community: CommunityId, channel: Uuid) -> Result<Vec<GoalRow>> {
        let community_id = *community.as_uuid();
        let graph = load_shape(&self.pool, community_id, channel).await?;
        let now = Utc::now().timestamp();
        self.fetch_goal_rows(community_id, channel, &graph, now, None)
            .await
    }

    async fn fetch_goal_rows(
        &self,
        community_id: Uuid,
        channel: Uuid,
        graph: &GoalGraph,
        now: i64,
        only: Option<&[String]>,
    ) -> Result<Vec<GoalRow>> {
        let inactive = graph.inactive(now);
        let mut links: HashMap<String, Vec<String>> = HashMap::new();
        for row in sqlx::query(
            "SELECT goal_id, event_id FROM goal_links \
             WHERE community_id = $1 AND channel_id = $2 \
                AND ($3::text[] IS NULL OR goal_id = ANY($3)) \
             ORDER BY event_id",
        )
        .bind(community_id)
        .bind(channel)
        .bind(only)
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
             FROM goals WHERE community_id = $1 AND channel_id = $2 \
                AND ($3::text[] IS NULL OR id = ANY($3)) \
             ORDER BY order_key, id",
        )
        .bind(community_id)
        .bind(channel)
        .bind(only)
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
