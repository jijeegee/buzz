//! Observer telemetry tier quotas.
//!
//! Agent observer telemetry (kind 24200, `frame=telemetry`) and agent typing
//! indicators (kind 20002) are limited by the owner's tier; agents inherit it.
//! Each tier has executor-side values that a conforming `buzz-acp` stays
//! within. The relay enforces those values × `headroom_pct` / 100, so it only
//! stops tampered or broken executors.
//!
//! Byte limits count plaintext. The relay cannot decrypt frames, so it charges
//! each frame the NIP-44 padded length reconstructed from the ciphertext length
//! ([`nip44_plaintext_upper_bound`]), which is never less than the plaintext.
//!
//! Counters live in Redis so every relay node shares one budget. One Lua script
//! makes each decision: a frame is charged against every limit only when every
//! limit admits it, so rejected frames consume nothing.

use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use buzz_core::observer::{
    OBSERVER_FRAME_TAG, OBSERVER_FRAME_TELEMETRY, OBSERVER_MAX_PLAINTEXT_LEN,
};
use buzz_core::CommunityId;
use nostr::{Event, PublicKey};
use serde::Serialize;
use tracing::warn;
use uuid::Uuid;

use crate::config::ConfigError;
use crate::connection::{AuthState, ConnectionState};
use crate::state::AppState;

/// Agents count toward the account's active-agent cap for this long after
/// their last admitted telemetry frame.
const ACTIVE_AGENT_WINDOW_MS: u64 = 60_000;
/// Agents count toward the account's typing cap for this long after their last
/// admitted typing indicator.
const TYPING_WINDOW_MS: u64 = 10_000;
/// Extra lifetime on fixed-window keys so a key never expires mid-window.
const COUNTER_TTL_SLACK_MS: u64 = 1_000;
/// Backoff hint sent when the shared quota store cannot be reached.
const UNAVAILABLE_RETRY_SECS: u64 = 5;
/// NIP-44 v2 payload bytes around the padded plaintext: version (1), nonce
/// (32), plaintext length prefix (2) and MAC (32).
const NIP44_V2_OVERHEAD: usize = 1 + 32 + 2 + 32;

/// Observer telemetry tier of an owner account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ObserverTier {
    /// 무료: summarised telemetry.
    Free,
    /// 일반: summarised telemetry with short arguments and results.
    Standard,
    /// 고가: full telemetry detail.
    Premium,
}

impl ObserverTier {
    /// Every tier, cheapest first.
    pub const ALL: [Self; 3] = [Self::Free, Self::Standard, Self::Premium];

    /// Stable wire and storage name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Standard => "standard",
            Self::Premium => "premium",
        }
    }
}

impl fmt::Display for ObserverTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ObserverTier {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|tier| tier.as_str() == raw)
            .ok_or_else(|| format!("unknown observer tier '{raw}'"))
    }
}

/// Limits for one tier. Byte values are plaintext bytes (decimal KB/MB).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TierLimits {
    /// Normal executor flush interval. Advisory; the relay does not enforce it.
    pub tick_ms: u64,
    /// Frames per minute per agent.
    pub agent_frames_per_min: u64,
    /// Burst frames per second per agent.
    pub agent_burst_per_sec: u64,
    /// Largest single frame per agent.
    pub agent_frame_max_bytes: u64,
    /// Bytes per minute per agent.
    pub agent_bytes_per_min: u64,
    /// Agents of one owner that may send telemetry within one minute.
    pub account_agents: u64,
    /// Frames per minute summed over the owner's agents.
    pub account_frames_per_min: u64,
    /// Burst frames per second summed over the owner's agents.
    pub account_burst_per_sec: u64,
    /// Bytes per minute summed over the owner's agents.
    pub account_bytes_per_min: u64,
    /// Bytes per UTC day summed over the owner's agents.
    pub account_bytes_per_day: u64,
    /// Agents of one owner that may show "working" (typing) at once.
    pub account_typing_agents: u64,
}

impl TierLimits {
    /// Executor defaults for [`ObserverTier::Free`].
    pub const FREE: Self = Self {
        tick_ms: 1_500,
        agent_frames_per_min: 60,
        agent_burst_per_sec: 5,
        agent_frame_max_bytes: 4_096,
        agent_bytes_per_min: 100_000,
        account_agents: 20,
        account_frames_per_min: 300,
        account_burst_per_sec: 20,
        account_bytes_per_min: 500_000,
        account_bytes_per_day: 100_000_000,
        account_typing_agents: 20,
    };

    /// Executor defaults for [`ObserverTier::Standard`].
    pub const STANDARD: Self = Self {
        tick_ms: 1_000,
        agent_frames_per_min: 60,
        agent_burst_per_sec: 5,
        agent_frame_max_bytes: 16_384,
        agent_bytes_per_min: 400_000,
        account_agents: 50,
        account_frames_per_min: 1_500,
        account_burst_per_sec: 50,
        account_bytes_per_min: 4_000_000,
        account_bytes_per_day: 1_000_000_000,
        account_typing_agents: 50,
    };

