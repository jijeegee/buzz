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
    let args = hermes_pair_args("wss://gateway.kuzz.app", "7QKF-M2XD-9RTB");
    let expected: Vec<OsString> = [
        "pair",
        "--hermes",
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
        "Usage: kuzz pair <CODE>\n      --hermes  Then install the Hermes plugin"
    ));
    assert!(lists_hermes_flag("Usage: kuzz pair [--hermes] <CODE>"));
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
