//! Reader-driven chat state changes (Chats stage S2): read positions, unread
//! marks, and explicit follow / unfollow.
//!
//! Each request is one transaction: every item applies or none does. Read
//! positions only move forward (`max(current, new)`), are clamped to the
//! channel's accepted sequence, and are stored with `read_origin = 'user'`.
//! Only current channel members may change state for a channel or its threads.
//!
//! Read positions and follows are private per-user state: never log them.

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::inbox_summary::MESSAGE_KINDS;
use crate::{observability, Db, Result};

/// Most items one `mark_read` request may carry.
pub const MAX_MARK_READ_ITEMS: usize = 100;

/// Where the reader has read up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadPosition {
    /// A channel sequence the caller got from the chats list.
    Seq(i64),
    /// A message the reader has seen (32-byte id); its channel sequence is used.
    Event(Vec<u8>),
    /// A time the reader has read up to (whole seconds, as clients store it),
    /// compared with each message's `created_at` (the signer's time, not the
    /// relay's receive time): the highest sequence of a live message created
    /// at or before it, among the room's messages or the thread's root and
    /// replies. Nothing that old means sequence 0.
    At(DateTime<Utc>),
}

/// What one `mark_read` item names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MarkReadTarget {
    /// A room's main timeline.
    Channel(Uuid),
    /// A thread, by root event id (32 bytes).
    Thread(Vec<u8>),
}

/// One `mark_read` item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkReadItem {
    /// Room or thread.
    pub target: MarkReadTarget,
    /// New read position; `None` changes only the unread mark.
    pub position: Option<ReadPosition>,
}

/// Stored state of one item after the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkReadResult {
    /// Room or thread, as requested.
    pub target: MarkReadTarget,
    /// Room the item belongs to.
    pub channel_id: Uuid,
    /// Stored read position after the merge (`None` when nothing is stored).
    pub read_seq: Option<i64>,
    /// Stored unread mark.
    pub unread_mark: bool,
    /// False for a thread the reader has no follow row for: there is nowhere to
    /// store its read position, so nothing changed.
    pub stored: bool,
}

/// Why a request was refused. Nothing was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatStateDenied {
    /// The reader is not a current member of the item's room, or the thread
    /// root does not exist in a room.
    NotMember {
        /// Index of the refused item (0 for `thread_follow`).
        index: usize,
    },
    /// The named event is not in that room or thread.
    UnknownEvent {
        /// Index of the refused item.
        index: usize,
    },
    /// The id names a reply, not a thread root.
    NotThreadRoot,
}

/// Explicit follow state a reader may set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowState {
    /// Follow the thread.
    Followed,
    /// Stop following; stays until the reader replies or follows again.
    Unfollowed,
}

impl FollowState {
    /// Stored `follow_state` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Followed => "followed",
            Self::Unfollowed => "unfollowed",
        }
    }
}

/// The room a thread lives in, from the root event.
async fn thread_channel(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    root: &[u8],
) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT channel_id FROM events \
         WHERE community_id = $1 AND id = $2 AND channel_id IS NOT NULL \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(community)
    .bind(root)
    .fetch_optional(&mut **tx)
    .await?
    .flatten())
}

/// What an id given as a thread root names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThreadRoot {
    /// No live message with that id in a room of this community (missing,
    /// deleted, or not a message kind).
    Unknown,
    /// A live reply in this room, not a root.
    Reply(Uuid),
    /// A live message in this room that is not a reply.
    Root(Uuid),
}

impl ThreadRoot {
    /// The room, when the id names a live message.
    pub(crate) fn channel(self) -> Option<Uuid> {
        match self {
            Self::Unknown => None,
            Self::Reply(channel) | Self::Root(channel) => Some(channel),
        }
    }
}

