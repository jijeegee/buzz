use super::*;
use std::path::PathBuf;

/// `Some(Some(value))` set, `Some(None)` explicitly removed, `None` untouched.
fn env_state(command: &Command, key: &str) -> Option<Option<String>> {
    command
        .get_envs()
        .find(|(k, _)| *k == key)
        .map(|(_, value)| value.and_then(|v| v.to_str()).map(str::to_owned))
}

fn generated_dir() -> PathBuf {
    std::env::temp_dir()
        .join("buzz-test-app-data")
        .join("agents")
        .join("routing")
}

/// The env a spawn might arrive with before the routing stamp.
#[derive(Clone, Copy, Debug)]
enum PriorEnv {
    Clean,
    /// Desktop process env (or a saved value) carrying both dispatcher keys.
    Dispatcher,
    /// A Lead-era subscription pointing at our generated rules file.
    GeneratedRules,
    /// The user's own Config-mode rules elsewhere.
    UserRules,
}

const PRIORS: [PriorEnv; 4] = [
    PriorEnv::Clean,
    PriorEnv::Dispatcher,
    PriorEnv::GeneratedRules,
    PriorEnv::UserRules,
];
const ROLES: [RoutingRole; 3] = [
    RoutingRole::None,
    RoutingRole::Dispatcher,
    RoutingRole::Lead,
];

fn user_rules_path() -> String {
    std::env::temp_dir()
        .join("my-rules.toml")
        .to_string_lossy()
        .into_owned()
}

fn command_with(prior: PriorEnv) -> Command {
    let mut command = Command::new("true");
    match prior {
        PriorEnv::Clean => {}
        PriorEnv::Dispatcher => {
            command.env(DISPATCHER_ENV_VAR, "true");
            command.env(DISPATCHER_CONFIG_ENV_VAR, "{\"deadbeef\":{}}");
        }
        PriorEnv::GeneratedRules => {
            command.env(SUBSCRIBE_ENV_VAR, "config");
            command.env(
                CONFIG_ENV_VAR,
                generated_dir().join("lead-abcdef012345.toml"),
            );
        }
        PriorEnv::UserRules => {
            command.env(SUBSCRIBE_ENV_VAR, "config");
            command.env(CONFIG_ENV_VAR, user_rules_path());
        }
    }
    command
}

#[test]
fn every_role_over_every_prior_env_leaves_only_its_own_keys() {
    let dir = generated_dir();
    for role in ROLES {
        for prior in PRIORS {
            let mut command = command_with(prior);
            apply_routing_env(&mut command, role, Some(&dir));
            let label = format!("{role:?} over {prior:?}");

            // The dispatcher flag is set exactly for a dispatcher.
            let expected_flag = if role == RoutingRole::Dispatcher {
                Some(Some("true".to_string()))
            } else {
                Some(None)
            };
            assert_eq!(
                env_state(&command, DISPATCHER_ENV_VAR),
                expected_flag,
                "{label}"
            );
            // The custom gate never survives.
            assert_eq!(
                env_state(&command, DISPATCHER_CONFIG_ENV_VAR),
                Some(None),
                "{label}"
            );

            match (role, prior) {
                // Our generated rules never outlive the Lead role.
                (RoutingRole::None | RoutingRole::Dispatcher, PriorEnv::GeneratedRules) => {
                    assert_eq!(
                        env_state(&command, SUBSCRIBE_ENV_VAR),
                        Some(None),
                        "{label}"
                    );
                    assert_eq!(env_state(&command, CONFIG_ENV_VAR), Some(None), "{label}");
                }
                // The user's own rules file is theirs.
                (_, PriorEnv::UserRules) => {
                    assert_eq!(
                        env_state(&command, SUBSCRIBE_ENV_VAR),
                        Some(Some("config".to_string())),
                        "{label}"
                    );
                    assert_eq!(
                        env_state(&command, CONFIG_ENV_VAR),
                        Some(Some(user_rules_path())),
                        "{label}"
                    );
                }
                // Nothing else touches the subscription keys.
                (RoutingRole::Lead, PriorEnv::GeneratedRules) => {
                    assert_eq!(
                        env_state(&command, SUBSCRIBE_ENV_VAR),
                        Some(Some("config".to_string())),
                        "{label}"
                    );
                }
                (_, PriorEnv::Clean | PriorEnv::Dispatcher) => {
                    assert_eq!(env_state(&command, SUBSCRIBE_ENV_VAR), None, "{label}");
                    assert_eq!(env_state(&command, CONFIG_ENV_VAR), None, "{label}");
                }
            }
        }
    }
}

#[test]
fn without_a_generated_dir_the_subscription_keys_are_left_alone() {
    let mut command = command_with(PriorEnv::GeneratedRules);
    apply_routing_env(&mut command, RoutingRole::None, None);
    assert_eq!(
        env_state(&command, SUBSCRIBE_ENV_VAR),
        Some(Some("config".to_string()))
    );
}

#[test]
fn policy_env_carries_the_flag_only_for_a_dispatcher() {
    for role in ROLES {
        let mut policy_env = BTreeMap::from([
            (DISPATCHER_ENV_VAR.to_string(), "true".to_string()),
            (DISPATCHER_CONFIG_ENV_VAR.to_string(), "{}".to_string()),
            ("KEEP_ME".to_string(), "yes".to_string()),
        ]);
        insert_routing_env(&mut policy_env, role);
        let mut expected = BTreeMap::from([("KEEP_ME".to_string(), "yes".to_string())]);
        if role == RoutingRole::Dispatcher {
            expected.insert(DISPATCHER_ENV_VAR.to_string(), "true".to_string());
        }
        assert_eq!(policy_env, expected, "{role:?}");
    }
}

