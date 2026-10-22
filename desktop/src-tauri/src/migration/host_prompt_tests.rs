use super::refresh_host_prompts_in_dir;
use crate::migration::test_support::{read_agents_json, write_agents_json};

fn agent(name: &str, pubkey: &str, preset: &str, prompt: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "pubkey": pubkey,
        "source_preset_id": preset,
        "system_prompt": prompt,
        "model": "kept",
        "unknown_future_field": 7
    })
}

#[test]
fn only_host_agents_get_the_current_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let leftover_preset_row = serde_json::json!({
        "name": "Host", "slug": "builtin:host", "pubkey": "",
        "source_preset_id": "builtin:host", "system_prompt": "old"
    });
    write_agents_json(
        dir.path(),
        &serde_json::json!([
            leftover_preset_row,
            agent("Host", &"a".repeat(64), "builtin:host", "old host prompt"),
            agent("Fizz", &"b".repeat(64), "builtin:fizz", "old fizz prompt"),
        ]),
    );

    let refreshed = refresh_host_prompts_in_dir(&dir.path().join("agents"), "new host").unwrap();
    assert_eq!(refreshed, vec!["Host".to_string()]);

    let rows = read_agents_json(dir.path());
    assert_eq!(
        rows[0]["system_prompt"], "old",
        "preset rows are not agents"
    );
    assert_eq!(rows[1]["system_prompt"], "new host");
    assert_eq!(rows[1]["model"], "kept");
    assert_eq!(
        rows[1]["unknown_future_field"], 7,
        "other fields kept as read"
    );
    assert_eq!(
        rows[2]["system_prompt"], "old fizz prompt",
        "other built-ins keep theirs"
    );
}

#[test]
fn an_up_to_date_host_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_agents_json(
        dir.path(),
        &serde_json::json!([agent("Host", &"a".repeat(64), "builtin:host", "same")]),
    );
    let path = dir.path().join("agents/managed-agents.json");
    let before_bytes = std::fs::read(&path).unwrap();

    let refreshed = refresh_host_prompts_in_dir(&dir.path().join("agents"), "same").unwrap();
    assert!(refreshed.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before_bytes);
}
