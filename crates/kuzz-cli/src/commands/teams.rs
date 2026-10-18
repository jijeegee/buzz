//! `kuzz teams` — the owner's teams of real agents (kind:30182).
//!
//! A team is a named group of the owner's agents, which may run on different
//! computers. It is owner-signed and global: `d` is the team id, content is
//! `{"v":1,"name","description"?,"agent_pubkeys":[hex]}` (at most 64).
//! Deleting publishes a kind:5 with only an `a` tag for the coordinate.
//!
//! Agents read their owner's teams; only the owner's own key can change them
//! (the relay only accepts the owner's events for this kind).

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use kuzz_core::kind::KIND_AGENT_TEAM;
use nostr::{Event, EventBuilder, Kind, Tag, Timestamp};
use serde::Serialize;

use crate::client::KuzzClient;
use crate::commands::parse_write_response;
use crate::commands::users::presence_subject;
use crate::error::CliError;
use crate::validate::validate_hex64;
use crate::TeamsCmd;

/// Most agents a team can have; the relay refuses more.
pub const MAX_TEAM_AGENTS: usize = 64;
/// Longest name and description, in characters; the relay refuses more.
pub const MAX_TEAM_NAME_CHARS: usize = 200;
pub const MAX_TEAM_DESCRIPTION_CHARS: usize = 2_000;

/// One team, as read from the relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub agent_pubkeys: Vec<String>,
    pub created_at: u64,
}

fn is_lower_hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn d_tag(event: &Event) -> Option<&str> {
    event.tags.iter().find_map(|tag| match tag.as_slice() {
        [name, value, ..] if name == "d" => Some(value.as_str()),
        _ => None,
    })
}

/// The owner's teams from kind:30182 [events]: the newest of each `d`,
/// skipping other authors and nameless teams. Agents are lowercased and
/// deduplicated; anything that is not a pubkey is dropped. Sorted by name.
pub fn teams_from_events(events: &[Event], owner: &str) -> Vec<Team> {
    let mut newest: HashMap<&str, &Event> = HashMap::new();
    for event in events {
        if event.kind != Kind::Custom(KIND_AGENT_TEAM as u16)
            || !event.pubkey.to_hex().eq_ignore_ascii_case(owner)
        {
            continue;
        }
        let Some(d) = d_tag(event) else { continue };
        match newest.get(d) {
            Some(seen) if seen.created_at >= event.created_at => {}
            _ => {
                newest.insert(d, event);
            }
        }
    }
    let mut teams: Vec<Team> = newest
        .into_iter()
        .filter_map(|(d, event)| {
            let content: serde_json::Value = serde_json::from_str(&event.content).ok()?;
            let name = content.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let description = content
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            let mut seen = HashSet::new();
            let agent_pubkeys = content
                .get("agent_pubkeys")
                .and_then(|v| v.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_ascii_lowercase)
                        .filter(|pk| is_lower_hex64(pk) && seen.insert(pk.clone()))
                        .collect()
                })
                .unwrap_or_default();
            Some(Team {
                id: d.to_string(),
                name: name.to_string(),
                description,
                agent_pubkeys,
                created_at: event.created_at.as_secs(),
            })
        })
        .collect();
    teams.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    teams
}

/// Normalize `--agent` values: lowercase 64-hex, deduplicated, order kept.
pub fn normalize_agents(agents: &[String]) -> Result<Vec<String>, CliError> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for agent in agents {
        let agent = agent.trim().to_ascii_lowercase();
        validate_hex64(&agent)?;
        if seen.insert(agent.clone()) {
            out.push(agent);
        }
    }
    Ok(out)
}

/// The kind:30182 content for a team. Refuses a nameless or oversized team.
pub fn team_content(
    name: &str,
    description: Option<&str>,
    agent_pubkeys: &[String],
) -> Result<String, CliError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CliError::Usage("a team needs a name".into()));
    }
    if name.chars().count() > MAX_TEAM_NAME_CHARS {
        return Err(CliError::Usage(format!(
            "a team name can be up to {MAX_TEAM_NAME_CHARS} characters"
        )));
    }
    if description.is_some_and(|d| d.trim().chars().count() > MAX_TEAM_DESCRIPTION_CHARS) {
        return Err(CliError::Usage(format!(
            "a description can be up to {MAX_TEAM_DESCRIPTION_CHARS} characters"
        )));
    }
    if agent_pubkeys.len() > MAX_TEAM_AGENTS {
        return Err(CliError::Usage(format!(
            "a team can have up to {MAX_TEAM_AGENTS} agents"
        )));
    }
    let mut content = serde_json::json!({
        "v": 1,
        "name": name,
        "agent_pubkeys": agent_pubkeys,
    });
    if let Some(description) = description.map(str::trim).filter(|d| !d.is_empty()) {
        content["description"] = serde_json::json!(description);
    }
    Ok(content.to_string())
}

