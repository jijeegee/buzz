//! Relay-signed kind:9005 deletion records for server-side removals.
//!
//! Staff deletes (admin console, report resolution) and kind:5 deletions by a
//! signer other than the target's raw author soft-delete a channel row that
//! device stores never learn about: stores hide a stored row only for a 9005,
//! or a kind:5 with the target's own pubkey. These paths therefore also store a
//! relay-signed 9005 (`h`=channel, `e`=target, `k`=target kind, empty content)
//! naming the removed row. Overlays (reactions, edits, deletions) a device
//! stores beside the rows get the same record, so a removed reaction or edit
//! leaves device stores too.
//!
//! Invariants:
//! - One relay record per target. The insert holds the channel lock, then a
//!   per-target advisory lock, and checks for an existing relay record first.
//!   A soft-deleted row takes no row lock, so the advisory lock is the dedupe
//!   fence.
//! - `created_at` is the signing time, taken inside the transaction right
//!   before the insert. Migration 0021 rejects channel rows older than commit
//!   time, so a signed record is never reused across transactions.
//! - Only soft-deleted content or overlay rows with a channel get a record.
//! - A delete that needs a record commits only with it: the delete and the
//!   record share one transaction, and when every attempt fails nothing is
//!   deleted (no removal devices cannot see).

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{Db, DbError, Result};

/// Kind of the deletion record (NIP-29 delete event).
pub const DELETION_RECORD_KIND: u16 = 9005;

/// Room content kinds that get a record. Mirrors the device stores' content
/// kinds (`desktop/src-tauri/src/message_store/classify.rs` `CONTENT_KINDS`).
pub const DELETION_RECORD_CONTENT_KINDS: [i32; 17] = [
    9, 40001, 40002, 40008, 40099, 43001, 43002, 43003, 43004, 43005, 43006, 45001, 45003, 48100,
    48101, 48102, 48103,
];

/// Overlay kinds that get a record: deletions, reactions, admin removals and
/// edits. Mirrors the device stores' overlay kinds (`classify.rs` `AUX_KINDS`).
pub const DELETION_RECORD_OVERLAY_KINDS: [i32; 4] = [5, 7, 9005, 40003];

/// Every kind that gets a record: content and overlay kinds.
pub const DELETION_RECORD_KINDS: [i32; 21] = [
    9, 40001, 40002, 40008, 40099, 43001, 43002, 43003, 43004, 43005, 43006, 45001, 45003, 48100,
    48101, 48102, 48103, 5, 7, 9005, 40003,
];

/// Attempts a recorded delete or [`Db::write_deletion_record`] makes, each in
/// a fresh transaction with a fresh signature.
const WRITE_ATTEMPTS: usize = 3;

/// Whether a removed row of `kind` gets a deletion record.
pub fn is_deletion_record_kind(kind: i32) -> bool {
    DELETION_RECORD_KINDS.contains(&kind)
}

/// Whether `event` is a relay-signed deletion record (it must stay undeletable).
pub fn is_relay_deletion_record(event: &Event, relay_pubkey: &nostr::PublicKey) -> bool {
    event.kind.as_u16() == DELETION_RECORD_KIND && event.pubkey == *relay_pubkey
}

/// Result of [`Db::execute_delete_with_marker_and_record`].
#[derive(Debug)]
pub struct AdminDeleteOutcome {
    /// This driver committed the delete and the step marker.
    pub committed: bool,
    /// The record this transaction stored, set only after a successful commit.
    /// Publish it to the channel; `None` means nothing new to deliver.
    pub record: Option<Event>,
}

/// Result of [`Db::soft_delete_event_with_record`].
#[derive(Debug)]
pub struct RecordedDelete {
    /// The row went from visible to deleted in this call.
    pub deleted: bool,
    /// The record stored with the delete, set only after a successful commit.
    /// Publish it to the channel; `None` means nothing new to deliver.
    pub record: Option<Event>,
}

/// One target the backfill would record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionRecordCandidate {
    /// Removed event id.
    pub target_id: Vec<u8>,
    /// Its channel.
    pub channel_id: Uuid,
    /// Its kind.
    pub kind: i32,
}

