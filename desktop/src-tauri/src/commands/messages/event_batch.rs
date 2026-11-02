use std::collections::HashSet;

use tauri::State;

use crate::{app_state::AppState, relay::query_relay};

// The relay clamps a single filter to this many events. Keep exact-ID reads in
// chunks so a large workflow list cannot silently lose late presentations.
const EVENT_QUERY_CHUNK_SIZE: usize = 1_000;

const GET_EVENT_KINDS: [u32; 15] = [
    0,
    1,
    3,
    5,
    7,
    9,
    30078,
    40002,
    40003,
    40008,
    40099,
    40100,
    45001,
    45003,
    kuzz_core_pkg::kind::KIND_HUDDLE_STARTED,
];

#[tauri::command]
pub async fn get_event(event_id: String, state: State<'_, AppState>) -> Result<String, String> {
    let events = query_relay(
        &state,
        &[serde_json::json!({
            "ids": [event_id],
            "kinds": GET_EVENT_KINDS,
            "limit": 1
        })],
    )
    .await?;

    let event = events
        .first()
        .ok_or_else(|| "event not found".to_string())?;
    serde_json::to_string(event).map_err(|error| format!("serialize event: {error}"))
}

/// The one `/query` for a thread's root
/// (docs/message-sync.md#opening-a-thread-from-its-root-id): the root by id,
/// and a deletion of it (an author's 5 or a removal 9005) in its room. Both
/// read the writer, so a root just sent is not missed on a lagging replica.
fn thread_root_filters(channel_id: &str, root_id: &str) -> [serde_json::Value; 2] {
    [
        serde_json::json!({
            "ids": [root_id],
            "kinds": crate::commands::channel_window::TIMELINE_KINDS,
            "limit": 1,
            "consistency": "strong",
        }),
        serde_json::json!({
            "kinds": [5, 9005],
            "#h": [channel_id],
            "#e": [root_id],
            "limit": 20,
            "consistency": "strong",
        }),
    ]
}

/// The relay's answer for a thread's root: the root and any deletion of it,
/// possibly empty. Asked as `account` on `relay_url`, the scope the thread
/// was opened in; refused once they are no longer the active ones. An error
/// means the request failed, never that the root is gone.
#[tauri::command]
pub async fn get_thread_root(
    channel_id: String,
    root_id: String,
    account: String,
    relay_url: String,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let credential = state.user_credential()?;
    let relay = crate::relay::relay_api_base_url_with_override(&state);
    let current = credential.public_key().to_hex();
    if !crate::commands::message_store_coverage::same_scope(&account, &relay_url, &current, &relay)
    {
        return Err("thread root asked for another account or server".to_string());
    }
    let events = crate::relay::query_relay_at_as(
        &state,
        &relay,
        &credential,
        &thread_root_filters(&channel_id, &root_id),
    )
    .await?;
    events
        .into_iter()
        .map(|event| {
            serde_json::to_value(event).map_err(|error| format!("serialize event: {error}"))
        })
        .collect()
}

/// Relay-signed excerpt of a quoted message the reader joined too late to
/// fetch (the room hides messages sent before they joined).
const KIND_QUOTE_PREVIEW: u16 = 39008;

fn build_quote_preview_filter(
    channel_id: &str,
    quoted_id: &str,
    quoting_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "quote_preview": true,
        "#h": [channel_id],
        "ids": [quoted_id],
        "#e": [quoting_id],
    })
}

/// Ask the relay for the quote preview of `quoted_id`, as seen from the
/// visible message `quoting_id` in `channel_id`. `None` when the relay has
/// no preview to give; the caller validates the event's tags and content.
#[tauri::command]
pub async fn get_quote_preview(
    channel_id: String,
    quoted_id: String,
    quoting_id: String,
    state: State<'_, AppState>,
) -> Result<Option<serde_json::Value>, String> {
    let filter = build_quote_preview_filter(&channel_id, &quoted_id, &quoting_id);
    let events = query_relay(&state, &[filter]).await?;
    events
        .into_iter()
        .find(|event| event.kind.as_u16() == KIND_QUOTE_PREVIEW)
        .map(|event| {
            serde_json::to_value(event).map_err(|error| format!("serialize event: {error}"))
        })
        .transpose()
}

