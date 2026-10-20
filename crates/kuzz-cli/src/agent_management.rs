//! Requests from an agent to its owner's Kuzz Desktop, published through Kuzz
//! observer frames: owner-reviewed drafts and direct agent start/stop.

use kuzz_core::observer::{encrypt_observer_payload, OBSERVER_FRAME_TELEMETRY};
use nostr::{Event, Keys, PublicKey};
use serde::Serialize;

use crate::error::CliError;

const AGENT_REQUEST_KIND: &str = "agent_management_request";
const PROJECT_CHANNEL_REQUEST_KIND: &str = "project_channel_request";
const AGENT_CONTROL_REQUEST_KIND: &str = "agent_control_request";
/// Payload type of the owner Desktop's reply to an agent control request.
pub const AGENT_CONTROL_RESULT_KIND: &str = "agent_control_result";
const MAX_NAME_CHARS: usize = 120;
const MAX_PROMPT_CHARS: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentDraft {
    pub channel_id: String,
    pub display_name: String,
    pub system_prompt: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentDraft {
    pub channel_id: String,
    pub agent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub respond_to: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectChannelDraft {
    pub home_channel_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub visibility: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template_name: Option<String>,
}

/// Which agent a control request is about; empty for `list`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentControlTarget {
    /// The agent's name or hex pubkey.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// What an agent may ask its owner's Desktop to do with the owner's agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentControlAction {
    List,
    Start,
    Stop,
    Restart,
}

impl AgentControlAction {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagementRequest<T> {
    #[serde(rename = "type")]
    request_type: &'static str,
    action: &'static str,
    request_id: String,
    request: T,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ObserverEvent<T> {
    seq: u64,
    timestamp: String,
    kind: &'static str,
    agent_index: Option<usize>,
    channel_id: Option<String>,
    session_id: Option<String>,
    turn_id: Option<String>,
    payload: ManagementRequest<T>,
}

#[derive(Debug)]
pub struct BuiltDraftRequest {
    pub event: Event,
    pub request_id: String,
    pub action: &'static str,
}

fn required(value: String, label: &str, max: usize) -> Result<String, CliError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CliError::Usage(format!("{label} is required")));
    }
    if value.chars().count() > max {
        return Err(CliError::Usage(format!(
            "{label} is too long (max {max} characters)"
        )));
    }
    Ok(value.to_owned())
}

fn optional(value: Option<String>, label: &str) -> Result<Option<String>, CliError> {
    value.map(|value| required(value, label, 300)).transpose()
}

fn build<T: Serialize>(
    keys: &Keys,
    owner: &PublicKey,
    channel_id: Option<String>,
    request_kind: &'static str,
    action: &'static str,
    request: T,
) -> Result<BuiltDraftRequest, CliError> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let payload = ObserverEvent {
        seq: 0,
        timestamp: chrono::Utc::now().to_rfc3339(),
        kind: request_kind,
        agent_index: None,
        channel_id,
        session_id: None,
        turn_id: None,
        payload: ManagementRequest {
            request_type: request_kind,
            action,
            request_id: request_id.clone(),
            request,
        },
    };
    let encrypted = encrypt_observer_payload(keys, owner, &payload).map_err(|error| {
        CliError::Other(format!("could not encrypt request to the owner: {error}"))
    })?;
    let event = kuzz_sdk::build_agent_observer_frame(
        &owner.to_hex(),
        &keys.public_key().to_hex(),
        OBSERVER_FRAME_TELEMETRY,
        &encrypted,
    )
    .map_err(|error| CliError::Other(format!("could not build request to the owner: {error}")))?
    .sign_with_keys(keys)
    .map_err(|error| CliError::Other(format!("could not sign request to the owner: {error}")))?;
    Ok(BuiltDraftRequest {
        event,
        request_id,
        action,
    })
}

pub fn build_create(
    keys: &Keys,
    owner: &PublicKey,
    draft: CreateAgentDraft,
) -> Result<BuiltDraftRequest, CliError> {
    let channel_id = required(draft.channel_id, "channel", 128)?;
    uuid::Uuid::parse_str(&channel_id)
        .map_err(|_| CliError::Usage(format!("invalid channel UUID: {channel_id}")))?;
    let request = CreateAgentDraft {
        channel_id: channel_id.clone(),
        display_name: required(draft.display_name, "display name", MAX_NAME_CHARS)?,
        system_prompt: required(draft.system_prompt, "system prompt", MAX_PROMPT_CHARS)?,
    };
    build(
        keys,
        owner,
        Some(channel_id),
        AGENT_REQUEST_KIND,
        "create",
        request,
    )
}

