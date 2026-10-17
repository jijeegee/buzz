//! Server-side chat state written with each new message (Chats stage S1).
//!
//! Runs inside the message insert transaction, after the event row, its
//! thread metadata, and its channel sequence exist. Only message kinds change
//! state; reactions, edits, deletes, and system events never reach this code.
//! Every statement is a single-row (or one-room bounded) upsert keyed by the
//! message's own channel, so the cost per message does not grow with room
//! size. The channel advisory lock taken by the sequence trigger is already
//! held, so writes for one channel are serialized in sequence order.
//!
//! Follow rows and read positions are private per-user state: never log them.

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use nostr::Event;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::error::Result;
use crate::event::ThreadMetadataParams;

/// Tag on a reply that closes its thread: `["kuzz:thread-closed", <root>]`.
///
/// Written by `kuzz threads close`. The same tag on a top-level message (the
/// task result on the main timeline) does not change thread state.
pub const THREAD_CLOSED_TAG: &str = "kuzz:thread-closed";

/// Tag on a reply that reopens its thread: `["kuzz:thread-reopened", <root>]`.
pub const THREAD_REOPENED_TAG: &str = "kuzz:thread-reopened";

/// True when a reply's tags carry a close or reopen marker. Marker replies
/// are shown as a system line and are not counted as replies (spec 4.2); the
/// rule matches the desktop's `isThreadMarkerReply`.
pub fn is_thread_marker_tags<'a>(tags: impl IntoIterator<Item = &'a [String]>) -> bool {
    tags.into_iter().any(|tag| {
        tag.len() >= 2
            && !tag[1].is_empty()
            && (tag[0] == THREAD_CLOSED_TAG || tag[0] == THREAD_REOPENED_TAG)
    })
}

/// What a chat-state write changed that the caller must announce after commit.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChatStateWriteOutcome {
    /// DM members (raw 32-byte pubkeys) whose hidden DM this message brought
    /// back. The caller republishes their DM visibility snapshot.
    pub unhidden_dm_members: Vec<Vec<u8>>,
    /// The message was a close or reopen marker that changed its thread's
    /// close state. The caller pings the author's other devices.
    pub thread_close_changed: bool,
}

/// A reply's close/reopen marker, when its tag names the reply's own root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThreadMarker {
    Closed,
    Reopened,
}

fn thread_marker(event: &Event, root_hex: &str) -> Option<ThreadMarker> {
    let mut marker = None;
    for tag in event.tags.iter() {
        let parts = tag.as_slice();
        if parts.len() < 2 || !parts[1].eq_ignore_ascii_case(root_hex) {
            continue;
        }
        match parts[0].as_str() {
            // A reopen tag wins over a close tag on the same event: reopening
            // is the safe direction (the thread stays visible).
            THREAD_REOPENED_TAG => return Some(ThreadMarker::Reopened),
            THREAD_CLOSED_TAG => marker = Some(ThreadMarker::Closed),
            _ => {}
        }
    }
    marker
}

/// Valid 32-byte pubkeys named by `p` tags, in tag order, deduplicated.
fn mentioned_pubkeys(tags: impl IntoIterator<Item = Vec<String>>) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = Vec::new();
    for parts in tags {
        if parts.len() < 2 || parts[0] != "p" {
            continue;
        }
        let Ok(bytes) = hex::decode(&parts[1]) else {
            continue;
        };
        if bytes.len() == 32 && !out.contains(&bytes) {
            out.push(bytes);
        }
    }
    out
}

fn event_tag_vecs(event: &Event) -> Vec<Vec<String>> {
    event.tags.iter().map(|t| t.as_slice().to_vec()).collect()
}

