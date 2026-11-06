//! Getting a Hermes gateway to load the Kuzz connector, on whichever Hermes
//! this computer has.
//!
//! - One gateway may serve every profile, or each profile may run its own:
//!   [`restart_profile`] picks the gateway that runs this profile.
//! - `--detach` exists only in newer Hermes, and older Hermes has no
//!   live-session guard: [`settle`] asks the launcher what it supports and
//!   waits for running work itself.
//! - An exit code says little (0 can mean only "requested"), so the screen
//!   reads Hermes's own state file ([`gateway_view`]). `kuzz/gateway-restart.log`
//!   under the Hermes home keeps what the last restarts printed.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use super::pairing::{hermes_home, DEFAULT_PROFILE};
use crate::managed_agents::process_is_running;

/// Hermes's state file, in each home that runs a gateway.
const STATE_FILE: &str = "gateway_state.json";
const LOG_FILE: &str = "gateway-restart.log";
/// How often a gateway with running work is looked at again.
const BUSY_POLL: Duration = Duration::from_secs(15);
/// Longest wait for running work. The row offers a restart after this.
const BUSY_LIMIT: Duration = Duration::from_secs(600);
/// Longest wait for the restart command. Hermes drains for up to 180 s, and
/// an older Hermes without a service stays on as the gateway.
const EXIT_LIMIT: Duration = Duration::from_secs(240);
/// Longest wait for Kuzz to come up after the restart exited.
const CONNECT_LIMIT: Duration = Duration::from_secs(90);
/// Longest the `--help` probe may take: Hermes starts slowly.
const HELP_LIMIT: Duration = Duration::from_secs(30);

/// `start_time` drifts between readings of one gateway, as the plugin's
/// `install.py` notes.
const START_TIME_DRIFT: f64 = 200.0;

/// Profiles with a restart under way.
static SETTLING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
/// Restarts run one at a time: two profiles can share a gateway.
static RESTARTS: Mutex<()> = Mutex::new(());

/// What Hermes's state file says about one profile's gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HermesGatewayView {
    running: bool,
    /// Hermes's state of the Kuzz platform; `None`: not loaded.
    kuzz: Option<String>,
    /// Hermes's words on what is wrong with the Kuzz platform.
    error: Option<String>,
    /// It has work running.
    busy: bool,
}

fn read_state(home: &Path) -> Value {
    std::fs::read(home.join(STATE_FILE))
        .ok()
        .and_then(|state| serde_json::from_slice(&state).ok())
        .unwrap_or(Value::Null)
}

fn is_served_by(host: &Value, profile: &str) -> bool {
    host.get("served_profiles")
        .and_then(Value::as_array)
        .is_some_and(|names| names.iter().any(|name| name.as_str() == Some(profile)))
}

/// The state file names a live state and that process exists: a crash or a
/// reboot leaves the file saying "running".
fn is_alive(record: &Value) -> bool {
    matches!(
        record.get("gateway_state").and_then(Value::as_str),
        Some("starting" | "running" | "degraded" | "draining")
    ) && record
        .get("pid")
        .and_then(Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .is_some_and(process_is_running)
}

/// Both records are one gateway process. Pure.
fn same_gateway(now: &Value, then: &Value) -> bool {
    let start = |record: &Value| record.get("start_time").and_then(Value::as_f64);
    let close = match (start(now), start(then)) {
        (Some(now), Some(then)) => (now - then).abs() <= START_TIME_DRIFT,
        _ => true,
    };
    now.get("pid") == then.get("pid") && close
}

fn busy(record: &Value) -> bool {
    record
        .get("active_agents")
        .and_then(Value::as_i64)
        .is_some_and(|count| count > 0)
}

/// Which `-p` runs `profile`: the host when it serves it or the profile never
/// ran a gateway of its own, else that gateway (a record, or `has_service`).
/// Pure.
fn restart_profile<'a>(profile: &'a str, host: &Value, own: &Value, has_service: bool) -> &'a str {
    if profile == DEFAULT_PROFILE
        || is_served_by(host, profile)
        || !(has_service || own.get("pid").is_some())
    {
        DEFAULT_PROFILE
    } else {
        profile
    }
}

