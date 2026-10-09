//! Observer receiving-device cap.
//!
//! An owner's apps receive their agents' observer frames (kind 24200) through
//! a subscription with `#p` = the owner. Each tier allows a number of devices
//! to hold such a subscription at once
//! ([`crate::observer_quota::ReceivingDevices`]); a REQ from one more device is
//! refused with `CLOSED` and registers nothing. Only that REQ is affected: the
//! connection's other subscriptions and its publishes keep working.
//!
//! A device is, in order of preference:
//! - `tok:<uuid>`: the server-assigned device of a token session;
//! - `tag:<id>`: the optional `["device", "<id>"]` tag of the NIP-42 AUTH event;
//! - `conn:<uuid>`: the connection itself.
//!
//! The prefixes keep the three namespaces apart. A client choosing its own tag
//! can only share or split its owner's own budget.
//!
//! Counts live in Redis so every relay node shares them: per owner, a sorted
//! set of devices and, per device, a sorted set of its live subscriptions
//! (`<conn_id>:<sub_id>`), all scored by last-seen time. CLOSE, sub_id reuse
//! and disconnect release a subscription at once; a device leaves the set when
//! its last subscription does. Each node refreshes its live subscriptions
//! every [`REFRESH_INTERVAL`]; entries older than [`DEVICE_STALE_MS`] are
//! pruned, which only matters when a node dies without releasing.
//!
//! When Redis or the tier lookup fails the REQ is admitted uncounted: the cap
//! protects the owner's fan-out, while telemetry volume stays bounded by the
//! telemetry quotas, which fail closed.

use std::collections::HashMap;
use std::sync::Arc;
#[cfg(test)]
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use buzz_auth::AuthContext;
use buzz_core::kind::KIND_AGENT_OBSERVER_FRAME;
use buzz_core::CommunityId;
use dashmap::DashMap;
use nostr::{Event, Filter, PublicKey};
use tracing::warn;
use uuid::Uuid;

use crate::connection::{AuthState, ConnectionState};
use crate::observer_quota::{now_ms, owner_key_prefix, reject_metric, resolve_tier, QuotaError};
use crate::state::AppState;

/// Observer subscriptions one device may hold at once.
pub const MAX_SUBS_PER_DEVICE: u64 = 2;
/// A device or subscription not refreshed for this long is presumed dead.
pub const DEVICE_STALE_MS: u64 = 180_000;
/// How often each node refreshes its live counted subscriptions.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// AUTH event tag naming the client's device.
pub const DEVICE_TAG: &str = "device";
/// Longest accepted device tag value.
pub const DEVICE_TAG_MAX_LEN: usize = 64;

/// One counted observer subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLease {
    /// Community of the subscription.
    pub community: CommunityId,
    /// Owner whose devices are counted.
    pub owner: PublicKey,
    /// Namespaced device id (`tok:`, `tag:` or `conn:`).
    pub device: String,
    /// The subscription: `<conn_id>:<sub_id>`.
    pub member: String,
}

impl DeviceLease {
    fn devices_key(&self) -> String {
        format!("{}:devices", owner_key_prefix(self.community, &self.owner))
    }

    fn subs_key(&self) -> String {
        format!(
            "{}:devsubs:{}",
            owner_key_prefix(self.community, &self.owner),
            self.device
        )
    }
}

/// Outcome of one device admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceDecision {
    /// The subscription is counted.
    Allowed,
    /// A new device would exceed the cap; `devices` are already live.
    DeviceCap {
        /// Live devices of the owner.
        devices: u64,
    },
    /// The device already holds `subs` observer subscriptions.
    SubsPerDevice {
        /// Live observer subscriptions of the device.
        subs: u64,
    },
}

/// Shared device counters.
#[async_trait]
pub trait ObserverDeviceStore: Send + Sync {
    /// Count `lease` unless that would exceed `cap` devices or `per_device`
    /// subscriptions on its device. A lease already counted is refreshed and
    /// allowed.
    async fn acquire_device(
        &self,
        lease: &DeviceLease,
        now_ms: u64,
        cap: u64,
        per_device: u64,
    ) -> Result<DeviceDecision, QuotaError>;

    /// Stop counting `lease`; drop its device once it has no subscription.
    async fn release_device(&self, lease: &DeviceLease) -> Result<(), QuotaError>;

    /// Mark a still-counted `lease` and its device live at `now_ms`. Never
    /// re-adds a released lease.
    async fn refresh_device(&self, lease: &DeviceLease, now_ms: u64) -> Result<(), QuotaError>;
}

