//! One-off import of goal tree heads (kind 40110) into the `goals` tables.
//! Reads `export_heads.sql` output on stdin and writes SQL on stdout; a
//! summary per room goes to stderr. See `scripts/cutover/goal_store/README.md`.
//!
//! Rows the deletion rule would have removed are left out, using the relay's
//! own rule, so the import ends in a state the relay could have reached.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};

use kuzz_core::goal_store::{GoalGraph, GoalShape, finished};
use kuzz_core::goal_tree::{GoalNode, GoalTree};
use serde::Deserialize;

/// One line of `export_heads.sql`.
#[derive(Deserialize)]
struct Head {
    community: String,
    channel: String,
    created_at: u64,
    content: String,
    /// Linked ids that count as threads: a top-level message with replies.
    threads: Vec<String>,
}

/// What the import does to one room.
struct Import<'a> {
    goals: Vec<&'a GoalNode>,
    /// Layer 1 takes no links.
    dropped_links: usize,
    /// The newest edit of what was left out, by the nearest goal kept: deleting
    /// a task counts as activity on what is above it.
    touched: HashMap<&'a str, u64>,
}

fn import<'a>(tree: &'a GoalTree, threads: &HashSet<&str>) -> Import<'a> {
    let is_thread = |node: &GoalNode| {
        node.parent.is_some() && node.threads.iter().any(|t| threads.contains(t.as_str()))
    };
    let mut graph = GoalGraph::new(
        tree.nodes
            .iter()
            .map(|node| GoalShape {
                id: node.id.clone(),
                parent: node.parent.clone(),
                status: node.status,
                order: node.order,
                updated_at: 0,
                has_thread: is_thread(node),
                last_reply_at: None,
            })
            .collect(),
    );
    // One finish can leave another goal deletable, so run until nothing moves.
    loop {
        let mut changed = false;
        for node in tree.nodes.iter().filter(|node| finished(node.status)) {
            let doomed = graph.deletions_after_finish(&node.id);
            changed |= !doomed.is_empty();
            graph.remove(&doomed);
        }
        if !changed {
            break;
        }
    }
    let parent_of: HashMap<&str, &str> = tree
        .nodes
        .iter()
        .filter_map(|node| Some((node.id.as_str(), node.parent.as_deref()?)))
        .collect();
    let mut touched = HashMap::new();
    for node in tree.nodes.iter().filter(|n| graph.get(&n.id).is_none()) {
        let mut id = node.id.as_str();
        while graph.get(id).is_none() {
            id = parent_of[id];
        }
        let at = touched.entry(id).or_default();
        *at = node.updated_at.unwrap_or(0).max(*at);
    }
    Import {
        touched,
        goals: tree
            .nodes
            .iter()
            .filter(|node| graph.get(&node.id).is_some())
            .collect(),
        dropped_links: tree
            .nodes
            .iter()
            .filter(|node| node.parent.is_none())
            .map(|node| node.threads.len())
            .sum(),
    }
}

fn text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn timestamp(unix: u64) -> String {
    format!("to_timestamp({unix})")
}