fn sign_record(keys: &Keys, channel_id: Uuid, target_id: &[u8], kind: i32) -> Result<Event> {
    let tags = [
        ["h".to_owned(), channel_id.to_string()],
        ["e".to_owned(), hex::encode(target_id)],
        ["k".to_owned(), kind.to_string()],
    ]
    .into_iter()
    .map(Tag::parse)
    .collect::<std::result::Result<Vec<_>, _>>()
    .map_err(|e| DbError::InvalidData(e.to_string()))?;
    EventBuilder::new(Kind::Custom(DELETION_RECORD_KIND), "")
        .tags(tags)
        .custom_created_at(Timestamp::now())
        .sign_with_keys(keys)
        .map_err(|e| DbError::InvalidData(e.to_string()))
}

/// Store a relay-signed record for `target_id` in the caller's transaction.
///
/// Returns the inserted record, or `None` when the target is missing, not
/// soft-deleted, has no channel, is not a recorded kind, or already has a
/// relay record. The caller commits; the record is visible (and may be published)
/// only after that commit succeeds.
pub(crate) async fn record_deletion_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    target_id: &[u8],
    keys: &Keys,
) -> Result<Option<Event>> {
    let community = community_id.as_uuid();
    let target: Option<(i32, Option<Uuid>, DateTime<Utc>, bool)> = sqlx::query_as(
        "SELECT kind, channel_id, created_at, deleted_at IS NOT NULL FROM events \
         WHERE community_id = $1 AND id = $2 \
         ORDER BY deleted_at IS NOT NULL DESC LIMIT 1",
    )
    .bind(community)
    .bind(target_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((kind, Some(channel_id), target_created_at, true)) = target else {
        return Ok(None);
    };
    if !is_deletion_record_kind(kind) {
        return Ok(None);
    }

    // Channel lock first (the insert trigger takes the same key), then the
    // target lock, so every record writer acquires them in one order.
    lock_channel(tx, community, channel_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "kuzz_deletion_record:{community}:{}",
            hex::encode(target_id)
        ))
        .execute(&mut **tx)
        .await?;

    // A record is signed after its target (or now, for a future-dated target),
    // so the lower bound prunes older partitions; the day of slack covers
    // clock skew in the target's timestamp.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events \
         WHERE community_id = $1 AND channel_id = $2 AND kind = $3 AND pubkey = $4 \
           AND created_at >= LEAST($5, now()) - interval '1 day' \
           AND tags @> jsonb_build_array(jsonb_build_array('e', $6::text)))",
    )
    .bind(community)
    .bind(channel_id)
    .bind(i32::from(DELETION_RECORD_KIND))
    .bind(keys.public_key().to_bytes().as_slice())
    .bind(target_created_at)
    .bind(hex::encode(target_id))
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        return Ok(None);
    }

    let record = sign_record(keys, channel_id, target_id, kind)?;
    let (_, inserted) =
        crate::event::insert_event_in_transaction(tx, community_id, &record, Some(channel_id))
            .await?;
    Ok(inserted.then_some(record))
}

/// Take the channel lock (the key the channel insert trigger takes).
async fn lock_channel(
    tx: &mut sqlx::PgConnection,
    community: &Uuid,
    channel_id: Uuid,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("kuzz_channel_membership:{community}:{channel_id}"))
        .execute(&mut *tx)
        .await
        .map(|_| ())
}

