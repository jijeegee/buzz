//! First-run upload of device-local chat state (Chats stage S2b, design §5).
//!
//! A new app's first run sends the read positions and follows it kept on the
//! device. The backfill seeded every reader's state from message history; the
//! upload corrects it once and never overrides anything the reader did since:
//!
//! - A read moves a read position up or down, but only while the position is
//!   still the backfill's (`read_origin` 'backfill', or none yet for a room);
//!   it then becomes the reader's own (`read_origin = 'user'`).
//! - A follow overwrites the follow state only of a backfilled row
//!   (`follow_origin = 'backfill'`, then 'user'). A thread the reader has no
//!   row for gets one, caught up to the thread's latest reply, or to the same
//!   request's read position for it.
//!
//! Items that cannot apply (not a member, unknown thread, already the reader's
//! own state) are skipped one by one with a reason; the rest still apply, in
//! one transaction.
//!
//! Read positions and follows are private per-user state: never log them.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::chat_user_state::{
    lock_membership, seq_at, thread_channel, FollowState, MarkReadTarget,
};
use crate::{observability, Db, Result};

/// Most reads plus follows one `chat_import` request may carry.
pub const MAX_CHAT_IMPORT_ITEMS: usize = 500;

/// One uploaded read position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRead {
    /// Room or thread.
    pub target: MarkReadTarget,
    /// Read up to this time (see `ReadPosition::At`). A thread read uses the
    /// later of this and the same request's read time for the thread's room.
    pub at: DateTime<Utc>,
}

/// One uploaded explicit follow state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFollow {
    /// Thread root (32 bytes).
    pub root: Vec<u8>,
    /// Followed or unfollowed.
    pub state: FollowState,
}

/// Why an uploaded item did not apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSkip {
    /// The reader is not a current member of the item's room in this
    /// community.
    NotMember,
    /// No thread root with that id in this community.
    UnknownThread,
    /// The id names a reply, not a thread root.
    NotThreadRoot,
    /// A thread read with no follow row: nowhere to store it.
    NotFollowed,
    /// The server already holds the reader's own state for it.
    AlreadySet,
}

impl ImportSkip {
    /// Wire name of the reason.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotMember => "not_member",
            Self::UnknownThread => "unknown_thread",
            Self::NotThreadRoot => "not_thread_root",
            Self::NotFollowed => "not_followed",
            Self::AlreadySet => "already_set",
        }
    }
}

/// Outcome of one uploaded item, in request order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportItemResult {
    /// Room the item belongs to, when known.
    pub channel_id: Option<Uuid>,
    /// `None` when the item applied.
    pub skipped: Option<ImportSkip>,
    /// Reads that applied: the stored read position.
    pub read_seq: Option<i64>,
}

impl ImportItemResult {
    fn skip(channel_id: Option<Uuid>, reason: ImportSkip) -> Self {
        Self {
            channel_id,
            skipped: Some(reason),
            read_seq: None,
        }
    }

    fn applied(channel_id: Uuid, read_seq: Option<i64>) -> Self {
        Self {
            channel_id: Some(channel_id),
            skipped: None,
            read_seq,
        }
    }

    /// Whether the item changed stored state.
    pub fn is_applied(&self) -> bool {
        self.skipped.is_none()
    }
}

/// Per-item outcomes of one `chat_import` request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatImportOutcome {
    /// One per uploaded read, in request order.
    pub reads: Vec<ImportItemResult>,
    /// One per uploaded follow, in request order.
    pub follows: Vec<ImportItemResult>,
}

impl ChatImportOutcome {
    /// Whether any item changed stored state.
    pub fn any_applied(&self) -> bool {
        self.reads
            .iter()
            .chain(&self.follows)
            .any(ImportItemResult::is_applied)
    }
}

/// Which uploaded item a work step handles. A thread's follow sorts before
/// its read so the read can find a row the follow inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Follow(usize),
    Read(usize),
}

/// The read position stored for `root` when its follow row was inserted by
/// this request.
type Inserted = HashMap<Vec<u8>, Option<i64>>;