    /// Executor defaults for [`ObserverTier::Premium`].
    pub const PREMIUM: Self = Self {
        tick_ms: 1_000,
        agent_frames_per_min: 60,
        agent_burst_per_sec: 5,
        agent_frame_max_bytes: 65_535,
        agent_bytes_per_min: 4_000_000,
        account_agents: 200,
        account_frames_per_min: 6_000,
        account_burst_per_sec: 200,
        account_bytes_per_min: 40_000_000,
        account_bytes_per_day: 15_000_000_000,
        account_typing_agents: 200,
    };

    /// Every field with its env-var suffix (`BUZZ_OBSERVER_{TIER}_{SUFFIX}`).
    fn fields_mut(&mut self) -> [(&'static str, &mut u64); 11] {
        [
            ("TICK_MS", &mut self.tick_ms),
            ("AGENT_FRAMES_PER_MIN", &mut self.agent_frames_per_min),
            ("AGENT_BURST_PER_SEC", &mut self.agent_burst_per_sec),
            ("AGENT_FRAME_MAX_BYTES", &mut self.agent_frame_max_bytes),
            ("AGENT_BYTES_PER_MIN", &mut self.agent_bytes_per_min),
            ("ACCOUNT_AGENTS", &mut self.account_agents),
            ("ACCOUNT_FRAMES_PER_MIN", &mut self.account_frames_per_min),
            ("ACCOUNT_BURST_PER_SEC", &mut self.account_burst_per_sec),
            ("ACCOUNT_BYTES_PER_MIN", &mut self.account_bytes_per_min),
            ("ACCOUNT_BYTES_PER_DAY", &mut self.account_bytes_per_day),
            ("ACCOUNT_TYPING_AGENTS", &mut self.account_typing_agents),
        ]
    }

    /// The executor's fair share when `active_agents` agents of one owner send
    /// at once: `(frames_per_min, bytes_per_min)`, each the account limit split
    /// evenly and capped at the per-agent limit.
    pub fn share(&self, active_agents: u64) -> (u64, u64) {
        let agents = active_agents.max(1);
        (
            (self.account_frames_per_min / agents).min(self.agent_frames_per_min),
            (self.account_bytes_per_min / agents).min(self.agent_bytes_per_min),
        )
    }
}

/// Observer tier configuration (env `BUZZ_OBSERVER_*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverQuotaConfig {
    /// Tier of owners without an `observer_tiers` override.
    pub default_tier: ObserverTier,
    /// Server limit = executor limit × `headroom_pct` / 100, rounded up. ≥ 100.
    pub headroom_pct: u64,
    /// Telemetry frames per second one connection may submit before the relay
    /// spends a signature verification on them.
    pub conn_frames_per_sec: u64,
    /// Executor limits for [`ObserverTier::Free`].
    pub free: TierLimits,
    /// Executor limits for [`ObserverTier::Standard`].
    pub standard: TierLimits,
    /// Executor limits for [`ObserverTier::Premium`].
    pub premium: TierLimits,
    /// Devices per tier that may hold the owner's observer subscription.
    pub receiving_devices: ReceivingDevices,
}

/// Devices of one owner that may receive observer frames at once, per tier.
///
/// A device count is not a rate, so `headroom_pct` never scales it: the
/// number apps show is the number the relay enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceivingDevices {
    /// [`ObserverTier::Free`].
    pub free: u64,
    /// [`ObserverTier::Standard`].
    pub standard: u64,
    /// [`ObserverTier::Premium`].
    pub premium: u64,
}

impl Default for ReceivingDevices {
    fn default() -> Self {
        Self {
            free: 2,
            standard: 3,
            premium: 5,
        }
    }
}

impl ReceivingDevices {
    /// The cap of `tier`.
    pub fn get(&self, tier: ObserverTier) -> u64 {
        match tier {
            ObserverTier::Free => self.free,
            ObserverTier::Standard => self.standard,
            ObserverTier::Premium => self.premium,
        }
    }

    fn get_mut(&mut self, tier: ObserverTier) -> &mut u64 {
        match tier {
            ObserverTier::Free => &mut self.free,
            ObserverTier::Standard => &mut self.standard,
            ObserverTier::Premium => &mut self.premium,
        }
    }
}

impl Default for ObserverQuotaConfig {
    fn default() -> Self {
        Self {
            // Existing accounts keep today's full detail on first deploy.
            default_tier: ObserverTier::Premium,
            headroom_pct: 150,
            conn_frames_per_sec: 30,
            free: TierLimits::FREE,
            standard: TierLimits::STANDARD,
            premium: TierLimits::PREMIUM,
            receiving_devices: ReceivingDevices::default(),
        }
    }
}

