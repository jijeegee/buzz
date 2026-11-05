//! Goal rows — the shape of one conversation's goals, with the rules that
//! need the whole shape: which finished goals get deleted and which unfinished
//! goals are inactive.
//!
//! Zero-I/O. The store loads the shape (no titles or notes) under the room
//! lock, asks these rules, and writes the answer back.
//!
//! Terms used by every rule:
//! - a *child* is any row below a goal, finished or not;
//! - *unfinished* means `open` or `in_progress`;
//! - a *thread goal* has a linked thread that has replies. Layer 1 is never one;
//! - a *task* has no thread of its own and has a thread goal above it.

use std::collections::{HashMap, HashSet};

use crate::goal_tree::{is_lower_hex_64, valid_id, validate_title, GoalStatus, GoalTreeError};
use crate::goal_tree::{MAX_ASSIGNEES, MAX_DEPTH, MAX_NOTE_CHARS};

/// A goal with no activity for this long is inactive: agents stop reading it
/// and apps fold it away. Any new activity makes it active again.
pub const INACTIVE_AFTER_SECS: i64 = 90 * 24 * 60 * 60;

/// One goal, without its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GoalShape {
    /// Goal id.
    pub id: String,
    /// Parent id; `None` only for layer 1.
    pub parent: Option<String>,
    /// Progress state.
    pub status: GoalStatus,
    /// Unix seconds of the last edit.
    pub updated_at: i64,
    /// A linked thread has replies.
    pub has_thread: bool,
    /// Unix seconds of the last reply in linked threads or messages.
    pub last_reply_at: Option<i64>,
}

/// All goals of one conversation.
#[derive(Clone, Debug, Default)]
pub struct GoalGraph {
    nodes: HashMap<String, GoalShape>,
    children: HashMap<String, Vec<String>>,
}

impl GoalGraph {
    /// Build from rows in any order. Parents are trusted (the table's foreign
    /// key guarantees them).
    pub fn new(rows: Vec<GoalShape>) -> Self {
        let mut graph = Self::default();
        for row in rows {
            graph.insert(row);
        }
        graph
    }

    /// Add or replace one goal.
    pub fn insert(&mut self, mut row: GoalShape) {
        self.detach(&row.id);
        if row.parent.is_none() {
            row.has_thread = false;
        }
        if let Some(parent) = &row.parent {
            self.children
                .entry(parent.clone())
                .or_default()
                .push(row.id.clone());
        }
        self.nodes.insert(row.id.clone(), row);
    }

    fn detach(&mut self, id: &str) {
        let parent = self.nodes.get(id).and_then(|n| n.parent.clone());
        if let Some(siblings) = parent.and_then(|p| self.children.get_mut(&p)) {
            siblings.retain(|c| c != id);
        }
    }

    /// One goal.
    pub fn get(&self, id: &str) -> Option<&GoalShape> {
        self.nodes.get(id)
    }

    /// The layer 1 goal's id.
    pub fn root(&self) -> Option<&str> {
        self.nodes
            .values()
            .find(|n| n.parent.is_none())
            .map(|n| n.id.as_str())
    }

    /// Ids directly below a goal.
    pub fn children(&self, id: &str) -> &[String] {
        self.children.get(id).map_or(&[], Vec::as_slice)
    }

    /// The goal and everything below it, parents before children.
    pub fn subtree(&self, id: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![id.to_owned()];
        while let Some(next) = stack.pop() {
            if self.nodes.contains_key(&next) {
                stack.extend(self.children(&next).iter().cloned());
                out.push(next);
            }
        }
        out
    }

    /// Layer of a goal (layer 1 = root).
    pub fn layer(&self, id: &str) -> usize {
        let mut layer = 0;
        let mut cursor = self.nodes.get(id);
        while let Some(node) = cursor {
            layer += 1;
            cursor = node.parent.as_deref().and_then(|p| self.nodes.get(p));
        }
        layer
    }

    /// Change a goal's parent.
    pub fn set_parent(&mut self, id: &str, parent: &str) {
        self.detach(id);
        if let Some(node) = self.nodes.get_mut(id) {
            node.parent = Some(parent.to_owned());
            self.children
                .entry(parent.to_owned())
                .or_default()
                .push(id.to_owned());
        }
    }

