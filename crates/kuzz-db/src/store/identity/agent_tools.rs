//! Owner tools on/off per agent key, agent key revocation and the agent
//! action log (AI tools plan §3, §4 and §8, stage S1).
//!
//! `key_registry.tool_level` is the owner's switch for one agent or external
//! key: `full`, `limited`, or NULL (never set, acts as limited). It counts
//! only in `tool_community_id`, and never together with `respond_class`
//! `anyone`. Only the owner writes it, from a proven device; the relay reads
//! it on every check (no cache). Nothing grants rights from it in S1.

use chrono::{DateTime, Utc};
use kuzz_core::principal::PrincipalId;
use kuzz_core::{CommunityId, StoredEvent};
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row as _;
use uuid::Uuid;

use super::admission::{key_admission, KeyAdmission, KeyRole};
use super::{begin, lock_subject};
use crate::error::Result;
use crate::observability::{self, TransactionOperation};
use crate::replaceable::{ParameterizedReplacePrecondition, ParameterizedReplaceStatus};
use crate::{Db, DbError};

/// The owner's switch for one agent key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolLevel {
    /// May act for its owner (once S2 and S7 use it).
    Full,
    /// Does everything an agent does today, nothing for its owner.
    Limited,
}

impl ToolLevel {
    /// Column and API value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Limited => "limited",
        }
    }

    /// Parse a column or API value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "limited" => Some(Self::Limited),
            _ => None,
        }
    }
}

/// Who an agent answers (`respond_to`), as far as its tools care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespondClass {
    /// Only its owner.
    OwnerOnly,
    /// Its owner and a list of people.
    Allowlist,
    /// Anyone; such an agent is never `full`.
    Anyone,
}

impl RespondClass {
    /// Column and API value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OwnerOnly => "owner-only",
            Self::Allowlist => "allowlist",
            Self::Anyone => "anyone",
        }
    }

    /// Parse a column or API value.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner-only" => Some(Self::OwnerOnly),
            "allowlist" => Some(Self::Allowlist),
            "anyone" => Some(Self::Anyone),
            _ => None,
        }
    }

    /// The class a kind:30177 managed-agent record states, read as its
    /// readers read it: `nobody` answers no one, so it is owner-only. `None`
    /// when the record names none (readers keep what they had).
    pub fn of_managed_agent(content: &str) -> Option<Self> {
        let content: serde_json::Value = serde_json::from_str(content).ok()?;
        match content.get("respond_to")?.as_str()? {
            "nobody" => Some(Self::OwnerOnly),
            other => Self::parse(other),
        }
    }
}

/// One owner write of the switch.
#[derive(Debug, Clone, Copy)]
pub struct ToolLevelWrite {
    /// The new level.
    pub level: ToolLevel,
    /// Who the agent answers now, as the writing app knows it.
    pub respond_class: RespondClass,
    /// A default write: only over a level never set.
    pub default_only: bool,
    /// The community of the owner's call; the level counts only there.
    pub community: CommunityId,
    /// The owner's device making the call.
    pub device: Uuid,
}

/// What [`Db::set_agent_tool_level`] did. Only `Set` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetToolLevel {
    /// Stored.
    Set(ToolLevel),
    /// A default write over a level already set; holds the stored level.
    AlreadySet(ToolLevel),
    /// Not a live agent or external key of this owner.
    NotFound,
    /// `full` for an agent that answers anyone.
    FullAnyone,
    /// `full` for a paired agent, or for an app agent from a device other
    /// than its host: the class sent cannot be trusted, and the stored one
    /// (from the owner's signed record) is unknown or differs.
    ClassStale,
}

/// One agent key's tool state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentToolState {
    /// The key.
    pub pubkey: [u8; 32],
    /// `Agent` or `External`.
    pub role: KeyRole,
    /// The owner's account.
    pub principal: PrincipalId,
    /// The stored level (`None`: never set).
    pub level: Option<ToolLevel>,
    /// Where the level counts.
    pub community: Option<CommunityId>,
    /// The stored class.
    pub respond_class: Option<RespondClass>,
    /// The device the key runs on.
    pub host_device_id: Option<Uuid>,
    /// That device's name.
    pub host_device_name: Option<String>,
    /// The key was revoked.
    pub revoked: bool,
}