impl Db {
    /// Apply one first-run `chat_import` upload for `reader`.
    ///
    /// Items resolve to rooms first, then lock and write in the same fixed
    /// (room, thread) order as `chat_mark_read`, so concurrent requests
    /// cannot deadlock. Returns one result per item.
    #[datastore_span(name = "chat_import", system = "postgresql")]
    pub async fn chat_import(
        &self,
        community: CommunityId,
        reader: &nostr::PublicKey,
        reads: &[ImportRead],
        follows: &[ImportFollow],
    ) -> Result<ChatImportOutcome> {
        let community = *community.as_uuid();
        let reader = reader.to_bytes();
        let reader = reader.as_slice();
        let conn =
            observability::acquire_writer(&self.pool, observability::WriterOperation::EventWrite)
                .await?;
        let mut tx = Transaction::begin(conn, None).await?;
        let mut outcome = ChatImportOutcome {
            reads: vec![ImportItemResult::skip(None, ImportSkip::UnknownThread); reads.len()],
            follows: vec![ImportItemResult::skip(None, ImportSkip::UnknownThread); follows.len()],
        };

        // Room read times, and each thread's read time raised to its room's.
        let mut room_at: HashMap<Uuid, DateTime<Utc>> = HashMap::new();
        for read in reads {
            if let MarkReadTarget::Channel(channel) = read.target {
                let entry = room_at.entry(channel).or_insert(read.at);
                *entry = (*entry).max(read.at);
            }
        }
        let mut thread_at: HashMap<Vec<u8>, DateTime<Utc>> = HashMap::new();

        let mut steps: Vec<(Uuid, Option<Vec<u8>>, Step)> = Vec::new();
        for (index, read) in reads.iter().enumerate() {
            match &read.target {
                MarkReadTarget::Channel(channel) => steps.push((*channel, None, Step::Read(index))),
                MarkReadTarget::Thread(root) => {
                    // Unknown roots keep the default `unknown_thread` result.
                    if let Some(channel) = thread_channel(&mut tx, community, root).await? {
                        let at = room_at.get(&channel).map_or(read.at, |r| (*r).max(read.at));
                        let entry = thread_at.entry(root.clone()).or_insert(at);
                        *entry = (*entry).max(at);
                        steps.push((channel, Some(root.clone()), Step::Read(index)));
                    }
                }
            }
        }
        for (index, follow) in follows.iter().enumerate() {
            if let Some(channel) = thread_channel(&mut tx, community, &follow.root).await? {
                steps.push((channel, Some(follow.root.clone()), Step::Follow(index)));
            }
        }
        steps.sort();

        let mut members: HashMap<Uuid, bool> = HashMap::new();
        let mut inserted: Inserted = HashMap::new();
        let mut followed_roots: HashSet<Vec<u8>> = HashSet::new();
        for (channel, root, step) in steps {
            let member = match members.get(&channel) {
                Some(member) => *member,
                None => {
                    let member = lock_membership(&mut tx, community, channel, reader).await?;
                    members.insert(channel, member);
                    member
                }
            };
            let result = if !member {
                ImportItemResult::skip(Some(channel), ImportSkip::NotMember)
            } else {
                match (step, root.as_deref()) {
                    (Step::Read(index), None) => {
                        import_room_read(&mut tx, community, channel, reader, reads[index].at)
                            .await?
                    }
                    (Step::Read(index), Some(root)) => {
                        let at = thread_at.get(root).copied().unwrap_or(reads[index].at);
                        import_thread_read(&mut tx, community, channel, reader, root, at, &inserted)
                            .await?
                    }
                    (Step::Follow(index), Some(root)) => {
                        if !followed_roots.insert(root.to_vec()) {
                            // A repeated root: the first one decided.
                            ImportItemResult::skip(Some(channel), ImportSkip::AlreadySet)
                        } else {
                            import_follow(
                                &mut tx,
                                community,
                                channel,
                                reader,
                                root,
                                follows[index].state,
                                thread_at.get(root).copied(),
                                &mut inserted,
                            )
                            .await?
                        }
                    }
                    (Step::Follow(_), None) => continue,
                }
            };
            match step {
                Step::Read(index) => outcome.reads[index] = result,
                Step::Follow(index) => outcome.follows[index] = result,
            }
        }
        tx.commit().await?;
        Ok(outcome)
    }
}

/// A room read applies while the reader has no read of their own there: the
/// backfill's position, or none yet.
async fn import_room_read(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel: Uuid,
    reader: &[u8],
    at: DateTime<Utc>,
) -> Result<ImportItemResult> {
    let seq = seq_at(tx, community, channel, None, at).await?;
    let stored: Option<Option<i64>> = sqlx::query_scalar(
        "UPDATE channel_members SET read_seq = $4, read_origin = 'user' \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3 \
           AND read_origin IS DISTINCT FROM 'user' \
         RETURNING read_seq",
    )
    .bind(community)
    .bind(channel)
    .bind(reader)
    .bind(seq)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match stored {
        Some(read_seq) => ImportItemResult::applied(channel, read_seq),
        None => ImportItemResult::skip(Some(channel), ImportSkip::AlreadySet),
    })
}

