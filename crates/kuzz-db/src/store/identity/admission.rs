//! Key admission (device-bound keys plan §2.2): may this message key be used
//! from this device right now?
//!
//! - person key (`account_key_backups`): on any device of its account that
//!   holds the presented device key and has a live login session;
//! - Kuzz agent key (`key_registry` role `agent`, principal = owner): only on
//!   its host device, while the owner has a live login session there;
//! - external agent key (role `external`): only on its own paired device
//!   until that device is revoked; no login is needed.
//!
//! A saved key is useless without a live login on that device. There is no
//! key ban: a revoke only revokes, and signing back in needs the identity
//! provider again anyway. One query per check; the relay caches positive
//! answers briefly.

use chrono::{DateTime, Utc};
use kuzz_datastore_tracing::datastore_span;
use sqlx::{PgPool, Row as _};
use uuid::Uuid;

use kuzz_core::principal::PrincipalId;

use super::principal_from_bytes;
use crate::error::Result;

/// What a message key is registered as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRole {
    /// An account's own key.
    Person,
    /// A Kuzz-managed agent's key, owned by an account.
    Agent,
    /// An external agent paired to an account.
    External,
}

impl KeyRole {
    /// Stable name for logs and metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Agent => "agent",
            Self::External => "external",
        }
    }
}

/// Why a registered key may not be used from a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRefusal {
    /// The account (or agent owner) is disabled.
    PrincipalDisabled,
    /// The key itself was revoked.
    KeyRevoked,
    /// No device proof was presented.
    NoDevice,
    /// The device key is not a device of the key's account.
    DeviceUnknown,
    /// The device was signed out remotely or revoked.
    DeviceRevoked,
    /// An agent key used from a device other than its host.
    WrongHost,
    /// The account has no live login session on the device.
    NotSignedIn,
}

impl KeyRefusal {
    /// Stable name for logs and metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PrincipalDisabled => "principal_disabled",
            Self::KeyRevoked => "key_revoked",
            Self::NoDevice => "no_device",
            Self::DeviceUnknown => "device_unknown",
            Self::DeviceRevoked => "device_revoked",
            Self::WrongHost => "wrong_host",
            Self::NotSignedIn => "not_signed_in",
        }
    }
}

/// The answer for one (message key, device key) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAdmission {
    /// The key is not registered anywhere.
    Unknown,
    /// The key may be used from this device.
    Admitted {
        /// What the key is.
        role: KeyRole,
        /// The account (person) or owner (agents).
        principal: PrincipalId,
        /// The device row it is used from.
        device_id: Uuid,
    },
    /// The key is registered but may not be used from this device.
    Refused {
        /// What the key is.
        role: KeyRole,
        /// Why not.
        reason: KeyRefusal,
    },
}

/// The facts [`decide`] needs, as read by [`key_admission`].
#[derive(Debug, Clone)]
pub(super) struct AdmissionFacts {
    pub role: KeyRole,
    pub principal: PrincipalId,
    pub key_revoked: bool,
    pub principal_disabled: bool,
    /// The device row checked: for a person the account's row holding the
    /// presented device key (a live row first), for agents the host row.
    pub device_id: Option<Uuid>,
    /// Whether that row holds the presented device key.
    pub device_key_matches: bool,
    pub device_revoked: bool,
    /// The row has an unrevoked session with an unused, unexpired refresh token.
    pub live_session: bool,
}

