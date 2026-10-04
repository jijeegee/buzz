use std::collections::BTreeMap;

use super::*;

const PUBKEY: &str = "3BF0C63FCB93463407AF97A5E5EE64FA883D107EF9E558472C4EB9AAAEFA459D";

fn profile_in(root: &Path) -> HermesProfile {
    HermesProfile::under_root(PUBKEY, root.to_path_buf()).expect("profile")
}

fn no_cli() -> Option<PathBuf> {
    None
}

fn write_ready_profile(profile: &HermesProfile) {
    std::fs::create_dir_all(&profile.dir).expect("mkdir");
    std::fs::write(profile.dir.join("config.yaml"), "model: {}\n").expect("write");
    std::fs::write(profile.dir.join(".env"), "").expect("write");
}

// ── Fake `hermes` CLI ─────────────────────────────────────────────────────────
//
// A script standing in for the real CLI so the create/delete wiring (argv,
// `HERMES_HOME`, exit-status handling, marker verification) runs for real.
// `profile create <name> ...` puts the name in the third argument.

/// Body that creates the `SOUL.md` marker for the profile named in `$3`.
#[cfg(windows)]
const CREATE_MARKER: &str =
    "mkdir \"%HERMES_HOME%\\profiles\\%3\"\r\necho soul> \"%HERMES_HOME%\\profiles\\%3\\SOUL.md\"\r\n";
#[cfg(not(windows))]
const CREATE_MARKER: &str =
    "mkdir -p \"$HERMES_HOME/profiles/$3\"\necho soul > \"$HERMES_HOME/profiles/$3/SOUL.md\"\n";

/// Body that records its argv under the root, so a test can prove the CLI ran.
#[cfg(windows)]
const RECORD_CALL: &str = "echo %*> \"%HERMES_HOME%\\called.txt\"\r\n";
#[cfg(not(windows))]
const RECORD_CALL: &str = "echo \"$@\" > \"$HERMES_HOME/called.txt\"\n";

#[cfg(windows)]
fn fake_cli(dir: &Path, name: &str, body: &str, exit_code: u8) -> PathBuf {
    let path = dir.join(format!("{name}.cmd"));
    std::fs::write(&path, format!("@echo off\r\n{body}exit /b {exit_code}\r\n")).expect("write");
    path
}

#[cfg(not(windows))]
fn fake_cli(dir: &Path, name: &str, body: &str, exit_code: u8) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}exit {exit_code}\n")).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

// ── Names, roots, identities ─────────────────────────────────────────────────

#[test]
fn profile_name_is_prefixed_lowercase_sixteen_hex() {
    let name = profile_name(PUBKEY).expect("hex pubkey");
    assert_eq!(name, "buzz-3bf0c63fcb934634");
    assert_eq!(name.len(), PROFILE_PREFIX.len() + PROFILE_PUBKEY_CHARS);
}