/// KEYS[1] = owner's devices, KEYS[2] = this device's subscriptions.
/// ARGV[1] = now ms, ARGV[2] = device, ARGV[3] = subscription, ARGV[4] = device
/// cap, ARGV[5] = subscriptions per device, ARGV[6] = stale ms.
/// Returns {0, 0} allowed, {1, devices} device cap, {2, subs} per-device cap.
const ACQUIRE_SCRIPT: &str = r#"
local now = tonumber(ARGV[1])
local stale = tonumber(ARGV[6])
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', now - stale)
redis.call('ZREMRANGEBYSCORE', KEYS[2], '-inf', now - stale)
local subs = redis.call('ZCARD', KEYS[2])
if subs == 0 then
    redis.call('ZREM', KEYS[1], ARGV[2])
end
if not redis.call('ZSCORE', KEYS[2], ARGV[3]) then
    if not redis.call('ZSCORE', KEYS[1], ARGV[2]) then
        local devices = redis.call('ZCARD', KEYS[1])
        if devices >= tonumber(ARGV[4]) then
            return {1, devices}
        end
    end
    if subs >= tonumber(ARGV[5]) then
        return {2, subs}
    end
end
redis.call('ZADD', KEYS[2], now, ARGV[3])
redis.call('ZADD', KEYS[1], now, ARGV[2])
redis.call('PEXPIRE', KEYS[1], stale)
redis.call('PEXPIRE', KEYS[2], stale)
return {0, 0}
"#;

/// KEYS as [`ACQUIRE_SCRIPT`]. ARGV[1] = device, ARGV[2] = subscription.
const RELEASE_SCRIPT: &str = r#"
redis.call('ZREM', KEYS[2], ARGV[2])
if redis.call('ZCARD', KEYS[2]) == 0 then
    redis.call('ZREM', KEYS[1], ARGV[1])
    redis.call('DEL', KEYS[2])
end
return 1
"#;

/// KEYS as [`ACQUIRE_SCRIPT`]. ARGV[1] = now ms, ARGV[2] = device,
/// ARGV[3] = subscription, ARGV[4] = stale ms. Returns 1 when still counted.
const REFRESH_SCRIPT: &str = r#"
if not redis.call('ZSCORE', KEYS[2], ARGV[3]) then
    return 0
end
local now = tonumber(ARGV[1])
redis.call('ZADD', KEYS[2], now, ARGV[3])
redis.call('ZADD', KEYS[1], now, ARGV[2])
redis.call('PEXPIRE', KEYS[1], ARGV[4])
redis.call('PEXPIRE', KEYS[2], ARGV[4])
return 1
"#;

/// Production device store: one Lua script per decision.
pub struct RedisObserverDevices {
    pool: deadpool_redis::Pool,
    acquire: redis::Script,
    release: redis::Script,
    refresh: redis::Script,
}

impl RedisObserverDevices {
    /// Create a store backed by `pool`.
    pub fn new(pool: deadpool_redis::Pool) -> Self {
        Self {
            pool,
            acquire: redis::Script::new(ACQUIRE_SCRIPT),
            release: redis::Script::new(RELEASE_SCRIPT),
            refresh: redis::Script::new(REFRESH_SCRIPT),
        }
    }

    async fn conn(&self) -> Result<deadpool_redis::Connection, QuotaError> {
        self.pool
            .get()
            .await
            .map_err(|e| QuotaError(format!("Redis pool: {e}")))
    }
}

#[async_trait]
impl ObserverDeviceStore for RedisObserverDevices {
    async fn acquire_device(
        &self,
        lease: &DeviceLease,
        now_ms: u64,
        cap: u64,
        per_device: u64,
    ) -> Result<DeviceDecision, QuotaError> {
        let mut conn = self.conn().await?;
        let (status, count): (i64, i64) = self
            .acquire
            .key(lease.devices_key())
            .key(lease.subs_key())
            .arg(now_ms)
            .arg(&lease.device)
            .arg(&lease.member)
            .arg(cap)
            .arg(per_device)
            .arg(DEVICE_STALE_MS)
            .invoke_async(&mut *conn)
            .await
            .map_err(|e| QuotaError(format!("Redis observer device script: {e}")))?;
        let count = u64::try_from(count).unwrap_or(0);
        match status {
            0 => Ok(DeviceDecision::Allowed),
            1 => Ok(DeviceDecision::DeviceCap { devices: count }),
            2 => Ok(DeviceDecision::SubsPerDevice { subs: count }),
            other => Err(QuotaError(format!("unexpected device script status {other}"))),
        }
    }

