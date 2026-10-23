//! External agent pairing (device-bound keys plan §14.7, PR A4).
//!
//! An owner starts a pairing from a signed-in device; an agent on another
//! computer claims its code with the agent key and that computer's device
//! key. The first valid claim connects at once, in one transaction: the
//! computer becomes an `external` device of the owner and the agent key is
//! bound to it (`key_registry` role `external`). The code is single use; the
//! owner disconnects by revoking the device. A key that is an account key or
//! already registered is never paired (plan S6), except an agent the 0080
//! backfill bound to the same owner without a host or copy: pairing gives it
//! its only host.

use chrono::{DateTime, Utc};
use kuzz_core::principal::PrincipalId;
use sqlx::{Postgres, Row as _, Transaction};
use uuid::Uuid;

use super::{begin, lock_subject, MAX_DEVICES_PER_PRINCIPAL};
use crate::error::Result;
use crate::{Db, DbError};

/// How long a code may be claimed.
pub const PAIRING_TTL_SECS: i64 = 600;
/// Most unused, unexpired pairings one owner may have at once.
pub const MAX_OPEN_PAIRINGS: i64 = 10;

/// Stored state of a pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingState {
    /// The code is shown and not used yet.
    Open,
    /// An agent used the code: it is paired.
    Connected,
}

impl PairingState {
    fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "open" => Self::Open,
            "connected" => Self::Connected,
            other => return Err(DbError::InvalidData(format!("pairing state {other}"))),
        })
    }
}

/// A pairing as its owner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingRecord {
    /// Pairing id.
    pub id: Uuid,
    /// Stored state.
    pub state: PairingState,
    /// The connected agent's key.
    pub agent_pubkey: Option<[u8; 32]>,
    /// The connected computer's device key (SEC1 compressed).
    pub device_pubkey: Option<[u8; 33]>,
    /// The name the connected computer sent.
    pub device_name: Option<String>,
    /// The external device it connected (none once that row is deleted).
    pub device_id: Option<Uuid>,
    /// When the owner started it.
    pub created_at: DateTime<Utc>,
    /// Claim deadline.
    pub expires_at: DateTime<Utc>,
    /// When it was used.
    pub claimed_at: Option<DateTime<Utc>>,
    /// Before `expires_at` by the database clock when read.
    pub live: bool,
}

impl PairingRecord {
    /// Unused and past its deadline.
    pub fn expired(&self) -> bool {
        self.state == PairingState::Open && !self.live
    }
}

/// What [`Db::start_pairing`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartPairing {
    /// A new open pairing.
    Started {
        /// Pairing id.
        id: Uuid,
        /// Claim deadline.
        expires_at: DateTime<Utc>,
    },
    /// The owner already has [`MAX_OPEN_PAIRINGS`] open pairings.
    TooMany,
}

/// A claim of a pairing code.
#[derive(Debug, Clone, Copy)]
pub struct PairingClaim<'a> {
    /// SHA-256 of the code.
    pub code_hash: &'a [u8; 32],
    /// The agent's key.
    pub agent_pubkey: &'a [u8; 32],
    /// The computer's device key (SEC1 compressed).
    pub device_pubkey: &'a [u8; 33],
    /// `hardware` or `software`.
    pub device_key_kind: &'a str,
    /// A name for the computer, shown to the owner.
    pub device_name: Option<&'a str>,
}

/// What [`Db::claim_pairing`] did. Only `Connected` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimPairing {
    /// Paired: the agent key is bound to this external device of the owner.
    Connected {
        /// Pairing id.
        pairing_id: Uuid,
        /// The external device.
        device_id: Uuid,
    },
    /// The code was used already.
    Used,
    /// No pairing has this code, or it expired unused.
    Invalid,
    /// The agent key is an account key or already registered (plan S6).
    KeyInUse,
    /// The owner already has [`MAX_DEVICES_PER_PRINCIPAL`] live external
    /// devices.
    DeviceLimit,
}

/// May `agent` be paired to `owner`?
enum KeyUse {
    /// Unknown key.
    Free,
    /// An agent bound to `owner` by the 0080 backfill, with no host or copy.
    Unbound,
    /// An account key, a registered key, or a key some profile attests to
    /// another owner.
    InUse,
}

