//! Device keys: every install keeps one P-256 key pair (in the TPM, Secure
//! Enclave or Android Keystore where available) and signs each use of its
//! message keys and tokens with it. A key or token copied to another machine
//! is then useless.
//!
//! Wire formats:
//! - device public key: SEC1 compressed (33 bytes) or uncompressed (65 bytes),
//!   hex; stored compressed.
//! - proof (account HTTP calls): `<unix ts>.<nonce>.<signature hex>` over
//!   [`proof_message`].
//! - event tag (NIP-42, NIP-98, Blossom auth events):
//!   `["device", <device pubkey hex>, <signature hex>]` over
//!   [`event_proof_message`], which covers the whole event.
//!
//! Signatures are raw `r || s` (64 bytes, the form TPM, Secure Enclave and
//! WebCrypto produce), ECDSA P-256 with SHA-256. High-S is accepted (those
//! signers do not normalize), so one proof has two valid encodings: replay
//! checks key on (device key, nonce), never on the signature or header text.
//!
//! The event message uses NIP-01 serialization (see
//! `device_proof_vectors.json`, which every client must reproduce).

use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};

/// Length of a stored (compressed) device public key.
pub const DEVICE_PUBKEY_LEN: usize = 33;
/// How far a proof timestamp may be from the server clock.
pub const PROOF_WINDOW_SECS: i64 = 60;
/// Domain separator of every proof message.
const DOMAIN: &str = "kuzz-device-proof/v1";
/// Domain separator of event tag proofs.
const EVENT_DOMAIN: &str = "kuzz-device-event/v1";
/// Name of the tag carrying an event's device proof.
pub const DEVICE_TAG: &str = "device";

/// Why a device key or proof was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceProofError {
    /// The public key is not a valid P-256 point.
    InvalidKey,
    /// The proof string is malformed.
    Malformed,
    /// The proof timestamp is outside [`PROOF_WINDOW_SECS`].
    Stale,
    /// The signature does not verify.
    BadSignature,
}

/// A device public key, normalized to SEC1 compressed form.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DevicePubkey([u8; DEVICE_PUBKEY_LEN]);

impl std::fmt::Debug for DevicePubkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DevicePubkey({})", hex::encode(self.0))
    }
}

impl DevicePubkey {
    /// Parse a hex SEC1 key (compressed or uncompressed).
    pub fn from_hex(raw: &str) -> Result<Self, DeviceProofError> {
        let bytes = hex::decode(raw.trim()).map_err(|_| DeviceProofError::InvalidKey)?;
        Self::from_sec1(&bytes)
    }

    /// Parse SEC1 bytes (compressed or uncompressed).
    pub fn from_sec1(bytes: &[u8]) -> Result<Self, DeviceProofError> {
        let key = VerifyingKey::from_sec1_bytes(bytes).map_err(|_| DeviceProofError::InvalidKey)?;
        let point = key.to_encoded_point(true);
        let compressed: [u8; DEVICE_PUBKEY_LEN] = point
            .as_bytes()
            .try_into()
            .map_err(|_| DeviceProofError::InvalidKey)?;
        Ok(Self(compressed))
    }

    /// The compressed SEC1 bytes.
    pub fn as_bytes(&self) -> &[u8; DEVICE_PUBKEY_LEN] {
        &self.0
    }

    /// Lower-case hex of the compressed key.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub(crate) fn verifying_key(&self) -> Result<VerifyingKey, DeviceProofError> {
        VerifyingKey::from_sec1_bytes(&self.0).map_err(|_| DeviceProofError::InvalidKey)
    }
}

/// A parsed `<ts>.<nonce>.<sig>` proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceProof {
    /// Unix seconds the client signed at.
    pub ts: i64,
    /// Client-chosen nonce (16-64 URL-safe characters).
    pub nonce: String,
    signature: [u8; 64],
}

impl DeviceProof {
    /// Parse the wire form.
    pub fn parse(raw: &str) -> Result<Self, DeviceProofError> {
        let mut parts = raw.trim().splitn(3, '.');
        let (Some(ts), Some(nonce), Some(sig)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(DeviceProofError::Malformed);
        };
        let ts: i64 = ts.parse().map_err(|_| DeviceProofError::Malformed)?;
        let nonce_ok = (16..=64).contains(&nonce.len())
            && nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !nonce_ok {
            return Err(DeviceProofError::Malformed);
        }
        let signature: [u8; 64] = hex::decode(sig)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(DeviceProofError::Malformed)?;
        Ok(Self {
            ts,
            nonce: nonce.to_owned(),
            signature,
        })
    }

