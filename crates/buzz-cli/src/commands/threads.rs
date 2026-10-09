//! Task threads: a thread opened for one piece of work, linked to the place
//! that asked for it, that reports its result back and then closes.
//!
//! Wire format (kind 9 only, no new relay kinds):
//! - Kickoff reply in the thread: `["buzz:task", <title>]`,
//!   `["buzz:parent", "main"]`, and a `p` tag for the requester.
//! - Result on the channel main timeline: `["buzz:sent-from-thread", <root>,
//!   <excerpt>]`, `["buzz:thread-closed", <root>]`, a `p` tag for the
//!   requester, and a `p` tag for the closing agent itself so its harness can
//!   relay the result to its main-timeline session.
//! - Close marker reply in the thread: `["buzz:thread-closed", <root>]` and a
//!   NIP-18 `q` tag pointing at the result. A thread whose latest message is a
//!   close marker reads as closed; any later message reopens it.
//! - A best-effort `✅` reaction on the thread root.

use buzz_core::goal_tree::{GoalStatus, GoalTree};
use clap::Subcommand;

use crate::client::BuzzClient;
use crate::commands::messages::{
    fetch_event, send_message, set_thread_name, thread_ref_from_event, SendMessageParams,
};
use crate::error::CliError;
use crate::validate::{parse_uuid, read_or_stdin, validate_hex64};

pub const TASK_TAG: &str = "buzz:task";
pub const PARENT_TAG: &str = "buzz:parent";
pub const THREAD_CLOSED_TAG: &str = "buzz:thread-closed";
const SENT_FROM_THREAD_TAG: &str = "buzz:sent-from-thread";
/// Parent scope value for a task thread started from the channel main timeline.
const PARENT_MAIN: &str = "main";
const MAX_TITLE_CHARS: usize = 120;
/// Reaction `close` adds to the thread root.
const DONE_REACTION: &str = "✅";
/// Matches Desktop's sent-from-thread excerpt limit.
const MAX_EXCERPT_CHARS: usize = 64;

#[derive(Subcommand)]
pub enum ThreadsCmd {
    /// Open a task thread under a top-level message, linked back to the main timeline
    #[command(
        after_help = "Example:\n  printf 'Plan:\\n- step 1\\n' | buzz threads start --channel <UUID> --from <EVENT_ID> --title \"Quote rendering\" --brief -"
    )]
    Start {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Top-level message that becomes the thread root (64-char hex)
        #[arg(long, value_name = "EVENT_ID")]
        from: String,
        /// Task title; also set as the shared thread name when it fits
        #[arg(long)]
        title: String,
        /// Kickoff details (use '-' to read from stdin)
        #[arg(long)]
        brief: Option<String>,
        /// Who the result is reported to (hex or npub); defaults to the author of --from
        #[arg(long)]
        requester: Option<String>,
    },
    /// Report a task thread's result to the main timeline and close the thread
    #[command(
        after_help = "Example:\n  printf '@akak Done: ...\\n' | buzz threads close --channel <UUID> --thread <ROOT_ID> --summary -"
    )]
    Close {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Thread root event ID (64-char hex)
        #[arg(long, value_name = "ROOT_ID")]
        thread: String,
        /// Result posted to the main timeline (use '-' to read from stdin)
        #[arg(long)]
        summary: String,
        /// Override who the result is reported to (hex or npub)
        #[arg(long)]
        requester: Option<String>,
        /// Also mark the goal this thread is linked to as done
        #[arg(long)]
        goal_done: bool,
        /// With --goal-done: what was delivered, saved as the goal's note
        #[arg(long, requires = "goal_done")]
        goal_note: Option<String>,
    },
}

pub async fn dispatch(cmd: ThreadsCmd, client: &BuzzClient) -> Result<(), CliError> {
    match cmd {
        ThreadsCmd::Start {
            channel,
            from,
            title,
            brief,
            requester,
        } => start(client, &channel, &from, &title, brief, requester).await,
        ThreadsCmd::Close {
            channel,
            thread,
            summary,
            requester,
            goal_done,
            goal_note,
        } => {
            let goal_done = goal_done.then(|| goal_note.map(|n| read_or_stdin(&n)).transpose());
            let goal_done = goal_done.transpose()?;
            close(client, &channel, &thread, &summary, requester, goal_done).await
        }
    }
}

