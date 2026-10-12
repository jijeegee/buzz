use tauri::State;

use crate::{app_state::AppState, models::ThreadCursor, relay::query_relay};

/// Conversation kinds a thread window may page (NIP-CW thread mode).
const THREAD_ROW_KINDS: [u32; 2] = [9, 40002];
/// The relay serves at most four thread windows per `/query`.
const MAX_BATCH_THREADS: usize = 4;
/// Kind of the relay-signed overlay that closes every thread window.
const KIND_THREAD_WINDOW_BOUNDS: u16 = 39007;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadWindowRequest {
    channel_id: String,
    root_id: String,
    /// Oldest reply already loaded; absent loads the newest page.
    cursor: Option<ThreadCursor>,
}

#[derive(serde::Serialize, Debug, PartialEq)]
pub struct ThreadWindowPage {
    channel_id: String,
    root_id: String,
    events: Vec<serde_json::Value>,
    has_more: bool,
    /// Cursor for the next older page, when `has_more`.
    next_cursor: Option<ThreadCursor>,
}

fn build_thread_window_filter(request: &ThreadWindowRequest, limit: u32) -> serde_json::Value {
    let mut filter = serde_json::json!({
        "thread_window": true,
        "#h": [request.channel_id],
        "#e": [request.root_id],
        "kinds": THREAD_ROW_KINDS,
        "limit": limit,
        // Like the legacy subtree read, rows only: reactions and edits reach
        // thread rows through their own paths.
        "include_aux": false,
    });
    if let Some(cursor) = &request.cursor {
        filter["until"] = serde_json::json!(cursor.created_at);
        filter["before_id"] = serde_json::json!(cursor.event_id);
    }
    filter
}

fn tag_value(event: &nostr::Event, name: &str) -> Option<String> {
    event
        .tags
        .iter()
        .find(|tag| tag.as_slice().first().map(String::as_str) == Some(name))
        .and_then(|tag| tag.as_slice().get(1).cloned())
}

/// Split one multi-thread `/query` response into per-thread pages.
///
/// The relay serves thread windows in request order and closes each with one
/// `39007` bounds overlay tagged with its channel and root; a thread the
/// reader cannot see emits nothing. Every run of events up to a bounds overlay
/// therefore belongs to that overlay's thread.
fn split_thread_windows(
    events: Vec<nostr::Event>,
    requests: &[ThreadWindowRequest],
) -> Vec<ThreadWindowPage> {
    let mut pages = Vec::new();
    let mut segment = Vec::new();
    for event in events {
        if event.kind.as_u16() != KIND_THREAD_WINDOW_BOUNDS {
            if let Ok(value) = serde_json::to_value(&event) {
                segment.push(value);
            }
            continue;
        }
        let events = std::mem::take(&mut segment);
        let (Some(channel_id), Some(root_id)) = (tag_value(&event, "h"), tag_value(&event, "e"))
        else {
            continue;
        };
        if !requests
            .iter()
            .any(|r| r.channel_id == channel_id && r.root_id == root_id)
        {
            continue;
        }
        let bounds: serde_json::Value =
            serde_json::from_str(&event.content).unwrap_or(serde_json::Value::Null);
        let has_more = bounds["has_more"].as_bool().unwrap_or(false);
        let next_cursor = bounds["next_cursor"].as_object().and_then(|cursor| {
            Some(ThreadCursor {
                created_at: cursor.get("created_at")?.as_i64()?,
                event_id: cursor.get("id")?.as_str()?.to_owned(),
            })
        });
        pages.push(ThreadWindowPage {
            channel_id,
            root_id,
            events,
            has_more: has_more && next_cursor.is_some(),
            next_cursor: if has_more { next_cursor } else { None },
        });
    }
    pages
}

/// Fetch newest-first reply pages for up to four threads in one `/query`
/// round trip. Threads the relay skips (no access) are absent from the result.
#[tauri::command]
pub async fn get_thread_windows(
    threads: Vec<ThreadWindowRequest>,
    limit_rows: Option<u32>,
    state: State<'_, AppState>,
) -> Result<Vec<ThreadWindowPage>, String> {
    if threads.len() > MAX_BATCH_THREADS {
        return Err(format!(
            "at most {MAX_BATCH_THREADS} thread windows per request"
        ));
    }
    if threads.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit_rows.unwrap_or(50).clamp(1, 200);
    let filters: Vec<_> = threads
        .iter()
        .map(|thread| build_thread_window_filter(thread, limit))
        .collect();
    let events = query_relay(&state, &filters).await?;
    Ok(split_thread_windows(events, &threads))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn reply(keys: &Keys, channel: &str) -> nostr::Event {
        EventBuilder::new(Kind::Custom(9), "")
            .tags(vec![Tag::parse(["h", channel]).unwrap()])
            .sign_with_keys(keys)
            .unwrap()
    }

    fn bounds(keys: &Keys, channel: &str, root: &str, content: &str) -> nostr::Event {
        EventBuilder::new(Kind::Custom(KIND_THREAD_WINDOW_BOUNDS), content)
            .tags(vec![
                Tag::parse(["h", channel]).unwrap(),
                Tag::parse(["e", root]).unwrap(),
            ])
            .sign_with_keys(keys)
            .unwrap()
    }

    fn request(channel: &str, root: &str) -> ThreadWindowRequest {
        ThreadWindowRequest {
            channel_id: channel.to_owned(),
            root_id: root.to_owned(),
            cursor: None,
        }
    }

    #[test]
    fn splits_each_thread_at_its_bounds_with_paging_state() {
        let keys = Keys::generate();
        let a1 = reply(&keys, "c");
        let b1 = reply(&keys, "c");
        let pages = split_thread_windows(
            vec![
                a1.clone(),
                bounds(
                    &keys,
                    "c",
                    "ra",
                    r#"{"has_more":true,"next_cursor":{"created_at":5,"id":"ab"}}"#,
                ),
                b1.clone(),
                bounds(&keys, "c", "rb", r#"{"has_more":false,"next_cursor":null}"#),
            ],
            &[request("c", "ra"), request("c", "rb")],
        );
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].root_id, "ra");
        assert_eq!(pages[0].events.len(), 1);
        assert_eq!(pages[0].events[0]["id"], a1.id.to_hex());
        assert!(pages[0].has_more);
        assert_eq!(
            pages[0].next_cursor,
            Some(ThreadCursor {
                created_at: 5,
                event_id: "ab".to_owned()
            })
        );
        assert_eq!(pages[1].root_id, "rb");
        assert_eq!(pages[1].events[0]["id"], b1.id.to_hex());
        assert!(!pages[1].has_more);
        assert_eq!(pages[1].next_cursor, None);
    }

    #[test]
    fn drops_threads_not_requested() {
        let keys = Keys::generate();
        let pages = split_thread_windows(
            vec![
                reply(&keys, "c"),
                bounds(&keys, "c", "rx", r#"{"has_more":false}"#),
            ],
            &[request("c", "ra")],
        );
        assert!(pages.is_empty());
    }
}
