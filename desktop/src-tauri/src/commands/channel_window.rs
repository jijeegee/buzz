use tauri::State;

use crate::{app_state::AppState, models::ChannelPageCursor, relay::query_relay};

const TIMELINE_KINDS: [u32; 11] = [
    9,
    40002,
    40008,
    40099,
    43001,
    43002,
    43003,
    43004,
    43005,
    43006,
    kuzz_core_pkg::kind::KIND_HUDDLE_STARTED,
];

fn build_channel_window_filter(
    channel_id: &str,
    cap: u32,
    cursor: Option<&ChannelPageCursor>,
) -> serde_json::Value {
    let mut filter = serde_json::Map::new();
    filter.insert("#h".to_string(), serde_json::json!([channel_id]));
    filter.insert("kinds".to_string(), serde_json::json!(TIMELINE_KINDS));
    filter.insert("limit".to_string(), serde_json::json!(cap));
    filter.insert("top_level".to_string(), serde_json::json!(true));
    filter.insert("include_summaries".to_string(), serde_json::json!(true));
    filter.insert("include_aux".to_string(), serde_json::json!(true));
    if let Some(cursor) = cursor {
        filter.insert("until".to_string(), serde_json::json!(cursor.created_at));
        filter.insert("before_id".to_string(), serde_json::json!(cursor.event_id));
    }
    serde_json::Value::Object(filter)
}

/// Fetch one server-assembled channel window over the existing `/query` bridge.
#[tauri::command]
pub async fn get_channel_window(
    channel_id: String,
    limit_rows: Option<u32>,
    cursor: Option<ChannelPageCursor>,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let filter = build_channel_window_filter(
        &channel_id,
        limit_rows.unwrap_or(50).min(200),
        cursor.as_ref(),
    );
    Ok(query_relay(&state, &[filter])
        .await?
        .iter()
        .filter_map(|event| serde_json::to_value(event).ok())
        .collect())
}

/// Most rooms one batched window request may carry.
const MAX_BATCH_CHANNELS: usize = 20;
/// Kind of the relay-signed overlay that closes every channel window.
const KIND_WINDOW_BOUNDS: u16 = 39006;

#[derive(serde::Serialize)]
pub struct ChannelWindowBatchEntry {
    channel_id: String,
    events: Vec<serde_json::Value>,
}

/// Split one multi-window `/query` response into per-channel windows.
///
/// The relay serves window filters in request order and closes each window
/// with exactly one `39006` bounds overlay tagged with its channel; an
/// inaccessible channel emits nothing at all. So every run of events up to a
/// bounds overlay belongs to that overlay's channel. Trailing events with no
/// closing overlay, or windows for channels not asked for, are dropped.
fn split_channel_windows(
    events: Vec<nostr::Event>,
    channel_ids: &[String],
) -> Vec<ChannelWindowBatchEntry> {
    let mut entries = Vec::new();
    let mut segment = Vec::new();
    for event in events {
        let bounds_channel = (event.kind.as_u16() == KIND_WINDOW_BOUNDS)
            .then(|| {
                event
                    .tags
                    .iter()
                    .find(|tag| tag.as_slice().first().map(String::as_str) == Some("h"))
                    .and_then(|tag| tag.as_slice().get(1).cloned())
            })
            .flatten();
        if let Ok(value) = serde_json::to_value(&event) {
            segment.push(value);
        }
        let Some(channel_id) = bounds_channel else {
            continue;
        };
        let events = std::mem::take(&mut segment);
        if channel_ids.contains(&channel_id) {
            entries.push(ChannelWindowBatchEntry { channel_id, events });
        }
    }
    entries
}

/// Fetch the newest window of several channels in one `/query` round trip.
/// Channels the relay skips (no access) are absent from the result.
#[tauri::command]
pub async fn get_channel_windows(
    channel_ids: Vec<String>,
    limit_rows: Option<u32>,
    state: State<'_, AppState>,
) -> Result<Vec<ChannelWindowBatchEntry>, String> {
    let mut seen = std::collections::HashSet::new();
    let channel_ids: Vec<String> = channel_ids
        .into_iter()
        .filter(|channel_id| seen.insert(channel_id.clone()))
        .take(MAX_BATCH_CHANNELS)
        .collect();
    if channel_ids.is_empty() {
        return Ok(Vec::new());
    }
    let cap = limit_rows.unwrap_or(50).min(200);
    let filters: Vec<_> = channel_ids
        .iter()
        .map(|channel_id| build_channel_window_filter(channel_id, cap, None))
        .collect();
    let events = query_relay(&state, &filters).await?;
    Ok(split_channel_windows(events, &channel_ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn event(keys: &Keys, kind: u16, channel: &str) -> nostr::Event {
        EventBuilder::new(Kind::Custom(kind), "")
            .tags(vec![Tag::parse(["h", channel]).unwrap()])
            .sign_with_keys(keys)
            .unwrap()
    }

    fn ids(entry: &ChannelWindowBatchEntry) -> Vec<String> {
        entry
            .events
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn splits_each_window_at_its_bounds_overlay() {
        let keys = Keys::generate();
        let a1 = event(&keys, 9, "a");
        let a_bounds = event(&keys, KIND_WINDOW_BOUNDS, "a");
        // An aux event without the room tag still belongs to the open window.
        let b_aux = EventBuilder::new(Kind::Custom(5), "")
            .sign_with_keys(&keys)
            .unwrap();
        let b1 = event(&keys, 9, "b");
        let b_bounds = event(&keys, KIND_WINDOW_BOUNDS, "b");
        let stray = event(&keys, 9, "c");
        let entries = split_channel_windows(
            vec![
                a1.clone(),
                a_bounds.clone(),
                b1.clone(),
                b_aux.clone(),
                b_bounds.clone(),
                stray,
            ],
            &["a".to_owned(), "b".to_owned(), "c".to_owned()],
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].channel_id, "a");
        assert_eq!(ids(&entries[0]), vec![a1.id.to_hex(), a_bounds.id.to_hex()]);
        assert_eq!(entries[1].channel_id, "b");
        assert_eq!(
            ids(&entries[1]),
            vec![b1.id.to_hex(), b_aux.id.to_hex(), b_bounds.id.to_hex()]
        );
    }

    #[test]
    fn drops_windows_for_channels_not_requested() {
        let keys = Keys::generate();
        let entries = split_channel_windows(
            vec![event(&keys, 9, "x"), event(&keys, KIND_WINDOW_BOUNDS, "x")],
            &["a".to_owned()],
        );
        assert!(entries.is_empty());
    }
}
