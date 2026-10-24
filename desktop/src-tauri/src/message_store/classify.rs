//! Event → stored role, from its kind and tags.

use serde_json::Value;

use super::{is_hex64, SYNC_KINDS};

pub(crate) const KIND_THREAD_SUMMARY: u32 = 39005;
const BOUNDS_KINDS: [u32; 2] = [39006, 39007];
const AUX_KINDS: [u32; 4] = [5, 7, 9005, 40003];
pub(crate) const DELETE_KINDS: [u32; 2] = [5, 9005];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    TopLevel = 0,
    ThreadReply = 1,
    BroadcastReply = 2,
    /// Reaction, edit or delete: overlays the events it targets.
    Aux = 3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredEvent {
    pub(crate) id: String,
    pub(crate) channel_id: String,
    pub(crate) created_at: i64,
    pub(crate) kind: u32,
    pub(crate) pubkey: String,
    pub(crate) role: Role,
    pub(crate) root_id: Option<String>,
    pub(crate) parent_id: Option<String>,
    /// Every `e` tag of an aux event.
    pub(crate) targets: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Classified {
    /// Not kept: unknown kind, bounds marker, malformed, or no room.
    Skip,
    /// A thread summary (39005) for `root_id`; newest wins.
    Summary {
        root_id: String,
        channel_id: String,
        created_at: i64,
    },
    Event(StoredEvent),
}

fn tags(event: &Value) -> Vec<Vec<&str>> {
    event
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_array)
                .map(|tag| tag.iter().map(|part| part.as_str().unwrap_or("")).collect())
                .collect()
        })
        .unwrap_or_default()
}

fn hex_id(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| is_hex64(value))
        .map(str::to_ascii_lowercase)
}

/// Same rule as the webview's `getThreadReference`: the last `reply`-marked
/// `e` tag is the parent; the `root`-marked one (else the parent) is the root.
fn thread_reference(tags: &[Vec<&str>]) -> (Option<String>, Option<String>) {
    let e_tags: Vec<&Vec<&str>> = tags
        .iter()
        .filter(|tag| tag.first() == Some(&"e"))
        .collect();
    let parent = e_tags
        .iter()
        .rev()
        .find(|tag| tag.get(3) == Some(&"reply"))
        .and_then(|tag| hex_id(tag.get(1).copied()));
    let Some(parent) = parent else {
        return (None, None);
    };
    let root = e_tags
        .iter()
        .find(|tag| tag.get(3) == Some(&"root"))
        .and_then(|tag| hex_id(tag.get(1).copied()))
        .unwrap_or_else(|| parent.clone());
    (Some(root), Some(parent))
}

