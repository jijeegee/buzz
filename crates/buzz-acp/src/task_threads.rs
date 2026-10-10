//! Owner-chosen situations in which an agent may open a task thread on its own.
//!
//! The enabled situations come as a comma-separated list (e.g.
//! `long_running,multi_step`) in `BUZZ_ACP_TASK_THREADS` or, from Buzz
//! Desktop, in the live settings file (see `live_settings`), so a change
//! reaches running agents without a restart. An empty list keeps the default
//! rule from the base prompt: open a task thread only when a human asks for
//! one. The guidance is appended to the standing base prompt under the thread
//! session policy only, because that is where task threads get their own
//! session and their results flow back to the main session.

use std::str::FromStr;

/// One situation in which the agent may open a task thread unasked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskThreadTrigger {
    LongRunning,
    MultiStep,
    Parallel,
    SideDiscussion,
    Delegation,
}

impl TaskThreadTrigger {
    const ALL: [Self; 5] = [
        Self::LongRunning,
        Self::MultiStep,
        Self::Parallel,
        Self::SideDiscussion,
        Self::Delegation,
    ];

    fn guidance(self) -> &'static str {
        match self {
            Self::LongRunning => {
                "**Long-running work:** builds, deploys, test suites, large migrations, or broad research that will take more than a few minutes."
            }
            Self::MultiStep => {
                "**Multi-step work:** work that moves through several stages (for example investigate → change → verify) and would need more than one progress update."
            }
            Self::Parallel => {
                "**Several independent tasks in one request:** open one thread per task so each result reports back on its own."
            }
            Self::SideDiscussion => {
                "**Side discussion:** a long back-and-forth on a tangent that would crowd out the main topic of the timeline."
            }
            Self::Delegation => {
                "**Delegating to another agent:** hand the work over inside a task thread so the assignment, progress, and result stay together."
            }
        }
    }
}

impl FromStr for TaskThreadTrigger {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "long_running" => Ok(Self::LongRunning),
            "multi_step" => Ok(Self::MultiStep),
            "parallel" => Ok(Self::Parallel),
            "side_discussion" => Ok(Self::SideDiscussion),
            "delegation" => Ok(Self::Delegation),
            other => Err(format!("unknown task thread trigger: {other}")),
        }
    }
}

/// Parse the env list. Unknown entries are skipped with a warning so a newer
/// desktop never stops an older harness from starting.
pub fn parse_task_thread_triggers(raw: Option<&str>) -> Vec<TaskThreadTrigger> {
    let mut triggers: Vec<TaskThreadTrigger> = raw
        .unwrap_or_default()
        .split(',')
        .filter(|entry| !entry.trim().is_empty())
        .filter_map(|entry| match entry.parse() {
            Ok(trigger) => Some(trigger),
            Err(error) => {
                tracing::warn!("ignoring BUZZ_ACP_TASK_THREADS entry: {error}");
                None
            }
        })
        .collect();
    triggers.sort();
    triggers.dedup();
    triggers
}

/// The self-opened task thread rules for `triggers`, or `None` when no
/// situation is enabled. Appended to the standing base prompt, and restated
/// in a live session's `<settings-update>` when they change.
pub fn task_thread_guidance(triggers: &[TaskThreadTrigger]) -> Option<String> {
    if triggers.is_empty() {
        return None;
    }
    let cases: String = TaskThreadTrigger::ALL
        .iter()
        .filter(|trigger| triggers.contains(trigger))
        .map(|trigger| format!("- {}\n", trigger.guidance()))
        .collect();
    Some(format!(
        "### Opening Task Threads Yourself\n\n\
This is an exception to the rule above that you open a thread only when a human asks for one. \
The owner allows you to open a task thread on your own when a top-level request on the channel main timeline that you are about to work on is one of these cases:\n\n\
{cases}\n\
Anything else stays on the main timeline: answer short questions and quick one-step tasks there directly. \
To open one, run `buzz threads start --channel <UUID> --from <the request's event id> --title \"…\" --brief -` with a brief of three lines in the requester's language: \
the goal, the deliverable, and what counts as done. \
Do the work and post progress in that thread, and finish with `buzz threads close` as described in Task Threads."
    ))
}

/// Append the self-opened task thread rules to `base`, or return it
/// unchanged when none apply.
pub fn append_task_thread_guidance(base: String, guidance: Option<&str>) -> String {
    match guidance {
        Some(guidance) => format!("{}\n\n{guidance}", base.trim_end()),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_missing_list_disables_guidance() {
        assert!(parse_task_thread_triggers(None).is_empty());
        assert!(parse_task_thread_triggers(Some("")).is_empty());
        assert!(parse_task_thread_triggers(Some(" , ")).is_empty());
        assert_eq!(task_thread_guidance(&[]), None);
        assert_eq!(append_task_thread_guidance("base".into(), None), "base");
    }

    #[test]
    fn parses_known_entries_and_skips_unknown_ones() {
        assert_eq!(
            parse_task_thread_triggers(Some("multi-step, LONG_RUNNING,bogus,multi_step")),
            vec![TaskThreadTrigger::LongRunning, TaskThreadTrigger::MultiStep]
        );
    }

    #[test]
    fn guidance_lists_only_enabled_cases() {
        let guidance = task_thread_guidance(&[
            TaskThreadTrigger::SideDiscussion,
            TaskThreadTrigger::LongRunning,
        ]);
        let prompt = append_task_thread_guidance("base\n".into(), guidance.as_deref());
        assert!(prompt.starts_with("base\n\n### Opening Task Threads Yourself"));
        assert!(prompt.contains("**Long-running work:**"));
        assert!(prompt.contains("**Side discussion:**"));
        assert!(!prompt.contains("**Multi-step work:**"));
        assert!(prompt.find("Long-running").unwrap() < prompt.find("Side discussion").unwrap());
    }
}
