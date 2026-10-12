use std::collections::HashMap;

use tauri::State;

use crate::{app_state::AppState, relay::query_relay_value};

/// Keyset cursor returned by the relay as `next`.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
pub struct InboxSummaryCursor {
    at_us: i64,
    key: String,
}

fn build_inbox_summary_filter(
    limit: u32,
    before: Option<&InboxSummaryCursor>,
    read: &HashMap<String, u64>,
) -> serde_json::Value {
    let mut filter = serde_json::json!({
        "inbox_summary": 1,
        "limit": limit.clamp(1, 100),
        "read": read,
    });
    if let Some(before) = before {
        filter["before"] = serde_json::json!(before);
    }
    filter
}

/// Fetch one inbox summary page over the `/query` bridge. `read` maps read
/// contexts (`<channel-id>`, `thread:<root-id>`) to unix seconds and is used by
/// the relay only to count unread messages for the returned rows.
#[tauri::command]
pub async fn get_inbox_summary(
    limit: Option<u32>,
    before: Option<InboxSummaryCursor>,
    read: Option<HashMap<String, u64>>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let filter = build_inbox_summary_filter(
        limit.unwrap_or(30),
        before.as_ref(),
        &read.unwrap_or_default(),
    );
    validated_summary(query_relay_value(&state, &[filter]).await?)
}

/// A relay without the envelope ignores the unknown keys and answers a plain
/// event array; reject anything that is not a version-1 summary so the
/// caller falls back to per-room loading.
fn validated_summary(response: serde_json::Value) -> Result<serde_json::Value, String> {
    let is_summary = response.get("inbox_summary") == Some(&serde_json::json!(1))
        && response
            .get("rows")
            .is_some_and(serde_json::Value::is_array);
    if is_summary {
        Ok(response)
    } else {
        Err("relay does not support inbox_summary".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_version_one_summaries_are_accepted() {
        let summary = serde_json::json!({"inbox_summary": 1, "rows": [], "next": null});
        assert!(validated_summary(summary).is_ok());
        assert!(validated_summary(serde_json::json!([])).is_err());
        assert!(validated_summary(serde_json::json!({"inbox_summary": 2, "rows": []})).is_err());
        assert!(validated_summary(serde_json::json!({"inbox_summary": 1})).is_err());
    }

    #[test]
    fn filter_carries_cursor_markers_and_bounded_limit() {
        let read = HashMap::from([("thread:ab".to_string(), 7u64)]);
        let cursor = InboxSummaryCursor {
            at_us: 42,
            key: "k".to_string(),
        };
        let filter = build_inbox_summary_filter(500, Some(&cursor), &read);
        assert_eq!(filter["inbox_summary"], 1);
        assert_eq!(filter["limit"], 100);
        assert_eq!(filter["read"]["thread:ab"], 7);
        assert_eq!(filter["before"]["at_us"], 42);
        assert_eq!(filter["before"]["key"], "k");
        assert!(build_inbox_summary_filter(30, None, &read)
            .get("before")
            .is_none());
    }
}