/// The team event. `created_at` is after [previous] so an edit in the same
/// second still replaces it.
pub fn build_team_event(
    id: &str,
    content: String,
    now: u64,
    previous: Option<u64>,
) -> Result<EventBuilder, CliError> {
    let d = Tag::parse(["d", id]).map_err(|e| CliError::Other(format!("invalid d tag: {e}")))?;
    let created_at = previous.map_or(now, |prev| now.max(prev + 1));
    Ok(
        EventBuilder::new(Kind::Custom(KIND_AGENT_TEAM as u16), content)
            .tags([d])
            .custom_created_at(Timestamp::from(created_at)),
    )
}

/// The kind:5 that deletes a team: an `a` tag for its coordinate only, so
/// the relay removes the live replaceable event. The relay deletes versions
/// up to the deletion's time, so it is dated after [team_created_at] even
/// when that version is ahead of this clock.
pub fn build_delete_event(
    owner: &str,
    id: &str,
    now: u64,
    team_created_at: u64,
) -> Result<EventBuilder, CliError> {
    let coordinate = format!("{KIND_AGENT_TEAM}:{owner}:{id}");
    let a = Tag::parse(["a", &coordinate])
        .map_err(|e| CliError::Other(format!("invalid a tag: {e}")))?;
    Ok(EventBuilder::new(Kind::EventDeletion, "")
        .tags([a])
        .custom_created_at(Timestamp::from(now.max(team_created_at + 1))))
}