#[test]
fn profile_name_satisfies_hermes_name_rule() {
    // Hermes: ^[a-z0-9][a-z0-9_-]{0,63}$
    let name = profile_name(PUBKEY).expect("hex pubkey");
    let mut chars = name.chars();
    let first = chars.next().expect("non-empty");
    assert!(first.is_ascii_lowercase() || first.is_ascii_digit());
    assert!(chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'));
    assert!(name.len() <= 64);
    assert!(!["hermes", "default", "test", "tmp", "root", "sudo"].contains(&name.as_str()));
}

#[test]
fn profile_name_rejects_non_hex_or_short_pubkeys() {
    assert_eq!(profile_name("abc"), None);
    assert_eq!(profile_name(""), None);
    assert_eq!(profile_name("../../../../etc/x"), None);
    assert_eq!(profile_name("zzzzzzzzzzzzzzzzzzzz"), None);
    // Multi-byte input must not panic on a char-boundary slice.
    assert_eq!(profile_name("ééééééééééééééééé"), None);
}

#[test]
fn profile_dir_follows_hermes_layout() {
    let root = PathBuf::from("hermes-root");
    assert_eq!(
        profile_dir(&root, "buzz-0123456789abcdef"),
        root.join("profiles").join("buzz-0123456789abcdef")
    );
}

#[test]
fn root_prefers_a_custom_hermes_home_outside_the_native_home() {
    let native = Some(PathBuf::from("native").join("hermes"));
    let custom = PathBuf::from("custom").join("hermes");
    assert_eq!(
        resolve_hermes_root(Some(custom.as_os_str()), native),
        Some(custom)
    );
}

#[test]
fn root_unwraps_a_custom_hermes_home_that_is_a_profile() {
    let root = PathBuf::from("custom").join("hermes");
    let profile = root.join("profiles").join("work");
    let native = Some(PathBuf::from("native").join("hermes"));
    assert_eq!(
        resolve_hermes_root(Some(profile.as_os_str()), native),
        Some(root)
    );
}

#[test]
fn root_is_the_native_home_for_any_hermes_home_under_it() {
    // Hermes's get_default_hermes_root: anything under the native home (the
    // home itself, one of its profiles, or an unrelated subdirectory such as
    // `<native>/alt`) resolves to the native home, so Buzz must too.
    let native = PathBuf::from("native").join("hermes");
    for home in [
        native.clone(),
        native.join("profiles").join("work"),
        native.join("alt"),
    ] {
        assert_eq!(
            resolve_hermes_root(Some(home.as_os_str()), Some(native.clone())),
            Some(native.clone()),
            "{}",
            home.display()
        );
    }
}

#[test]
fn root_compares_resolved_paths_when_they_exist() {
    let temp = tempfile::tempdir().expect("tempdir");
    let native = temp.path().join("hermes");
    let custom = temp.path().join("custom");
    std::fs::create_dir_all(&native).expect("mkdir");
    std::fs::create_dir_all(&custom).expect("mkdir");
    // `<native>/../custom` starts with `<native>` lexically but resolves
    // outside it: a custom root, exactly as Hermes's `Path.resolve()` sees it.
    let escaping = native.join("..").join("custom");
    let resolved = resolve_hermes_root(Some(escaping.as_os_str()), Some(native.clone()));
    assert_eq!(resolved, Some(escaping));
}

#[test]
fn root_falls_back_to_the_native_home_when_unset_or_empty() {
    let native = Some(PathBuf::from("native-root"));
    assert_eq!(resolve_hermes_root(None, native.clone()), native);
    assert_eq!(
        resolve_hermes_root(Some(OsStr::new("")), native.clone()),
        native
    );
    assert_eq!(resolve_hermes_root(None, None), None);
}

#[test]
fn hermes_commands_are_recognised_across_spellings() {
    for command in [
        "hermes-acp",
        "HERMES-ACP.EXE",
        r"C:\Users\test\AppData\Local\hermes\bin\hermes-acp.exe",
        r"C:\Users\test\AppData\Roaming\npm\hermes-acp.cmd",
        "/opt/hermes/bin/hermes-acp",
        "hermes",
        "hermes-agent",
    ] {
        assert!(is_hermes_command(command), "{command} should be Hermes");
    }
}

#[test]
fn non_hermes_commands_are_not_recognised() {
    for command in [
        "claude-agent-acp",
        "goose",
        "codex-acp",
        "openclaw",
        "hermes-acp-wrapper",
        "my-hermes-acp",
        "",
    ] {
        assert!(!is_hermes_command(command), "{command} must not be Hermes");
    }
}

#[test]
fn deletion_guard_accepts_only_exact_buzz_profile_dirs() {
    let root = PathBuf::from("hermes-root");
    let profiles = root.join("profiles");
    assert!(is_buzz_profile_dir(
        &root,
        &profiles.join("buzz-0123456789abcdef")
    ));
    for rejected in [
        root.clone(),
        profiles.clone(),
        profiles.join("work"),
        profiles.join("buzz-"),
        profiles.join("buzz-0123456789ABCDEF"),
        profiles.join("buzz-0123456789abcdef0"),
        profiles.join("buzz-0123456789abcdeg"),
        root.join("buzz-0123456789abcdef"),
        PathBuf::from("elsewhere")
            .join("profiles")
            .join("buzz-0123456789abcdef"),
    ] {
        assert!(
            !is_buzz_profile_dir(&root, &rejected),
            "{} must not be deletable",
            rejected.display()
        );
    }
}

// ── Which starts need a profile ───────────────────────────────────────────────

fn record_on(runtime: Option<&str>) -> ManagedAgentRecord {
    let mut record = serde_json::from_value::<ManagedAgentRecord>(serde_json::json!({
        "pubkey": PUBKEY.to_ascii_lowercase(),
        "name": "agent",
        "persona_id": "persona-1",
        "relay_url": "ws://localhost:3000",
        "acp_command": "buzz-acp",
        "agent_command": "",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 320,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
    }))
    .expect("record");
    record.runtime = runtime.map(str::to_string);
    record
}

fn persona_on(runtime: &str) -> super::super::AgentDefinition {
    serde_json::from_value(serde_json::json!({
        "id": "persona-1",
        "display_name": "Persona",
        "system_prompt": "",
        "runtime": runtime,
        "is_active": true,
        "created_at": "",
        "updated_at": "",
    }))
    .expect("persona")
}

fn hermes_for(record: &ManagedAgentRecord, persona: &str) -> Option<String> {
    hermes_command_for_start(
        record,
        &[persona_on(persona)],
        &super::super::GlobalAgentConfig::default(),
    )
}

#[test]
fn a_fresh_hermes_persona_instance_needs_a_profile() {
    // `record.runtime` is None on create; the persona supplies the harness.
    assert_eq!(
        hermes_for(&record_on(None), "hermes").as_deref(),
        Some("hermes-acp")
    );
    assert_eq!(hermes_for(&record_on(None), "goose"), None);
}

#[test]
fn a_persona_switched_to_hermes_needs_a_profile_before_its_resnapshot() {
    // The stored record still says goose; a caller that re-snapshots before
    // spawning will launch Hermes, so the profile must exist already.
    assert_eq!(
        hermes_for(&record_on(Some("goose")), "hermes").as_deref(),
        Some("hermes-acp")
    );
}

#[test]
fn a_stale_hermes_record_needs_a_profile_without_a_resnapshot() {
    // A caller that does not re-snapshot spawns the stored Hermes runtime even
    // though the persona has moved on.
    assert_eq!(
        hermes_for(&record_on(Some("hermes")), "goose").as_deref(),
        Some("hermes-acp")
    );
}

// ── Spawn env ordering (the production seam) ─────────────────────────────────

#[test]
fn hermes_home_is_written_after_the_user_env_layer() {
    let profile = profile_in(Path::new("hermes-root"));
    let mut command = Command::new("buzz-acp");
    // A user-env value for HERMES_HOME — reserved, so normally stripped
    // upstream; this binds the second line of defence, the write order.
    let user_env = BTreeMap::from([
        (HERMES_HOME_ENV_VAR.to_string(), "shared-root".to_string()),
        ("OTHER".to_string(), "kept".to_string()),
    ]);
    apply_user_env_then_hermes_home(&mut command, &user_env, Some(&profile));
    let env: BTreeMap<_, _> = command
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        env.get(HERMES_HOME_ENV_VAR).cloned().flatten(),
        Some(profile.dir.to_string_lossy().into_owned())
    );
    assert_eq!(
        env.get("OTHER").cloned().flatten(),
        Some("kept".to_string())
    );
}