#[test]
fn the_deployed_role_is_read_back_from_the_payload_policy_env() {
    for role in ROLES {
        let mut policy_env = BTreeMap::new();
        insert_routing_env(&mut policy_env, role);
        let agent_json = serde_json::json!({ "launch": { "policy_env": policy_env } });
        let expected = if role == RoutingRole::Dispatcher {
            RoutingRole::Dispatcher
        } else {
            RoutingRole::None
        };
        assert_eq!(deployed_routing_role(&agent_json), expected, "{role:?}");
    }
    assert_eq!(
        deployed_routing_role(&serde_json::json!({})),
        RoutingRole::None
    );
}

#[test]
fn lead_env_overrides_user_rules_and_appends_the_addendum() {
    let rules = generated_dir().join("lead-abcdef012345.toml");
    let mut command = command_with(PriorEnv::UserRules);
    command.env(SYSTEM_PROMPT_ENV_VAR, "You are Coder.\n");
    apply_lead_env(&mut command, &rules, "ADDENDUM").expect("lead env");
    assert_eq!(
        env_state(&command, SUBSCRIBE_ENV_VAR),
        Some(Some("config".to_string()))
    );
    assert_eq!(
        env_state(&command, CONFIG_ENV_VAR),
        Some(Some(rules.to_string_lossy().into_owned()))
    );
    assert_eq!(
        env_state(&command, CHANNEL_ROSTER_ENV_VAR),
        Some(Some("true".to_string()))
    );
    assert_eq!(
        env_state(&command, SYSTEM_PROMPT_ENV_VAR),
        Some(Some("You are Coder.\n\nADDENDUM".to_string()))
    );
    assert_eq!(env_state(&command, SYSTEM_PROMPT_FILE_ENV_VAR), Some(None));
}

#[test]
fn lead_env_without_a_prompt_is_the_addendum_alone() {
    let mut command = Command::new("true");
    // The spawn path removes the key when no prompt resolves.
    command.env_remove(SYSTEM_PROMPT_ENV_VAR);
    apply_lead_env(&mut command, &generated_dir().join("lead.toml"), "ADDENDUM").expect("lead env");
    assert_eq!(
        env_state(&command, SYSTEM_PROMPT_ENV_VAR),
        Some(Some("ADDENDUM".to_string()))
    );
}

/// The harness rejects both prompt keys together, so a prompt file is inlined
/// ahead of the addendum and its key dropped; an unreadable one fails.
#[test]
fn lead_env_inlines_a_prompt_file() {
    let path = std::env::temp_dir().join(format!(
        "buzz-lead-prompt-{}.md",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&path, "From a file.").expect("write prompt");
    let mut command = Command::new("true");
    command.env_remove(SYSTEM_PROMPT_ENV_VAR);
    command.env(SYSTEM_PROMPT_FILE_ENV_VAR, &path);
    apply_lead_env(&mut command, &generated_dir().join("lead.toml"), "ADDENDUM").expect("lead env");
    assert_eq!(
        env_state(&command, SYSTEM_PROMPT_ENV_VAR),
        Some(Some("From a file.\n\nADDENDUM".to_string()))
    );
    assert_eq!(env_state(&command, SYSTEM_PROMPT_FILE_ENV_VAR), Some(None));
    let _ = std::fs::remove_file(&path);

    let mut command = Command::new("true");
    command.env_remove(SYSTEM_PROMPT_ENV_VAR);
    command.env(SYSTEM_PROMPT_FILE_ENV_VAR, &path);
    assert!(apply_lead_env(&mut command, &generated_dir().join("lead.toml"), "ADDENDUM").is_err());
}

#[test]
fn generated_path_match_resolves_dot_segments_and_windows_case() {
    let dir = generated_dir();
    let dotted = dir
        .join("..")
        .join("routing")
        .join("lead-abcdef012345.toml");
    assert!(is_generated_rules_path(dotted.as_os_str(), &dir));
    let escaped = dir.join("..").join("elsewhere.toml");
    assert!(!is_generated_rules_path(escaped.as_os_str(), &dir));
    if cfg!(windows) {
        let upper = dir.join("lead-x.toml").to_string_lossy().to_uppercase();
        assert!(is_generated_rules_path(OsStr::new(&upper), &dir));
    }
}

/// The inherited branch: a key the command leaves alone reads from the
/// desktop's own environment; a command-level removal wins over it. (A unique
/// key, so parallel tests reading the real routing keys are unaffected.)
#[test]
fn effective_value_falls_back_to_the_inherited_environment() {
    let key = format!("BUZZ_TEST_INHERITED_{}", uuid::Uuid::new_v4().simple());
    std::env::set_var(&key, "from-parent");
    let mut command = Command::new("true");
    assert_eq!(
        effective_env_value(&command, &key),
        Some("from-parent".into())
    );
    command.env_remove(&key);
    assert_eq!(effective_env_value(&command, &key), None);
    std::env::remove_var(&key);
}
