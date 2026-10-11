//! Give each agent only the base-prompt sections it needs.
//!
//! `base_prompt.md` keeps upstream's section layout so merges stay clean;
//! sections are dropped or replaced here, by their `## ` heading, when the
//! prompt is composed. A heading that is not found (a custom base file, or
//! drift after an upstream edit) leaves the prompt untouched; the
//! `default_base_prompt_has_every_trimmed_heading` test catches drift in the
//! compiled-in file.

const PROJECTS: &str = "## Projects";
const AGENT_CREATION: &str = "## Conversational Agent Creation";
const ENGINEERING: &str = "## Engineering Discipline";
const WORKING_IN_REPO: &str = "## Working in the Repo";

const PROJECTS_POINTER: &str = "## Projects

Full project guidance appears only in a project's home channel (see `buzz projects --help`); never run `buzz projects create` for work that already has a project, and `mkdir` in `REPOS/` is not a Buzz repository. When announcing a PR, issue, repo, or project, include the `link` value the CLI returned verbatim; never invent HTTPS URLs for Buzz-hosted repos (`link` and the `clone` URL are the only shareable references). Assign issues only with `buzz issues assign`: names in the issue body or `issues create --to` only notify.";

const AGENT_CREATION_POINTER: &str = "## Conversational Agent Creation

To create an agent from chat, ask at most for its name and day-to-day job, write `--system-prompt` yourself, and don't ask about runtime, model, or credentials; see `buzz agents draft-create --help` (`draft-update` for changes). Never claim the agent exists until the owner saves it.";

/// Per-agent trimming, applied once when the harness assembles its base.
///
/// Agent creation always shrinks to a pointer. Without `code_work` the
/// engineering and repository guidance is removed.
pub(crate) fn for_agent(base: &str, code_work: bool) -> String {
    let base = replace_section(
        base.to_string(),
        AGENT_CREATION,
        Some(AGENT_CREATION_POINTER),
    );
    if code_work {
        return base;
    }
    let base = replace_section(base, ENGINEERING, None);
    replace_section(base, WORKING_IN_REPO, None)
}

/// Per-session trimming: the full project guidance only in a project's home
/// channel, a pointer everywhere else.
pub(crate) fn for_channel(base: String, project_home: bool) -> String {
    if project_home {
        return base;
    }
    replace_section(base, PROJECTS, Some(PROJECTS_POINTER))
}

/// Byte range of the section headed exactly `heading`: its heading line up to
/// the next `## ` heading, or the end of the prompt.
fn section_range(prompt: &str, heading: &str) -> Option<std::ops::Range<usize>> {
    let mut offset = 0;
    let mut start = None;
    for line in prompt.split_inclusive('\n') {
        let title = line.trim_end();
        match start {
            None if title == heading => start = Some(offset),
            Some(start) if title.starts_with("## ") => return Some(start..offset),
            _ => {}
        }
        offset += line.len();
    }
    start.map(|start| start..prompt.len())
}