#[test]
fn non_hermes_spawn_leaves_hermes_home_to_the_user_env() {
    let mut command = Command::new("buzz-acp");
    let user_env = BTreeMap::from([("OTHER".to_string(), "kept".to_string())]);
    apply_user_env_then_hermes_home(&mut command, &user_env, None);
    assert!(command
        .get_envs()
        .all(|(key, _)| key != OsStr::new(HERMES_HOME_ENV_VAR)));
    assert_eq!(require_for_spawn("claude-agent-acp", PUBKEY, "a"), Ok(None));
}

// ── Readiness (what the locked spawn path checks) ─────────────────────────────

#[test]
fn a_profile_without_its_own_env_file_is_not_ready() {
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    std::fs::create_dir_all(&profile.dir).expect("mkdir");
    std::fs::write(profile.dir.join("SOUL.md"), "soul").expect("write");

    let error = check_ready(&profile).expect_err("no .env");
    assert!(error.contains(".env"), "{error}");

    std::fs::write(profile.dir.join(".env"), "").expect("write");
    assert_eq!(check_ready(&profile), Ok(()));
}

#[test]
fn a_missing_profile_is_not_ready() {
    let root = tempfile::tempdir().expect("tempdir");
    assert!(check_ready(&profile_in(root.path())).is_err());
}

