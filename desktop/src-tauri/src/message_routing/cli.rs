//! Subscription routes: one routing call as a one-shot run of the official
//! CLI (`codex exec` / `claude -p`) on that CLI's own sign-in.
//!
//! Buzz never reads, copies, or logs subscription tokens here — the CLI owns
//! them. The router instructions replace each CLI's own agent prompt (read
//! from [`INSTRUCTIONS_FILE`] in the scratch directory), the user turn goes
//! in on stdin (never the command line), every tool we can switch off is
//! off, no project docs or settings load, nothing is persisted as a session,
//! and the run happens in that empty scratch directory under a hard deadline
//! that kills the whole process tree (`output_with_timeout_and_input`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::model::{CliState, RouterProvider};
use crate::managed_agents::{
    output_with_timeout_and_input, probe_auth_status, readiness::cli_probe, resolve_command,
    AuthStatus,
};

/// How long a sign-in probe result is reused. The Task models row and the
/// composer both ask for readiness; one `codex login status` / `claude auth
/// status` spawn per minute is plenty.
const SIGN_IN_TTL: Duration = Duration::from_secs(60);

/// Agent tools Codex enables by default, all switched off for a routing
/// call: it answers from the prompt alone. `-c features.<name>=false` is
/// used rather than `--disable`, which fails on a name a newer or older
/// Codex does not know.
const CODEX_DISABLED_FEATURES: &[&str] = &[
    "shell_tool",
    "unified_exec",
    "apps",
    "plugins",
    "remote_plugin",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "image_generation",
    "multi_agent",
    "hooks",
    "skill_search",
    "tool_suggest",
    "goals",
    "view_image",
    "sleep_tool",
    "personality",
];

/// The router instructions as a file in the scratch directory, named
/// relative to it: Codex reads it as `model_instructions_file` (replacing
/// its base instructions) and Claude Code as `--system-prompt-file`
/// (replacing its agent prompt). A relative name keeps every argument free
/// of quotes and spaces, safe through a Windows `.cmd` shim.
pub const INSTRUCTIONS_FILE: &str = "router-instructions.md";

/// The CLI binary and its sign-in probe for a subscription route.
fn cli_command(provider: RouterProvider) -> Option<(&'static str, &'static [&'static str])> {
    match provider {
        RouterProvider::Codex => Some(("codex", &["codex", "login", "status"])),
        RouterProvider::ClaudeCode => Some(("claude", &["claude", "auth", "status"])),
        RouterProvider::Anthropic | RouterProvider::Openai | RouterProvider::Openrouter => None,
    }
}

fn sign_in_cache() -> &'static Mutex<HashMap<RouterProvider, (Instant, CliState)>> {
    static CACHE: OnceLock<Mutex<HashMap<RouterProvider, (Instant, CliState)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Install and sign-in state of a subscription route's CLI, probed with the
/// CLI's own status command (`codex login status`, `claude auth status`) and
/// cached for [`SIGN_IN_TTL`]. Blocking: may spawn the probe.
pub fn cli_state(provider: RouterProvider) -> CliState {
    let Some((binary, probe)) = cli_command(provider) else {
        return CliState::NotInstalled;
    };
    if let Some((at, state)) = sign_in_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&provider).cloned())
    {
        if at.elapsed() < SIGN_IN_TTL {
            return state;
        }
    }
    let state = match resolve_command(binary) {
        None => CliState::NotInstalled,
        Some(program) => match probe_auth_status(&program, probe) {
            AuthStatus::LoggedIn => CliState::Ready(program),
            _ => CliState::SignedOut,
        },
    };
    if let Ok(mut cache) = sign_in_cache().lock() {
        cache.insert(provider, (Instant::now(), state.clone()));
    }
    state
}

/// Forget a cached sign-in so the next check re-probes (after a failed call:
/// the user may have signed out since).
pub fn forget_cli_state(provider: RouterProvider) {
    if let Ok(mut cache) = sign_in_cache().lock() {
        cache.remove(&provider);
    }
}

/// A model id that is safe as one CLI argument on every platform — including
/// through a Windows `.cmd` shim, which re-parses its arguments.
pub fn is_safe_cli_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 128
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'/'))
}

/// Arguments after the binary for one routing call. The prompt is not here:
/// it is written to stdin.
pub fn cli_args(provider: RouterProvider, model: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    match provider {
        RouterProvider::Codex => {
            for arg in [
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "--ignore-user-config",
                "--ignore-rules",
                "--sandbox",
                "read-only",
                "--color",
                "never",
                "--model",
                model,
                "-c",
                "model_reasoning_effort=low",
                "-c",
                "web_search=disabled",
                // Router instructions instead of Codex's ~3.5k-token base
                // prompt; no AGENTS.md, sandbox, or environment preamble.
                "-c",
                "model_instructions_file=router-instructions.md",
                "-c",
                "project_doc_max_bytes=0",
                "-c",
                "include_permissions_instructions=false",
                "-c",
                "include_environment_context=false",
            ] {
                args.push(arg.to_string());
            }
            for feature in CODEX_DISABLED_FEATURES {
                args.push("-c".to_string());
                args.push(format!("features.{feature}=false"));
            }
            // Read the prompt from stdin.
            args.push("-".to_string());
        }
        RouterProvider::ClaudeCode => {
            for arg in [
                "--print",
                "--model",
                model,
                "--tools",
                "",
                "--system-prompt-file",
                INSTRUCTIONS_FILE,
                "--no-session-persistence",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--output-format",
                "text",
            ] {
                args.push(arg.to_string());
            }
        }
        RouterProvider::Anthropic | RouterProvider::Openai | RouterProvider::Openrouter => {}
    }
    args
}