    async fn release_device(&self, lease: &DeviceLease) -> Result<(), QuotaError> {
        let mut conn = self.conn().await?;
        let _: i64 = self
            .release
            .key(lease.devices_key())
            .key(lease.subs_key())
            .arg(&lease.device)
            .arg(&lease.member)
            .invoke_async(&mut *conn)
            .await
            .map_err(|e| QuotaError(format!("Redis observer device release: {e}")))?;
        Ok(())
    }

    async fn refresh_device(&self, lease: &DeviceLease, now_ms: u64) -> Result<(), QuotaError> {
        let mut conn = self.conn().await?;
        let _: i64 = self
            .refresh
            .key(lease.devices_key())
            .key(lease.subs_key())
            .arg(now_ms)
            .arg(&lease.device)
            .arg(&lease.member)
            .arg(DEVICE_STALE_MS)
            .invoke_async(&mut *conn)
            .await
            .map_err(|e| QuotaError(format!("Redis observer device refresh: {e}")))?;
        Ok(())
    }
}

/// In-memory store with the Redis scripts' semantics, for tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryObserverDevices {
    /// key → member → last seen ms
    sets: Mutex<HashMap<String, HashMap<String, u64>>>,
}

#[cfg(test)]
#[async_trait]
impl ObserverDeviceStore for MemoryObserverDevices {
    async fn acquire_device(
        &self,
        lease: &DeviceLease,
        now_ms: u64,
        cap: u64,
        per_device: u64,
    ) -> Result<DeviceDecision, QuotaError> {
        let mut sets = self.sets.lock().unwrap_or_else(PoisonError::into_inner);
        let floor = now_ms.saturating_sub(DEVICE_STALE_MS);
        let (devices_key, subs_key) = (lease.devices_key(), lease.subs_key());
        for key in [&devices_key, &subs_key] {
            sets.entry(key.clone())
                .or_default()
                .retain(|_, seen| *seen > floor);
        }
        let subs = &sets[&subs_key];
        let sub_count = subs.len() as u64;
        let counted = subs.contains_key(&lease.member);
        if sub_count == 0 {
            sets.entry(devices_key.clone())
                .or_default()
                .remove(&lease.device);
        }
        if !counted {
            let devices = &sets[&devices_key];
            if !devices.contains_key(&lease.device) && devices.len() as u64 >= cap {
                return Ok(DeviceDecision::DeviceCap {
                    devices: devices.len() as u64,
                });
            }
            if sub_count >= per_device {
                return Ok(DeviceDecision::SubsPerDevice { subs: sub_count });
            }
        }
        sets.entry(subs_key)
            .or_default()
            .insert(lease.member.clone(), now_ms);
        sets.entry(devices_key)
            .or_default()
            .insert(lease.device.clone(), now_ms);
        Ok(DeviceDecision::Allowed)
    }

    async fn release_device(&self, lease: &DeviceLease) -> Result<(), QuotaError> {
        let mut sets = self.sets.lock().unwrap_or_else(PoisonError::into_inner);
        let subs_key = lease.subs_key();
        let empty = sets.get_mut(&subs_key).is_none_or(|subs| {
            subs.remove(&lease.member);
            subs.is_empty()
        });
        if empty {
            sets.remove(&subs_key);
            if let Some(devices) = sets.get_mut(&lease.devices_key()) {
                devices.remove(&lease.device);
            }
        }
        Ok(())
    }

    async fn refresh_device(&self, lease: &DeviceLease, now_ms: u64) -> Result<(), QuotaError> {
        let mut sets = self.sets.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(seen) = sets
            .get_mut(&lease.subs_key())
            .and_then(|subs| subs.get_mut(&lease.member))
        else {
            return Ok(());
        };
        *seen = now_ms;
        sets.entry(lease.devices_key())
            .or_default()
            .insert(lease.device.clone(), now_ms);
        Ok(())
    }
}

/// This node's counted observer subscriptions and AUTH device tags.
#[derive(Debug, Default)]
pub struct DeviceLeases {
    /// conn_id → validated `device` tag of its NIP-42 AUTH event.
    auth_devices: DashMap<Uuid, String>,
    /// conn_id → sub_id → lease.
    leases: DashMap<Uuid, HashMap<String, DeviceLease>>,
}

