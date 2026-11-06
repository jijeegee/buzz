//! Who a paired agent answers. The connector reads the owner-signed
//! `kind:30177` with the agent's key as `d`, the record app agents publish;
//! no record or a bad one means owner-only. Disconnecting narrows it first,
//! so pairing the same key again can't bring back a wider setting.

use std::future::Future;
use std::time::Duration;

use nostr::{Event, EventBuilder, Kind, PublicKey, Tag, Timestamp};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::app_state::AppState;
use crate::managed_agents::agent_events::ManagedAgentEventContent;
use crate::managed_agents::RespondTo;
use kuzz_core_pkg::kind::KIND_MANAGED_AGENT;

/// A stalled relay must not hold up a disconnect.
const NARROW_LIMIT: Duration = Duration::from_secs(10);

/// A paired agent's setting, as the card shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalAccess {
    pub respond_to: RespondTo,
    pub respond_to_allowlist: Vec<String>,
}

fn canonical_pubkey(raw: &str) -> Result<String, String> {
    PublicKey::from_hex(raw.trim())
        .map(|key| key.to_hex())
        .map_err(|_| format!("invalid public key: {raw}"))
}

fn content_of(head: &Event) -> Value {
    serde_json::from_str(&head.content).unwrap_or(Value::Null)
}

/// The setting in `head`, read as the connector reads it; none or a bad
/// record is owner-only.
fn access_of(head: Option<&Event>) -> ExternalAccess {
    let content = head.map_or(Value::Null, content_of);
    let (respond_to, respond_to_allowlist) = match content["respond_to"].as_str() {
        Some("anyone") => (RespondTo::Anyone, Vec::new()),
        Some("allowlist") => (
            RespondTo::Allowlist,
            content["respond_to_allowlist"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|person| canonical_pubkey(person).ok())
                .collect(),
        ),
        _ => (RespondTo::OwnerOnly, Vec::new()),
    };
    ExternalAccess {
        respond_to,
        respond_to_allowlist,
    }
}

/// The record's content. Only an allowlist keeps its people.
fn access_content(
    name: &str,
    respond_to: RespondTo,
    allowlist: Vec<String>,
) -> Result<String, String> {
    serde_json::to_string(&ManagedAgentEventContent {
        name: name.to_string(),
        persona_id: None,
        system_prompt: None,
        model: None,
        provider: None,
        persona_source_version: None,
        parallelism: 1,
        respond_to,
        respond_to_allowlist: if respond_to == RespondTo::Allowlist {
            allowlist
        } else {
            Vec::new()
        },
    })
    .map_err(|error| format!("failed to serialize agent access: {error}"))
}

/// A replacement must be newer than the head, also within one second.
fn next_created_at(previous: Option<u64>, now: u64) -> u64 {
    previous.map_or(now, |previous| now.max(previous + 1))
}

/// The active account's newest record for `agent`.
async fn read_head(state: &AppState, agent: &str) -> Result<Option<Event>, String> {
    let owner = state.user_credential()?.public_key();
    let events = crate::relay::query_relay(
        state,
        &[serde_json::json!({
            "kinds": [KIND_MANAGED_AGENT],
            "authors": [owner.to_hex()],
            "#d": [agent],
        })],
    )
    .await?;
    Ok(events
        .into_iter()
        .filter(|event| event.pubkey == owner)
        .max_by_key(|event| event.created_at))
}

async fn publish(
    state: &AppState,
    agent: &str,
    content: String,
    previous: Option<u64>,
) -> Result<(), String> {
    // A token session sends drafts, which the connector refuses as unsigned.
    if state.user_credential()?.is_token() {
        return Err("This account can't change agent access.".to_string());
    }
    let tag = Tag::parse(["d", agent]).map_err(|error| format!("invalid d-tag: {error}"))?;
    let created_at = next_created_at(previous, Timestamp::now().as_secs());
    let builder = EventBuilder::new(Kind::Custom(KIND_MANAGED_AGENT as u16), content)
        .tags(vec![tag])
        .custom_created_at(Timestamp::from_secs(created_at));
    let result = crate::relay::submit_event(builder, state).await?;
    if result.accepted {
        Ok(())
    } else {
        Err(result.message)
    }
}

/// Whether a read of the head leaves `agent` possibly wider than owner-only.
/// A mode that can't be read counts as wider.
fn needs_narrowing(read: &Result<Option<Event>, String>) -> bool {
    match read {
        Ok(None) => false,
        Ok(Some(head)) => access_of(Some(head)).respond_to != RespondTo::OwnerOnly,
        Err(_) => true,
    }
}

/// Publish owner-only for `agent` unless it is known to be owner-only.
async fn narrow(state: &AppState, agent: &str) -> Result<(), String> {
    let read = read_head(state, agent).await;
    if !needs_narrowing(&read) {
        return Ok(());
    }
    let head = read.ok().flatten();
    let name = head
        .as_ref()
        .map(content_of)
        .and_then(|content| content["name"].as_str().map(str::to_string))
        .unwrap_or_else(|| "Agent".to_string());
    let previous = head.as_ref().map(|head| head.created_at.as_secs());
    let content = access_content(&name, RespondTo::OwnerOnly, Vec::new())?;
    publish(state, agent, content, previous).await
}

