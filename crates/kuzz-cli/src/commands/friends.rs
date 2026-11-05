//! This identity's own friend list (kind 3).
//!
//! An edit reads the published list and republishes all of it, keeping what
//! the list holds besides `p` tags, the way the app does. A failed read
//! publishes nothing.

use kuzz_core::engram::monotonic_created_at;
use nostr::Timestamp;
use serde_json::{json, Value};

use crate::client::KuzzClient;
use crate::commands::parse_write_response;
use crate::error::{classify_relay, CliError};
use crate::FriendsCmd;

#[derive(Debug, Clone, PartialEq)]
struct Friend {
    pubkey: String,
    relay_url: Option<String>,
    petname: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
struct FriendList {
    created_at: Option<u64>,
    friends: Vec<Friend>,
    content: String,
    other_tags: Vec<Vec<String>>,
}

fn optional(value: Option<&String>) -> Option<String> {
    value.filter(|value| !value.is_empty()).cloned()
}

fn parse_friend_list(raw: &str) -> Result<FriendList, CliError> {
    let bad = |detail: String| CliError::Other(format!("cannot read friend list: {detail}"));
    let events: Vec<Value> = serde_json::from_str(raw).map_err(|e| bad(e.to_string()))?;
    let Some(event) = events
        .iter()
        .max_by_key(|event| event["created_at"].as_u64().unwrap_or(0))
    else {
        return Ok(FriendList::default());
    };
    let tags: Vec<Vec<String>> =
        serde_json::from_value(event["tags"].clone()).map_err(|e| bad(format!("bad tags: {e}")))?;
    let mut list = FriendList {
        created_at: event["created_at"].as_u64(),
        content: event["content"].as_str().unwrap_or("").to_string(),
        ..FriendList::default()
    };
    for tag in tags {
        match tag.first().map(String::as_str) {
            Some("p") => {
                let pubkey = tag.get(1).cloned().unwrap_or_default().to_lowercase();
                if pubkey.is_empty() {
                    return Err(bad("p tag without a pubkey".into()));
                }
                list.friends.push(Friend {
                    pubkey,
                    relay_url: optional(tag.get(2)),
                    petname: optional(tag.get(3)),
                });
            }
            // Signing adds this identity's current auth tag; a kept copy
            // would be a second one, which it refuses.
            Some("auth") | None => {}
            Some(_) => list.other_tags.push(tag),
        }
    }
    Ok(list)
}

/// Every edit starts from this read, so it must not come from a replica: a
/// list that lags would be republished over the real one.
fn friend_list_filter(pubkey: &str) -> Value {
    json!({
        "kinds": [3],
        "authors": [pubkey],
        "limit": 1,
        "consistency": "strong",
    })
}

async fn read_friend_list(client: &KuzzClient) -> Result<FriendList, CliError> {
    let raw = client
        .query(&friend_list_filter(&client.pubkey().to_hex()))
        .await?;
    parse_friend_list(&raw)
}

/// True when the list changed. Naming a friend again renames them.
fn add_friend(list: &mut FriendList, pubkey: &str, name: Option<&str>) -> bool {
    match list.friends.iter_mut().find(|f| f.pubkey == pubkey) {
        Some(friend) => match name {
            Some(name) if friend.petname.as_deref() != Some(name) => {
                friend.petname = Some(name.to_string());
                true
            }
            _ => false,
        },
        None => {
            list.friends.push(Friend {
                pubkey: pubkey.to_string(),
                relay_url: None,
                petname: name.map(str::to_string),
            });
            true
        }
    }
}

fn remove_friend(list: &mut FriendList, pubkey: &str) -> bool {
    let before = list.friends.len();
    list.friends.retain(|friend| friend.pubkey != pubkey);
    list.friends.len() != before
}

fn parse_person(client: &KuzzClient, value: &str) -> Result<String, CliError> {
    let person = nostr::PublicKey::parse(value)
        .map_err(|_| CliError::Usage(format!("invalid pubkey {value:?} (hex or npub)")))?;
    if person == client.pubkey() {
        return Err(CliError::Usage(
            "you cannot list yourself as a friend".into(),
        ));
    }
    Ok(person.to_hex())
}

async fn publish(client: &KuzzClient, list: &FriendList) -> Result<String, CliError> {
    let contacts: Vec<(&str, Option<&str>, Option<&str>)> = list
        .friends
        .iter()
        .map(|f| {
            (
                f.pubkey.as_str(),
                f.relay_url.as_deref(),
                f.petname.as_deref(),
            )
        })
        .collect();
    let now = Timestamp::now().as_secs();
    let created_at = monotonic_created_at(now, list.created_at);
    let builder = kuzz_sdk::build_contact_list_keeping(&contacts, &list.content, &list.other_tags)
        .map_err(|e| CliError::Usage(format!("invalid friend list: {e}")))?
        .custom_created_at(Timestamp::from_secs(created_at));
    let event = client.sign_event(builder)?;
    let resp = client.submit_event(event).await?;
    parse_write_response(&resp, "the relay kept another version of the friend list")
}

/// A retry that already landed also reads as a duplicate, so look before
/// calling it lost.
async fn publish_checked(
    client: &KuzzClient,
    list: &FriendList,
    has_person: impl Fn(&FriendList) -> bool,
) -> Result<(), CliError> {
    match publish(client, list).await {
        Ok(_) => Ok(()),
        Err(CliError::Conflict(detail)) => {
            let current = read_friend_list(client).await?;
            if has_person(&current) {
                Ok(())
            } else {
                Err(CliError::Conflict(detail))
            }
        }
        Err(error) => Err(error),
    }
}

async fn cmd_list(client: &KuzzClient) -> Result<(), CliError> {
    let list = read_friend_list(client).await?;
    let friends: Vec<Value> = list
        .friends
        .iter()
        .map(|f| json!({ "pubkey": f.pubkey, "petname": f.petname }))
        .collect();
    println!("{}", json!({ "friends": friends }));
    Ok(())
}

async fn cmd_add(client: &KuzzClient, person: &str, name: Option<&str>) -> Result<(), CliError> {
    let pubkey = parse_person(client, person)?;
    let mut list = read_friend_list(client).await?;
    let changed = add_friend(&mut list, &pubkey, name);
    if changed {
        let check = |current: &FriendList| {
            let friend = current.friends.iter().find(|f| f.pubkey == pubkey);
            friend.is_some_and(|f| name.is_none() || f.petname.as_deref() == name)
        };
        publish_checked(client, &list, check).await?;
    }
    println!("{}", json!({ "changed": changed, "pubkey": pubkey }));
    Ok(())
}

async fn cmd_remove(client: &KuzzClient, person: &str) -> Result<(), CliError> {
    let pubkey = parse_person(client, person)?;
    let mut list = read_friend_list(client).await?;
    let changed = remove_friend(&mut list, &pubkey);
    if changed {
        let check = |current: &FriendList| current.friends.iter().all(|f| f.pubkey != pubkey);
        publish_checked(client, &list, check).await?;
    }
    println!("{}", json!({ "changed": changed, "pubkey": pubkey }));
    Ok(())
}

pub async fn dispatch(cmd: FriendsCmd, client: &KuzzClient) -> Result<(), CliError> {
    run(cmd, client).await.map_err(classify_relay)
}

async fn run(cmd: FriendsCmd, client: &KuzzClient) -> Result<(), CliError> {
    match cmd {
        FriendsCmd::List { as_self: _ } => cmd_list(client).await,
        FriendsCmd::Add {
            as_self: _,
            person,
            name,
        } => cmd_add(client, &person, name.as_deref()).await,
        FriendsCmd::Remove { as_self: _, person } => cmd_remove(client, &person).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn stored() -> FriendList {
        let raw = json!([{
            "created_at": 100,
            "content": "{\"relay\":{}}",
            "tags": [
                ["p", A.to_uppercase(), "wss://relay", "Ann"],
                ["t", "kuzz"],
                ["auth", "owner", "condition", "signature"],
            ],
        }]);
        parse_friend_list(&raw.to_string()).unwrap()
    }

    #[test]
    fn the_list_is_read_from_the_primary() {
        assert_eq!(friend_list_filter(A)["consistency"], "strong");
    }

    #[test]
    fn an_empty_answer_is_an_empty_list() {
        assert_eq!(parse_friend_list("[]").unwrap(), FriendList::default());
    }

    #[test]
    fn an_unreadable_answer_is_an_error() {
        assert!(parse_friend_list("{\"error\":1}").is_err());
        assert!(parse_friend_list("[{\"tags\":\"x\"}]").is_err());
        assert!(parse_friend_list("[{\"tags\":[[\"p\"]]}]").is_err());
    }

    #[test]
    fn parsing_keeps_everything_but_the_auth_tag() {
        let list = stored();
        assert_eq!(list.created_at, Some(100));
        assert_eq!(list.content, "{\"relay\":{}}");
        assert_eq!(list.other_tags, vec![vec!["t".to_string(), "kuzz".into()]]);
        assert_eq!(
            list.friends,
            vec![Friend {
                pubkey: A.into(),
                relay_url: Some("wss://relay".into()),
                petname: Some("Ann".into()),
            }]
        );
    }

    #[test]
    fn adding_keeps_existing_friends() {
        let mut list = stored();
        assert!(add_friend(&mut list, B, Some("Bo")));
        assert_eq!(list.friends.len(), 2);
        assert_eq!(list.friends[0].relay_url.as_deref(), Some("wss://relay"));
    }

    #[test]
    fn adding_again_changes_nothing_unless_renamed() {
        let mut list = stored();
        assert!(!add_friend(&mut list, A, None));
        assert!(!add_friend(&mut list, A, Some("Ann")));
        assert!(add_friend(&mut list, A, Some("Annie")));
        assert_eq!(list.friends[0].petname.as_deref(), Some("Annie"));
        assert_eq!(list.friends[0].relay_url.as_deref(), Some("wss://relay"));
    }

    #[test]
    fn removing_someone_absent_changes_nothing() {
        let mut list = stored();
        assert!(!remove_friend(&mut list, B));
        assert!(remove_friend(&mut list, A));
        assert!(list.friends.is_empty());
    }
}