/// Resolve many exact event IDs in relay-sized chunks. Callers still validate
/// event kind, channel scope, and requested ID before using presentation data.
fn normalized_event_id_chunks(event_ids: Vec<String>) -> Vec<Vec<String>> {
    let mut seen_ids = HashSet::new();
    let event_ids = event_ids
        .into_iter()
        .map(|event_id| event_id.trim().to_ascii_lowercase())
        .filter(|event_id| event_id.len() == 64 && event_id.chars().all(|c| c.is_ascii_hexdigit()))
        .filter(|event_id| seen_ids.insert(event_id.clone()))
        .collect::<Vec<_>>();
    event_ids
        .chunks(EVENT_QUERY_CHUNK_SIZE)
        .map(<[String]>::to_vec)
        .collect()
}

#[tauri::command]
pub async fn get_events(
    event_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let event_id_chunks = normalized_event_id_chunks(event_ids);
    if event_id_chunks.is_empty() {
        return Ok(Vec::new());
    }

    let mut events_by_id = std::collections::HashMap::new();
    for event_ids in event_id_chunks {
        let events = query_relay(
            &state,
            &[serde_json::json!({
                "ids": event_ids,
                "kinds": GET_EVENT_KINDS,
                "limit": event_ids.len()
            })],
        )
        .await?;
        for event in events {
            events_by_id.entry(event.id).or_insert(event);
        }
    }

    events_by_id
        .into_values()
        .map(|event| {
            serde_json::to_value(event).map_err(|error| format!("serialize event: {error}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_preview_filter_carries_extension_and_both_ids() {
        let filter = build_quote_preview_filter("chan", "quoted", "quoting");
        assert_eq!(filter["quote_preview"], serde_json::json!(true));
        assert_eq!(filter["#h"], serde_json::json!(["chan"]));
        assert_eq!(filter["ids"], serde_json::json!(["quoted"]));
        assert_eq!(filter["#e"], serde_json::json!(["quoting"]));
    }

    #[test]
    fn thread_root_filters_ask_root_and_its_deletions_from_the_writer() {
        let [root, deletions] = thread_root_filters("room", "root");
        assert_eq!(root["ids"], serde_json::json!(["root"]));
        assert_eq!(
            root["kinds"],
            serde_json::json!(crate::commands::channel_window::TIMELINE_KINDS)
        );
        assert!(root.get("#h").is_none());
        assert_eq!(deletions["kinds"], serde_json::json!([5, 9005]));
        assert_eq!(deletions["#h"], serde_json::json!(["room"]));
        assert_eq!(deletions["#e"], serde_json::json!(["root"]));
        for filter in [&root, &deletions] {
            assert_eq!(filter["consistency"], serde_json::json!("strong"));
        }
    }

    #[test]
    fn keeps_exact_relay_ceiling_in_one_chunk() {
        let chunks = normalized_event_id_chunks(
            (0..EVENT_QUERY_CHUNK_SIZE)
                .map(|index| format!("{index:064x}"))
                .collect(),
        );

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), EVENT_QUERY_CHUNK_SIZE);
    }

    #[test]
    fn normalizes_deduplicates_and_keeps_ids_beyond_relay_ceiling() {
        let last_id = format!("{:064x}", EVENT_QUERY_CHUNK_SIZE);
        let mut event_ids = (0..=EVENT_QUERY_CHUNK_SIZE)
            .map(|index| format!("{index:064X}"))
            .collect::<Vec<_>>();
        event_ids.extend(["not-an-event-id".to_string(), format!(" {last_id} ")]);

        let chunks = normalized_event_id_chunks(event_ids);

        assert_eq!(chunks.iter().map(Vec::len).sum::<usize>(), 1_001);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), EVENT_QUERY_CHUNK_SIZE);
        assert_eq!(chunks[1], [last_id]);
    }
}