async fn start(
    client: &BuzzClient,
    channel: &str,
    from: &str,
    title: &str,
    brief: Option<String>,
    requester: Option<String>,
) -> Result<(), CliError> {
    let title = validate_title(title)?;
    let root = load_top_level_message(client, channel, from).await?;
    let requester = match requester {
        Some(requester) => requester,
        None => event_author(&root)?,
    };
    let brief = brief.map(|b| read_or_stdin(&b)).transpose()?;
    let content = match brief.as_deref().map(str::trim) {
        Some(brief) if !brief.is_empty() => format!("**{title}**\n\n{brief}"),
        _ => format!("**{title}**"),
    };

    let kickoff = send_message(
        client,
        SendMessageParams {
            channel_id: channel.to_string(),
            content,
            kind: None,
            reply_to: Some(from.to_ascii_lowercase()),
            quote: None,
            broadcast: false,
            files: Vec::new(),
            mentions: vec![requester],
            extra_tags: vec![tag([TASK_TAG, title])?, tag([PARENT_TAG, PARENT_MAIN])?],
        },
    )
    .await?;

    // The shared thread name makes the task visible in reply summaries and the
    // sidebar. It is best effort: an older relay rejects the kind.
    let name = thread_name_for(title);
    let thread_name = match set_thread_name(client, channel, from, &name).await {
        Ok(_) => Some(name),
        Err(e) => {
            eprintln!("warning: could not set the thread name ({e})");
            None
        }
    };

    println!(
        "{}",
        serde_json::json!({
            "thread_root": from.to_ascii_lowercase(),
            "kickoff": kickoff,
            "thread_name": thread_name,
        })
    );
    Ok(())
}

async fn close(
    client: &BuzzClient,
    channel: &str,
    thread: &str,
    summary: &str,
    requester: Option<String>,
    goal_done: Option<Option<String>>,
) -> Result<(), CliError> {
    let summary = read_or_stdin(summary)?;
    if summary.trim().is_empty() {
        return Err(CliError::Usage("--summary must not be empty".into()));
    }
    let root_id = thread.to_ascii_lowercase();
    let root = load_top_level_message(client, channel, &root_id).await?;
    // Before anything is posted, so a failed goal write leaves the thread
    // open and the whole close can simply be retried.
    let goal = close_goal(client, channel, &root_id, goal_done).await?;

    let replies = client
        .query(&serde_json::json!({
            "kinds": [9],
            "#h": [channel],
            "#e": [&root_id],
            "limit": 500,
        }))
        .await?;
    let replies: Vec<serde_json::Value> = serde_json::from_str(&replies)
        .map_err(|e| CliError::Other(format!("invalid thread response: {e}")))?;
    let task = find_task(&replies);

    let requester = match requester.or_else(|| task.as_ref().and_then(|t| t.requester.clone())) {
        Some(requester) => requester,
        None => event_author(&root)?,
    };
    let excerpt = result_excerpt(task.as_ref(), &root);
    let mut sent_from = vec![SENT_FROM_THREAD_TAG.to_string(), root_id.clone()];
    sent_from.extend(excerpt);

    let result = send_message(
        client,
        SendMessageParams {
            channel_id: channel.to_string(),
            content: summary,
            kind: None,
            reply_to: None,
            quote: None,
            broadcast: false,
            files: Vec::new(),
            mentions: vec![requester],
            extra_tags: vec![
                nostr::Tag::parse(sent_from).map_err(|e| CliError::Other(e.to_string()))?,
                tag([THREAD_CLOSED_TAG, &root_id])?,
                // Self-address the result so this agent's own harness, which
                // subscribes to its mentions, hands it to the main-timeline
                // session. A raw `p` adds no `@` text or mention snapshot.
                tag(["p", &client.pubkey().to_hex()])?,
            ],
        },
    )
    .await?;
    let result_id = result
        .get("event_id")
        .and_then(|id| id.as_str())
        .ok_or_else(|| CliError::Other(format!("result was not accepted: {result}")))?
        .to_string();

    let marker = send_message(
        client,
        SendMessageParams {
            channel_id: channel.to_string(),
            content: "✅ Task closed. The result is posted on the channel timeline.".into(),
            kind: None,
            reply_to: Some(root_id.clone()),
            quote: Some(result_id),
            broadcast: false,
            files: Vec::new(),
            mentions: Vec::new(),
            extra_tags: vec![tag([THREAD_CLOSED_TAG, &root_id])?],
        },
    )
    .await?;

    // Mark the thread root done so the closed task reads at a glance in the
    // timeline. Best effort: the result and close marker are already posted.
    let root_reaction = match react_done(client, &root_id).await {
        Ok(()) => Some(DONE_REACTION),
        Err(e) => {
            eprintln!("warning: could not add {DONE_REACTION} to the thread root ({e})");
            None
        }
    };

    println!(
        "{}",
        serde_json::json!({
            "thread_root": root_id,
            "result": result,
            "close_marker": marker,
            "root_reaction": root_reaction,
            "goal": goal.json(),
            "warning": goal.warning(),
        })
    );
    Ok(())
}