impl DeviceLeases {
    /// Remember the `device` tag a connection authenticated with.
    pub(crate) fn set_auth_device(&self, conn_id: Uuid, device: String) {
        self.auth_devices.insert(conn_id, device);
    }

    /// Forget a connection's `device` tag after its auth commit failed.
    pub(crate) fn forget_auth_device(&self, conn_id: Uuid) {
        self.auth_devices.remove(&conn_id);
    }

    fn auth_device(&self, conn_id: Uuid) -> Option<String> {
        self.auth_devices.get(&conn_id).map(|d| d.clone())
    }

    /// Record `lease` (or none) for `sub_id`, returning the displaced lease
    /// when it differs and must be released.
    fn swap(
        &self,
        conn_id: Uuid,
        sub_id: &str,
        lease: Option<DeviceLease>,
    ) -> Option<DeviceLease> {
        let previous = match lease.clone() {
            Some(lease) => self
                .leases
                .entry(conn_id)
                .or_default()
                .insert(sub_id.to_owned(), lease),
            None => self.take(conn_id, sub_id),
        };
        previous.filter(|previous| Some(previous) != lease.as_ref())
    }

    fn take(&self, conn_id: Uuid, sub_id: &str) -> Option<DeviceLease> {
        let mut subs = self.leases.get_mut(&conn_id)?;
        let lease = subs.remove(sub_id);
        let empty = subs.is_empty();
        drop(subs);
        if empty {
            self.leases.remove_if(&conn_id, |_, subs| subs.is_empty());
        }
        lease
    }

    fn take_connection(&self, conn_id: Uuid) -> Vec<DeviceLease> {
        self.auth_devices.remove(&conn_id);
        self.leases
            .remove(&conn_id)
            .map(|(_, subs)| subs.into_values().collect())
            .unwrap_or_default()
    }

    fn snapshot(&self) -> Vec<DeviceLease> {
        self.leases
            .iter()
            .flat_map(|entry| entry.value().values().cloned().collect::<Vec<_>>())
            .collect()
    }

    /// Counted subscriptions on this node.
    pub fn len(&self) -> usize {
        self.leases.iter().map(|entry| entry.value().len()).sum()
    }

    /// Whether this node counts no subscription.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The validated `["device", "<id>"]` tag of an AUTH event: exactly one such
/// tag whose value is 1–64 characters of `[A-Za-z0-9_-]`. Anything else is
/// ignored, so the connection counts as its own device; a malformed optional
/// tag never fails authentication.
pub fn auth_device_tag(event: &Event) -> Option<String> {
    let mut tags = event
        .tags
        .iter()
        .filter(|tag| tag.as_slice().first().map(String::as_str) == Some(DEVICE_TAG));
    let tag = tags.next()?;
    if tags.next().is_some() {
        return None;
    }
    let value = tag.as_slice().get(1)?;
    let valid = (1..=DEVICE_TAG_MAX_LEN).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    valid.then(|| value.clone())
}

/// The namespaced device of a connection: the token session's device, else
/// its AUTH `device` tag, else the connection.
pub fn device_id(ctx: &AuthContext, auth_tag: Option<&str>, conn_id: Uuid) -> String {
    if let Some(device) = ctx.token.as_ref().and_then(|token| token.device_id) {
        return format!("tok:{device}");
    }
    match auth_tag {
        Some(tag) => format!("tag:{tag}"),
        None => format!("conn:{conn_id}"),
    }
}

/// Whether a REQ is an owner observer subscription: some filter names kind
/// 24200 with `#p` = the authenticated pubkey.
pub fn is_owner_observer_req(filters: &[Filter], authed: &PublicKey) -> bool {
    let p = nostr::SingleLetterTag::lowercase(nostr::Alphabet::P);
    let authed = authed.to_hex();
    filters.iter().any(|filter| {
        filter.kinds.as_ref().is_some_and(|kinds| {
            kinds
                .iter()
                .any(|kind| u32::from(kind.as_u16()) == KIND_AGENT_OBSERVER_FRAME)
        }) && filter
            .generic_tags
            .get(&p)
            .is_some_and(|values| values.contains(&authed))
    })
}

/// Device admission of one live REQ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObserverAdmission {
    /// Not an owner observer subscription, or not countable right now.
    Uncounted,
    /// Counted; the caller records the lease when it registers the REQ.
    Counted(DeviceLease),
    /// Refused with this `CLOSED` reason.
    Rejected(String),
}

