//! Opening a room from the store: its newest window of top-level rows, by
//! the relay's channel-window rules, and whether that window is certain.
//! Rules: docs/message-sync.md#opening-a-room-from-the-store; shared cases:
//! test-fixtures/message-store-room-head.json.

use std::collections::HashSet;

use rusqlite::{params, params_from_iter, Connection};
use serde_json::{json, Value};

use super::coverage::ROOM_STREAM;

/// Receive time can trail `created_at` by this much (relay future tolerance
/// plus storage fence), so coverage must begin this far before a row.
pub(crate) const ROW_RECEIVE_MARGIN_SECS: i64 = 900 + 960 + 5;

/// Overlays returned with the rows that they reference.
const AUX_KINDS: &str = "5, 7, 9005, 40003";
/// Overlays returned with the overlays that they reference.
const AUX_REMOVAL_KINDS: &str = "5, 9005";

/// A room's newest window as the store has it.
#[derive(Debug, Default)]
pub(crate) struct RoomHead {
    /// Every event of the window is known stored and the room's live REQ
    /// is still receiving, so the window equals what the relay would send.
    pub certain: bool,
    pub rows: Vec<Value>,
    pub aux: Vec<Value>,
    pub summaries: Vec<Value>,
    /// `(created_at, id)` of the oldest row.
    pub next_cursor: Option<(i64, String)>,
}

impl RoomHead {
    /// The window in the shape of a relay channel-window response: rows,
    /// overlays, summaries and one `39006` bounds event for `room`'s head.
    pub(crate) fn into_response(self, room: &str) -> Vec<Value> {
        let next_cursor = self
            .next_cursor
            .map(|(created_at, id)| json!({ "created_at": created_at, "id": id }));
        let bounds = json!({
            "id": "0".repeat(64),
            "pubkey": "0".repeat(64),
            "sig": "0".repeat(128),
            "kind": 39006,
            "created_at": 0,
            "tags": [["d", format!("{}:head", room.to_ascii_lowercase())], ["h", room]],
            "content": json!({ "has_more": next_cursor.is_some(), "next_cursor": next_cursor })
                .to_string(),
        });
        let mut events = self.rows;
        events.extend(self.aux);
        events.extend(self.summaries);
        events.push(bounds);
        events
    }
}

pub(super) fn parse(raw: String) -> Option<Value> {
    serde_json::from_str(&raw).ok()
}

/// The parent of a reply: its last `reply`-marked `e` tag.
fn parent_of(event: &Value) -> Option<&str> {
    event
        .get("tags")?
        .as_array()?
        .iter()
        .rev()
        .filter_map(Value::as_array)
        .find(|tag| {
            tag.first().and_then(Value::as_str) == Some("e")
                && tag.get(3).and_then(Value::as_str) == Some("reply")
        })
        .and_then(|tag| tag.get(1)?.as_str())
}

/// Whether a stored deletion removes `id` (by `author`): an admin removal
/// (9005, accepted by the relay only when allowed) or the author's own 5.
pub(super) fn is_deleted(conn: &Connection, id: &str, author: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare_cached(
        "SELECT 1 FROM targets t JOIN events e ON e.id = t.id WHERE t.target = ?1 \
         AND (e.kind = 9005 OR (e.kind = 5 AND e.pubkey = ?2)) LIMIT 1",
    )?;
    statement.exists(params![id, author])
}

fn overlays(conn: &Connection, targets: &[String], kinds: &str) -> rusqlite::Result<Vec<Value>> {
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let marks = vec!["?"; targets.len()].join(", ");
    let mut statement = conn.prepare(&format!(
        "SELECT DISTINCT e.id, e.raw FROM targets t JOIN events e ON e.id = t.id \
         WHERE t.target IN ({marks}) AND e.kind IN ({kinds}) ORDER BY e.created_at, e.id"
    ))?;
    let rows = statement.query_map(params_from_iter(targets), |row| row.get::<_, String>(1))?;
    Ok(rows.filter_map(|raw| raw.ok().and_then(parse)).collect())
}

fn id_of(event: &Value) -> String {
    event
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// `room`'s newest `limit` top-level rows of `kinds`, with their overlays
/// and thread summaries. `live_at` is when the room's live REQ reached EOSE
/// on the current connection, if it is live.
pub(crate) fn room_head(
    conn: &Connection,
    room: &str,
    kinds: &[u64],
    limit: usize,
    live_at: Option<i64>,
) -> rusqlite::Result<RoomHead> {
    let mut head = RoomHead::default();
    if kinds.is_empty() || limit == 0 {
        return Ok(head);
    }
    let kind_list = kinds
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut candidates = conn.prepare(&format!(
        "SELECT id, pubkey, created_at, root, raw FROM events \
         WHERE room = ?1 AND kind IN ({kind_list}) AND (root IS NULL OR broadcast = 1) \
         ORDER BY created_at DESC, id ASC"
    ))?;
    let mut found = candidates.query(params![room])?;
    let mut row_ids = Vec::new();
    while let Some(row) = found.next()? {
        let (id, author, created_at): (String, String, i64) =
            (row.get(0)?, row.get(1)?, row.get(2)?);
        let root: Option<String> = row.get(3)?;
        let Some(event) = parse(row.get(4)?) else {
            continue;
        };
        // A broadcast reply is a row only as a direct reply to the root.
        if root.is_some() && parent_of(&event) != root.as_deref() {
            continue;
        }
        if is_deleted(conn, &id, &author)? {
            continue;
        }
        head.next_cursor = Some((created_at, id.clone()));
        row_ids.push(id);
        head.rows.push(event);
        if head.rows.len() == limit {
            break;
        }
    }
    drop(found);

    head.aux = overlays(conn, &row_ids, AUX_KINDS)?;
    let aux_ids: Vec<String> = head.aux.iter().map(id_of).collect();
    let seen: HashSet<String> = aux_ids.iter().cloned().collect();
    for removal in overlays(conn, &aux_ids, AUX_REMOVAL_KINDS)? {
        if !seen.contains(&id_of(&removal)) {
            head.aux.push(removal);
        }
    }

    if !row_ids.is_empty() {
        let marks = vec!["?"; row_ids.len()].join(", ");
        let mut statement = conn.prepare(&format!(
            "SELECT raw FROM summaries WHERE root IN ({marks}) ORDER BY root"
        ))?;
        let summaries = statement.query_map(params_from_iter(&row_ids), |row| row.get(0))?;
        head.summaries = summaries
            .filter_map(|raw| raw.ok().and_then(parse))
            .collect();
    }

    head.certain = match (live_at, &head.next_cursor) {
        (Some(live_at), Some((oldest, _))) if head.rows.len() == limit => conn
            .prepare_cached(
                "SELECT 1 FROM coverage WHERE room = ?1 AND stream = ?2 \
                 AND lo_at <= ?3 AND hi_at >= ?4 LIMIT 1",
            )?
            .exists(params![
                room,
                ROOM_STREAM,
                oldest - ROW_RECEIVE_MARGIN_SECS,
                live_at
            ])?,
        _ => false,
    };
    Ok(head)
}