/// Find a team by id, else by name (case-insensitive). Several teams with
/// that name is an error listing their ids.
pub fn resolve_team<'a>(teams: &'a [Team], query: &str) -> Result<&'a Team, CliError> {
    let query = query.trim();
    if let Some(team) = teams.iter().find(|t| t.id == query) {
        return Ok(team);
    }
    let matches: Vec<&Team> = teams
        .iter()
        .filter(|t| t.name.eq_ignore_ascii_case(query))
        .collect();
    match matches.as_slice() {
        [team] => Ok(team),
        [] => Err(CliError::NotFound(format!("no team {query:?}"))),
        several => Err(CliError::Usage(format!(
            "several teams are named {query:?}; pass an id: {}",
            several
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Whose teams to read: the agent's owner, or this key when it is no agent.
fn owner_hex(client: &KuzzClient, owner: Option<&str>) -> Result<String, CliError> {
    if let Some(owner) = owner {
        let owner = owner.trim().to_ascii_lowercase();
        validate_hex64(&owner)?;
        return Ok(owner);
    }
    Ok(client
        .auth_tag_owner_hex()
        .unwrap_or_else(|| client.pubkey().to_hex())
        .to_ascii_lowercase())
}

/// Teams are signed by their owner. An agent's key cannot change them.
fn writable_owner(client: &KuzzClient) -> Result<String, CliError> {
    let me = client.pubkey().to_hex();
    if let Some(owner) = client.auth_tag_owner_hex() {
        if !owner.eq_ignore_ascii_case(&me) {
            return Err(CliError::Usage(format!(
                "teams belong to your owner ({owner}); change them in the Kuzz app"
            )));
        }
    }
    Ok(me)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

async fn fetch_teams(client: &KuzzClient, owner: &str) -> Result<Vec<Team>, CliError> {
    let filter = serde_json::json!({
        "kinds": [KIND_AGENT_TEAM],
        "authors": [owner],
        "limit": 200,
    });
    let raw = client.query(&filter).await?;
    let events: Vec<Event> = serde_json::from_str(&raw)
        .map_err(|e| CliError::Other(format!("failed to parse relay response: {e}")))?;
    Ok(teams_from_events(&events, owner))
}

/// Names (kind:0) and presence (kind:40902) for [pubkeys]. Best effort: a
/// failed lookup leaves the agent without a name or status.
async fn agent_details(
    client: &KuzzClient,
    pubkeys: &[String],
) -> (HashMap<String, String>, HashMap<String, String>) {
    let mut names = HashMap::new();
    let mut statuses = HashMap::new();
    if pubkeys.is_empty() {
        return (names, statuses);
    }
    let profiles = client
        .query(&serde_json::json!({"kinds": [0], "authors": pubkeys, "limit": pubkeys.len()}))
        .await;
    if let Ok(raw) = profiles {
        let events: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
        for event in events {
            let Some(pubkey) = event.get("pubkey").and_then(|v| v.as_str()) else {
                continue;
            };
            let content: serde_json::Value = event
                .get("content")
                .and_then(|v| v.as_str())
                .and_then(|c| serde_json::from_str(c).ok())
                .unwrap_or_default();
            let name = ["display_name", "name"]
                .iter()
                .find_map(|key| content.get(*key).and_then(|v| v.as_str()))
                .map(str::trim)
                .filter(|n| !n.is_empty());
            if let Some(name) = name {
                names.insert(pubkey.to_ascii_lowercase(), name.to_string());
            }
        }
    }
    let presence = client
        .query(&serde_json::json!({"kinds": [40902], "authors": pubkeys, "limit": pubkeys.len()}))
        .await;
    if let Ok(raw) = presence {
        let events: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
        for event in &events {
            let status = event.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if !status.is_empty() {
                statuses.insert(
                    presence_subject(event).to_ascii_lowercase(),
                    status.to_string(),
                );
            }
        }
    }
    (names, statuses)
}

async fn teams_json(client: &KuzzClient, teams: &[Team]) -> serde_json::Value {
    let mut all: Vec<String> = teams
        .iter()
        .flat_map(|t| t.agent_pubkeys.iter().cloned())
        .collect();
    all.sort();
    all.dedup();
    let (names, statuses) = agent_details(client, &all).await;
    serde_json::Value::Array(
        teams
            .iter()
            .map(|team| {
                let mut value = serde_json::to_value(team).unwrap_or_default();
                value["agents"] = team
                    .agent_pubkeys
                    .iter()
                    .map(|pk| {
                        serde_json::json!({
                            "pubkey": pk,
                            "name": names.get(pk),
                            "status": statuses.get(pk).map_or("offline", String::as_str),
                        })
                    })
                    .collect();
                value
            })
            .collect(),
    )
}

async fn publish_team(
    client: &KuzzClient,
    id: &str,
    name: &str,
    description: Option<&str>,
    agents: &[String],
    previous: Option<u64>,
) -> Result<(), CliError> {
    let content = team_content(name, description, agents)?;
    let event = client.sign_event(build_team_event(id, content, now_secs(), previous)?)?;
    let raw = client.submit_event(event).await?;
    parse_write_response(&raw, "a newer version of this team exists; run it again")?;
    let team = Team {
        id: id.to_string(),
        name: name.trim().to_string(),
        description: description
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        agent_pubkeys: agents.to_vec(),
        created_at: 0,
    };
    println!(
        "{}",
        teams_json(client, std::slice::from_ref(&team)).await[0]
    );
    Ok(())
}

pub async fn dispatch(cmd: TeamsCmd, client: &KuzzClient) -> Result<(), CliError> {
    match cmd {
        TeamsCmd::List { owner } => {
            let owner = owner_hex(client, owner.as_deref())?;
            let teams = fetch_teams(client, &owner).await?;
            println!("{}", teams_json(client, &teams).await);
            Ok(())
        }
        TeamsCmd::Show { team, owner } => {
            let owner = owner_hex(client, owner.as_deref())?;
            let teams = fetch_teams(client, &owner).await?;
            let team = resolve_team(&teams, &team)?;
            println!(
                "{}",
                teams_json(client, std::slice::from_ref(team)).await[0]
            );
            Ok(())
        }
        TeamsCmd::Create {
            name,
            description,
            agents,
        } => {
            writable_owner(client)?;
            let agents = normalize_agents(&agents)?;
            let id = uuid::Uuid::new_v4().to_string();
            publish_team(client, &id, &name, description.as_deref(), &agents, None).await
        }
        TeamsCmd::AddAgent { team, agents } => {
            let owner = writable_owner(client)?;
            let teams = fetch_teams(client, &owner).await?;
            let team = resolve_team(&teams, &team)?;
            let mut members = team.agent_pubkeys.clone();
            for agent in normalize_agents(&agents)? {
                if !members.contains(&agent) {
                    members.push(agent);
                }
            }
            publish_team(
                client,
                &team.id,
                &team.name,
                team.description.as_deref(),
                &members,
                Some(team.created_at),
            )
            .await
        }
        TeamsCmd::RemoveAgent { team, agents } => {
            let owner = writable_owner(client)?;
            let teams = fetch_teams(client, &owner).await?;
            let team = resolve_team(&teams, &team)?;
            let remove = normalize_agents(&agents)?;
            let members: Vec<String> = team
                .agent_pubkeys
                .iter()
                .filter(|pk| !remove.contains(pk))
                .cloned()
                .collect();
            publish_team(
                client,
                &team.id,
                &team.name,
                team.description.as_deref(),
                &members,
                Some(team.created_at),
            )
            .await
        }
        TeamsCmd::Delete { team } => {
            let owner = writable_owner(client)?;
            let teams = fetch_teams(client, &owner).await?;
            let team = resolve_team(&teams, &team)?;
            let event = client.sign_event(build_delete_event(
                &owner,
                &team.id,
                now_secs(),
                team.created_at,
            )?)?;
            let raw = client.submit_event(event).await?;
            parse_write_response(&raw, "team was already deleted")?;
            println!(
                "{}",
                serde_json::json!({"deleted": team.id, "name": team.name})
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::Keys;

    fn team_event(keys: &Keys, d: &str, content: serde_json::Value, at: u64) -> Event {
        EventBuilder::new(Kind::Custom(KIND_AGENT_TEAM as u16), content.to_string())
            .tags([Tag::parse(["d", d]).unwrap()])
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    #[test]
    fn reads_the_newest_of_each_team_for_the_owner() {
        let owner = Keys::generate();
        let other = Keys::generate();
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let events = vec![
            team_event(
                &owner,
                "t1",
                serde_json::json!({"v":1,"name":"Old","agent_pubkeys":[a]}),
                1,
            ),
            team_event(
                &owner,
                "t1",
                serde_json::json!({"v":1,"name":" Build ","description":" ships ",
                    "agent_pubkeys":[b, a.to_uppercase(), b, "nope"]}),
                5,
            ),
            team_event(
                &owner,
                "t2",
                serde_json::json!({"v":1,"name":"docs","agent_pubkeys":[]}),
                5,
            ),
            team_event(&owner, "t3", serde_json::json!({"v":1,"name":" "}), 5),
            team_event(&other, "t4", serde_json::json!({"v":1,"name":"Theirs"}), 5),
        ];
        let teams = teams_from_events(&events, &owner.public_key().to_hex());
        let names: Vec<&str> = teams.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Build", "docs"]);
        assert_eq!(teams[0].description.as_deref(), Some("ships"));
        assert_eq!(teams[0].agent_pubkeys, [b.clone(), a.clone()]);
        assert_eq!(teams[0].created_at, 5);
    }

    #[test]
    fn content_and_events_match_the_relay_rules() {
        let a = "a".repeat(64);
        let content = team_content(" Build ", Some(""), std::slice::from_ref(&a)).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&content).unwrap(),
            serde_json::json!({"v":1,"name":"Build","agent_pubkeys":[a]})
        );
        assert!(matches!(
            team_content(" ", None, &[]),
            Err(CliError::Usage(_))
        ));
        // Characters, not bytes: 200 Korean characters fit.
        assert!(team_content(&"팀".repeat(200), None, &[]).is_ok());
        assert!(matches!(
            team_content(&"팀".repeat(201), None, &[]),
            Err(CliError::Usage(_))
        ));
        let many: Vec<String> = (0..=MAX_TEAM_AGENTS).map(|i| format!("{i:064x}")).collect();
        assert!(matches!(
            team_content("Big", None, &many),
            Err(CliError::Usage(_))
        ));

        let keys = Keys::generate();
        // An edit in the same second still replaces the previous version.
        let event = build_team_event("t1", content, 100, Some(100))
            .unwrap()
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(event.created_at.as_secs(), 101);
        assert_eq!(event.kind, Kind::Custom(30182));
        assert_eq!(event.tags.iter().next().unwrap().as_slice(), ["d", "t1"]);

        let owner = keys.public_key().to_hex();
        let deletion = build_delete_event(&owner, "t1", 100, 150)
            .unwrap()
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(deletion.kind, Kind::EventDeletion);
        // After the team's version, even when it is ahead of this clock.
        assert_eq!(deletion.created_at.as_secs(), 151);
        let tags: Vec<Vec<String>> = deletion
            .tags
            .iter()
            .map(|t| t.as_slice().to_vec())
            .collect();
        assert_eq!(tags, [vec!["a".to_string(), format!("30182:{owner}:t1")]]);
    }

    #[test]
    fn agents_are_normalized_and_teams_resolve_by_id_or_name() {
        let a = "a".repeat(64);
        assert_eq!(
            normalize_agents(&[a.to_uppercase(), a.clone()]).unwrap(),
            [a.as_str()]
        );
        assert!(normalize_agents(&["nope".into()]).is_err());

        let team = |id: &str, name: &str| Team {
            id: id.into(),
            name: name.into(),
            description: None,
            agent_pubkeys: vec![],
            created_at: 1,
        };
        let teams = [team("t1", "Build"), team("t2", "Docs"), team("t3", "docs")];
        assert_eq!(resolve_team(&teams, "t2").unwrap().id, "t2");
        assert_eq!(resolve_team(&teams, "build").unwrap().id, "t1");
        assert!(matches!(
            resolve_team(&teams, "Docs"),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            resolve_team(&teams, "Ops"),
            Err(CliError::NotFound(_))
        ));
    }
}
