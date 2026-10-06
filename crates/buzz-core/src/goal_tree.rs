//! Goal tree (kind 40110) — the per-conversation goal hierarchy.
//!
//! One document per channel or DM holds the whole tree. Its root is the
//! conversation's single Layer 1 goal; every other node hangs below exactly
//! one parent, so a node's layer is its depth from the root. The latest event
//! is the live head and earlier events are its history, exactly like the
//! channel canvas (kind 40100).
//!
//! This module is zero-I/O: wire types, validation shared by the relay and
//! every writer, read helpers for prompt and UI projections, and the edit
//! operations writers re-apply after a revision conflict.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Current document schema version.
pub const GOAL_TREE_VERSION: u32 = 1;
/// Maximum UTF-8 bytes of a serialized goal tree.
pub const MAX_CONTENT_BYTES: usize = 64 * 1024;
/// Maximum nodes in one tree.
pub const MAX_NODES: usize = 500;
/// Maximum layer (root = layer 1).
pub const MAX_DEPTH: usize = 128;
/// Maximum characters in a node title.
pub const MAX_TITLE_CHARS: usize = 200;
/// Maximum characters in a node note.
pub const MAX_NOTE_CHARS: usize = 4_000;
/// Maximum characters in a node id.
pub const MAX_ID_CHARS: usize = 64;
/// Maximum assignees per node.
pub const MAX_ASSIGNEES: usize = 16;
/// Maximum linked threads per node.
pub const MAX_THREADS: usize = 32;

/// Progress state of one goal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// Not started.
    #[default]
    Open,
    /// Someone is working on it.
    InProgress,
    /// Achieved.
    Done,
    /// Abandoned without being achieved.
    Dropped,
}

impl GoalStatus {
    /// Parse the wire name (`open`, `in_progress`, `done`, `dropped`).
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "open" => Self::Open,
            "in_progress" => Self::InProgress,
            "done" => Self::Done,
            "dropped" => Self::Dropped,
            _ => return None,
        })
    }

    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Dropped => "dropped",
        }
    }
}

/// One goal in the tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalNode {
    /// Stable id, unique within the tree.
    pub id: String,
    /// Parent id; `None` only for the Layer 1 root.
    #[serde(default)]
    pub parent: Option<String>,
    /// One-line goal statement.
    pub title: String,
    /// Key information about this goal (markdown).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Progress state.
    #[serde(default)]
    pub status: GoalStatus,
    /// Assigned pubkeys (lowercase hex).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<String>,
    /// Root event ids of threads working on this goal (lowercase hex).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<String>,
    /// Sort key among siblings (ascending, ties by id).
    #[serde(default)]
    pub order: i64,
    /// Pubkey of the last editor of this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    /// Unix seconds of the last edit of this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

/// The whole goal tree of one conversation. An empty tree has no goals.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalTree {
    /// Schema version.
    pub v: u32,
    /// All nodes, in any order.
    #[serde(default)]
    pub nodes: Vec<GoalNode>,
}

/// Why a goal tree or edit was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GoalTreeError {
    /// Content is not a goal tree document.
    #[error("goal tree is not valid JSON: {0}")]
    Json(String),
    /// Structural or size rule violated.
    #[error("{0}")]
    Invalid(String),
    /// A referenced node does not exist.
    #[error("goal node not found: {0}")]
    NotFound(String),
}

