//! NIP-01 client/relay message parsing and formatting.

use kuzz_core::event::StoredEvent;
use nostr::{Event, Filter};
use serde_json::Value;

use crate::error::{RelayError, Result};

/// NIP-11 advertised limit: subscription IDs longer than this are rejected.
const MAX_SUB_ID_LENGTH: usize = 256;

/// NIP-11 advertised limit: REQ messages with more filters than this are rejected.
const MAX_FILTERS_PER_REQ: usize = 10;

/// Most values one filter may list in `ids`, `authors`, `kinds`, or any single
/// `#tag`. Bounds the SQL `IN` lists a single filter can force. The desktop
/// batches its largest list (user-status authors) at exactly this size.
pub(crate) const MAX_FILTER_LIST_VALUES: usize = 1_000;

/// Rejects a filter whose `ids`, `authors`, `kinds`, or any `#tag` list holds
/// more than [`MAX_FILTER_LIST_VALUES`] values. Shared by the WebSocket
/// REQ/COUNT parser and the HTTP `/query` and `/count` bridge.
pub(crate) fn check_filter_list_limits(filters: &[Filter]) -> std::result::Result<(), String> {
    for filter in filters {
        let lists = [
            ("ids", filter.ids.as_ref().map_or(0, |v| v.len())),
            ("authors", filter.authors.as_ref().map_or(0, |v| v.len())),
            ("kinds", filter.kinds.as_ref().map_or(0, |v| v.len())),
        ]
        .into_iter()
        .chain(filter.generic_tags.values().map(|v| ("#tag", v.len())));
        for (name, len) in lists {
            if len > MAX_FILTER_LIST_VALUES {
                return Err(format!(
                    "filter {name} lists {len} values, maximum is {MAX_FILTER_LIST_VALUES}"
                ));
            }
        }
    }
    Ok(())
}

/// A message sent by a NIP-01 client to the relay.
#[derive(Debug, Clone)]
pub enum ClientMessage {
    /// An EVENT message submitting a signed Nostr event.
    Event(Event),
    /// A REQ message opening a subscription with one or more filters.
    Req {
        /// The client-assigned subscription identifier.
        sub_id: String,
        /// The filters that determine which events are delivered.
        filters: Vec<Filter>,
        /// Optional per-filter composite cursor tiebreaks from raw extension fields.
        before_ids: Vec<Option<Vec<u8>>>,
        /// Optional per-filter `after_seq` order-number cursors (one channel).
        after_seqs: Vec<Option<i64>>,
    },
    /// A CLOSE message cancelling an active subscription.
    Close(String),
    /// A COUNT message requesting aggregate counts (NIP-45).
    Count {
        /// The client-assigned subscription identifier.
        sub_id: String,
        /// The filters to count against.
        filters: Vec<Filter>,
    },
    /// An AUTH message responding to a NIP-42 challenge.
    Auth(Event),
    /// `["AUTH", {"token": "…"}]` — centralized-identity token AUTH. Only
    /// produced when [`ParseMode::token_auth`] is set.
    AuthToken(kuzz_auth::TokenSecret),
    /// An EVENT body from a token-authenticated connection: an unsigned
    /// draft the server stamps. Only produced when [`ParseMode::drafts`] is set.
    EventDraft(Value),
}

/// Which centralized-identity frame shapes [`ClientMessage::parse_with`]
/// recognises. The default (all off) parses exactly like the key-auth relay.
#[derive(Debug, Clone, Copy, Default)]
pub struct ParseMode {
    /// Recognise `["AUTH", {"token": "…"}]` (token auth enabled).
    pub token_auth: bool,
    /// Treat every EVENT body as a draft (connection authenticated by token).
    pub drafts: bool,
}

