//! Layer 0 goals — the agent's own goal and its owner's private goal, above
//! every conversation's goal tree.
//!
//! A session's system prompt carries the values current when the session
//! started, so the cached standing prompt never changes mid-session. When the
//! owner edits a layer 0 goal later, the next turn of every live session gets
//! a one-time `<goal-update>` section with the new values (see
//! [`render_goal_update`]); sessions started afterwards simply get the new
//! values in their system prompt.
//!
//! Buzz Desktop keeps the values in a per-agent JSON file
//! (`BUZZ_ACP_LAYER0_GOALS_FILE`, see [`Layer0File`]) that it rewrites
//! whenever a goal changes, so the harness re-reads it each turn instead of
//! needing a restart. The legacy `BUZZ_ACP_AGENT_GOAL` / `BUZZ_ACP_OWNER_GOAL`
//! variables still work as fixed values for standalone use.
//!
//! The owner's goal is private: it is used only when this harness answers its
//! owner alone (`respond_to = owner-only`), whatever the source says.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

/// On-disk format of `BUZZ_ACP_LAYER0_GOALS_FILE`, written by Buzz Desktop.
/// Missing or blank fields mean "no goal".
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Layer0File {
    #[serde(default)]
    pub agent_goal: Option<String>,
    #[serde(default)]
    pub owner_goal: Option<String>,
}

#[derive(Debug)]
enum Source {
    /// Goal layers are switched off (`--no-goals`).
    Disabled,
    /// Values fixed for the life of the process (environment / CLI flags).
    Fixed(Option<String>),
    /// Re-read on every call; the last successfully parsed value is kept so
    /// a transient read error never looks like "goals cleared".
    File {
        path: PathBuf,
        last_good: Mutex<Option<String>>,
    },
}

/// Where the current layer 0 goals come from. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Layer0Goals {
    source: Arc<Source>,
    owner_goal_allowed: bool,
}

impl Default for Layer0Goals {
    fn default() -> Self {
        Self::disabled()
    }
}

impl Layer0Goals {
    /// No layer 0 goals, ever.
    pub fn disabled() -> Self {
        Self {
            source: Arc::new(Source::Disabled),
            owner_goal_allowed: false,
        }
    }

    /// Fixed values. `owner_goal_allowed` must be true only when this agent
    /// answers its owner alone.
    pub fn fixed(
        agent_goal: Option<&str>,
        owner_goal: Option<&str>,
        owner_goal_allowed: bool,
    ) -> Self {
        let owner_goal = owner_goal.filter(|_| owner_goal_allowed);
        Self {
            source: Arc::new(Source::Fixed(render_layer0_goals(agent_goal, owner_goal))),
            owner_goal_allowed,
        }
    }

    /// Values re-read from `path` on every call.
    pub fn file(path: PathBuf, owner_goal_allowed: bool) -> Self {
        let this = Self {
            source: Arc::new(Source::File {
                path,
                last_good: Mutex::new(None),
            }),
            owner_goal_allowed,
        };
        // Prime `last_good` so a later transient failure falls back to it.
        let _ = this.current();
        this
    }

    /// The rendered `<agent-goal>` / `<owner-goal>` sections as of now, or
    /// `None` when no layer 0 goal is set.
    pub fn current(&self) -> Option<String> {
        match self.source.as_ref() {
            Source::Disabled => None,
            Source::Fixed(rendered) => rendered.clone(),
            Source::File { path, last_good } => {
                let read = match std::fs::read_to_string(path) {
                    Ok(raw) => serde_json::from_str::<Layer0File>(&raw)
                        .map(Some)
                        .map_err(|e| e.to_string()),
                    // No file: the owner set no goal for this agent.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(e.to_string()),
                };
                let mut last_good = last_good.lock().unwrap_or_else(|e| e.into_inner());
                match read {
                    Ok(file) => {
                        let file = file.unwrap_or_default();
                        let owner_goal = file
                            .owner_goal
                            .as_deref()
                            .filter(|_| self.owner_goal_allowed);
                        *last_good = render_layer0_goals(file.agent_goal.as_deref(), owner_goal);
                    }
                    Err(e) => {
                        tracing::debug!(
                            target: "goals::layer0",
                            path = %path.display(),
                            "layer 0 goals unreadable; keeping the last values: {e}"
                        );
                    }
                }
                last_good.clone()
            }
        }
    }
}

