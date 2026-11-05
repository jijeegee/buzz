//! This identity's own reminders (kind 30300), encrypted to itself, the same
//! events the app reads and writes.
//!
//! An edit decrypts the stored reminder and changes only its status and time,
//! so fields this CLI does not know survive. One it cannot read publishes
//! nothing.

use kuzz_core::engram::monotonic_created_at;
use kuzz_core::kind::KIND_EVENT_REMINDER;
use nostr::nips::nip44::{self, Version};
use nostr::{Keys, Timestamp};
use serde_json::{json, Map, Value};

use crate::client::KuzzClient;
use crate::commands::messages::fetch_event;
use crate::commands::parse_write_response;
use crate::error::{classify_relay, CliError};
use crate::validate::validate_hex64;
use crate::RemindersCmd;

const PREVIEW_CHARS: usize = 100;
const DAY_SECS: u64 = 86_400;
const LOST_WRITE: &str = "the relay kept another version of this reminder";

#[derive(Debug, Clone, PartialEq)]
struct Reminder {
    id: String,
    not_before: Option<u64>,
    created_at: u64,
    event_id: String,
    content: Map<String, Value>,
}

impl Reminder {
    fn status(&self) -> &str {
        self.content["status"].as_str().unwrap_or("")
    }

    fn is_due(&self, now: u64) -> bool {
        self.status() == "pending" && self.not_before.is_some_and(|at| at <= now)
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "status": self.status(),
            "not_before": self.not_before,
            "note": self.content.get("note"),
            "target": self.content.get("target"),
            "created_at": self.created_at,
            "event_id": self.event_id,
        })
    }
}

fn now_secs() -> u64 {
    Timestamp::now().as_secs()
}

fn parse_time(value: &str, now: u64) -> Result<u64, CliError> {
    let time = match value.parse::<u64>() {
        Ok(secs) => secs,
        Err(_) => chrono::DateTime::parse_from_rfc3339(value)
            .ok()
            .and_then(|time| u64::try_from(time.timestamp()).ok())
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "invalid time {value:?}: use RFC 3339 or unix seconds"
                ))
            })?,
    };
    if time <= now {
        return Err(CliError::Usage(format!("{value:?} is not in the future")));
    }
    Ok(time)
}

fn is_target(value: &Value) -> bool {
    ["eventId", "channelId", "preview", "authorPubkey"]
        .iter()
        .all(|field| value[field].is_string())
}

/// The same checks as the app, which ignores a reminder that fails any.
fn parse_content(plaintext: &str) -> Option<Map<String, Value>> {
    let Value::Object(content) = serde_json::from_str(plaintext).ok()? else {
        return None;
    };
    let status = content.get("status")?.as_str()?;
    if !matches!(status, "pending" | "done" | "cancelled") {
        return None;
    }
    let note = match content.get("note") {
        None => "",
        Some(note) => note.as_str()?,
    };
    match content.get("target") {
        None if note.is_empty() => None,
        None => Some(content),
        Some(target) => is_target(target).then_some(content),
    }
}

fn tag_value(event: &Value, name: &str) -> Option<String> {
    event["tags"]
        .as_array()?
        .iter()
        .find(|tag| tag[0] == name)?
        .get(1)?
        .as_str()
        .map(str::to_string)
}

fn parse_reminder(event: &Value, keys: &Keys) -> Option<Reminder> {
    let id = tag_value(event, "d").filter(|id| !id.is_empty())?;
    let plaintext = nip44::decrypt(
        keys.secret_key(),
        &keys.public_key(),
        event["content"].as_str()?,
    )
    .ok()?;
    Some(Reminder {
        id,
        not_before: tag_value(event, "not_before").and_then(|at| at.parse().ok()),
        created_at: event["created_at"].as_u64()?,
        event_id: event["id"].as_str()?.to_string(),
        content: parse_content(&plaintext)?,
    })
}

/// An edit starts from this read, so it must not come from a replica.
fn reminders_filter(pubkey: &str, id: Option<&str>) -> Value {
    let mut filter = json!({
        "kinds": [KIND_EVENT_REMINDER],
        "authors": [pubkey],
        "consistency": "strong",
    });
    if let Some(id) = id {
        filter["#d"] = json!([id]);
    }
    filter
}