/// A key's admission and, for agent and external keys, its tool state.
#[derive(Debug, Clone)]
pub struct KeyAuthority {
    /// [`Db::key_admission`], unchanged.
    pub admission: KeyAdmission,
    /// `None` for person and unregistered keys.
    pub tool: Option<AgentToolState>,
}

/// What [`Db::revoke_agent_key`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeAgentKey {
    /// Revoked now.
    Revoked,
    /// Already revoked.
    AlreadyRevoked,
    /// Not an agent or external key of this owner.
    NotFound,
    /// The calling device is not the key's host.
    NotHost,
}

/// How an agent action ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStatus {
    /// Done.
    Ok,
    /// Refused by a rule.
    Refused,
    /// Tried and failed.
    Failed,
}

impl ActionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Failed => "failed",
        }
    }
}

/// Who recorded an agent action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionSource {
    /// The relay, for what passed through it.
    Relay,
    /// The owner's app, for what ran on the computer.
    App,
}

impl ActionSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Relay => "relay",
            Self::App => "app",
        }
    }
}

/// One row of `agent_actions`. No payload.
#[derive(Debug, Clone)]
pub struct NewAgentAction<'a> {
    /// The community the request went through.
    pub community: CommunityId,
    /// The owner's account key.
    pub owner_pubkey: [u8; 32],
    /// The agent's key.
    pub agent_pubkey: [u8; 32],
    /// What, e.g. `tool_level.set`.
    pub action: &'a str,
    /// On what, if anything.
    pub target: Option<&'a str>,
    /// Which computer.
    pub computer: Option<&'a str>,
    /// How it ended.
    pub status: ActionStatus,
    /// Who recorded it.
    pub source: ActionSource,
}

fn key32(bytes: Vec<u8>, what: &str) -> Result<[u8; 32]> {
    bytes
        .try_into()
        .map_err(|_| DbError::InvalidData(format!("invalid {what}")))
}

fn stored<T>(value: Option<String>, parse: fn(&str) -> Option<T>, what: &str) -> Result<Option<T>> {
    value
        .map(|value| parse(&value).ok_or_else(|| DbError::InvalidData(format!("{what} {value}"))))
        .transpose()
}

/// The start of a [`tool_state`] query over `key_registry r`.
macro_rules! select_tool_state {
    () => {
        "SELECT r.pubkey, r.role, r.principal_id, r.tool_level, r.tool_community_id, \
                r.respond_class, r.host_device_id, d.name AS host_device_name, \
                r.revoked_at IS NOT NULL AS revoked \
           FROM key_registry r LEFT JOIN devices d ON d.id = r.host_device_id "
    };
}

fn tool_state(row: &sqlx::postgres::PgRow) -> Result<AgentToolState> {
    let principal: Vec<u8> = row.try_get("principal_id")?;
    Ok(AgentToolState {
        pubkey: key32(row.try_get("pubkey")?, "agent key")?,
        role: match row.try_get::<&str, _>("role")? {
            "agent" => KeyRole::Agent,
            "external" => KeyRole::External,
            _ => KeyRole::Person,
        },
        principal: super::principal_from_bytes(&principal)?,
        level: stored(row.try_get("tool_level")?, ToolLevel::parse, "tool level")?,
        community: row
            .try_get::<Option<Uuid>, _>("tool_community_id")?
            .map(CommunityId::from_uuid),
        respond_class: stored(
            row.try_get("respond_class")?,
            RespondClass::parse,
            "respond class",
        )?,
        host_device_id: row.try_get("host_device_id")?,
        host_device_name: row.try_get("host_device_name")?,
        revoked: row.try_get("revoked")?,
    })
}