/// Look up `root` as a thread root: a live message of a chat-list kind
/// (`MESSAGE_KINDS`) in a room, and whether it is itself a reply. Callers
/// check membership before telling a reply apart from a root.
pub(crate) async fn thread_root(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    root: &[u8],
) -> Result<ThreadRoot> {
    let row: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT e.channel_id, EXISTS ( \
                SELECT 1 FROM thread_metadata r \
                 WHERE r.community_id = $1 AND r.event_id = e.id AND r.depth > 0) \
         FROM events e \
         WHERE e.community_id = $1 AND e.id = $2 AND e.channel_id IS NOT NULL \
           AND e.deleted_at IS NULL AND e.kind = ANY($3) \
         ORDER BY e.created_at DESC LIMIT 1",
    )
    .bind(community)
    .bind(root)
    .bind(&MESSAGE_KINDS[..])
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match row {
        None => ThreadRoot::Unknown,
        Some((channel, true)) => ThreadRoot::Reply(channel),
        Some((channel, false)) => ThreadRoot::Root(channel),
    })
}

/// Lock the reader's current membership row; `false` when not a member.
pub(crate) async fn lock_membership(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel_id: Uuid,
    reader: &[u8],
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM channel_members \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3 AND removed_at IS NULL \
         FOR UPDATE",
    )
    .bind(community)
    .bind(channel_id)
    .bind(reader)
    .fetch_optional(&mut **tx)
    .await?
    .is_some())
}

/// Channel sequence of `event` when it is in `channel_id` (and, for a thread,
/// is the root or one of its replies).
async fn event_seq(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel_id: Uuid,
    event: &[u8],
    thread_root: Option<&[u8]>,
) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT e.channel_sequence FROM events e \
         WHERE e.community_id = $1 AND e.id = $2 AND e.channel_id = $3 \
           AND e.channel_sequence > 0 \
           AND ($4::bytea IS NULL OR e.id = $4 OR EXISTS ( \
               SELECT 1 FROM thread_metadata r \
               WHERE r.community_id = $1 AND r.event_id = e.id AND r.root_event_id = $4)) \
         ORDER BY e.created_at DESC LIMIT 1",
    )
    .bind(community)
    .bind(event)
    .bind(channel_id)
    .bind(thread_root)
    .fetch_optional(&mut **tx)
    .await?)
}

/// How many of the room's newest messages at or before `at` (by `created_at`)
/// [`seq_at`] considers.
const SEQ_AT_ROOM_WINDOW: i64 = 64;