fn room_sql(head: &Head, import: &Import) -> String {
    let ids = format!("{}, {}", text(&head.community), text(&head.channel));
    let mut sql = String::new();
    // One statement, so a parent may come after its child: Postgres checks the
    // foreign key when the statement ends.
    let rows: Vec<String> = import
        .goals
        .iter()
        .map(|node| {
            let edited = node.updated_at.unwrap_or(head.created_at);
            let touched = import.touched.get(node.id.as_str()).copied();
            let at = timestamp(touched.unwrap_or(0).max(edited));
            let assignees: Vec<String> = node.assignees.iter().map(|a| text(a)).collect();
            format!(
                "({ids}, {}, {}, {}, {}, {}, {}, ARRAY[{}]::text[], {}, {at}, {at})",
                text(&node.id),
                node.parent.as_deref().map_or("NULL".to_owned(), text),
                text(&node.title),
                text(&node.note),
                text(node.status.as_str()),
                node.order,
                assignees.join(", "),
                node.updated_by.as_deref().map_or("NULL".to_owned(), text),
            )
        })
        .collect();
    if !rows.is_empty() {
        sql.push_str(&format!(
            "INSERT INTO goals (community_id, channel_id, id, parent_id, title, note, status,              order_key, assignees, updated_by, updated_at, created_at) VALUES
{};
",
            rows.join(",
"),
        ));
    }
    for node in import.goals.iter().filter(|node| node.parent.is_some()) {
        for link in &node.threads {
            sql.push_str(&format!(
                "INSERT INTO goal_links (community_id, channel_id, event_id, goal_id) VALUES \
                 ({ids}, decode({}, 'hex'), {});\n",
                text(link),
                text(&node.id),
            ));
        }
    }
    sql
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = io::stdout().lock();
    // Every room, not only those with a head: a room first given goals during a
    // switch that was rolled back has rows and no head.
    out.write_all(b"DELETE FROM goals;\n")?;
    for line in io::stdin().lock().lines() {
        let head: Head = serde_json::from_str(&line?)?;
        let tree = GoalTree::parse(&head.content)?;
        let threads = head.threads.iter().map(String::as_str).collect();
        let import = import(&tree, &threads);
        out.write_all(room_sql(&head, &import).as_bytes())?;
        eprintln!(
            "{}: {} goals, {} left out, {} layer 1 links dropped",
            head.channel,
            import.goals.len(),
            tree.nodes.len() - import.goals.len(),
            import.dropped_links,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "aa";
    const OTHER: &str = "bb";
    const MESSAGE: &str = "cc";

    fn goal(id: &str, parent: Option<&str>, status: &str, links: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "id": id, "parent": parent, "title": id, "status": status, "threads": links,
        })
    }

    fn kept(nodes: Vec<serde_json::Value>) -> Vec<String> {
        let tree: GoalTree = serde_json::from_value(serde_json::json!({ "v": 1, "nodes": nodes }))
            .expect("valid tree");
        let threads = HashSet::from([THREAD, OTHER]);
        let mut ids: Vec<String> = import(&tree, &threads)
            .goals
            .iter()
            .map(|node| node.id.clone())
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn plan_goals_stay_finished_or_not() {
        assert_eq!(
            kept(vec![
                goal("root", None, "open", &[]),
                goal("plan", Some("root"), "done", &[]),
                goal("step", Some("plan"), "dropped", &[]),
            ]),
            ["plan", "root", "step"],
        );
    }

    #[test]
    fn finished_tasks_go_with_their_finished_parents() {
        assert_eq!(
            kept(vec![
                goal("root", None, "open", &[]),
                goal("work", Some("root"), "open", &[THREAD]),
                goal("mid", Some("work"), "done", &[]),
                goal("leaf", Some("mid"), "done", &[]),
                goal("open", Some("work"), "open", &[]),
            ]),
            ["open", "root", "work"],
        );
    }

    #[test]
    fn a_finished_thread_goal_takes_unfinished_branches_but_not_nested_threads() {
        assert_eq!(
            kept(vec![
                goal("root", None, "open", &[]),
                goal("work", Some("root"), "done", &[THREAD]),
                goal("task", Some("work"), "open", &[]),
                goal("branch", Some("work"), "open", &[]),
                goal("nested", Some("branch"), "open", &[OTHER]),
                goal("bare", Some("nested"), "open", &[]),
            ]),
            ["bare", "branch", "nested", "root", "work"],
        );
    }

    #[test]
    fn a_link_without_replies_is_no_thread_goal() {
        assert_eq!(
            kept(vec![
                goal("root", None, "open", &[]),
                goal("plan", Some("root"), "done", &[MESSAGE]),
                goal("step", Some("plan"), "open", &[]),
            ]),
            ["plan", "root", "step"],
        );
    }

    #[test]
    fn a_task_goes_even_with_a_message_link() {
        assert_eq!(
            kept(vec![
                goal("root", None, "open", &[]),
                goal("work", Some("root"), "open", &[THREAD]),
                goal("task", Some("work"), "done", &[MESSAGE]),
            ]),
            ["root", "work"],
        );
    }

    #[test]
    fn layer_one_links_are_dropped_and_text_is_quoted() {
        let mut root = goal("root", None, "open", &[THREAD]);
        root["title"] = "it's".into();
        let tree: GoalTree =
            serde_json::from_value(serde_json::json!({ "v": 1, "nodes": [root] })).unwrap();
        let threads = HashSet::from([THREAD]);
        let result = import(&tree, &threads);
        assert_eq!(result.dropped_links, 1);
        let head = Head {
            community: "c".into(),
            channel: "r".into(),
            created_at: 7,
            content: String::new(),
            threads: Vec::new(),
        };
        let sql = room_sql(&head, &result);
        assert!(sql.contains("'it''s'"));
        assert!(sql.contains("to_timestamp(7), to_timestamp(7)"));
        assert!(!sql.contains("goal_links"));
    }

    #[test]
    fn a_left_out_task_counts_as_activity_on_the_goal_kept_above_it() {
        let mut work = goal("work", Some("root"), "open", &[THREAD]);
        work["updated_at"] = 100.into();
        let mut task = goal("task", Some("work"), "done", &[]);
        task["updated_at"] = 200.into();
        let tree: GoalTree = serde_json::from_value(serde_json::json!({
            "v": 1,
            "nodes": [goal("root", None, "open", &[]), work, task],
        }))
        .unwrap();
        let threads = HashSet::from([THREAD]);
        let result = import(&tree, &threads);
        let head = Head {
            community: "c".into(),
            channel: "r".into(),
            created_at: 7,
            content: String::new(),
            threads: Vec::new(),
        };
        let sql = room_sql(&head, &result);
        assert!(sql.contains("to_timestamp(200), to_timestamp(200)"));
        assert!(!sql.contains("to_timestamp(100)"));
    }

    #[test]
    fn a_room_is_one_insert_so_rows_may_precede_their_parent() {
        let tree: GoalTree = serde_json::from_value(serde_json::json!({
            "v": 1,
            "nodes": [goal("step", Some("root"), "open", &[]), goal("root", None, "open", &[])],
        }))
        .unwrap();
        let head = Head {
            community: "c".into(),
            channel: "r".into(),
            created_at: 7,
            content: String::new(),
            threads: Vec::new(),
        };
        let sql = room_sql(&head, &import(&tree, &HashSet::new()));
        assert_eq!(sql.matches("INSERT INTO goals").count(), 1);
    }
}
