//! Kuzz agent registration and key copies (device-bound keys plan §4, PR A3).
//!
//! An owner registers each agent key from the device that runs it: one
//! transaction binds `key_registry(agent, owner, host = that device)` and
//! stores the encrypted copy in `agent_key_backups`. A key is never re-owned
//! and never takes a second role; moving it to another of the owner's
//! devices is explicit. The copies are restored only to their host device.

use kuzz_core::principal::PrincipalId;
use sqlx::Row as _;
use uuid::Uuid;

use super::admission::KeyRole;
use super::{begin, lock_subject};
use crate::error::Result;
use crate::{Db, DbError};

/// Most live agents one owner hosts on one device; also the size of one
/// listing, so a listing is never cut short.
pub const MAX_AGENT_KEYS_PER_DEVICE: i64 = 500;

/// Ciphertext-only database representation of an agent key copy.
#[derive(Clone)]
pub struct AgentKeyBackupRecord {
    /// The agent's Nostr public key.
    pub agent_pubkey: [u8; 32],
    /// AEAD/AAD envelope version.
    pub version: u16,
    /// Nonsecret master-key label.
    pub key_id: String,
    /// Random 96-bit nonce.
    pub nonce: Vec<u8>,
    /// 32 encrypted secret bytes plus 16-byte authentication tag.
    pub ciphertext: Vec<u8>,
}

/// What [`Db::register_agent_key`] did. Only the first three commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterAgentKey {
    /// New binding (or a backfilled one bound to its first host).
    Registered,
    /// Already registered by this owner on this device; the copy is renewed.
    Unchanged,
    /// Moved from another of the owner's devices to this one.
    Moved,
    /// The agent runs on another of the owner's devices; `move` is required.
    OtherHost,
    /// Another account owns this agent key, or an agent profile in some
    /// community names another owner (`users.agent_owner_pubkey`).
    OtherOwner,
    /// The key is an account key or an external agent key.
    OtherRole,
    /// The agent was removed; its key is not registered again.
    Revoked,
    /// The device is not a live device of the owner.
    DeviceGone,
    /// The device already hosts [`MAX_AGENT_KEYS_PER_DEVICE`] of the owner's
    /// live agents.
    DeviceFull,
}

/// One key as seen by its owner (`GET /agents/{pubkey}/status`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedKey {
    /// The key.
    pub pubkey: [u8; 32],
    /// What it is.
    pub role: KeyRole,
    /// Device key of the agent's host device (`None`: persons, unbound or
    /// keyless hosts).
    pub host_device_pubkey: Option<[u8; 33]>,
}

