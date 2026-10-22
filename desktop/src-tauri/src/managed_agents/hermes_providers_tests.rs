use super::*;

fn keys(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn model(id: &str, provider_label: &str) -> AgentModelInfo {
    AgentModelInfo {
        supported_efforts: None,
        default_effort: None,
        id: id.to_string(),
        name: None,
        description: Some(format!("Provider: {provider_label}")),
    }
}

/// What `hermes-acp` lists on a machine whose Hermes is signed in to ChatGPT
/// but also finds an OS-level OpenAI key, a Claude Code login and a free
/// catalog.
fn hermes_listing(selected_model: Option<&str>) -> AgentModelsResponse {
    AgentModelsResponse {
        agent_name: "hermes-agent".to_string(),
        agent_version: "0.21.1".to_string(),
        models: vec![
            model("anthropic:claude-opus-5", "Anthropic"),
            model("openai-codex:gpt-6-sol", "ChatGPT or Codex Subscription"),
            model(
                "openai-codex:gpt-6-astra",
                "ChatGPT or Codex Subscription • current",
            ),
            model("openai-api:gpt-5", "OpenAI API"),
            model("opencode-free:big-pickle", "OpenCode Free"),
            model("custom:local:llama3:8b", "local"),
        ],
        agent_default_model: Some("openai-codex:gpt-6-astra".to_string()),
        selected_model: selected_model.map(str::to_string),
        supports_switching: true,
        harness_providers: None,
    }
}

fn setup(providers: &[&str], key_names: &[&str]) -> HermesSetup {
    HermesSetup {
        default_provider: Some("openai-codex".to_string()),
        default_model: Some("gpt-6-astra".to_string()),
        providers: keys(providers),
        key_names: keys(key_names),
    }
}

fn ids(response: &AgentModelsResponse) -> Vec<&str> {
    response.models.iter().map(|m| m.id.as_str()).collect()
}

fn provider_ids(response: &AgentModelsResponse) -> Vec<(&str, bool)> {
    response
        .harness_providers
        .as_ref()
        .expect("providers listed")
        .iter()
        .map(|p| (p.id.as_str(), p.is_default))
        .collect()
}

#[test]
fn keeps_only_providers_set_up_in_hermes_plus_user_endpoints() {
    let mut response = hermes_listing(None);
    apply_setup(&mut response, &setup(&["openai-codex"], &[]));
    assert_eq!(
        ids(&response),
        [
            "openai-codex:gpt-6-sol",
            "openai-codex:gpt-6-astra",
            "custom:local:llama3:8b"
        ]
    );
    assert_eq!(
        provider_ids(&response),
        [("openai-codex", true), ("custom:local", false)]
    );
    let default = &response.harness_providers.as_ref().unwrap()[0];
    assert_eq!(default.label, "ChatGPT or Codex Subscription");
}

#[test]
fn default_provider_alias_is_resolved_from_the_current_row() {
    let mut response = hermes_listing(None);
    let mut aliased = setup(&[], &[]);
    aliased.default_provider = Some("codex".to_string());
    apply_setup(&mut response, &aliased);
    assert_eq!(provider_ids(&response)[0], ("openai-codex", true));
}

#[test]
fn api_key_in_hermes_env_offers_its_provider() {
    let mut response = hermes_listing(None);
    apply_setup(
        &mut response,
        &setup(&["openai-codex"], &["OPENAI_API_KEY"]),
    );
    assert!(ids(&response).contains(&"openai-api:gpt-5"));
    assert!(!ids(&response).contains(&"anthropic:claude-opus-5"));
}

#[test]
fn saved_model_hermes_no_longer_offers_stays_visible_and_marked() {
    let mut response = hermes_listing(Some("anthropic:claude-opus-4"));
    apply_setup(&mut response, &setup(&["openai-codex"], &[]));
    let saved = response.models.last().unwrap();
    assert_eq!(saved.id, "anthropic:claude-opus-4");
    assert_eq!(
        saved.description.as_deref(),
        Some(UNAVAILABLE_MODEL_DESCRIPTION)
    );
}

#[test]
fn set_up_sources_follow_hermes_flows_not_borrowed_credentials() {
    let hermes_keys = keys(&["OPENROUTER_API_KEY"]);
    for source in [
        "manual",
        "manual:device_code",
        "device_code",
        "loopback_pkce",
        "hermes_pkce",
        "oauth",
        "config:my-endpoint",
        "model_config",
        "env:OPENROUTER_API_KEY",
    ] {
        assert!(is_set_up_source(source, &hermes_keys), "{source}");
    }
    for source in [
        "gh_cli",
        "claude_code",
        "qwen-cli",
        "env:OPENAI_API_KEY",
        "",
    ] {
        assert!(!is_set_up_source(source, &hermes_keys), "{source}");
    }
}

#[test]
fn env_key_names_never_include_empty_or_commented_keys() {
    let names = parse_env_key_names(
        "# OPENAI_API_KEY=sk-x\nANTHROPIC_API_KEY=\nexport XAI_API_KEY=\"abc\"\nOPENROUTER_API_KEY = k\nnot a line\n",
    );
    assert_eq!(names, keys(&["OPENROUTER_API_KEY", "XAI_API_KEY"]));
}

#[test]
fn read_setup_merges_profile_auth_over_root_per_provider() {
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profiles").join("kuzz-0123456789abcdef");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(
        root.path().join(AUTH_FILE),
        r#"{"active_provider":"openai-codex","providers":{"nous":{}},
            "credential_pool":{
              "openai-codex":[{"source":"manual:device_code"}],
              "copilot":[{"source":"gh_cli"}],
              "openai-api":[{"source":"env:OPENAI_API_KEY"}],
              "xai":[{"source":"device_code"}]}}"#,
    )
    .unwrap();
    std::fs::write(
        profile.join(AUTH_FILE),
        r#"{"credential_pool":{"xai":[{"source":"gh_cli"}]}}"#,
    )
    .unwrap();
    std::fs::write(
        profile.join(CONFIG_FILE),
        "model:\n  default: gpt-6-astra\n  provider: openai-codex\n",
    )
    .unwrap();
    std::fs::write(profile.join(ENV_FILE), "# none\n").unwrap();

    let read = read_setup(&profile, &BTreeSet::new()).expect("setup read");
    assert_eq!(read.default_provider.as_deref(), Some("openai-codex"));
    assert_eq!(read.default_model.as_deref(), Some("gpt-6-astra"));
    // copilot is borrowed, openai-api's key lives only in the OS environment,
    // and the profile's own xai row overrides the root's.
    assert_eq!(read.providers, keys(&["nous", "openai-codex"]));

    let with_agent_key = read_setup(&profile, &keys(&["OPENAI_API_KEY"])).unwrap();
    assert!(with_agent_key.providers.contains("openai-api"));
}

