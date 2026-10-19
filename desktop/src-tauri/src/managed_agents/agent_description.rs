//! Effective public agent description — the Rust twin of
//! `desktop/src/features/agents/lib/agentDescription.ts`.
//!
//! The desktop publishes an agent's effective description as the `about`
//! field of its kind:0 profile event. Only the owner-authored
//! `AgentDefinition.description` publishes; a blank description publishes an
//! empty `about`, exactly as before the field existed.

use super::ManagedAgentRecord;

/// The description to publish for an agent: the authored `description`,
/// trimmed, when non-empty; otherwise `None`.
///
/// TS twin: `effectiveAgentDescription` in `lib/agentDescription.ts`.
pub(crate) fn effective_agent_description(description: Option<&str>) -> Option<String> {
    let authored = description.map(str::trim).unwrap_or("");
    if authored.is_empty() {
        return None;
    }
    Some(authored.to_string())
}

/// Effective description for a managed-agent record's kind:0 profile: the
/// agent's own.
pub(crate) fn record_effective_description(record: &ManagedAgentRecord) -> Option<String> {
    effective_agent_description(record.description.as_deref())
}

// Tests mirror `lib/agentDescription.test.mjs` case-for-case so the Rust
// publish path and the TS display path cannot drift silently.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_description_wins() {
        assert_eq!(
            effective_agent_description(Some("Reviews desktop PRs.")).as_deref(),
            Some("Reviews desktop PRs.")
        );
    }

    #[test]
    fn authored_description_is_trimmed() {
        assert_eq!(
            effective_agent_description(Some("  Reviews desktop PRs.  ")).as_deref(),
            Some("Reviews desktop PRs.")
        );
    }

    #[test]
    fn blank_and_none_descriptions_yield_none() {
        assert_eq!(effective_agent_description(None), None);
        assert_eq!(effective_agent_description(Some("")), None);
        assert_eq!(effective_agent_description(Some("   ")), None);
    }

    fn record_with(description: Option<&str>) -> ManagedAgentRecord {
        let mut record: ManagedAgentRecord = serde_json::from_str(
            r#"{
                "pubkey": "abcd1234",
                "name": "test-agent",
                "private_key_nsec": "nsec1fake",
                "relay_url": "wss://localhost:3000",
                "acp_command": "kuzz-acp",
                "agent_command": "goose",
                "agent_args": [],
                "mcp_command": "",
                "turn_timeout_seconds": 320,
                "system_prompt": null,
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "last_started_at": null,
                "last_stopped_at": null,
                "last_exit_code": null,
                "last_error": null
            }"#,
        )
        .expect("sample record");
        record.description = description.map(str::to_string);
        record
    }

    #[test]
    fn record_publishes_its_own_description() {
        let record = record_with(Some("Record description."));
        assert_eq!(
            record_effective_description(&record).as_deref(),
            Some("Record description.")
        );
    }

    #[test]
    fn source_preset_label_does_not_change_the_description() {
        let mut record = record_with(Some("Record description."));
        record.source_preset_id = Some("p1".to_string());
        assert_eq!(
            record_effective_description(&record).as_deref(),
            Some("Record description.")
        );
    }

    #[test]
    fn no_description_anywhere_yields_none() {
        let record = record_with(None);
        assert_eq!(record_effective_description(&record), None);
    }
}
