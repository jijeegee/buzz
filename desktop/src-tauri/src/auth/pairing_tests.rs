use super::*;

fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| OsString::from(value))
    }
}

#[cfg(windows)]
const HOME: &str = r"C:\Users\u\hermes-home";
#[cfg(not(windows))]
const HOME: &str = "/home/u/hermes-home";
#[cfg(windows)]
const LOCAL: &str = r"C:\Users\u\AppData\Local";
#[cfg(not(windows))]
const LOCAL: &str = "/home/u/local";
#[cfg(windows)]
const USER: &str = r"C:\Users\u";
#[cfg(not(windows))]
const USER: &str = "/home/u";

/// Where Hermes is looked for when `HERMES_HOME` is not set.
fn default_home() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(LOCAL).join("hermes")
    } else {
        PathBuf::from(USER).join(".hermes")
    }
}

#[test]
fn hermes_pairing_runs_exactly_this_argv() {
    let args = hermes_pair_args("wss://gateway.kuzz.app", "7QKF-M2XD-9RTB", None);
    let expected: Vec<OsString> = [
        "pair",
        "--hermes",
        "--no-restart",
        "--relay",
        "wss://gateway.kuzz.app",
        "7QKF-M2XD-9RTB",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    assert_eq!(args, expected);
}

#[test]
fn hermes_is_found_by_hermes_home_first() {
    let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
    let found = hermes_home_from(&var, Some(Path::new(USER)), |_| true);
    assert_eq!(found, Some(PathBuf::from(HOME)));
}

#[test]
fn hermes_is_found_in_its_default_home() {
    let var = vars(&[("LOCALAPPDATA", LOCAL)]);
    let expected = default_home();
    let found = hermes_home_from(&var, Some(Path::new(USER)), |path| path == expected);
    assert_eq!(found, Some(default_home()));
}

#[test]
fn hermes_home_that_does_not_exist_falls_back() {
    let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
    let expected = default_home();
    let found = hermes_home_from(&var, Some(Path::new(USER)), |path| path == expected);
    assert_eq!(found, Some(default_home()));
}

#[test]
fn no_hermes_without_folders_or_with_relative_paths() {
    let var = vars(&[("HERMES_HOME", HOME), ("LOCALAPPDATA", LOCAL)]);
    assert_eq!(
        hermes_home_from(&var, Some(Path::new(USER)), |_| false),
        None
    );
    assert_eq!(hermes_home_from(vars(&[]), None, |_| true), None);
    let relative = vars(&[("HERMES_HOME", "hermes"), ("LOCALAPPDATA", "")]);
    assert_eq!(hermes_home_from(relative, None, |_| true), None);
}

#[test]
fn hermes_flag_is_read_from_pair_help() {
    assert!(lists_hermes_flag(
        "Usage: kuzz pair <CODE>\n      --hermes  Then install the Hermes plugin\n      --no-restart  With --hermes\n      --hermes-home <DIR>  The profile"
    ));
    assert!(lists_hermes_flag(
        "Usage: kuzz pair [--hermes] [--no-restart] [--hermes-home=DIR] <CODE>"
    ));
    // An older kuzz.
    assert!(!lists_hermes_flag("Usage: kuzz pair [--hermes] <CODE>"));
    assert!(!lists_hermes_flag(
        "Usage: kuzz pair [--hermes] [--no-restart] <CODE>"
    ));
    assert!(!lists_hermes_flag(
        "Usage: kuzz pair <CODE>\n  --hermes-home <DIR>"
    ));
    assert!(!lists_hermes_flag(""));
}

#[test]
fn plugin_roots_match_where_kuzz_looks() {
    let data = PathBuf::from(LOCAL);
    // The app's sidecar folder, then this user's data folder.
    let app = PathBuf::from(USER).join("Kuzz").join("kuzz");
    assert_eq!(
        hermes_plugin_roots(&app, Some(&data)),
        vec![PathBuf::from(USER).join("Kuzz"), data.join("kuzz")]
    );
    // A `<prefix>/bin/kuzz` also looks in `<prefix>/share/kuzz`.
    let prefix = PathBuf::from(USER).join("usr");
    let packaged = prefix.join("bin").join("kuzz");
    assert_eq!(
        hermes_plugin_roots(&packaged, None),
        vec![prefix.join("bin"), prefix.join("share").join("kuzz")]
    );
}

#[test]
fn the_old_hermes_key_file_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(LEGACY_HERMES_FILE);
    std::fs::write(&file, b"[]").unwrap();
    remove_legacy_hermes_file(dir.path());
    assert!(!file.exists());
    // Nothing to remove is fine.
    remove_legacy_hermes_file(dir.path());
}

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
fn a_named_profile_is_passed_by_its_folder() {
    let home = PathBuf::from(HOME).join("profiles").join("work");
    let args = hermes_pair_args("wss://r", "CODE", Some(&home));
    let expected: Vec<OsString> = [
        OsString::from("pair"),
        "--hermes".into(),
        "--no-restart".into(),
        "--hermes-home".into(),
        home.clone().into(),
        "--relay".into(),
        "wss://r".into(),
        "CODE".into(),
    ]
    .into_iter()
    .collect();
    assert_eq!(args, expected);
}