impl ObserverQuotaConfig {
    /// Executor-side limits of `tier`.
    pub fn executor_limits(&self, tier: ObserverTier) -> &TierLimits {
        match tier {
            ObserverTier::Free => &self.free,
            ObserverTier::Standard => &self.standard,
            ObserverTier::Premium => &self.premium,
        }
    }

    fn executor_limits_mut(&mut self, tier: ObserverTier) -> &mut TierLimits {
        match tier {
            ObserverTier::Free => &mut self.free,
            ObserverTier::Standard => &mut self.standard,
            ObserverTier::Premium => &mut self.premium,
        }
    }

    /// Relay-enforced limits of `tier`: every executor value except `tick_ms`
    /// scaled by `headroom_pct`, rounded up. The frame cap never exceeds the
    /// largest plaintext NIP-44 can carry.
    pub fn server_limits(&self, tier: ObserverTier) -> TierLimits {
        let mut limits = *self.executor_limits(tier);
        for (name, value) in limits.fields_mut() {
            if name != "TICK_MS" {
                let scaled = (u128::from(*value) * u128::from(self.headroom_pct)).div_ceil(100);
                *value = u64::try_from(scaled).unwrap_or(u64::MAX);
            }
        }
        limits.agent_frame_max_bytes = limits
            .agent_frame_max_bytes
            .min(OBSERVER_MAX_PLAINTEXT_LEN as u64);
        limits
    }

    /// The policy document served to executors for `tier`.
    pub fn policy(&self, tier: ObserverTier) -> ObserverPolicy {
        ObserverPolicy {
            tier,
            headroom_pct: self.headroom_pct,
            executor: *self.executor_limits(tier),
            server: self.server_limits(tier),
            conn_frames_per_sec: self.conn_frames_per_sec,
            receiving_devices: self.receiving_devices.get(tier),
        }
    }

    /// Load from `BUZZ_OBSERVER_DEFAULT_TIER`, `BUZZ_OBSERVER_HEADROOM_PCT`,
    /// `BUZZ_OBSERVER_CONN_FRAMES_PER_SEC` and per-tier
    /// `BUZZ_OBSERVER_{FREE|STANDARD|PREMIUM}_{FIELD}` (e.g.
    /// `BUZZ_OBSERVER_FREE_ACCOUNT_AGENTS`), including
    /// `BUZZ_OBSERVER_{TIER}_RECEIVING_DEVICES`. Unset values keep the defaults.
    pub(crate) fn from_env() -> Result<Self, ConfigError> {
        let defaults = Self::default();
        let default_tier = match std::env::var("BUZZ_OBSERVER_DEFAULT_TIER") {
            Ok(raw) => raw.trim().parse().map_err(|_| {
                ConfigError::InvalidValue(
                    "BUZZ_OBSERVER_DEFAULT_TIER must be one of free, standard, premium".into(),
                )
            })?,
            Err(std::env::VarError::NotPresent) => defaults.default_tier,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(ConfigError::InvalidValue(
                    "BUZZ_OBSERVER_DEFAULT_TIER must be valid Unicode".into(),
                ));
            }
        };
        let headroom_pct = crate::config::positive_u64_from_env(
            "BUZZ_OBSERVER_HEADROOM_PCT",
            defaults.headroom_pct,
        )?;
        if headroom_pct < 100 {
            return Err(ConfigError::InvalidValue(
                "BUZZ_OBSERVER_HEADROOM_PCT must be at least 100".into(),
            ));
        }
        let conn_frames_per_sec = crate::config::positive_u64_from_env(
            "BUZZ_OBSERVER_CONN_FRAMES_PER_SEC",
            defaults.conn_frames_per_sec,
        )?;
        let mut config = Self {
            default_tier,
            headroom_pct,
            conn_frames_per_sec,
            ..defaults
        };
        for tier in ObserverTier::ALL {
            let prefix = tier.as_str().to_ascii_uppercase();
            for (field, value) in config.executor_limits_mut(tier).fields_mut() {
                *value = crate::config::positive_u64_from_env(
                    &format!("BUZZ_OBSERVER_{prefix}_{field}"),
                    *value,
                )?;
            }
            let devices = config.receiving_devices.get_mut(tier);
            *devices = crate::config::positive_u64_from_env(
                &format!("BUZZ_OBSERVER_{prefix}_RECEIVING_DEVICES"),
                *devices,
            )?;
        }
        Ok(config)
    }
}

/// `GET /api/observer/policy` response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObserverPolicy {
    /// Tier that applies to the caller's telemetry.
    pub tier: ObserverTier,
    /// Server limit = executor limit × `headroom_pct` / 100.
    pub headroom_pct: u64,
    /// Limits a conforming executor stays within.
    pub executor: TierLimits,
    /// Limits the relay enforces.
    pub server: TierLimits,
    /// Pre-verification telemetry frames per second per connection.
    pub conn_frames_per_sec: u64,
    /// Devices of the owner that may hold an observer subscription at once.
    /// Not scaled by `headroom_pct`.
    pub receiving_devices: u64,
}

