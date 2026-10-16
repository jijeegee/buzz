use std::collections::HashMap;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use super::{
    append_log_marker, current_instance_id, now_iso, process_belongs_to_us,
    process_has_kuzz_marker, process_is_running, terminate_process, ManagedAgentPairRuntime,
    ManagedAgentRecord, ManagedAgentRuntimeKey,
};

pub(crate) fn managed_agent_runtime_keys<T>(
    runtimes: &HashMap<ManagedAgentRuntimeKey, T>,
    pubkey: &str,
) -> Vec<ManagedAgentRuntimeKey> {
    runtimes
        .keys()
        .filter(|key| key.pubkey.eq_ignore_ascii_case(pubkey))
        .cloned()
        .collect()
}

#[cfg(test)]
pub(crate) fn managed_agent_runtime_relay_urls<T>(
    runtimes: &HashMap<ManagedAgentRuntimeKey, T>,
    pubkey: &str,
) -> Vec<String> {
    managed_agent_runtime_keys(runtimes, pubkey)
        .into_iter()
        .map(|key| key.relay_url)
        .collect()
}

/// How long a stopped pair gets to exit before it is killed outright.
const STOP_EXIT_GRACE: Duration = Duration::from_secs(5);
/// How long a killed pair gets to exit before the stop gives up.
const STOP_KILL_GRACE: Duration = Duration::from_secs(2);

/// Terminate an owned pair runtime's process tree and reap it, never waiting
/// without a bound: a stop runs under the runtime locks, so an unbounded wait
/// on a child that did not die stalls every agent operation (and the whole
/// app when the stop came in on the main thread). Unix signals the process
/// group; Windows drops the Job Object (KILL_ON_JOB_CLOSE reaps the tree) or
/// kills the child when no job was assigned. Already-exited children are
/// reaped without a signal. Err means the child is still running.
pub(crate) fn terminate_and_reap(
    runtime: &mut ManagedAgentPairRuntime,
) -> Result<ExitStatus, String> {
    #[cfg(windows)]
    let job = runtime.job.take();
    reap_within(
        &mut runtime.child,
        |child| {
            #[cfg(unix)]
            {
                let _ = child;
                terminate_process(child.id())
            }
            #[cfg(windows)]
            match job {
                Some(job) => {
                    drop(job);
                    Ok(())
                }
                None => child
                    .kill()
                    .map_err(|error| format!("failed to kill agent process: {error}")),
            }
            #[cfg(not(any(unix, windows)))]
            child
                .kill()
                .map_err(|error| format!("failed to kill agent process: {error}"))
        },
        |child| {
            let _ = child.kill();
        },
        STOP_EXIT_GRACE,
        STOP_KILL_GRACE,
    )
}

/// [`terminate_and_reap`] with the termination steps and bounds supplied.
fn reap_within(
    child: &mut Child,
    terminate: impl FnOnce(&mut Child) -> Result<(), String>,
    force: impl FnOnce(&mut Child),
    grace: Duration,
    force_grace: Duration,
) -> Result<ExitStatus, String> {
    if let Some(status) = try_reap(child)? {
        return Ok(status);
    }
    terminate(child)?;
    if let Some(status) = wait_for_exit(child, grace)? {
        return Ok(status);
    }
    force(child);
    wait_for_exit(child, force_grace)?.ok_or_else(|| {
        format!(
            "agent process {} did not exit within {}s of being stopped",
            child.id(),
            (grace + force_grace).as_secs()
        )
    })
}

fn try_reap(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    child
        .try_wait()
        .map_err(|error| format!("failed to wait for agent shutdown: {error}"))
}

