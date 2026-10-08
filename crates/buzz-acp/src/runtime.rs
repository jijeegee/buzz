//! Shared agent capabilities. Intake and scheduling belong to each entry point;
//! environment, credentials, adapter settings and prompt equipment belong here.
use crate::{
    build_mcp_servers, config::Config, current_working_directory, git, observer, pool, relay,
    resolve_agent_owner, PromptContext,
};
use anyhow::Result;
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

pub(crate) enum SessionMode {
    Conversation,
    Task,
}

/// Own shared launch resources until all adapter work has been drained.
/// Add new runtime capabilities here, not separately to the CLI entry points.
pub(crate) struct AgentRuntime {
    config: Config,
    _git_environment: git::GitEnvironment,
}

impl AgentRuntime {
    pub(crate) fn prepare(mut config: Config) -> Result<Self> {
        let git_environment = git::GitEnvironment::for_config(&mut config)?;
        Ok(Self {
            config,
            _git_environment: git_environment,
        })
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) fn startup(&self, observer: Option<observer::ObserverHandle>) -> PoolStartup {
        PoolStartup::from_config(&self.config, observer)
    }

    pub(crate) fn prompt_context(
        &self,
        rest: relay::RestClient,
        channels: HashMap<Uuid, relay::ChannelInfo>,
        mode: SessionMode,
    ) -> Result<PromptContext> {
        make_prompt_context(&self.config, rest, channels, mode)
    }
}

#[derive(Clone)]
pub(crate) struct PoolStartup {
    pub(crate) agents: u32,
    pub(crate) command: String,
    pub(crate) args: Vec<String>,
    pub(crate) extra_env: Vec<(String, String)>,
    pub(crate) has_generated_codex_config: bool,
    pub(crate) model: Option<String>,
    pub(crate) effort_level: Option<String>,
    pub(crate) observer: Option<observer::ObserverHandle>,
}

impl PoolStartup {
    fn from_config(config: &Config, observer: Option<observer::ObserverHandle>) -> Self {
        Self {
            agents: config.agents,
            command: config.agent_command.clone(),
            args: config.agent_args.clone(),
            extra_env: config.persona_env_vars.clone(),
            has_generated_codex_config: config.has_generated_codex_config,
            model: config.model.clone(),
            effort_level: config.effort_level.clone(),
            observer,
        }
    }
}

/// The standing base for `mode`: task sessions get the task session model;
/// conversation sessions get the configured policy's model, plus the
/// self-opened task thread guidance for thread-policy agents that are not
/// route-only dispatchers.
fn assemble_base_prompt(
    policy: crate::scope::SessionPolicy,
    dispatcher: bool,
    task_threads: &[crate::task_threads::TaskThreadTrigger],
    mode: &SessionMode,
    base: &str,
) -> String {
    if matches!(mode, SessionMode::Task) {
        return format!("{base}\n\n{}", include_str!("session_model_task.md"));
    }
    let base = policy.append_session_model(base);
    if policy == crate::scope::SessionPolicy::Thread && !dispatcher {
        crate::task_threads::append_task_thread_guidance(base, task_threads)
    } else {
        base
    }
}

#[cfg(test)]
mod assemble_base_prompt_tests {
    use super::*;
    use crate::scope::SessionPolicy;
    use crate::task_threads::TaskThreadTrigger;

    const GUIDANCE: &str = "### Opening Task Threads Yourself";
    const TRIGGERS: &[TaskThreadTrigger] = &[TaskThreadTrigger::LongRunning];

    #[test]
    fn thread_policy_conversation_gets_the_guidance() {
        let prompt = assemble_base_prompt(
            SessionPolicy::Thread,
            false,
            TRIGGERS,
            &SessionMode::Conversation,
            "base",
        );
        assert!(prompt.contains(GUIDANCE));
    }