/// Upper bound on the plaintext length of a NIP-44 v2 ciphertext of
/// `content_len` base64 characters.
///
/// Returns the padded plaintext length, which is at least the real plaintext
/// length (padding adds at most 25% just above a power of two, ≤ 12.5% for
/// larger frames, and rounds tiny messages up to 32 bytes). Base64 `=` padding
/// can make the decoded length look up to 2 bytes longer; padded lengths are
/// multiples of 32, so rounding down to one recovers the exact padded length.
pub fn nip44_plaintext_upper_bound(content_len: usize) -> usize {
    let padded = (content_len.saturating_mul(3) / 4).saturating_sub(NIP44_V2_OVERHEAD);
    padded - padded % 32
}

/// NIP-44 v2 padded length of a `plaintext_len`-byte message.
pub fn nip44_padded_len(plaintext_len: usize) -> usize {
    if plaintext_len <= 32 {
        return 32;
    }
    let next_power = 1usize << (usize::BITS - (plaintext_len - 1).leading_zeros());
    let chunk = if next_power <= 256 {
        32
    } else {
        next_power / 8
    };
    chunk * ((plaintext_len - 1) / chunk + 1)
}

/// Whether a NIP-44 ciphertext of `content_len` characters may carry more
/// plaintext than `limits.agent_frame_max_bytes`. Compares padded lengths: any
/// plaintext within the cap pads to at most the cap's padded length.
pub fn frame_exceeds_cap(content_len: usize, limits: &TierLimits) -> bool {
    nip44_plaintext_upper_bound(content_len)
        > nip44_padded_len(limits.agent_frame_max_bytes as usize)
}

/// Which observer limit denied a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// The owner already has the maximum number of active agents.
    ActiveAgents,
    /// Per-agent frames per second.
    AgentBurst,
    /// Per-agent frames per minute.
    AgentFrames,
    /// Per-agent bytes per minute.
    AgentBytes,
    /// Per-account frames per second.
    AccountBurst,
    /// Per-account frames per minute.
    AccountFrames,
    /// Per-account bytes per minute.
    AccountBytes,
    /// Per-account bytes per UTC day.
    AccountDailyBytes,
}

impl LimitKind {
    /// Human-readable limit name used in rejection messages.
    pub fn description(self) -> &'static str {
        match self {
            Self::ActiveAgents => "active agents per account",
            Self::AgentBurst => "agent frames per second",
            Self::AgentFrames => "agent frames per minute",
            Self::AgentBytes => "agent bytes per minute",
            Self::AccountBurst => "account frames per second",
            Self::AccountFrames => "account frames per minute",
            Self::AccountBytes => "account bytes per minute",
            Self::AccountDailyBytes => "account bytes per day",
        }
    }

    /// Bounded metric label.
    pub fn label(self) -> &'static str {
        match self {
            Self::ActiveAgents => "active_agents",
            Self::AgentBurst => "agent_burst",
            Self::AgentFrames => "agent_frames",
            Self::AgentBytes => "agent_bytes",
            Self::AccountBurst => "account_burst",
            Self::AccountFrames => "account_frames",
            Self::AccountBytes => "account_bytes",
            Self::AccountDailyBytes => "account_daily_bytes",
        }
    }
}

/// Outcome of one telemetry admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Charged against every limit.
    Allowed {
        /// Active agents of the owner, including this one.
        active_agents: u64,
    },
    /// Nothing was charged.
    Denied {
        /// The first limit that denied the frame.
        limit: LimitKind,
        /// Seconds until that limit admits again (≥ 1).
        retry_after_secs: u64,
        /// Active agents of the owner, including the sender.
        active_agents: u64,
    },
}

/// The quota store could not make a decision.
#[derive(Debug, thiserror::Error)]
#[error("observer quota store unavailable: {0}")]
pub struct QuotaError(pub(crate) String);

/// Shared observer quota counters.
#[async_trait]
pub trait ObserverQuotaStore: Send + Sync {
    /// Atomically check every telemetry limit for one `plaintext_bytes` frame
    /// at `now_ms` (Unix ms) and charge all of them only if all admit it.
    async fn admit_telemetry(
        &self,
        community: CommunityId,
        owner: &PublicKey,
        agent: &PublicKey,
        plaintext_bytes: u64,
        now_ms: u64,
        limits: &TierLimits,
    ) -> Result<Decision, QuotaError>;

    /// Admit one typing indicator: allowed when `agent` is already among the
    /// owner's typing agents of the last 10 s or fewer than `cap` are.
    async fn admit_typing(
        &self,
        community: CommunityId,
        owner: &PublicKey,
        agent: &PublicKey,
        now_ms: u64,
        cap: u64,
    ) -> Result<bool, QuotaError>;
}

/// One fixed-window counter checked by a telemetry decision.
#[derive(Debug, Clone)]
struct WindowCounter {
    key: String,
    limit_kind: LimitKind,
    increment: u64,
    limit: u64,
    window_end_ms: u64,
}