/// Artifact queries are HTTP-only; reject rather than silently drop their
/// predicates on the generic WebSocket path.
fn reject_artifact_query_filters(filters: &[serde_json::Value]) -> Result<()> {
    use kuzz_core::artifact::{route_filter, FilterRoute};
    for filter in filters {
        match route_filter(filter) {
            FilterRoute::Generic => {}
            FilterRoute::Artifact => {
                return Err(RelayError::InvalidMessage(
                    "artifact queries require HTTP /query or /count".into(),
                ))
            }
            FilterRoute::Rejected(reason) => return Err(RelayError::InvalidMessage(reason.into())),
        }
    }
    Ok(())
}

impl ClientMessage {
    /// Parse a raw JSON WebSocket frame into a [`ClientMessage`].
    pub fn parse(raw: &str) -> Result<Self> {
        Self::parse_with(raw, ParseMode::default())
    }

    /// Parse a frame, additionally recognising the token-auth shapes enabled
    /// in `mode`.
    pub fn parse_with(raw: &str, mode: ParseMode) -> Result<Self> {
        let value: Value = serde_json::from_str(raw)
            .map_err(|e| RelayError::InvalidMessage(format!("JSON parse error: {e}")))?;

        let arr = value
            .as_array()
            .ok_or_else(|| RelayError::InvalidMessage("expected JSON array".to_string()))?;

        if arr.is_empty() {
            return Err(RelayError::InvalidMessage("empty array".to_string()));
        }

        let msg_type = arr[0].as_str().ok_or_else(|| {
            RelayError::InvalidMessage("first element must be a string".to_string())
        })?;

        match msg_type {
            "EVENT" => {
                if arr.len() < 2 {
                    return Err(RelayError::InvalidMessage(
                        "EVENT requires event object".to_string(),
                    ));
                }
                if mode.drafts {
                    return Ok(ClientMessage::EventDraft(arr[1].clone()));
                }
                let event: Event = serde_json::from_value(arr[1].clone())
                    .map_err(|e| RelayError::InvalidMessage(format!("invalid event: {e}")))?;
                Ok(ClientMessage::Event(event))
            }
            "REQ" => {
                if arr.len() < 2 {
                    return Err(RelayError::InvalidMessage(
                        "REQ requires sub_id".to_string(),
                    ));
                }
                let sub_id = arr[1]
                    .as_str()
                    .ok_or_else(|| {
                        RelayError::InvalidMessage("REQ sub_id must be a string".to_string())
                    })?
                    .to_string();
                if sub_id.is_empty() {
                    return Err(RelayError::InvalidMessage(
                        "REQ sub_id must not be empty".to_string(),
                    ));
                }
                // Enforce NIP-11 advertised max_subid_length: 256
                if sub_id.len() > MAX_SUB_ID_LENGTH {
                    return Err(RelayError::InvalidMessage(format!(
                        "REQ sub_id exceeds maximum length of {MAX_SUB_ID_LENGTH} bytes"
                    )));
                }
                let filter_values = &arr[2..];
                reject_artifact_query_filters(filter_values)?;

                // Enforce NIP-11 advertised max_filters: 10
                if filter_values.len() > MAX_FILTERS_PER_REQ {
                    return Err(RelayError::InvalidMessage(format!(
                        "REQ contains {} filters, maximum is {MAX_FILTERS_PER_REQ}",
                        filter_values.len()
                    )));
                }
                let filters: Vec<Filter> = filter_values
                    .iter()
                    .map(|v| {
                        serde_json::from_value(v.clone())
                            .map_err(|e| RelayError::InvalidMessage(format!("invalid filter: {e}")))
                    })
                    .collect::<Result<Vec<_>>>()?;
                check_filter_list_limits(&filters).map_err(RelayError::InvalidMessage)?;
                let before_ids = filter_values
                    .iter()
                    .map(|value| {
                        let Some(raw) = value.get("before_id") else {
                            return Ok(None);
                        };
                        if value.get("until").is_none() {
                            return Err(RelayError::InvalidMessage(
                                "before_id requires until to be set".to_string(),
                            ));
                        }
                        let Some(hex) = raw.as_str() else {
                            return Err(RelayError::InvalidMessage(
                                "before_id must be a 64-char hex event id".to_string(),
                            ));
                        };
                        let bytes = hex::decode(hex).map_err(|_| {
                            RelayError::InvalidMessage(
                                "before_id must be a 64-char hex event id".to_string(),
                            )
                        })?;
                        if bytes.len() != 32 {
                            return Err(RelayError::InvalidMessage(
                                "before_id must be a 64-char hex event id".to_string(),
                            ));
                        }
                        Ok(Some(bytes))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let after_seqs = filter_values
                    .iter()
                    .map(|value| {
                        crate::handlers::sequence_page::parse_after_seq(value)
                            .map_err(RelayError::InvalidMessage)
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(ClientMessage::Req {
                    sub_id,
                    filters,
                    before_ids,
                    after_seqs,
                })
            }
            "COUNT" => {
                if arr.len() < 2 {
                    return Err(RelayError::InvalidMessage(
                        "COUNT requires sub_id".to_string(),
                    ));
                }
                let sub_id = arr[1]
                    .as_str()
                    .ok_or_else(|| {
                        RelayError::InvalidMessage("COUNT sub_id must be a string".to_string())
                    })?
                    .to_string();
                if sub_id.is_empty() {
                    return Err(RelayError::InvalidMessage(
                        "COUNT sub_id must not be empty".to_string(),
                    ));
                }
                if sub_id.len() > MAX_SUB_ID_LENGTH {
                    return Err(RelayError::InvalidMessage(format!(
                        "COUNT sub_id exceeds maximum length of {MAX_SUB_ID_LENGTH} bytes"
                    )));
                }
                let filter_values = &arr[2..];
                reject_artifact_query_filters(filter_values)?;

                if filter_values.len() > MAX_FILTERS_PER_REQ {
                    return Err(RelayError::InvalidMessage(format!(
                        "COUNT contains {} filters, maximum is {MAX_FILTERS_PER_REQ}",
                        filter_values.len()
                    )));
                }
                let filters: Vec<Filter> = filter_values
                    .iter()
                    .map(|v| {
                        serde_json::from_value(v.clone())
                            .map_err(|e| RelayError::InvalidMessage(format!("invalid filter: {e}")))
                    })
                    .collect::<Result<Vec<_>>>()?;
                check_filter_list_limits(&filters).map_err(RelayError::InvalidMessage)?;
                Ok(ClientMessage::Count { sub_id, filters })
            }
            "CLOSE" => {
                if arr.len() < 2 {
                    return Err(RelayError::InvalidMessage(
                        "CLOSE requires sub_id".to_string(),
                    ));
                }
                let sub_id = arr[1]
                    .as_str()
                    .ok_or_else(|| {
                        RelayError::InvalidMessage("CLOSE sub_id must be a string".to_string())
                    })?
                    .to_string();
                Ok(ClientMessage::Close(sub_id))
            }
            "AUTH" => {
                if arr.len() < 2 {
                    return Err(RelayError::InvalidMessage(
                        "AUTH requires event object".to_string(),
                    ));
                }
                if mode.token_auth {
                    if let Some(token) = arr[1]
                        .as_object()
                        .filter(|obj| !obj.contains_key("kind"))
                        .and_then(|obj| obj.get("token"))
                    {
                        let token = token.as_str().ok_or_else(|| {
                            RelayError::InvalidMessage("AUTH token must be a string".to_string())
                        })?;
                        return Ok(ClientMessage::AuthToken(kuzz_auth::TokenSecret::new(
                            token.to_owned(),
                        )));
                    }
                }
                let event: Event = serde_json::from_value(arr[1].clone())
                    .map_err(|e| RelayError::InvalidMessage(format!("invalid auth event: {e}")))?;
                Ok(ClientMessage::Auth(event))
            }
            other => Err(RelayError::InvalidMessage(format!(
                "unknown message type: {other}"
            ))),
        }
    }
}

/// Helpers for formatting NIP-01 relay-to-client messages as JSON strings.
pub struct RelayMessage;

/// The trailing object opted-in (`?kuzz_seq=1`) connections get on EVENT and
/// OK frames. NIP-01 parsers ignore extra trailing elements.
pub(crate) fn sequence_extra(channel_sequence: i64) -> Value {
    serde_json::json!({ "channel_sequence": channel_sequence })
}

impl RelayMessage {
    /// Format an AUTH challenge message.
    pub fn auth_challenge(challenge: &str) -> String {
        serde_json::json!(["AUTH", challenge]).to_string()
    }

    /// Format an EVENT message delivering an event to a subscriber.
    pub fn event(sub_id: &str, event: &Event) -> String {
        let event_json = serde_json::to_value(event)
            .expect("SAFETY: nostr::Event serialization is infallible for well-formed events");
        serde_json::json!(["EVENT", sub_id, event_json]).to_string()
    }

    /// Format a NOTICE message with a human-readable string.
    pub fn notice(message: &str) -> String {
        serde_json::json!(["NOTICE", message]).to_string()
    }

    /// Format an EOSE (End of Stored Events) message for a subscription.
    pub fn eose(sub_id: &str) -> String {
        serde_json::json!(["EOSE", sub_id]).to_string()
    }

    /// Format an EVENT message for a stored event. Connections that opted in
    /// with `?kuzz_seq=1` get the channel order number as a 4th element when
    /// it is known; everything else gets the plain NIP-01 frame.
    pub fn stored_event(sub_id: &str, stored: &StoredEvent, kuzz_seq: bool) -> String {
        match stored.channel_sequence.filter(|_| kuzz_seq) {
            Some(sequence) => {
                let event_json = serde_json::to_value(&stored.event).expect(
                    "SAFETY: nostr::Event serialization is infallible for well-formed events",
                );
                serde_json::json!(["EVENT", sub_id, event_json, sequence_extra(sequence)])
                    .to_string()
            }
            None => Self::event(sub_id, &stored.event),
        }
    }

    /// Format an OK message acknowledging an EVENT submission.
    pub fn ok(event_id: &str, accepted: bool, message: &str) -> String {
        serde_json::json!(["OK", event_id, accepted, message]).to_string()
    }

    /// Format an OK message carrying the channel order number as a 4th
    /// element, for connections that opted in with `?kuzz_seq=1`.
    pub fn ok_with_sequence(
        event_id: &str,
        accepted: bool,
        message: &str,
        channel_sequence: i64,
    ) -> String {
        serde_json::json!([
            "OK",
            event_id,
            accepted,
            message,
            sequence_extra(channel_sequence)
        ])
        .to_string()
    }

    /// Format a CLOSED message indicating a subscription was terminated by the relay.
    pub fn closed(sub_id: &str, message: &str) -> String {
        serde_json::json!(["CLOSED", sub_id, message]).to_string()
    }

    /// Format a COUNT response (NIP-45).
    pub fn count(sub_id: &str, count: u64) -> String {
        serde_json::json!(["COUNT", sub_id, {"count": count}]).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::RelayMessage;
    use kuzz_core::event::StoredEvent;
    use nostr::JsonUtil as _;

    fn signed_event() -> nostr::Event {
        nostr::EventBuilder::text_note("hi")
            .sign_with_keys(&nostr::Keys::generate())
            .expect("sign")
    }

    #[test]
    fn legacy_event_and_ok_frames_are_unchanged() {
        let event = signed_event();
        // `RelayMessage::event` serializes through `Value` (sorted keys).
        let event_json = serde_json::to_value(&event).unwrap().to_string();
        let mut stored = StoredEvent::new(event.clone(), Some(uuid::Uuid::new_v4()));
        stored.channel_sequence = Some(7);
        let legacy = format!(r#"["EVENT","s",{event_json}]"#);
        // Not opted in: byte-identical to the plain frame even with a sequence.
        assert_eq!(RelayMessage::stored_event("s", &stored, false), legacy);
        assert_eq!(RelayMessage::event("s", &event), legacy);
        assert_eq!(
            RelayMessage::ok("ab", true, "duplicate: already have this event"),
            r#"["OK","ab",true,"duplicate: already have this event"]"#
        );
        // Opted in but no known sequence: still the plain frame.
        stored.channel_sequence = None;
        assert_eq!(RelayMessage::stored_event("s", &stored, true), legacy);
    }

    #[test]
    fn opted_in_frames_carry_the_sequence_as_a_fourth_element() {
        let event = signed_event();
        // `RelayMessage::event` serializes through `Value` (sorted keys).
        let event_json = serde_json::to_value(&event).unwrap().to_string();
        let mut stored = StoredEvent::new(event, Some(uuid::Uuid::new_v4()));
        stored.channel_sequence = Some(42);
        assert_eq!(
            RelayMessage::stored_event("s", &stored, true),
            format!(r#"["EVENT","s",{event_json},{{"channel_sequence":42}}]"#)
        );
        assert_eq!(
            RelayMessage::ok_with_sequence("ab", true, "", 42),
            r#"["OK","ab",true,"",{"channel_sequence":42}]"#
        );
    }

    /// kuzz-acp and other Rust clients parse with the `nostr` crate; its
    /// RelayMessage parser must tolerate the trailing object.
    #[test]
    fn nostr_crate_parses_frames_with_a_fourth_element() {
        let event = signed_event();
        let mut stored = StoredEvent::new(event.clone(), Some(uuid::Uuid::new_v4()));
        stored.channel_sequence = Some(9);
        match nostr::RelayMessage::from_json(RelayMessage::stored_event("s", &stored, true))
            .expect("4-element EVENT parses")
        {
            nostr::RelayMessage::Event {
                subscription_id,
                event: parsed,
            } => {
                assert_eq!(subscription_id.as_str(), "s");
                assert_eq!(parsed.id, event.id);
            }
            other => panic!("expected EVENT, got {other:?}"),
        }
        let ok = RelayMessage::ok_with_sequence(&event.id.to_hex(), true, "", 9);
        match nostr::RelayMessage::from_json(ok).expect("5-element OK parses") {
            nostr::RelayMessage::Ok {
                event_id, status, ..
            } => {
                assert_eq!(event_id, event.id);
                assert!(status);
            }
            other => panic!("expected OK, got {other:?}"),
        }
    }

    #[test]
    fn non_artifact_extensions_keep_existing_ws_behavior() {
        for command in ["REQ", "COUNT"] {
            let request = serde_json::json!([command, "projects", {"kinds":[30621],"#kuzz-channel":["channel"]}]);
            assert!(super::ClientMessage::parse(&request.to_string()).is_ok());
        }
    }

    #[test]
    fn artifact_live_filters_preserved_extensions_rejected() {
        let id = "f".repeat(64);
        for request in [
            r##"["REQ","ar",{"kinds":[45010,45011],"#h":["channel"]}]"##.to_owned(),
            format!(r##"["REQ","ar",{{"ids":["{id}"],"#h":["channel"]}}]"##),
        ] {
            assert!(super::ClientMessage::parse(&request).is_ok(), "{request}");
        }
        for request in [
            r##"["REQ","ar",{"artifact":"history","#d":["artifact"]}]"##.to_owned(),
            r##"["REQ","ar",{"kinds":[45010],"#project":["project"]}]"##.to_owned(),
            r##"["COUNT","ar",{"artifact":"current"}]"##.to_owned(),
            // No `kinds` may match artifacts, so the predicate cannot be dropped.
            format!(r##"["REQ","ar",{{"ids":["{id}"],"#project":["P"]}}]"##),
            format!(r##"["COUNT","ar",{{"ids":["{id}"],"#project":["P"]}}]"##),
        ] {
            assert!(super::ClientMessage::parse(&request).is_err(), "{request}");
        }
    }

    use super::*;
    use kuzz_core::test_helpers::make_event;
    use nostr::{EventBuilder, Keys, Kind};

    fn make_auth_event(keys: &Keys, challenge: &str, relay: &str) -> Event {
        let url: nostr::RelayUrl = relay.parse().expect("url");
        EventBuilder::auth(challenge, url)
            .sign_with_keys(keys)
            .expect("sign")
    }

    // Type alias to avoid clippy::type_complexity warning on the test case table.
    // The tuple holds: raw JSON string + a boxed checker closure.
    type ParseCase<'a> = (&'a str, Box<dyn Fn(ClientMessage)>);

    #[test]
    fn parse_valid_messages() {
        let keys = Keys::generate();
        let event = make_event(Kind::TextNote);
        let auth_event = make_auth_event(&keys, "challenge", "wss://relay.example.com");
        let filter = Filter::new().kind(Kind::TextNote);

        let cases: &[ParseCase<'_>] = &[
            (
                &serde_json::json!(["EVENT", serde_json::to_value(&event).unwrap()]).to_string(),
                Box::new(move |m| match m {
                    ClientMessage::Event(e) => assert_eq!(e.id, event.id),
                    _ => panic!("expected Event"),
                }),
            ),
            (
                &serde_json::json!(["REQ", "sub1", serde_json::to_value(&filter).unwrap()])
                    .to_string(),
                Box::new(|m| match m {
                    ClientMessage::Req {
                        sub_id, filters, ..
                    } => {
                        assert_eq!(sub_id, "sub1");
                        assert_eq!(filters.len(), 1);
                    }
                    _ => panic!("expected Req"),
                }),
            ),
            (
                r#"["CLOSE", "sub1"]"#,
                Box::new(|m| match m {
                    ClientMessage::Close(id) => assert_eq!(id, "sub1"),
                    _ => panic!("expected Close"),
                }),
            ),
            (
                &serde_json::json!(["AUTH", serde_json::to_value(&auth_event).unwrap()])
                    .to_string(),
                Box::new(move |m| match m {
                    ClientMessage::Auth(e) => assert_eq!(e.id, auth_event.id),
                    _ => panic!("expected Auth"),
                }),
            ),
        ];

        for (raw, check) in cases {
            let msg = ClientMessage::parse(raw).expect("parse");
            check(msg);
        }
    }

    #[test]
    fn parse_req_multiple_filters() {
        let f1 = Filter::new().kind(Kind::TextNote);
        let f2 = Filter::new().kind(Kind::Metadata);
        let raw = serde_json::json!([
            "REQ",
            "sub2",
            serde_json::to_value(&f1).unwrap(),
            serde_json::to_value(&f2).unwrap()
        ])
        .to_string();
        match ClientMessage::parse(&raw).unwrap() {
            ClientMessage::Req {
                sub_id, filters, ..
            } => {
                assert_eq!(sub_id, "sub2");
                assert_eq!(filters.len(), 2);
            }
            _ => panic!("expected Req"),
        }
    }

    #[test]
    fn parse_req_composite_cursor_preserves_filter_alignment() {
        let raw = serde_json::json!([
            "REQ",
            "sub-cursor",
            { "kinds": [9] },
            {
                "kinds": [48100],
                "until": 1_000,
                "before_id": "ab".repeat(32),
            }
        ])
        .to_string();

        match ClientMessage::parse(&raw).unwrap() {
            ClientMessage::Req {
                filters,
                before_ids,
                ..
            } => {
                assert_eq!(filters.len(), 2);
                assert_eq!(before_ids, vec![None, Some(vec![0xab; 32])]);
            }
            _ => panic!("expected Req"),
        }
    }

    #[test]
    fn parse_req_composite_cursor_rejects_invalid_pairs() {
        for raw in [
            serde_json::json!(["REQ", "sub", { "before_id": "ab".repeat(32) }]),
            serde_json::json!(["REQ", "sub", {
                "until": 1_000,
                "before_id": "short",
            }]),
        ] {
            assert!(matches!(
                ClientMessage::parse(&raw.to_string()),
                Err(RelayError::InvalidMessage(_))
            ));
        }
    }

    #[test]
    fn parse_invalid_messages() {
        let cases = [
            ("not json", "JSON"),
            ("[]", "empty"),
            (r#"["UNKNOWN", "data"]"#, "unknown"),
            (r#"["EVENT"]"#, "EVENT requires"),
            (r#"["REQ"]"#, "REQ requires"),
            (r#"["REQ", ""]"#, "must not be empty"),
        ];

        for (raw, hint) in cases {
            let err = ClientMessage::parse(raw).unwrap_err();
            assert!(
                matches!(err, RelayError::InvalidMessage(_)),
                "expected InvalidMessage for {raw:?}, got {err:?}"
            );
            let _ = hint; // used for readability only
        }
    }

    #[test]
    fn parse_req_sub_id_too_long_is_rejected() {
        let long_id = "x".repeat(MAX_SUB_ID_LENGTH + 1);
        let raw = serde_json::json!(["REQ", long_id]).to_string();
        let err = ClientMessage::parse(&raw).unwrap_err();
        assert!(
            matches!(err, RelayError::InvalidMessage(_)),
            "expected InvalidMessage for oversized sub_id, got {err:?}"
        );
    }

    #[test]
    fn parse_req_too_many_filters_is_rejected() {
        let filter = Filter::new().kind(Kind::TextNote);
        let filter_val = serde_json::to_value(&filter).unwrap();
        let mut arr: Vec<serde_json::Value> = vec![
            serde_json::Value::String("REQ".to_string()),
            serde_json::Value::String("sub3".to_string()),
        ];
        for _ in 0..=MAX_FILTERS_PER_REQ {
            arr.push(filter_val.clone());
        }
        let raw = serde_json::Value::Array(arr).to_string();
        let err = ClientMessage::parse(&raw).unwrap_err();
        assert!(
            matches!(err, RelayError::InvalidMessage(_)),
            "expected InvalidMessage for too many filters, got {err:?}"
        );
    }

    #[test]
    fn parse_req_exactly_max_filters_is_accepted() {
        let filter = Filter::new().kind(Kind::TextNote);
        let filter_val = serde_json::to_value(&filter).unwrap();
        let mut arr: Vec<serde_json::Value> = vec![
            serde_json::Value::String("REQ".to_string()),
            serde_json::Value::String("sub4".to_string()),
        ];
        for _ in 0..MAX_FILTERS_PER_REQ {
            arr.push(filter_val.clone());
        }
        let raw = serde_json::Value::Array(arr).to_string();
        assert!(
            ClientMessage::parse(&raw).is_ok(),
            "exactly {MAX_FILTERS_PER_REQ} filters should be accepted"
        );
    }

    fn authors_value(count: usize) -> serde_json::Value {
        let authors: Vec<String> = (0..count).map(|i| format!("{i:064x}")).collect();
        serde_json::json!({ "authors": authors })
    }

    #[test]
    fn parse_req_and_count_reject_oversized_filter_lists() {
        for verb in ["REQ", "COUNT"] {
            let at_limit =
                serde_json::json!([verb, "s", authors_value(MAX_FILTER_LIST_VALUES)]).to_string();
            assert!(
                ClientMessage::parse(&at_limit).is_ok(),
                "{verb}: exactly {MAX_FILTER_LIST_VALUES} authors should be accepted"
            );

            let over = serde_json::json!([verb, "s", authors_value(MAX_FILTER_LIST_VALUES + 1)])
                .to_string();
            let err = ClientMessage::parse(&over).unwrap_err();
            assert!(
                matches!(&err, RelayError::InvalidMessage(m) if m.contains("authors")),
                "{verb}: expected authors limit rejection, got {err:?}"
            );
        }
    }

    #[test]
    fn filter_list_limit_covers_ids_kinds_and_tags() {
        let ids: Vec<String> = (0..=MAX_FILTER_LIST_VALUES)
            .map(|i| format!("{i:064x}"))
            .collect();
        let kinds: Vec<u16> = (0..=MAX_FILTER_LIST_VALUES as u16).collect();
        let tags: Vec<String> = (0..=MAX_FILTER_LIST_VALUES)
            .map(|i| i.to_string())
            .collect();
        for (name, value) in [
            ("ids", serde_json::json!({ "ids": ids })),
            ("kinds", serde_json::json!({ "kinds": kinds })),
            ("#tag", serde_json::json!({ "#t": tags })),
        ] {
            let filter: Filter = serde_json::from_value(value).unwrap();
            let err = check_filter_list_limits(&[filter]).unwrap_err();
            assert!(err.contains(name), "{name}: {err}");
        }
    }

    // Type alias to avoid clippy::type_complexity warning on the format test table.
    type FormatCase<'a> = (&'a str, Box<dyn Fn()>);

    #[test]
    fn format_relay_messages() {
        let event = make_event(Kind::TextNote);

        let cases: &[FormatCase<'_>] = &[
            (
                "auth_challenge",
                Box::new(|| {
                    let msg = RelayMessage::auth_challenge("abc123");
                    let v: Value = serde_json::from_str(&msg).unwrap();
                    assert_eq!(v[0], "AUTH");
                    assert_eq!(v[1], "abc123");
                }),
            ),
            (
                "event",
                Box::new({
                    let event = event.clone();
                    move || {
                        let msg = RelayMessage::event("sub1", &event);
                        let v: Value = serde_json::from_str(&msg).unwrap();
                        assert_eq!(v[0], "EVENT");
                        assert_eq!(v[1], "sub1");
                        assert_eq!(v[2]["id"], event.id.to_hex());
                    }
                }),
            ),
            (
                "notice",
                Box::new(|| {
                    let msg = RelayMessage::notice("hello");
                    let v: Value = serde_json::from_str(&msg).unwrap();
                    assert_eq!(v[0], "NOTICE");
                    assert_eq!(v[1], "hello");
                }),
            ),
            (
                "eose",
                Box::new(|| {
                    let msg = RelayMessage::eose("sub1");
                    let v: Value = serde_json::from_str(&msg).unwrap();
                    assert_eq!(v[0], "EOSE");
                    assert_eq!(v[1], "sub1");
                }),
            ),
            (
                "ok_accepted",
                Box::new(|| {
                    let msg = RelayMessage::ok("eid", true, "");
                    let v: Value = serde_json::from_str(&msg).unwrap();
                    assert_eq!(v[0], "OK");
                    assert_eq!(v[2], true);
                    assert_eq!(v[3], "");
                }),
            ),
            (
                "ok_rejected",
                Box::new(|| {
                    let msg = RelayMessage::ok("eid", false, "auth-required");
                    let v: Value = serde_json::from_str(&msg).unwrap();
                    assert_eq!(v[2], false);
                    assert_eq!(v[3], "auth-required");
                }),
            ),
            (
                "closed",
                Box::new(|| {
                    let msg = RelayMessage::closed("sub1", "auth-required: not authenticated");
                    let v: Value = serde_json::from_str(&msg).unwrap();
                    assert_eq!(v[0], "CLOSED");
                    assert_eq!(v[1], "sub1");
                    assert_eq!(v[2], "auth-required: not authenticated");
                }),
            ),
        ];

        for (name, check) in cases {
            let _ = name;
            check();
        }
    }
}