async fn read_one(client: &KuzzClient, keys: &Keys, id: &str) -> Result<Reminder, CliError> {
    let filter = reminders_filter(&client.pubkey().to_hex(), Some(id));
    let events = client.query_all(filter).await?;
    let event = events
        .iter()
        .max_by_key(|event| event["created_at"].as_u64().unwrap_or(0))
        .ok_or_else(|| CliError::NotFound(format!("no reminder {id}")))?;
    parse_reminder(event, keys).ok_or_else(|| CliError::Other(format!("cannot read reminder {id}")))
}

struct Write<'a> {
    id: &'a str,
    content: &'a Map<String, Value>,
    not_before: Option<u64>,
    expiration: Option<u64>,
    created_at: Option<u64>,
}

fn sign_reminder(
    client: &KuzzClient,
    keys: &Keys,
    write: &Write<'_>,
) -> Result<nostr::Event, CliError> {
    let plaintext = serde_json::to_string(write.content)
        .map_err(|e| CliError::Other(format!("cannot encode reminder: {e}")))?;
    let ciphertext = nip44::encrypt(
        keys.secret_key(),
        &keys.public_key(),
        plaintext,
        Version::V2,
    )
    .map_err(|e| CliError::Other(format!("cannot encrypt reminder: {e}")))?;
    let mut builder =
        kuzz_sdk::build_event_reminder(write.id, &ciphertext, write.not_before, write.expiration)
            .map_err(|e| CliError::Usage(format!("invalid reminder: {e}")))?;
    if let Some(created_at) = write.created_at {
        builder = builder.custom_created_at(Timestamp::from_secs(created_at));
    }
    client.sign_event(builder)
}

fn pending_or_refuse(reminder: &Reminder) -> Result<(), CliError> {
    match reminder.status() {
        "pending" => Ok(()),
        status => Err(CliError::Usage(format!(
            "reminder {} is already {status}",
            reminder.id
        ))),
    }
}

/// A retry that already landed also reads as a duplicate, so look before
/// calling it lost.
async fn publish_edit(
    client: &KuzzClient,
    keys: &Keys,
    edited: &Reminder,
    expiration: Option<u64>,
    created_at: u64,
) -> Result<(), CliError> {
    let event = sign_reminder(
        client,
        keys,
        &Write {
            id: &edited.id,
            content: &edited.content,
            not_before: edited.not_before,
            expiration,
            created_at: Some(created_at),
        },
    )?;
    let resp = client.submit_event(event).await?;
    match parse_write_response(&resp, LOST_WRITE) {
        Ok(_) => Ok(()),
        Err(CliError::Conflict(detail)) => {
            let current = read_one(client, keys, &edited.id).await?;
            if current.status() == edited.status() && current.not_before == edited.not_before {
                Ok(())
            } else {
                Err(CliError::Conflict(detail))
            }
        }
        Err(error) => Err(error),
    }
}

fn target_of(event: &Value, event_id: &str) -> Result<Value, CliError> {
    let channel = tag_value(event, "h").ok_or_else(|| {
        CliError::Usage(format!("event {event_id} is not a message in a channel"))
    })?;
    let content = event["content"].as_str().unwrap_or("");
    Ok(json!({
        "eventId": event_id,
        "channelId": channel,
        "preview": content.chars().take(PREVIEW_CHARS).collect::<String>(),
        "authorPubkey": event["pubkey"],
    }))
}

fn new_content(target: Option<Value>, note: Option<&str>) -> Result<Map<String, Value>, CliError> {
    let note = note.filter(|note| !note.is_empty());
    if target.is_none() && note.is_none() {
        return Err(CliError::Usage("--note or --event is required".into()));
    }
    let mut content = Map::new();
    content.insert("status".into(), json!("pending"));
    // Absent, not null: the app ignores a reminder holding a null.
    if let Some(target) = target {
        content.insert("target".into(), target);
    }
    if let Some(note) = note {
        content.insert("note".into(), json!(note));
    }
    Ok(content)
}

