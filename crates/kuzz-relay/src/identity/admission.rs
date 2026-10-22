//! Key admission (device-bound keys plan §2.2, PR A1c): may a message key be
//! used from the device that signed its auth event?
//!
//! The rules live in [`kuzz_db::identity::KeyAdmission`]: a person or Kuzz
//! agent key needs a live login on its device, an external agent key its
//! paired device. Keys registered nowhere (older keys, bots, keys from other
//! clients) are admitted until registration covers them (A2/A3); they are
//! still counted as "would refuse" with reason `unregistered`.
//!
//! The surface's [`DeviceAdmissionMode`] decides what a refusal does, as for
//! account calls: nothing and no work at all (`off`), a deduped log line and
//! metric (`log`), or a refusal (`enforce`). Positive answers are cached for
//! [`ADMIT_TTL`] per (key, device key), so a sign-out may take that long to
//! reach a reconnecting socket; closing already open sockets is A4's job.
//! Refusals and "unregistered" answers are cached for [`DENY_TTL`] (plan O6),
//! so `log` on a busy HTTP surface costs no query per request; a login
//! clears them for its account key and device ([`forget_login`]).
//!
//! Every entrance that verifies a message key's signature calls in here
//! (PR A2), each on its own [`Surface`]: [`admit_key_with_owner`] for NIP-42
//! AUTH, [`admit_key_for_event`] for a verified NIP-98, Blossom or huddle auth
//! event, [`admit_key_for_nip98`] for a request the bridge verifier
//! authenticated. A registered key's NIP-OA owner comes from the registry: a
//! tag naming anyone else is refused (`owner_mismatch`). Token message paths
//! are closed separately ([`super::device::close_token_path`]).

use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use base64::Engine as _;
use kuzz_auth::device_proof::{
    verify_event_device_tag, DeviceProofError, DevicePubkey, DEVICE_PUBKEY_LEN,
};
use kuzz_db::identity::{KeyAdmission, KeyRefusal};

use super::config::DeviceAdmissionMode;
use super::device::{self, Surface};
use crate::state::AppState;

/// How long a positive answer is reused.
const ADMIT_TTL: Duration = Duration::from_secs(30);
/// How long a refusal or an "unregistered" answer is reused (plan O6).
const DENY_TTL: Duration = Duration::from_secs(10);

/// Why a key was refused (only returned under `enforce`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyRefused {
    /// The auth event's `device` tag is malformed or its signature fails.
    BadTag,
    /// No signed auth event at all (the `X-Pubkey` dev header).
    Unsigned,
    /// A NIP-OA tag names another owner than the key's registry owner.
    OwnerMismatch,
    /// The registered key may not be used from this device.
    Key(KeyRefusal),
    /// The registry could not be read; fail closed.
    Unavailable,
}

impl KeyRefused {
    /// Stable name for logs and metrics.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::BadTag => "bad_sig",
            Self::Unsigned => "unsigned",
            Self::OwnerMismatch => "owner_mismatch",
            Self::Key(reason) => reason.as_str(),
            Self::Unavailable => "unavailable",
        }
    }

    /// The key's owner is not signed in on the device (any more), so a
    /// supervisor should park the agent instead of restarting it (plan F9).
    pub(crate) fn signed_out(self) -> bool {
        matches!(
            self,
            Self::Key(KeyRefusal::NotSignedIn | KeyRefusal::DeviceRevoked)
        )
    }

    /// An agent key bound to its owner by the registry backfill but not yet
    /// registered from a device: a supervisor should park it until the owner
    /// registers it, never restart it.
    pub(crate) fn unbound(self) -> bool {
        matches!(self, Self::Key(KeyRefusal::Unbound))
    }

    /// The key or its account is gone for good (a deleted agent, a disabled
    /// account): a supervisor should park it too, never restart it.
    pub(crate) fn revoked(self) -> bool {
        matches!(
            self,
            Self::Key(KeyRefusal::KeyRevoked | KeyRefusal::PrincipalDisabled)
        )
    }

    /// The client-visible refusal: `restricted: signed_out` and
    /// `restricted: revoked` tell a supervisor to park the key.
    pub(crate) fn reason(self) -> &'static str {
        if self == Self::Unavailable {
            "error: internal error checking device"
        } else if self.signed_out() {
            "restricted: signed_out"
        } else if self.revoked() {
            "restricted: revoked"
        } else if self.unbound() {
            "restricted: unbound"
        } else if self == Self::OwnerMismatch {
            "restricted: owner"
        } else {
            "restricted: device"
        }
    }

    /// The HTTP answer: 503 when the registry could not be read, else 403.
    pub(crate) fn http_error(self) -> (StatusCode, Json<serde_json::Value>) {
        let status = if self == Self::Unavailable {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::FORBIDDEN
        };
        crate::api::api_error(status, self.reason())
    }
}