impl Db {
    /// Set `owner`'s switch for `agent` in one UPDATE (see [`SetToolLevel`]).
    /// Only an app agent's host and the owner's signed record know its class:
    /// a write from any other device, and every write for a paired (external)
    /// key, keeps the stored class, and turning `full` on there needs the
    /// stored class to equal the one sent.
    #[datastore_span(name = "set_agent_tool_level", system = "postgresql")]
    pub async fn set_agent_tool_level(
        &self,
        owner: &PrincipalId,
        agent: &[u8; 32],
        write: &ToolLevelWrite,
    ) -> Result<SetToolLevel> {
        if write.level == ToolLevel::Full && write.respond_class == RespondClass::Anyone {
            return Ok(SetToolLevel::FullAnyone);
        }
        let full = write.level == ToolLevel::Full;
        let class = write.respond_class.as_str();
        let set: Option<String> = sqlx::query_scalar(
            "UPDATE key_registry SET tool_level = $3, tool_community_id = $4, \
                    respond_class = CASE WHEN role = 'external' \
                                            OR host_device_id IS DISTINCT FROM $5 \
                                     THEN respond_class ELSE $6 END, \
                    tool_set_at = now() \
              WHERE pubkey = $1 AND principal_id = $2 AND revoked_at IS NULL \
                AND role IN ('agent', 'external') \
                AND (NOT $7 OR tool_level IS NULL) \
                AND NOT ($8 AND (role = 'external' OR host_device_id IS DISTINCT FROM $5) \
                         AND respond_class IS DISTINCT FROM $6) \
             RETURNING tool_level",
        )
        .bind(agent.as_slice())
        .bind(owner.as_bytes().as_slice())
        .bind(write.level.as_str())
        .bind(write.community.as_uuid())
        .bind(write.device)
        .bind(class)
        .bind(write.default_only)
        .bind(full)
        .fetch_optional(&self.pool)
        .await?;
        if set.is_some() {
            return Ok(SetToolLevel::Set(write.level));
        }
        // Nothing written: say why. A race with another write only changes
        // which refusal is named.
        let row = sqlx::query(
            "SELECT role, tool_level, respond_class, host_device_id FROM key_registry \
              WHERE pubkey = $1 AND principal_id = $2 AND revoked_at IS NULL \
                AND role IN ('agent', 'external')",
        )
        .bind(agent.as_slice())
        .bind(owner.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(SetToolLevel::NotFound);
        };
        let level = stored(row.try_get("tool_level")?, ToolLevel::parse, "tool level")?;
        if let (true, Some(level)) = (write.default_only, level) {
            return Ok(SetToolLevel::AlreadySet(level));
        }
        Ok(SetToolLevel::ClassStale)
    }

    /// The tool state of those of `pubkeys` that are `owner`'s live agent
    /// and external keys; any other key is left out.
    #[datastore_span(name = "agent_tool_levels", system = "postgresql")]
    pub async fn agent_tool_levels(
        &self,
        owner: &PrincipalId,
        pubkeys: &[[u8; 32]],
    ) -> Result<Vec<AgentToolState>> {
        let keys: Vec<Vec<u8>> = pubkeys.iter().map(|key| key.to_vec()).collect();
        let rows = sqlx::query(concat!(
            select_tool_state!(),
            "WHERE r.principal_id = $1 AND r.pubkey = ANY($2) AND r.revoked_at IS NULL"
        ))
        .bind(owner.as_bytes().as_slice())
        .bind(&keys)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(tool_state).collect()
    }

    /// [`Db::key_admission`] unchanged, then one read of the key's tool
    /// state. Never cached: a change counts on the next call.
    #[datastore_span(name = "key_authority", system = "postgresql")]
    pub async fn key_authority(
        &self,
        pubkey: &[u8; 32],
        device_pubkey: Option<&[u8; 33]>,
    ) -> Result<KeyAuthority> {
        let admission = key_admission(&self.pool, pubkey, device_pubkey).await?;
        let row = sqlx::query(concat!(select_tool_state!(), "WHERE r.pubkey = $1"))
            .bind(pubkey.as_slice())
            .fetch_optional(&self.pool)
            .await?;
        Ok(KeyAuthority {
            admission,
            tool: row.as_ref().map(tool_state).transpose()?,
        })
    }