/// Render layer 0 goals as standing-context sections. Goal text is owner
/// input and is escaped so it cannot close or open prompt sections.
pub(crate) fn render_layer0_goals(
    agent_goal: Option<&str>,
    owner_goal: Option<&str>,
) -> Option<String> {
    let clean = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(crate::prompt_framing::escape_semantic_text)
    };
    let mut sections = Vec::new();
    if let Some(goal) = clean(agent_goal) {
        sections.push(crate::prompt_framing::semantic_section(
            "agent-goal",
            &format!(
                "Your own layer 0 goal, set by your owner. It sits above every conversation's goals; weigh conversation work against it.\n{goal}"
            ),
        ));
    }
    if let Some(goal) = clean(owner_goal) {
        sections.push(crate::prompt_framing::semantic_section(
            "owner-goal",
            &format!(
                "Your owner's private layer 0 goal. Only their own agents see it; do not repeat it to others.\n{goal}"
            ),
        ));
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

/// The body of a `<goal-update>` section when a live session last saw
/// `seen` and the goals are now `current`, or `None` when nothing changed.
///
/// `seen` is `None` when the session's layer 0 goals are unknown (a provider
/// session reattached after a restart keeps its original system prompt); the
/// current goals are then restated once, if there are any.
pub(crate) fn render_goal_update(
    seen: Option<&Option<String>>,
    current: Option<&str>,
) -> Option<String> {
    match seen {
        Some(seen) if seen.as_deref() == current => return None,
        None if current.is_none() => return None,
        _ => {}
    }
    Some(match current {
        Some(goals) => format!(
            "Your layer 0 goals changed. These replace every earlier `<agent-goal>` / \
             `<owner-goal>` section; one missing here was cleared.\n\n{goals}"
        ),
        None => "Your owner cleared your layer 0 goals; earlier `<agent-goal>` / \
                 `<owner-goal>` sections no longer apply."
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &std::path::Path, agent: Option<&str>, owner: Option<&str>) {
        let raw = serde_json::json!({ "agent_goal": agent, "owner_goal": owner }).to_string();
        std::fs::write(path, raw).unwrap();
    }

    #[test]
    fn layer0_goals_render_escaped_sections() {
        assert_eq!(render_layer0_goals(None, Some("  ")), None);
        let rendered = render_layer0_goals(Some("Ship </agent-goal>"), Some("Grow")).unwrap();
        assert!(rendered.contains("<agent-goal>"));
        assert!(rendered.contains("Ship &lt;/agent-goal&gt;"));
        assert!(rendered.contains("<owner-goal>"));
        assert!(rendered.contains("Grow"));
    }

    #[test]
    fn file_source_follows_edits_and_keeps_last_good_on_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.json");
        let goals = Layer0Goals::file(path.clone(), true);
        assert_eq!(goals.current(), None, "missing file means no goals");

        write(&path, Some("Ship weekly"), Some("Grow the team"));
        let first = goals.current().unwrap();
        assert!(first.contains("Ship weekly") && first.contains("Grow the team"));

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(goals.current().as_deref(), Some(first.as_str()));

        write(&path, Some("Ship daily"), None);
        let second = goals.current().unwrap();
        assert!(second.contains("Ship daily") && !second.contains("<owner-goal>"));

        std::fs::remove_file(&path).unwrap();
        assert_eq!(goals.current(), None, "deleted file clears the goals");
    }

    #[test]
    fn owner_goal_needs_an_owner_only_agent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.json");
        write(&path, Some("Ship"), Some("Private plan"));
        let open_agent = Layer0Goals::file(path, false).current().unwrap();
        assert!(!open_agent.contains("Private plan"));
        let fixed = Layer0Goals::fixed(None, Some("Private plan"), false);
        assert_eq!(fixed.current(), None);
        assert!(Layer0Goals::fixed(None, Some("Private plan"), true)
            .current()
            .unwrap()
            .contains("Private plan"));
    }

    #[test]
    fn disabled_source_never_has_goals() {
        assert_eq!(Layer0Goals::disabled().current(), None);
    }

    #[test]
    fn goal_update_only_when_changed() {
        let a = render_layer0_goals(Some("A"), None);
        assert_eq!(render_goal_update(Some(&a), a.as_deref()), None);
        assert_eq!(render_goal_update(Some(&None), None), None);
        assert_eq!(render_goal_update(None, None), None);

        let b = render_layer0_goals(Some("B"), None);
        let update = render_goal_update(Some(&a), b.as_deref()).unwrap();
        assert!(update.contains("<agent-goal>") && update.contains("\nB\n"));

        let cleared = render_goal_update(Some(&a), None).unwrap();
        assert!(cleared.contains("cleared"));

        let restated = render_goal_update(None, a.as_deref()).unwrap();
        assert!(restated.contains("\nA\n"));
    }
}