type CacheKey = ([u8; 32], Option<[u8; DEVICE_PUBKEY_LEN]>);

/// A cached negative answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Denied {
    /// The key is registered nowhere (admitted, reported).
    Unregistered,
    /// The registry refuses the key on this device.
    Refused(KeyRefusal),
}

/// Recent registry answers per (key, device key): admissions with the key's
/// owner key ([`KeyAdmission::Admitted::owner_pubkey`]) and, shorter,
/// refusals and "unregistered" answers. A lookup error is never cached.
pub(crate) struct AdmissionCache {
    admitted: moka::sync::Cache<CacheKey, Option<[u8; 32]>>,
    denied: moka::sync::Cache<CacheKey, Denied>,
}

impl Default for AdmissionCache {
    fn default() -> Self {
        Self {
            admitted: moka::sync::Cache::builder()
                .max_capacity(100_000)
                .time_to_live(ADMIT_TTL)
                .support_invalidation_closures()
                .build(),
            denied: moka::sync::Cache::builder()
                .max_capacity(100_000)
                .time_to_live(DENY_TTL)
                .support_invalidation_closures()
                .build(),
        }
    }
}

/// A login just completed for the account whose own message key is
/// `account_key` (when it has one) on the device holding `device_key`: drop
/// cached answers for that key, and cached refusals of any key on that
/// device (its agents), so the fresh login is not refused for [`DENY_TTL`].
pub(crate) fn forget_login(
    state: &AppState,
    account_key: Option<[u8; 32]>,
    device_key: Option<[u8; DEVICE_PUBKEY_LEN]>,
) {
    if account_key.is_none() && device_key.is_none() {
        return;
    }
    let cache = &state.identity.admissions;
    if let Some(account_key) = account_key {
        // Only fails when closures are not enabled (they are, above).
        let _ = cache
            .admitted
            .invalidate_entries_if(move |key, _| key.0 == account_key);
    }
    let _ = cache.denied.invalidate_entries_if(move |key, _| {
        Some(key.0) == account_key || (device_key.is_some() && key.1 == device_key)
    });
}

impl AdmissionCache {
    /// Forget every cached answer for `pubkey` on this replica (an agent
    /// registered or moved to another host). Other replicas forget theirs
    /// within [`ADMIT_TTL`].
    pub(crate) fn evict_key(&self, pubkey: &[u8; 32]) {
        let pubkey = *pubkey;
        // Only fails when closures are not enabled (they are, above).
        let _ = self
            .admitted
            .invalidate_entries_if(move |key, _| key.0 == pubkey);
        let _ = self
            .denied
            .invalidate_entries_if(move |key, _| key.0 == pubkey);
    }
}

/// Admit `pubkey` on `surface` given its auth event's verified `device` tag
/// (see [`kuzz_auth::device_proof::verify_event_device_tag`]). `Err` only
/// under `enforce`.
#[cfg(test)]
pub(crate) async fn admit_key(
    state: &AppState,
    pubkey: &nostr::PublicKey,
    device_tag: Result<Option<DevicePubkey>, DeviceProofError>,
    surface: Surface,
) -> Result<(), KeyRefused> {
    admit_key_with_owner(state, pubkey, device_tag, None, surface).await
}