    /// Revoke `owner`'s agent key `agent` for good, from its host `device`
    /// only: the key is refused everywhere, its level cleared and its key
    /// copy deleted, in one transaction. The key is never registered again.
    #[datastore_span(name = "revoke_agent_key", system = "postgresql")]
    pub async fn revoke_agent_key(
        &self,
        owner: &PrincipalId,
        agent: &[u8; 32],
        device: Uuid,
    ) -> Result<RevokeAgentKey> {
        let mut tx = begin(&self.pool).await?;
        // Serializes with registration, pairing and device revokes.
        lock_subject(&mut tx, owner).await?;
        let row = sqlx::query(
            "SELECT host_device_id, revoked_at IS NOT NULL AS revoked FROM key_registry \
              WHERE pubkey = $1 AND principal_id = $2 AND role IN ('agent', 'external') \
              FOR UPDATE",
        )
        .bind(agent.as_slice())
        .bind(owner.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(RevokeAgentKey::NotFound);
        };
        if row.try_get::<Option<Uuid>, _>("host_device_id")? != Some(device) {
            return Ok(RevokeAgentKey::NotHost);
        }
        if row.try_get::<bool, _>("revoked")? {
            return Ok(RevokeAgentKey::AlreadyRevoked);
        }
        sqlx::query(
            "UPDATE key_registry SET revoked_at = now(), tool_level = NULL, \
                    tool_community_id = NULL, respond_class = NULL, tool_set_at = NULL \
              WHERE pubkey = $1",
        )
        .bind(agent.as_slice())
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM agent_key_backups WHERE agent_pubkey = $1")
            .bind(agent.as_slice())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(RevokeAgentKey::Revoked)
    }

    /// Store a kind:30177 managed-agent record and, when it became the live
    /// record, bring the agent's stored class in line with it in the same
    /// transaction. Only the owner's own record counts; `anyone` turns a
    /// `full` level off. An older record changes nothing.
    #[datastore_span(name = "replace_managed_agent_event", system = "postgresql")]
    pub async fn replace_managed_agent_event(
        &self,
        community: CommunityId,
        event: &nostr::Event,
        d_tag: &str,
        channel_id: Option<Uuid>,
    ) -> Result<(StoredEvent, bool)> {
        let (mut tx, timer) = observability::begin_transaction(
            &self.pool,
            TransactionOperation::ReplaceParameterizedEvent,
        )
        .await?;
        timer
            .observe(async {
                let result = self
                    .replace_parameterized_event_in_transaction(
                        &mut tx,
                        community,
                        event,
                        d_tag,
                        channel_id,
                        ParameterizedReplacePrecondition::Unconditional,
                    )
                    .await?;
                let inserted = result.status == ParameterizedReplaceStatus::Inserted;
                if !inserted {
                    tx.rollback().await?;
                    return Ok((result.event, false));
                }
                let class = RespondClass::of_managed_agent(&event.content);
                let agent = hex::decode(d_tag).ok().filter(|key| key.len() == 32);
                if let (Some(class), Some(agent)) = (class, agent) {
                    sync_respond_class(&mut tx, community, &agent, &event.pubkey.to_bytes(), class)
                        .await?;
                }
                tx.commit().await?;
                Ok((result.event, true))
            })
            .await
    }