// ── ensure_profile ────────────────────────────────────────────────────────────

#[test]
fn ensure_profile_skips_the_cli_for_an_existing_profile() {
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    write_ready_profile(&profile);
    let ensured = ensure_profile(
        &profile,
        || panic!("an existing profile must not resolve the CLI"),
        "Buzz agent: a",
    );
    assert_eq!(ensured, Ok(()));
}

#[test]
fn ensure_profile_adds_a_missing_env_file_without_overwriting_one() {
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    std::fs::create_dir_all(&profile.dir).expect("mkdir");
    std::fs::write(profile.dir.join("config.yaml"), "model: {}\n").expect("write");

    // The source home has a .env, but an EXISTING profile is never re-seeded:
    // it only gets a placeholder.
    std::fs::write(root.path().join(".env"), "OPENROUTER_API_KEY=root\n").expect("write");
    assert_eq!(ensure_profile(&profile, no_cli, "Buzz agent: a"), Ok(()));
    let env_path = profile.dir.join(".env");
    assert_eq!(
        std::fs::read_to_string(&env_path).expect("read"),
        PROFILE_ENV_HEADER
    );

    std::fs::write(&env_path, "OPENAI_API_KEY=agent-own\n").expect("write");
    assert_eq!(ensure_profile(&profile, no_cli, "Buzz agent: a"), Ok(()));
    assert_eq!(
        std::fs::read_to_string(&env_path).expect("read"),
        "OPENAI_API_KEY=agent-own\n"
    );
}

#[test]
fn ensure_profile_fails_closed_without_the_cli() {
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    let error = ensure_profile(&profile, no_cli, "Buzz agent: a").expect_err("must refuse");
    assert!(error.contains("`hermes` command was not found"), "{error}");
    assert!(!profile.dir.exists());
}

#[test]
fn ensure_profile_fails_closed_when_the_cli_cannot_run() {
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    let missing = root.path().join("no-such-hermes");
    let error = ensure_profile(&profile, || Some(missing), "Buzz agent: a").expect_err("refuse");
    assert!(error.contains("hermes profile create"), "{error}");
}

#[test]
fn ensure_profile_accepts_a_failed_run_that_left_the_profile_behind() {
    let bin = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    let hermes = fake_cli(bin.path(), "hermes", CREATE_MARKER, 1);

    assert_eq!(
        ensure_profile(&profile, || Some(hermes), "Buzz agent: a"),
        Ok(())
    );
    assert!(profile.dir.join("SOUL.md").is_file());
    assert!(
        profile.dir.join(".env").is_file(),
        "Buzz must give the profile its own .env"
    );
}