/// Read position for "read up to time `at`", where time is the message's
/// `created_at`: the time its signer put on it, not when the relay received
/// it. The highest channel sequence of a live message created at or before
/// `at`, in the room (`thread_root` none) or among the thread's root and
/// replies, clamped to the channel's accepted sequence; 0 when there is none.
///
/// A room is resolved through the `(community_id, channel_id, created_at)`
/// events index: the newest [`SEQ_AT_ROOM_WINDOW`] messages at or before
/// `at`, highest sequence among them. The cost is bounded however old `at`
/// is (an `at` before the first message finds nothing and gives 0). Exact
/// unless a message signed further back than that window arrived later than
/// all of it; such a message then counts as unread. A thread scans only its
/// own root and replies.
pub(crate) async fn seq_at(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel_id: Uuid,
    thread_root: Option<&[u8]>,
    at: DateTime<Utc>,
) -> Result<i64> {
    let seq: Option<i64> = match thread_root {
        None => {
            sqlx::query_scalar(
                "SELECT max(w.seq) FROM ( \
                     SELECT e.channel_sequence AS seq FROM events e \
                      WHERE e.community_id = $1 AND e.channel_id = $2 \
                        AND e.created_at <= $3 AND e.channel_sequence > 0 \
                        AND e.kind = ANY($4) AND e.deleted_at IS NULL \
                      ORDER BY e.created_at DESC LIMIT $5) w",
            )
            .bind(community)
            .bind(channel_id)
            .bind(at)
            .bind(&MESSAGE_KINDS[..])
            .bind(SEQ_AT_ROOM_WINDOW)
            .fetch_one(&mut **tx)
            .await?
        }
        Some(root) => {
            sqlx::query_scalar(
                "SELECT max(s.seq) FROM ( \
                     SELECT e.channel_sequence AS seq FROM events e \
                      WHERE e.community_id = $1 AND e.id = $5 AND e.channel_id = $2 \
                        AND e.channel_sequence > 0 AND e.created_at <= $3 \
                        AND e.kind = ANY($4) AND e.deleted_at IS NULL \
                     UNION ALL \
                     SELECT e.channel_sequence FROM thread_metadata r \
                       JOIN events e ON e.community_id = r.community_id \
                        AND e.created_at = r.event_created_at AND e.id = r.event_id \
                      WHERE r.community_id = $1 AND r.root_event_id = $5 AND r.depth > 0 \
                        AND e.channel_id = $2 AND e.channel_sequence > 0 \
                        AND e.created_at <= $3 AND e.kind = ANY($4) \
                        AND e.deleted_at IS NULL) s",
            )
            .bind(community)
            .bind(channel_id)
            .bind(at)
            .bind(&MESSAGE_KINDS[..])
            .bind(root)
            .fetch_one(&mut **tx)
            .await?
        }
    };
    let Some(seq) = seq else {
        return Ok(0);
    };
    let accepted: Option<i64> = sqlx::query_scalar(
        "SELECT accepted_sequence FROM channels WHERE community_id = $1 AND id = $2",
    )
    .bind(community)
    .bind(channel_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(accepted.map_or(seq, |accepted| seq.min(accepted)))
}

impl Db {
    /// Apply one `mark_read` request for `reader` atomically.
    ///
    /// Each position becomes `read_seq = max(current, position)`, clamped to
    /// the channel's accepted sequence; every item's unread mark is set to
    /// `unread_mark`. Returns `Ok(Err(..))`, with nothing written, when any
    /// item is refused.
    #[datastore_span(name = "chat_mark_read", system = "postgresql")]
    pub async fn chat_mark_read(
        &self,
        community: CommunityId,
        reader: &nostr::PublicKey,
        items: &[MarkReadItem],
        unread_mark: bool,
    ) -> Result<std::result::Result<Vec<MarkReadResult>, ChatStateDenied>> {
        let community_uuid = *community.as_uuid();
        let reader = reader.to_bytes();
        let reader = reader.as_slice();
        let conn =
            observability::acquire_writer(&self.pool, observability::WriterOperation::EventWrite)
                .await?;
        let mut tx = Transaction::begin(conn, None).await?;
        // Resolve every item's room first, then lock and write in a fixed
        // (room, thread) order so two requests naming the same rooms in a
        // different order cannot deadlock.
        let mut resolved = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            let (channel_id, root) = match &item.target {
                MarkReadTarget::Channel(channel_id) => (Some(*channel_id), None),
                MarkReadTarget::Thread(root) => (
                    thread_channel(&mut tx, community_uuid, root).await?,
                    Some(root.as_slice()),
                ),
            };
            let Some(channel_id) = channel_id else {
                return Ok(Err(ChatStateDenied::NotMember { index }));
            };
            resolved.push((channel_id, root, index));
        }
        resolved.sort();
        let mut slots: Vec<Option<MarkReadResult>> = vec![None; items.len()];
        for (channel_id, root, index) in resolved {
            let item = &items[index];
            if !lock_membership(&mut tx, community_uuid, channel_id, reader).await? {
                return Ok(Err(ChatStateDenied::NotMember { index }));
            }
            let seq = match &item.position {
                None => None,
                Some(ReadPosition::Seq(seq)) => Some(*seq),
                Some(ReadPosition::Event(event)) => {
                    match event_seq(&mut tx, community_uuid, channel_id, event, root).await? {
                        Some(seq) => Some(seq),
                        None => return Ok(Err(ChatStateDenied::UnknownEvent { index })),
                    }
                }
                Some(ReadPosition::At(at)) => {
                    Some(seq_at(&mut tx, community_uuid, channel_id, root, *at).await?)
                }
            };
            let row = match root {
                None => {
                    sqlx::query(
                        "UPDATE channel_members cm \
                         SET read_seq = CASE WHEN $4::bigint IS NULL THEN cm.read_seq \
                                 ELSE GREATEST(COALESCE(cm.read_seq, 0), \
                                               LEAST($4, c.accepted_sequence)) END, \
                             read_origin = CASE WHEN $4::bigint IS NULL THEN cm.read_origin \
                                 ELSE 'user' END, \
                             unread_mark = $5 \
                         FROM channels c \
                         WHERE c.community_id = cm.community_id AND c.id = cm.channel_id \
                           AND cm.community_id = $1 AND cm.channel_id = $2 AND cm.pubkey = $3 \
                         RETURNING cm.read_seq, cm.unread_mark",
                    )
                    .bind(community_uuid)
                    .bind(channel_id)
                    .bind(reader)
                    .bind(seq)
                    .bind(unread_mark)
                    .fetch_optional(&mut *tx)
                    .await?
                }
                Some(root) => {
                    sqlx::query(
                        "UPDATE thread_follows tf \
                         SET read_seq = CASE WHEN $4::bigint IS NULL THEN tf.read_seq \
                                 ELSE GREATEST(COALESCE(tf.read_seq, 0), \
                                               LEAST($4, c.accepted_sequence)) END, \
                             read_origin = CASE WHEN $4::bigint IS NULL THEN tf.read_origin \
                                 ELSE 'user' END, \
                             unread_mark = $5, updated_at = NOW() \
                         FROM channels c \
                         WHERE c.community_id = tf.community_id AND c.id = tf.channel_id \
                           AND tf.community_id = $1 AND tf.root_event_id = $2 AND tf.pubkey = $3 \
                         RETURNING tf.read_seq, tf.unread_mark",
                    )
                    .bind(community_uuid)
                    .bind(root)
                    .bind(reader)
                    .bind(seq)
                    .bind(unread_mark)
                    .fetch_optional(&mut *tx)
                    .await?
                }
            };
            slots[index] = Some(match row {
                Some(row) => MarkReadResult {
                    target: item.target.clone(),
                    channel_id,
                    read_seq: row.try_get("read_seq")?,
                    unread_mark: row.try_get("unread_mark")?,
                    stored: true,
                },
                None => MarkReadResult {
                    target: item.target.clone(),
                    channel_id,
                    read_seq: None,
                    unread_mark: false,
                    stored: false,
                },
            });
        }
        tx.commit().await?;
        Ok(Ok(slots.into_iter().flatten().collect()))
    }

    /// Set `reader`'s explicit follow state for the thread rooted at `root`
    /// (`follow_origin = 'user'`). The reader must be a current member of the
    /// root's room; `root` must be a live message of a chat-list kind and not
    /// a reply.
    #[datastore_span(name = "chat_thread_follow", system = "postgresql")]
    pub async fn chat_thread_follow(
        &self,
        community: CommunityId,
        reader: &nostr::PublicKey,
        root: &[u8],
        state: FollowState,
    ) -> Result<std::result::Result<Uuid, ChatStateDenied>> {
        let community_uuid = *community.as_uuid();
        let reader = reader.to_bytes();
        let reader = reader.as_slice();
        let conn =
            observability::acquire_writer(&self.pool, observability::WriterOperation::EventWrite)
                .await?;
        let mut tx = Transaction::begin(conn, None).await?;
        let found = thread_root(&mut tx, community_uuid, root).await?;
        let Some(channel_id) = found.channel() else {
            return Ok(Err(ChatStateDenied::NotMember { index: 0 }));
        };
        if !lock_membership(&mut tx, community_uuid, channel_id, reader).await? {
            return Ok(Err(ChatStateDenied::NotMember { index: 0 }));
        }
        if matches!(found, ThreadRoot::Reply(_)) {
            return Ok(Err(ChatStateDenied::NotThreadRoot));
        }
        // A new row starts caught up: read up to the thread's latest reply.
        // An existing row keeps its read position and where it came from (a
        // backfilled position stays backfill, so a first-run upload may still
        // move it).
        sqlx::query(
            "INSERT INTO thread_follows \
                 (community_id, root_event_id, pubkey, channel_id, follow_state, follow_origin, \
                  read_seq, updated_at) \
             VALUES ($1, $2, $3, $4, $5, 'user', \
                     (SELECT ts.last_seq FROM thread_state ts \
                       WHERE ts.community_id = $1 AND ts.root_event_id = $2), \
                     NOW()) \
             ON CONFLICT (community_id, root_event_id, pubkey) DO UPDATE \
             SET follow_state = EXCLUDED.follow_state, follow_origin = 'user', \
                 read_origin = COALESCE(thread_follows.read_origin, \
                     CASE WHEN thread_follows.follow_origin = 'backfill' THEN 'backfill' END), \
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(community_uuid)
        .bind(root)
        .bind(reader)
        .bind(channel_id)
        .bind(state.as_str())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok(channel_id))
    }
}