fn wait_for_exit(child: &mut Child, within: Duration) -> Result<Option<ExitStatus>, String> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = try_reap(child)? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stop the single tracked runtime pair at `key`, if present.
///
/// Terminates the child, records the exit code, removes the pair receipt,
/// and appends a stop marker to the pair log. On teardown failure the
/// runtime is reinserted so the pair stays visible and stoppable instead of
/// becoming an invisible orphan. Touches no other pair for the agent and
/// does no record-level stop bookkeeping — callers own that.
fn stop_managed_agent_pair<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &mut ManagedAgentRecord,
    runtimes: &mut HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>,
    key: &ManagedAgentRuntimeKey,
) -> Result<(), String> {
    let Some(mut runtime) = runtimes.remove(key) else {
        return Ok(());
    };
    let result = (|| -> Result<(), String> {
        let status = terminate_and_reap(&mut runtime)?;
        record.last_exit_code = status.code();
        super::super::remove_agent_runtime_receipt(app, key);
        if let Err(error) = append_log_marker(
            &runtime.log_path,
            &format!(
                "=== stopped {} ({}) at {} ===",
                record.name,
                record.pubkey,
                now_iso()
            ),
        ) {
            eprintln!(
                "kuzz-desktop: failed to append stop marker for {} on {}: {error}",
                record.pubkey, key.relay_url
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        // Keep failed teardown visible/manageable instead of orphaning it.
        runtimes.insert(key.clone(), runtime);
        return Err(error);
    }
    Ok(())
}

/// Terminate a legacy scalar-PID child (pre-pair records) and remove the
/// agent-scoped pid file. Pair receipts are restored separately.
fn stop_legacy_scalar_pid<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &mut ManagedAgentRecord,
) -> Result<(), String> {
    if let Some(pid) = record.runtime_pid.take() {
        if process_is_running(pid)
            && process_belongs_to_us(pid)
            && process_has_kuzz_marker(pid, &current_instance_id(app))
        {
            terminate_process(pid)?;
        }
        record.updated_at = now_iso();
    }
    super::super::remove_agent_pid_file(app, &record.pubkey);
    Ok(())
}

/// Stop the runtime pair this record resolves to for the active workspace
/// (explicit relay pin, else the active workspace relay) — the pair-scoped
/// counterpart of [`stop_managed_agent_process`], which drains every pair.
///
/// Community-scoped surfaces (profile panel, Agents tab, auto-restart) stop
/// through here so stopping an agent in one community never tears down its
/// pairs in other communities. Clears the matching agent session cache
/// (pair-scoped when a pair key resolves). When no pair is tracked for this
/// workspace, only legacy scalar-PID cleanup runs.
pub fn stop_managed_agent_workspace_pair(
    app: &AppHandle,
    record: &mut ManagedAgentRecord,
    runtimes: &mut HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>,
) -> Result<(), String> {
    use tauri::Manager;
    let state = app.state::<crate::app_state::AppState>();
    match super::workspace_pair_key(app, record) {
        Some(pair_key) if runtimes.contains_key(&pair_key) => {
            stop_managed_agent_pair(app, record, runtimes, &pair_key)?;
            state.clear_agent_session_cache(&pair_key);
            super::super::remove_agent_pid_file(app, &record.pubkey);
            let now = now_iso();
            record.runtime_pid = None;
            record.updated_at = now.clone();
            record.last_stopped_at = Some(now);
            record.last_error = None;
            record.last_error_code = None;
        }
        Some(pair_key) => {
            // No tracked pair here — a pubkey-wide cache clear would disturb
            // live pairs in other communities, so stay pair-scoped.
            stop_legacy_scalar_pid(app, record)?;
            state.clear_agent_session_cache(&pair_key);
        }
        None => {
            stop_legacy_scalar_pid(app, record)?;
            state.clear_agent_session_caches(&record.pubkey);
        }
    }
    Ok(())
}

pub fn stop_managed_agent_process<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &mut ManagedAgentRecord,
    runtimes: &mut HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>,
) -> Result<(), String> {
    let keys = managed_agent_runtime_keys(runtimes, &record.pubkey);
    if keys.is_empty() {
        return stop_legacy_scalar_pid(app, record);
    }

    let mut errors = Vec::new();
    for key in keys {
        if let Err(error) = stop_managed_agent_pair(app, record, runtimes, &key) {
            errors.push(format!("{}: {error}", key.relay_url));
        }
    }

    let now = now_iso();
    record.runtime_pid = None;
    record.updated_at = now.clone();
    record.last_stopped_at = Some(now);
    record.last_error = None;
    record.last_error_code = None;
    super::super::remove_agent_pid_file(app, &record.pubkey);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "failed to stop one or more managed-agent runtimes: {}",
            errors.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_preserving_restart_targets_exact_original_relays() {
        let agent = "aa".repeat(32);
        let other = "bb".repeat(32);
        let first = ManagedAgentRuntimeKey::new(&agent, "wss://one.example").unwrap();
        let second = ManagedAgentRuntimeKey::new(&agent, "wss://two.example").unwrap();
        let unrelated = ManagedAgentRuntimeKey::new(other, "wss://fallback.example").unwrap();
        let runtimes = HashMap::from([(first, ()), (second, ()), (unrelated, ())]);

        let mut relays = managed_agent_runtime_relay_urls(&runtimes, &agent);
        relays.sort();
        assert_eq!(
            relays,
            vec![
                "wss://one.example".to_string(),
                "wss://two.example".to_string()
            ]
        );
    }

    #[test]
    fn pair_scoped_selection_targets_only_the_exact_pair() {
        // stop_managed_agent_workspace_pair resolves one key and removes only
        // that map entry: the same agent's pair on another relay and other
        // agents' pairs must survive a pair-scoped stop.
        let agent = "aa".repeat(32);
        let other = "bb".repeat(32);
        let viewed = ManagedAgentRuntimeKey::new(&agent, "wss://one.example").unwrap();
        let elsewhere = ManagedAgentRuntimeKey::new(&agent, "wss://two.example").unwrap();
        let unrelated = ManagedAgentRuntimeKey::new(other, "wss://one.example").unwrap();
        let mut runtimes = HashMap::from([
            (viewed.clone(), ()),
            (elsewhere.clone(), ()),
            (unrelated.clone(), ()),
        ]);

        // Non-canonical spelling of the viewed workspace relay resolves to
        // the same canonical key that spawn stamped.
        let resolved = ManagedAgentRuntimeKey::new(&agent, "WSS://One.Example:443/").unwrap();
        assert_eq!(resolved, viewed);
        assert!(runtimes.remove(&resolved).is_some());
        assert!(runtimes.contains_key(&elsewhere));
        assert!(runtimes.contains_key(&unrelated));
    }

    #[test]
    fn agent_wide_selection_drains_every_pair_only_for_that_agent() {
        let agent = "aa".repeat(32);
        let other = "bb".repeat(32);
        let first = ManagedAgentRuntimeKey::new(&agent, "wss://one.example").unwrap();
        let second = ManagedAgentRuntimeKey::new(&agent, "wss://two.example").unwrap();
        let unrelated = ManagedAgentRuntimeKey::new(other, "wss://one.example").unwrap();
        let runtimes = HashMap::from([(first.clone(), ()), (second.clone(), ()), (unrelated, ())]);

        let mut selected = managed_agent_runtime_keys(&runtimes, &agent);
        selected.sort_by(|left, right| left.relay_url.cmp(&right.relay_url));
        assert_eq!(selected, vec![first, second]);
    }

    /// A child that runs until it is killed.
    fn long_running_child() -> Child {
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            let mut command = std::process::Command::new("ping");
            command
                .args(["-n", "60", "127.0.0.1"])
                .creation_flags(0x0800_0000);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = std::process::Command::new("sleep");
            command.arg("60");
            command
        };
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn long-running child")
    }

    #[test]
    fn process_is_running_tracks_a_real_child() {
        let mut child = long_running_child();
        assert!(process_is_running(child.id()));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!process_is_running(child.id()));
    }

    #[test]
    fn reap_within_terminates_and_reaps_a_live_child() {
        let mut child = long_running_child();
        let status = reap_within(
            &mut child,
            |child| child.kill().map_err(|error| error.to_string()),
            |_| panic!("terminate already ended the child"),
            Duration::from_secs(5),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(!status.success());
        assert!(!process_is_running(child.id()));
    }

    #[test]
    fn reap_within_escalates_when_terminate_does_not_end_the_child() {
        let mut child = long_running_child();
        let mut forced = false;
        reap_within(
            &mut child,
            |_| Ok(()),
            |child| {
                forced = true;
                let _ = child.kill();
            },
            Duration::from_millis(200),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(forced);
    }

    #[test]
    fn reap_within_gives_up_instead_of_waiting_forever() {
        // The stop that froze the app: the child never exits. The stop must
        // return an error within its bounds so the runtime locks are released.
        let mut child = long_running_child();
        let started = Instant::now();
        let result = reap_within(
            &mut child,
            |_| Ok(()),
            |_| {},
            Duration::from_millis(200),
            Duration::from_millis(200),
        );
        assert!(result.is_err(), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            process_is_running(child.id()),
            "the child is left for a retry"
        );
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn reap_within_reaps_an_exited_child_without_signalling_it() {
        let mut child = long_running_child();
        child.kill().unwrap();
        child.wait().unwrap();
        reap_within(
            &mut child,
            |_| panic!("an exited child is not terminated again"),
            |_| panic!("an exited child is not forced"),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .unwrap();
    }
}