    /// Verify this proof for `scope` against `key`, with the timestamp
    /// within [`PROOF_WINDOW_SECS`] of `now`. Replay of the nonce is the
    /// caller's check.
    pub fn verify(
        &self,
        key: &DevicePubkey,
        scope: &ProofScope<'_>,
        now: i64,
    ) -> Result<(), DeviceProofError> {
        if now
            .checked_sub(self.ts)
            .is_none_or(|age| age.abs() > PROOF_WINDOW_SECS)
        {
            return Err(DeviceProofError::Stale);
        }
        let message = proof_message(scope, self.ts, &self.nonce);
        let signature =
            Signature::from_slice(&self.signature).map_err(|_| DeviceProofError::BadSignature)?;
        key.verifying_key()?
            .verify(message.as_bytes(), &signature)
            .map_err(|_| DeviceProofError::BadSignature)
    }
}

/// What a header proof is bound to.
#[derive(Debug, Clone, Copy)]
pub struct ProofScope<'a> {
    /// The use: `bind` (login), `auth` (account call), `refresh`.
    pub purpose: &'a str,
    /// The exchange: `login:<code sha256 hex>`, or `<METHOD> <full URL>`.
    pub binding: &'a str,
    /// SHA-256 of the bearer token used, if any.
    pub token_hash: Option<&'a [u8; 32]>,
    /// SHA-256 of the request body, if it has one.
    pub body_hash: Option<&'a [u8; 32]>,
}

/// Purpose of the new key's possession proof in a device key rotation
/// (`POST /auth/device/rotate`).
pub const PURPOSE_ROTATE: &str = "rotate";

/// What a rotation's new key signs over (purpose [`PURPOSE_ROTATE`], with
/// the session's access token hash): the device row and both keys,
/// `rotate:<device id>:<old key hex>:<new key hex>`. `device_id` is the
/// lower-case hyphenated UUID.
pub fn rotate_binding(device_id: &str, old: &DevicePubkey, new: &DevicePubkey) -> String {
    format!("rotate:{device_id}:{}:{}", old.to_hex(), new.to_hex())
}

/// The exact text a device signs for a header proof:
///
/// ```text
/// kuzz-device-proof/v1
/// <purpose>
/// <binding>
/// <token sha256 hex, or "-">
/// <body sha256 hex, or "-">
/// <ts>
/// <nonce>
/// ```
pub fn proof_message(scope: &ProofScope<'_>, ts: i64, nonce: &str) -> String {
    let hash = |value: Option<&[u8; 32]>| value.map_or_else(|| "-".to_owned(), hex::encode);
    format!(
        "{DOMAIN}
{}
{}
{}
{}
{ts}
{nonce}",
        scope.purpose,
        scope.binding,
        hash(scope.token_hash),
        hash(scope.body_hash)
    )
}

/// The exact text a device signs for an event: [`EVENT_DOMAIN`], a newline,
/// then the event's NIP-01 id serialization
/// `[0, pubkey, created_at, kind, tags, content]` in which the signature field
/// of the `device` tag is the empty string. The proof therefore covers the
/// content and every other tag (challenge, relay, url, method, payload, ...),
/// and the event id in turn covers the proof.
pub fn event_proof_message(
    pubkey_hex: &str,
    created_at: u64,
    kind: u16,
    tags: &[Vec<String>],
    content: &str,
) -> String {
    let blanked: Vec<Vec<String>> = tags
        .iter()
        .map(|tag| {
            if tag.first().map(String::as_str) == Some(DEVICE_TAG) {
                let mut tag = tag.clone();
                if let Some(sig) = tag.get_mut(2) {
                    sig.clear();
                }
                tag
            } else {
                tag.clone()
            }
        })
        .collect();
    let serialized = serde_json::json!([0, pubkey_hex, created_at, kind, blanked, content]);
    format!(
        "{EVENT_DOMAIN}
{serialized}"
    )
}