impl WindowCounter {
    fn ttl_ms(&self, now_ms: u64) -> u64 {
        self.window_end_ms.saturating_sub(now_ms) + COUNTER_TTL_SLACK_MS
    }
}

/// Every key and limit of one telemetry decision. Shared by the Redis and
/// in-memory stores so both evaluate identical windows.
struct TelemetryPlan {
    agents_key: String,
    member: String,
    agent_cap: u64,
    counters: Vec<WindowCounter>,
}

/// Key prefix of one owner's counters. The owner is a Redis Cluster hash tag so
/// every key of one decision lands in one slot.
pub(crate) fn owner_key_prefix(community: CommunityId, owner: &PublicKey) -> String {
    format!("buzz:{community}:obsq:{{{}}}", owner.to_hex())
}

fn secs_until(end_ms: u64, now_ms: u64) -> u64 {
    end_ms.saturating_sub(now_ms).div_ceil(1000).max(1)
}

impl TelemetryPlan {
    fn new(
        community: CommunityId,
        owner: &PublicKey,
        agent: &PublicKey,
        bytes: u64,
        now_ms: u64,
        limits: &TierLimits,
    ) -> Self {
        let prefix = owner_key_prefix(community, owner);
        let member = agent.to_hex();
        let counter = |scope: &str, limit_kind, window_ms: u64, increment, limit| {
            let index = now_ms / window_ms;
            WindowCounter {
                key: format!("{prefix}:{scope}:{}:{index}", LimitKind::label(limit_kind)),
                limit_kind,
                increment,
                limit,
                window_end_ms: (index + 1) * window_ms,
            }
        };
        let agent_scope = format!("a:{member}");
        let counters = vec![
            counter(
                &agent_scope,
                LimitKind::AgentBurst,
                1_000,
                1,
                limits.agent_burst_per_sec,
            ),
            counter(
                &agent_scope,
                LimitKind::AgentFrames,
                60_000,
                1,
                limits.agent_frames_per_min,
            ),
            counter(
                &agent_scope,
                LimitKind::AgentBytes,
                60_000,
                bytes,
                limits.agent_bytes_per_min,
            ),
            counter(
                "o",
                LimitKind::AccountBurst,
                1_000,
                1,
                limits.account_burst_per_sec,
            ),
            counter(
                "o",
                LimitKind::AccountFrames,
                60_000,
                1,
                limits.account_frames_per_min,
            ),
            counter(
                "o",
                LimitKind::AccountBytes,
                60_000,
                bytes,
                limits.account_bytes_per_min,
            ),
            counter(
                "o",
                LimitKind::AccountDailyBytes,
                86_400_000,
                bytes,
                limits.account_bytes_per_day,
            ),
        ];
        Self {
            agents_key: format!("{prefix}:agents"),
            member,
            agent_cap: limits.account_agents,
            counters,
        }
    }

    fn slot_denied(&self, active_agents: u64, oldest_ms: u64, now_ms: u64) -> Decision {
        Decision::Denied {
            limit: LimitKind::ActiveAgents,
            retry_after_secs: secs_until(oldest_ms + ACTIVE_AGENT_WINDOW_MS, now_ms),
            active_agents,
        }
    }

    fn counter_denied(&self, index: usize, active_agents: u64, now_ms: u64) -> Decision {
        let counter = &self.counters[index];
        Decision::Denied {
            limit: counter.limit_kind,
            retry_after_secs: secs_until(counter.window_end_ms, now_ms),
            active_agents,
        }
    }
}

fn typing_key(community: CommunityId, owner: &PublicKey) -> String {
    format!("{}:typing", owner_key_prefix(community, owner))
}

/// KEYS[1] = active-agent sorted set, KEYS[2..] = fixed-window counters.
/// ARGV[1] = now ms, ARGV[2] = agent, ARGV[3] = agent cap, ARGV[4] = active
/// window ms, then per counter: increment, limit, TTL ms.
/// Returns {0, active, 0} allowed, {1, active, oldest_ms} agent cap,
/// {2, active, counter_index (1-based)} counter limit.
const TELEMETRY_SCRIPT: &str = r#"
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[4])
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now - window)
local active = redis.call('ZCARD', KEYS[1])
if not redis.call('ZSCORE', KEYS[1], ARGV[2]) then
    if active >= tonumber(ARGV[3]) then
        local oldest = redis.call('ZRANGE', KEYS[1], 0, 0, 'WITHSCORES')
        return {1, active, tonumber(oldest[2])}
    end
    active = active + 1
end
for i = 2, #KEYS do
    local base = 5 + (i - 2) * 3
    local current = tonumber(redis.call('GET', KEYS[i]) or '0')
    if current + tonumber(ARGV[base]) > tonumber(ARGV[base + 1]) then
        return {2, active, i - 1}
    end