pub fn build_update(
    keys: &Keys,
    owner: &PublicKey,
    draft: UpdateAgentDraft,
) -> Result<BuiltDraftRequest, CliError> {
    let channel_id = required(draft.channel_id, "channel", 128)?;
    uuid::Uuid::parse_str(&channel_id)
        .map_err(|_| CliError::Usage(format!("invalid channel UUID: {channel_id}")))?;
    let respond_to = optional(draft.respond_to, "respond-to")?;
    if respond_to
        .as_deref()
        .is_some_and(|value| value != "owner-only" && value != "anyone")
    {
        return Err(CliError::Usage(
            "respond-to must be owner-only or anyone".into(),
        ));
    }
    let request = UpdateAgentDraft {
        channel_id: channel_id.clone(),
        agent_name: required(draft.agent_name, "agent name", MAX_NAME_CHARS)?,
        display_name: optional(draft.display_name, "display name")?,
        system_prompt: draft
            .system_prompt
            .map(|value| required(value, "system prompt", MAX_PROMPT_CHARS))
            .transpose()?,
        runtime: optional(draft.runtime, "runtime")?,
        provider: optional(draft.provider, "provider")?,
        model: optional(draft.model, "model")?,
        respond_to,
    };
    if request.display_name.is_none()
        && request.system_prompt.is_none()
        && request.runtime.is_none()
        && request.provider.is_none()
        && request.model.is_none()
        && request.respond_to.is_none()
    {
        return Err(CliError::Usage(
            "include at least one field to update".into(),
        ));
    }
    build(
        keys,
        owner,
        Some(channel_id),
        AGENT_REQUEST_KIND,
        "update",
        request,
    )
}

pub fn build_project_channel(
    keys: &Keys,
    owner: &PublicKey,
    draft: CreateProjectChannelDraft,
) -> Result<BuiltDraftRequest, CliError> {
    let home_channel_id = required(draft.home_channel_id, "home channel", 128)?;
    uuid::Uuid::parse_str(&home_channel_id)
        .map_err(|_| CliError::Usage(format!("invalid channel UUID: {home_channel_id}")))?;
    let visibility = required(draft.visibility, "visibility", 16)?;
    if visibility != "open" && visibility != "private" {
        return Err(CliError::Usage("visibility must be open or private".into()));
    }
    if draft.ttl_seconds == Some(0) {
        return Err(CliError::Usage("ttl must be greater than zero".into()));
    }
    let request = CreateProjectChannelDraft {
        home_channel_id: home_channel_id.clone(),
        name: required(draft.name, "name", MAX_NAME_CHARS)?,
        description: draft
            .description
            .map(|value| required(value, "description", 2_048))
            .transpose()?,
        visibility,
        ttl_seconds: draft.ttl_seconds,
        template_name: optional(draft.template_name, "template")?,
    };
    build(
        keys,
        owner,
        Some(home_channel_id),
        PROJECT_CHANNEL_REQUEST_KIND,
        "create",
        request,
    )
}

pub fn build_control(
    keys: &Keys,
    owner: &PublicKey,
    action: AgentControlAction,
    agent: Option<String>,
) -> Result<BuiltDraftRequest, CliError> {
    let agent = match action {
        AgentControlAction::List => None,
        _ => Some(required(
            agent.unwrap_or_default(),
            "agent name or pubkey",
            MAX_NAME_CHARS,
        )?),
    };
    build(
        keys,
        owner,
        None,
        AGENT_CONTROL_REQUEST_KIND,
        action.as_wire(),
        AgentControlTarget { agent },
    )
}

