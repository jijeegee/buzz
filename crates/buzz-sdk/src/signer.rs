//! Event signing for both authentication models (centralized identity, Phase 1).
//!
//! [`EventSigner::Keys`] is today's model: the client holds a secp256k1 key
//! and Schnorr-signs every event.
//!
//! [`EventSigner::Principal`] is the token model: the client holds a bearer
//! token and knows its server-issued principal id. It builds the event with
//! `pubkey = principal`, computes the NIP-01 id, and carries the all-zero
//! [`buzz_core::draft::SENTINEL_SIG`]. A token-authenticated relay treats the
//! body as a draft: it re-checks `pubkey`, recomputes the id (identical; a
//! `created_at` skewed by more than five minutes is rejected) and ignores `sig`. The
//! sentinel is never a valid signature, so such an event can never be passed
//! off as a key-signed one.

use buzz_core::draft::SENTINEL_SIG;
use nostr::secp256k1::schnorr::Signature;
use nostr::{Event, EventBuilder, Keys, PublicKey};

/// Why an event could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    /// Schnorr signing failed (key mode).
    #[error("signing failed: {0}")]
    Sign(String),
    /// The operation needs a secret key, which token mode does not have.
    #[error("{0} requires key-based auth; it is unavailable with a bearer token")]
    KeyRequired(&'static str),
}

/// Who the client is and how its events are produced.
#[derive(Clone)]
pub enum EventSigner {
    /// Key-based identity: events are Schnorr-signed.
    Keys(Keys),
    /// Token identity: events are drafts stamped by the relay for this principal.
    Principal(PublicKey),
}

impl std::fmt::Debug for EventSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keys(keys) => f
                .debug_tuple("Keys")
                .field(&keys.public_key().to_hex())
                .finish(),
            Self::Principal(principal) => f
                .debug_tuple("Principal")
                .field(&principal.to_hex())
                .finish(),
        }
    }
}

impl EventSigner {
    /// The identity events are attributed to (the key's pubkey or the principal id).
    pub fn public_key(&self) -> PublicKey {
        match self {
            Self::Keys(keys) => keys.public_key(),
            Self::Principal(principal) => *principal,
        }
    }

    /// Whether this signer is the token (principal) model.
    pub fn is_token(&self) -> bool {
        matches!(self, Self::Principal(_))
    }

    /// The secret keys, for operations that cannot work without them
    /// (NIP-44 encryption, NIP-98, NIP-42). `what` names the operation for
    /// the error message.
    pub fn keys(&self, what: &'static str) -> Result<&Keys, SignerError> {
        match self {
            Self::Keys(keys) => Ok(keys),
            Self::Principal(_) => Err(SignerError::KeyRequired(what)),
        }
    }

    /// Produce an event from `builder`: signed in key mode, a sentinel-signed
    /// draft attributed to the principal in token mode.
    pub fn sign(&self, builder: EventBuilder) -> Result<Event, SignerError> {
        match self {
            Self::Keys(keys) => builder
                .sign_with_keys(keys)
                .map_err(|error| SignerError::Sign(error.to_string())),
            Self::Principal(principal) => draft_event(builder, *principal),
        }
    }
}

/// Build `builder` as an unsigned draft attributed to `principal`, carrying the
/// sentinel signature. Only a token-authenticated relay connection accepts it.
pub fn draft_event(builder: EventBuilder, principal: PublicKey) -> Result<Event, SignerError> {
    let mut unsigned = builder.build(principal);
    unsigned.ensure_id();
    let id = unsigned
        .id
        .ok_or_else(|| SignerError::Sign("event id was not computed".into()))?;
    // `from_slice` only checks the length, fixed at 64 here.
    let sig = Signature::from_slice(&SENTINEL_SIG)
        .map_err(|error| SignerError::Sign(error.to_string()))?;
    Ok(Event::new(
        id,
        unsigned.pubkey,
        unsigned.created_at,
        unsigned.kind,
        unsigned.tags.to_vec(),
        unsigned.content,
        sig,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_core::principal::PrincipalId;
    use nostr::{Kind, Tag};

    #[test]
    fn principal_draft_matches_server_stamp() {
        let principal = PrincipalId::generate();
        let signer = EventSigner::Principal(principal.as_public_key());
        let builder = EventBuilder::new(Kind::Custom(40002), "hello").tags([Tag::parse([
            "h",
            "00000000-0000-0000-0000-000000000001",
        ])
        .unwrap()]);
        let event = signer.sign(builder).unwrap();
        assert_eq!(event.pubkey, principal.as_public_key());
        assert_eq!(event.sig.as_ref(), &SENTINEL_SIG);
        assert!(event.verify_id(), "client-computed id is the NIP-01 id");

        // The relay's stamp of the same draft yields the same id.
        let draft = serde_json::to_value(&event).unwrap();
        let stamped =
            buzz_core::draft::stamp_draft(&draft, &principal, event.created_at.as_secs() as i64)
                .unwrap();
        assert_eq!(stamped.id, event.id);
    }

    #[test]
    fn key_mode_signs_and_token_mode_has_no_keys() {
        let keys = Keys::generate();
        let signer = EventSigner::Keys(keys.clone());
        let event = signer.sign(EventBuilder::text_note("x")).unwrap();
        assert!(event.verify().is_ok());
        assert!(signer.keys("nip44").is_ok());

        let token = EventSigner::Principal(keys.public_key());
        assert!(token.is_token());
        assert!(matches!(
            token.keys("nip44"),
            Err(SignerError::KeyRequired("nip44"))
        ));
        let draft = token.sign(EventBuilder::text_note("x")).unwrap();
        assert!(draft.verify().is_err(), "sentinel never verifies");
    }
}
