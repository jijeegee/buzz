//! Reports to moderators. A report changes nothing by itself: the relay queues
//! it for them and stores no event.

use serde_json::Value;

use crate::client::{normalize_write_response, KuzzClient};
use crate::commands::messages::fetch_event;
use crate::error::{classify_relay, CliError};
use crate::validate::validate_hex64;
use crate::ReportCmd;

const NOT_YOURSELF: &str = "you cannot report yourself";

fn parse_person(client: &KuzzClient, value: &str) -> Result<String, CliError> {
    let person = nostr::PublicKey::parse(value)
        .map_err(|_| CliError::Usage(format!("invalid pubkey {value:?} (hex or npub)")))?;
    if person == client.pubkey() {
        return Err(CliError::Usage(NOT_YOURSELF.into()));
    }
    Ok(person.to_hex())
}

fn message_author(client: &KuzzClient, message: &Value) -> Result<String, CliError> {
    let author = message["pubkey"]
        .as_str()
        .ok_or_else(|| CliError::Other("the message has no author".into()))?;
    if author == client.pubkey().to_hex() {
        return Err(CliError::Usage(NOT_YOURSELF.into()));
    }
    Ok(author.to_string())
}

async fn publish(client: &KuzzClient, builder: nostr::EventBuilder) -> Result<(), CliError> {
    let event = client.sign_event(builder)?;
    let resp = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&resp));
    Ok(())
}

pub async fn dispatch(cmd: ReportCmd, client: &KuzzClient) -> Result<(), CliError> {
    run(cmd, client).await.map_err(classify_relay)
}

async fn run(cmd: ReportCmd, client: &KuzzClient) -> Result<(), CliError> {
    let builder = match cmd {
        ReportCmd::Message {
            event,
            reason,
            note,
        } => {
            validate_hex64(&event)?;
            let message = fetch_event(client, &event).await?;
            let author = message_author(client, &message)?;
            let note = note.as_deref().unwrap_or("");
            kuzz_sdk::build_report_message(&author, &event, reason.as_str(), note)
        }
        ReportCmd::Person {
            pubkey,
            reason,
            note,
        } => {
            let person = parse_person(client, &pubkey)?;
            let note = note.as_deref().unwrap_or("");
            kuzz_sdk::build_report_person(&person, reason.as_str(), note)
        }
    }
    .map_err(|e| CliError::Usage(format!("invalid report: {e}")))?;
    publish(client, builder).await
}