/// The owner Desktop's reply to `request_id`, or `None` for any other payload.
pub fn parse_control_result(
    payload: &serde_json::Value,
    request_id: &str,
) -> Option<serde_json::Value> {
    (payload.get("type").and_then(|v| v.as_str()) == Some(AGENT_CONTROL_RESULT_KIND)
        && payload.get("requestId").and_then(|v| v.as_str()) == Some(request_id))
    .then(|| payload.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuzz_core::observer::{decrypt_observer_payload, OBSERVER_AGENT_TAG, OBSERVER_FRAME_TAG};

    const CHANNEL: &str = "7c07e659-3610-42f4-9a5e-1e9973c09da9";

    #[test]
    fn create_is_owner_encrypted_and_matches_desktop_contract() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let built = build_create(
            &agent,
            &owner.public_key(),
            CreateAgentDraft {
                channel_id: CHANNEL.into(),
                display_name: "Research helper".into(),
                system_prompt: "Find sources.".into(),
            },
        )
        .unwrap();

        assert_eq!(built.event.kind.as_u16(), 24_200);
        let tags: Vec<Vec<String>> = built
            .event
            .tags
            .iter()
            .map(|tag| tag.as_slice().to_vec())
            .collect();
        assert!(tags
            .iter()
            .any(|tag| tag == &["p", &owner.public_key().to_hex()]));
        assert!(tags
            .iter()
            .any(|tag| tag == &[OBSERVER_AGENT_TAG, &agent.public_key().to_hex()]));
        assert!(tags
            .iter()
            .any(|tag| tag == &[OBSERVER_FRAME_TAG, OBSERVER_FRAME_TELEMETRY]));
        assert!(!tags
            .iter()
            .any(|tag| tag.first().map(String::as_str) == Some("h")));

        let payload: serde_json::Value = decrypt_observer_payload(&owner, &built.event).unwrap();
        assert_eq!(payload["kind"], AGENT_REQUEST_KIND);
        assert_eq!(payload["channelId"], CHANNEL);
        assert_eq!(payload["payload"]["type"], AGENT_REQUEST_KIND);
        assert_eq!(payload["payload"]["action"], "create");
        assert_eq!(
            payload["payload"]["request"]["displayName"],
            "Research helper"
        );
        assert!(payload["payload"]["request"].get("runtime").is_none());
        assert!(payload["payload"]["request"].get("respondTo").is_none());
    }

    #[test]
    fn control_request_is_owner_encrypted_without_channel() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let built = build_control(
            &agent,
            &owner.public_key(),
            AgentControlAction::Restart,
            Some(" Scout ".into()),
        )
        .unwrap();

        assert_eq!(built.event.kind.as_u16(), 24_200);
        assert_eq!(built.action, "restart");
        let payload: serde_json::Value = decrypt_observer_payload(&owner, &built.event).unwrap();
        assert_eq!(payload["kind"], AGENT_CONTROL_REQUEST_KIND);
        assert!(payload["channelId"].is_null());
        assert_eq!(payload["payload"]["type"], AGENT_CONTROL_REQUEST_KIND);
        assert_eq!(payload["payload"]["action"], "restart");
        assert_eq!(payload["payload"]["requestId"], built.request_id.as_str());
        assert_eq!(payload["payload"]["request"]["agent"], "Scout");
    }

    #[test]
    fn control_list_sends_no_target_and_start_requires_one() {
        let owner = Keys::generate().public_key();
        let built = build_control(
            &Keys::generate(),
            &owner,
            AgentControlAction::List,
            Some("ignored".into()),
        )
        .unwrap();
        assert_eq!(built.action, "list");

        let error =
            build_control(&Keys::generate(), &owner, AgentControlAction::Start, None).unwrap_err();
        assert!(error.to_string().contains("agent name or pubkey"));
    }

    #[test]
    fn control_result_matches_only_its_request() {
        let reply = serde_json::json!({
            "type": AGENT_CONTROL_RESULT_KIND,
            "requestId": "abc",
            "ok": true,
        });
        assert!(parse_control_result(&reply, "abc").is_some());
        assert!(parse_control_result(&reply, "other").is_none());
        assert!(parse_control_result(
            &serde_json::json!({"type": "control_result", "requestId": "abc"}),
            "abc"
        )
        .is_none());
    }

    #[test]
    fn update_requires_a_change() {
        let error = build_update(
            &Keys::generate(),
            &Keys::generate().public_key(),
            UpdateAgentDraft {
                channel_id: CHANNEL.into(),
                agent_name: "Scout".into(),
                display_name: None,
                system_prompt: None,
                runtime: None,
                provider: None,
                model: None,
                respond_to: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("at least one field"));
    }

    #[test]
    fn create_rejects_invalid_channel() {
        let error = build_create(
            &Keys::generate(),
            &Keys::generate().public_key(),
            CreateAgentDraft {
                channel_id: "general".into(),
                display_name: "Scout".into(),
                system_prompt: "Help".into(),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("invalid channel UUID"));
    }

    #[test]
    fn project_channel_request_is_owner_encrypted() {
        let agent = Keys::generate();
        let owner = Keys::generate();
        let built = build_project_channel(
            &agent,
            &owner.public_key(),
            CreateProjectChannelDraft {
                home_channel_id: CHANNEL.into(),
                name: "release-planning".into(),
                description: Some("Coordinate the next release.".into()),
                visibility: "open".into(),
                ttl_seconds: None,
                template_name: Some("Release team".into()),
            },
        )
        .unwrap();

        let payload: serde_json::Value = decrypt_observer_payload(&owner, &built.event).unwrap();
        assert_eq!(payload["kind"], PROJECT_CHANNEL_REQUEST_KIND);
        assert_eq!(payload["channelId"], CHANNEL);
        assert_eq!(payload["payload"]["type"], PROJECT_CHANNEL_REQUEST_KIND);
        assert_eq!(payload["payload"]["action"], "create");
        assert_eq!(payload["payload"]["request"]["homeChannelId"], CHANNEL);
        assert_eq!(
            payload["payload"]["request"]["templateName"],
            "Release team"
        );
    }
}
