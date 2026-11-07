//! The profile an owner sets for an external agent.
//!
//! An external agent has no principal: `key_registry` ties its own key to the
//! owner's account, and its kind:0 is signed on its computer. An owner edit is
//! kept here, and the relay stamps the agent's kind:0 from it, so it works from
//! any of the owner's devices while the agent's computer is off.

use kuzz_core::principal::PrincipalId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row as _;

use crate::error::Result;
use crate::Db;

/// An external agent's owner-set profile, as absolute state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAgentProfile {
    /// Display name.
    pub display_name: String,
    /// Picture URL; `None` clears it.
    pub avatar_url: Option<String>,
    /// Description; `None` clears it.
    pub about: Option<String>,
}

impl Db {
    /// Save `owner`'s profile for their external agent. A disconnected agent
    /// still counts. Returns false when `agent` is not such a key.
    #[datastore_span(name = "set_external_agent_profile", system = "postgresql")]
    pub async fn set_external_agent_profile(
        &self,
        owner: &PrincipalId,
        agent: &[u8],
        profile: &ExternalAgentProfile,
    ) -> Result<bool> {
        let updated = sqlx::query(
            "UPDATE key_registry SET profile_name = $3, profile_avatar_url = $4, \
                    profile_about = $5, profile_set_at = now() \
             WHERE pubkey = $1 AND principal_id = $2 AND role = 'external' \
               AND revoked_at IS NULL",
        )
        .bind(agent)
        .bind(owner.as_bytes().as_slice())
        .bind(&profile.display_name)
        .bind(&profile.avatar_url)
        .bind(&profile.about)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() > 0)
    }

    /// The profile `agent`'s owner set, if any: while there is one, the owner
    /// is the only writer of the agent's kind:0.
    #[datastore_span(name = "external_agent_profile", system = "postgresql")]
    pub async fn external_agent_profile(
        &self,
        agent: &[u8],
    ) -> Result<Option<ExternalAgentProfile>> {
        let row = sqlx::query(
            "SELECT profile_name, profile_avatar_url, profile_about FROM key_registry \
             WHERE pubkey = $1 AND role = 'external' AND profile_set_at IS NOT NULL \
               AND revoked_at IS NULL",
        )
        .bind(agent)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(ExternalAgentProfile {
                display_name: row.try_get("profile_name")?,
                avatar_url: row.try_get("profile_avatar_url")?,
                about: row.try_get("profile_about")?,
            })
        })
        .transpose()
    }
}