/// Before a delete that writes a record: take the channel lock the record
/// insert needs, so it is held before the delete locks thread_metadata rows,
/// matching channel inserts (channel lock in the insert trigger, then
/// thread_metadata). Recorded kinds only: head kinds take the head
/// replacement lock first and get no record.
async fn lock_channel_for_record(
    tx: &mut sqlx::PgConnection,
    community_id: CommunityId,
    target_id: &[u8],
) -> std::result::Result<(), sqlx::Error> {
    let community = community_id.as_uuid();
    let row: Option<(i32, Option<Uuid>)> = sqlx::query_as(
        "SELECT kind, channel_id FROM events WHERE community_id = $1 AND id = $2 LIMIT 1",
    )
    .bind(community)
    .bind(target_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((kind, Some(channel_id))) = row {
        if is_deletion_record_kind(kind) {
            lock_channel(tx, community, channel_id).await?;
        }
    }
    Ok(())
}

/// Soft-delete `target_event_id`, store its relay record when `record_keys` is
/// set, and commit the step marker, all in one transaction fenced by
/// `action_id` AND the caller's `lease_token`.
///
/// The delete is idempotent: if the event is already deleted the marker is still
/// committed (soft-delete is already-done = success), and the record is written
/// if the target has none yet.
///
/// The delete commits only with its record. A failed record write or a commit
/// that rejects the record retries with a fresh transaction and signature;
/// after [`WRITE_ATTEMPTS`] the error is returned and nothing was deleted.
#[allow(clippy::too_many_arguments)]
pub async fn execute_delete_with_record(
    pool: &PgPool,
    action_id: Uuid,
    lease_token: Uuid,
    community_id: CommunityId,
    target_event_id: &[u8],
    parent_event_id: Option<&[u8]>,
    root_event_id: Option<&[u8]>,
    record_keys: Option<&Keys>,
) -> Result<AdminDeleteOutcome> {
    let target = DeleteTarget {
        action_id,
        lease_token,
        community_id,
        target_event_id,
        parent_event_id,
        root_event_id,
    };
    retry_record(|| delete_once(pool, &target, record_keys)).await
}

/// Run `attempt` until it succeeds, fails for a reason other than its record,
/// or [`WRITE_ATTEMPTS`] record failures happened (then that error).
async fn retry_record<T, F, Fut>(mut attempt: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, DeleteError>>,
{
    let mut tries = 0;
    loop {
        tries += 1;
        match attempt().await {
            Err(DeleteError::Record(error)) if tries < WRITE_ATTEMPTS => {
                tracing::debug!(%error, tries, "deletion record retry");
            }
            other => return other.map_err(Into::into),
        }
    }
}

struct DeleteTarget<'a> {
    action_id: Uuid,
    lease_token: Uuid,
    community_id: CommunityId,
    target_event_id: &'a [u8],
    parent_event_id: Option<&'a [u8]>,
    root_event_id: Option<&'a [u8]>,
}

enum DeleteError {
    /// The record write, or the commit after it, failed; nothing was committed.
    Record(DbError),
    Other(DbError),
}

impl From<DbError> for DeleteError {
    fn from(error: DbError) -> Self {
        Self::Other(error)
    }
}

impl From<sqlx::Error> for DeleteError {
    fn from(error: sqlx::Error) -> Self {
        Self::Other(error.into())
    }
}

impl From<DeleteError> for DbError {
    fn from(error: DeleteError) -> Self {
        match error {
            DeleteError::Record(error) | DeleteError::Other(error) => error,
        }
    }
}

/// Write the record for the target just deleted in `tx`, then commit. Both
/// failures count as record failures when a record was due.
async fn record_and_commit(
    mut tx: Transaction<'_, Postgres>,
    community_id: CommunityId,
    target_id: &[u8],
    record_keys: Option<&Keys>,
) -> std::result::Result<Option<Event>, DeleteError> {
    let Some(keys) = record_keys else {
        tx.commit().await?;
        return Ok(None);
    };
    let record = record_deletion_in_tx(&mut tx, community_id, target_id, keys)
        .await
        .map_err(DeleteError::Record)?;
    match tx.commit().await {
        Ok(()) => Ok(record),
        // Deferred checks (the 0021 created_at fence) run at commit and can
        // reject the record; retry with a fresh signature.
        Err(error) if record.is_some() => Err(DeleteError::Record(error.into())),
        Err(error) => Err(error.into()),
    }
}

async fn delete_once(
    pool: &PgPool,
    target: &DeleteTarget<'_>,
    record_keys: Option<&Keys>,
) -> std::result::Result<AdminDeleteOutcome, DeleteError> {
    let lost = AdminDeleteOutcome {
        committed: false,
        record: None,
    };
    let mut tx = pool.begin().await?;

    let owned: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM relay_admin_actions
            WHERE id = $1
              AND action_lease_token = $2
              AND action_lease_expires_at > now()
              AND state = 'enforcing'
        )
        "#,
    )
    .bind(target.action_id)
    .bind(target.lease_token)
    .fetch_one(&mut *tx)
    .await?;

    if !owned {
        tx.rollback().await?;
        return Ok(lost);
    }

    // Acquire the action-row lock before evaluating the wall-clock expiry check.
    sqlx::query("SELECT id FROM relay_admin_actions WHERE id = $1 FOR UPDATE")
        .bind(target.action_id)
        .fetch_optional(&mut *tx)
        .await?;

    if record_keys.is_some() {
        lock_channel_for_record(&mut tx, target.community_id, target.target_event_id).await?;
    }

    // Canonical delete + thread_metadata counters, fenced by this transaction.
    // Counters move only when the row transitions to deleted, so a second
    // action against an already-deleted target leaves them unchanged.
    crate::event::soft_delete_event_and_update_thread_in_tx(
        &mut tx,
        target.community_id,
        target.target_event_id,
        target.parent_event_id,
        target.root_event_id,
    )
    .await?;

    let marker = sqlx::query(
        r#"
        UPDATE relay_admin_actions
        SET step_marker = 'mutation_committed', updated_at = now()
        WHERE id = $1
          AND action_lease_token = $2
          AND action_lease_expires_at > clock_timestamp()
          AND state = 'enforcing'
          AND step_marker IS NULL
        "#,
    )
    .bind(target.action_id)
    .bind(target.lease_token)
    .execute(&mut *tx)
    .await?;

    if marker.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(lost);
    }

    let record =
        record_and_commit(tx, target.community_id, target.target_event_id, record_keys).await?;
    Ok(AdminDeleteOutcome {
        committed: true,
        record,
    })
}