fn invalid(message: impl Into<String>) -> GoalTreeError {
    GoalTreeError::Invalid(message.into())
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn validate_title(title: &str) -> Result<(), GoalTreeError> {
    if title.trim().is_empty() {
        return Err(invalid("goal title must not be empty"));
    }
    if title.contains('\n') || title.contains('\r') {
        return Err(invalid("goal title must be a single line"));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(invalid(format!(
            "goal title exceeds {MAX_TITLE_CHARS} characters"
        )));
    }
    Ok(())
}

impl GoalTree {
    /// An empty tree.
    pub fn empty() -> Self {
        Self {
            v: GOAL_TREE_VERSION,
            nodes: Vec::new(),
        }
    }

    /// Parse and fully validate serialized content.
    pub fn parse(content: &str) -> Result<Self, GoalTreeError> {
        if content.len() > MAX_CONTENT_BYTES {
            return Err(invalid(format!(
                "goal tree exceeds {MAX_CONTENT_BYTES} bytes"
            )));
        }
        let tree: Self =
            serde_json::from_str(content).map_err(|e| GoalTreeError::Json(e.to_string()))?;
        tree.validate()?;
        Ok(tree)
    }

    /// Serialize after validating, so no writer can publish a broken tree.
    pub fn to_content(&self) -> Result<String, GoalTreeError> {
        self.validate()?;
        let content =
            serde_json::to_string(self).map_err(|e| GoalTreeError::Json(e.to_string()))?;
        if content.len() > MAX_CONTENT_BYTES {
            return Err(invalid(format!(
                "goal tree exceeds {MAX_CONTENT_BYTES} bytes"
            )));
        }
        Ok(content)
    }

    /// Check every structural rule: one root, known parents, no cycles,
    /// bounded size and depth, well-formed fields, each thread linked once.
    pub fn validate(&self) -> Result<(), GoalTreeError> {
        if self.v != GOAL_TREE_VERSION {
            return Err(invalid(format!("unsupported goal tree version {}", self.v)));
        }
        if self.nodes.len() > MAX_NODES {
            return Err(invalid(format!("goal tree exceeds {MAX_NODES} nodes")));
        }
        let mut ids = HashSet::new();
        let mut threads = HashSet::new();
        let mut roots = 0usize;
        for node in &self.nodes {
            if !valid_id(&node.id) {
                return Err(invalid(format!("invalid goal id {:?}", node.id)));
            }
            if !ids.insert(node.id.as_str()) {
                return Err(invalid(format!("duplicate goal id {}", node.id)));
            }
            validate_title(&node.title)?;
            if node.note.chars().count() > MAX_NOTE_CHARS {
                return Err(invalid(format!(
                    "goal note exceeds {MAX_NOTE_CHARS} characters"
                )));
            }
            if node.assignees.len() > MAX_ASSIGNEES {
                return Err(invalid(format!(
                    "goal has more than {MAX_ASSIGNEES} assignees"
                )));
            }
            if node.threads.len() > MAX_THREADS {
                return Err(invalid(format!(
                    "goal has more than {MAX_THREADS} linked threads"
                )));
            }
            let mut assignees = HashSet::new();
            for pubkey in &node.assignees {
                if !is_lower_hex_64(pubkey) || !assignees.insert(pubkey) {
                    return Err(invalid(format!("invalid assignee {pubkey:?}")));
                }
            }
            for thread in &node.threads {
                if !is_lower_hex_64(thread) {
                    return Err(invalid(format!("invalid thread id {thread:?}")));
                }
                if !threads.insert(thread.as_str()) {
                    return Err(invalid(format!(
                        "thread {thread} is linked to more than one goal"
                    )));
                }
            }
            if node
                .updated_by
                .as_deref()
                .is_some_and(|p| !is_lower_hex_64(p))
            {
                return Err(invalid("invalid updated_by pubkey"));
            }
            if node.parent.is_none() {
                roots += 1;
            }
        }
        if !self.nodes.is_empty() && roots != 1 {
            return Err(invalid(format!(
                "goal tree must have exactly one layer 1 goal (found {roots})"
            )));
        }
        let parents: HashMap<&str, Option<&str>> = self
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.parent.as_deref()))
            .collect();
        for node in &self.nodes {
            if let Some(parent) = node.parent.as_deref() {
                if !parents.contains_key(parent) {
                    return Err(invalid(format!(
                        "goal {} has unknown parent {parent}",
                        node.id
                    )));
                }
            }
            // Walking up must reach the root within MAX_DEPTH steps; a longer
            // walk is either a cycle or a tree deeper than allowed.
            let mut depth = 1usize;
            let mut cursor = node.parent.as_deref();
            while let Some(id) = cursor {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(invalid(format!(
                        "goal {} is deeper than layer {MAX_DEPTH} or part of a cycle",
                        node.id
                    )));
                }
                cursor = parents.get(id).copied().flatten();
            }
        }
        Ok(())
    }

    /// The Layer 1 goal.
    pub fn root(&self) -> Option<&GoalNode> {
        self.nodes.iter().find(|n| n.parent.is_none())
    }

    /// Node by id.
    pub fn node(&self, id: &str) -> Option<&GoalNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn node_mut(&mut self, id: &str) -> Result<&mut GoalNode, GoalTreeError> {
        self.nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or_else(|| GoalTreeError::NotFound(id.to_string()))
    }

    /// Direct children of `id`, sorted by `order` then id.
    pub fn children(&self, id: &str) -> Vec<&GoalNode> {
        let mut children: Vec<&GoalNode> = self
            .nodes
            .iter()
            .filter(|n| n.parent.as_deref() == Some(id))
            .collect();
        children.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
        children
    }

    /// Nodes from the root down to `id`, inclusive.
    pub fn path(&self, id: &str) -> Vec<&GoalNode> {
        let mut path = Vec::new();
        let mut cursor = self.node(id);
        while let Some(node) = cursor {
            path.push(node);
            if path.len() > MAX_DEPTH {
                break;
            }
            cursor = node.parent.as_deref().and_then(|p| self.node(p));
        }
        path.reverse();
        path
    }

    /// Layer of `id` (root = 1).
    pub fn layer(&self, id: &str) -> Option<usize> {
        let path = self.path(id);
        (!path.is_empty()).then_some(path.len())
    }

    /// `id` and all its descendants in depth-first display order, each with
    /// its layer.
    pub fn subtree(&self, id: &str) -> Vec<(usize, &GoalNode)> {
        let mut out = Vec::new();
        if let (Some(node), Some(layer)) = (self.node(id), self.layer(id)) {
            self.walk(node, layer, &mut out);
        }
        out
    }

    /// The whole tree in depth-first display order, each node with its layer.
    pub fn outline(&self) -> Vec<(usize, &GoalNode)> {
        let mut out = Vec::new();
        if let Some(root) = self.root() {
            self.walk(root, 1, &mut out);
        }
        out
    }

    fn walk<'a>(&'a self, node: &'a GoalNode, layer: usize, out: &mut Vec<(usize, &'a GoalNode)>) {
        out.push((layer, node));
        if layer >= MAX_DEPTH {
            return;
        }
        for child in self.children(&node.id) {
            self.walk(child, layer + 1, out);
        }
    }

    /// All nodes on `layer`, in display order.
    pub fn layer_nodes(&self, layer: usize) -> Vec<&GoalNode> {
        self.outline()
            .into_iter()
            .filter(|(l, _)| *l == layer)
            .map(|(_, n)| n)
            .collect()
    }

    /// The goal a thread is linked to.
    pub fn node_for_thread(&self, thread_root: &str) -> Option<&GoalNode> {
        let thread_root = thread_root.to_ascii_lowercase();
        self.nodes
            .iter()
            .find(|n| n.threads.iter().any(|t| *t == thread_root))
    }

    /// `(done, total)` over the descendants of `id` (excluding `id`).
    pub fn progress(&self, id: &str) -> (usize, usize) {
        let descendants = self.subtree(id);
        let total = descendants.len().saturating_sub(1);
        let done = descendants
            .iter()
            .skip(1)
            .filter(|(_, n)| n.status == GoalStatus::Done)
            .count();
        (done, total)
    }

    /// Apply one edit, stamping the editor on touched nodes, then validate.
    /// On error the tree is left unchanged.
    pub fn apply(&mut self, op: &GoalOp, editor: &str, now: u64) -> Result<(), GoalTreeError> {
        let mut next = self.clone();
        next.apply_unchecked(op, editor, now)?;
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn touch(&mut self, id: &str, editor: &str, now: u64) -> Result<(), GoalTreeError> {
        let node = self.node_mut(id)?;
        node.updated_by = Some(editor.to_ascii_lowercase());
        node.updated_at = Some(now);
        Ok(())
    }

    fn next_order(&self, parent: &str) -> i64 {
        self.children(parent)
            .last()
            .map_or(0, |n| n.order.saturating_add(1))
    }

    fn apply_unchecked(
        &mut self,
        op: &GoalOp,
        editor: &str,
        now: u64,
    ) -> Result<(), GoalTreeError> {
        match op {
            GoalOp::SetRoot { id, title, note } => {
                validate_title(title)?;
                if let Some(root_id) = self.root().map(|r| r.id.clone()) {
                    let root = self.node_mut(&root_id)?;
                    root.title = title.clone();
                    if let Some(note) = note {
                        root.note = note.clone();
                    }
                    self.touch(&root_id, editor, now)?;
                } else {
                    self.nodes.push(GoalNode {
                        id: id.clone(),
                        parent: None,
                        title: title.clone(),
                        note: note.clone().unwrap_or_default(),
                        status: GoalStatus::Open,
                        assignees: Vec::new(),
                        threads: Vec::new(),
                        order: 0,
                        updated_by: None,
                        updated_at: None,
                    });
                    self.touch(id, editor, now)?;
                }
            }
            GoalOp::Add {
                id,
                parent,
                title,
                note,
                assignees,
            } => {
                if self.node(parent).is_none() {
                    return Err(GoalTreeError::NotFound(parent.clone()));
                }
                if self.node(id).is_some() {
                    return Err(invalid(format!("duplicate goal id {id}")));
                }
                let order = self.next_order(parent);
                self.nodes.push(GoalNode {
                    id: id.clone(),
                    parent: Some(parent.clone()),
                    title: title.clone(),
                    note: note.clone().unwrap_or_default(),
                    status: GoalStatus::Open,
                    assignees: assignees.iter().map(|a| a.to_ascii_lowercase()).collect(),
                    threads: Vec::new(),
                    order,
                    updated_by: None,
                    updated_at: None,
                });
                self.touch(id, editor, now)?;
            }
            GoalOp::Update {
                id,
                title,
                note,
                status,
                add_assignees,
                remove_assignees,
            } => {
                let node = self.node_mut(id)?;
                if let Some(title) = title {
                    node.title = title.clone();
                }
                if let Some(note) = note {
                    node.note = note.clone();
                }
                if let Some(status) = status {
                    node.status = *status;
                }
                for pubkey in add_assignees {
                    let pubkey = pubkey.to_ascii_lowercase();
                    if !node.assignees.contains(&pubkey) {
                        node.assignees.push(pubkey);
                    }
                }
                for pubkey in remove_assignees {
                    let pubkey = pubkey.to_ascii_lowercase();
                    node.assignees.retain(|a| *a != pubkey);
                }
                self.touch(id, editor, now)?;
            }
            GoalOp::Move { id, parent, order } => {
                let node = self
                    .node(id)
                    .ok_or_else(|| GoalTreeError::NotFound(id.clone()))?;
                if node.parent.is_none() {
                    return Err(invalid("the layer 1 goal cannot be moved"));
                }
                if self.node(parent).is_none() {
                    return Err(GoalTreeError::NotFound(parent.clone()));
                }
                if self.subtree(id).iter().any(|(_, n)| n.id == *parent) {
                    return Err(invalid("a goal cannot move under itself"));
                }
                let order = order.unwrap_or_else(|| self.next_order(parent));
                let node = self.node_mut(id)?;
                node.parent = Some(parent.clone());
                node.order = order;
                self.touch(id, editor, now)?;
            }
            GoalOp::Remove { id, recursive } => {
                let doomed: HashSet<String> =
                    self.subtree(id).iter().map(|(_, n)| n.id.clone()).collect();
                if doomed.is_empty() {
                    return Err(GoalTreeError::NotFound(id.clone()));
                }
                if doomed.len() > 1 && !recursive {
                    return Err(invalid(format!(
                        "goal {id} has {} sub-goals; remove them too with recursive",
                        doomed.len() - 1
                    )));
                }
                self.nodes.retain(|n| !doomed.contains(&n.id));
            }
            GoalOp::Link { id, thread } => {
                let thread = thread.to_ascii_lowercase();
                if !is_lower_hex_64(&thread) {
                    return Err(invalid(format!("invalid thread id {thread:?}")));
                }
                self.node_mut(id)?;
                for node in &mut self.nodes {
                    node.threads.retain(|t| *t != thread);
                }
                self.node_mut(id)?.threads.push(thread);
                self.touch(id, editor, now)?;
            }
            GoalOp::Unlink { thread } => {
                let thread = thread.to_ascii_lowercase();
                for node in &mut self.nodes {
                    node.threads.retain(|t| *t != thread);
                }
            }
        }
        Ok(())
    }
}