#[test]
fn ensure_profile_rejects_a_successful_run_that_created_nothing() {
    let bin = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    let hermes = fake_cli(bin.path(), "hermes", "", 0);

    let error = ensure_profile(&profile, || Some(hermes), "Buzz agent: a").expect_err("refuse");
    assert!(error.contains("not created"), "{error}");
    assert!(!profile.dir.exists(), "no .env may be written into a void");
}

#[test]
fn ensure_profile_reports_cli_failure_output() {
    let bin = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    let hermes = fake_cli(bin.path(), "hermes", "echo boom-from-hermes\n", 3);

    let error = ensure_profile(&profile, || Some(hermes), "Buzz agent: a").expect_err("refuse");
    assert!(error.contains("boom-from-hermes"), "{error}");
}

// ── .env seeding (B2) ─────────────────────────────────────────────────────────

const SOURCE_ENV: &str = "\
# root secrets
BUZZ_PRIVATE_KEY=nsec1shared
buzz_relay_url=wss://relay.example
export BUZZ_AUTH_TAG=tag
HERMES_HOME=/elsewhere
NOSTR_PRIVATE_KEY=nsec1other
GIT_CONFIG_COUNT=0
OPENROUTER_API_KEY=sk-or-123
QUOTED=\"a=b c\"
export ANTHROPIC_API_KEY='sk-ant=x'
  PADDED = keep me
NO_EQUALS_LINE
MULTI=\"line one
line two\"
BUZZ_MULTI=\"secret
still secret\"
LAST=1
";

#[test]
fn strip_removes_every_buzz_owned_key() {
    let kept = strip_buzz_owned_env(SOURCE_ENV);
    for removed in [
        "BUZZ_PRIVATE_KEY",
        "buzz_relay_url",
        "BUZZ_AUTH_TAG",
        "HERMES_HOME",
        "NOSTR_PRIVATE_KEY",
        "GIT_CONFIG_COUNT",
        "BUZZ_MULTI",
        "still secret",
        "nsec1",
        "root secrets",
        "NO_EQUALS_LINE",
    ] {
        assert!(
            !kept.contains(removed),
            "{removed} must be stripped:\n{kept}"
        );
    }
}

#[test]
fn strip_keeps_other_assignments_verbatim() {
    assert_eq!(
        strip_buzz_owned_env(SOURCE_ENV),
        "\
OPENROUTER_API_KEY=sk-or-123
QUOTED=\"a=b c\"
export ANTHROPIC_API_KEY='sk-ant=x'
  PADDED = keep me
MULTI=\"line one
line two\"
LAST=1
"
    );
}

#[test]
fn strip_handles_crlf_and_escaped_quotes() {
    let kept = strip_buzz_owned_env(
        "A=\"x \\\" y\"\r\nBUZZ_PRIVATE_KEY=nsec1\r\nexportX=1\r\nexport\tB=2\r\n",
    );
    assert_eq!(kept, "A=\"x \\\" y\"\nexportX=1\nexport\tB=2\n");
}

fn created_by_fake_cli(source_env: Option<&str>) -> (tempfile::TempDir, HermesProfile) {
    let bin = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("tempdir");
    if let Some(contents) = source_env {
        std::fs::write(root.path().join(".env"), contents).expect("write");
    }
    let profile = profile_in(root.path());
    // Like the real CLI, the fake drops its own placeholder .env.
    #[cfg(windows)]
    let body =
        format!("{CREATE_MARKER}echo # placeholder> \"%HERMES_HOME%\\profiles\\%3\\.env\"\r\n");
    #[cfg(not(windows))]
    let body = format!("{CREATE_MARKER}echo '# placeholder' > \"$HERMES_HOME/profiles/$3/.env\"\n");
    let hermes = fake_cli(bin.path(), "hermes", &body, 0);
    assert_eq!(
        ensure_profile(&profile, || Some(hermes), "Buzz agent: a"),
        Ok(())
    );
    (root, profile)
}

