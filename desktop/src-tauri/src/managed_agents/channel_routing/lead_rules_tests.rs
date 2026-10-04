use super::*;
use crate::managed_agents::channel_routing::{routing_role_for, ChannelRoutingMode};

/// The harness tests match events against this exact file, so a rendering
/// change here must be mirrored there (and vice versa).
const FIXTURE: &str = include_str!("../../../../../crates/buzz-acp/tests/fixtures/lead_rules.toml");
/// Pubkey of secret key 1 — the owner the fixture listens to.
const FIXTURE_OWNER: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const AGENT: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
const OTHER_AGENT: &str = "0123456789ab0123456789ab0123456789ab0123456789ab0123456789ab0123";

/// `Some(Some(value))` set, `Some(None)` explicitly removed, `None` untouched.
fn env_state(command: &Command, key: &str) -> Option<Option<String>> {
    command
        .get_envs()
        .find(|(k, _)| *k == key)
        .map(|(_, value)| value.and_then(|v| v.to_str()).map(str::to_owned))
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "buzz-lead-rules-{label}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn record(local: bool) -> ManagedAgentRecord {
    let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": AGENT,
        "name": "Coder",
        "private_key_nsec": "",
        "relay_url": "",
        "acp_command": "buzz-acp",
        "agent_command": "goose",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 300,
        "created_at": "",
        "updated_at": "",
    }))
    .expect("minimal record deserializes");
    record.is_default_ai = true;
    if !local {
        record.backend = BackendKind::Provider {
            id: "cloud".into(),
            config: serde_json::Value::Null,
        };
    }
    record
}

#[test]
fn rendering_is_byte_identical_to_the_harness_fixture() {
    let rendered = render_lead_rules(FIXTURE_OWNER).expect("valid owner");
    assert_eq!(rendered, FIXTURE.replace("\r\n", "\n"));
    // Uppercase input renders the same lowercase key the harness compares.
    assert_eq!(
        render_lead_rules(&FIXTURE_OWNER.to_ascii_uppercase()).expect("valid owner"),
        rendered
    );
}

#[test]
fn the_mention_rule_comes_first_with_the_harness_default_kinds() {
    let rendered = render_lead_rules(FIXTURE_OWNER).expect("valid owner");
    let mentions = rendered.find(r#"name = "mentions""#).expect("mention rule");
    let listen = rendered
        .find(r#"name = "lead-listen""#)
        .expect("listener rule");
    assert!(
        mentions < listen,
        "first match wins: mentions must come first"
    );
    assert!(rendered.contains("kinds = [9, 40003, 46010, 40007]"));
}

#[test]
fn an_owner_that_is_not_a_hex_key_is_refused_before_it_reaches_the_filter() {
    for owner in [
        "",
        "abc",
        &format!("{}\" || true || \"", &FIXTURE_OWNER[..40]),
        &"zz".repeat(32),
    ] {
        assert!(render_lead_rules(owner).is_err(), "{owner:?}");
    }
}

#[test]
fn file_name_uses_the_first_twelve_hex_chars() {
    assert_eq!(lead_rules_file_name(AGENT), "lead-abcdef012345.toml");
}

#[test]
fn a_lead_spawn_writes_its_rules_and_stamps_the_listening_env() {
    let dir = temp_dir("spawn");
    let mut command = Command::new("true");
    command.env("BUZZ_ACP_SYSTEM_PROMPT", "You are Coder.");
    apply_lead_spawn(
        &mut command,
        RoutingRole::Lead,
        AGENT,
        Some(FIXTURE_OWNER),
        Ok(dir.clone()),
    )
    .expect("lead spawn");

    let path = dir.join("lead-abcdef012345.toml");
    assert_eq!(
        std::fs::read_to_string(&path).expect("rules written"),
        render_lead_rules(FIXTURE_OWNER).expect("valid owner")
    );
    assert_eq!(
        env_state(&command, "BUZZ_ACP_SUBSCRIBE"),
        Some(Some("config".into()))
    );
    assert_eq!(
        env_state(&command, "BUZZ_ACP_CONFIG"),
        Some(Some(path.to_string_lossy().into_owned()))
    );
    assert_eq!(
        env_state(&command, "BUZZ_ACP_SYSTEM_PROMPT"),
        Some(Some(format!("You are Coder.\n\n{LEAD_LISTEN_ADDENDUM}")))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_lead_spawn_fails_rather_than_falling_back_to_mentions() {
    let mut command = Command::new("true");
    assert!(apply_lead_spawn(
        &mut command,
        RoutingRole::Lead,
        AGENT,
        Some(FIXTURE_OWNER),
        Err("no app data dir".into()),
    )
    .is_err());
    assert!(apply_lead_spawn(
        &mut command,
        RoutingRole::Lead,
        AGENT,
        None,
        Ok(temp_dir("no-owner")),
    )
    .is_err());
    assert_eq!(env_state(&command, "BUZZ_ACP_SUBSCRIBE"), None);
}

#[test]
fn a_non_lead_spawn_drops_only_its_own_leftover_file() {
    let dir = temp_dir("discard");
    let mine = write_lead_rules_in(&dir, AGENT, FIXTURE_OWNER).expect("write");
    let theirs = write_lead_rules_in(&dir, OTHER_AGENT, FIXTURE_OWNER).expect("write");
    for role in [RoutingRole::None, RoutingRole::Dispatcher] {
        let mut command = Command::new("true");
        apply_lead_spawn(
            &mut command,
            role,
            AGENT,
            Some(FIXTURE_OWNER),
            Ok(dir.clone()),
        )
        .expect("non-lead spawn never fails on cleanup");
        assert!(!mine.exists(), "{role:?}");
        assert!(theirs.exists(), "{role:?}");
        assert_eq!(env_state(&command, "BUZZ_ACP_SUBSCRIBE"), None, "{role:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sweep_keeps_running_leads_and_ignores_other_files() {
    let dir = temp_dir("sweep");
    let kept = write_lead_rules_in(&dir, AGENT, FIXTURE_OWNER).expect("write");
    let stale = write_lead_rules_in(&dir, OTHER_AGENT, FIXTURE_OWNER).expect("write");
    let unrelated = dir.join("notes.toml");
    std::fs::write(&unrelated, "x").expect("write");

    sweep_lead_rules_in(&dir, &[AGENT]).expect("sweep");
    assert!(kept.exists());
    assert!(!stale.exists());
    assert!(unrelated.exists());

    sweep_lead_rules_in(&dir, &[]).expect("sweep");
    assert!(!kept.exists());
    assert!(unrelated.exists());

    let _ = std::fs::remove_dir_all(&dir);
    sweep_lead_rules_in(&dir, &[]).expect("a missing directory is already clean");
}

/// `set_channel_routing` refuses exactly the Lead picks that
/// `routing_role_for` would resolve to no role.
#[test]
fn lead_refusals_match_the_role_table() {
    for local in [true, false] {
        for owner in [Some(FIXTURE_OWNER), None] {
            let record = record(local);
            let reason = lead_unavailable_reason(&record, owner);
            let role = routing_role_for(&record, ChannelRoutingMode::Lead, owner);
            assert_eq!(
                reason.is_none(),
                role == RoutingRole::Lead,
                "local={local} owner={owner:?}"
            );
        }
    }
    assert_eq!(
        lead_unavailable_reason(&record(false), Some(FIXTURE_OWNER)),
        Some("Lead needs an agent on this computer.")
    );
}