/// [`admit_key`] for an auth event that also names a NIP-OA owner
/// (`claimed_owner`, from a verified tag): a registered key must name its
/// registry owner.
pub(crate) async fn admit_key_with_owner(
    state: &AppState,
    pubkey: &nostr::PublicKey,
    device_tag: Result<Option<DevicePubkey>, DeviceProofError>,
    claimed_owner: Option<nostr::PublicKey>,
    surface: Surface,
) -> Result<(), KeyRefused> {
    if device::mode(state, surface) == DeviceAdmissionMode::Off {
        return Ok(());
    }
    let subject = pubkey.to_hex();
    let outcome = match device_tag {
        Err(_) => Err(KeyRefused::BadTag),
        Ok(device) => {
            lookup(
                state,
                pubkey,
                device.as_ref(),
                claimed_owner,
                surface,
                &subject,
            )
            .await
        }
    };
    judge(state, surface, &subject, outcome)
}

/// Admit the signer of `event`, a NIP-98, Blossom or huddle auth event whose
/// own id and signature already verified: its `device` tag, and the owner a
/// NIP-OA tag (`auth_tag`, JSON; from the event or `X-Auth-Tag`) names.
/// Nothing is parsed or verified while `surface` is off.
pub(crate) async fn admit_key_for_event(
    state: &AppState,
    event: &nostr::Event,
    auth_tag: Option<&str>,
    surface: Surface,
) -> Result<(), KeyRefused> {
    if device::mode(state, surface) == DeviceAdmissionMode::Off {
        return Ok(());
    }
    let claimed_owner = crate::api::relay_members::extract_nip_oa_owner(
        event.pubkey.as_bytes(),
        auth_tag,
        Some(event.created_at.as_secs()),
    );
    admit_key_with_owner(
        state,
        &event.pubkey,
        verify_event_device_tag(event),
        claimed_owner,
        surface,
    )
    .await
}

/// Admit `pubkey`, which the bridge verifier
/// ([`crate::api::bridge::verify_bridge_auth_with_options`]) authenticated
/// from `headers`. The verifier stays synchronous and free of device work;
/// only while `surface` is on is its NIP-98 event read again here (the same
/// first `Authorization` value it verified). The `X-Pubkey` dev header has
/// no event and is refused as `unsigned` under `enforce`.
pub(crate) async fn admit_key_for_nip98(
    state: &AppState,
    headers: &HeaderMap,
    pubkey: &nostr::PublicKey,
    surface: Surface,
) -> Result<(), KeyRefused> {
    if device::mode(state, surface) == DeviceAdmissionMode::Off {
        return Ok(());
    }
    match nip98_event(headers) {
        Some(Some(event)) if event.pubkey == *pubkey => {
            let auth_tag = crate::api::relay_members::extract_auth_tag_header(headers);
            admit_key_for_event(state, &event, auth_tag, surface).await
        }
        Some(_) => judge(state, surface, &pubkey.to_hex(), Err(KeyRefused::BadTag)),
        None => judge(state, surface, &pubkey.to_hex(), Err(KeyRefused::Unsigned)),
    }
}

/// The NIP-98 event of the first `Authorization: Nostr <base64>` header:
/// `None` without one, `Some(None)` when it does not parse.
///
/// Reads exactly what [`crate::api::bridge::verify_bridge_auth_with_options`]
/// verified: the first `authorization` value, the case-sensitive `Nostr `
/// prefix, standard base64 without trimming. Anything else is no NIP-98 to it
/// either (it falls back to `X-Pubkey`).
fn nip98_event(headers: &HeaderMap) -> Option<Option<nostr::Event>> {
    let credential = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Nostr "))?;
    Some(
        base64::engine::general_purpose::STANDARD
            .decode(credential)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|json| serde_json::from_str(&json).ok()),
    )
}

/// Apply the surface's mode to an admission outcome: report a refusal and
/// return it only under `enforce`.
fn judge(
    state: &AppState,
    surface: Surface,
    subject: &str,
    outcome: Result<(), KeyRefused>,
) -> Result<(), KeyRefused> {
    let Err(refused) = outcome else {
        return Ok(());
    };
    let enforced = device::mode(state, surface) == DeviceAdmissionMode::Enforce;
    device::report(state, surface, subject, refused.as_str(), enforced);
    if enforced {
        Err(refused)
    } else {
        Ok(())
    }
}