#[test]
fn a_new_profile_env_is_seeded_from_the_source_home_minus_buzz_keys() {
    let (_root, profile) = created_by_fake_cli(Some(SOURCE_ENV));
    let env = std::fs::read_to_string(profile.dir.join(".env")).expect("read");
    assert!(env.starts_with(PROFILE_ENV_HEADER), "{env}");
    assert!(env.contains("OPENROUTER_API_KEY=sk-or-123\n"), "{env}");
    assert!(!env.contains("BUZZ_PRIVATE_KEY"), "{env}");
    assert!(
        !env.contains("placeholder"),
        "Hermes's placeholder is replaced"
    );
    assert!(!profile.dir.join(".env.buzz-seed").exists());
    assert_eq!(check_ready(&profile), Ok(()));
}

#[test]
fn a_new_profile_without_a_source_env_gets_the_placeholder() {
    let (_root, profile) = created_by_fake_cli(None);
    let env = std::fs::read_to_string(profile.dir.join(".env")).expect("read");
    assert_eq!(env, PROFILE_ENV_HEADER);
}

#[test]
fn a_desktop_hermes_home_is_the_seed_source() {
    let source = tempfile::tempdir().expect("tempdir");
    std::fs::write(source.path().join(".env"), "FROM_PROFILE=1\nBUZZ_X=2\n").expect("write");
    let root = tempfile::tempdir().expect("tempdir");
    let mut profile = profile_in(root.path());
    profile.source_home = source.path().to_path_buf();
    std::fs::create_dir_all(&profile.dir).expect("mkdir");
    seed_profile_env(&profile).expect("seed");
    assert_eq!(
        std::fs::read_to_string(profile.dir.join(".env")).expect("read"),
        format!("{PROFILE_ENV_HEADER}FROM_PROFILE=1\n")
    );
}

#[test]
fn an_existing_profile_env_is_never_reseeded_or_overwritten() {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join(".env"), "OPENROUTER_API_KEY=root\n").expect("write");
    let profile = profile_in(root.path());
    std::fs::create_dir_all(&profile.dir).expect("mkdir");
    std::fs::write(profile.dir.join("SOUL.md"), "soul").expect("write");
    std::fs::write(profile.dir.join(".env"), "MY_EDIT=1\n").expect("write");

    assert_eq!(
        ensure_profile(&profile, || panic!("no CLI for an existing profile"), "x"),
        Ok(())
    );
    assert_eq!(
        std::fs::read_to_string(profile.dir.join(".env")).expect("read"),
        "MY_EDIT=1\n"
    );
}

#[test]
fn source_home_defaults_to_the_root() {
    let profile = profile_in(Path::new("hermes-root"));
    assert_eq!(profile.source_home, PathBuf::from("hermes-root"));
}

// ── CLI lookup ────────────────────────────────────────────────────────────────

#[test]
fn hermes_cli_is_found_next_to_the_agent_command() {
    let bin = tempfile::tempdir().expect("tempdir");
    let agent = fake_cli(bin.path(), "hermes-acp", "", 0);
    #[cfg(windows)]
    let names = ["hermes.exe", "hermes.cmd"];
    #[cfg(not(windows))]
    let names = ["hermes"];
    for name in names {
        let sibling = bin.path().join(name);
        std::fs::write(&sibling, "").expect("write");
        assert_eq!(
            find_hermes_cli(&agent.to_string_lossy()),
            Some(sibling.clone()),
            "{name}"
        );
        std::fs::remove_file(&sibling).expect("remove");
    }
}

// ── Deletion ──────────────────────────────────────────────────────────────────