/// One edit to a goal tree. Writers keep the op, not the resulting tree, so
/// after a revision conflict they re-read the head and re-apply the same op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoalOp {
    /// Create the layer 1 goal (with `id`) or rewrite the existing one.
    SetRoot {
        /// Id used only when creating.
        id: String,
        /// Goal sentence.
        title: String,
        /// Replacement note, if given.
        note: Option<String>,
    },
    /// Add a child goal at the end of `parent`'s children.
    Add {
        /// New node id.
        id: String,
        /// Parent node id.
        parent: String,
        /// Goal sentence.
        title: String,
        /// Note, if any.
        note: Option<String>,
        /// Initial assignees.
        assignees: Vec<String>,
    },
    /// Change fields of one goal.
    Update {
        /// Node id.
        id: String,
        /// New title.
        title: Option<String>,
        /// New note (empty string clears).
        note: Option<String>,
        /// New status.
        status: Option<GoalStatus>,
        /// Pubkeys to assign.
        add_assignees: Vec<String>,
        /// Pubkeys to unassign.
        remove_assignees: Vec<String>,
    },
    /// Re-parent a goal (not the root).
    Move {
        /// Node id.
        id: String,
        /// New parent id.
        parent: String,
        /// Sort key; appended last when absent.
        order: Option<i64>,
    },
    /// Delete a goal. Removing the root empties the tree.
    Remove {
        /// Node id.
        id: String,
        /// Required when the goal has sub-goals.
        recursive: bool,
    },
    /// Link a thread to a goal, moving it off any other goal.
    Link {
        /// Node id.
        id: String,
        /// Thread root event id.
        thread: String,
    },
    /// Detach a thread from whichever goal it is linked to.
    Unlink {
        /// Thread root event id.
        thread: String,
    },
}