impl Db {
    /// Register `backup.agent_pubkey` as `owner`'s agent hosted on `device`
    /// and store its copy, in one transaction (see [`RegisterAgentKey`]).
    pub async fn register_agent_key(
        &self,
        owner: &PrincipalId,
        device: Uuid,
        backup: &AgentKeyBackupRecord,
        allow_move: bool,
    ) -> Result<RegisterAgentKey> {
        if backup.nonce.len() != 12
            || backup.ciphertext.len() != 48
            || backup.key_id.is_empty()
            || backup.key_id.len() > 64
            || !backup
                .key_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(DbError::InvalidData("invalid agent key envelope".into()));
        }
        let owner_bytes = owner.as_bytes().as_slice();
        let agent = backup.agent_pubkey.as_slice();
        let mut tx = begin(&self.pool).await?;
        // Serializes with revocations of the owner's devices and sessions.
        lock_subject(&mut tx, owner).await?;
        let live_device: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM devices WHERE id = $1 AND principal_id = $2 \
             AND kind = 'login' AND revoked_at IS NULL)",
        )
        .bind(device)
        .bind(owner_bytes)
        .fetch_one(&mut *tx)
        .await?;
        if !live_device {
            return Ok(RegisterAgentKey::DeviceGone);
        }
        let account_key: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM account_key_backups WHERE pubkey = $1)",
        )
        .bind(agent)
        .fetch_one(&mut *tx)
        .await?;
        if account_key {
            return Ok(RegisterAgentKey::OtherRole);
        }
        // The S1 backfill ran once; owners attested later (or skipped as
        // ambiguous) still bind: no community may name another owner.
        let named_elsewhere: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users u WHERE u.pubkey = $1 \
               AND u.agent_owner_pubkey IS NOT NULL \
               AND u.agent_owner_pubkey IS DISTINCT FROM \
                   (SELECT b.pubkey FROM account_key_backups b WHERE b.account_id = $2))",
        )
        .bind(agent)
        .bind(owner_bytes)
        .fetch_one(&mut *tx)
        .await?;
        if named_elsewhere {
            return Ok(RegisterAgentKey::OtherOwner);
        }
        // Under the owner lock, so concurrent registrations cannot overshoot.
        let hosted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM key_registry WHERE principal_id = $1 \
               AND host_device_id = $2 AND role = 'agent' AND revoked_at IS NULL \
               AND pubkey <> $3",
        )
        .bind(owner_bytes)
        .bind(device)
        .bind(agent)
        .fetch_one(&mut *tx)
        .await?;
        let device_full = hosted >= MAX_AGENT_KEYS_PER_DEVICE;
        let taken: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM key_registry WHERE pubkey = $1)")
                .bind(agent)
                .fetch_one(&mut *tx)
                .await?;
        if !taken && device_full {
            return Ok(RegisterAgentKey::DeviceFull);
        }
        let inserted = sqlx::query(
            "INSERT INTO key_registry (pubkey, principal_id, role, host_device_id) \
             VALUES ($1, $2, 'agent', $3) ON CONFLICT (pubkey) DO NOTHING",
        )
        .bind(agent)
        .bind(owner_bytes)
        .bind(device)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        let outcome = if inserted {
            RegisterAgentKey::Registered
        } else {
            let row = sqlx::query(
                "SELECT principal_id, role, host_device_id, revoked_at IS NOT NULL AS revoked \
                 FROM key_registry WHERE pubkey = $1 FOR UPDATE",
            )
            .bind(agent)
            .fetch_one(&mut *tx)
            .await?;
            let principal: Vec<u8> = row.try_get("principal_id")?;
            let role: String = row.try_get("role")?;
            let host: Option<Uuid> = row.try_get("host_device_id")?;
            let outcome = if role != "agent" {
                RegisterAgentKey::OtherRole
            } else if principal != owner_bytes {
                RegisterAgentKey::OtherOwner
            } else if row.try_get::<bool, _>("revoked")? {
                RegisterAgentKey::Revoked
            } else if host == Some(device) {
                RegisterAgentKey::Unchanged
            } else if device_full {
                RegisterAgentKey::DeviceFull
            } else if host.is_none() {
                RegisterAgentKey::Registered
            } else if allow_move {
                RegisterAgentKey::Moved
            } else {
                RegisterAgentKey::OtherHost
            };
            if !matches!(
                outcome,
                RegisterAgentKey::Registered
                    | RegisterAgentKey::Unchanged
                    | RegisterAgentKey::Moved
            ) {
                return Ok(outcome);
            }
            sqlx::query("UPDATE key_registry SET host_device_id = $2 WHERE pubkey = $1")
                .bind(agent)
                .bind(device)
                .execute(&mut *tx)
                .await?;
            outcome
        };
        sqlx::query(
            "INSERT INTO agent_key_backups \
               (agent_pubkey, owner_id, host_device_id, version, key_id, nonce, ciphertext) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (agent_pubkey) DO UPDATE SET owner_id = EXCLUDED.owner_id, \
               host_device_id = EXCLUDED.host_device_id, version = EXCLUDED.version, \
               key_id = EXCLUDED.key_id, nonce = EXCLUDED.nonce, \
               ciphertext = EXCLUDED.ciphertext, updated_at = now()",
        )
        .bind(agent)
        .bind(owner_bytes)
        .bind(device)
        .bind(backup.version as i16)
        .bind(&backup.key_id)
        .bind(&backup.nonce)
        .bind(&backup.ciphertext)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// The copies of `owner`'s live agents hosted on `device`, oldest first
    /// (at most [`MAX_AGENT_KEYS_PER_DEVICE`]).
    pub async fn device_agent_keys(
        &self,
        owner: &PrincipalId,
        device: Uuid,
    ) -> Result<Vec<AgentKeyBackupRecord>> {
        let rows = sqlx::query(
            "SELECT b.agent_pubkey, b.version, b.key_id, b.nonce, b.ciphertext \
               FROM agent_key_backups b \
               JOIN key_registry r ON r.pubkey = b.agent_pubkey \
              WHERE b.owner_id = $1 AND b.host_device_id = $2 \
                AND r.principal_id = $1 AND r.host_device_id = $2 \
                AND r.role = 'agent' AND r.revoked_at IS NULL \
              ORDER BY b.created_at, b.agent_pubkey \
              LIMIT $3",
        )
        .bind(owner.as_bytes().as_slice())
        .bind(device)
        .bind(MAX_AGENT_KEYS_PER_DEVICE)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let pubkey: Vec<u8> = row.try_get("agent_pubkey")?;
                let version: i16 = row.try_get("version")?;
                Ok(AgentKeyBackupRecord {
                    agent_pubkey: pubkey
                        .try_into()
                        .map_err(|_| DbError::InvalidData("invalid agent pubkey".into()))?,
                    version: u16::try_from(version)
                        .map_err(|_| DbError::InvalidData("invalid agent copy version".into()))?,
                    key_id: row.try_get("key_id")?,
                    nonce: row.try_get("nonce")?,
                    ciphertext: row.try_get("ciphertext")?,
                })
            })
            .collect()
    }

    /// Which of `pubkeys` are `owner`'s own key or its live agent and
    /// external keys. Keys of other accounts and unknown keys are left out.
    pub async fn owned_keys(
        &self,
        owner: &PrincipalId,
        pubkeys: &[[u8; 32]],
    ) -> Result<Vec<OwnedKey>> {
        let keys: Vec<Vec<u8>> = pubkeys.iter().map(|key| key.to_vec()).collect();
        let rows = sqlx::query(
            "SELECT b.pubkey, 'person'::text AS role, NULL::bytea AS host_key \
               FROM account_key_backups b \
              WHERE b.account_id = $1 AND b.pubkey = ANY($2) \
             UNION ALL \
             SELECT r.pubkey, r.role, d.device_pubkey \
               FROM key_registry r LEFT JOIN devices d ON d.id = r.host_device_id \
              WHERE r.principal_id = $1 AND r.pubkey = ANY($2) AND r.revoked_at IS NULL",
        )
        .bind(owner.as_bytes().as_slice())
        .bind(&keys)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let pubkey: Vec<u8> = row.try_get("pubkey")?;
                let host: Option<Vec<u8>> = row.try_get("host_key")?;
                Ok(OwnedKey {
                    pubkey: pubkey
                        .try_into()
                        .map_err(|_| DbError::InvalidData("invalid key".into()))?,
                    role: match row.try_get::<String, _>("role")?.as_str() {
                        "person" => KeyRole::Person,
                        "agent" => KeyRole::Agent,
                        _ => KeyRole::External,
                    },
                    host_device_pubkey: host.and_then(|key| key.try_into().ok()),
                })
            })
            .collect()
    }

    /// The account's own Nostr key (`account_key_backups`), if it has one.
    pub async fn account_pubkey(&self, account: &PrincipalId) -> Result<Option<[u8; 32]>> {
        let key: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT pubkey FROM account_key_backups WHERE account_id = $1")
                .bind(account.as_bytes().as_slice())
                .fetch_optional(&self.pool)
                .await?;
        key.map(|key| {
            key.try_into()
                .map_err(|_| DbError::InvalidData("invalid account pubkey".into()))
        })
        .transpose()
    }
}