end
for i = 2, #KEYS do
    local base = 5 + (i - 2) * 3
    redis.call('INCRBY', KEYS[i], ARGV[base])
    redis.call('PEXPIRE', KEYS[i], ARGV[base + 2])
end
redis.call('ZADD', KEYS[1], now, ARGV[2])
redis.call('PEXPIRE', KEYS[1], window)
return {0, active, 0}
"#;

/// KEYS[1] = typing sorted set. ARGV[1] = now ms, ARGV[2] = agent, ARGV[3] =
/// cap, ARGV[4] = window ms. Returns 1 when admitted.
const TYPING_SCRIPT: &str = r#"
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[4])
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now - window)
if not redis.call('ZSCORE', KEYS[1], ARGV[2])
    and redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[3]) then
    return 0
end
redis.call('ZADD', KEYS[1], now, ARGV[2])
redis.call('PEXPIRE', KEYS[1], window)
return 1
"#;

/// Production quota store: one Lua script per decision.
pub struct RedisObserverQuota {
    pool: deadpool_redis::Pool,
    telemetry: redis::Script,
    typing: redis::Script,
}

impl RedisObserverQuota {
    /// Create a store backed by `pool`.
    pub fn new(pool: deadpool_redis::Pool) -> Self {
        Self {
            pool,
            telemetry: redis::Script::new(TELEMETRY_SCRIPT),
            typing: redis::Script::new(TYPING_SCRIPT),
        }
    }
}

#[async_trait]
impl ObserverQuotaStore for RedisObserverQuota {
    async fn admit_telemetry(
        &self,
        community: CommunityId,
        owner: &PublicKey,
        agent: &PublicKey,
        plaintext_bytes: u64,
        now_ms: u64,
        limits: &TierLimits,
    ) -> Result<Decision, QuotaError> {
        let plan = TelemetryPlan::new(community, owner, agent, plaintext_bytes, now_ms, limits);
        let mut invocation = self.telemetry.prepare_invoke();
        invocation.key(&plan.agents_key);
        for counter in &plan.counters {
            invocation.key(&counter.key);
        }
        invocation
            .arg(now_ms)
            .arg(&plan.member)
            .arg(plan.agent_cap)
            .arg(ACTIVE_AGENT_WINDOW_MS);
        for counter in &plan.counters {
            invocation
                .arg(counter.increment)
                .arg(counter.limit)
                .arg(counter.ttl_ms(now_ms));
        }
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| QuotaError(format!("Redis pool: {e}")))?;
        let (status, active, detail): (i64, i64, i64) =
            invocation
                .invoke_async(&mut *conn)
                .await
                .map_err(|e| QuotaError(format!("Redis observer quota script: {e}")))?;
        let active = u64::try_from(active).unwrap_or(0);
        match status {
            0 => Ok(Decision::Allowed {
                active_agents: active,
            }),
            1 => Ok(plan.slot_denied(active, u64::try_from(detail).unwrap_or(0), now_ms)),
            2 => usize::try_from(detail - 1)
                .ok()
                .filter(|index| *index < plan.counters.len())
                .map(|index| plan.counter_denied(index, active, now_ms))
                .ok_or_else(|| QuotaError(format!("unexpected counter index {detail}"))),
            other => Err(QuotaError(format!("unexpected script status {other}"))),
        }
    }

    async fn admit_typing(
        &self,
        community: CommunityId,
        owner: &PublicKey,
        agent: &PublicKey,
        now_ms: u64,
        cap: u64,
    ) -> Result<bool, QuotaError> {
        let mut conn = self
            .pool
            .get()
            .await
            .map_err(|e| QuotaError(format!("Redis pool: {e}")))?;
        let admitted: i64 = self
            .typing
            .key(typing_key(community, owner))
            .arg(now_ms)
            .arg(agent.to_hex())
            .arg(cap)
            .arg(TYPING_WINDOW_MS)
            .invoke_async(&mut *conn)
            .await
            .map_err(|e| QuotaError(format!("Redis observer typing script: {e}")))?;
        Ok(admitted == 1)
    }
}

/// In-memory store with the Redis script's semantics, for tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryObserverQuota {
    state: Mutex<MemoryQuotaState>,
}

#[cfg(test)]
#[derive(Default)]
struct MemoryQuotaState {
    /// key → (value, expires at ms)
    counters: std::collections::HashMap<String, (u64, u64)>,
    /// key → member → last seen ms
    sets: std::collections::HashMap<String, std::collections::HashMap<String, u64>>,
}

#[cfg(test)]
impl MemoryQuotaState {
    /// Prunes `key` to members seen within `window_ms`; returns the live set.
    fn live_set(
        &mut self,
        key: &str,
        now_ms: u64,
        window_ms: u64,
    ) -> &mut std::collections::HashMap<String, u64> {
        let set = self.sets.entry(key.to_owned()).or_default();
        set.retain(|_, seen| *seen > now_ms.saturating_sub(window_ms));
        set
    }
}