    #[test]
    fn channel_policy_dispatchers_and_task_sessions_do_not() {
        for (policy, dispatcher, mode) in [
            (SessionPolicy::Channel, false, SessionMode::Conversation),
            (SessionPolicy::Thread, true, SessionMode::Conversation),
            (SessionPolicy::Thread, false, SessionMode::Task),
        ] {
            let prompt = assemble_base_prompt(policy, dispatcher, TRIGGERS, &mode, "base");
            assert!(
                !prompt.contains(GUIDANCE),
                "{policy} dispatcher={dispatcher}"
            );
        }
    }
}

fn make_prompt_context(
    config: &Config,
    rest_client: relay::RestClient,
    channels: HashMap<Uuid, relay::ChannelInfo>,
    mode: SessionMode,
) -> Result<PromptContext> {
    let base_prompt_content = config.base_prompt_content.as_ref();
    let cwd = current_working_directory()?;
    Ok(PromptContext {
        mcp_servers: build_mcp_servers(config),
        initial_message: config.initial_message.clone(),
        idle_timeout: Duration::from_secs(config.idle_timeout_secs),
        max_turn_duration: Duration::from_secs(config.max_turn_duration_secs),
        turn_liveness_interval: Duration::from_secs(config.turn_liveness_secs),
        dedup_mode: config.dedup_mode,
        system_prompt: config.system_prompt.clone(),
        session_title: config.session_title.clone(),
        team_instructions: config.team_instructions.clone(),
        layer0_goals: config.layer0_goals.clone(),
        goals_enabled: config.goals_enabled,
        base_prompt: if config.no_base_prompt {
            None
        } else {
            // Build standing context once under the configured policy, before
            // any session/new. Both modern ACP and legacy first-turn framing
            // consume this same assembled base (including custom base files).
            let base = base_prompt_content
                .map(String::as_str)
                .unwrap_or(include_str!("base_prompt.md"));
            // Goal-layer rules only reach agents whose owner turned the
            // experimental feature on.
            let with_goals;
            let base = if config.goals_enabled {
                with_goals = format!("{base}\n\n{}", include_str!("goals_prompt.md"));
                with_goals.as_str()
            } else {
                base
            };
            Some(assemble_base_prompt(
                config.session_policy,
                config.dispatcher,
                &config.task_threads,
                &mode,
                base,
            ))
        },
        heartbeat_prompt: config.heartbeat_prompt.clone(),
        cwd,
        rest_client: rest_client.clone(),
        channel_info: pool::ChannelInfoResolver::new(channels, rest_client),
        context_message_limit: config.context_message_limit,
        context_history: config.context_history,
        max_turns_per_session: config.max_turns_per_session,
        permission_mode: config.permission_mode,
        agent_keys: config.keys.clone(),
        agent_owner_pubkey: resolve_agent_owner(config)
            .as_deref()
            .and_then(|hex| nostr::PublicKey::from_hex(hex).ok()),
        memory_enabled: config.memory_enabled,
        harness_name: crate::config::normalize_agent_command_identity(&config.agent_command),
        relay_url: config.relay_url.clone(),
        dispatcher: config.dispatcher,
        channel_roster: config.channel_roster,
        session_ledger: session_ledger_for(config, mode),
    })
}

/// The durable resume ledger, enabled only for conversation sessions under the
/// thread policy with a configured state directory. Other policies
/// keep their existing restart-from-relay-context behavior unchanged.
fn session_ledger_for(
    config: &Config,
    mode: SessionMode,
) -> Option<std::sync::Arc<crate::session_ledger::SessionLedger>> {
    if !matches!(mode, SessionMode::Conversation)
        || config.session_policy != crate::scope::SessionPolicy::Thread
    {
        return None;
    }
    let state_dir = config.state_dir.as_ref()?;
    let path = crate::session_ledger::ledger_path(
        state_dir,
        &config.keys.public_key().to_hex(),
        &config.relay_url,
    );
    tracing::info!(target: "acp::ledger", path = %path.display(), "session resume ledger enabled");
    Some(std::sync::Arc::new(
        crate::session_ledger::SessionLedger::open(path),
    ))
}
