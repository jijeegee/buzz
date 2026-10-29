//! Which received events the store keeps and how it files them. The cases
//! live in `test-fixtures/message-store.json`, shared with mobile.

use serde_json::Value;

/// Room content kinds: rows on a room timeline or in a thread.
const CONTENT_KINDS: &[u64] = &[
    9, 40001, 40002, 40008, 40099, 43001, 43002, 43003, 43004, 43005, 43006, 45001, 45003, 48100,
    48101, 48102, 48103,
];
/// Overlays on another event, kept by the ids they reference.
const AUX_KINDS: &[u64] = &[5, 7, 9005, 40003];
const THREAD_SUMMARY: u64 = 39005;

/// A received event as the store files it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Filed {
    Event(EventRow),
    /// The latest 39005 of a thread replaces the previous one.
    Summary(SummaryRow),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EventRow {
    pub id: String,
    pub kind: u64,
    pub pubkey: String,
    pub created_at: i64,
    pub room: Option<String>,
    pub root: Option<String>,
    pub broadcast: bool,
    pub targets: Vec<String>,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SummaryRow {
    pub root: String,
    pub room: Option<String>,
    pub created_at: i64,
    pub raw: String,
}

fn tags(event: &Value) -> impl Iterator<Item = &Vec<Value>> {
    event
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
}

fn tag_value(tag: &[Value], index: usize) -> Option<&str> {
    tag.get(index).and_then(Value::as_str)
}

fn first_tag(event: &Value, name: &str) -> Option<String> {
    tags(event)
        .find(|tag| tag_value(tag, 0) == Some(name))
        .and_then(|tag| tag_value(tag, 1))
        .map(str::to_owned)
}

/// The thread a reply belongs to, by the same rule as the clients'
/// `getThreadReference`: the last `reply`-marked `e` tag is the parent and
/// the `root`-marked one (else the parent) the root. No reply marker means
/// top-level.
fn thread_root(event: &Value) -> Option<String> {
    let e_tags: Vec<&Vec<Value>> = tags(event)
        .filter(|tag| tag_value(tag, 0) == Some("e") && tag_value(tag, 1).is_some())
        .collect();
    let parent = e_tags
        .iter()
        .rev()
        .find(|tag| tag_value(tag, 3) == Some("reply"))
        .and_then(|tag| tag_value(tag, 1))?;
    let root = e_tags
        .iter()
        .find(|tag| tag_value(tag, 3) == Some("root"))
        .and_then(|tag| tag_value(tag, 1))
        .unwrap_or(parent);
    Some(root.to_owned())
}

/// Files `event`, or `None` when the store does not keep it.
pub(crate) fn classify(event: &Value) -> Option<Filed> {
    let id = event.get("id")?.as_str()?.to_owned();
    let kind = event.get("kind")?.as_u64()?;
    let created_at = event.get("created_at")?.as_i64()?;
    let room = first_tag(event, "h");
    let raw = event.to_string();
    if kind == THREAD_SUMMARY {
        let root = first_tag(event, "e")?;
        return Some(Filed::Summary(SummaryRow {
            root,
            room,
            created_at,
            raw,
        }));
    }
    let aux = AUX_KINDS.contains(&kind);
    if !aux && !CONTENT_KINDS.contains(&kind) {
        return None;
    }
    // A reaction or deletion may name only its target; anything else
    // without a room cannot be placed.
    if room.is_none() && !matches!(kind, 5 | 7) {
        return None;
    }
    let targets = if aux {
        tags(event)
            .filter(|tag| tag_value(tag, 0) == Some("e"))
            .filter_map(|tag| tag_value(tag, 1).map(str::to_owned))
            .collect()
    } else {
        Vec::new()
    };
    Some(Filed::Event(EventRow {
        id,
        kind,
        pubkey: event.get("pubkey")?.as_str()?.to_owned(),
        created_at,
        room,
        root: if aux { None } else { thread_root(event) },
        broadcast: tags(event)
            .any(|tag| tag_value(tag, 0) == Some("broadcast") && tag_value(tag, 1) == Some("1")),
        targets,
        raw,
    }))
}
