//! `after_seq` filter extension: one channel's events after an order number,
//! in server acceptance order (`channel_sequence`), shared by WebSocket REQ
//! and HTTP `/query`.
//!
//! A page ends with one relay-signed `kind:39006` bounds event whose content
//! carries `last_seq` (the next `after_seq`), `has_more`, and the
//! `[event_id, seq]` pairs of the returned events, so HTTP clients — which
//! only receive raw signed events — still learn every order number.

use kuzz_core::StoredEvent;
use kuzz_db::EventQuery;
use serde_json::Value;

/// Page size when the filter has no `limit`.
pub(crate) const AFTER_SEQ_DEFAULT_LIMIT: i64 = 100;
/// Largest page an `after_seq` filter may request.
pub(crate) const AFTER_SEQ_MAX_LIMIT: i64 = 500;

/// Filter fields that page by sender time or by another mode; none of them
/// has a meaning next to an order-number cursor.
const CONFLICTING_FIELDS: [&str; 17] = [
    "since",
    "until",
    "before_id",
    "after_id",
    "search",
    "page",
    "feed_types",
    "depth_limit",
    "thread_window",
    // `/query` modes that answer before the order-number path runs.
    "artifact",
    "resolve_thread_roots",
    "inbox_summary",
    "read_state_snapshot",
    "chat_list",
    "chat_import",
    "mark_read",
    "thread_follow",
];

/// Read and validate the raw `after_seq` field of one filter.
///
/// `Ok(None)` when absent. The value must be a non-negative integer, the
/// filter must name exactly one `#h` channel, and it must not carry a time
/// cursor or another query mode. Channel access is the caller's check.
pub(crate) fn parse_after_seq(raw: &Value) -> Result<Option<i64>, String> {
    let Some(value) = raw.get("after_seq") else {
        return Ok(None);
    };
    let after = value
        .as_u64()
        .and_then(|n| i64::try_from(n).ok())
        .ok_or_else(|| "after_seq must be a non-negative integer".to_string())?;
    let one_channel = raw
        .get("#h")
        .and_then(Value::as_array)
        .filter(|values| values.len() == 1)
        .and_then(|values| values[0].as_str())
        .is_some_and(|value| value.parse::<uuid::Uuid>().is_ok());
    if !one_channel {
        return Err("after_seq requires exactly one #h channel".to_string());
    }
    if let Some(field) = CONFLICTING_FIELDS.iter().find(|f| raw.get(**f).is_some()) {
        return Err(format!("after_seq cannot be combined with {field}"));
    }
    // Presence is synthesized from Redis, never stored, so it has no order.
    let presence = [
        kuzz_core::kind::KIND_PRESENCE_UPDATE,
        kuzz_core::kind::KIND_PRESENCE_SNAPSHOT,
    ];
    if raw
        .get("kinds")
        .and_then(Value::as_array)
        .is_some_and(|kinds| {
            kinds
                .iter()
                .any(|k| k.as_u64().is_some_and(|k| presence.contains(&(k as u32))))
        })
    {
        return Err("after_seq cannot read presence kinds".to_string());
    }
    if raw.get("top_level").and_then(Value::as_bool) == Some(true) {
        return Err("after_seq on a top_level window requires order \"seq\"".to_string());
    }
    Ok(Some(after))
}

/// The page size for an `after_seq` filter.
pub(crate) fn page_limit(filter: &nostr::Filter) -> i64 {
    filter
        .limit
        .map_or(AFTER_SEQ_DEFAULT_LIMIT, |limit| {
            (limit as i64).min(AFTER_SEQ_MAX_LIMIT)
        })
        .max(1)
}

/// Turn a channel query into an order-number page with a `limit + 1` probe.
pub(crate) fn apply(query: &mut EventQuery, after: i64, limit: i64) {
    query.after_sequence = Some(after);
    query.order_by_sequence = true;
    query.limit = Some(limit + 1);
    query.max_limit = Some(limit + 1);
}

/// Exhaustion facts of one page, taken from the database rows before any
/// reader post-filter, so hidden rows still advance the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageBounds {
    /// The order number to send as the next `after_seq`.
    pub last_seq: i64,
    /// Whether rows exist past `last_seq`.
    pub has_more: bool,
}

/// Drop the probe row and report the page bounds.
///
/// `has_more` comes from the `limit + 1` probe. The database layer skips a
/// stored row it cannot decode (ingest validates events, so this should not
/// happen); such a skipped probe can report `false` early, but `last_seq`
/// never passes an unreturned row, so the next sync resumes without a gap.
pub(crate) fn finish(rows: &mut Vec<StoredEvent>, after: i64, limit: i64) -> PageBounds {
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);
    let last_seq = rows
        .iter()
        .filter_map(|row| row.channel_sequence)
        .max()
        .unwrap_or(after)
        .max(after);
    PageBounds { last_seq, has_more }
}