#[cfg(test)]
#[async_trait]
impl ObserverQuotaStore for MemoryObserverQuota {
    async fn admit_telemetry(
        &self,
        community: CommunityId,
        owner: &PublicKey,
        agent: &PublicKey,
        plaintext_bytes: u64,
        now_ms: u64,
        limits: &TierLimits,
    ) -> Result<Decision, QuotaError> {
        let plan = TelemetryPlan::new(community, owner, agent, plaintext_bytes, now_ms, limits);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let set = state.live_set(&plan.agents_key, now_ms, ACTIVE_AGENT_WINDOW_MS);
        let mut active = set.len() as u64;
        if !set.contains_key(&plan.member) {
            if active >= plan.agent_cap {
                let oldest = set.values().copied().min().unwrap_or(now_ms);
                return Ok(plan.slot_denied(active, oldest, now_ms));
            }
            active += 1;
        }
        let current = |state: &MemoryQuotaState, key: &str| {
            state
                .counters
                .get(key)
                .filter(|(_, expires)| *expires > now_ms)
                .map_or(0, |(value, _)| *value)
        };
        for (index, counter) in plan.counters.iter().enumerate() {
            if current(&state, &counter.key) + counter.increment > counter.limit {
                return Ok(plan.counter_denied(index, active, now_ms));
            }
        }
        for counter in &plan.counters {
            let value = current(&state, &counter.key) + counter.increment;
            state.counters.insert(
                counter.key.clone(),
                (value, now_ms + counter.ttl_ms(now_ms)),
            );
        }
        state
            .live_set(&plan.agents_key, now_ms, ACTIVE_AGENT_WINDOW_MS)
            .insert(plan.member, now_ms);
        Ok(Decision::Allowed {
            active_agents: active,
        })
    }

    async fn admit_typing(
        &self,
        community: CommunityId,
        owner: &PublicKey,
        agent: &PublicKey,
        now_ms: u64,
        cap: u64,
    ) -> Result<bool, QuotaError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let set = state.live_set(&typing_key(community, owner), now_ms, TYPING_WINDOW_MS);
        let member = agent.to_hex();
        if !set.contains_key(&member) && set.len() as u64 >= cap {
            return Ok(false);
        }
        set.insert(member, now_ms);
        Ok(true)
    }
}

pub(crate) fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// Resolve `owner`'s tier: the `observer_tiers` override, else the configured
/// default. Overrides are cached for 60 s.
pub(crate) async fn resolve_tier(
    state: &AppState,
    community: CommunityId,
    owner: &PublicKey,
) -> Result<ObserverTier, buzz_db::DbError> {
    let key = (community, owner.to_bytes());
    let stored = match state.observer_tier_cache.get(&key) {
        Some(stored) => stored,
        None => {
            let stored = state
                .db
                .get_observer_tier(community, &key.1)
                .await?
                .map(|name| name.parse::<ObserverTier>())
                .transpose()
                .map_err(buzz_db::DbError::InvalidData)?;
            state.observer_tier_cache.insert(key, stored);
            stored
        }
    };
    Ok(stored.unwrap_or(state.config.observer_quota.default_tier))
}

/// The registered owner of agent `pubkey`, or `None` for non-agents. Cached
/// for 60 s; agent ownership is first-write-wins.
pub(crate) async fn agent_owner(
    state: &AppState,
    community: CommunityId,
    pubkey: &PublicKey,
) -> Result<Option<PublicKey>, buzz_db::DbError> {
    let key = (community, pubkey.to_bytes());
    if let Some(owner) = state.agent_owner_cache.get(&key) {
        return Ok(owner.and_then(|bytes| PublicKey::from_slice(&bytes).ok()));
    }
    let owner = state
        .db
        .get_agent_channel_policy(community, &key.1)
        .await?
        .and_then(|(_, owner)| owner)
        .and_then(|bytes| PublicKey::from_slice(&bytes).ok());
    state
        .agent_owner_cache
        .insert(key, owner.map(|owner| owner.to_bytes()));
    Ok(owner)
}

/// Why a telemetry frame was not admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TelemetryRejection {
    /// The frame alone exceeds the tier's frame cap.
    TooLarge(String),
    /// A rate or volume limit denied the frame.
    Limited(String),
    /// Tier or quota state could not be read; fail closed with a backoff hint.
    Unavailable,
}

impl TelemetryRejection {
    /// The `OK false` message. Rate rejections keep the `retry in {N}s` form
    /// that `buzz-acp` parses.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::TooLarge(message) | Self::Limited(message) => message.clone(),
            Self::Unavailable => format!(
                "rate-limited: observer quota unavailable; retry in {UNAVAILABLE_RETRY_SECS}s"
            ),
        }
    }
}