async fn lookup(
    state: &AppState,
    pubkey: &nostr::PublicKey,
    device: Option<&DevicePubkey>,
    claimed_owner: Option<nostr::PublicKey>,
    surface: Surface,
    subject: &str,
) -> Result<(), KeyRefused> {
    // A registered key acts for its registry owner only. With no owner key on
    // record there is nothing to compare, so nothing is refused.
    let check_owner = |owner: Option<[u8; 32]>| match (claimed_owner, owner) {
        (Some(claimed), Some(owner)) if owner != claimed.to_bytes() => {
            Err(KeyRefused::OwnerMismatch)
        }
        _ => Ok(()),
    };
    let key: CacheKey = (pubkey.to_bytes(), device.map(|key| *key.as_bytes()));
    let cache = &state.identity.admissions;
    if let Some(owner) = cache.admitted.get(&key) {
        return check_owner(owner);
    }
    match cache.denied.get(&key) {
        Some(Denied::Unregistered) => {
            device::report(state, surface, subject, "unregistered", false);
            return Ok(());
        }
        Some(Denied::Refused(reason)) => return Err(KeyRefused::Key(reason)),
        None => {}
    }
    match state.db.key_admission(&key.0, key.1.as_ref()).await {
        Ok(KeyAdmission::Admitted { owner_pubkey, .. }) => {
            cache.admitted.insert(key, owner_pubkey);
            check_owner(owner_pubkey)
        }
        Ok(KeyAdmission::Unknown) => {
            // Cached briefly: a new registration takes effect within DENY_TTL.
            cache.denied.insert(key, Denied::Unregistered);
            device::report(state, surface, subject, "unregistered", false);
            Ok(())
        }
        Ok(KeyAdmission::Refused { role, reason }) => {
            tracing::debug!(
                subject,
                role = role.as_str(),
                reason = reason.as_str(),
                "key refused"
            );
            cache.denied.insert(key, Denied::Refused(reason));
            Err(KeyRefused::Key(reason))
        }
        Err(error) => {
            tracing::warn!(%error, subject, "key admission lookup failed");
            Err(KeyRefused::Unavailable)
        }
    }
}

/// Device keys and device-tagged auth events for tests.
#[cfg(test)]
pub(crate) mod test_support {
    use kuzz_auth::device_proof::{event_proof_message, DevicePubkey, DEVICE_TAG};
    use p256::ecdsa::signature::Signer as _;
    use p256::ecdsa::{Signature, SigningKey};

    /// A device key pair from a fixed seed byte.
    pub(crate) fn device(seed: u8) -> (SigningKey, DevicePubkey) {
        let signing = SigningKey::from_slice(&[seed; 32]).expect("device key");
        let public =
            DevicePubkey::from_sec1(signing.verifying_key().to_encoded_point(true).as_bytes())
                .expect("device pubkey");
        (signing, public)
    }

    /// A NIP-42 AUTH event by `keys` for `challenge` and `relay`, with a
    /// `device` tag signed by `device` (`None`: no tag).
    pub(crate) fn auth_event(
        keys: &nostr::Keys,
        challenge: &str,
        relay: &str,
        device: Option<&SigningKey>,
    ) -> nostr::Event {
        signed_event(
            keys,
            nostr::Kind::Authentication,
            vec![
                vec!["relay".to_owned(), relay.to_owned()],
                vec!["challenge".to_owned(), challenge.to_owned()],
            ],
            "",
            device,
        )
    }