/// Narrow the agents `agents` lists before a disconnect, owner tools off
/// first. Logged, never in the way of it: the disconnect must work when this
/// can't.
pub(super) async fn narrow_before_disconnect<F>(
    state: &AppState,
    origin: &str,
    access: &str,
    agents: F,
) where
    F: Future<Output = Result<Vec<String>, String>>,
{
    let work = async {
        for agent in agents.await? {
            super::tool_levels::lower_before_disconnect(state, origin, access, &agent).await;
            // One failure must not leave the others wide.
            if let Err(error) = narrow(state, &agent).await {
                eprintln!("kuzz-desktop: narrowing a disconnected agent's access failed: {error}");
            }
        }
        Ok::<(), String>(())
    };
    let result = tokio::time::timeout(NARROW_LIMIT, work)
        .await
        .unwrap_or_else(|_| Err("timed out".to_string()));
    if let Err(error) = result {
        eprintln!("kuzz-desktop: narrowing a disconnected agent's access failed: {error}");
    }
}

#[tauri::command]
pub async fn get_external_agent_access(
    app: AppHandle,
    pubkey: String,
) -> Result<ExternalAccess, String> {
    let agent = canonical_pubkey(&pubkey)?;
    let state = app.state::<AppState>();
    Ok(access_of(read_head(&state, &agent).await?.as_ref()))
}

/// Saved only once the relay accepted it.
#[tauri::command]
pub async fn set_external_agent_access(
    app: AppHandle,
    pubkey: String,
    name: String,
    respond_to: RespondTo,
    allowlist: Vec<String>,
    tool_level: Option<super::tool_levels::ToolLevel>,
) -> Result<(), String> {
    if crate::commands::agent_access_owner_only() && respond_to != RespondTo::OwnerOnly {
        return Err("This build allows owner-only access.".to_string());
    }
    let agent = canonical_pubkey(&pubkey)?;
    let allowlist = allowlist
        .iter()
        .map(|person| canonical_pubkey(person))
        .collect::<Result<Vec<_>, _>>()?;
    let state = app.state::<AppState>();
    let previous = read_head(&state, &agent)
        .await?
        .map(|head| head.created_at.as_secs());
    // Owner tools first: with the level on, a failed call fails the save.
    super::tool_levels::before_external_save(&state, &agent, respond_to, tool_level).await?;
    let content = access_content(&name, respond_to, allowlist)?;
    publish(&state, &agent, content, previous).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::Keys;

    fn head(content: &str) -> Event {
        EventBuilder::new(Kind::Custom(KIND_MANAGED_AGENT as u16), content)
            .sign_with_keys(&Keys::generate())
            .expect("sign")
    }

    #[test]
    fn no_record_or_a_bad_one_is_owner_only() {
        let owner_only = ExternalAccess {
            respond_to: RespondTo::OwnerOnly,
            respond_to_allowlist: Vec::new(),
        };
        assert_eq!(access_of(None), owner_only);
        assert_eq!(access_of(Some(&head("not json"))), owner_only);
        assert_eq!(access_of(Some(&head(r#"{"name":"a"}"#))), owner_only);
        assert_eq!(
            access_of(Some(&head(r#"{"respond_to":"all"}"#))),
            owner_only
        );
    }

    #[test]
    fn a_record_reads_as_the_connector_reads_it() {
        let person = "a".repeat(64);
        assert_eq!(
            access_of(Some(&head(r#"{"respond_to":"anyone"}"#))).respond_to,
            RespondTo::Anyone
        );
        let mixed =
            format!(r#"{{"respond_to":"allowlist","respond_to_allowlist":["{person}",1,"bad"]}}"#);
        assert_eq!(
            access_of(Some(&head(&mixed))).respond_to_allowlist,
            vec![person]
        );
    }

    #[test]
    fn only_a_setting_known_to_be_owner_only_stays() {
        assert!(!needs_narrowing(&Ok(None)));
        assert!(!needs_narrowing(&Ok(Some(head(
            r#"{"respond_to":"owner-only"}"#
        )))));
        assert!(needs_narrowing(&Ok(Some(head(
            r#"{"respond_to":"anyone"}"#
        )))));
        assert!(needs_narrowing(&Err("offline".to_string())));
    }

    #[test]
    fn content_reads_back() {
        let person = "a".repeat(64);
        let content = access_content("Hermes", RespondTo::Allowlist, vec![person.clone()]).unwrap();
        assert_eq!(
            access_of(Some(&head(&content))),
            ExternalAccess {
                respond_to: RespondTo::Allowlist,
                respond_to_allowlist: vec![person],
            }
        );
        let wire: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(wire["respond_to"], "allowlist");
        assert_eq!(wire["parallelism"], 1);
    }

    #[test]
    fn only_an_allowlist_keeps_its_people() {
        let content = access_content("Hermes", RespondTo::Anyone, vec!["a".repeat(64)]).unwrap();
        let wire: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(wire["respond_to"], "anyone");
        assert!(wire.get("respond_to_allowlist").is_none());
    }

    #[test]
    fn a_replacement_is_newer_than_the_head() {
        assert_eq!(next_created_at(None, 100), 100);
        assert_eq!(next_created_at(Some(40), 100), 100);
        assert_eq!(next_created_at(Some(100), 100), 101);
        assert_eq!(next_created_at(Some(500), 100), 501);
    }
}