    /// Change a goal's status.
    pub fn set_status(&mut self, id: &str, status: GoalStatus) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = status;
        }
    }

    /// Record an edit time.
    pub fn touch(&mut self, id: &str, now: i64) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.updated_at = now;
        }
    }

    /// Delete goals. Each given goal's children must be given too.
    pub fn remove(&mut self, ids: &[String]) {
        for id in ids {
            self.detach(id);
            self.children.remove(id);
            self.nodes.remove(id);
        }
    }

    fn has_thread_above(&self, id: &str) -> bool {
        let mut cursor = self.nodes.get(id).and_then(|n| n.parent.as_deref());
        while let Some(parent) = cursor {
            let Some(node) = self.nodes.get(parent) else {
                return false;
            };
            if node.has_thread {
                return true;
            }
            cursor = node.parent.as_deref();
        }
        false
    }

    /// Whether the goal is a task. See the module docs.
    pub fn is_task(&self, id: &str) -> bool {
        self.nodes.get(id).is_some_and(|n| !n.has_thread) && self.has_thread_above(id)
    }

    /// What to delete now that `id` has just become done or dropped. Call only
    /// on that transition: links, unlinks and moves never delete retroactively.
    ///
    /// A finished thread goal takes every branch below it that holds no thread
    /// goal, finished or not. A finished task goes once it has no children, and
    /// takes up with it any finished parent left the same way. Plan goals stay.
    pub fn deletions_after_finish(&self, id: &str) -> Vec<String> {
        let Some(node) = self.nodes.get(id) else {
            return Vec::new();
        };
        let mut doomed = Vec::new();
        if node.has_thread {
            for child in self.children(id) {
                self.prune_threadless(child, &mut doomed);
            }
        }
        let mut gone: HashSet<&str> = doomed.iter().map(String::as_str).collect();
        let mut chain = Vec::new();
        let mut cursor = Some(id);
        while let Some(current) = cursor {
            let Some(node) = self.nodes.get(current) else {
                break;
            };
            let childless = self
                .children(current)
                .iter()
                .all(|c| gone.contains(c.as_str()));
            if !finished(node.status) || !self.is_task(current) || !childless {
                break;
            }
            gone.insert(current);
            chain.push(current.to_owned());
            cursor = node.parent.as_deref();
        }
        doomed.extend(chain);
        doomed
    }

    /// Collect the threadless parts of a branch; true when the goal stays. A
    /// nested thread goal keeps its own tasks, so the walk stops there.
    fn prune_threadless(&self, id: &str, doomed: &mut Vec<String>) -> bool {
        if self.nodes.get(id).is_some_and(|n| n.has_thread) {
            return true;
        }
        let mut stays = false;
        for child in self.children(id) {
            stays |= self.prune_threadless(child, doomed);
        }
        if !stays {
            doomed.push(id.to_owned());
        }
        stays
    }

    /// Each goal's last activity: its own edits, replies in its links, and the
    /// same for everything below it, finished goals included.
    pub fn last_activity(&self) -> HashMap<&str, i64> {
        let mut activity: HashMap<&str, i64> = self
            .nodes
            .values()
            .map(|n| {
                let own = n.updated_at.max(n.last_reply_at.unwrap_or(i64::MIN));
                (n.id.as_str(), own)
            })
            .collect();
        // Deepest first, so each goal is final before it reaches its parent.
        let mut by_depth: Vec<(&str, usize)> = self
            .nodes
            .keys()
            .map(|id| (id.as_str(), self.layer(id)))
            .collect();
        by_depth.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        for (id, _) in by_depth {
            let latest = activity[id];
            if let Some(parent) = self.nodes[id].parent.as_deref() {
                if let Some(slot) = activity.get_mut(parent) {
                    *slot = (*slot).max(latest);
                }
            }
        }
        activity
    }

    /// Unfinished goals below layer 1 with no activity since the cutoff.
    pub fn inactive(&self, now: i64) -> HashSet<&str> {
        let activity = self.last_activity();
        self.nodes
            .values()
            .filter(|n| n.parent.is_some() && !finished(n.status))
            .filter(|n| now - activity[n.id.as_str()] >= INACTIVE_AFTER_SECS)
            .map(|n| n.id.as_str())
            .collect()
    }
}

/// Done or dropped.
pub fn finished(status: GoalStatus) -> bool {
    matches!(status, GoalStatus::Done | GoalStatus::Dropped)
}

/// Check a new id the way the document format did.
pub fn validate_id(id: &str) -> Result<(), GoalTreeError> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(GoalTreeError::Invalid(format!("invalid goal id {id:?}")))
    }
}

