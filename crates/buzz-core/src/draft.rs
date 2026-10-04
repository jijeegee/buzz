//! Server-side sender stamping of unsigned event drafts (centralized identity).
//!
//! A token-authenticated client submits an event *draft*: `kind`, `tags`,
//! `content` and optionally `created_at`, `pubkey` and `id`. The relay — not
//! the client — decides who sent it: [`stamp_draft`] sets `pubkey` to the
//! authenticated principal, corrects a missing or skewed `created_at`,
//! recomputes the NIP-01 `id`, and fills `sig` with [`SENTINEL_SIG`]. A client
//! `sig` is ignored.
//!
//! The sentinel exists only because `nostr::Event` always carries a signature
//! (Phase 0–3). It is never a valid Schnorr signature, so a stamped event can
//! never be mistaken for a client-signed one by a verifier.

use nostr::{Event, EventId};
use serde_json::Value;

use crate::principal::PrincipalId;

/// The 64-byte all-zero signature carried by server-stamped events.
pub const SENTINEL_SIG: [u8; 64] = [0u8; 64];

/// Maximum distance (seconds) between a draft's `created_at` and server time
/// before the server replaces it with its own clock.
pub const DRAFT_MAX_SKEW_SECS: i64 = 300;

/// Why a draft could not be stamped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StampError {
    /// The draft named a `pubkey` other than the authenticated principal.
    ///
    /// `claimed_id` is the NIP-01 id computed with the claimed pubkey, when it
    /// parses, so the rejection can be correlated with the client's request.
    PubkeyMismatch {
        /// Event id the client would have computed for its claimed pubkey.
        claimed_id: Option<String>,
    },
    /// The draft is not a well-formed event body.
    Invalid(String),
}

/// Whether `sig` is the server-stamp sentinel.
pub fn is_sentinel_sig(sig: &[u8]) -> bool {
    sig == SENTINEL_SIG
}

/// Stamp `draft` as sent by `principal` at server time `now` (unix seconds).
pub fn stamp_draft(draft: &Value, principal: &PrincipalId, now: i64) -> Result<Event, StampError> {
    let object = draft
        .as_object()
        .ok_or_else(|| StampError::Invalid("event draft must be a JSON object".into()))?;

    let kind = object
        .get("kind")
        .and_then(Value::as_u64)
        .filter(|kind| *kind <= u64::from(u16::MAX))
        .ok_or_else(|| StampError::Invalid("event draft requires a numeric kind".into()))?;
    let tags = object
        .get("tags")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let content = match object.get("content") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(content)) => content.clone(),
        Some(_) => return Err(StampError::Invalid("content must be a string".into())),
    };
    let created_at = match object.get("created_at").and_then(Value::as_i64) {
        Some(ts) if (ts - now).abs() <= DRAFT_MAX_SKEW_SECS => ts,
        _ => now,
    };

    let build = |pubkey_hex: &str| -> Result<Event, StampError> {
        let event_json = serde_json::json!({
            "id": "00".repeat(32),
            "pubkey": pubkey_hex,
            "created_at": created_at,
            "kind": kind,
            "tags": tags,
            "content": content,
            "sig": hex::encode(SENTINEL_SIG),
        });
        let mut event: Event = serde_json::from_value(event_json)
            .map_err(|e| StampError::Invalid(format!("invalid event draft: {e}")))?;
        event.id = EventId::new(
            &event.pubkey,
            &event.created_at,
            &event.kind,
            &event.tags,
            &event.content,
        );
        Ok(event)
    };

    let principal_hex = principal.to_hex();
    if let Some(claimed) = object.get("pubkey").filter(|v| !v.is_null()) {
        let claimed = claimed
            .as_str()
            .ok_or_else(|| StampError::Invalid("pubkey must be a string".into()))?;
        if !claimed.eq_ignore_ascii_case(&principal_hex) {
            let claimed_id = PrincipalId::from_hex(claimed)
                .ok()
                .and_then(|pk| build(&pk.to_hex()).ok())
                .map(|event| event.id.to_hex());
            return Err(StampError::PubkeyMismatch { claimed_id });
        }
    }

    build(&principal_hex)
}