    /// An event of `kind` by `keys` with `tags` and `content`, plus a
    /// `device` tag signed by `device` (`None`: no tag).
    pub(crate) fn signed_event(
        keys: &nostr::Keys,
        kind: nostr::Kind,
        mut tags: Vec<Vec<String>>,
        content: &str,
        device: Option<&SigningKey>,
    ) -> nostr::Event {
        let created_at = nostr::Timestamp::now();
        if let Some(device) = device {
            let public =
                DevicePubkey::from_sec1(device.verifying_key().to_encoded_point(true).as_bytes())
                    .expect("device pubkey");
            tags.push(vec![DEVICE_TAG.to_owned(), public.to_hex(), String::new()]);
            let message = event_proof_message(
                &keys.public_key().to_hex(),
                created_at.as_secs(),
                kind.as_u16(),
                &tags,
                content,
            );
            let signature: Signature = device.sign(message.as_bytes());
            let last = tags.len() - 1;
            tags[last][2] = hex::encode(signature.to_bytes());
        }
        nostr::EventBuilder::new(kind, content)
            .tags(
                tags.into_iter()
                    .map(|tag| nostr::Tag::parse(tag).expect("tag")),
            )
            .custom_created_at(created_at)
            .sign_with_keys(keys)
            .expect("sign")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_names_and_the_signed_out_class() {
        assert_eq!(KeyRefused::BadTag.as_str(), "bad_sig");
        assert_eq!(KeyRefused::Unavailable.as_str(), "unavailable");
        assert_eq!(KeyRefused::Key(KeyRefusal::NoDevice).as_str(), "no_device");
        let signed_out: Vec<_> = [
            KeyRefusal::PrincipalDisabled,
            KeyRefusal::KeyRevoked,
            KeyRefusal::Unbound,
            KeyRefusal::NoDevice,
            KeyRefusal::DeviceUnknown,
            KeyRefusal::DeviceRevoked,
            KeyRefusal::WrongHost,
            KeyRefusal::NotSignedIn,
        ]
        .into_iter()
        .filter(|reason| KeyRefused::Key(*reason).signed_out())
        .collect();
        assert_eq!(
            signed_out,
            [KeyRefusal::DeviceRevoked, KeyRefusal::NotSignedIn]
        );
        assert!(!KeyRefused::BadTag.signed_out());
        assert!(!KeyRefused::Unavailable.signed_out());
        assert!(KeyRefused::Key(KeyRefusal::Unbound).unbound());
        assert!(!KeyRefused::Key(KeyRefusal::WrongHost).unbound());
        assert!(!KeyRefused::Unavailable.unbound());
    }

    #[test]
    fn client_reasons_and_http_status() {
        for (refused, reason, status) in [
            (
                KeyRefused::Unavailable,
                "error: internal error checking device",
                503,
            ),
            (
                KeyRefused::Key(KeyRefusal::NotSignedIn),
                "restricted: signed_out",
                403,
            ),
            (
                KeyRefused::Key(KeyRefusal::DeviceRevoked),
                "restricted: signed_out",
                403,
            ),
            (
                KeyRefused::Key(KeyRefusal::KeyRevoked),
                "restricted: revoked",
                403,
            ),
            (
                KeyRefused::Key(KeyRefusal::PrincipalDisabled),
                "restricted: revoked",
                403,
            ),
            (KeyRefused::OwnerMismatch, "restricted: owner", 403),
            (KeyRefused::Unsigned, "restricted: device", 403),
            (KeyRefused::BadTag, "restricted: device", 403),
            (
                KeyRefused::Key(KeyRefusal::WrongHost),
                "restricted: device",
                403,
            ),
        ] {
            assert_eq!(refused.reason(), reason, "{refused:?}");
            assert_eq!(refused.http_error().0.as_u16(), status, "{refused:?}");
        }
        assert_eq!(KeyRefused::Unsigned.as_str(), "unsigned");
        assert_eq!(KeyRefused::OwnerMismatch.as_str(), "owner_mismatch");
    }

    #[test]
    fn nip98_event_reads_only_a_nostr_authorization() {
        let keys = nostr::Keys::generate();
        let event = test_support::auth_event(&keys, "c", "wss://r", None);
        let header = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(
                axum::http::header::AUTHORIZATION,
                value.parse().expect("header"),
            );
            headers
        };
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("json"));
        let parsed = nip98_event(&header(&format!("Nostr {encoded}")))
            .expect("nostr scheme")
            .expect("event");
        assert_eq!(parsed.id, event.id);
        assert!(nip98_event(&header("Bearer abc")).is_none());
        assert!(nip98_event(&HeaderMap::new()).is_none());
        assert!(matches!(nip98_event(&header("Nostr !!!")), Some(None)));
        // Exactly the verifier's reading: case-sensitive prefix, no trim.
        assert!(nip98_event(&header(&format!("nostr {encoded}"))).is_none());
        assert!(matches!(
            nip98_event(&header(&format!("Nostr  {encoded}"))),
            Some(None)
        ));
    }

    /// Refusals and "unregistered" answers are reused for DENY_TTL without
    /// a lookup (an unreachable database would answer `unavailable`), and a
    /// login clears them for its account key and for every key on its
    /// device; other devices keep theirs.
    #[tokio::test]
    async fn negative_answers_are_cached_until_a_login_clears_them() {
        let state = crate::state::tests::test_state_with_config(
            "postgres://kuzz:kuzz_dev@127.0.0.1:1/kuzz",
            |config| config.auth_token.device_admission = DeviceAdmissionMode::Enforce,
        )
        .await;
        let person = nostr::Keys::generate().public_key();
        let agent = nostr::Keys::generate().public_key();
        let stranger = nostr::Keys::generate().public_key();
        let (_, pc) = test_support::device(11);
        let (_, laptop) = test_support::device(12);
        let denied = &state.identity.admissions.denied;
        let refused = Denied::Refused(KeyRefusal::NotSignedIn);
        denied.insert((person.to_bytes(), Some(*laptop.as_bytes())), refused);
        denied.insert((agent.to_bytes(), Some(*pc.as_bytes())), refused);
        denied.insert((agent.to_bytes(), Some(*laptop.as_bytes())), refused);
        denied.insert((stranger.to_bytes(), None), Denied::Unregistered);
        let admit = |key: nostr::PublicKey, device: Option<DevicePubkey>| {
            let state = &state;
            async move { admit_key(state, &key, Ok(device), Surface::Http).await }
        };
        let signed_out = Err(KeyRefused::Key(KeyRefusal::NotSignedIn));
        assert_eq!(admit(person, Some(laptop)).await, signed_out);
        assert_eq!(admit(agent, Some(pc)).await, signed_out);
        assert_eq!(admit(stranger, None).await, Ok(()));

        // The person signs in on the pc.
        forget_login(&state, Some(person.to_bytes()), Some(*pc.as_bytes()));
        denied.run_pending_tasks();
        assert_eq!(
            admit(person, Some(laptop)).await,
            Err(KeyRefused::Unavailable)
        );
        assert_eq!(admit(agent, Some(pc)).await, Err(KeyRefused::Unavailable));
        assert_eq!(admit(agent, Some(laptop)).await, signed_out);
        assert_eq!(admit(stranger, None).await, Ok(()));
    }

    /// A cached admission still checks the NIP-OA owner: the registry owner
    /// and no claim pass, another owner is refused under `enforce`; with no
    /// owner on record nothing is refused.
    #[tokio::test]
    async fn a_claimed_owner_must_be_the_registry_owner() {
        let state = crate::state::tests::test_state_with_config(
            "postgres://kuzz:kuzz_dev@127.0.0.1:1/kuzz",
            |config| config.auth_token.device_admission = DeviceAdmissionMode::Enforce,
        )
        .await;
        let pubkey = nostr::Keys::generate().public_key();
        let owner = nostr::Keys::generate().public_key();
        let stranger = nostr::Keys::generate().public_key();
        let (_, device) = test_support::device(7);
        let (_, other_device) = test_support::device(8);
        let cache = &state.identity.admissions.admitted;
        cache.insert(
            (pubkey.to_bytes(), Some(*device.as_bytes())),
            Some(owner.to_bytes()),
        );
        cache.insert((pubkey.to_bytes(), Some(*other_device.as_bytes())), None);
        let admit = |device, claimed| {
            admit_key_with_owner(&state, &pubkey, Ok(Some(device)), claimed, Surface::Ws)
        };
        assert_eq!(admit(device, None).await, Ok(()));
        assert_eq!(admit(device, Some(owner)).await, Ok(()));
        assert_eq!(
            admit(device, Some(stranger)).await,
            Err(KeyRefused::OwnerMismatch)
        );
        assert_eq!(admit(other_device, Some(stranger)).await, Ok(()));
    }

    /// The bridge entrance: no `Authorization: Nostr` (the `X-Pubkey` dev
    /// header) is `unsigned`, a header for another key is a bad tag; both
    /// only under `enforce`, and `off` reads nothing.
    #[tokio::test]
    async fn the_nip98_entrance_refuses_unsigned_and_foreign_events() {
        let keys = nostr::Keys::generate();
        let event = test_support::auth_event(&keys, "c", "wss://r", None);
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&event).expect("json"));
        let mut signed = HeaderMap::new();
        signed.insert(
            axum::http::header::AUTHORIZATION,
            format!("Nostr {encoded}").parse().expect("header"),
        );
        let stranger = nostr::Keys::generate().public_key();
        for (mode, refuses) in [
            (DeviceAdmissionMode::Enforce, true),
            (DeviceAdmissionMode::Log, false),
            (DeviceAdmissionMode::Off, false),
        ] {
            let state = crate::state::tests::test_state_with_config(
                "postgres://kuzz:kuzz_dev@127.0.0.1:1/kuzz",
                |config| config.auth_token.device_admission = mode,
            )
            .await;
            let unsigned =
                admit_key_for_nip98(&state, &HeaderMap::new(), &keys.public_key(), Surface::Http)
                    .await;
            let foreign = admit_key_for_nip98(&state, &signed, &stranger, Surface::Http).await;
            if refuses {
                assert_eq!(unsigned, Err(KeyRefused::Unsigned));
                assert_eq!(foreign, Err(KeyRefused::BadTag));
            } else {
                assert_eq!(unsigned, Ok(()), "{mode:?}");
                assert_eq!(foreign, Ok(()), "{mode:?}");
            }
        }
    }

    /// A cached admission needs no lookup: with an unreachable database
    /// an uncached pair is refused under `enforce`, a cached one admitted.
    /// The cache is keyed on the device key too.
    #[tokio::test]
    async fn a_cached_admission_skips_the_lookup_for_its_device_only() {
        let state = crate::state::tests::test_state_with_config(
            "postgres://kuzz:kuzz_dev@127.0.0.1:1/kuzz",
            |config| config.auth_token.device_admission = DeviceAdmissionMode::Enforce,
        )
        .await;
        let pubkey = nostr::Keys::generate().public_key();
        let (_, device) = test_support::device(9);

        assert_eq!(
            admit_key(&state, &pubkey, Ok(Some(device)), Surface::Ws).await,
            Err(KeyRefused::Unavailable)
        );
        state
            .identity
            .admissions
            .admitted
            .insert((pubkey.to_bytes(), Some(*device.as_bytes())), None);
        assert_eq!(
            admit_key(&state, &pubkey, Ok(Some(device)), Surface::Ws).await,
            Ok(())
        );
        assert_eq!(
            admit_key(&state, &pubkey, Ok(None), Surface::Ws).await,
            Err(KeyRefused::Unavailable)
        );
        // Evicting the key drops it on every device.
        state.identity.admissions.evict_key(&pubkey.to_bytes());
        assert_eq!(
            admit_key(&state, &pubkey, Ok(Some(device)), Surface::Ws).await,
            Err(KeyRefused::Unavailable)
        );
        // A bad tag is refused before any cache or lookup.
        assert_eq!(
            admit_key(
                &state,
                &pubkey,
                Err(DeviceProofError::BadSignature),
                Surface::Ws
            )
            .await,
            Err(KeyRefused::BadTag)
        );
    }

    /// `log` admits what `enforce` refuses but reports it; `off` does no
    /// work at all (no lookup, so no "unavailable" report).
    #[tokio::test(flavor = "current_thread")]
    async fn log_reports_and_admits_off_does_nothing() {
        for (mode, reports) in [(DeviceAdmissionMode::Log, 2), (DeviceAdmissionMode::Off, 0)] {
            let recorder = metrics_util::debugging::DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            let _guard = metrics::set_default_local_recorder(&recorder);
            let state = crate::state::tests::test_state_with_config(
                "postgres://kuzz:kuzz_dev@127.0.0.1:1/kuzz",
                |config| config.auth_token.device_admission = mode,
            )
            .await;
            let pubkey = nostr::Keys::generate().public_key();
            assert_eq!(
                admit_key(&state, &pubkey, Ok(None), Surface::Ws).await,
                Ok(())
            );
            assert_eq!(
                admit_key(
                    &state,
                    &pubkey,
                    Err(DeviceProofError::Malformed),
                    Surface::Ws
                )
                .await,
                Ok(())
            );
            let counted: u64 = snapshotter
                .snapshot()
                .into_vec()
                .into_iter()
                .filter(|(key, ..)| key.key().name() == "kuzz_device_proof_refusals_total")
                .map(|(.., value)| match value {
                    metrics_util::debugging::DebugValue::Counter(value) => value,
                    _ => 0,
                })
                .sum();
            assert_eq!(counted, reports, "{mode:?}");
        }
    }
}
