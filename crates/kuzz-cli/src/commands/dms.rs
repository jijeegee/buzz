use uuid::Uuid;

use crate::client::{extract_d_tag, normalize_write_response, KuzzClient};
use crate::error::CliError;
use crate::validate::{parse_uuid, validate_hex64};

/// Shown when someone tries to put more than one other person in a DM.
const DM_ONE_TO_ONE_HINT: &str = "Direct messages are 1:1. Start a group chat instead: \
     kuzz channels create --chat --member <pubkey> ...";

/// List DM conversations by querying kind:41001 (relay-confirmed DMs) filtered by our pubkey.
pub async fn cmd_list_dms(client: &KuzzClient, limit: Option<u32>) -> Result<(), CliError> {
    let my_pk = client.pubkey().to_hex();
    let limit = limit.unwrap_or(50).min(200);
    let filter = serde_json::json!({
        "kinds": [41001],
        "#p": [my_pk],
        "limit": limit
    });
    let resp = client.query(&filter).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&resp).unwrap_or_default();
    let dms: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            let dm_id = extract_d_tag(e);
            let participants: Vec<String> = e
                .get("tags")
                .and_then(|t| t.as_array())
                .map(|tags| {
                    tags.iter()
                        .filter_map(|tag| {
                            let arr = tag.as_array()?;
                            if arr.first()?.as_str()? == "p" {
                                arr.get(1)?.as_str().map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            serde_json::json!({
                "dm_id": dm_id,
                "participants": participants,
                "created_at": e.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0),
            })
        })
        .collect();
    let output = serde_json::to_string(&dms).unwrap_or_default();
    println!("{output}");
    Ok(())
}

/// The single pubkey a 1:1 DM opens with: the other person, or your own for
/// notes to self. Repeats of the same key collapse; more than one person is
/// rejected with the group-chat hint.
fn dm_open_pubkey(pubkeys: &[String]) -> Result<String, CliError> {
    let mut distinct: Vec<String> = Vec::with_capacity(1);
    for pk in pubkeys {
        validate_hex64(pk)?;
        let pk = pk.to_ascii_lowercase();
        if !distinct.contains(&pk) {
            distinct.push(pk);
        }
    }
    match distinct.len() {
        1 => Ok(distinct.remove(0)),
        0 => Err(CliError::Usage(
            "--pubkey: pass the other person's pubkey (or your own for notes to self)".into(),
        )),
        _ => Err(CliError::Usage(DM_ONE_TO_ONE_HINT.into())),
    }
}

/// Open a 1:1 DM — sign and submit a kind:41010 event with a d-tag.
pub async fn cmd_open_dm(client: &KuzzClient, pubkeys: &[String]) -> Result<(), CliError> {
    let pubkey = dm_open_pubkey(pubkeys)?;
    let dm_id = Uuid::new_v4().to_string();
    let refs = [pubkey.as_str()];

    // build_dm_open doesn't accept a d-tag, so we build the event manually
    // using the SDK builder and add the d-tag ourselves.
    use nostr::{EventBuilder, Kind, Tag};
    let mut tags: Vec<Tag> = refs
        .iter()
        .map(|pk| Tag::parse(["p", *pk]).map_err(|e| CliError::Other(format!("tag error: {e}"))))
        .collect::<Result<Vec<_>, _>>()?;
    tags.push(Tag::parse(["d", &dm_id]).map_err(|e| CliError::Other(format!("tag error: {e}")))?);
    let builder = EventBuilder::new(Kind::Custom(41010), "").tags(tags);
    let event = client.sign_event(builder)?;

    let resp = client.submit_event(event).await?;
    // Try to extract relay-assigned channel_id from response message.
    // Relay returns: {"event_id":"...","accepted":true,"message":"response:{\"channel_id\":\"...\",\"created\":true}"}
    let relay_dm_id = serde_json::from_str::<serde_json::Value>(&resp)
        .ok()
        .and_then(|v| v.get("message")?.as_str().map(|s| s.to_string()))
        .and_then(|msg| {
            let json_part = msg.strip_prefix("response:")?;
            serde_json::from_str::<serde_json::Value>(json_part).ok()
        })
        .and_then(|v| v.get("channel_id")?.as_str().map(|s| s.to_string()));
    let final_dm_id = relay_dm_id.unwrap_or(dm_id);

    let mut normalized: serde_json::Value =
        serde_json::from_str(&resp).unwrap_or(serde_json::json!({}));
    normalized["dm_id"] = serde_json::json!(final_dm_id);
    if normalized.get("accepted").is_none() {
        normalized["accepted"] = serde_json::json!(true);
    }
    println!("{normalized}");
    Ok(())
}

/// Hide a DM channel — sign and submit a kind:41012 event with h-tag.
pub async fn cmd_hide_dm(client: &KuzzClient, channel_id: &str) -> Result<(), CliError> {
    let channel_uuid = parse_uuid(channel_id)?;

    use nostr::{EventBuilder, Kind, Tag};
    let tags = vec![Tag::parse(["h", &channel_uuid.to_string()])
        .map_err(|e| CliError::Other(format!("tag error: {e}")))?];
    let builder =
        EventBuilder::new(Kind::Custom(kuzz_sdk::kind::KIND_DM_HIDE as u16), "").tags(tags);
    let event = client.sign_event(builder)?;

    let resp = client.submit_event(event).await?;
    println!("{}", normalize_write_response(&resp));
    Ok(())
}

/// `dms add-member` is no longer supported: DMs are 1:1. Fails with the
/// group-chat hint without sending anything.
pub fn cmd_add_dm_member() -> Result<(), CliError> {
    Err(CliError::Usage(DM_ONE_TO_ONE_HINT.into()))
}

pub async fn dispatch(cmd: crate::DmsCmd, client: &KuzzClient) -> Result<(), CliError> {
    use crate::DmsCmd;
    match cmd {
        DmsCmd::List { limit } => cmd_list_dms(client, limit).await,
        DmsCmd::Open { pubkeys } => cmd_open_dm(client, &pubkeys).await,
        DmsCmd::AddMember { .. } => cmd_add_dm_member(),
        DmsCmd::Hide { channel } => cmd_hide_dm(client, &channel).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_open_takes_exactly_one_person() {
        let a = "a".repeat(64);
        assert_eq!(dm_open_pubkey(std::slice::from_ref(&a)).unwrap(), a);
        assert_eq!(
            dm_open_pubkey(&[a.clone(), a.to_ascii_uppercase()]).unwrap(),
            a,
            "repeats of one key collapse"
        );
        let err = dm_open_pubkey(&[a, "b".repeat(64)]).unwrap_err();
        assert!(
            matches!(&err, CliError::Usage(m) if m.contains("kuzz channels create --chat")),
            "{err}"
        );
        assert!(matches!(dm_open_pubkey(&[]), Err(CliError::Usage(_))));
        assert!(dm_open_pubkey(&["short".into()]).is_err());
    }

    #[test]
    fn dm_add_member_explains_group_chats() {
        let err = cmd_add_dm_member().unwrap_err();
        assert!(
            matches!(&err, CliError::Usage(m) if m.starts_with("Direct messages are 1:1.")
                && m.contains("kuzz channels create --chat --member <pubkey>")),
            "{err}"
        );
    }
}