/// The relay-signed `kind:39006` bounds event that closes an `after_seq` page.
///
/// `d` = `<channel>:after_seq:<after>`; content =
/// `{"order":"seq","after_seq":N,"last_seq":M,"has_more":B,"sequences":[[id,seq],..]}`.
pub(crate) fn bounds_event(
    keys: &nostr::Keys,
    channel: uuid::Uuid,
    after: i64,
    bounds: PageBounds,
    sequences: &[(String, i64)],
) -> Result<nostr::Event, String> {
    let content = serde_json::json!({
        "order": "seq",
        "after_seq": after,
        "last_seq": bounds.last_seq,
        "has_more": bounds.has_more,
        "sequences": sequences,
    });
    let channel = channel.to_string();
    let tags = [
        nostr::Tag::parse(["d", &format!("{channel}:after_seq:{after}")]),
        nostr::Tag::parse(["h", &channel]),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("after_seq bounds tag: {e}"))?;
    nostr::EventBuilder::new(
        nostr::Kind::Custom(kuzz_core::kind::KIND_WINDOW_BOUNDS as u16),
        content.to_string(),
    )
    .tags(tags)
    .sign_with_keys(keys)
    .map_err(|e| format!("after_seq bounds sign: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CH: &str = "11111111-1111-4111-8111-111111111111";

    #[test]
    fn absent_after_seq_is_not_a_sequence_page() {
        assert_eq!(parse_after_seq(&json!({"kinds": [9]})), Ok(None));
    }

    #[test]
    fn accepts_one_channel_and_a_non_negative_integer() {
        assert_eq!(
            parse_after_seq(&json!({"#h": [CH], "after_seq": 0, "kinds": [9]})),
            Ok(Some(0))
        );
        assert_eq!(
            parse_after_seq(&json!({"#h": [CH], "after_seq": 42})),
            Ok(Some(42))
        );
    }

    #[test]
    fn rejects_invalid_values_and_combinations() {
        for raw in [
            json!({"#h": [CH], "after_seq": -1}),
            json!({"#h": [CH], "after_seq": 1.5}),
            json!({"#h": [CH], "after_seq": "3"}),
            json!({"after_seq": 3}),
            json!({"#h": [CH, "22222222-2222-4222-8222-222222222222"], "after_seq": 3}),
            json!({"#h": ["not-a-uuid"], "after_seq": 3}),
            json!({"#h": [CH], "after_seq": 3, "since": 1}),
            json!({"#h": [CH], "after_seq": 3, "until": 1}),
            json!({"#h": [CH], "after_seq": 3, "until": 1, "before_id": "00"}),
            json!({"#h": [CH], "after_seq": 3, "search": "x"}),
            json!({"#h": [CH], "after_seq": 3, "top_level": true}),
            json!({"#h": [CH], "after_seq": 3, "resolve_thread_roots": true}),
            json!({"#h": [CH], "after_seq": 3, "inbox_summary": true}),
            json!({"#h": [CH], "after_seq": 3, "chat_list": 1}),
            json!({"#h": [CH], "after_seq": 3, "artifact": {}}),
            json!({"#h": [CH], "after_seq": 3, "kinds": [9, 20001]}),
        ] {
            assert!(parse_after_seq(&raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn page_limit_defaults_and_caps() {
        let filter = nostr::Filter::new();
        assert_eq!(page_limit(&filter), AFTER_SEQ_DEFAULT_LIMIT);
        assert_eq!(
            page_limit(&filter.clone().limit(10_000)),
            AFTER_SEQ_MAX_LIMIT
        );
        assert_eq!(page_limit(&filter.clone().limit(0)), 1);
        assert_eq!(page_limit(&filter.limit(7)), 7);
    }

    #[test]
    fn probe_row_decides_has_more() {
        let keys = nostr::Keys::generate();
        let row = |seq: i64| {
            let event = nostr::EventBuilder::new(nostr::Kind::Custom(9), "x")
                .sign_with_keys(&keys)
                .unwrap();
            let mut stored = StoredEvent::with_received_at(event, chrono::Utc::now(), None, true);
            stored.channel_sequence = Some(seq);
            stored
        };
        let mut rows = vec![row(4), row(5), row(6)];
        assert_eq!(
            finish(&mut rows, 3, 2),
            PageBounds {
                last_seq: 5,
                has_more: true
            }
        );
        assert_eq!(rows.len(), 2);
        let mut rows = vec![row(4), row(5)];
        assert_eq!(
            finish(&mut rows, 3, 2),
            PageBounds {
                last_seq: 5,
                has_more: false
            }
        );
    }

    #[test]
    fn empty_page_keeps_the_requested_cursor() {
        let mut rows = Vec::new();
        assert_eq!(
            finish(&mut rows, 9, 5),
            PageBounds {
                last_seq: 9,
                has_more: false
            }
        );
    }
}