/// Soft-delete `target_id` (and its thread counters) and, when `record_keys`
/// is set, store its relay record in the same transaction; see
/// [`Db::soft_delete_event_with_record`].
pub async fn soft_delete_event_with_record(
    pool: &PgPool,
    community_id: CommunityId,
    target_id: &[u8],
    parent_event_id: Option<&[u8]>,
    root_event_id: Option<&[u8]>,
    record_keys: Option<&Keys>,
) -> Result<RecordedDelete> {
    retry_record(|| async move {
        let connection = crate::observability::acquire_writer(
            pool,
            crate::observability::WriterOperation::EventWrite,
        )
        .await?;
        let mut tx = sqlx::Transaction::begin(connection, None).await?;
        if record_keys.is_some() {
            lock_channel_for_record(&mut tx, community_id, target_id).await?;
        }
        let deleted = crate::event::soft_delete_event_and_update_thread_in_tx(
            &mut tx,
            community_id,
            target_id,
            parent_event_id,
            root_event_id,
        )
        .await?;
        // An already-deleted row was recorded (or not) by its first delete.
        let keys = record_keys.filter(|_| deleted);
        let record = record_and_commit(tx, community_id, target_id, keys).await?;
        Ok(RecordedDelete { deleted, record })
    })
    .await
}

impl Db {
    /// Soft-delete `target_id` and decrement its thread counters; when
    /// `record_keys` is set, also store its relay record in the same
    /// transaction (channel lock before the counters).
    ///
    /// The delete commits only with its record: a failed record write or a
    /// commit that rejects it retries with a fresh transaction and signature,
    /// and after [`WRITE_ATTEMPTS`] the error is returned with nothing deleted.
    /// A row that was already deleted is left as it is and gets no record.
    #[datastore_span(name = "soft_delete_event_with_record", system = "postgresql")]
    pub async fn soft_delete_event_with_record(
        &self,
        community_id: CommunityId,
        target_id: &[u8],
        parent_event_id: Option<&[u8]>,
        root_event_id: Option<&[u8]>,
        record_keys: Option<&Keys>,
    ) -> Result<RecordedDelete> {
        soft_delete_event_with_record(
            &self.pool,
            community_id,
            target_id,
            parent_event_id,
            root_event_id,
            record_keys,
        )
        .await
    }

    /// [`execute_delete_with_record`] on this database.
    #[allow(clippy::too_many_arguments)]
    #[datastore_span(name = "execute_delete_with_marker_and_record", system = "postgresql")]
    pub async fn execute_delete_with_marker_and_record(
        &self,
        action_id: Uuid,
        lease_token: Uuid,
        community_id: CommunityId,
        target_event_id: &[u8],
        parent_event_id: Option<&[u8]>,
        root_event_id: Option<&[u8]>,
        record_keys: Option<&Keys>,
    ) -> Result<AdminDeleteOutcome> {
        execute_delete_with_record(
            &self.pool,
            action_id,
            lease_token,
            community_id,
            target_event_id,
            parent_event_id,
            root_event_id,
            record_keys,
        )
        .await
    }