async fn cmd_add(
    client: &KuzzClient,
    keys: &Keys,
    at: &str,
    note: Option<&str>,
    event: Option<&str>,
) -> Result<(), CliError> {
    let not_before = parse_time(at, now_secs())?;
    let target = match event {
        Some(event) => {
            validate_hex64(event)?;
            Some(target_of(&fetch_event(client, event).await?, event)?)
        }
        None => None,
    };
    let content = new_content(target, note)?;
    let id = hex::encode(rand::random::<[u8; 16]>());
    let signed = sign_reminder(
        client,
        keys,
        &Write {
            id: &id,
            content: &content,
            not_before: Some(not_before),
            expiration: None,
            created_at: None,
        },
    )?;
    let resp = client.submit_event(signed).await?;
    // The id is new, so a duplicate can only be this write's own retry.
    match parse_write_response(&resp, LOST_WRITE) {
        Ok(_) | Err(CliError::Conflict(_)) => {}
        Err(error) => return Err(error),
    }
    println!("{}", json!({ "id": id, "not_before": not_before }));
    Ok(())
}

async fn cmd_list(client: &KuzzClient, keys: &Keys, due: bool, all: bool) -> Result<(), CliError> {
    let filter = reminders_filter(&client.pubkey().to_hex(), None);
    let events = client.query_all(filter).await?;
    let mut reminders: Vec<Reminder> = events
        .iter()
        .filter_map(|event| parse_reminder(event, keys))
        .collect();
    let skipped = events.len() - reminders.len();
    let now = now_secs();
    reminders.retain(|reminder| {
        if due {
            reminder.is_due(now)
        } else {
            all || reminder.status() == "pending"
        }
    });
    reminders.sort_by_key(|reminder| (reminder.not_before.is_none(), reminder.not_before));
    let rows: Vec<Value> = reminders.iter().map(Reminder::to_json).collect();
    println!("{}", json!({ "reminders": rows, "skipped": skipped }));
    Ok(())
}

fn finished(mut reminder: Reminder, status: &str) -> Reminder {
    reminder.content.insert("status".into(), json!(status));
    reminder.not_before = None;
    reminder
}

fn finish_expiration(now: u64) -> u64 {
    now + rand::random_range(30..90) * DAY_SECS
}

async fn cmd_finish(
    client: &KuzzClient,
    keys: &Keys,
    id: &str,
    status: &str,
) -> Result<(), CliError> {
    let reminder = read_one(client, keys, id).await?;
    pending_or_refuse(&reminder)?;
    let now = now_secs();
    let created_at = monotonic_created_at(now, Some(reminder.created_at));
    let edited = finished(reminder, status);
    let expiration = finish_expiration(now);
    publish_edit(client, keys, &edited, Some(expiration), created_at).await?;
    println!("{}", json!({ "id": id, "status": status }));
    Ok(())
}

async fn cmd_snooze(client: &KuzzClient, keys: &Keys, id: &str, at: &str) -> Result<(), CliError> {
    let now = now_secs();
    let not_before = parse_time(at, now)?;
    let mut reminder = read_one(client, keys, id).await?;
    pending_or_refuse(&reminder)?;
    let created_at = monotonic_created_at(now, Some(reminder.created_at));
    reminder.not_before = Some(not_before);
    publish_edit(client, keys, &reminder, None, created_at).await?;
    println!("{}", json!({ "id": id, "not_before": not_before }));
    Ok(())
}

pub async fn dispatch(cmd: RemindersCmd, client: &KuzzClient) -> Result<(), CliError> {
    run(cmd, client).await.map_err(classify_relay)
}