/// Admit one verified, owner-authorized telemetry frame of `content_len`
/// ciphertext characters against the owner's tier.
pub(crate) async fn admit_telemetry_frame(
    state: &AppState,
    community: CommunityId,
    owner: &PublicKey,
    agent: &PublicKey,
    content_len: usize,
) -> Result<(), TelemetryRejection> {
    let config = &state.config.observer_quota;
    let tier = resolve_tier(state, community, owner).await.map_err(|e| {
        warn!(error = %e, "observer tier lookup failed");
        reject_metric("unavailable");
        TelemetryRejection::Unavailable
    })?;
    let server = config.server_limits(tier);
    if frame_exceeds_cap(content_len, &server) {
        reject_metric("frame_size");
        return Err(TelemetryRejection::TooLarge(format!(
            "invalid: observer frame exceeds {} bytes for tier {tier}",
            server.agent_frame_max_bytes
        )));
    }
    let decision = state
        .observer_quota
        .admit_telemetry(
            community,
            owner,
            agent,
            nip44_plaintext_upper_bound(content_len) as u64,
            now_ms(),
            &server,
        )
        .await
        .map_err(|e| {
            warn!(error = %e, "observer quota unavailable");
            reject_metric("unavailable");
            TelemetryRejection::Unavailable
        })?;
    match decision {
        Decision::Allowed { .. } => Ok(()),
        Decision::Denied {
            limit,
            retry_after_secs,
            active_agents,
        } => {
            reject_metric(limit.label());
            let (frames, bytes) = config.executor_limits(tier).share(active_agents);
            Err(TelemetryRejection::Limited(format!(
                "rate-limited: observer {} exceeded (tier {tier}); retry in {retry_after_secs}s; \
                 share frames_per_min={frames} bytes_per_min={bytes}",
                limit.description()
            )))
        }
    }
}

pub(crate) fn reject_metric(reason: &'static str) {
    metrics::counter!("buzz_observer_quota_rejections_total", "reason" => reason).increment(1);
}

/// Whether a typing indicator from `sender` should be fanned out. Only agents
/// are capped, by their owner's typing-agent limit. Errors admit the indicator:
/// typing is cosmetic and must never block on quota storage.
pub(crate) async fn typing_admitted(
    state: &AppState,
    conn: &ConnectionState,
    sender: &PublicKey,
) -> bool {
    let community = conn.tenant.community();
    let session_owner = match conn.auth_state_snapshot() {
        AuthState::Authenticated(ctx) if ctx.pubkey == *sender => ctx.agent_owner_pubkey,
        _ => None,
    };
    let owner = match session_owner {
        Some(owner) => owner,
        None => match agent_owner(state, community, sender).await {
            Ok(Some(owner)) => owner,
            Ok(None) => return true,
            Err(e) => {
                warn!(error = %e, "typing owner lookup failed; admitting indicator");
                return true;
            }
        },
    };
    let tier = match resolve_tier(state, community, &owner).await {
        Ok(tier) => tier,
        Err(e) => {
            warn!(error = %e, "observer tier lookup failed; admitting typing indicator");
            return true;
        }
    };
    let cap = state
        .config
        .observer_quota
        .server_limits(tier)
        .account_typing_agents;
    match state
        .observer_quota
        .admit_typing(community, &owner, sender, now_ms(), cap)
        .await
    {
        Ok(admitted) => {
            if !admitted {
                reject_metric("typing_agents");
            }
            admitted
        }
        Err(e) => {
            warn!(error = %e, "observer typing quota unavailable; admitting indicator");
            true
        }
    }
}

/// Whether `event` is an agent→owner observer telemetry frame by its cleartext
/// tags. Signature and routing are verified later by the EVENT handler.
pub(crate) fn is_telemetry_frame(event: &Event) -> bool {
    event.kind.as_u16() as u32 == buzz_core::kind::KIND_AGENT_OBSERVER_FRAME
        && event.tags.iter().any(|tag| {
            let parts = tag.as_slice();
            parts.len() >= 2
                && parts[0] == OBSERVER_FRAME_TAG
                && parts[1] == OBSERVER_FRAME_TELEMETRY
        })
}

/// Per-connection telemetry cap applied before signature verification.
/// Fixed one-second windows; `second` is the current Unix second.
pub(crate) fn conn_frame_admitted_at(state: &AppState, conn_id: Uuid, second: u64) -> bool {
    let window = state
        .observer_conn_frames
        .get_with(conn_id, || Arc::new(Mutex::new((second, 0))));
    let mut window = window.lock().unwrap_or_else(PoisonError::into_inner);
    if window.0 != second {
        *window = (second, 0);
    }
    if window.1 >= state.config.observer_quota.conn_frames_per_sec {
        return false;
    }
    window.1 += 1;
    true
}

/// [`conn_frame_admitted_at`] at the current time.
pub(crate) fn conn_frame_admitted(state: &AppState, conn_id: Uuid) -> bool {
    conn_frame_admitted_at(state, conn_id, now_ms() / 1000)
}

#[cfg(test)]
mod tests;
