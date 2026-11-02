//! Generations that fence membership-cache refills against invalidations.
//!
//! A cache miss reads the database and then inserts the answer. Without a
//! fence, an invalidation that lands between that read and the insert is
//! lost: the stale answer is cached for the full TTL (a kicked user still
//! "member", or a joined user "not member", for 10 s).
//!
//! Two counters per key, both kept in fixed hashed slot arrays (bounded memory,
//! no expiry; a collision only makes a fence or a compare more conservative):
//!
//! - the **refill fence**, bumped by every local drop of the key, including a
//!   drop received from another pod or a community-wide channel-deleted drop;
//! - the **origin generation**, bumped once per membership change on every
//!   pod: by [`AppState::invalidate_membership`] on the pod where the change
//!   happened, and on receipt of its Redis drop on every other pod. The drop
//!   names its origin pod ([`MembershipCacheGen::pod_id`]), so the origin
//!   pod's own loopback is not counted again. The user stream's membership
//!   re-checks compare it before applying a result.
//!
//! [`AppState::invalidate_membership`]: crate::state::AppState::invalidate_membership

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use kuzz_core::tenant::CommunityId;
use uuid::Uuid;

const SLOTS: usize = 4096;

/// Snapshot taken before a membership-cache refill reads the database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RefillToken {
    slot: u64,
    wide: u64,
}

/// Per-key membership generations; see the module docs.
pub struct MembershipCacheGen {
    refill: Box<[AtomicU64]>,
    origin: Box<[AtomicU64]>,
    wide: AtomicU64,
    pod_id: Uuid,
}

impl Default for MembershipCacheGen {
    fn default() -> Self {
        Self::new()
    }
}

impl MembershipCacheGen {
    /// Empty generations (every key at 0).
    pub fn new() -> Self {
        let slots = || (0..SLOTS).map(|_| AtomicU64::new(0)).collect();
        Self {
            refill: slots(),
            origin: slots(),
            wide: AtomicU64::new(0),
            pod_id: Uuid::new_v4(),
        }
    }

    /// This process's id in the membership drops it publishes.
    pub fn pod_id(&self) -> Uuid {
        self.pod_id
    }

    /// Count a membership drop received over Redis. `origin` is the pod that
    /// made the change: its own loopback was counted at the origin already.
    pub fn bump_origin_received(
        &self,
        origin: Option<Uuid>,
        community: CommunityId,
        channel_id: Uuid,
        pubkey: &[u8],
    ) {
        if origin != Some(self.pod_id) {
            let _ = self.bump_origin(community, channel_id, pubkey);
        }
    }

    fn slot(community: CommunityId, channel_id: Uuid, pubkey: &[u8]) -> usize {
        let mut hasher = DefaultHasher::new();
        community.as_uuid().hash(&mut hasher);
        channel_id.hash(&mut hasher);
        pubkey.hash(&mut hasher);
        (hasher.finish() as usize) % SLOTS
    }

    /// Take before reading the database for a cache refill.
    pub fn refill_token(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        pubkey: &[u8],
    ) -> RefillToken {
        let slot = Self::slot(community, channel_id, pubkey);
        RefillToken {
            wide: self.wide.load(Ordering::SeqCst),
            slot: self.refill[slot].load(Ordering::SeqCst),
        }
    }

    /// Whether no drop of the key happened since `token` was taken. Check it
    /// **after** inserting the refilled value and drop the value when false:
    /// every invalidation bumps first and drops second, so either it removes
    /// the inserted value or this check sees its bump.
    pub fn refill_still_current(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        pubkey: &[u8],
        token: RefillToken,
    ) -> bool {
        self.refill_token(community, channel_id, pubkey) == token
    }

    /// Record a local drop of one key. Call before dropping the cache entry.
    pub fn bump_refill(&self, community: CommunityId, channel_id: Uuid, pubkey: &[u8]) {
        self.refill[Self::slot(community, channel_id, pubkey)].fetch_add(1, Ordering::SeqCst);
    }

    /// Record a drop of many keys at once (channel deleted). Call before the drop.
    pub fn bump_refill_all(&self) {
        self.wide.fetch_add(1, Ordering::SeqCst);
    }

    /// Record a membership change at its origin. Call before dropping the
    /// entry. Returns the key's generation right after this change, so the
    /// caller can apply its own result only while no later change happened.
    pub fn bump_origin(&self, community: CommunityId, channel_id: Uuid, pubkey: &[u8]) -> u64 {
        self.origin[Self::slot(community, channel_id, pubkey)].fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Current origin generation of a key, for compare-and-apply re-checks.
    pub fn origin_generation(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        pubkey: &[u8],
    ) -> u64 {
        self.origin[Self::slot(community, channel_id, pubkey)].load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> (CommunityId, Uuid, Vec<u8>) {
        (
            CommunityId::from_uuid(Uuid::new_v4()),
            Uuid::new_v4(),
            vec![7u8; 32],
        )
    }

    #[test]
    fn refill_token_changes_on_key_and_wide_bumps_only() {
        let gen = MembershipCacheGen::new();
        let (c, ch, pk) = key();
        let token = gen.refill_token(c, ch, &pk);
        assert!(gen.refill_still_current(c, ch, &pk, token));
        gen.bump_origin(c, ch, &pk);
        assert!(
            gen.refill_still_current(c, ch, &pk, token),
            "the origin generation is separate from the refill fence"
        );
        gen.bump_refill(c, ch, &pk);
        assert!(!gen.refill_still_current(c, ch, &pk, token));
        let token = gen.refill_token(c, ch, &pk);
        gen.bump_refill_all();
        assert!(!gen.refill_still_current(c, ch, &pk, token));
    }

    #[test]
    fn origin_generation_counts_origin_bumps_only() {
        let gen = MembershipCacheGen::new();
        let (c, ch, pk) = key();
        assert_eq!(gen.origin_generation(c, ch, &pk), 0);
        gen.bump_refill(c, ch, &pk);
        gen.bump_refill_all();
        assert_eq!(gen.origin_generation(c, ch, &pk), 0);
        gen.bump_origin(c, ch, &pk);
        assert_eq!(gen.origin_generation(c, ch, &pk), 1);
    }

    #[test]
    fn a_received_drop_counts_once_except_the_own_loopback() {
        let origin_pod = MembershipCacheGen::new();
        let other_pod = MembershipCacheGen::new();
        let (c, ch, pk) = key();
        origin_pod.bump_origin(c, ch, &pk);
        let from = Some(origin_pod.pod_id());
        origin_pod.bump_origin_received(from, c, ch, &pk);
        other_pod.bump_origin_received(from, c, ch, &pk);
        assert_eq!(origin_pod.origin_generation(c, ch, &pk), 1);
        assert_eq!(other_pod.origin_generation(c, ch, &pk), 1);
        other_pod.bump_origin_received(None, c, ch, &pk);
        assert_eq!(
            other_pod.origin_generation(c, ch, &pk),
            2,
            "a drop from an older pod without an origin still counts"
        );
    }
}