/// Count an owner observer REQ against the owner's receiving-device cap.
/// Agent connections are never counted. Store or tier failures admit the REQ
/// uncounted.
pub(crate) async fn admit_subscription(
    state: &AppState,
    conn: &ConnectionState,
    sub_id: &str,
    filters: &[Filter],
) -> ObserverAdmission {
    let AuthState::Authenticated(ctx) = conn.auth_state_snapshot() else {
        return ObserverAdmission::Uncounted;
    };
    let is_agent = ctx.agent_owner_pubkey.is_some()
        || ctx
            .token
            .as_ref()
            .is_some_and(|token| token.bot_id.is_some());
    if is_agent || !is_owner_observer_req(filters, &ctx.pubkey) {
        return ObserverAdmission::Uncounted;
    }
    let community = conn.tenant.community();
    let tier = match resolve_tier(state, community, &ctx.pubkey).await {
        Ok(tier) => tier,
        Err(e) => {
            warn!(error = %e, "observer tier lookup failed; admitting observer subscription uncounted");
            return ObserverAdmission::Uncounted;
        }
    };
    let cap = state.config.observer_quota.receiving_devices.get(tier);
    let auth_tag = state.observer_device_leases.auth_device(conn.conn_id);
    let lease = DeviceLease {
        community,
        owner: ctx.pubkey,
        device: device_id(&ctx, auth_tag.as_deref(), conn.conn_id),
        member: format!("{}:{sub_id}", conn.conn_id),
    };
    match state
        .observer_devices
        .acquire_device(&lease, now_ms(), cap, MAX_SUBS_PER_DEVICE)
        .await
    {
        Ok(DeviceDecision::Allowed) => ObserverAdmission::Counted(lease),
        Ok(DeviceDecision::DeviceCap { .. }) => {
            reject_metric("receiving_devices");
            ObserverAdmission::Rejected(format!(
                "restricted: observer device limit reached (tier {tier}: {cap} devices)"
            ))
        }
        Ok(DeviceDecision::SubsPerDevice { .. }) => {
            reject_metric("subs_per_device");
            ObserverAdmission::Rejected(format!(
                "restricted: observer subscription limit reached \
                 ({MAX_SUBS_PER_DEVICE} per device)"
            ))
        }
        Err(e) => {
            warn!(error = %e, "observer device store unavailable; admitting observer subscription uncounted");
            ObserverAdmission::Uncounted
        }
    }
}

async fn release(state: &AppState, lease: &DeviceLease) {
    if let Err(e) = state.observer_devices.release_device(lease).await {
        // The stale prune frees the slot within DEVICE_STALE_MS: this node
        // stops refreshing a lease it no longer holds.
        warn!(error = %e, "observer device release failed; slot frees when stale");
    }
}

/// Record the outcome of a registered REQ for `sub_id`: its new lease, or none
/// for an uncounted REQ. Releases whatever lease the sub_id held before. The
/// caller holds the connection's lifecycle lock.
pub(crate) async fn record_claim(
    state: &AppState,
    conn_id: Uuid,
    sub_id: &str,
    lease: Option<DeviceLease>,
) {
    if let Some(displaced) = state.observer_device_leases.swap(conn_id, sub_id, lease) {
        release(state, &displaced).await;
    }
}

/// Release a lease acquired for a REQ that then failed to register.
pub(crate) async fn release_unclaimed(state: &AppState, lease: &DeviceLease) {
    release(state, lease).await;
}

/// Release the lease of a retired subscription, if it had one. The caller
/// holds the connection's lifecycle lock.
pub(crate) async fn release_subscription(state: &AppState, conn_id: Uuid, sub_id: &str) {
    if let Some(lease) = state.observer_device_leases.take(conn_id, sub_id) {
        release(state, &lease).await;
    }
}

/// Release every lease of a closed connection and forget its AUTH device.
pub(crate) async fn release_connection(state: &AppState, conn_id: Uuid) {
    for lease in state.observer_device_leases.take_connection(conn_id) {
        release(state, &lease).await;
    }
}

/// Refresh every lease this node holds once.
pub async fn refresh_live_leases(state: &AppState) {
    let now = now_ms();
    for lease in state.observer_device_leases.snapshot() {
        if let Err(e) = state.observer_devices.refresh_device(&lease, now).await {
            warn!(error = %e, "observer device refresh failed; retrying next interval");
            return;
        }
    }
}

/// Refresh this node's live leases every [`REFRESH_INTERVAL`], forever.
pub async fn run_refresh(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(REFRESH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        interval.tick().await;
        refresh_live_leases(&state).await;
    }
}

#[cfg(test)]
mod tests;
