//! Silent blocks on the relay side: cached block lists and the helpers every
//! read and delivery path uses to hide a blocked author's DM events from the
//! person who blocked them.
//!
//! The blocked person is never told: their writes are accepted as usual and
//! only the blocker's reads, live delivery, unread counts and pushes skip
//! them. Shared-channel events are delivered as usual (clients fold them).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use kuzz_core::{CommunityId, TenantContext};
use kuzz_db::DbError;
use kuzz_pubsub::cache_invalidation::CacheInvalidation;
use uuid::Uuid;

use crate::state::AppState;

/// A set of 32-byte pubkeys.
pub type PubkeySet = Arc<HashSet<[u8; 32]>>;

/// How long a cached block list may serve before a fresh read. Block and
/// unblock drop both sides' entries locally and on every pod, so this only
/// bounds a missed cross-pod publish.
const BLOCK_CACHE_TTL: Duration = Duration::from_secs(30);

/// Channel type never changes after creation, so DM-ness caches long.
const DM_CACHE_TTL: Duration = Duration::from_secs(600);

/// Per-pod caches for block lists and DM-ness of channels.
pub struct BlockCaches {
    /// `(community, author)` → everyone who blocked `author`.
    blockers: moka::sync::Cache<(CommunityId, [u8; 32]), PubkeySet>,
    /// `(community, reader)` → everyone `reader` blocked.
    blocked: moka::sync::Cache<(CommunityId, [u8; 32]), PubkeySet>,
    /// `(community, channel)` → whether the channel is a DM.
    dm: moka::sync::Cache<(CommunityId, Uuid), bool>,
}

impl BlockCaches {
    /// Empty caches.
    pub fn new() -> Self {
        let block_cache = || {
            moka::sync::Cache::builder()
                .max_capacity(20_000)
                .time_to_live(BLOCK_CACHE_TTL)
                .build()
        };
        Self {
            blockers: block_cache(),
            blocked: block_cache(),
            dm: moka::sync::Cache::builder()
                .max_capacity(50_000)
                .time_to_live(DM_CACHE_TTL)
                .build(),
        }
    }

    /// Test seam: record whether `channel` is a DM without a database.
    #[cfg(test)]
    pub(crate) fn seed_dm(&self, community: CommunityId, channel: Uuid, is_dm: bool) {
        self.dm.insert((community, channel), is_dm);
    }

    /// Test seam: record who blocked `author` without a database.
    #[cfg(test)]
    pub(crate) fn seed_blockers(
        &self,
        community: CommunityId,
        author: [u8; 32],
        blockers: impl IntoIterator<Item = [u8; 32]>,
    ) {
        self.blockers.insert(
            (community, author),
            Arc::new(blockers.into_iter().collect()),
        );
    }

    /// Test seam: record whom `reader` blocked without a database.
    #[cfg(test)]
    pub(crate) fn seed_blocked(
        &self,
        community: CommunityId,
        reader: [u8; 32],
        blocked: impl IntoIterator<Item = [u8; 32]>,
    ) {
        self.blocked
            .insert((community, reader), Arc::new(blocked.into_iter().collect()));
    }
}

impl Default for BlockCaches {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for BlockCaches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockCaches").finish_non_exhaustive()
    }
}

fn key32(bytes: &[u8]) -> Option<[u8; 32]> {
    bytes.try_into().ok()
}

fn to_set(rows: Vec<Vec<u8>>) -> PubkeySet {
    Arc::new(rows.iter().filter_map(|row| key32(row)).collect())
}

impl AppState {
    /// Everyone who blocked `author` (cached).
    pub async fn blockers_of_cached(
        &self,
        community: CommunityId,
        author: &[u8],
    ) -> Result<PubkeySet, DbError> {
        let Some(key) = key32(author) else {
            return Ok(PubkeySet::default());
        };
        if let Some(hit) = self.blocks.blockers.get(&(community, key)) {
            return Ok(hit);
        }
        let set = to_set(self.db.blockers_of(community, author).await?);
        self.blocks
            .blockers
            .insert((community, key), Arc::clone(&set));
        Ok(set)
    }

    /// Everyone `reader` blocked (cached).
    pub async fn blocked_by_cached(
        &self,
        community: CommunityId,
        reader: &[u8],
    ) -> Result<PubkeySet, DbError> {
        let Some(key) = key32(reader) else {
            return Ok(PubkeySet::default());
        };
        if let Some(hit) = self.blocks.blocked.get(&(community, key)) {
            return Ok(hit);
        }
        let set = to_set(self.db.blocked_by(community, reader).await?);
        self.blocks
            .blocked
            .insert((community, key), Arc::clone(&set));
        Ok(set)
    }

    /// Which of `channels` are DMs (cached per channel).
    pub async fn dm_channels_cached(
        &self,
        community: CommunityId,
        channels: impl IntoIterator<Item = Uuid>,
    ) -> Result<HashSet<Uuid>, DbError> {
        let mut dms = HashSet::new();
        let mut unknown = Vec::new();
        for channel in channels.into_iter().collect::<HashSet<_>>() {
            match self.blocks.dm.get(&(community, channel)) {
                Some(true) => {
                    dms.insert(channel);
                }
                Some(false) => {}
                None => unknown.push(channel),
            }
        }
        for chunk in unknown.chunks(kuzz_db::blocks::MAX_DM_CHANNEL_LOOKUP) {
            let found: HashSet<Uuid> = self
                .db
                .dm_channel_ids(community, chunk)
                .await?
                .into_iter()
                .collect();
            for channel in chunk {
                let is_dm = found.contains(channel);
                self.blocks.dm.insert((community, *channel), is_dm);
                if is_dm {
                    dms.insert(*channel);
                }
            }
        }
        Ok(dms)
    }