#[test]
fn read_setup_is_none_without_hermes_files() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_setup(dir.path(), &BTreeSet::new()), None);
}

#[test]
fn saved_model_on_a_borrowed_login_is_marked_not_set_up() {
    let mut response = hermes_listing(Some("anthropic:claude-opus-5"));
    apply_setup(&mut response, &setup(&["openai-codex"], &[]));
    let saved = response
        .models
        .iter()
        .find(|m| m.id == "anthropic:claude-opus-5")
        .expect("saved model kept");
    assert_eq!(
        saved.description.as_deref(),
        Some(NOT_SET_UP_MODEL_DESCRIPTION)
    );
}

#[test]
fn unknown_default_provider_leaves_the_list_unfiltered() {
    let mut response = hermes_listing(None);
    for model in &mut response.models {
        model.description = Some("Provider: x".to_string());
    }
    response.agent_default_model = None;
    let mut no_default = setup(&["openai-codex"], &[]);
    no_default.default_provider = None;
    apply_setup(&mut response, &no_default);
    assert_eq!(response.models.len(), 6);
    assert!(response.harness_providers.is_none());
}

#[test]
fn endpoint_label_drops_hermes_description_parts() {
    let row = model("custom:local:llama3:8b", "local • Local Ollama • current");
    assert_eq!(provider_label(&row).as_deref(), Some("local"));
}

#[test]
fn read_setup_is_none_when_a_hermes_file_cannot_be_parsed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(CONFIG_FILE), "model: [unclosed\n").unwrap();
    std::fs::write(dir.path().join(AUTH_FILE), r#"{"active_provider":"nous"}"#).unwrap();
    assert_eq!(read_setup(dir.path(), &BTreeSet::new()), None);
}

#[test]
fn read_setup_reads_single_row_pools_and_skips_secret_fields() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(AUTH_FILE),
        r#"{"credential_pool":{
              "nous":{"source":"device_code","access_token":"secret","extra":{"a":[1]}},
              "copilot":{"source":"gh_cli"},
              "empty":null}}"#,
    )
    .unwrap();
    let read = read_setup(dir.path(), &BTreeSet::new()).expect("setup read");
    assert_eq!(read.providers, keys(&["nous"]));
}
