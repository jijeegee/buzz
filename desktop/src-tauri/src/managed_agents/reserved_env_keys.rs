// Canonical reserved-env-key list, `include!`d into BOTH `build.rs`
// (compile-time rejection of baked `KUZZ_BUILD_AGENT_ENV` collisions) and
// `managed_agents/env_vars.rs` (save-time validation and spawn-time
// filtering). Build scripts cannot import from the crate, so sharing the
// source via `include!` is what guarantees the build-time check and the
// runtime filter use one identical list — zero drift surface. See
// `commands/reconnect_hook_config.rs` for the same pattern.
//
// Keep this file dependency-free: no crate-internal imports, no external
// crates. Both consumers compile it as-is.

/// Env var keys that Kuzz sets itself and users must not override from
/// the persona/agent env_vars UI. Three categories:
///
/// 1. **Identity / secrets** — overriding would swap the agent's nsec or
///    leak credentials.
/// 2. **Code-execution surface** — overriding the binary/args lets the
///    user run arbitrary code as the agent process.
/// 3. **Security gates** — overriding the respond-to mode/allowlist or
///    relay URL would silently break the saved security settings (the UI
///    shows owner-only while the running agent answers anyone, for
///    example), or redirect the agent to an attacker-controlled relay.
///
/// This list is deliberately narrow — it only covers keys with security
/// implications. Behavior knobs (GOOSE_MODE, KUZZ_ACP_MODEL, KUZZ_ACP_SYSTEM_PROMPT, …) remain freely
/// overridable; those have dedicated UI fields but power users may want
/// to bypass them.
pub(crate) const RESERVED_ENV_KEYS: &[&str] = &[
    // Identity / secrets.
    "KUZZ_PRIVATE_KEY",
    "NOSTR_PRIVATE_KEY",
    "KUZZ_AUTH_TAG",
    "KUZZ_API_TOKEN",
    "KUZZ_ACP_PRIVATE_KEY",
    "KUZZ_ACP_API_TOKEN",
    // Centralized-identity bearer credentials (token mode): Desktop injects
    // the bot token itself and kuzz-acp hands children only its loopback
    // broker. A saved value would swap the agent's identity or leak a token.
    "KUZZ_BOT_TOKEN",
    "KUZZ_BOT_TOKEN_EXPIRES_AT",
    "KUZZ_ACCESS_TOKEN",
    "KUZZ_TOKEN_BROKER_URL",
    "KUZZ_TOKEN_BROKER_SECRET",
    // Relay URL: overriding would let a malicious config redirect the
    // agent to an attacker-controlled relay.
    "KUZZ_RELAY_URL",
    // Code-execution surface: overriding would let the user run arbitrary
    // binaries/args as the agent process.
    "KUZZ_ACP_AGENT_COMMAND",
    "KUZZ_ACP_AGENT_ARGS",
    "KUZZ_ACP_MCP_COMMAND",
    "KUZZ_ACP_LAUNCH_PREFIX",
    // Control-plane parallelism: the Desktop resolves the effective
    // worker-pool size (applying any per-harness cap) and writes it into
    // launch.policy_env. A user-supplied KUZZ_ACP_AGENTS would bypass the
    // harness cap and cause OpenClaw agents to spawn uncapped workers.
    "KUZZ_ACP_AGENTS",
    // Security gates: respond-to mode + allowlist + deployment allowlist +
    // legacy owner-only fallback. Overriding would make the running agent's
    // gate diverge from the saved/UI-visible settings.
    "KUZZ_ACP_RESPOND_TO",
    "KUZZ_ACP_RESPOND_TO_ALLOWLIST",
    "KUZZ_ACP_ALLOWED_RESPOND_TO",
    "KUZZ_ACP_AGENT_OWNER",
    // Stable agent identity used for git attribution and private-conversation
    // provenance must come from the managed-agent record, not user overrides.
    "KUZZ_ACP_DISPLAY_NAME",
    // Remote lifetime/presence policy: user env must not disable the
    // desktop/provider-owned bounds while the saved record still promises them.
    "KUZZ_ACP_EXIT_AFTER_INACTIVITY",
    // Desktop-owned pool lifetime policy: user env must not disable or reset
    // the idle worker-reclamation window while the desktop launcher sets it.
    "KUZZ_ACP_IDLE_POOL_SLEEP",
    // Definition-owned policy: user env cannot override whether channel
    // threads receive independent ACP sessions.
    "KUZZ_ACP_SESSION_POLICY",
    "KUZZ_ACP_STATE_DIR",
    "KUZZ_ACP_NO_PRESENCE",
    // Dispatcher mode follows the default-AI star on the record. User env must
    // not turn an agent the UI shows as ordinary into a channel router, nor
    // hand a per-channel gate policy to a harness the desktop launched on the
    // default gate (channel owner/admin + the agent's owner, no AI authors).
    "KUZZ_ACP_DISPATCHER",
    "KUZZ_ACP_DISPATCHER_CONFIG",
    // The roster section follows the Lead role the same way.
    "KUZZ_ACP_CHANNEL_ROSTER",
    // Readiness handoff: desktop is the ONLY readiness source. A saved or
    // ambient env var must not be able to forge setup mode (NotReady) on a
    // Ready agent or suppress it (empty/stale payload) on a NotReady one.
    "KUZZ_ACP_SETUP_PAYLOAD",
    // Demo-build identity owns the child agent config root. A user override
    // could silently reconnect a demo harness to production OAuth state.
    "KUZZ_AGENT_CONFIG_DIR",
    // Per-agent Hermes profile: every Hermes agent's config root (SOUL.md,
    // memories, `.env`) is chosen by the desktop. An override could point the
    // agent back at the shared root, whose `.env` overrides the agent's
    // signing key.
    "HERMES_HOME",
    // Desktop ownership markers: these brand every spawned harness with the
    // launching Desktop instance. A user-supplied override would let a
    // definition masquerade as a different instance or fake the nonce used
    // for same-session sweep decisions.
    "KUZZ_MANAGED_AGENT",
    "KUZZ_MANAGED_AGENT_START_NONCE",
];

pub(crate) fn is_reserved_env_key(key: &str) -> bool {
    is_reserved_git_config_key(key)
        || RESERVED_ENV_KEYS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(key))
}

/// The complete `GIT_CONFIG*` env family — `GIT_CONFIG` plus every
/// `GIT_CONFIG_*` var (`GIT_CONFIG_COUNT`, the indexed `GIT_CONFIG_KEY_<n>` /
/// `GIT_CONFIG_VALUE_<n>` pairs, `GIT_CONFIG_GLOBAL` / `_SYSTEM` / `_NOSYSTEM`,
/// `GIT_CONFIG_PARAMETERS`) — is reserved.
///
/// Kuzz stages git config through these indexed vars: custom harnesses get
/// only the relay credential helper (`runtime.rs`), while built-in harnesses
/// get it plus the agent identity/signing config from `kuzz-acp`'s
/// `GitEnvironment`. A user override is layered onto
/// the spawn command *after* the credential helper, so a single
/// `GIT_CONFIG_COUNT=0` (or any index collision) silently orphans it, and other
/// family members can redirect git's config resolution entirely. It is a
/// prefix rule because the indexed keys are unbounded; matching the exact
/// `GIT_CONFIG` name and the `GIT_CONFIG_` prefix covers the whole family
/// without catching unrelated names like `GIT_CONFIGURATION`.
fn is_reserved_git_config_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper == "GIT_CONFIG" || upper.starts_with("GIT_CONFIG_")
}
