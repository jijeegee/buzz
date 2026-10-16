fn main() {
    adopt_legacy_env();
    if let Err(e) = kuzz_agent::run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Copies former `KUZZ_*` variables to their `KUZZ_*` names (see
/// `kuzz_core::legacy_env`; this crate does not depend on kuzz-core).
fn adopt_legacy_env() {
    let pending: Vec<(String, std::ffi::OsString)> = std::env::vars_os()
        .filter_map(|(key, value)| {
            let rest = key.to_str()?.strip_prefix("BUZZ_")?; // kuzz-rename: keep
            let current = format!("KUZZ_{rest}");
            std::env::var_os(&current).is_none().then_some((current, value))
        })
        .collect();
    for (key, value) in pending {
        std::env::set_var(key, value);
    }
}