/// Verify the `device` tag of `event`: `Ok(None)` when it has none, the
/// device key when its signature covers the event, an error otherwise (also
/// for more than one `device` tag, or one of another shape). Freshness and
/// replay are the event's own rules (AUTH challenge, NIP-98 window, Blossom
/// expiry). Call it only for auth events, after checking the event's own id
/// and signature: other events may carry unrelated `device` tags.
pub fn verify_event_device_tag(
    event: &nostr::Event,
) -> Result<Option<DevicePubkey>, DeviceProofError> {
    let tags: Vec<Vec<String>> = event
        .tags
        .iter()
        .map(|tag| tag.as_slice().to_vec())
        .collect();
    let mut device_tags = tags
        .iter()
        .filter(|tag| tag.first().map(String::as_str) == Some(DEVICE_TAG));
    let Some(tag) = device_tags.next() else {
        return Ok(None);
    };
    if device_tags.next().is_some() || tag.len() != 3 {
        return Err(DeviceProofError::Malformed);
    }
    let key = DevicePubkey::from_hex(&tag[1])?;
    let signature: [u8; 64] = hex::decode(&tag[2])
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(DeviceProofError::Malformed)?;
    let message = event_proof_message(
        &event.pubkey.to_hex(),
        event.created_at.as_secs(),
        event.kind.as_u16(),
        &tags,
        &event.content,
    );
    let signature =
        Signature::from_slice(&signature).map_err(|_| DeviceProofError::BadSignature)?;
    key.verifying_key()?
        .verify(message.as_bytes(), &signature)
        .map_err(|_| DeviceProofError::BadSignature)?;
    Ok(Some(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Signer as _;
    use p256::ecdsa::SigningKey;

    fn device() -> (SigningKey, DevicePubkey) {
        let signing = SigningKey::from_slice(&[7u8; 32]).expect("key");
        let public =
            DevicePubkey::from_sec1(signing.verifying_key().to_encoded_point(false).as_bytes())
                .expect("public");
        (signing, public)
    }

    fn scope<'a>(
        purpose: &'a str,
        binding: &'a str,
        token: Option<&'a [u8; 32]>,
    ) -> ProofScope<'a> {
        ProofScope {
            purpose,
            binding,
            token_hash: token,
            body_hash: None,
        }
    }

    fn sign(key: &SigningKey, scope: &ProofScope<'_>, ts: i64) -> DeviceProof {
        let nonce = "nonce-0123456789ab";
        let message = proof_message(scope, ts, nonce);
        let signature: Signature = key.sign(message.as_bytes());
        DeviceProof::parse(&format!(
            "{ts}.{nonce}.{}",
            hex::encode(signature.to_bytes())
        ))
        .expect("parse")
    }

    #[test]
    fn a_rotation_proof_binds_the_device_and_both_keys() {
        let (key, new) = device();
        let old = DevicePubkey::from_sec1(
            SigningKey::from_slice(&[9u8; 32])
                .expect("key")
                .verifying_key()
                .to_encoded_point(true)
                .as_bytes(),
        )
        .expect("old");
        let device = "6f9619ff-8b86-d011-b42d-00c04fc964ff";
        let binding = rotate_binding(device, &old, &new);
        assert_eq!(
            binding,
            format!("rotate:{device}:{}:{}", old.to_hex(), new.to_hex())
        );
        let token = [2u8; 32];
        let proof = sign(&key, &scope(PURPOSE_ROTATE, &binding, Some(&token)), 50);
        assert_eq!(
            proof.verify(&new, &scope(PURPOSE_ROTATE, &binding, Some(&token)), 50),
            Ok(())
        );
        // Swapped keys or another device: another message.
        let swapped = rotate_binding(device, &new, &old);
        let other = rotate_binding("00000000-0000-0000-0000-000000000000", &old, &new);
        for binding in [swapped, other] {
            assert_eq!(
                proof.verify(&new, &scope(PURPOSE_ROTATE, &binding, Some(&token)), 50),
                Err(DeviceProofError::BadSignature)
            );
        }
    }

    #[test]
    fn a_proof_verifies_only_for_its_key_and_scope() {
        let (key, public) = device();
        let token = [1u8; 32];
        let proof = sign(&key, &scope("auth", "b", Some(&token)), 1_000);
        assert_eq!(
            proof.verify(&public, &scope("auth", "b", Some(&token)), 1_010),
            Ok(())
        );
        for wrong in [
            scope("refresh", "b", Some(&token)),
            scope("auth", "other", Some(&token)),
            scope("auth", "b", Some(&[2u8; 32])),
            ProofScope {
                body_hash: Some(&[3u8; 32]),
                ..scope("auth", "b", Some(&token))
            },
        ] {
            assert_eq!(
                proof.verify(&public, &wrong, 1_010),
                Err(DeviceProofError::BadSignature)
            );
        }
        let other = DevicePubkey::from_sec1(
            SigningKey::from_slice(&[9u8; 32])
                .expect("key")
                .verifying_key()
                .to_encoded_point(true)
                .as_bytes(),
        )
        .expect("other");
        assert_eq!(
            proof.verify(&other, &scope("auth", "b", Some(&token)), 1_010),
            Err(DeviceProofError::BadSignature)
        );
    }

    #[test]
    fn a_stale_proof_is_refused() {
        let (key, public) = device();
        let proof = sign(&key, &scope("bind", "b", None), 1_000);
        assert_eq!(
            proof.verify(
                &public,
                &scope("bind", "b", None),
                1_000 + PROOF_WINDOW_SECS + 1
            ),
            Err(DeviceProofError::Stale)
        );
    }

    #[test]
    fn keys_normalize_to_compressed_and_garbage_is_refused() {
        let (_, public) = device();
        assert_eq!(DevicePubkey::from_hex(&public.to_hex()), Ok(public));
        assert_eq!(public.as_bytes().len(), DEVICE_PUBKEY_LEN);
        assert_eq!(
            DevicePubkey::from_hex("02ff"),
            Err(DeviceProofError::InvalidKey)
        );
        assert_eq!(
            DeviceProof::parse("1.short.00"),
            Err(DeviceProofError::Malformed)
        );
        assert_eq!(
            DeviceProof::parse("x.nonce-0123456789ab.00"),
            Err(DeviceProofError::Malformed)
        );
    }

    fn event_with_device(
        device: &SigningKey,
        public: &DevicePubkey,
        content: &str,
        tamper: bool,
    ) -> nostr::Event {
        let other = vec![vec!["challenge".to_owned(), "c1".to_owned()]];
        device_event(device, public, 22242, other, content, tamper)
    }

    fn device_event(
        device: &SigningKey,
        public: &DevicePubkey,
        kind: u16,
        other: Vec<Vec<String>>,
        content: &str,
        tamper: bool,
    ) -> nostr::Event {
        use nostr::{EventBuilder, Keys, Kind, Tag};
        let keys = Keys::new(nostr::SecretKey::from_slice(&[5u8; 32]).expect("secret"));
        let created_at = nostr::Timestamp::from(1_700_000_000);
        let mut tags = other;
        tags.push(vec![DEVICE_TAG.to_owned(), public.to_hex(), String::new()]);
        let message = event_proof_message(
            &keys.public_key().to_hex(),
            created_at.as_secs(),
            kind,
            &tags,
            content,
        );
        let signature: Signature = device.sign(message.as_bytes());
        if let Some(sig) = tags.last_mut().and_then(|tag| tag.get_mut(2)) {
            *sig = hex::encode(signature.to_bytes());
        }
        let signed_content = if tamper { "changed" } else { content };
        EventBuilder::new(Kind::Custom(kind), signed_content)
            .tags(tags.into_iter().map(|tag| Tag::parse(tag).expect("tag")))
            .custom_created_at(created_at)
            .sign_with_keys(&keys)
            .expect("sign")
    }

    /// Content exercising every JSON escaping rule clients must match.
    const ESCAPING_CONTENT: &str = "quote \" backslash \\ newline \n tab \t cr \r bell \u{7} \
         del \u{7f} slash / \u{d55c}\u{ae00} \u{1f600} ls \u{2028}";

    /// Prints the escaping entry of `device_proof_vectors.json` (run with
    /// `--ignored --nocapture` when changing vectors).
    #[test]
    #[ignore = "vector generator"]
    fn print_escaping_vector() {
        let (key, public) = device();
        let tags = vec![
            vec![
                "u".to_owned(),
                "https://relay.example/api/v1/upload?a=1&b=/x".to_owned(),
            ],
            vec!["method".to_owned(), "POST".to_owned()],
        ];
        let event = device_event(&key, &public, 27235, tags, ESCAPING_CONTENT, false);
        let event_tags: Vec<Vec<String>> =
            event.tags.iter().map(|t| t.as_slice().to_vec()).collect();
        let message = event_proof_message(
            &event.pubkey.to_hex(),
            event.created_at.as_secs(),
            event.kind.as_u16(),
            &event_tags,
            &event.content,
        );
        println!(
            "{}",
            serde_json::json!({ "event": event, "message": message })
        );
    }

    #[test]
    fn an_event_device_tag_verifies_only_for_the_event_it_signed() {
        let (key, public) = device();
        let event = event_with_device(&key, &public, "hello", false);
        assert_eq!(verify_event_device_tag(&event), Ok(Some(public)));
        let tampered = event_with_device(&key, &public, "hello", true);
        assert_eq!(
            verify_event_device_tag(&tampered),
            Err(DeviceProofError::BadSignature)
        );
    }

    #[test]
    fn an_event_without_a_device_tag_has_no_device() {
        use nostr::{EventBuilder, Keys, Kind};
        let event = EventBuilder::new(Kind::Custom(22242), "")
            .sign_with_keys(&Keys::generate())
            .expect("sign");
        assert_eq!(verify_event_device_tag(&event), Ok(None));
    }

    /// The shared vectors every client tests against.
    const VECTORS: &str = include_str!("device_proof_vectors.json");

    #[test]
    fn the_shared_test_vectors_hold() {
        let vectors: serde_json::Value = serde_json::from_str(VECTORS).expect("vectors");
        let (_, public) = device();
        assert_eq!(vectors["device_pubkey_hex"], public.to_hex());

        let header = &vectors["header_proof"];
        let token: [u8; 32] = hex::decode(header["token_sha256_hex"].as_str().expect("token"))
            .expect("hex")
            .try_into()
            .expect("32");
        let body: [u8; 32] = hex::decode(header["body_sha256_hex"].as_str().expect("body"))
            .expect("hex")
            .try_into()
            .expect("32");
        let scope = ProofScope {
            purpose: header["purpose"].as_str().expect("purpose"),
            binding: header["binding"].as_str().expect("binding"),
            token_hash: Some(&token),
            body_hash: Some(&body),
        };
        let ts = header["ts"].as_i64().expect("ts");
        let nonce = header["nonce"].as_str().expect("nonce");
        assert_eq!(proof_message(&scope, ts, nonce), header["message"]);
        let proof = DeviceProof::parse(header["header"].as_str().expect("header")).expect("parse");
        assert_eq!(proof.verify(&public, &scope, ts), Ok(()));

        let rotation = &vectors["rotation"];
        assert_eq!(rotation["purpose"], PURPOSE_ROTATE);
        assert_eq!(rotation["new_pubkey_hex"], public.to_hex());
        let old_secret =
            hex::decode(rotation["old_secret_hex"].as_str().expect("old")).expect("hex");
        let old = DevicePubkey::from_sec1(
            SigningKey::from_slice(&old_secret)
                .expect("key")
                .verifying_key()
                .to_encoded_point(true)
                .as_bytes(),
        )
        .expect("old");
        assert_eq!(rotation["old_pubkey_hex"], old.to_hex());
        assert_eq!(
            rotation["binding"],
            rotate_binding(
                rotation["device_id"].as_str().expect("device"),
                &old,
                &public
            )
        );

        let tagged = vectors["event_tags"].as_array().expect("event_tags");
        assert_eq!(tagged.len(), 2);
        assert_eq!(tagged[1]["event"]["content"], ESCAPING_CONTENT);
        for tagged in tagged {
            let event: nostr::Event =
                serde_json::from_value(tagged["event"].clone()).expect("event");
            let tags: Vec<Vec<String>> = event.tags.iter().map(|t| t.as_slice().to_vec()).collect();
            assert_eq!(
                event_proof_message(
                    &event.pubkey.to_hex(),
                    event.created_at.as_secs(),
                    event.kind.as_u16(),
                    &tags,
                    &event.content,
                ),
                tagged["message"]
            );
            assert_eq!(verify_event_device_tag(&event), Ok(Some(public)));
        }
    }
}