/// What closing a thread does to the goal it is linked to.
#[derive(Debug, PartialEq, Eq)]
enum CloseGoal {
    /// The thread is not linked to a goal (or the conversation has none).
    Unlinked,
    /// `--goal-done`: the linked goal was marked done.
    MarkedDone {
        id: String,
        report: serde_json::Value,
    },
    /// The linked goal was left as it was.
    Left { id: String, status: GoalStatus },
}

impl CloseGoal {
    fn plan(tree: &GoalTree, root_id: &str, goal_done: bool) -> Self {
        match tree.node_for_thread(root_id) {
            None => Self::Unlinked,
            Some(node) if goal_done => Self::MarkedDone {
                id: node.id.clone(),
                report: serde_json::Value::Null,
            },
            Some(node) => Self::Left {
                id: node.id.clone(),
                status: node.status,
            },
        }
    }

    fn json(&self) -> serde_json::Value {
        match self {
            Self::Unlinked => serde_json::Value::Null,
            Self::MarkedDone { id, report } => {
                let mut out = serde_json::json!({ "id": id, "status": "done" });
                if let (Some(out), Some(report)) = (out.as_object_mut(), report.as_object()) {
                    out.extend(report.clone());
                }
                out
            }
            Self::Left { id, status } => {
                serde_json::json!({ "id": id, "status": status.as_str() })
            }
        }
    }

    fn warning(&self) -> Option<String> {
        match self {
            Self::Left { id, status } if *status == GoalStatus::InProgress => Some(format!(
                "linked goal {id} is still {}; pass --goal-done to mark it done",
                status.as_str()
            )),
            _ => None,
        }
    }
}

async fn close_goal(
    client: &BuzzClient,
    channel: &str,
    root_id: &str,
    goal_done: Option<Option<String>>,
) -> Result<CloseGoal, CliError> {
    let Some(head) = super::goals::fetch_head(client, channel, goal_done.is_some()).await? else {
        return Ok(CloseGoal::Unlinked);
    };
    let mut goal = CloseGoal::plan(&head.tree, root_id, goal_done.is_some());
    if let (CloseGoal::MarkedDone { id, report }, Some(note)) = (&mut goal, goal_done) {
        let op = super::goals::status_note_op(id, GoalStatus::Done, note);
        *report = super::goals::update_goal(client, channel, op).await?;
        // Keep only the tree report; the event id is not this goal's concern.
        if let Some(r) = report.as_object_mut() {
            r.remove("event_id");
            r.remove("accepted");
        }
    }
    Ok(goal)
}

async fn react_done(client: &BuzzClient, root_id: &str) -> Result<(), CliError> {
    let target = nostr::EventId::parse(root_id)
        .map_err(|e| CliError::Usage(format!("invalid event ID: {e}")))?;
    let builder = buzz_sdk::build_reaction(target, DONE_REACTION)
        .map_err(|e| CliError::Other(format!("build_reaction failed: {e}")))?;
    let event = client.sign_event(builder)?;
    client.submit_event(event).await?;
    Ok(())
}

/// Fetch `event_id` and require a top-level message in `channel`.
async fn load_top_level_message(
    client: &BuzzClient,
    channel: &str,
    event_id: &str,
) -> Result<serde_json::Value, CliError> {
    validate_hex64(event_id)?;
    let channel_id = parse_uuid(channel)?.to_string();
    let event = fetch_event(client, &event_id.to_ascii_lowercase()).await?;
    let in_channel = event
        .get("tags")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .any(|t| {
            tag_name(t) == Some("h") && t.get(1).and_then(|v| v.as_str()) == Some(&channel_id)
        });
    if !in_channel {
        return Err(CliError::Usage(format!(
            "event {event_id} is not in channel {channel_id}"
        )));
    }
    let thread = thread_ref_from_event(&event_id.to_ascii_lowercase(), &event)?;
    if thread.root_event_id != thread.parent_event_id {
        return Err(CliError::Usage(
            "a task thread starts from a top-level message; nested task threads are not supported"
                .into(),
        ));
    }
    Ok(event)
}