async fn key_use(
    tx: &mut Transaction<'static, Postgres>,
    agent: &[u8; 32],
    owner: &[u8],
) -> Result<KeyUse> {
    let row = sqlx::query(
        "SELECT EXISTS (SELECT 1 FROM account_key_backups WHERE pubkey = $1) AS person, \
                EXISTS (SELECT 1 FROM users u WHERE u.pubkey = $1 \
                          AND u.agent_owner_pubkey IS NOT NULL \
                          AND u.agent_owner_pubkey IS DISTINCT FROM \
                              (SELECT b.pubkey FROM account_key_backups b \
                                WHERE b.account_id = $2)) AS named_elsewhere, \
                (SELECT r.role = 'agent' AND r.principal_id = $2 \
                        AND r.host_device_id IS NULL AND r.revoked_at IS NULL \
                        AND NOT EXISTS (SELECT 1 FROM agent_key_backups c \
                                         WHERE c.agent_pubkey = r.pubkey) \
                   FROM key_registry r WHERE r.pubkey = $1) AS registered_unbound",
    )
    .bind(agent.as_slice())
    .bind(owner)
    .fetch_one(&mut **tx)
    .await?;
    let person: bool = row.try_get("person")?;
    let named_elsewhere: bool = row.try_get("named_elsewhere")?;
    let registered: Option<bool> = row.try_get("registered_unbound")?;
    Ok(match (person || named_elsewhere, registered) {
        (true, _) | (false, Some(false)) => KeyUse::InUse,
        (false, Some(true)) => KeyUse::Unbound,
        (false, None) => KeyUse::Free,
    })
}

fn record(row: &sqlx::postgres::PgRow) -> Result<PairingRecord> {
    let agent: Option<Vec<u8>> = row.try_get("agent_pubkey")?;
    let device: Option<Vec<u8>> = row.try_get("device_pubkey")?;
    Ok(PairingRecord {
        id: row.try_get("id")?,
        state: PairingState::parse(row.try_get::<&str, _>("state")?)?,
        agent_pubkey: agent
            .map(|key| key.try_into())
            .transpose()
            .map_err(|_| DbError::InvalidData("invalid pairing agent key".into()))?,
        device_pubkey: device
            .map(|key| key.try_into())
            .transpose()
            .map_err(|_| DbError::InvalidData("invalid pairing device key".into()))?,
        device_name: row.try_get("device_name")?,
        device_id: row.try_get("device_id")?,
        created_at: row.try_get("created_at")?,
        expires_at: row.try_get("expires_at")?,
        claimed_at: row.try_get("claimed_at")?,
        live: row.try_get("live")?,
    })
}