/// Check goal text and assignees.
pub fn validate_fields(
    title: Option<&str>,
    note: Option<&str>,
    assignees: &[String],
) -> Result<(), GoalTreeError> {
    if let Some(title) = title {
        validate_title(title)?;
    }
    if note.is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS) {
        return Err(GoalTreeError::Invalid(format!(
            "goal note exceeds {MAX_NOTE_CHARS} characters"
        )));
    }
    if assignees.len() > MAX_ASSIGNEES {
        return Err(GoalTreeError::Invalid(format!(
            "goal has more than {MAX_ASSIGNEES} assignees"
        )));
    }
    if let Some(bad) = assignees
        .iter()
        .find(|a| !is_lower_hex_64(&a.to_ascii_lowercase()))
    {
        return Err(GoalTreeError::Invalid(format!("invalid assignee {bad:?}")));
    }
    Ok(())
}

/// Check that `id` can sit under `parent`: no cycle, not too deep.
pub fn validate_move(graph: &GoalGraph, id: &str, parent: &str) -> Result<(), GoalTreeError> {
    let moved = graph.subtree(id);
    if moved.iter().any(|m| m == parent) {
        return Err(GoalTreeError::Invalid(
            "a goal cannot move under itself".into(),
        ));
    }
    let moved_depth = moved
        .iter()
        .map(|m| graph.layer(m) - graph.layer(id) + 1)
        .max()
        .unwrap_or(1);
    if graph.layer(parent) + moved_depth > MAX_DEPTH {
        return Err(GoalTreeError::Invalid(format!(
            "goal would be deeper than layer {MAX_DEPTH}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60;

    fn goal(id: &str, parent: Option<&str>) -> GoalShape {
        GoalShape {
            id: id.into(),
            parent: parent.map(Into::into),
            status: GoalStatus::Open,
            updated_at: 0,
            has_thread: false,
            last_reply_at: None,
        }
    }

    fn thread(id: &str, parent: &str) -> GoalShape {
        GoalShape {
            has_thread: true,
            ..goal(id, Some(parent))
        }
    }

    fn done(mut row: GoalShape) -> GoalShape {
        row.status = GoalStatus::Done;
        row
    }

    fn sorted(mut ids: Vec<String>) -> Vec<String> {
        ids.sort();
        ids
    }

    #[test]
    fn plan_goals_are_never_deleted() {
        let graph = GoalGraph::new(vec![
            goal("root", None),
            done(goal("plan", Some("root"))),
            done(goal("sub", Some("plan"))),
        ]);
        assert!(graph.deletions_after_finish("plan").is_empty());
        assert!(graph.deletions_after_finish("sub").is_empty());
    }

    #[test]
    fn a_link_on_layer_1_never_makes_a_thread_goal() {
        let mut root = goal("root", None);
        root.has_thread = true;
        let graph = GoalGraph::new(vec![root, done(goal("plan", Some("root")))]);
        assert!(!graph.get("root").unwrap().has_thread);
        assert!(graph.deletions_after_finish("plan").is_empty());
        assert!(graph.deletions_after_finish("root").is_empty());
    }

    #[test]
    fn a_finished_task_goes_and_takes_finished_childless_parents() {
        let graph = GoalGraph::new(vec![
            goal("root", None),
            thread("t", "root"),
            done(goal("a", Some("t"))),
            done(goal("b", Some("a"))),
        ]);
        assert_eq!(graph.deletions_after_finish("b"), vec!["b", "a"]);
    }

    #[test]
    fn a_finished_task_with_children_stays_until_they_go() {
        let graph = GoalGraph::new(vec![
            goal("root", None),
            thread("t", "root"),
            done(goal("a", Some("t"))),
            goal("b", Some("a")),
        ]);
        assert!(graph.deletions_after_finish("a").is_empty());
    }

    #[test]
    fn an_open_parent_stops_the_chain() {
        let graph = GoalGraph::new(vec![
            goal("root", None),
            thread("t", "root"),
            goal("a", Some("t")),
            done(goal("b", Some("a"))),
        ]);
        assert_eq!(graph.deletions_after_finish("b"), vec!["b"]);
    }

    #[test]
    fn a_dropped_parent_counts_as_finished() {
        let mut parent = goal("a", Some("t"));
        parent.status = GoalStatus::Dropped;
        let graph = GoalGraph::new(vec![
            goal("root", None),
            thread("t", "root"),
            parent,
            done(goal("b", Some("a"))),
        ]);
        assert_eq!(graph.deletions_after_finish("b"), vec!["b", "a"]);
    }

    #[test]
    fn a_finished_thread_goal_stays_and_takes_threadless_branches() {
        let graph = GoalGraph::new(vec![
            goal("root", None),
            done(thread("t", "root")),
            goal("open_task", Some("t")),
            goal("open_sub", Some("open_task")),
            done(goal("done_task", Some("t"))),
        ]);
        assert_eq!(
            sorted(graph.deletions_after_finish("t")),
            vec!["done_task", "open_sub", "open_task"]
        );
    }

    #[test]
    fn a_branch_holding_a_thread_goal_stays_but_loses_its_threadless_parts() {
        let graph = GoalGraph::new(vec![
            goal("root", None),
            done(thread("t", "root")),
            goal("mid", Some("t")),
            thread("inner", "mid"),
            goal("inner_task", Some("inner")),
            goal("side", Some("mid")),
        ]);
        assert_eq!(graph.deletions_after_finish("t"), vec!["side"]);
    }

    #[test]
    fn a_task_finishing_under_a_nested_thread_goal_stops_there() {
        let graph = GoalGraph::new(vec![
            goal("root", None),
            done(thread("outer", "root")),
            done(thread("inner", "outer")),
            done(goal("task", Some("inner"))),
        ]);
        assert_eq!(graph.deletions_after_finish("task"), vec!["task"]);
    }

    #[test]
    fn a_missing_goal_deletes_nothing() {
        assert!(GoalGraph::default().deletions_after_finish("x").is_empty());
    }

    #[test]
    fn activity_rolls_up_through_finished_goals() {
        let mut child = goal("child", Some("mid"));
        child.updated_at = 500;
        let mut mid = done(goal("mid", Some("top")));
        mid.updated_at = 100;
        let graph = GoalGraph::new(vec![
            goal("root", None),
            goal("top", Some("root")),
            mid,
            child,
        ]);
        let activity = graph.last_activity();
        assert_eq!(activity["top"], 500);
        assert_eq!(activity["root"], 500);
    }

    #[test]
    fn replies_count_as_activity() {
        let mut linked = thread("t", "root");
        linked.last_reply_at = Some(200 * DAY);
        let graph = GoalGraph::new(vec![goal("root", None), linked, goal("task", Some("t"))]);
        let now = 250 * DAY;
        let inactive = graph.inactive(now);
        assert!(!inactive.contains("t"));
        assert!(inactive.contains("task"));
    }

    #[test]
    fn inactive_starts_at_exactly_90_days_and_skips_root_and_finished() {
        let mut finished_goal = done(goal("gone", Some("root")));
        finished_goal.updated_at = 0;
        let mut fresh = goal("fresh", Some("root"));
        fresh.updated_at = 1;
        let graph = GoalGraph::new(vec![
            goal("root", None),
            goal("old", Some("root")),
            fresh,
            finished_goal,
        ]);
        let inactive = graph.inactive(90 * DAY);
        assert_eq!(inactive, HashSet::from(["old"]));
    }

    #[test]
    fn remove_and_set_parent_keep_children_right() {
        let mut graph = GoalGraph::new(vec![
            goal("root", None),
            goal("a", Some("root")),
            goal("b", Some("root")),
            goal("c", Some("a")),
        ]);
        graph.set_parent("c", "b");
        assert!(graph.children("a").is_empty());
        assert_eq!(graph.children("b"), ["c"]);
        assert_eq!(graph.layer("c"), 3);
        graph.remove(&["c".to_owned()]);
        assert!(graph.get("c").is_none());
        assert!(graph.children("b").is_empty());
    }

    #[test]
    fn insert_twice_does_not_duplicate_the_child_entry() {
        let mut graph = GoalGraph::new(vec![goal("root", None), goal("a", Some("root"))]);
        graph.insert(goal("a", Some("root")));
        assert_eq!(graph.children("root"), ["a"]);
    }

    #[test]
    fn move_rejects_cycles_and_excess_depth() {
        let mut rows = vec![goal("n1", None)];
        for i in 2..=MAX_DEPTH {
            let parent = format!("n{}", i - 1);
            rows.push(goal(&format!("n{i}"), Some(&parent)));
        }
        rows.push(goal("side", Some("n1")));
        rows.push(goal("side_child", Some("side")));
        let graph = GoalGraph::new(rows);
        assert!(validate_move(&graph, "n2", "n3").is_err());
        assert!(validate_move(&graph, "side", &format!("n{}", MAX_DEPTH - 1)).is_err());
        assert!(validate_move(&graph, "side_child", &format!("n{}", MAX_DEPTH - 1)).is_ok());
    }

    #[test]
    fn fields_are_checked() {
        assert!(validate_fields(Some(" "), None, &[]).is_err());
        assert!(validate_fields(Some("ok"), Some(&"x".repeat(MAX_NOTE_CHARS + 1)), &[]).is_err());
        assert!(validate_fields(Some("ok"), None, &["nope".into()]).is_err());
        assert!(validate_fields(Some("ok"), Some("note"), &["a".repeat(64)]).is_ok());
        assert!(validate_id("g_1").is_ok());
        assert!(validate_id("").is_err());
    }
}