/// Decide admission from the stored facts. `device_presented` is whether the
/// caller sent a verified device key at all.
pub(super) fn decide(facts: Option<AdmissionFacts>, device_presented: bool) -> KeyAdmission {
    let Some(facts) = facts else {
        return KeyAdmission::Unknown;
    };
    let refuse = |reason| KeyAdmission::Refused {
        role: facts.role,
        reason,
    };
    if facts.principal_disabled {
        return refuse(KeyRefusal::PrincipalDisabled);
    }
    if facts.key_revoked {
        return refuse(KeyRefusal::KeyRevoked);
    }
    if !device_presented {
        return refuse(KeyRefusal::NoDevice);
    }
    let device_id = match facts.device_id {
        Some(id) if facts.device_key_matches => id,
        _ if facts.role == KeyRole::Person => return refuse(KeyRefusal::DeviceUnknown),
        _ => return refuse(KeyRefusal::WrongHost),
    };
    if facts.device_revoked {
        return refuse(KeyRefusal::DeviceRevoked);
    }
    if facts.role != KeyRole::External && !facts.live_session {
        return refuse(KeyRefusal::NotSignedIn);
    }
    KeyAdmission::Admitted {
        role: facts.role,
        principal: facts.principal,
        device_id,
    }
}

pub(super) async fn key_admission(
    pool: &PgPool,
    pubkey: &[u8; 32],
    device_pubkey: Option<&[u8; 33]>,
) -> Result<KeyAdmission> {
    // A revoked device row may share its key with a newer live one (the key
    // index covers live rows only), so the live row is preferred.
    let row = sqlx::query(
        "WITH k AS ( \
           SELECT 'person'::text AS role, b.account_id AS principal_id, \
                  NULL::uuid AS host_device_id, NULL::timestamptz AS key_revoked_at \
             FROM account_key_backups b WHERE b.pubkey = $1 \
           UNION ALL \
           SELECT r.role, r.principal_id, r.host_device_id, r.revoked_at \
             FROM key_registry r WHERE r.pubkey = $1 \
         ) \
         SELECT k.role, k.principal_id, k.key_revoked_at, p.disabled_at, \
                d.id AS device_id, COALESCE(d.device_pubkey = $2, false) AS device_key_matches, \
                d.revoked_at AS device_revoked_at, \
                EXISTS (SELECT 1 FROM sessions s \
                         WHERE s.device_id = d.id AND s.revoked_at IS NULL \
                           AND EXISTS (SELECT 1 FROM refresh_tokens rt \
                                        WHERE rt.session_id = s.id AND rt.used_at IS NULL \
                                          AND rt.expires_at > now())) AS live_session \
           FROM k \
           JOIN principals p ON p.id = k.principal_id \
           LEFT JOIN LATERAL ( \
             SELECT d.id, d.device_pubkey, d.revoked_at FROM devices d \
              WHERE d.principal_id = k.principal_id \
                AND CASE WHEN k.role = 'person' THEN d.device_pubkey = $2 \
                         ELSE d.id = k.host_device_id END \
              ORDER BY d.revoked_at IS NULL DESC, d.last_seen_at DESC LIMIT 1 \
           ) d ON true \
          ORDER BY k.role = 'person' DESC \
          LIMIT 1",
    )
    .bind(pubkey.as_slice())
    .bind(device_pubkey.map(|key| key.to_vec()))
    .fetch_optional(pool)
    .await?;
    let facts = match row {
        None => None,
        Some(row) => {
            let role = match row.try_get::<String, _>("role")?.as_str() {
                "person" => KeyRole::Person,
                "agent" => KeyRole::Agent,
                _ => KeyRole::External,
            };
            let principal: Vec<u8> = row.try_get("principal_id")?;
            Some(AdmissionFacts {
                role,
                principal: principal_from_bytes(&principal)?,
                key_revoked: row
                    .try_get::<Option<DateTime<Utc>>, _>("key_revoked_at")?
                    .is_some(),
                principal_disabled: row
                    .try_get::<Option<DateTime<Utc>>, _>("disabled_at")?
                    .is_some(),
                device_id: row.try_get("device_id")?,
                device_key_matches: row.try_get("device_key_matches")?,
                device_revoked: row
                    .try_get::<Option<DateTime<Utc>>, _>("device_revoked_at")?
                    .is_some(),
                live_session: row.try_get("live_session")?,
            })
        }
    };
    Ok(decide(facts, device_pubkey.is_some()))
}