#[test]
fn delete_profile_runs_the_cli_then_removes_what_it_left() {
    let bin = tempfile::tempdir().expect("tempdir");
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    write_ready_profile(&profile);
    let sibling = root.path().join("profiles").join("work");
    std::fs::create_dir_all(&sibling).expect("mkdir");
    // The fake CLI records its call and fails without deleting anything.
    let hermes = fake_cli(bin.path(), "hermes", RECORD_CALL, 1);

    delete_profile(&profile, || Some(hermes));

    let called = std::fs::read_to_string(root.path().join("called.txt")).expect("CLI invoked");
    assert!(
        called.contains("profile delete buzz-3bf0c63fcb934634 -y"),
        "{called}"
    );
    assert!(!profile.dir.exists(), "fallback must remove the profile");
    assert!(sibling.exists(), "other profiles must survive");
}

#[test]
fn delete_profile_removes_the_dir_without_the_cli() {
    let root = tempfile::tempdir().expect("tempdir");
    let profile = profile_in(root.path());
    std::fs::create_dir_all(profile.dir.join("memories")).expect("mkdir");

    delete_profile(&profile, no_cli);

    assert!(!profile.dir.exists());
}

#[test]
fn delete_profile_refuses_a_dir_outside_the_guard_and_never_runs_the_cli() {
    let root = tempfile::tempdir().expect("tempdir");
    let profile = HermesProfile {
        name: "work".to_string(),
        root: root.path().to_path_buf(),
        dir: root.path().join("profiles").join("work"),
        source_home: root.path().to_path_buf(),
    };
    std::fs::create_dir_all(&profile.dir).expect("mkdir");

    delete_profile(&profile, || panic!("guarded delete must not run the CLI"));

    assert!(profile.dir.exists());
}

#[test]
fn delete_profile_of_a_missing_profile_does_nothing() {
    let root = tempfile::tempdir().expect("tempdir");
    delete_profile(&profile_in(root.path()), || {
        panic!("no profile, no CLI lookup")
    });
}

// ── Wiring: every start and removal path goes through this module ────────────
//
// The lifecycle commands need a full Tauri app to drive end to end, so these
// pin the call sites at source level: dropping one fails here.

#[test]
fn every_start_entry_point_prepares_the_profile_before_its_locks() {
    for (file, source, lock) in [
        (
            "commands/agents.rs",
            include_str!("../commands/agents.rs"),
            ".managed_agent_runtime_transition",
        ),
        (
            "managed_agents/runtime_commands.rs",
            include_str!("runtime_commands.rs"),
            ".managed_agent_runtime_transition",
        ),
        (
            "managed_agents/restore.rs",
            include_str!("restore.rs"),
            "spawn_and_register_restored_agents(\n",
        ),
    ] {
        let prepare = source
            .find("hermes_profile::prepare_for_start(")
            .unwrap_or_else(|| panic!("{file} must call prepare_for_start"));
        assert!(
            source[prepare..].contains(lock),
            "{file}: {lock} must follow prepare_for_start"
        );
    }
}

#[test]
fn spawn_checks_the_profile_and_writes_env_through_the_ordered_seam() {
    let source = include_str!("runtime.rs");
    assert!(source.contains("hermes_profile::require_for_spawn("));
    assert!(source.contains("hermes_profile::apply_user_env_then_hermes_home("));
    assert!(
        !source.contains("in &descriptor.env {"),
        "user env must be written only through apply_user_env_then_hermes_home"
    );
    assert!(
        !source.contains("prepare_for_start("),
        "the locked spawn path must not create profiles"
    );
}

#[test]
fn every_agent_removal_path_cleans_up_the_profile() {
    for (file, source) in [
        ("commands/agents.rs", include_str!("../commands/agents.rs")),
        (
            "commands/personas/mod.rs",
            include_str!("../commands/personas/mod.rs"),
        ),
    ] {
        assert!(
            source.contains("hermes_profile::delete_profile_for_deleted_agent("),
            "{file} must clean up deleted agents' Hermes profiles"
        );
    }
}

#[test]
fn description_is_flattened_to_one_line() {
    assert_eq!(
        sanitize_description("Buzz agent: a\nb\u{0}c "),
        "Buzz agent: a b c"
    );
}