fn gateway_view_from(record: &Value, key: &str) -> Option<HermesGatewayView> {
    let running = is_alive(record);
    let platforms = record.get("platforms").and_then(Value::as_object);
    if running && platforms.is_none() {
        return None;
    }
    // A new gateway keeps the last one's entries until it writes its own.
    let kuzz = platforms
        .and_then(|platforms| platforms.get(key))
        .filter(|kuzz| {
            kuzz.get("writer_pid")
                .is_none_or(|writer| Some(writer) == record.get("pid"))
        });
    let text = |name: &str| {
        kuzz.and_then(|platform| platform.get(name)?.as_str())
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Some(HermesGatewayView {
        running,
        kuzz: text("state"),
        error: text("error_message"),
        busy: running && busy(record),
    })
}

/// `None`: a live gateway that doesn't report its platforms, so nothing is
/// known. No state file means no gateway has run.
pub(super) fn gateway_view(root: &Path, profile: &str) -> Option<HermesGatewayView> {
    let host = read_state(root);
    if profile == DEFAULT_PROFILE {
        return gateway_view_from(&host, "kuzz");
    }
    if is_served_by(&host, profile) {
        return gateway_view_from(&host, &format!("{profile}:kuzz"));
    }
    gateway_view_from(&read_state(&profile_home(root, profile)), "kuzz")
}

fn profile_home(root: &Path, profile: &str) -> PathBuf {
    if profile == DEFAULT_PROFILE {
        root.to_owned()
    } else {
        root.join("profiles").join(profile)
    }
}

/// Whether `profile` has a launchd job of its own, running or not.
fn has_service(profile: &str) -> bool {
    cfg!(target_os = "macos")
        && dirs::home_dir().is_some_and(|home| {
            home.join("Library/LaunchAgents")
                .join(format!("ai.hermes.gateway-{profile}.plist"))
                .is_file()
        })
}

pub(super) fn is_settling(profile: &str) -> bool {
    SETTLING
        .lock()
        .is_ok_and(|settling| settling.contains(profile))
}

/// Ends the profile's restart however the thread ends.
struct Settling(String);

impl Drop for Settling {
    fn drop(&mut self) {
        if let Ok(mut settling) = SETTLING.lock() {
            settling.remove(&self.0);
        }
    }
}

/// Make the gateway running `profile` load the plugin and home channel: once
/// it has no running work, one restart. Returns at once; a second call while
/// one runs does nothing, and a restart by anyone else ends the wait.
pub(super) fn settle(profile: &str) {
    let inserted = SETTLING
        .lock()
        .is_ok_and(|mut settling| settling.insert(profile.to_owned()));
    if !inserted {
        return;
    }
    let guard = Settling(profile.to_owned());
    std::thread::spawn(move || {
        let Some(root) = hermes_home() else {
            return;
        };
        let _one_at_a_time = RESTARTS.lock();
        if let Err(reason) = restart(&root, &guard.0) {
            log(&root, &reason);
        }
    });
}

fn restart(root: &Path, profile: &str) -> Result<(), String> {
    let hermes = hermes_launcher(root).ok_or("no hermes launcher; gateway not restarted")?;
    let target = restart_profile(
        profile,
        &read_state(root),
        &read_state(&profile_home(root, profile)),
        has_service(profile),
    )
    .to_owned();
    let detach = supports_detach(&hermes, root);
    if !wait_for_idle(&profile_home(root, &target)) {
        log(root, "no restart: restarted meanwhile, or still busy");
        return Ok(());
    }
    let mut command = hermes_command(&hermes, root);
    command.args(restart_args(&target, detach));
    let output = log_file(root);
    if let Some(output) = output.as_ref().and_then(|file| file.try_clone().ok()) {
        command.stdout(output);
    }
    if let Some(output) = output {
        command.stderr(output);
    }
    log(root, &format!("restarting {target} (detach: {detach})"));
    let target_home = profile_home(root, &target);
    let before = read_state(&target_home);
    let mut child = command
        .spawn()
        .map_err(|error| format!("gateway restart failed to start: {error}"))?;
    let mut deadline = Instant::now() + EXIT_LIMIT;
    let mut exited = false;
    while Instant::now() < deadline {
        let now = read_state(&target_home);
        if !now.is_null()
            && !same_gateway(&now, &before)
            && gateway_view(root, profile)
                .is_some_and(|view| view.running && view.kuzz.as_deref() == Some("connected"))
        {
            log(root, "Kuzz is connected");
            reap(child);
            return Ok(());
        }
        if !exited {
            let status = child
                .try_wait()
                .map_err(|error| format!("gateway restart: {error}"))?;
            if let Some(status) = status {
                log(
                    root,
                    &format!("gateway restart exited ({:?})", status.code()),
                );
                if !status.success() {
                    return Ok(());
                }
                exited = true;
                deadline = Instant::now() + CONNECT_LIMIT;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    log(root, "Kuzz is not connected yet");
    reap(child);
    Ok(())
}

/// A restart that stays on as the gateway: on exit it must not stay behind
/// as a zombie, which [`is_alive`] would take for a running gateway.
fn reap(mut child: Child) {
    std::thread::spawn(move || child.wait());
}

/// Wait while the gateway in `home` has work. False: it was restarted
/// meanwhile, or the work outlasted [`BUSY_LIMIT`].
fn wait_for_idle(home: &Path) -> bool {
    let first = read_state(home);
    let started = Instant::now();
    let mut record = first.clone();
    while busy(&record) && is_alive(&record) {
        if started.elapsed() >= BUSY_LIMIT {
            return false;
        }
        std::thread::sleep(BUSY_POLL);
        // Null: a read that met a write; the last record stands.
        let next = read_state(home);
        if !next.is_null() {
            if !same_gateway(&next, &first) {
                return false;
            }
            record = next;
        }
    }
    true
}

/// Pure.
fn restart_args(profile: &str, detach: bool) -> Vec<&str> {
    let mut args = vec!["-p", profile, "gateway", "restart"];
    if detach {
        args.push("--detach");
    }
    args
}

fn supports_detach(hermes: &Path, root: &Path) -> bool {
    let mut help = hermes_command(hermes, root);
    help.args(["gateway", "restart", "--help"]);
    help.stdout(Stdio::piped());
    crate::managed_agents::output_with_timeout(help, HELP_LIMIT)
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).contains("--detach"))
}

/// Hermes outside any job of this app, no pipes, no window, without Kuzz
/// settings or secrets in its env (it reads them from its own `.env`).
fn hermes_command(hermes: &Path, root: &Path) -> Command {
    let mut command = Command::new(hermes);
    command
        .env("HERMES_HOME", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if is_kuzz_setting(&name) {
            command.env_remove(name);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn log_file(root: &Path) -> Option<File> {
    let folder = root.join("kuzz");
    std::fs::create_dir_all(&folder).ok()?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(folder.join(LOG_FILE))
        .ok()
}

fn log(root: &Path, line: &str) {
    eprintln!("kuzz-desktop: hermes: {line}");
    if let Some(mut file) = log_file(root) {
        let now = chrono::Utc::now().to_rfc3339();
        let _ = writeln!(file, "{now} {line}");
    }
}

/// The `hermes` launcher: on `PATH`, else where installers put it. On Windows
/// only `hermes.exe` (a `.cmd`/`.bat` is never run), as the plugin installer.
pub(super) fn hermes_launcher(home: &Path) -> Option<PathBuf> {
    let name = format!("hermes{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    let in_home = [
        home.join("bin"),
        home.join("hermes-agent").join("venv").join("Scripts"),
        home.join("hermes-agent").join("venv").join("bin"),
    ];
    let local = dirs::home_dir().map(|user| user.join(".local").join("bin"));
    on_path
        .into_iter()
        .chain(in_home)
        .chain(local)
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
}

/// Whether an inherited variable is a Kuzz/Buzz setting a restarted gateway
/// must not inherit. Pure.
pub(super) fn is_kuzz_setting(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        let upper = name.to_ascii_uppercase();
        upper.starts_with("KUZZ_") || upper.starts_with("BUZZ_")
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_restarted_gateway_gets_no_kuzz_settings() {
        for name in ["KUZZ_PRIVATE_KEY", "kuzz_relay_url", "BUZZ_HOME_CHANNEL"] {
            assert!(is_kuzz_setting(std::ffi::OsStr::new(name)), "{name}");
        }
        for name in ["PATH", "HERMES_HOME", "XKUZZ_A"] {
            assert!(!is_kuzz_setting(std::ffi::OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn the_gateway_running_the_profile_is_restarted() {
        let live = json!({"pid": std::process::id(), "gateway_state": "running"});
        let stopped = json!({"pid": 7, "gateway_state": "stopped"});
        let host = json!({"served_profiles": ["default", "served"]});
        let alone = json!({"served_profiles": []});
        // The default profile is always the host.
        assert_eq!(restart_profile("default", &alone, &live, true), "default");
        // A host serving the profile loads plugins for it.
        assert_eq!(restart_profile("served", &host, &live, true), "default");
        // One launchd job per profile (a Mac mini fleet).
        assert_eq!(restart_profile("own", &alone, &live, false), "own");
        assert_eq!(restart_profile("own", &alone, &stopped, true), "own");
        // Its own gateway crashed: the host doesn't serve it.
        assert_eq!(restart_profile("own", &alone, &stopped, false), "own");
        // Nothing runs it: the host picks a new profile up.
        assert_eq!(
            restart_profile("new", &alone, &Value::Null, false),
            "default"
        );
    }

    #[test]
    fn detach_is_passed_only_to_a_hermes_that_has_it() {
        assert_eq!(
            restart_args("marketing", true),
            ["-p", "marketing", "gateway", "restart", "--detach"]
        );
        assert_eq!(
            restart_args("default", false),
            ["-p", "default", "gateway", "restart"]
        );
    }

    #[test]
    fn the_view_reads_this_profiles_kuzz_platform() {
        let record = json!({
            "pid": std::process::id(),
            "gateway_state": "running",
            "active_agents": 2,
            "platforms": {
                "telegram": {"state": "connected"},
                "kuzz": {"state": "fatal", "error_message": "not paired"},
                "work:kuzz": {"state": "connected", "error_message": null},
            },
        });
        assert_eq!(
            gateway_view_from(&record, "kuzz"),
            Some(HermesGatewayView {
                running: true,
                kuzz: Some("fatal".into()),
                error: Some("not paired".into()),
                busy: true,
            })
        );
        assert_eq!(
            gateway_view_from(&record, "work:kuzz").unwrap().kuzz,
            Some("connected".into())
        );
        // A gateway that hasn't loaded the plugin.
        assert_eq!(gateway_view_from(&record, "other:kuzz").unwrap().kuzz, None);
        // A new gateway, with the entry of the one before it.
        let pid = std::process::id();
        let inherited = json!({
            "pid": pid,
            "gateway_state": "starting",
            "platforms": {"kuzz": {"state": "connected", "writer_pid": pid + 1}},
        });
        assert_eq!(gateway_view_from(&inherited, "kuzz").unwrap().kuzz, None);
        let own = json!({
            "pid": pid,
            "gateway_state": "running",
            "platforms": {"kuzz": {"state": "connected", "writer_pid": pid}},
        });
        assert_eq!(
            gateway_view_from(&own, "kuzz").unwrap().kuzz.as_deref(),
            Some("connected")
        );
        // A live Hermes without platforms says nothing about Kuzz.
        let bare = json!({"pid": std::process::id(), "gateway_state": "running"});
        assert_eq!(gateway_view_from(&bare, "kuzz"), None);
        // No state file: no gateway.
        let stopped = HermesGatewayView {
            running: false,
            kuzz: None,
            error: None,
            busy: false,
        };
        assert_eq!(gateway_view_from(&Value::Null, "kuzz"), Some(stopped));
    }

    #[test]
    fn a_crashed_gateway_is_not_alive_whatever_its_file_says() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        let record = json!({
            "pid": pid,
            "gateway_state": "running",
            "active_agents": 3,
            "platforms": {"kuzz": {"state": "connected"}},
        });
        assert!(!is_alive(&record));
        let view = gateway_view_from(&record, "kuzz").unwrap();
        assert!(!view.running);
        assert!(!view.busy);
    }

    #[test]
    fn only_another_process_is_another_gateway() {
        let then = json!({"pid": 7, "start_time": 1000});
        assert!(same_gateway(&json!({"pid": 7, "start_time": 1100}), &then));
        assert!(!same_gateway(&json!({"pid": 7, "start_time": 1300}), &then));
        assert!(!same_gateway(&json!({"pid": 8, "start_time": 1000}), &then));
        assert!(!same_gateway(
            &json!({"pid": 7, "start_time": 1000}),
            &Value::Null
        ));
    }

    #[test]
    fn a_served_profile_is_read_from_the_host_record() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(STATE_FILE),
            json!({
                "served_profiles": ["default", "work"],
                "platforms": {"work:kuzz": {"state": "connected"}},
            })
            .to_string(),
        )
        .unwrap();
        let view = gateway_view(root.path(), "work").unwrap();
        assert_eq!(view.kuzz.as_deref(), Some("connected"));
        // Nothing serves it and it has no gateway of its own.
        let other = gateway_view(root.path(), "other").unwrap();
        assert!(!other.running);
        assert_eq!(other.kuzz, None);
    }

    #[test]
    fn a_settled_profile_can_settle_again() {
        drop(Settling("a".into()));
        assert!(!is_settling("a"));
        assert!(SETTLING.lock().unwrap().insert("a".into()));
        assert!(is_settling("a"));
        drop(Settling("a".into()));
        assert!(!is_settling("a"));
    }

    #[test]
    fn idle_needs_no_wait() {
        let home = tempfile::tempdir().unwrap();
        assert!(wait_for_idle(home.path()));
    }
}