impl Db {
    /// Start a pairing for `owner` from its device `device`, with the code
    /// whose SHA-256 is `code_hash`. Also drops the owner's pairings that
    /// expired more than a day ago.
    pub async fn start_pairing(
        &self,
        owner: &PrincipalId,
        device: Uuid,
        code_hash: &[u8; 32],
    ) -> Result<StartPairing> {
        let owner_bytes = owner.as_bytes().as_slice();
        let mut tx = begin(&self.pool).await?;
        lock_subject(&mut tx, owner).await?;
        // Pairings are kept a day for the owner's list, then dropped here.
        sqlx::query(
            "DELETE FROM pairing_requests WHERE principal_id = $1 \
               AND expires_at < now() - interval '1 day'",
        )
        .bind(owner_bytes)
        .execute(&mut *tx)
        .await?;
        let open: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pairing_requests WHERE principal_id = $1 \
               AND state = 'open' AND expires_at > now()",
        )
        .bind(owner_bytes)
        .fetch_one(&mut *tx)
        .await?;
        if open >= MAX_OPEN_PAIRINGS {
            return Ok(StartPairing::TooMany);
        }
        let (id, expires_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
            "INSERT INTO pairing_requests (principal_id, started_device_id, code_hash, expires_at) \
             VALUES ($1, $2, $3, now() + make_interval(secs => $4)) RETURNING id, expires_at",
        )
        .bind(owner_bytes)
        .bind(device)
        .bind(code_hash.as_slice())
        .bind(PAIRING_TTL_SECS as f64)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(StartPairing::Started { id, expires_at })
    }

    /// Use the pairing code that hashes to `claim.code_hash`: atomically add
    /// the computer as an external device of the code's owner, bind the agent
    /// key to it and mark the pairing connected (see [`ClaimPairing`]). The
    /// caller has verified both signatures.
    pub async fn claim_pairing(&self, claim: &PairingClaim<'_>) -> Result<ClaimPairing> {
        let owner: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT principal_id FROM pairing_requests WHERE code_hash = $1")
                .bind(claim.code_hash.as_slice())
                .fetch_optional(&self.pool)
                .await?;
        let Some(owner_bytes) = owner else {
            return Ok(ClaimPairing::Invalid);
        };
        let owner = PrincipalId::from_slice(&owner_bytes)
            .map_err(|_| DbError::InvalidData("invalid pairing owner".into()))?;
        let mut tx = begin(&self.pool).await?;
        // The owner lock first, as for every device change of the owner
        // (serializes with revocations), then the pairing row.
        lock_subject(&mut tx, &owner).await?;
        let row = sqlx::query(
            "SELECT *, expires_at > now() AS live FROM pairing_requests \
              WHERE code_hash = $1 AND principal_id = $2 FOR UPDATE",
        )
        .bind(claim.code_hash.as_slice())
        .bind(owner_bytes.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(ClaimPairing::Invalid);
        };
        let pairing = record(&row)?;
        match pairing.state {
            PairingState::Connected => return Ok(ClaimPairing::Used),
            PairingState::Open if !pairing.live => return Ok(ClaimPairing::Invalid),
            PairingState::Open => {}
        }
        let key = key_use(&mut tx, claim.agent_pubkey, &owner_bytes).await?;
        if matches!(key, KeyUse::InUse) {
            return Ok(ClaimPairing::KeyInUse);
        }
        // One external device per computer and owner; a revoked one is
        // never revived (a new row is made instead).
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM devices WHERE principal_id = $1 AND kind = 'external' \
               AND device_pubkey = $2 AND revoked_at IS NULL FOR UPDATE",
        )
        .bind(owner_bytes.as_slice())
        .bind(claim.device_pubkey.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let device_id = match existing {
            Some(device_id) => device_id,
            None => {
                let live: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM devices WHERE principal_id = $1 \
                       AND kind = 'external' AND revoked_at IS NULL",
                )
                .bind(owner_bytes.as_slice())
                .fetch_one(&mut *tx)
                .await?;
                if live >= MAX_DEVICES_PER_PRINCIPAL {
                    return Ok(ClaimPairing::DeviceLimit);
                }
                sqlx::query_scalar(
                    "INSERT INTO devices \
                       (principal_id, name, platform, kind, device_pubkey, device_key_kind) \
                     VALUES ($1, $2, 'external', 'external', $3, $4) RETURNING id",
                )
                .bind(owner_bytes.as_slice())
                .bind(claim.device_name.unwrap_or("External agent"))
                .bind(claim.device_pubkey.as_slice())
                .bind(claim.device_key_kind)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        let bound = match key {
            KeyUse::Free => sqlx::query(
                "INSERT INTO key_registry (pubkey, principal_id, role, host_device_id) \
                 VALUES ($1, $2, 'external', $3) ON CONFLICT (pubkey) DO NOTHING",
            )
            .bind(claim.agent_pubkey.as_slice())
            .bind(owner_bytes.as_slice())
            .bind(device_id),
            _ => sqlx::query(
                "UPDATE key_registry SET role = 'external', host_device_id = $3 \
                 WHERE pubkey = $1 AND principal_id = $2 AND role = 'agent' \
                   AND host_device_id IS NULL AND revoked_at IS NULL",
            )
            .bind(claim.agent_pubkey.as_slice())
            .bind(owner_bytes.as_slice())
            .bind(device_id),
        }
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if bound != 1 {
            // Registered concurrently (outside the owner lock): nothing is
            // kept and the code stays usable.
            return Ok(ClaimPairing::KeyInUse);
        }
        sqlx::query(
            "UPDATE pairing_requests SET state = 'connected', agent_pubkey = $2, \
                    device_pubkey = $3, device_name = $4, device_id = $5, claimed_at = now() \
             WHERE id = $1",
        )
        .bind(pairing.id)
        .bind(claim.agent_pubkey.as_slice())
        .bind(claim.device_pubkey.as_slice())
        .bind(claim.device_name)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ClaimPairing::Connected {
            pairing_id: pairing.id,
            device_id,
        })
    }

    /// `owner`'s pairings started in the last day, newest first.
    pub async fn recent_pairings(&self, owner: &PrincipalId) -> Result<Vec<PairingRecord>> {
        let rows = sqlx::query(
            "SELECT *, expires_at > now() AS live FROM pairing_requests WHERE principal_id = $1 \
               AND created_at > now() - interval '1 day' \
             ORDER BY created_at DESC, id LIMIT 50",
        )
        .bind(owner.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(record).collect()
    }
}
