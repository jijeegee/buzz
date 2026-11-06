//! May this key act for its owner now? (AI tools plan §3, stage S1.)
//!
//! [`agent_authority`] is the one answer every owner-tools gate uses. It
//! reads the database on each call (no cache), so the owner's switch, a
//! revoke or a ban counts at once. It fails closed: with the WebSocket
//! device surface `off` devices are not read, so it always says no.
//!
//! In order: the key must be admitted from this device by
//! [`kuzz_db::Db::key_admission`], unchanged (revoked key or device, wrong
//! host, disabled owner, and for app agents no live owner session); be an
//! agent or external key; have its owner's switch `full`, set in this
//! community, while it does not answer anyone; have a known owner key; and
//! neither it nor its owner may be banned here. In S1 only `/agents/me` and
//! tests call it; nothing is granted from it yet.

use kuzz_core::CommunityId;
use kuzz_db::identity::{KeyAdmission, KeyAuthority, KeyRefusal, KeyRole, RespondClass, ToolLevel};

use crate::identity::config::DeviceAdmissionMode;
use crate::identity::device::{self, Surface};
use crate::state::AppState;

/// Why a key may not act for its owner now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Denial {
    /// Device admission for WebSocket keys is off: nothing is evaluated.
    AdmissionOff,
    /// The key is not registered.
    Unregistered,
    /// Registered, not admitted from this device.
    NotAdmitted(KeyRefusal),
    /// A person's own key.
    NotAnAgent,
    /// The owner never set the switch.
    NotSet,
    /// The owner turned it off.
    Limited,
    /// Set in another community.
    OtherCommunity,
    /// The agent answers anyone.
    AnswersAnyone,
    /// The owner has no account key.
    OwnerUnknown,
    /// The agent or its owner is banned in this community.
    Banned,
}

impl Denial {
    /// Stable name for answers and logs.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AdmissionOff => "admission_off",
            Self::Unregistered => "unregistered",
            Self::NotAdmitted(refusal) => refusal.as_str(),
            Self::NotAnAgent => "not_an_agent",
            Self::NotSet => "not_set",
            Self::Limited => "limited",
            Self::OtherCommunity => "other_community",
            Self::AnswersAnyone => "respond_anyone",
            Self::OwnerUnknown => "owner_unknown",
            Self::Banned => "banned",
        }
    }
}

/// The answer and the facts it was read from (`None` when nothing was read).
pub(crate) struct Authority {
    /// `Ok`: may act for `owner`; `Err`: why not.
    pub(crate) decision: Result<[u8; 32], Denial>,
    /// What was read.
    pub(crate) facts: Option<KeyAuthority>,
}

/// May `key`, used from the device holding `device`, act for its owner in
/// `community` now? See the module docs. `Err` only when the store fails.
pub(crate) async fn agent_authority(
    state: &AppState,
    key: &[u8; 32],
    device: Option<&[u8; 33]>,
    community: CommunityId,
) -> Result<Authority, kuzz_db::DbError> {
    if device::mode(state, Surface::Ws) == DeviceAdmissionMode::Off {
        return Ok(Authority {
            decision: Err(Denial::AdmissionOff),
            facts: None,
        });
    }
    let facts = state.db.key_authority(key, device).await?;
    let decision = match decide(&facts, community) {
        Ok(owner) => not_banned(state, community, key, owner).await?,
        Err(denial) => Err(denial),
    };
    Ok(Authority {
        decision,
        facts: Some(facts),
    })
}

/// Everything but the ban check, from the stored facts alone.
pub(crate) fn decide(facts: &KeyAuthority, community: CommunityId) -> Result<[u8; 32], Denial> {
    let (role, owner) = match facts.admission {
        KeyAdmission::Unknown => return Err(Denial::Unregistered),
        KeyAdmission::Refused { reason, .. } => return Err(Denial::NotAdmitted(reason)),
        KeyAdmission::Admitted {
            role, owner_pubkey, ..
        } => (role, owner_pubkey),
    };
    if role == KeyRole::Person {
        return Err(Denial::NotAnAgent);
    }
    let Some(tool) = &facts.tool else {
        return Err(Denial::Unregistered);
    };
    match tool.level {
        None => return Err(Denial::NotSet),
        Some(ToolLevel::Limited) => return Err(Denial::Limited),
        Some(ToolLevel::Full) => {}
    }
    if tool.community != Some(community) {
        return Err(Denial::OtherCommunity);
    }
    if tool.respond_class == Some(RespondClass::Anyone) {
        return Err(Denial::AnswersAnyone);
    }
    owner.ok_or(Denial::OwnerUnknown)
}

