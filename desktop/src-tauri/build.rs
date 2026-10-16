// Shared schema, included from the same source the runtime command parses with,
// so the build-time validation below and the runtime parse cannot drift.
include!("src/commands/reconnect_hook_config.rs");
// Same source of truth the runtime filters with, so a baked build env cannot
// carry a reserved key the runtime believes it already rejected.
include!("src/managed_agents/reserved_env_keys.rs");

use base64::Engine as _;

fn main() {
    println!("cargo:rerun-if-env-changed=KUZZ_ENV");
    println!("cargo:rerun-if-env-changed=KUZZ_RELAY_URL");
    println!("cargo:rerun-if-env-changed=KUZZ_RELAY_HTTP");
    println!("cargo:rerun-if-env-changed=KUZZ_UPDATER_PUBLIC_KEY");
    println!("cargo:rerun-if-env-changed=KUZZ_UPDATER_ENDPOINT");
    println!("cargo:rerun-if-env-changed=KUZZ_BUILD_KUZZ_AGENT_PROVIDER");
    println!("cargo:rerun-if-env-changed=KUZZ_BUILD_KUZZ_AGENT_MODEL");
    println!("cargo:rerun-if-env-changed=KUZZ_BUILD_AGENT_ENV");
    println!("cargo:rerun-if-env-changed=KUZZ_BUILD_RELAY_RECONNECT_CMD");
    println!("cargo:rerun-if-env-changed=KUZZ_BUILD_AGENT_ACCESS_OWNER_ONLY");
    println!("cargo:rerun-if-env-changed=KUZZ_BUILD_AUTO_CONNECT_DEFAULT_RELAY");
    println!("cargo:rerun-if-env-changed=KUZZ_BUILD_DEMO_SLUG");
    println!("cargo:rustc-check-cfg=cfg(kuzz_updater_enabled)");

    if let Ok(slug) = std::env::var("KUZZ_BUILD_DEMO_SLUG") {
        let valid = !slug.is_empty()
            && slug.len() <= 48
            && slug
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && slug
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            && slug
                .bytes()
                .last()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
        if !valid {
            panic!("KUZZ_BUILD_DEMO_SLUG must be a lowercase ASCII slug");
        }
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_DEMO_SLUG={slug}");
    }

    // Explicit owner-only agent-access capability. Release packaging sets this
    // presence-only marker; OSS/custom builds leave agent access configurable.
    if std::env::var("KUZZ_BUILD_AGENT_ACCESS_OWNER_ONLY").is_ok() {
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_AGENT_ACCESS_OWNER_ONLY=1");
    }

    // Kuzz server environment (see `relay::KuzzEnv`); a typo must not
    // silently fall back to another server.
    let kuzz_env = std::env::var("KUZZ_ENV").unwrap_or_default();
    match kuzz_env.trim() {
        "" | "prod" => println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_ENV=prod"),
        "dev" => println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_ENV=dev"),
        other => panic!("KUZZ_ENV must be `prod` or `dev`, got `{other}`"),
    }

    if let Ok(relay_url) = std::env::var("KUZZ_RELAY_URL") {
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_RELAY_URL={relay_url}");
    }

    if let Ok(relay_http) = std::env::var("KUZZ_RELAY_HTTP") {
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_RELAY_HTTP={relay_http}");
    }

    if let Ok(provider) = std::env::var("KUZZ_BUILD_KUZZ_AGENT_PROVIDER") {
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_KUZZ_AGENT_PROVIDER={provider}");
    }

    if let Ok(model) = std::env::var("KUZZ_BUILD_KUZZ_AGENT_MODEL") {
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_KUZZ_AGENT_MODEL={model}");
    }

    // Generic KEY=VALUE pairs to inject into every spawned agent process.
    // Newline-delimited; each line must be non-empty and contain exactly one
    // `=` separator with a non-empty key.  OSS builds leave this unset.
    // The validated value is base64-encoded before emitting so the single-line
    // Cargo build-script output carries all pairs (Cargo output is line-oriented;
    // a raw multiline value would be silently truncated to the first line).
    if let Ok(raw) = std::env::var("KUZZ_BUILD_AGENT_ENV") {
        for (line_no, line) in raw.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let eq = line.find('=').unwrap_or_else(|| {
                panic!(
                    "KUZZ_BUILD_AGENT_ENV line {}: missing '=' separator in {:?}",
                    line_no + 1,
                    line
                )
            });
            let key = &line[..eq];
            if key.is_empty() {
                panic!(
                    "KUZZ_BUILD_AGENT_ENV line {}: key must not be empty in {:?}",
                    line_no + 1,
                    line
                );
            }
            // The baked env is written into every spawned agent's environment
            // LAST (see `managed_agents/runtime.rs`), after Kuzz sets the
            // access gates and identity vars. A baked reserved key would
            // therefore silently override the gate the UI promises, so reject
            // it at build time instead of shipping a binary that bypasses its
            // own enforcement.
            if is_reserved_env_key(key) {
                panic!(
                    "KUZZ_BUILD_AGENT_ENV line {}: `{}` is reserved by Kuzz and cannot be baked \
                     into a build (it would override Kuzz's own identity/access env)",
                    line_no + 1,
                    key
                );
            }
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_AGENT_ENV={encoded}");
    }

    if let Ok(val) = std::env::var("KUZZ_BUILD_RELAY_RECONNECT_CMD") {
        let parsed: serde_json::Value = serde_json::from_str(&val)
            .unwrap_or_else(|e| panic!("KUZZ_BUILD_RELAY_RECONNECT_CMD is not valid JSON: {e}"));
        serde_json::from_value::<ReconnectHookConfig>(parsed).unwrap_or_else(|e| {
            panic!("KUZZ_BUILD_RELAY_RECONNECT_CMD doesn't match ReconnectHookConfig: {e}")
        });
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_RELAY_RECONNECT_CMD={val}");
    }

    // Presence-only release capability: internal desktop builds opt into
    // auto-connecting their configured default relay on first run. OSS builds
    // leave this unset and retain explicit community selection.
    if std::env::var("KUZZ_BUILD_AUTO_CONNECT_DEFAULT_RELAY").is_ok() {
        println!("cargo:rustc-env=KUZZ_DESKTOP_BUILD_AUTO_CONNECT_DEFAULT_RELAY=1");
    }

    let updater_public_key = std::env::var("KUZZ_UPDATER_PUBLIC_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let updater_endpoint = std::env::var("KUZZ_UPDATER_ENDPOINT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if updater_public_key.is_some() && updater_endpoint.is_some() {
        println!("cargo:rustc-cfg=kuzz_updater_enabled");
    }

    // Cargo test executables get no embedded Windows manifest (tauri_build
    // attaches one to bin targets only), so the loader binds comctl32 v5, which
    // lacks TaskDialogIndirect (statically imported via tauri-plugin-dialog/rfd)
    // and debug test exes die at load with STATUS_ENTRYPOINT_NOT_FOUND. Declaring
    // the Common Controls v6 dependency makes link.exe emit a side-by-side
    // <exe>.manifest that the loader honors for manifest-less executables;
    // binaries with an embedded manifest (the real app) ignore it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }

    tauri_build::try_build(
        tauri_build::Attributes::new().plugin(
            "websocket",
            tauri_build::InlinedPlugin::new()
                .commands(&["connect", "send", "disconnect", "disconnect_all"])
                .default_permission(tauri_build::DefaultPermissionRule::AllowAllCommands),
        ),
    )
    .expect("failed to build Tauri application");
}