    /// The value for [`kuzz_db::EventQuery::blocked_dm_reader`]: `Some(reader)`
    /// when the reader blocked anyone (or the lookup failed, so SQL decides),
    /// `None` when the reader has no blocks and the clause can be skipped.
    pub async fn blocked_dm_reader(
        &self,
        community: CommunityId,
        reader: &[u8],
    ) -> Option<Vec<u8>> {
        match self.blocked_by_cached(community, reader).await {
            Ok(blocked) if blocked.is_empty() => None,
            Ok(_) => Some(reader.to_vec()),
            Err(e) => {
                tracing::warn!("block list lookup failed; filtering in SQL: {e}");
                Some(reader.to_vec())
            }
        }
    }

    /// Drop both sides' cached block lists here and on every other pod.
    pub fn invalidate_block(&self, tenant: &TenantContext, blocker: &[u8], blocked: &[u8]) {
        self.invalidate_block_local(tenant.community(), blocker, blocked);
        self.spawn_cache_invalidation(
            tenant,
            CacheInvalidation::Block {
                blocker: blocker.to_vec(),
                blocked: blocked.to_vec(),
            },
        );
    }

    /// Local-only block-list drop (the cross-pod consumer calls this).
    pub(crate) fn invalidate_block_local(
        &self,
        community: CommunityId,
        blocker: &[u8],
        blocked: &[u8],
    ) {
        if let Some(key) = key32(blocker) {
            self.blocks.blocked.invalidate(&(community, key));
        }
        if let Some(key) = key32(blocked) {
            self.blocks.blockers.invalidate(&(community, key));
        }
    }

    /// Keep only items `reader` may see: drop DM events whose author `reader`
    /// blocked. `key` names an item's author and channel (`None` for items
    /// that are not channel events, which are always kept).
    pub async fn retain_unblocked_for_reader<T>(
        &self,
        community: CommunityId,
        reader: &[u8],
        items: &mut Vec<T>,
        key: impl Fn(&T) -> Option<([u8; 32], Uuid)>,
    ) -> Result<(), DbError> {
        if items.is_empty() {
            return Ok(());
        }
        let blocked = self.blocked_by_cached(community, reader).await?;
        if blocked.is_empty() {
            return Ok(());
        }
        let candidates: HashSet<Uuid> = items
            .iter()
            .filter_map(&key)
            .filter(|(author, _)| blocked.contains(author))
            .map(|(_, channel)| channel)
            .collect();
        if candidates.is_empty() {
            return Ok(());
        }
        let dms = self.dm_channels_cached(community, candidates).await?;
        items.retain(|item| {
            !key(item).is_some_and(|(author, channel)| {
                blocked.contains(&author) && dms.contains(&channel)
            })
        });
        Ok(())
    }

    /// Thread summaries of one channel as `reader` sees them: in a DM, replies
    /// and participants from people `reader` blocked are taken out, so a
    /// summary never promises replies the thread view hides. `summaries`
    /// pairs each summarized event id with its summary.
    pub async fn hide_blocked_in_thread_summaries(
        &self,
        community: CommunityId,
        reader: &[u8],
        channel: Uuid,
        summaries: Vec<(Vec<u8>, &mut kuzz_db::thread::ThreadSummary)>,
    ) -> Result<(), DbError> {
        if summaries.is_empty() {
            return Ok(());
        }
        let blocked = self.blocked_by_cached(community, reader).await?;
        if blocked.is_empty()
            || !self
                .dm_channels_cached(community, [channel])
                .await?
                .contains(&channel)
        {
            return Ok(());
        }
        let targets: Vec<Vec<u8>> = summaries.iter().map(|(id, _)| id.clone()).collect();
        let blocked_list: Vec<Vec<u8>> = blocked.iter().map(|k| k.to_vec()).collect();
        let hidden = self
            .db
            .blocked_thread_replies(community, &targets, &blocked_list)
            .await?;
        for (id, summary) in summaries {
            let hidden_here = hidden.iter().find(|h| h.target == id);
            summary.without_blocked(hidden_here, |p| {
                key32(p).is_some_and(|key| blocked.contains(&key))
            });
        }
        Ok(())
    }
}

/// Author and `h`-tag channel of an event rendered as JSON (bridge rows).
pub fn json_event_author_channel(value: &serde_json::Value) -> Option<([u8; 32], Uuid)> {
    let author = hex::decode(value.get("pubkey")?.as_str()?).ok()?;
    let author = key32(&author)?;
    let channel = value
        .get("tags")?
        .as_array()?
        .iter()
        .filter_map(|tag| tag.as_array())
        .find(|tag| tag.first().and_then(|v| v.as_str()) == Some("h"))
        .and_then(|tag| tag.get(1)?.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())?;
    Some((author, channel))
}

/// Author and stored channel of a stored event.
pub fn stored_event_author_channel(
    event: &kuzz_core::event::StoredEvent,
) -> Option<([u8; 32], Uuid)> {
    Some((event.event.pubkey.to_bytes(), event.channel_id?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_rows_resolve_author_and_h_channel() {
        let channel = Uuid::new_v4();
        let author = [7u8; 32];
        let value = serde_json::json!({
            "pubkey": hex::encode(author),
            "tags": [["p", "x"], ["h", channel.to_string()]],
        });
        assert_eq!(json_event_author_channel(&value), Some((author, channel)));
        let global = serde_json::json!({"pubkey": hex::encode(author), "tags": []});
        assert_eq!(json_event_author_channel(&global), None);
    }
}