async fn run(cmd: RemindersCmd, client: &KuzzClient) -> Result<(), CliError> {
    let keys = client.keys("reminders")?;
    match cmd {
        RemindersCmd::Add { at, note, event } => {
            cmd_add(client, keys, &at, note.as_deref(), event.as_deref()).await
        }
        RemindersCmd::List { due, all } => cmd_list(client, keys, due, all).await,
        RemindersCmd::Done { id } => cmd_finish(client, keys, &id, "done").await,
        RemindersCmd::Cancel { id } => cmd_finish(client, keys, &id, "cancelled").await,
        RemindersCmd::Snooze { id, at } => cmd_snooze(client, keys, &id, &at).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "abababababababababababababababab";

    fn stored(keys: &Keys, plaintext: &str, tags: Value) -> Value {
        let content = nip44::encrypt(
            keys.secret_key(),
            &keys.public_key(),
            plaintext,
            Version::V2,
        )
        .unwrap();
        json!({ "id": "e", "created_at": 100, "content": content, "tags": tags })
    }

    fn note_only() -> Reminder {
        let keys = Keys::generate();
        let event = stored(
            &keys,
            r#"{"status":"pending","note":"hi","future":1}"#,
            json!([["d", ID], ["not_before", "500"]]),
        );
        parse_reminder(&event, &keys).unwrap()
    }

    #[test]
    fn a_reminder_round_trips_through_encryption() {
        let reminder = note_only();
        assert_eq!(reminder.id, ID);
        assert_eq!(reminder.not_before, Some(500));
        assert_eq!(reminder.status(), "pending");
        assert_eq!(reminder.to_json()["note"], "hi");
    }

    #[test]
    fn another_key_cannot_read_it() {
        let event = stored(
            &Keys::generate(),
            r#"{"status":"pending","note":"hi"}"#,
            json!([["d", ID]]),
        );
        assert_eq!(parse_reminder(&event, &Keys::generate()), None);
    }

    #[test]
    fn content_the_app_would_ignore_is_refused() {
        for bad in [
            "not json",
            "[]",
            r#"{"status":"pending"}"#,
            r#"{"status":"pending","note":""}"#,
            r#"{"status":"pending","note":null}"#,
            r#"{"status":"snoozed","note":"hi"}"#,
            r#"{"status":"pending","note":"hi","target":null}"#,
            r#"{"status":"pending","target":{"eventId":"e"}}"#,
        ] {
            assert_eq!(parse_content(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_new_reminder_holds_no_nulls() {
        let content = new_content(None, Some("hi")).unwrap();
        let text = serde_json::to_string(&content).unwrap();
        assert!(!text.contains("null"), "{text}");
        assert!(parse_content(&text).is_some());
    }

    #[test]
    fn a_target_reminder_passes_the_app_checks() {
        let event = json!({ "pubkey": "p", "content": "가".repeat(150), "tags": [["h", "c"]] });
        let target = target_of(&event, "e").unwrap();
        assert_eq!(target["channelId"], "c");
        assert_eq!(target["preview"].as_str().unwrap().chars().count(), 100);
        let content = new_content(Some(target), None).unwrap();
        let text = serde_json::to_string(&content).unwrap();
        assert!(parse_content(&text).is_some());
    }

    #[test]
    fn a_message_outside_a_channel_cannot_be_a_target() {
        let event = json!({ "pubkey": "p", "content": "x", "tags": [] });
        assert!(target_of(&event, "e").is_err());
    }

    #[test]
    fn a_reminder_needs_a_note_or_a_message() {
        assert!(new_content(None, None).is_err());
        assert!(new_content(None, Some("")).is_err());
    }

    #[test]
    fn finishing_keeps_every_other_field_and_drops_the_time() {
        let edited = finished(note_only(), "done");
        assert_eq!(edited.status(), "done");
        assert_eq!(edited.not_before, None);
        assert_eq!(edited.content["note"], "hi");
        assert_eq!(edited.content["future"], 1);
    }

    #[test]
    fn a_finished_reminder_stays_finished() {
        assert!(pending_or_refuse(&note_only()).is_ok());
        assert!(pending_or_refuse(&finished(note_only(), "done")).is_err());
    }

    #[test]
    fn finished_reminders_expire_within_the_app_window() {
        for _ in 0..200 {
            let days = (finish_expiration(1_000) - 1_000) / DAY_SECS;
            assert!((30..90).contains(&days), "{days}");
        }
    }

    #[test]
    fn only_a_pending_reminder_past_its_time_is_due() {
        let reminder = note_only();
        assert!(reminder.is_due(500));
        assert!(!reminder.is_due(499));
        assert!(!finished(reminder, "done").is_due(500));
    }

    #[test]
    fn times_take_two_forms_and_must_be_ahead() {
        assert_eq!(parse_time("2000", 1000).unwrap(), 2000);
        assert_eq!(parse_time("1970-01-01T00:16:40Z", 999).unwrap(), 1000);
        assert_eq!(parse_time("1970-01-01T09:16:40+09:00", 999).unwrap(), 1000);
        assert!(parse_time("1000", 1000).is_err());
        assert!(parse_time("tomorrow", 1000).is_err());
    }

    #[test]
    fn reads_come_from_the_primary() {
        let filter = reminders_filter("p", Some(ID));
        assert_eq!(filter["consistency"], "strong");
        assert_eq!(filter["#d"], json!([ID]));
    }
}