/// A thread read applies to a row whose read position is still the
/// backfill's. Rows the backfill wrote before `read_origin` existed count as
/// backfill too.
async fn import_thread_read(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel: Uuid,
    reader: &[u8],
    root: &[u8],
    at: DateTime<Utc>,
    inserted: &Inserted,
) -> Result<ImportItemResult> {
    if let Some(read_seq) = inserted.get(root) {
        // This request's follow created the row at this read position.
        return Ok(ImportItemResult::applied(channel, *read_seq));
    }
    let seq = seq_at(tx, community, channel, Some(root), at).await?;
    let stored: Option<Option<i64>> = sqlx::query_scalar(
        "UPDATE thread_follows SET read_seq = $4, read_origin = 'user', updated_at = NOW() \
         WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3 \
           AND COALESCE(read_origin, \
                        CASE WHEN follow_origin = 'backfill' THEN 'backfill' END) = 'backfill' \
         RETURNING read_seq",
    )
    .bind(community)
    .bind(root)
    .bind(reader)
    .bind(seq)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(read_seq) = stored {
        return Ok(ImportItemResult::applied(channel, read_seq));
    }
    let has_row: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM thread_follows \
                        WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3)",
    )
    .bind(community)
    .bind(root)
    .bind(reader)
    .fetch_one(&mut **tx)
    .await?;
    let reason = if has_row {
        ImportSkip::AlreadySet
    } else {
        ImportSkip::NotFollowed
    };
    Ok(ImportItemResult::skip(Some(channel), reason))
}

/// A follow overwrites only a backfilled row; with no row it inserts one.
#[allow(clippy::too_many_arguments)]
async fn import_follow(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel: Uuid,
    reader: &[u8],
    root: &[u8],
    state: FollowState,
    read_at: Option<DateTime<Utc>>,
    inserted: &mut Inserted,
) -> Result<ImportItemResult> {
    let is_reply: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM thread_metadata \
                        WHERE community_id = $1 AND event_id = $2 AND depth > 0)",
    )
    .bind(community)
    .bind(root)
    .fetch_one(&mut **tx)
    .await?;
    if is_reply {
        return Ok(ImportItemResult::skip(
            Some(channel),
            ImportSkip::NotThreadRoot,
        ));
    }
    // The read position stays backfill, so the same upload's read (or a
    // later one) may still correct it.
    let updated = sqlx::query(
        "UPDATE thread_follows \
         SET follow_state = $4, follow_origin = 'user', \
             read_origin = COALESCE(read_origin, 'backfill'), updated_at = NOW() \
         WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3 \
           AND follow_origin = 'backfill'",
    )
    .bind(community)
    .bind(root)
    .bind(reader)
    .bind(state.as_str())
    .execute(&mut **tx)
    .await?
    .rows_affected();
    if updated > 0 {
        return Ok(ImportItemResult::applied(channel, None));
    }
    let read_seq = match read_at {
        Some(at) => Some(seq_at(tx, community, channel, Some(root), at).await?),
        None => None,
    };
    let stored: Option<Option<i64>> = sqlx::query_scalar(
        "INSERT INTO thread_follows \
             (community_id, root_event_id, pubkey, channel_id, follow_state, follow_origin, \
              read_seq, read_origin, updated_at) \
         VALUES ($1, $2, $3, $4, $5, 'user', \
                 COALESCE($6, (SELECT ts.last_seq FROM thread_state ts \
                                WHERE ts.community_id = $1 AND ts.root_event_id = $2)), \
                 'user', NOW()) \
         ON CONFLICT (community_id, root_event_id, pubkey) DO NOTHING \
         RETURNING read_seq",
    )
    .bind(community)
    .bind(root)
    .bind(reader)
    .bind(channel)
    .bind(state.as_str())
    .bind(read_seq)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match stored {
        Some(read_seq) => {
            inserted.insert(root.to_vec(), read_seq);
            ImportItemResult::applied(channel, None)
        }
        None => ImportItemResult::skip(Some(channel), ImportSkip::AlreadySet),
    })
}
