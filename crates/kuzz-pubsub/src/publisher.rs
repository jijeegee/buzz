//! Event publishing — PUBLISH to Redis via pool connection.

use kuzz_core::TenantContext;
use deadpool_redis::Pool;
use nostr::JsonUtil;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::PubSubError;
use crate::topic::{self, EventTopic};

/// Version tag of the event envelope `{"v":1,"event":{...},"channel_sequence":N}`.
pub const EVENT_ENVELOPE_VERSION: u8 = 1;

/// Every envelope this crate writes starts with this text: serde writes
/// struct fields in declaration order and `v` is first.
const ENVELOPE_PREFIX: &str = "{\"v\":";

#[derive(Serialize)]
struct EventEnvelopeRef<'a> {
    v: u8,
    event: &'a nostr::Event,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_sequence: Option<i64>,
}

#[derive(Deserialize)]
struct EventEnvelope {
    v: u8,
    event: nostr::Event,
    #[serde(default)]
    channel_sequence: Option<i64>,
}

/// Returns the Redis pub/sub channel key for `channel_id` under `ctx`.
pub fn channel_key(ctx: &TenantContext, channel_id: Uuid) -> String {
    topic::channel_key(ctx, channel_id)
}

/// Returns the Redis pub/sub channel key for community-global events under `ctx`.
pub fn global_key(ctx: &TenantContext) -> String {
    topic::global_key(ctx)
}

/// Encodes an event for the Redis event topics.
///
/// Writes the envelope only when `seq_envelope` is on and the sequence is
/// known; otherwise the bare event JSON, which every relay version reads.
pub fn encode_event_payload(
    event: &nostr::Event,
    channel_sequence: Option<i64>,
    seq_envelope: bool,
) -> Result<String, PubSubError> {
    match channel_sequence {
        Some(_) if seq_envelope => Ok(serde_json::to_string(&EventEnvelopeRef {
            v: EVENT_ENVELOPE_VERSION,
            event,
            channel_sequence,
        })?),
        _ => Ok(event.as_json()),
    }
}

/// Decodes a Redis event-topic payload: either the bare event JSON (older
/// relays, or no known sequence) or the version-1 envelope.
pub fn decode_event_payload(payload: &str) -> Result<(nostr::Event, Option<i64>), PubSubError> {
    if !payload.trim_start().starts_with(ENVELOPE_PREFIX) {
        // Same parse as `nostr::Event::from_json` (plain serde, no signature check).
        return Ok((serde_json::from_str(payload)?, None));
    }
    let envelope: EventEnvelope = serde_json::from_str(payload)?;
    if envelope.v != EVENT_ENVELOPE_VERSION {
        return Err(PubSubError::Serialization(serde::de::Error::custom(
            format_args!("unsupported event envelope version {}", envelope.v),
        )));
    }
    Ok((envelope.event, envelope.channel_sequence))
}

/// Returns the number of subscribers that received the message.
pub async fn publish_event(
    pool: &Pool,
    ctx: &TenantContext,
    topic: EventTopic,
    event: &nostr::Event,
    channel_sequence: Option<i64>,
    seq_envelope: bool,
) -> Result<i64, PubSubError> {
    let mut conn = pool.get().await?;
    let key = crate::topic::EventTopicKey::from_context(ctx, topic).redis_channel();
    let payload = encode_event_payload(event, channel_sequence, seq_envelope)?;
    let subscriber_count: i64 = redis::cmd("PUBLISH")
        .arg(&key)
        .arg(&payload)
        .query_async(&mut conn)
        .await?;
    Ok(subscriber_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};

    fn event() -> nostr::Event {
        EventBuilder::new(Kind::TextNote, "hello \"envelope\"")
            .sign_with_keys(&Keys::generate())
            .expect("signing failed")
    }

    #[test]
    fn flag_off_writes_byte_identical_bare_event() {
        let event = event();
        for sequence in [None, Some(7)] {
            let payload = encode_event_payload(&event, sequence, false).unwrap();
            assert_eq!(payload, event.as_json());
        }
    }

    #[test]
    fn flag_on_without_sequence_writes_bare_event() {
        let event = event();
        let payload = encode_event_payload(&event, None, true).unwrap();
        assert_eq!(payload, event.as_json());
    }

    #[test]
    fn envelope_round_trips_sequence() {
        let event = event();
        let payload = encode_event_payload(&event, Some(42), true).unwrap();
        assert!(payload.starts_with("{\"v\":1,\"event\":{"), "{payload}");
        assert!(payload.ends_with(",\"channel_sequence\":42}"), "{payload}");

        let (decoded, sequence) = decode_event_payload(&payload).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(sequence, Some(42));
    }

    #[test]
    fn bare_event_from_older_relay_still_decodes() {
        let event = event();
        let (decoded, sequence) = decode_event_payload(&event.as_json()).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(sequence, None);
    }

    #[test]
    fn envelope_with_missing_or_null_sequence_decodes_as_none() {
        let event = event();
        for payload in [
            format!("{{\"v\":1,\"event\":{}}}", event.as_json()),
            format!(
                "{{\"v\":1,\"event\":{},\"channel_sequence\":null}}",
                event.as_json()
            ),
        ] {
            let (decoded, sequence) = decode_event_payload(&payload).unwrap();
            assert_eq!(decoded, event);
            assert_eq!(sequence, None);
        }
    }

    #[test]
    fn unknown_envelope_version_is_rejected() {
        let payload = format!(
            "{{\"v\":2,\"event\":{},\"channel_sequence\":1}}",
            event().as_json()
        );
        assert!(decode_event_payload(&payload).is_err());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(decode_event_payload("not json").is_err());
        assert!(decode_event_payload("{\"v\":1}").is_err());
    }
}