    /// Log one agent action.
    #[datastore_span(name = "insert_agent_action", system = "postgresql")]
    pub async fn insert_agent_action(&self, action: &NewAgentAction<'_>) -> Result<()> {
        sqlx::query(
            "INSERT INTO agent_actions (community_id, owner_pubkey, agent_pubkey, action, \
                    target, computer, status, source) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(action.community.as_uuid())
        .bind(action.owner_pubkey.as_slice())
        .bind(action.agent_pubkey.as_slice())
        .bind(action.action)
        .bind(action.target)
        .bind(action.computer)
        .bind(action.status.as_str())
        .bind(action.source.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Delete agent actions older than `cutoff`, `batch` rows a statement,
    /// in live communities (a community being deleted is purged whole by its
    /// deletion). Returns how many went.
    #[datastore_span(name = "purge_agent_actions_before", system = "postgresql")]
    pub async fn purge_agent_actions_before(
        &self,
        cutoff: DateTime<Utc>,
        batch: i64,
    ) -> Result<u64> {
        let mut total = 0;
        loop {
            let deleted = sqlx::query(
                "DELETE FROM agent_actions WHERE (community_id, id) IN ( \
                   SELECT a.community_id, a.id FROM agent_actions a \
                     JOIN communities c ON c.id = a.community_id \
                    WHERE a.created_at < $1 AND c.deletion_state = 'active' \
                    LIMIT $2)",
            )
            .bind(cutoff)
            .bind(batch)
            .execute(&self.pool)
            .await?
            .rows_affected();
            total += deleted;
            if deleted < batch as u64 {
                return Ok(total);
            }
        }
    }
}

/// Keep `agent`'s stored class in line with its owner's live record: the
/// record's author must be the account key of the key's owner. `anyone`
/// counts everywhere and turns `full` off; another class only in the
/// community the level counts in (or when none is set).
async fn sync_respond_class(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    community: CommunityId,
    agent: &[u8],
    author: &[u8; 32],
    class: RespondClass,
) -> Result<()> {
    sqlx::query(
        "UPDATE key_registry r SET respond_class = $3, \
                tool_level = CASE WHEN $3 = 'anyone' AND r.tool_level = 'full' \
                                  THEN 'limited' ELSE r.tool_level END, \
                tool_set_at = CASE WHEN $3 = 'anyone' AND r.tool_level = 'full' \
                                   THEN now() ELSE r.tool_set_at END \
          WHERE r.pubkey = $1 AND r.revoked_at IS NULL AND r.role IN ('agent', 'external') \
            AND r.principal_id = (SELECT b.account_id FROM account_key_backups b \
                                   WHERE b.pubkey = $2) \
            AND r.respond_class IS DISTINCT FROM $3 \
            AND ($3 = 'anyone' OR r.tool_community_id IS NULL OR r.tool_community_id = $4)",
    )
    .bind(agent)
    .bind(author.as_slice())
    .bind(class.as_str())
    .bind(community.as_uuid())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_managed_agent_record_states_its_class_as_readers_read_it() {
        let class = |content: &str| RespondClass::of_managed_agent(content);
        assert_eq!(
            class(r#"{"respond_to":"owner-only"}"#),
            Some(RespondClass::OwnerOnly)
        );
        assert_eq!(
            class(r#"{"respond_to":"allowlist","respond_to_allowlist":["a"]}"#),
            Some(RespondClass::Allowlist)
        );
        assert_eq!(
            class(r#"{"respond_to":"anyone"}"#),
            Some(RespondClass::Anyone)
        );
        assert_eq!(
            class(r#"{"respond_to":"nobody"}"#),
            Some(RespondClass::OwnerOnly)
        );
        // No class named, an unknown one, or no JSON: nothing changes.
        for content in [
            r#"{"display_name":"x"}"#,
            r#"{"respond_to":null}"#,
            r#"{"respond_to":"everyone"}"#,
            "not json",
        ] {
            assert_eq!(class(content), None, "{content}");
        }
    }

    #[test]
    fn levels_and_classes_round_trip() {
        for level in [ToolLevel::Full, ToolLevel::Limited] {
            assert_eq!(ToolLevel::parse(level.as_str()), Some(level));
        }
        for class in [
            RespondClass::OwnerOnly,
            RespondClass::Allowlist,
            RespondClass::Anyone,
        ] {
            assert_eq!(RespondClass::parse(class.as_str()), Some(class));
        }
        assert_eq!(ToolLevel::parse("on"), None);
        assert_eq!(RespondClass::parse("nobody"), None);
    }
}