/// The agent's own ban, and its owner's: a paired agent has no
/// `users.agent_owner_pubkey`, so the owner from admission is read too.
async fn not_banned(
    state: &AppState,
    community: CommunityId,
    key: &[u8; 32],
    owner: [u8; 32],
) -> Result<Result<[u8; 32], Denial>, kuzz_db::DbError> {
    for pubkey in [key, &owner] {
        if state
            .db
            .moderation_restriction_state(community, pubkey)
            .await?
            .banned
        {
            return Ok(Err(Denial::Banned));
        }
    }
    Ok(Ok(owner))
}

#[cfg(test)]
mod tests {
    use kuzz_core::principal::PrincipalId;
    use kuzz_db::identity::AgentToolState;
    use uuid::Uuid;

    use super::*;

    const OWNER: [u8; 32] = [2u8; 32];

    fn here() -> CommunityId {
        CommunityId::from_uuid(Uuid::from_u128(1))
    }

    fn full_agent(role: KeyRole) -> KeyAuthority {
        let principal = PrincipalId::from_slice(&[1u8; 32]).expect("principal");
        KeyAuthority {
            admission: KeyAdmission::Admitted {
                role,
                principal,
                device_id: Uuid::from_u128(7),
                owner_pubkey: Some(OWNER),
            },
            tool: Some(AgentToolState {
                pubkey: [3u8; 32],
                role,
                principal,
                level: Some(ToolLevel::Full),
                community: Some(here()),
                respond_class: Some(RespondClass::OwnerOnly),
                host_device_id: Some(Uuid::from_u128(7)),
                host_device_name: Some("pc".into()),
                revoked: false,
            }),
        }
    }

    fn denied(facts: KeyAuthority) -> Denial {
        decide(&facts, here()).expect_err("denied")
    }

    #[test]
    fn a_full_agent_or_external_key_acts_for_its_owner() {
        for role in [KeyRole::Agent, KeyRole::External] {
            assert_eq!(decide(&full_agent(role), here()), Ok(OWNER));
        }
    }

    #[test]
    fn every_other_state_is_denied() {
        let refused = |reason| {
            let mut facts = full_agent(KeyRole::Agent);
            facts.admission = KeyAdmission::Refused {
                role: KeyRole::Agent,
                reason,
            };
            facts
        };
        // Revoked key, disconnected or signed-out device, deleted or wrong
        // host: admission refuses, and says why.
        for reason in [
            KeyRefusal::KeyRevoked,
            KeyRefusal::DeviceRevoked,
            KeyRefusal::WrongHost,
            KeyRefusal::NotSignedIn,
            KeyRefusal::Unbound,
            KeyRefusal::PrincipalDisabled,
            KeyRefusal::NoDevice,
        ] {
            assert_eq!(denied(refused(reason)), Denial::NotAdmitted(reason));
        }

        let mut unknown = full_agent(KeyRole::Agent);
        unknown.admission = KeyAdmission::Unknown;
        unknown.tool = None;
        assert_eq!(denied(unknown), Denial::Unregistered);

        let mut person = full_agent(KeyRole::Person);
        person.tool = None;
        assert_eq!(denied(person), Denial::NotAnAgent);

        let with = |change: fn(&mut AgentToolState)| {
            let mut facts = full_agent(KeyRole::Agent);
            change(facts.tool.as_mut().expect("tool"));
            facts
        };
        assert_eq!(denied(with(|tool| tool.level = None)), Denial::NotSet);
        assert_eq!(
            denied(with(|tool| tool.level = Some(ToolLevel::Limited))),
            Denial::Limited
        );
        assert_eq!(
            denied(with(|tool| {
                tool.community = Some(CommunityId::from_uuid(Uuid::from_u128(2)))
            })),
            Denial::OtherCommunity
        );
        assert_eq!(
            denied(with(|tool| tool.respond_class = Some(RespondClass::Anyone))),
            Denial::AnswersAnyone
        );

        let mut ownerless = full_agent(KeyRole::External);
        ownerless.admission = KeyAdmission::Admitted {
            role: KeyRole::External,
            principal: PrincipalId::from_slice(&[1u8; 32]).expect("principal"),
            device_id: Uuid::from_u128(7),
            owner_pubkey: None,
        };
        assert_eq!(denied(ownerless), Denial::OwnerUnknown);
    }
}