#[test]
fn only_profiles_a_person_made_are_offered() {
    for name in [
        "work",
        "exttest",
        "a_b-1",
        "kuzz-short",
        "buzz-2911eff5fdf2613",
    ] {
        assert!(is_person_profile(name), "{name}");
    }
    for name in [
        "default",
        "kuzz-2911eff5fdf2613a",
        "buzz-36cbc1babbc441c6",
        ".deleted",
        "-x",
        "Work",
        "",
    ] {
        assert!(!is_person_profile(name), "{name}");
    }
}

#[test]
fn profiles_are_read_each_with_its_own_pin_and_owner() {
    let root = tempfile::tempdir().unwrap();
    let (pin, owner) = ("a".repeat(64), "B".repeat(64));
    let profile = |name: &str, env: Option<String>| {
        let home = root.path().join("profiles").join(name);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.yaml"), b"").unwrap();
        if let Some(env) = env {
            std::fs::write(home.join(".env"), env).unwrap();
        }
    };
    profile(
        "work",
        Some(format!(
            "# c\nexport KUZZ_PAIRED_AGENT='{pin}'\nOTHER=1\nKUZZ_OWNER_PUBKEY={owner}\n"
        )),
    );
    profile("home", None);
    profile("kuzz-2911eff5fdf2613a", None);
    std::fs::create_dir_all(root.path().join("profiles").join("no-config")).unwrap();
    std::fs::write(root.path().join(".env"), "KUZZ_PAIRED_AGENT=nope\n").unwrap();

    let views = hermes_profiles(root.path());
    let names: Vec<&str> = views.iter().map(|view| view.name.as_str()).collect();
    assert_eq!(names, ["default", "home", "work"]);
    assert!(views[0].is_default);
    assert_eq!(views[0].paired_agent, None);
    assert_eq!(views[2].paired_agent.as_deref(), Some(pin.as_str()));
    assert_eq!(views[2].owner, Some(owner.to_ascii_lowercase()));
    assert_eq!(hermes_profile_target(root.path(), "default"), Some(None));
    assert_eq!(
        hermes_profile_target(root.path(), "work"),
        Some(Some(root.path().join("profiles").join("work")))
    );
    assert_eq!(
        hermes_profile_target(root.path(), "kuzz-2911eff5fdf2613a"),
        None
    );
    assert_eq!(hermes_profile_target(root.path(), "../x"), None);
}

#[test]
fn a_shared_bot_token_blocks_named_profiles_only() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("profiles").join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.yaml"), b"").unwrap();
    let state = |reason: &str| {
        std::fs::write(
            root.path().join("gateway_state.json"),
            serde_json::json!({ "multiplex_standalone_reason": reason }).to_string(),
        )
        .unwrap();
    };
    state("Profiles 'default' and 'work' both hold the same telegram credential");
    let views = hermes_profiles(root.path());
    assert_eq!(views[0].blocked_reason, None);
    assert!(views[1].blocked_reason.is_some());
    assert_eq!(hermes_profile_target(root.path(), "work"), None);
    assert_eq!(hermes_profile_target(root.path(), "default"), Some(None));
    // Cleared by the restart after Connect.
    state("only one profile exists (nothing to multiplex)");
    assert!(hermes_profile_target(root.path(), "work").is_some());
}