/// A fresh node id: `g_` + 16 random lowercase hex characters.
pub fn new_node_id() -> String {
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    format!("g_{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const THREAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn add(tree: &mut GoalTree, id: &str, parent: &str) {
        tree.apply(
            &GoalOp::Add {
                id: id.into(),
                parent: parent.into(),
                title: format!("goal {id}"),
                note: None,
                assignees: vec![],
            },
            ED,
            1,
        )
        .unwrap();
    }

    fn sample() -> GoalTree {
        let mut tree = GoalTree::empty();
        tree.apply(
            &GoalOp::SetRoot {
                id: "root".into(),
                title: "Ship the app".into(),
                note: None,
            },
            ED,
            1,
        )
        .unwrap();
        add(&mut tree, "a", "root");
        add(&mut tree, "b", "root");
        add(&mut tree, "a1", "a");
        add(&mut tree, "a2", "a");
        add(&mut tree, "b1", "b");
        add(&mut tree, "a1x", "a1");
        tree
    }

    #[test]
    fn round_trips_and_validates() {
        let tree = sample();
        let content = tree.to_content().unwrap();
        assert_eq!(GoalTree::parse(&content).unwrap(), tree);
        assert_eq!(GoalTree::parse(r#"{"v":1}"#).unwrap(), GoalTree::empty());
    }

    #[test]
    fn outline_layers_and_paths() {
        let tree = sample();
        let outline: Vec<(usize, &str)> = tree
            .outline()
            .into_iter()
            .map(|(l, n)| (l, n.id.as_str()))
            .collect();
        assert_eq!(
            outline,
            vec![
                (1, "root"),
                (2, "a"),
                (3, "a1"),
                (4, "a1x"),
                (3, "a2"),
                (2, "b"),
                (3, "b1")
            ]
        );
        let path: Vec<&str> = tree.path("a1x").iter().map(|n| n.id.as_str()).collect();
        assert_eq!(path, vec!["root", "a", "a1", "a1x"]);
        let layer3: Vec<&str> = tree.layer_nodes(3).iter().map(|n| n.id.as_str()).collect();
        assert_eq!(layer3, vec!["a1", "a2", "b1"]);
        assert_eq!(tree.layer("b1"), Some(3));
    }

    #[test]
    fn rejects_second_root_unknown_parent_and_cycle() {
        let mut tree = sample();
        tree.nodes.push(GoalNode {
            parent: None,
            id: "root2".into(),
            ..tree.nodes[0].clone()
        });
        assert!(tree.validate().is_err());

        let mut tree = sample();
        tree.nodes[1].parent = Some("ghost".into());
        assert!(tree.validate().is_err());

        let mut tree = sample();
        // a -> a1 -> a -> ... never reaches the root.
        let a = tree.nodes.iter().position(|n| n.id == "a").unwrap();
        tree.nodes[a].parent = Some("a1".into());
        assert!(tree.validate().is_err());
    }

    #[test]
    fn rejects_multiline_title_and_oversize() {
        let mut tree = sample();
        assert!(tree
            .apply(
                &GoalOp::SetRoot {
                    id: "x".into(),
                    title: "one\ntwo".into(),
                    note: None
                },
                ED,
                2
            )
            .is_err());
        assert_eq!(tree.root().unwrap().title, "Ship the app");
        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        assert!(tree
            .apply(
                &GoalOp::Update {
                    id: "a".into(),
                    title: Some(long),
                    note: None,
                    status: None,
                    add_assignees: vec![],
                    remove_assignees: vec![]
                },
                ED,
                2
            )
            .is_err());
    }

    #[test]
    fn move_cannot_create_cycle_or_move_root() {
        let mut tree = sample();
        let under_self = GoalOp::Move {
            id: "a".into(),
            parent: "a1x".into(),
            order: None,
        };
        assert!(tree.apply(&under_self, ED, 2).is_err());
        let root = GoalOp::Move {
            id: "root".into(),
            parent: "a".into(),
            order: None,
        };
        assert!(tree.apply(&root, ED, 2).is_err());
        let ok = GoalOp::Move {
            id: "a2".into(),
            parent: "b".into(),
            order: None,
        };
        tree.apply(&ok, ED, 2).unwrap();
        assert_eq!(tree.node("a2").unwrap().parent.as_deref(), Some("b"));
    }

    #[test]
    fn remove_requires_recursive_for_subgoals_and_root_empties_tree() {
        let mut tree = sample();
        let shallow = GoalOp::Remove {
            id: "a".into(),
            recursive: false,
        };
        assert!(tree.apply(&shallow, ED, 2).is_err());
        tree.apply(
            &GoalOp::Remove {
                id: "a1x".into(),
                recursive: false,
            },
            ED,
            2,
        )
        .unwrap();
        tree.apply(
            &GoalOp::Remove {
                id: "root".into(),
                recursive: true,
            },
            ED,
            2,
        )
        .unwrap();
        assert!(tree.nodes.is_empty());
        assert!(tree.validate().is_ok());
    }

    #[test]
    fn link_moves_thread_between_goals() {
        let mut tree = sample();
        let link = |id: &str| GoalOp::Link {
            id: id.into(),
            thread: THREAD.to_ascii_uppercase(),
        };
        tree.apply(&link("a1"), ED, 2).unwrap();
        tree.apply(&link("b1"), ED, 3).unwrap();
        assert_eq!(tree.node_for_thread(THREAD).unwrap().id, "b1");
        assert!(tree.node("a1").unwrap().threads.is_empty());
        tree.apply(
            &GoalOp::Unlink {
                thread: THREAD.into(),
            },
            ED,
            4,
        )
        .unwrap();
        assert!(tree.node_for_thread(THREAD).is_none());
    }

    #[test]
    fn progress_counts_done_descendants() {
        let mut tree = sample();
        tree.apply(
            &GoalOp::Update {
                id: "a1".into(),
                title: None,
                note: None,
                status: Some(GoalStatus::Done),
                add_assignees: vec![],
                remove_assignees: vec![],
            },
            ED,
            2,
        )
        .unwrap();
        assert_eq!(tree.progress("a"), (1, 3));
        assert_eq!(tree.progress("root"), (1, 6));
    }

    #[test]
    fn rejects_bad_assignee_and_duplicate_thread() {
        let mut tree = sample();
        tree.nodes[1].assignees = vec!["nothex".into()];
        assert!(tree.validate().is_err());
        let mut tree = sample();
        tree.nodes[1].threads = vec![THREAD.into()];
        tree.nodes[2].threads = vec![THREAD.into()];
        assert!(tree.validate().is_err());
    }
}
