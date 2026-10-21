use tauri::State;

use crate::{app_state::AppState, relay::query_relay};

const MAX_REPAIR_PAGE_LIMIT: u32 = 500;
const CHANNEL_REPAIR_KINDS: [u32; 15] = [
    5, 7, 9, 9005, 40001, 40002, 40003, 40008, 40099, 45001, 45003, 48100, 48101, 48102, 48103,
];

/// Where a repair page starts: sender time (`since`, keyset-paged by
/// `until` + `before_id`) or a `kuzz-seq` relay's channel order number
/// (`after_seq`, which the relay pages itself and closes with a 39006 bounds
/// event).
enum RepairCursor<'a> {
    Since {
        since: u64,
        until: Option<u64>,
        before_id: Option<&'a str>,
    },
    AfterSeq(u64),
}

fn repair_cursor<'a>(
    since: Option<u64>,
    after_seq: Option<u64>,
    until: Option<u64>,
    before_id: Option<&'a str>,
) -> Result<RepairCursor<'a>, String> {
    match (since, after_seq) {
        (Some(since), None) => Ok(RepairCursor::Since {
            since,
            until,
            before_id,
        }),
        (None, Some(after)) if until.is_none() && before_id.is_none() => {
            Ok(RepairCursor::AfterSeq(after))
        }
        (None, Some(_)) => Err("after_seq cannot be combined with until or before_id".to_string()),
        _ => Err("exactly one of since or after_seq is required".to_string()),
    }
}

fn build_channel_reconnect_repair_filter(
    channel_id: &str,
    cursor: RepairCursor<'_>,
    limit: u32,
) -> Result<serde_json::Value, String> {
    uuid::Uuid::parse_str(channel_id).map_err(|_| "invalid channel id".to_string())?;
    if limit == 0 || limit > MAX_REPAIR_PAGE_LIMIT {
        return Err(format!(
            "limit must be between 1 and {MAX_REPAIR_PAGE_LIMIT}"
        ));
    }
    let mut filter = serde_json::Map::new();
    filter.insert("#h".to_string(), serde_json::json!([channel_id]));
    filter.insert("kinds".to_string(), serde_json::json!(CHANNEL_REPAIR_KINDS));
    filter.insert("limit".to_string(), serde_json::json!(limit));
    let (since, until, before_id) = match cursor {
        RepairCursor::AfterSeq(after) => {
            filter.insert("after_seq".to_string(), serde_json::json!(after));
            return Ok(serde_json::Value::Object(filter));
        }
        RepairCursor::Since {
            since,
            until,
            before_id,
        } => (since, until, before_id),
    };
    if before_id.is_some() && until.is_none() {
        return Err("before_id requires until".to_string());
    }
    if let Some(event_id) = before_id {
        if event_id.len() != 64 || !event_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("before_id must be a 64-character hex event id".to_string());
        }
    }

    filter.insert("since".to_string(), serde_json::json!(since));
    if let Some(value) = until {
        filter.insert("until".to_string(), serde_json::json!(value));
    }
    if let Some(value) = before_id {
        filter.insert("before_id".to_string(), serde_json::json!(value));
    }
    Ok(serde_json::Value::Object(filter))
}

/// Fetch one lossless page for reconnect repair using a fixed channel-event
/// filter: a keyset page by time, or an `after_seq` page (rows in order-number
/// order followed by the relay's 39006 bounds event).
#[tauri::command]
pub async fn get_channel_reconnect_repair(
    channel_id: String,
    since: Option<u64>,
    after_seq: Option<u64>,
    limit: u32,
    until: Option<u64>,
    before_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let cursor = repair_cursor(since, after_seq, until, before_id.as_deref())?;
    let filter = build_channel_reconnect_repair_filter(&channel_id, cursor, limit)?;
    Ok(query_relay(&state, &[filter])
        .await?
        .iter()
        .filter_map(|event| serde_json::to_value(event).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_filter_is_fixed_and_keyset_scoped() {
        let id = "ab".repeat(32);
        let filter = build_channel_reconnect_repair_filter(
            "270f6caf-0feb-4055-93f3-cdbeb567ff28",
            repair_cursor(Some(100), None, Some(200), Some(&id)).expect("cursor"),
            500,
        )
        .expect("valid filter");
        assert_eq!(
            filter["#h"],
            serde_json::json!(["270f6caf-0feb-4055-93f3-cdbeb567ff28"])
        );
        assert_eq!(filter["kinds"], serde_json::json!(CHANNEL_REPAIR_KINDS));
        assert_eq!(filter["since"], 100);
        assert_eq!(filter["limit"], 500);
        assert_eq!(filter["until"], 200);
        assert_eq!(filter["before_id"], id);
        assert!(filter.get("top_level").is_none());
        assert!(filter.get("include_summaries").is_none());
        assert!(filter.get("include_aux").is_none());
    }

    #[test]
    fn repair_filter_rejects_renderer_escape_hatches() {
        let since = |before_id| repair_cursor(Some(0), None, Some(1), before_id).expect("cursor");
        assert!(build_channel_reconnect_repair_filter("not-a-channel", since(None), 1).is_err());
        assert!(build_channel_reconnect_repair_filter(
            "270f6caf-0feb-4055-93f3-cdbeb567ff28",
            since(None),
            0,
        )
        .is_err());
        assert!(build_channel_reconnect_repair_filter(
            "270f6caf-0feb-4055-93f3-cdbeb567ff28",
            since(Some("bad")),
            1,
        )
        .is_err());
        assert!(build_channel_reconnect_repair_filter(
            "270f6caf-0feb-4055-93f3-cdbeb567ff28",
            repair_cursor(Some(0), None, None, None).expect("cursor"),
            1,
        )
        .is_ok());
    }

    #[test]
    fn after_seq_filter_has_no_time_cursor() {
        let filter = build_channel_reconnect_repair_filter(
            "270f6caf-0feb-4055-93f3-cdbeb567ff28",
            repair_cursor(None, Some(42), None, None).expect("cursor"),
            500,
        )
        .expect("valid filter");
        assert_eq!(filter["after_seq"], 42);
        assert_eq!(filter["limit"], 500);
        assert_eq!(filter["kinds"], serde_json::json!(CHANNEL_REPAIR_KINDS));
        for field in ["since", "until", "before_id"] {
            assert!(filter.get(field).is_none(), "{field} must be absent");
        }
    }

    #[test]
    fn repair_cursor_needs_exactly_one_start() {
        let id = "ab".repeat(32);
        assert!(repair_cursor(None, None, None, None).is_err());
        assert!(repair_cursor(Some(1), Some(1), None, None).is_err());
        assert!(repair_cursor(None, Some(1), Some(5), None).is_err());
        assert!(repair_cursor(None, Some(1), None, Some(&id)).is_err());
    }
}