impl crate::Db {
    /// May `pubkey` be used from the device holding `device_pubkey` (`None`:
    /// no device proof)? See the module docs for the rules.
    #[datastore_span(name = "key_admission", system = "postgresql")]
    pub async fn key_admission(
        &self,
        pubkey: &[u8; 32],
        device_pubkey: Option<&[u8; 33]>,
    ) -> Result<KeyAdmission> {
        key_admission(&self.pool, pubkey, device_pubkey).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(role: KeyRole) -> AdmissionFacts {
        AdmissionFacts {
            role,
            principal: PrincipalId::from_slice(&[1u8; 32]).expect("principal"),
            key_revoked: false,
            principal_disabled: false,
            device_id: Some(Uuid::from_u128(7)),
            device_key_matches: true,
            device_revoked: false,
            live_session: true,
        }
    }

    fn refused(admission: KeyAdmission) -> Option<KeyRefusal> {
        match admission {
            KeyAdmission::Refused { reason, .. } => Some(reason),
            _ => None,
        }
    }

    #[test]
    fn every_role_is_admitted_on_its_device_with_its_session() {
        for role in [KeyRole::Person, KeyRole::Agent, KeyRole::External] {
            assert_eq!(
                decide(Some(facts(role)), true),
                KeyAdmission::Admitted {
                    role,
                    principal: facts(role).principal,
                    device_id: Uuid::from_u128(7),
                }
            );
        }
        assert_eq!(decide(None, true), KeyAdmission::Unknown);
        assert_eq!(decide(None, false), KeyAdmission::Unknown);
    }

    #[test]
    fn a_key_without_its_device_is_refused() {
        // No proof at all: refused even when a host row exists.
        for role in [KeyRole::Person, KeyRole::Agent, KeyRole::External] {
            assert_eq!(
                refused(decide(Some(facts(role)), false)),
                Some(KeyRefusal::NoDevice)
            );
        }

        let mut unknown = facts(KeyRole::Person);
        unknown.device_id = None;
        unknown.device_key_matches = false;
        assert_eq!(
            refused(decide(Some(unknown), true)),
            Some(KeyRefusal::DeviceUnknown)
        );

        for role in [KeyRole::Agent, KeyRole::External] {
            let mut elsewhere = facts(role);
            elsewhere.device_key_matches = false;
            assert_eq!(
                refused(decide(Some(elsewhere), true)),
                Some(KeyRefusal::WrongHost)
            );
            let mut host_gone = facts(role);
            host_gone.device_id = None;
            host_gone.device_key_matches = false;
            assert_eq!(
                refused(decide(Some(host_gone), true)),
                Some(KeyRefusal::WrongHost)
            );
        }

        let mut revoked = facts(KeyRole::Person);
        revoked.device_revoked = true;
        assert_eq!(
            refused(decide(Some(revoked), true)),
            Some(KeyRefusal::DeviceRevoked)
        );
    }

    #[test]
    fn a_saved_key_needs_a_live_login_except_for_external_agents() {
        for role in [KeyRole::Person, KeyRole::Agent] {
            let mut signed_out = facts(role);
            signed_out.live_session = false;
            assert_eq!(
                refused(decide(Some(signed_out), true)),
                Some(KeyRefusal::NotSignedIn)
            );
        }

        // An external agent needs no owner session, only its own device.
        let mut external = facts(KeyRole::External);
        external.live_session = false;
        assert!(matches!(
            decide(Some(external), true),
            KeyAdmission::Admitted { .. }
        ));
    }

    #[test]
    fn account_and_key_state_come_first() {
        let mut disabled = facts(KeyRole::Person);
        disabled.principal_disabled = true;
        disabled.device_id = None;
        assert_eq!(
            refused(decide(Some(disabled), false)),
            Some(KeyRefusal::PrincipalDisabled)
        );

        let mut key_revoked = facts(KeyRole::External);
        key_revoked.key_revoked = true;
        assert_eq!(
            refused(decide(Some(key_revoked), true)),
            Some(KeyRefusal::KeyRevoked)
        );
    }
}