/// Apply the chat-state write rules for one newly inserted event.
///
/// No-op for non-message kinds, channel-less events, and events whose channel
/// row does not exist (sequence 0).
pub(crate) async fn apply_message_write(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event: &Event,
    channel_id: Option<Uuid>,
    thread_meta: Option<ThreadMetadataParams<'_>>,
    received_at: DateTime<Utc>,
) -> Result<ChatStateWriteOutcome> {
    let mut outcome = ChatStateWriteOutcome::default();
    let kind = i32::from(event.kind.as_u16());
    if !crate::inbox_summary::MESSAGE_KINDS.contains(&kind) {
        return Ok(outcome);
    }
    let Some(channel_id) = channel_id else {
        return Ok(outcome);
    };
    let created_at_secs = event.created_at.as_secs() as i64;
    let Some(created_at) = DateTime::from_timestamp(created_at_secs, 0) else {
        return Ok(outcome);
    };
    let community = *community_id.as_uuid();
    let event_id = event.id.as_bytes().as_slice();
    let author = event.pubkey.to_bytes();
    let author = author.as_slice();

    let seq: Option<i64> = sqlx::query_scalar(
        "SELECT channel_sequence FROM events \
         WHERE community_id = $1 AND created_at = $2 AND id = $3",
    )
    .bind(community)
    .bind(created_at)
    .bind(event_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(seq) = seq.filter(|s| *s > 0) else {
        return Ok(outcome);
    };

    // A reply is any message with a parent; its thread is keyed by the root
    // (the parent itself for a direct reply). Thread metadata for another
    // channel is never trusted.
    let reply = thread_meta
        .filter(|meta| meta.channel_id == channel_id)
        .and_then(|meta| {
            let parent = meta.parent_event_id?;
            Some((meta.root_event_id.unwrap_or(parent), meta.broadcast))
        });
    let root_hex = reply.map(|(root, _)| hex::encode(root));
    let marker = root_hex
        .as_deref()
        .and_then(|hex| thread_marker(event, hex));

    // Markers are control replies: they never move ordering, unread, read
    // positions, follows, or hidden DMs.
    if marker.is_none() {
        let is_main = match reply {
            None => true,
            Some((_, broadcast)) => broadcast,
        };
        if is_main {
            sqlx::query(
                "UPDATE channels \
                 SET main_last_seq = $3, main_last_at = $4, main_last_event_id = $5 \
                 WHERE community_id = $1 AND id = $2 \
                   AND (main_last_seq IS NULL OR main_last_seq < $3)",
            )
            .bind(community)
            .bind(channel_id)
            .bind(seq)
            .bind(received_at)
            .bind(event_id)
            .execute(&mut **tx)
            .await?;

            // The author has read the room up to their own message. That is
            // the reader's own read: a later first-run upload must not move it.
            sqlx::query(
                "UPDATE channel_members \
                 SET read_seq = GREATEST(COALESCE(read_seq, 0), $4), read_origin = 'user', \
                     unread_mark = FALSE \
                 WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3 \
                   AND removed_at IS NULL",
            )
            .bind(community)
            .bind(channel_id)
            .bind(author)
            .bind(seq)
            .execute(&mut **tx)
            .await?;
        }

        outcome.unhidden_dm_members =
            unhide_dm_for_recipients(tx, community, channel_id, author).await?;
    }

    if let Some((root, _)) = reply {
        let applied = apply_reply(
            tx,
            community,
            channel_id,
            root,
            event,
            author,
            seq,
            received_at,
            marker,
        )
        .await?;
        outcome.thread_close_changed = applied && marker.is_some();
    }

    Ok(outcome)
}

/// Apply a reply's thread-state and follow writes. Returns `false` when the
/// root is not a live event in this channel (nothing was written).
#[allow(clippy::too_many_arguments)]
async fn apply_reply(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel_id: Uuid,
    root: &[u8],
    event: &Event,
    author: &[u8],
    seq: i64,
    received_at: DateTime<Utc>,
    marker: Option<ThreadMarker>,
) -> Result<bool> {
    // The root must be a live event in this same channel. Its author and
    // mentions seed follows.
    let root_row = sqlx::query(
        "SELECT pubkey, tags FROM events \
         WHERE community_id = $1 AND id = $2 AND channel_id = $3 AND deleted_at IS NULL \
         LIMIT 1",
    )
    .bind(community)
    .bind(root)
    .bind(channel_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(root_row) = root_row else {
        return Ok(false);
    };

    match marker {
        Some(ThreadMarker::Closed) => {
            sqlx::query(
                "INSERT INTO thread_state (community_id, root_event_id, channel_id, closed_at, closed_by) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (community_id, root_event_id) DO UPDATE \
                 SET closed_at = EXCLUDED.closed_at, closed_by = EXCLUDED.closed_by",
            )
            .bind(community)
            .bind(root)
            .bind(channel_id)
            .bind(received_at)
            .bind(author)
            .execute(&mut **tx)
            .await?;
            return Ok(true);
        }
        Some(ThreadMarker::Reopened) => {
            sqlx::query(
                "INSERT INTO thread_state (community_id, root_event_id, channel_id) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (community_id, root_event_id) DO UPDATE \
                 SET closed_at = NULL, closed_by = NULL",
            )
            .bind(community)
            .bind(root)
            .bind(channel_id)
            .execute(&mut **tx)
            .await?;
            return Ok(true);
        }
        None => {}
    }

    // Any other reply moves the thread forward and reopens it.
    sqlx::query(
        "INSERT INTO thread_state (community_id, root_event_id, channel_id, last_seq, last_at) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (community_id, root_event_id) DO UPDATE \
         SET last_seq = GREATEST(COALESCE(thread_state.last_seq, 0), EXCLUDED.last_seq), \
             last_at = GREATEST(thread_state.last_at, EXCLUDED.last_at), \
             closed_at = NULL, closed_by = NULL",
    )
    .bind(community)
    .bind(root)
    .bind(channel_id)
    .bind(seq)
    .bind(received_at)
    .execute(&mut **tx)
    .await?;

    // Reply author: follows (again, if they had unfollowed) and has read the
    // thread up to this reply. Members only. Replying is the reader's own
    // act, so a backfilled row stops being backfill: a later first-run upload
    // changes neither its follow state nor its read position.
    sqlx::query(
        "INSERT INTO thread_follows \
             (community_id, root_event_id, pubkey, channel_id, follow_state, follow_origin, \
              read_seq, read_origin, unread_mark, updated_at) \
         SELECT cm.community_id, $2, cm.pubkey, cm.channel_id, 'auto', 'live', $5, 'user', \
                FALSE, $6 \
           FROM channel_members cm \
          WHERE cm.community_id = $1 AND cm.channel_id = $3 AND cm.pubkey = $4 \
            AND cm.removed_at IS NULL \
         ON CONFLICT (community_id, root_event_id, pubkey) DO UPDATE \
         SET follow_state = CASE WHEN thread_follows.follow_state = 'unfollowed' \
                                 THEN 'followed' ELSE thread_follows.follow_state END, \
             follow_origin = CASE WHEN thread_follows.follow_state = 'unfollowed' \
                                       OR thread_follows.follow_origin = 'backfill' \
                                  THEN 'live' ELSE thread_follows.follow_origin END, \
             read_seq = GREATEST(COALESCE(thread_follows.read_seq, 0), EXCLUDED.read_seq), \
             read_origin = 'user', \
             unread_mark = FALSE, \
             updated_at = EXCLUDED.updated_at",
    )
    .bind(community)
    .bind(root)
    .bind(channel_id)
    .bind(author)
    .bind(seq)
    .bind(received_at)
    .execute(&mut **tx)
    .await?;

    // People mentioned in the reply, the root author, and people mentioned in
    // the root: auto-follow when they have no row. Seeding on every reply (not
    // only the first) keeps this right when a close marker was the first reply.
    // They have read everything before this reply, so an old thread does not
    // show its whole history as unread. An existing row (including
    // `unfollowed`) is left as it is.
    let mut candidates = mentioned_pubkeys(event_tag_vecs(event));
    let root_author: Vec<u8> = root_row.try_get("pubkey")?;
    let root_tags: serde_json::Value = root_row.try_get("tags")?;
    let root_tags: Vec<Vec<String>> = serde_json::from_value(root_tags).unwrap_or_default();
    for pubkey in std::iter::once(root_author).chain(mentioned_pubkeys(root_tags)) {
        if pubkey.len() == 32 && !candidates.contains(&pubkey) {
            candidates.push(pubkey);
        }
    }
    candidates.retain(|pubkey| pubkey.as_slice() != author);
    if candidates.is_empty() {
        return Ok(true);
    }
    sqlx::query(
        "INSERT INTO thread_follows \
             (community_id, root_event_id, pubkey, channel_id, follow_state, follow_origin, \
              read_seq, updated_at) \
         SELECT cm.community_id, $2, cm.pubkey, cm.channel_id, 'auto', 'live', $6, $5 \
           FROM channel_members cm \
          WHERE cm.community_id = $1 AND cm.channel_id = $3 AND cm.pubkey = ANY($4) \
            AND cm.removed_at IS NULL \
         ON CONFLICT (community_id, root_event_id, pubkey) DO NOTHING",
    )
    .bind(community)
    .bind(root)
    .bind(channel_id)
    .bind(&candidates)
    .bind(received_at)
    .bind(seq - 1)
    .execute(&mut **tx)
    .await?;
    Ok(true)
}

/// A new message in a DM brings it back for every other member who hid it,
/// except members who blocked the author (silent block).
async fn unhide_dm_for_recipients(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel_id: Uuid,
    author: &[u8],
) -> Result<Vec<Vec<u8>>> {
    let rows = sqlx::query(
        "UPDATE channel_members cm SET hidden_at = NULL \
           FROM channels c \
          WHERE c.community_id = cm.community_id AND c.id = cm.channel_id \
            AND c.channel_type = 'dm' \
            AND cm.community_id = $1 AND cm.channel_id = $2 AND cm.pubkey <> $3 \
            AND cm.removed_at IS NULL AND cm.hidden_at IS NOT NULL \
            AND NOT EXISTS (SELECT 1 FROM user_blocks ub \
                             WHERE ub.community_id = cm.community_id \
                               AND ub.blocker = cm.pubkey AND ub.blocked = $3) \
         RETURNING cm.pubkey",
    )
    .bind(community)
    .bind(channel_id)
    .bind(author)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| row.try_get::<Vec<u8>, _>("pubkey").map_err(Into::into))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn reply_with(tags: &[[&str; 2]]) -> Event {
        EventBuilder::new(Kind::Custom(9), "x")
            .tags(tags.iter().map(|t| Tag::parse(*t).expect("tag")))
            .sign_with_keys(&Keys::generate())
            .expect("sign")
    }

    #[test]
    fn marker_requires_own_root() {
        let root = "a".repeat(64);
        let other = "b".repeat(64);
        assert_eq!(
            thread_marker(&reply_with(&[[THREAD_CLOSED_TAG, &root]]), &root),
            Some(ThreadMarker::Closed)
        );
        assert_eq!(
            thread_marker(&reply_with(&[[THREAD_CLOSED_TAG, &other]]), &root),
            None
        );
        assert_eq!(
            thread_marker(&reply_with(&[[THREAD_REOPENED_TAG, &root]]), &root),
            Some(ThreadMarker::Reopened)
        );
        assert_eq!(
            thread_marker(
                &reply_with(&[[THREAD_CLOSED_TAG, &root], [THREAD_REOPENED_TAG, &root]]),
                &root
            ),
            Some(ThreadMarker::Reopened)
        );
    }

    #[test]
    fn mentions_skip_malformed_and_duplicates() {
        let a = "1".repeat(64);
        let tags = vec![
            vec!["p".to_string(), a.clone()],
            vec!["p".to_string(), a.clone()],
            vec!["p".to_string(), "zz".to_string()],
            vec!["e".to_string(), "2".repeat(64)],
        ];
        assert_eq!(mentioned_pubkeys(tags), vec![hex::decode(a).expect("hex")]);
    }
}