fn event_author(event: &serde_json::Value) -> Result<String, CliError> {
    event
        .get("pubkey")
        .and_then(|p| p.as_str())
        .map(str::to_string)
        .ok_or_else(|| CliError::Other("event has no author".into()))
}

fn tag<const N: usize>(parts: [&str; N]) -> Result<nostr::Tag, CliError> {
    nostr::Tag::parse(parts).map_err(|e| CliError::Other(format!("invalid tag: {e}")))
}

fn tag_name(tag: &serde_json::Value) -> Option<&str> {
    tag.get(0).and_then(|v| v.as_str())
}

fn validate_title(title: &str) -> Result<&str, CliError> {
    let title = title.trim();
    if title.is_empty()
        || title.chars().count() > MAX_TITLE_CHARS
        || title.chars().any(char::is_control)
    {
        return Err(CliError::Usage(format!(
            "--title must be one non-empty line of at most {MAX_TITLE_CHARS} characters"
        )));
    }
    Ok(title)
}

/// Fit a title into the shared thread-name budget (ASCII one unit, other
/// characters two, 40 units), ending clipped names with an ellipsis.
fn thread_name_for(title: &str) -> String {
    let weight = |c: char| if c.is_ascii() { 1 } else { 2 };
    let max = buzz_core::thread_name::MAX_THREAD_NAME_WEIGHT;
    if title.chars().map(weight).sum::<usize>() <= max {
        return title.to_string();
    }
    let mut used = weight('…');
    let mut name = String::new();
    for c in title.chars() {
        if used + weight(c) > max {
            break;
        }
        used += weight(c);
        name.push(c);
    }
    format!("{}…", name.trim_end())
}

/// Single-line excerpt of markdown content for the sent-from-thread tag, or
/// `None` when empty. Markdown syntax characters are dropped.
fn excerpt_for(text: &str) -> Option<String> {
    clip_excerpt(&text.replace(['*', '`', '#', '>', '_', '~', '|'], " "))
}

/// Single-line excerpt of a plain-text task title for the sent-from-thread
/// tag. Clients render the excerpt as plain text, so every character the
/// title carries (`~`, `*`, `_`, `` ` ``, `[`) is kept as written.
fn title_excerpt(title: &str) -> Option<String> {
    clip_excerpt(title)
}

/// Excerpt shown as the result's "Sent from thread" label: the task title
/// when the thread has a kickoff, otherwise the root message's content.
fn result_excerpt(task: Option<&TaskInfo>, root: &serde_json::Value) -> Option<String> {
    match task {
        // A task title is plain text, so it is shown verbatim; only the
        // root's markdown content needs its syntax stripped.
        Some(task) => title_excerpt(&task.title),
        None => excerpt_for(root.get("content").and_then(|c| c.as_str()).unwrap_or("")),
    }
}

/// Collapse control characters and whitespace to single spaces and clip to
/// the excerpt limit, or `None` when nothing remains.
fn clip_excerpt(text: &str) -> Option<String> {
    let line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if line.is_empty() {
        return None;
    }
    if line.chars().count() <= MAX_EXCERPT_CHARS {
        return Some(line);
    }
    let clipped: String = line.chars().take(MAX_EXCERPT_CHARS - 1).collect();
    Some(format!("{}…", clipped.trim_end()))
}

#[derive(Debug, PartialEq)]
struct TaskInfo {
    title: String,
    requester: Option<String>,
}