/// Classifies one relay event. `fallback_channel` places events whose wire
/// tags carry no `h` (the relay scopes some reactions by their stored room).
pub(crate) fn classify(event: &Value, fallback_channel: Option<&str>) -> Classified {
    let (Some(id), Some(pubkey), Some(created_at), Some(kind)) = (
        hex_id(event.get("id").and_then(Value::as_str)),
        hex_id(event.get("pubkey").and_then(Value::as_str)),
        event.get("created_at").and_then(Value::as_i64),
        event
            .get("kind")
            .and_then(Value::as_u64)
            .and_then(|kind| u32::try_from(kind).ok()),
    ) else {
        return Classified::Skip;
    };
    if BOUNDS_KINDS.contains(&kind) {
        return Classified::Skip;
    }
    let tags = tags(event);
    let channel_id = tags
        .iter()
        .find(|tag| tag.first() == Some(&"h"))
        .and_then(|tag| tag.get(1).copied())
        .filter(|channel| !channel.is_empty())
        .or(fallback_channel);
    let Some(channel_id) = channel_id.map(str::to_string) else {
        return Classified::Skip;
    };
    let first_e = || {
        tags.iter()
            .filter(|tag| tag.first() == Some(&"e"))
            .find_map(|tag| hex_id(tag.get(1).copied()))
    };
    if kind == KIND_THREAD_SUMMARY {
        return match first_e() {
            Some(root_id) => Classified::Summary {
                root_id,
                channel_id,
                created_at,
            },
            None => Classified::Skip,
        };
    }
    if !SYNC_KINDS.contains(&kind) {
        return Classified::Skip;
    }
    let mut stored = StoredEvent {
        id,
        channel_id,
        created_at,
        kind,
        pubkey,
        role: Role::TopLevel,
        root_id: None,
        parent_id: None,
        targets: Vec::new(),
    };
    if AUX_KINDS.contains(&kind) {
        stored.role = Role::Aux;
        for tag in tags.iter().filter(|tag| tag.first() == Some(&"e")) {
            if let Some(target) = hex_id(tag.get(1).copied()) {
                if !stored.targets.contains(&target) {
                    stored.targets.push(target);
                }
            }
        }
        return Classified::Event(stored);
    }
    let (root_id, parent_id) = thread_reference(&tags);
    if parent_id.is_some() {
        let broadcast = tags
            .iter()
            .any(|tag| tag.first() == Some(&"broadcast") && tag.get(1) == Some(&"1"));
        stored.role = if broadcast {
            Role::BroadcastReply
        } else {
            Role::ThreadReply
        };
    }
    stored.root_id = root_id;
    stored.parent_id = parent_id;
    Classified::Event(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn id(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    fn event(kind: u32, tags: Value) -> Value {
        json!({"id": id(1), "pubkey": id(2), "created_at": 10, "kind": kind, "tags": tags, "content": "x", "sig": ""})
    }

    fn stored(classified: Classified) -> StoredEvent {
        match classified {
            Classified::Event(stored) => stored,
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn top_level_message() {
        let stored = stored(classify(&event(9, json!([["h", "room"]])), None));
        assert_eq!(stored.role, Role::TopLevel);
        assert_eq!(stored.channel_id, "room");
        assert_eq!(stored.root_id, None);
    }

    #[test]
    fn thread_reply_and_broadcast_reply() {
        let tags = json!([
            ["h", "room"],
            ["e", id(3), "", "root"],
            ["e", id(4), "", "reply"]
        ]);
        let reply = stored(classify(&event(9, tags), None));
        assert_eq!(reply.role, Role::ThreadReply);
        assert_eq!(reply.root_id, Some(id(3)));
        assert_eq!(reply.parent_id, Some(id(4)));

        let tags = json!([["h", "room"], ["e", id(3), "", "reply"], ["broadcast", "1"]]);
        let broadcast = stored(classify(&event(40002, tags), None));
        assert_eq!(broadcast.role, Role::BroadcastReply);
        assert_eq!(broadcast.root_id, Some(id(3)));
    }

    #[test]
    fn unmarked_e_tag_is_not_a_reply() {
        let tags = json!([["h", "room"], ["e", id(3)]]);
        assert_eq!(
            stored(classify(&event(43002, tags), None)).role,
            Role::TopLevel
        );
    }

    #[test]
    fn delete_targets_every_e_tag() {
        let tags = json!([
            ["h", "room"],
            ["e", id(3)],
            ["e", id(4)],
            ["e", id(3)],
            ["e", "bad"]
        ]);
        let delete = stored(classify(&event(5, tags), None));
        assert_eq!(delete.role, Role::Aux);
        assert_eq!(delete.targets, vec![id(3), id(4)]);
    }

    #[test]
    fn reaction_without_h_uses_the_fetched_room() {
        let tags = json!([["e", id(3)]]);
        assert_eq!(classify(&event(7, tags.clone()), None), Classified::Skip);
        assert_eq!(
            stored(classify(&event(7, tags), Some("room"))).channel_id,
            "room"
        );
    }

    #[test]
    fn summaries_bounds_and_unknown_kinds() {
        let summary = classify(&event(39005, json!([["h", "room"], ["e", id(3)]])), None);
        assert!(matches!(summary, Classified::Summary { ref root_id, .. } if *root_id == id(3)));
        assert_eq!(
            classify(&event(39006, json!([["h", "room"]])), None),
            Classified::Skip
        );
        assert_eq!(
            classify(&event(30078, json!([["h", "room"]])), None),
            Classified::Skip
        );
        let mut malformed = event(9, json!([["h", "room"]]));
        malformed["id"] = json!("short");
        assert_eq!(classify(&malformed, None), Classified::Skip);
    }
}
