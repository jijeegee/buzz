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

use std::time::Duration;

use kuzz_auth::device_proof::{DeviceProofError, DevicePubkey, DEVICE_PUBKEY_LEN};
use kuzz_db::identity::{KeyAdmission, KeyRefusal};

use super::config::DeviceAdmissionMode;
use super::device::{self, Surface};
use crate::state::AppState;

/// How long a positive answer is reused.
const ADMIT_TTL: Duration = Duration::from_secs(30);

/// Why a key was refused (only returned under `enforce`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyRefused {
    /// The auth event's `device` tag is malformed or its signature fails.
    BadTag,
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

    /// The key or its account is gone for good (a deleted agent, a disabled
    /// account): a supervisor should park it too, never restart it.
    pub(crate) fn revoked(self) -> bool {
        matches!(
            self,
            Self::Key(KeyRefusal::KeyRevoked | KeyRefusal::PrincipalDisabled)
        )
    }
}

type CacheKey = ([u8; 32], Option<[u8; DEVICE_PUBKEY_LEN]>);

/// Recently admitted (key, device key) pairs.
pub(crate) struct AdmissionCache(moka::sync::Cache<CacheKey, ()>);

impl Default for AdmissionCache {
    fn default() -> Self {
        Self(
            moka::sync::Cache::builder()
                .max_capacity(100_000)
                .time_to_live(ADMIT_TTL)
                .build(),
        )
    }
}

/// Admit `pubkey` on `surface` given its auth event's verified `device` tag
/// (see [`kuzz_auth::device_proof::verify_event_device_tag`]). `Err` only
/// under `enforce`.
pub(crate) async fn admit_key(
    state: &AppState,
    pubkey: &nostr::PublicKey,
    device_tag: Result<Option<DevicePubkey>, DeviceProofError>,
    surface: Surface,
) -> Result<(), KeyRefused> {
    let mode = device::mode(state, surface);
    if mode == DeviceAdmissionMode::Off {
        return Ok(());
    }
    let subject = pubkey.to_hex();
    let outcome = match device_tag {
        Err(_) => Err(KeyRefused::BadTag),
        Ok(device) => lookup(state, pubkey, device.as_ref(), surface, &subject).await,
    };
    let Err(refused) = outcome else {
        return Ok(());
    };
    let enforced = mode == DeviceAdmissionMode::Enforce;
    device::report(state, surface, &subject, refused.as_str(), enforced);
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
    surface: Surface,
    subject: &str,
) -> Result<(), KeyRefused> {
    let key: CacheKey = (pubkey.to_bytes(), device.map(|key| *key.as_bytes()));
    let cache = &state.identity.admissions.0;
    if cache.contains_key(&key) {
        return Ok(());
    }
    match state.db.key_admission(&key.0, key.1.as_ref()).await {
        Ok(KeyAdmission::Admitted { .. }) => {
            cache.insert(key, ());
            Ok(())
        }
        Ok(KeyAdmission::Unknown) => {
            // Not cached: a registration must take effect at once.
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
        let created_at = nostr::Timestamp::now();
        let mut tags = vec![
            vec!["relay".to_owned(), relay.to_owned()],
            vec!["challenge".to_owned(), challenge.to_owned()],
        ];
        if let Some(device) = device {
            let public =
                DevicePubkey::from_sec1(device.verifying_key().to_encoded_point(true).as_bytes())
                    .expect("device pubkey");
            tags.push(vec![DEVICE_TAG.to_owned(), public.to_hex(), String::new()]);
            let message = event_proof_message(
                &keys.public_key().to_hex(),
                created_at.as_secs(),
                nostr::Kind::Authentication.as_u16(),
                &tags,
                "",
            );
            let signature: Signature = device.sign(message.as_bytes());
            tags[2][2] = hex::encode(signature.to_bytes());
        }
        nostr::EventBuilder::new(nostr::Kind::Authentication, "")
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
            .0
            .insert((pubkey.to_bytes(), Some(*device.as_bytes())), ());
        assert_eq!(
            admit_key(&state, &pubkey, Ok(Some(device)), Surface::Ws).await,
            Ok(())
        );
        assert_eq!(
            admit_key(&state, &pubkey, Ok(None), Surface::Ws).await,
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