/// Integrity check for an event **served by the relay**.
///
/// Key-signed events must carry a valid id and Schnorr signature. A
/// server-stamped event (sentinel signature) is accepted when its id is the
/// NIP-01 id: in the centralized-identity model the relay — not a signature —
/// vouches for `pubkey`, so a client that already trusts the relay connection
/// must not drop token-authored events. Use this, not `Event::verify`, for
/// events read back from the relay.
pub fn verify_served_event(event: &Event) -> bool {
    event.verify_id() && (is_sentinel_sig(event.sig.as_ref()) || event.verify_signature())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal() -> PrincipalId {
        PrincipalId::generate()
    }

    #[test]
    fn stamps_principal_recomputes_id_and_uses_sentinel() {
        let p = principal();
        let now = 1_760_000_000;
        let draft = serde_json::json!({
            "kind": 40002,
            "created_at": now - 10,
            "tags": [["h", "c0ffee00-0000-0000-0000-000000000000"]],
            "content": "hi",
            "id": "ff".repeat(32),
            "sig": "11".repeat(64),
        });
        let event = stamp_draft(&draft, &p, now).expect("stamp");
        assert_eq!(event.pubkey, p.as_public_key());
        assert_eq!(event.created_at.as_secs() as i64, now - 10);
        assert!(event.verify_id(), "server recomputes the NIP-01 id");
        assert_ne!(event.id.to_hex(), "ff".repeat(32));
        assert!(is_sentinel_sig(event.sig.as_ref()));
        assert!(
            !event.verify_signature(),
            "sentinel is never a valid signature"
        );
    }

    #[test]
    fn skewed_or_missing_created_at_uses_server_time() {
        let p = principal();
        let now = 1_760_000_000;
        let skewed = serde_json::json!({"kind": 1, "created_at": now - 3600, "content": ""});
        assert_eq!(
            stamp_draft(&skewed, &p, now).unwrap().created_at.as_secs() as i64,
            now
        );
        let missing = serde_json::json!({"kind": 1});
        assert_eq!(
            stamp_draft(&missing, &p, now).unwrap().created_at.as_secs() as i64,
            now
        );
    }

    #[test]
    fn foreign_pubkey_is_rejected_with_claimed_id() {
        let p = principal();
        let other = principal();
        let draft = serde_json::json!({"kind": 1, "pubkey": other.to_hex(), "content": "x"});
        match stamp_draft(&draft, &p, 1_760_000_000) {
            Err(StampError::PubkeyMismatch { claimed_id }) => assert!(claimed_id.is_some()),
            other => panic!("expected mismatch, got {other:?}"),
        }
        let own = serde_json::json!({"kind": 1, "pubkey": p.to_hex()});
        assert!(stamp_draft(&own, &p, 1_760_000_000).is_ok());
    }

    #[test]
    fn served_event_check_accepts_stamped_and_signed_only() {
        let p = principal();
        let stamped = stamp_draft(
            &serde_json::json!({"kind": 1, "content": "x"}),
            &p,
            1_760_000_000,
        )
        .unwrap();
        assert!(verify_served_event(&stamped));
        let mut tampered = stamped.clone();
        tampered.content = "y".into();
        assert!(!verify_served_event(&tampered), "id must still match");

        let keys = nostr::Keys::generate();
        let signed = nostr::EventBuilder::text_note("z")
            .sign_with_keys(&keys)
            .unwrap();
        assert!(verify_served_event(&signed));
        let mut forged = signed.clone();
        forged.sig = stamped.sig;
        forged.pubkey = keys.public_key();
        assert!(
            verify_served_event(&forged),
            "sentinel + valid id is the stamp shape"
        );
        let mut bad_sig = signed;
        bad_sig.sig = nostr::EventBuilder::text_note("z")
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap()
            .sig;
        assert!(!verify_served_event(&bad_sig));
    }

    #[test]
    fn malformed_drafts_are_invalid() {
        let p = principal();
        for draft in [
            serde_json::json!([]),
            serde_json::json!({"content": "no kind"}),
            serde_json::json!({"kind": 70000}),
            serde_json::json!({"kind": 1, "content": 5}),
            serde_json::json!({"kind": 1, "tags": "nope"}),
        ] {
            assert!(
                matches!(stamp_draft(&draft, &p, 0), Err(StampError::Invalid(_))),
                "{draft}"
            );
        }
    }
}
