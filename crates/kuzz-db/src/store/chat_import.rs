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
//! A read time is matched against each message's `created_at`, the time its
//! signer put on it (not when the relay received it).
//!
//! A thread root must be a live message of a chat-list kind in a room; a
//! reply id is `not_thread_root`. Items that cannot apply (not a member,
//! unknown thread, already the reader's own state) are skipped one by one
//! with a reason, before any read time is resolved; the rest still apply, in
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
    lock_membership, seq_at, thread_root, FollowState, MarkReadTarget, ThreadRoot,
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
    /// No live message of a chat-list kind with that id in a room of this
    /// community.
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

        // (room, thread root, step, the id is a reply). Unknown roots keep the
        // default `unknown_thread` result.
        let mut steps: Vec<(Uuid, Option<Vec<u8>>, Step, bool)> = Vec::new();
        for (index, read) in reads.iter().enumerate() {
            match &read.target {
                MarkReadTarget::Channel(channel) => {
                    steps.push((*channel, None, Step::Read(index), false))
                }
                MarkReadTarget::Thread(root) => {
                    let found = thread_root(&mut tx, community, root).await?;
                    if let Some(channel) = found.channel() {
                        let at = room_at.get(&channel).map_or(read.at, |r| (*r).max(read.at));
                        let entry = thread_at.entry(root.clone()).or_insert(at);
                        *entry = (*entry).max(at);
                        let is_reply = matches!(found, ThreadRoot::Reply(_));
                        steps.push((channel, Some(root.clone()), Step::Read(index), is_reply));
                    }
                }
            }
        }
        for (index, follow) in follows.iter().enumerate() {
            let found = thread_root(&mut tx, community, &follow.root).await?;
            if let Some(channel) = found.channel() {
                let is_reply = matches!(found, ThreadRoot::Reply(_));
                steps.push((
                    channel,
                    Some(follow.root.clone()),
                    Step::Follow(index),
                    is_reply,
                ));
            }
        }
        steps.sort();

        let mut members: HashMap<Uuid, bool> = HashMap::new();
        let mut inserted: Inserted = HashMap::new();
        let mut followed_roots: HashSet<Vec<u8>> = HashSet::new();
        for (channel, root, step, is_reply) in steps {
            let member = match members.get(&channel) {
                Some(member) => *member,
                None => {
                    let member = lock_membership(&mut tx, community, channel, reader).await?;
                    members.insert(channel, member);
                    member
                }
            };
            // Membership first: a non-member learns nothing about the id.
            let result = if !member {
                ImportItemResult::skip(Some(channel), ImportSkip::NotMember)
            } else if is_reply {
                ImportItemResult::skip(Some(channel), ImportSkip::NotThreadRoot)
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
/// backfill's position, or none yet. The origin is checked (on the row the
/// membership lock already holds) before the time is resolved, so a skipped
/// item costs one row read.
async fn import_room_read(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    channel: Uuid,
    reader: &[u8],
    at: DateTime<Utc>,
) -> Result<ImportItemResult> {
    let origin: Option<String> = sqlx::query_scalar(
        "SELECT read_origin FROM channel_members \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
    )
    .bind(community)
    .bind(channel)
    .bind(reader)
    .fetch_one(&mut **tx)
    .await?;
    if origin.as_deref() == Some("user") {
        return Ok(ImportItemResult::skip(
            Some(channel),
            ImportSkip::AlreadySet,
        ));
    }
    let seq = seq_at(tx, community, channel, None, at).await?;
    let read_seq: Option<i64> = sqlx::query_scalar(
        "UPDATE channel_members SET read_seq = $4, read_origin = 'user' \
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3 \
         RETURNING read_seq",
    )
    .bind(community)
    .bind(channel)
    .bind(reader)
    .bind(seq)
    .fetch_one(&mut **tx)
    .await?;
    Ok(ImportItemResult::applied(channel, read_seq))
}

/// The reader's follow row for `root`, locked: (follow origin, whether its
/// read position is still the backfill's). A NULL read origin on a
/// backfilled follow counts as backfill.
async fn lock_follow_row(
    tx: &mut Transaction<'_, Postgres>,
    community: Uuid,
    reader: &[u8],
    root: &[u8],
) -> Result<Option<(String, bool)>> {
    Ok(sqlx::query_as(
        "SELECT follow_origin, \
                COALESCE(read_origin, \
                         CASE WHEN follow_origin = 'backfill' THEN 'backfill' END) \
                    IS NOT DISTINCT FROM 'backfill' \
         FROM thread_follows \
         WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3 \
         FOR UPDATE",
    )
    .bind(community)
    .bind(root)
    .bind(reader)
    .fetch_optional(&mut **tx)
    .await?)
}

/// A thread read applies to a row whose read position is still the
/// backfill's. The row is checked before the time is resolved.
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
    match lock_follow_row(tx, community, reader, root).await? {
        None => {
            return Ok(ImportItemResult::skip(
                Some(channel),
                ImportSkip::NotFollowed,
            ))
        }
        Some((_, false)) => {
            return Ok(ImportItemResult::skip(
                Some(channel),
                ImportSkip::AlreadySet,
            ))
        }
        Some((_, true)) => {}
    }
    let seq = seq_at(tx, community, channel, Some(root), at).await?;
    let read_seq: Option<i64> = sqlx::query_scalar(
        "UPDATE thread_follows SET read_seq = $4, read_origin = 'user', updated_at = NOW() \
         WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3 \
         RETURNING read_seq",
    )
    .bind(community)
    .bind(root)
    .bind(reader)
    .bind(seq)
    .fetch_one(&mut **tx)
    .await?;
    Ok(ImportItemResult::applied(channel, read_seq))
}

/// A follow overwrites only a backfilled row; with no row it inserts one.
/// The row is checked before any read time is resolved.
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
    match lock_follow_row(tx, community, reader, root).await? {
        Some((origin, _)) if origin == "backfill" => {
            // The read position stays backfill, so the same upload's read
            // (or a later one) may still correct it.
            sqlx::query(
                "UPDATE thread_follows \
                 SET follow_state = $4, follow_origin = 'user', \
                     read_origin = COALESCE(read_origin, 'backfill'), updated_at = NOW() \
                 WHERE community_id = $1 AND root_event_id = $2 AND pubkey = $3",
            )
            .bind(community)
            .bind(root)
            .bind(reader)
            .bind(state.as_str())
            .execute(&mut **tx)
            .await?;
            return Ok(ImportItemResult::applied(channel, None));
        }
        Some(_) => {
            return Ok(ImportItemResult::skip(
                Some(channel),
                ImportSkip::AlreadySet,
            ))
        }
        None => {}
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