/// The earliest kickoff (`buzz:task`) among the thread's replies.
fn find_task(replies: &[serde_json::Value]) -> Option<TaskInfo> {
    replies
        .iter()
        .filter_map(|event| {
            let tags = event.get("tags")?.as_array()?;
            let title = tags
                .iter()
                .find(|t| tag_name(t) == Some(TASK_TAG))?
                .get(1)?
                .as_str()?
                .to_string();
            let requester = tags
                .iter()
                .find(|t| tag_name(t) == Some("p"))
                .and_then(|t| t.get(1))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let created_at = event
                .get("created_at")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            Some((created_at, TaskInfo { title, requester }))
        })
        .min_by_key(|(created_at, _)| *created_at)
        .map(|(_, task)| task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_marks_or_warns_about_the_linked_goal() {
        use buzz_core::goal_tree::GoalOp;
        let ed = "a".repeat(64);
        let thread = "b".repeat(64);
        let mut tree = GoalTree::empty();
        for op in [
            GoalOp::SetRoot {
                id: "r".into(),
                title: "Root".into(),
                note: None,
            },
            GoalOp::Add {
                id: "g".into(),
                parent: "r".into(),
                title: "Goal".into(),
                note: None,
                assignees: vec![],
            },
            GoalOp::Link {
                id: "g".into(),
                thread: thread.clone(),
            },
        ] {
            tree.apply(&op, &ed, 1).unwrap();
        }
        assert_eq!(
            CloseGoal::plan(&tree, &thread, true),
            CloseGoal::MarkedDone {
                id: "g".into(),
                report: serde_json::Value::Null
            }
        );
        let open = CloseGoal::plan(&tree, &thread, false);
        assert!(open.warning().is_none(), "only an in_progress goal warns");
        let left = CloseGoal::Left {
            id: "g".into(),
            status: GoalStatus::InProgress,
        };
        assert!(left
            .warning()
            .unwrap()
            .contains("linked goal g is still in_progress"));
        assert_eq!(left.json()["id"], "g");
        let unlinked = CloseGoal::plan(&tree, &"c".repeat(64), false);
        assert_eq!(unlinked, CloseGoal::Unlinked);
        assert!(unlinked.warning().is_none() && unlinked.json().is_null());
        let done = CloseGoal::Left {
            id: "g".into(),
            status: GoalStatus::Done,
        };
        assert!(done.warning().is_none());
    }

    #[test]
    fn thread_name_fits_the_shared_budget() {
        assert_eq!(thread_name_for("Quote rendering"), "Quote rendering");
        assert_eq!(thread_name_for(&"가".repeat(20)), "가".repeat(20));
        let clipped = thread_name_for(&"가".repeat(30));
        assert_eq!(clipped, format!("{}…", "가".repeat(19)));
        assert!(buzz_core::thread_name::validate_thread_name(&clipped).is_ok());
        let clipped = thread_name_for(&"word ".repeat(20));
        assert!(buzz_core::thread_name::validate_thread_name(&clipped).is_ok());
    }

    #[test]
    fn title_must_be_one_short_line() {
        assert_eq!(validate_title("  작업 스레드  ").unwrap(), "작업 스레드");
        assert!(validate_title("").is_err());
        assert!(validate_title("two\nlines").is_err());
        assert!(validate_title(&"a".repeat(MAX_TITLE_CHARS + 1)).is_err());
    }

    #[test]
    fn excerpt_is_single_line_and_bounded() {
        assert_eq!(excerpt_for("**Fix**\n`quote`"), Some("Fix quote".into()));
        assert_eq!(excerpt_for("  \n "), None);
        let long = excerpt_for(&"a ".repeat(100)).unwrap();
        assert!(long.chars().count() <= MAX_EXCERPT_CHARS);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn title_excerpt_keeps_markdown_characters_literally() {
        assert_eq!(
            title_excerpt("1~100 소수 개수"),
            Some("1~100 소수 개수".into())
        );
        assert_eq!(
            title_excerpt("fix *a* _b_ `c` [d](e) ~~f~~ #g > h | i"),
            Some("fix *a* _b_ `c` [d](e) ~~f~~ #g > h | i".into())
        );
        assert_eq!(title_excerpt(" a\tb\n "), Some("a b".into()));
        assert_eq!(title_excerpt("  "), None);
        let long = title_excerpt(&"~".repeat(100)).unwrap();
        assert_eq!(long, format!("{}…", "~".repeat(MAX_EXCERPT_CHARS - 1)));
    }

    #[test]
    fn result_excerpt_shows_the_task_title_literally() {
        let root = serde_json::json!({"content": "**root** ~~body~~"});
        let task = TaskInfo {
            title: "1~100 소수 개수 *a* _b_ `c` [d]".into(),
            requester: None,
        };
        assert_eq!(
            result_excerpt(Some(&task), &root),
            Some("1~100 소수 개수 *a* _b_ `c` [d]".into())
        );
        assert_eq!(result_excerpt(None, &root), Some("root body".into()));
    }

    #[test]
    fn find_task_uses_the_earliest_kickoff() {
        let replies = vec![
            serde_json::json!({"created_at": 5, "tags": [["e", "x"]]}),
            serde_json::json!({"created_at": 9, "tags": [[TASK_TAG, "later"], ["p", "bb"]]}),
            serde_json::json!({"created_at": 3, "tags": [[TASK_TAG, "first"], ["p", "aa"]]}),
        ];
        assert_eq!(
            find_task(&replies),
            Some(TaskInfo {
                title: "first".into(),
                requester: Some("aa".into())
            })
        );
        assert_eq!(find_task(&replies[..1]), None);
    }
}