    /// Store a relay record for an already soft-deleted target in its own
    /// transaction, re-signing on each attempt.
    ///
    /// Returns the inserted record (publish it after this returns), or `None`
    /// when no record is needed or one already exists.
    #[datastore_span(name = "write_deletion_record", system = "postgresql")]
    pub async fn write_deletion_record(
        &self,
        community_id: CommunityId,
        target_id: &[u8],
        keys: &Keys,
    ) -> Result<Option<Event>> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = async {
                let mut tx = self.begin_event_write_transaction().await?;
                let record = record_deletion_in_tx(&mut tx, community_id, target_id, keys).await?;
                tx.commit().await?;
                Ok::<_, DbError>(record)
            }
            .await;
            match result {
                Err(error) if attempt < WRITE_ATTEMPTS => {
                    tracing::debug!(%error, attempt, "deletion record write retry");
                }
                other => return other,
            }
        }
    }

    /// Signer of the community's newest relay group-state event (kind
    /// 39000/39002), which the relay signs with its own key.
    #[datastore_span(name = "newest_group_state_signer", system = "postgresql")]
    pub async fn newest_group_state_signer(
        &self,
        community_id: CommunityId,
    ) -> Result<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar(
            "SELECT pubkey FROM events              WHERE community_id = $1 AND kind IN (39000, 39002) AND deleted_at IS NULL              ORDER BY created_at DESC LIMIT 1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?)
    }

    /// One page of soft-deleted content and overlay rows that lack a relay
    /// record, ordered by target id after `after`.
    ///
    /// Candidates: targets of succeeded staff deletes (direct or by report) and
    /// targets of kind:5 deletions signed by someone other than the target's
    /// raw author.
    #[datastore_span(name = "deletion_record_backfill_candidates", system = "postgresql")]
    pub async fn deletion_record_backfill_candidates(
        &self,
        community_id: CommunityId,
        relay_pubkey: &nostr::PublicKey,
        after: Option<&[u8]>,
        limit: u32,
    ) -> Result<Vec<DeletionRecordCandidate>> {
        let rows: Vec<(Vec<u8>, Uuid, i32)> = sqlx::query_as(
            r#"
            WITH candidates AS (
                SELECT COALESCE(a.enforcement_target_event_id, r.target_event_id) AS target
                  FROM relay_admin_actions a
                  LEFT JOIN moderation_reports r
                    ON r.community_id = a.report_community_id AND r.id = a.report_id
                 WHERE a.report_community_id = $1
                   AND a.action = 'delete'
                   AND (a.state = 'succeeded' OR a.step_marker IS NOT NULL)
                UNION
                SELECT decode(tag->>1, 'hex')
                  FROM events d
                 CROSS JOIN LATERAL jsonb_array_elements(d.tags) tag
                 WHERE d.community_id = $1 AND d.kind = 5 AND d.channel_id IS NOT NULL
                   AND tag->>0 = 'e' AND tag->>1 ~ '^[0-9a-f]{64}$'
                   AND EXISTS (SELECT 1 FROM events t
                                WHERE t.community_id = $1 AND t.id = decode(tag->>1, 'hex')
                                  AND t.pubkey <> d.pubkey)
            )
            SELECT DISTINCT ON (t.id) t.id, t.channel_id, t.kind
              FROM candidates c
              JOIN events t ON t.community_id = $1 AND t.id = c.target
             WHERE c.target IS NOT NULL
               AND ($2::bytea IS NULL OR c.target > $2)
               AND t.deleted_at IS NOT NULL AND t.channel_id IS NOT NULL
               AND t.kind = ANY($3)
               AND NOT EXISTS (
                   SELECT 1 FROM events rec
                    WHERE rec.community_id = $1 AND rec.channel_id = t.channel_id
                      AND rec.kind = 9005 AND rec.pubkey = $4
                      AND rec.created_at >= LEAST(t.created_at, now()) - interval '1 day'
                      AND rec.tags @> jsonb_build_array(
                          jsonb_build_array('e', encode(t.id, 'hex'))))
             ORDER BY t.id
             LIMIT $5
            "#,
        )
        .bind(community_id.as_uuid())
        .bind(after)
        .bind(&DELETION_RECORD_KINDS[..])
        .bind(relay_pubkey.to_bytes().as_slice())
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(target_id, channel_id, kind)| DeletionRecordCandidate {
                target_id,
                channel_id,
                kind,
            })
            .collect())
    }
}
