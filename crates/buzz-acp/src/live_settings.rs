//! Agent settings that are only text, applied without restarting the agent:
//! the agent's instructions (`<agent-instructions>`), its team's instructions
//! (`<team-instructions>`), the self-opened task thread rules, and the
//! new-session history budget.
//!
//! Buzz Desktop keeps the current values in a per-agent JSON file
//! (`BUZZ_ACP_LIVE_SETTINGS_FILE`, see [`LiveSettingsFile`]) that it rewrites
//! whenever one of them changes, and the harness re-reads it every turn. A
//! session's system prompt is sent once, at `session/new`, so it keeps the
//! values current when the session started: sessions started later simply
//! get the new values, and a live session whose values changed gets a
//! one-time `<settings-update>` section on its next turn (see
//! [`render_settings_update`]). The history budget only shapes a new
//! session's first context fetch, so it needs no notice.
//!
//! Without the file the values given at launch (environment / CLI flags)
//! stay fixed for the life of the process.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::context_history::ContextHistory;
use crate::task_threads::TaskThreadTrigger;

/// On-disk format of `BUZZ_ACP_LIVE_SETTINGS_FILE`, written by Buzz Desktop.
/// Values use the same spelling as their launch variables
/// (`BUZZ_ACP_SYSTEM_PROMPT`, `BUZZ_ACP_TEAM_INSTRUCTIONS`,
/// `BUZZ_ACP_TASK_THREADS`, `BUZZ_ACP_CONTEXT_HISTORY`). A missing or blank
/// field means "not set".
#[derive(Debug, Default, Deserialize)]
pub(crate) struct LiveSettingsFile {
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub team_instructions: Option<String>,
    #[serde(default)]
    pub task_threads: Option<String>,
    #[serde(default)]
    pub context_history: Option<String>,
}

/// The values of the live settings at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveValues {
    pub system_prompt: Option<String>,
    pub team_instructions: Option<String>,
    pub task_threads: Vec<TaskThreadTrigger>,
    pub context_history: ContextHistory,
}

/// Rules the launch fixed for the life of the process; the live file never
/// overrides them.
#[derive(Clone, Copy, Debug, Default)]
pub struct LivePins {
    /// The system prompt came from `--system-prompt-file`, which Desktop does
    /// not manage; keep it.
    pub system_prompt: bool,
    /// Dispatchers always keep the recent-N history window.
    pub recent_history_only: bool,
}

#[derive(Debug)]
struct FileSource {
    path: PathBuf,
    pins: LivePins,
    /// The last successfully read values, primed with the launch values so
    /// a missing or unreadable file never looks like "settings cleared".
    last_good: Mutex<LiveValues>,
}

/// Where the current live settings come from. Cheap to clone. The default
/// has no file: callers use the values fixed at launch.
#[derive(Debug, Clone, Default)]
pub struct LiveSettings {
    file: Option<Arc<FileSource>>,
}

impl LiveSettings {
    /// Values re-read from `path` on every call; `launch` holds the values
    /// given at launch, used until the file is first read successfully.
    pub fn file(path: PathBuf, launch: LiveValues, pins: LivePins) -> Self {
        let this = Self {
            file: Some(Arc::new(FileSource {
                path,
                pins,
                last_good: Mutex::new(launch),
            })),
        };
        let _ = this.current();
        this
    }

    /// The current values, or `None` when there is no live file and the
    /// launch values apply.
    pub fn current(&self) -> Option<LiveValues> {
        let source = self.file.as_deref()?;
        let read = std::fs::read_to_string(&source.path)
            .map_err(|e| e.to_string())
            .and_then(|raw| {
                serde_json::from_str::<LiveSettingsFile>(&raw).map_err(|e| e.to_string())
            });
        let mut last_good = source.last_good.lock().unwrap_or_else(|e| e.into_inner());
        match read {
            Ok(file) => {
                let mut values = LiveValues {
                    system_prompt: non_blank(file.system_prompt),
                    team_instructions: non_blank(file.team_instructions)
                        .map(|value| value.trim().to_string()),
                    task_threads: crate::task_threads::parse_task_thread_triggers(
                        file.task_threads.as_deref(),
                    ),
                    context_history: crate::context_history::parse_context_history(
                        file.context_history.as_deref(),
                    ),
                };
                if source.pins.system_prompt {
                    values.system_prompt = last_good.system_prompt.clone();
                }
                if source.pins.recent_history_only {
                    values.context_history = ContextHistory::Recent;
                }
                *last_good = values;
            }
            Err(e) => {
                tracing::debug!(
                    target: "acp::live_settings",
                    path = %source.path.display(),
                    "live settings unreadable; keeping the last values: {e}"
                );
            }
        }
        Some(last_good.clone())
    }
}

fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// The parts of a session's standing context that follow live settings.
/// A live session remembers the version it last received, and a turn whose
/// current version differs carries a `<settings-update>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveText {
    /// `<agent-instructions>` body.
    pub system_prompt: Option<String>,
    /// `<team-instructions>` body.
    pub team_instructions: Option<String>,
    /// The "Opening Task Threads Yourself" rules appended to `<base>`, present
    /// only where they apply (see `runtime::assemble_base_prompt`).
    pub task_thread_rules: Option<String>,
}

impl LiveText {
    /// The live text for `values`. `task_thread_rules_apply` is whether this
    /// session kind gets the task thread rules at all.
    pub fn new(values: &LiveValues, task_thread_rules_apply: bool) -> Self {
        Self {
            system_prompt: values.system_prompt.clone(),
            team_instructions: values.team_instructions.clone(),
            task_thread_rules: task_thread_rules_apply
                .then(|| crate::task_threads::task_thread_guidance(&values.task_threads))
                .flatten(),
        }
    }

    fn is_empty(&self) -> bool {
        self.system_prompt.is_none()
            && self.team_instructions.is_none()
            && self.task_thread_rules.is_none()
    }
}

/// The body of a `<settings-update>` section when a live session last saw
/// `seen` and the settings are now `current`, or `None` when nothing changed.
/// Only the parts that changed are restated.
///
/// `seen` is `None` when unknown (a provider session reattached after a
/// restart keeps its original system prompt); every current part is then
/// restated once, if there is any.
pub(crate) fn render_settings_update(
    seen: Option<&LiveText>,
    current: &LiveText,
) -> Option<String> {
    match seen {
        Some(seen) if seen == current => return None,
        None if current.is_empty() => return None,
        _ => {}
    }
    let changed = |part: fn(&LiveText) -> &Option<String>| {
        seen.is_none_or(|seen| part(seen) != part(current))
    };
    let mut parts = Vec::new();
    if changed(|text| &text.system_prompt) {
        match &current.system_prompt {
            Some(prompt) => parts.push(crate::prompt_framing::semantic_section(
                "agent-instructions",
                prompt,
            )),
            None if seen.is_some() => {
                parts.push("Your `<agent-instructions>` were removed.".to_string())
            }
            None => {}
        }
    }
    if changed(|text| &text.team_instructions) {
        match &current.team_instructions {
            Some(instructions) => parts.push(crate::prompt_framing::semantic_section(
                "team-instructions",
                instructions,
            )),
            None if seen.is_some() => {
                parts.push("Your `<team-instructions>` were removed.".to_string())
            }
            None => {}
        }
    }
    if changed(|text| &text.task_thread_rules) {
        match &current.task_thread_rules {
            Some(rules) => parts.push(format!(
                "These rules replace any earlier \"Opening Task Threads Yourself\" rules in `<base>`:\n\n{rules}"
            )),
            None if seen.is_some() => parts.push(
                "The \"Opening Task Threads Yourself\" rules in `<base>` no longer apply: \
                 open a task thread only when a human asks for one."
                    .to_string(),
            ),
            None => {}
        }
    }
    Some(format!(
        "Your owner updated your settings. Follow these from now on; each part replaces the \
         earlier version in your instructions.\n\n{}",
        parts.join("\n\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_history::ContextBudget;

    fn write(path: &std::path::Path, value: serde_json::Value) {
        std::fs::write(path, value.to_string()).unwrap();
    }

    fn launch() -> LiveValues {
        LiveValues {
            system_prompt: Some("Launch prompt".into()),
            team_instructions: Some("Launch team".into()),
            task_threads: vec![TaskThreadTrigger::LongRunning],
            context_history: ContextHistory::Recent,
        }
    }

    #[test]
    fn no_file_means_launch_values() {
        assert_eq!(LiveSettings::default().current(), None);
    }

    #[test]
    fn file_source_follows_edits_and_keeps_last_good_on_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.json");
        let settings = LiveSettings::file(path.clone(), launch(), LivePins::default());
        assert_eq!(
            settings.current(),
            Some(launch()),
            "missing file keeps the launch values"
        );

        write(
            &path,
            serde_json::json!({
                "system_prompt": "You are Eva.",
                "team_instructions": "  Ship small.  ",
                "task_threads": "multi_step,long_running",
                "context_history": "medium",
            }),
        );
        let first = settings.current().unwrap();
        assert_eq!(first.system_prompt.as_deref(), Some("You are Eva."));
        assert_eq!(first.team_instructions.as_deref(), Some("Ship small."));
        assert_eq!(
            first.task_threads,
            vec![TaskThreadTrigger::LongRunning, TaskThreadTrigger::MultiStep]
        );
        assert_eq!(
            first.context_history,
            ContextHistory::Budget(ContextBudget::Medium)
        );

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(settings.current(), Some(first.clone()));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            settings.current(),
            Some(first),
            "a vanished file is a read error"
        );

        write(&path, serde_json::json!({ "team_instructions": " " }));
        assert_eq!(
            settings.current(),
            Some(LiveValues::default()),
            "absent and blank fields clear their setting"
        );
    }

    #[test]
    fn pinned_values_ignore_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.json");
        write(
            &path,
            serde_json::json!({ "system_prompt": "From desktop", "context_history": "large" }),
        );
        let settings = LiveSettings::file(
            path,
            launch(),
            LivePins {
                system_prompt: true,
                recent_history_only: true,
            },
        );
        let current = settings.current().unwrap();
        assert_eq!(current.system_prompt.as_deref(), Some("Launch prompt"));
        assert_eq!(current.context_history, ContextHistory::Recent);
    }

    #[test]
    fn task_thread_rules_follow_applicability() {
        let values = launch();
        assert!(LiveText::new(&values, true)
            .task_thread_rules
            .unwrap()
            .contains("**Long-running work:**"));
        assert_eq!(LiveText::new(&values, false).task_thread_rules, None);
        let off = LiveValues {
            task_threads: Vec::new(),
            ..launch()
        };
        assert_eq!(LiveText::new(&off, true).task_thread_rules, None);
    }

    #[test]
    fn settings_update_only_when_changed_and_only_the_changed_parts() {
        let before = LiveText::new(&launch(), true);
        assert_eq!(render_settings_update(Some(&before), &before), None);
        assert_eq!(
            render_settings_update(None, &LiveText::default()),
            None,
            "nothing to restate"
        );

        let after = LiveText {
            system_prompt: Some("New prompt".into()),
            ..before.clone()
        };
        let update = render_settings_update(Some(&before), &after).unwrap();
        assert!(update.contains("<agent-instructions>\nNew prompt\n</agent-instructions>"));
        assert!(!update.contains("<team-instructions>"), "{update}");
        assert!(
            !update.contains("Opening Task Threads Yourself"),
            "{update}"
        );

        let cleared = LiveText {
            team_instructions: None,
            task_thread_rules: None,
            ..before.clone()
        };
        let update = render_settings_update(Some(&before), &cleared).unwrap();
        assert!(update.contains("`<team-instructions>` were removed"));
        assert!(update.contains("open a task thread only when a human asks"));
        assert!(!update.contains("<agent-instructions>"));

        let restated = render_settings_update(None, &before).unwrap();
        assert!(restated.contains("Launch prompt"));
        assert!(restated.contains("Launch team"));
        assert!(restated.contains("### Opening Task Threads Yourself"));
        assert!(!restated.contains("removed"));
    }
}
