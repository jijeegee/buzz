//! Auth watchdog: restarts managed agents whose `buzz-acp` exited 78.
//!
//! Exits are observed by the regular process sync (UI poll) and, while any
//! community is signed in with Google, by this loop's own sync pass, so a
//! hidden window still recovers its agents. Each exit goes through the
//! [`super::bots::RestartLimiter`]: within the cap the agent is restarted
//! (which reissues its bot token); at the cap it stays stopped and the UI
//! shows why, with Start as the manual reset (Rules 4 and 6).

use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use super::bots::{self, RestartDecision};
use crate::app_state::AppState;

const TICK: Duration = Duration::from_secs(3);

/// Run forever (app lifetime).
pub(crate) async fn run(app: AppHandle) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if any_token_session(&app) {
            let sync_app = app.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || sync_processes(&sync_app)).await;
        }
        for (agent, relay_url) in bots::take_exits() {
            let state = app.state::<AppState>();
            match state
                .token_auth
                .bots
                .record_auth_exit(&agent, Instant::now())
            {
                RestartDecision::Capped => {
                    eprintln!(
                        "buzz-desktop: auth: agent {agent} hit the auth restart cap; waiting for a manual restart"
                    );
                }
                RestartDecision::Restart => {
                    let restart_app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let result = crate::managed_agents::start_pair_after_auth_exit(
                            agent.clone(),
                            relay_url,
                            restart_app.clone(),
                        );
                        let state = restart_app.state::<AppState>();
                        settle_restart(&state.token_auth.bots, &agent, result);
                    });
                }
            }
        }
    }
}

/// Settle an automatic restart: a failure (e.g. the bot token could not be
/// reissued) parks the agent as auth-failed with the reason, so the UI shows
/// why it stopped and Start is the way back (Rule 6), instead of the agent
/// silently staying down.
pub(crate) fn settle_restart(bots: &bots::BotRuntime, agent: &str, result: Result<(), String>) {
    if let Err(error) = result {
        eprintln!("buzz-desktop: auth: restart of {agent} failed: {error}");
        bots.park(
            agent,
            format!("The agent could not be restarted with a new sign-in token: {error}. Press Start to try again."),
        );
    }
}

fn any_token_session(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    matches!(
        state.current_credential_mode(),
        super::CredentialMode::Token(_)
    )
}

/// The same locked sync `list_managed_agents` performs.
fn sync_processes(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(_store_guard) = state.managed_agents_store_lock.lock() else {
        return;
    };
    let Ok(mut records) = crate::managed_agents::load_managed_agents(app) else {
        return;
    };
    let Ok(mut runtimes) = state.managed_agent_processes.lock() else {
        return;
    };
    let (changed, exited) = crate::managed_agents::sync_managed_agent_processes(
        &mut records,
        &mut runtimes,
        &crate::managed_agents::current_instance_id(app),
    );
    if changed {
        let _ = crate::managed_agents::save_managed_agents(app, &records);
    }
    for pubkey in &exited {
        state.clear_agent_session_caches(pubkey);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_restart_parks_the_agent_with_the_reason() {
        let bots = bots::BotRuntime::default();
        settle_restart(&bots, "a1", Ok(()));
        assert_eq!(bots.auth_failed("a1"), None);
        settle_restart(&bots, "a1", Err("relay said 401".into()));
        let reason = bots.auth_failed("a1").expect("parked");
        assert!(reason.contains("relay said 401"), "{reason}");
        bots.reset("a1");
        assert_eq!(bots.auth_failed("a1"), None);
    }
}