/// Write the router instructions to [`INSTRUCTIONS_FILE`] in `dir`, which
/// both CLIs load in place of their own system prompt. Staged and renamed so
/// a concurrent run never reads a half-written file; skipped when unchanged.
fn write_instructions(dir: &Path, system: &str) -> Result<(), String> {
    let path = dir.join(INSTRUCTIONS_FILE);
    if std::fs::read_to_string(&path).is_ok_and(|current| current == system) {
        return Ok(());
    }
    let staged = dir.join(format!("{INSTRUCTIONS_FILE}.{}", std::process::id()));
    std::fs::write(&staged, system)
        .and_then(|()| std::fs::rename(&staged, &path))
        .map_err(|e| format!("router instructions: {e}"))
}

/// The reply text from a finished run. Both CLIs print only the final
/// message on stdout (diagnostics go to stderr), so a clean exit with output
/// is the answer; anything else is a provider error. stderr is reduced to a
/// short excerpt: it can echo account details, never worth logging whole.
pub fn cli_reply(
    provider: RouterProvider,
    output: Option<std::process::Output>,
) -> Result<String, String> {
    let name = provider.short_name();
    let Some(output) = output else {
        return Err(format!("{name} did not finish"));
    };
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !stdout.is_empty() {
        return Ok(stdout);
    }
    let first_line = String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect::<String>();
    Err(format!(
        "{name} exited with {} ({first_line})",
        output.status
    ))
}

/// Empty working directory for routing runs, so neither CLI picks up a
/// project's instructions or files.
fn scratch_dir() -> PathBuf {
    std::env::temp_dir().join("buzz-router")
}

/// Run one routing call on a subscription route. Blocking (call it from
/// `spawn_blocking`); bounded by `timeout`, which kills the whole tree.
pub fn complete_via_cli(
    provider: RouterProvider,
    program: &Path,
    model: &str,
    system: &str,
    user: &str,
    timeout: Duration,
) -> Result<String, String> {
    if !is_safe_cli_model(model) {
        return Err(format!(
            "model id {model:?} can't be passed to {}",
            provider.short_name()
        ));
    }
    let dir = scratch_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("router scratch dir: {e}"))?;
    write_instructions(&dir, system)?;
    let mut command = std::process::Command::new(program);
    command.args(cli_args(provider, model)).current_dir(&dir);
    if let Some(path) = cli_probe::augmented_path() {
        command.env("PATH", path);
    }
    if provider == RouterProvider::ClaudeCode {
        // The user chose the Claude sign-in: an inherited API key would make
        // `claude -p` bill that key instead. `CLAUDECODE` marks a nested run.
        command
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            .env_remove("CLAUDECODE")
            // A routing pick needs no extended thinking: it only costs tokens.
            .env("MAX_THINKING_TOKENS", "0");
    }
    let output = output_with_timeout_and_input(command, format!("{user}\n").into_bytes(), timeout);
    let reply = cli_reply(provider, output);
    if reply.is_err() {
        forget_cli_state(provider);
    }
    reply
}

/// Model ids offered for a subscription route in the Task models row.
/// Codex: the "list"-visible slugs from the Codex CLI's own model cache
/// (`$CODEX_HOME/models_cache.json`; a model list, no credentials), falling
/// back to the default. Claude Code: its stable model aliases.
pub fn cli_models(provider: RouterProvider) -> Vec<String> {
    match provider {
        RouterProvider::Codex => {
            let home = std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
            let listed = home
                .and_then(|home| std::fs::read_to_string(home.join("models_cache.json")).ok())
                .map(|content| codex_listed_models(&content))
                .unwrap_or_default();
            if listed.is_empty() {
                vec![provider.default_model().to_string()]
            } else {
                listed
            }
        }
        RouterProvider::ClaudeCode => ["haiku", "sonnet"].map(str::to_string).to_vec(),
        RouterProvider::Anthropic | RouterProvider::Openai | RouterProvider::Openrouter => {
            Vec::new()
        }
    }
}

/// Slugs with `"visibility": "list"` from a Codex `models_cache.json`.
pub fn codex_listed_models(content: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Vec::new();
    };
    value
        .get("models")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter(|model| model.get("visibility").and_then(|v| v.as_str()) == Some("list"))
        .filter_map(|model| model.get("slug").and_then(|slug| slug.as_str()))
        .filter(|slug| is_safe_cli_model(slug))
        .map(str::to_string)
        .collect()
}