/// Replace the section headed `heading` with `replacement` (a whole section,
/// heading included), or drop it when `None`. Untouched when not found.
fn replace_section(prompt: String, heading: &str, replacement: Option<&str>) -> String {
    let Some(range) = section_range(&prompt, heading) else {
        tracing::debug!(heading, "base prompt section not found; left as is");
        return prompt;
    };
    let mut out = String::with_capacity(prompt.len());
    out.push_str(&prompt[..range.start]);
    if let Some(replacement) = replacement {
        out.push_str(replacement);
        out.push_str(if range.end < prompt.len() {
            "\n\n"
        } else {
            "\n"
        });
    }
    out.push_str(&prompt[range.end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = include_str!("base_prompt.md");

    #[test]
    fn default_base_prompt_has_every_trimmed_heading() {
        // Fails when an upstream edit renames or removes a heading the
        // trimming depends on: rename the constant above to match.
        for heading in [PROJECTS, AGENT_CREATION, ENGINEERING, WORKING_IN_REPO] {
            let lines = BASE.lines().filter(|line| *line == heading).count();
            assert_eq!(lines, 1, "{heading} must appear once in base_prompt.md");
        }
    }

    #[test]
    fn code_work_keeps_engineering_and_repo_guidance() {
        let prompt = for_agent(BASE, true);
        assert!(prompt.contains(ENGINEERING));
        assert!(prompt.contains("**Understand before changing.**"));
        assert!(prompt.contains(WORKING_IN_REPO));
        assert!(prompt.contains("read its root `AGENTS.md`"));
    }

    #[test]
    fn without_code_work_engineering_and_repo_guidance_are_removed() {
        let prompt = for_agent(BASE, false);
        assert!(!prompt.contains(ENGINEERING));
        assert!(!prompt.contains("**Understand before changing.**"));
        assert!(!prompt.contains(WORKING_IN_REPO));
        assert!(!prompt.contains("read its root `AGENTS.md`"));
        // Neighbouring sections survive intact.
        assert!(prompt.contains("## Agent Memory"));
        assert!(prompt.contains("Cite sources with paths"));
        assert!(prompt.contains("## Autonomy"));
        assert!(prompt.contains("Surface to the user only for product intent"));
    }

    #[test]
    fn agent_creation_is_always_a_pointer() {
        for code_work in [true, false] {
            let prompt = for_agent(BASE, code_work);
            assert!(prompt.contains(AGENT_CREATION_POINTER));
            assert!(!prompt.contains("ask for at most two things"));
            assert!(prompt.contains("Never claim the agent exists until the owner saves it"));
            assert!(prompt.contains("don't ask about runtime, model, or credentials"));
            assert!(prompt.contains("## Communication Patterns"));
        }
    }

    #[test]
    fn projects_in_full_only_in_a_project_home() {
        let home = for_channel(BASE.to_string(), true);
        assert_eq!(home, BASE);

        let elsewhere = for_channel(BASE.to_string(), false);
        assert!(elsewhere.contains(PROJECTS_POINTER));
        assert!(!elsewhere.contains("buzz repos create --id"));
        // General rules that `--help` does not teach stay outside project homes.
        for rule in [
            "never run `buzz projects create`",
            "`mkdir` in `REPOS/` is not a Buzz repository",
            "include the `link` value the CLI returned verbatim",
            "never invent HTTPS URLs for Buzz-hosted repos",
            "Assign issues only with `buzz issues assign`",
            "`issues create --to` only notify",
        ] {
            assert!(elsewhere.contains(rule), "{rule}");
        }
        assert!(elsewhere.contains("## Conversational Agent Creation"));
    }

    #[test]
    fn missing_heading_leaves_prompt_untouched() {
        let custom = "You are a router.\n\n## Routing\n\nRoute things.\n";
        assert_eq!(for_agent(custom, false), custom);
        assert_eq!(for_channel(custom.to_string(), false), custom);
    }

    #[test]
    fn only_exact_level_two_headings_bound_a_section() {
        let prompt = "intro\n\n## Projects\n\nbody\n\n### Sub\n\nsub body\n\n## Next\n\nkept\n";
        assert_eq!(
            replace_section(prompt.to_string(), PROJECTS, None),
            "intro\n\n## Next\n\nkept\n"
        );
        // A heading that merely starts with the name is not a match.
        let prefixed = "## Projects and more\n\nbody\n";
        assert_eq!(
            replace_section(prefixed.to_string(), PROJECTS, None),
            prefixed
        );
    }

    #[test]
    fn last_section_can_be_replaced_or_dropped() {
        let prompt = "intro\n\n## Projects\n\nbody\n";
        assert_eq!(
            replace_section(prompt.to_string(), PROJECTS, None),
            "intro\n\n"
        );
        assert_eq!(
            replace_section(prompt.to_string(), PROJECTS, Some("## Projects\n\nshort")),
            "intro\n\n## Projects\n\nshort\n"
        );
    }
}
