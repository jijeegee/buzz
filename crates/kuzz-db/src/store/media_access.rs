//! Attachment permissions (`media_uploads`, `media_references`).
//!
//! A blob is readable by the people who uploaded it (and their agents, so an
//! agent can publish an avatar its owner uploaded) and by the people who can
//! currently read an event that references it: a live channel they are an
//! active member of, an open channel, or a community-wide event (NULL
//! channel). Deleting the event, deleting the channel, or leaving a private
//! channel takes access away. The community icon is readable by every member.

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::Db;

/// Most blob hashes one event may link, and one access check may name.
pub const MAX_MEDIA_HASHES_PER_EVENT: usize = 64;

/// Most blob hashes one cleanup check may name.
pub const MAX_CLEANUP_HASHES_PER_CALL: usize = 1000;

fn check_hashes(hashes: &[String]) -> Result<()> {
    if hashes.len() > MAX_MEDIA_HASHES_PER_EVENT {
        return Err(DbError::InvalidData(format!(
            "at most {MAX_MEDIA_HASHES_PER_EVENT} media hashes per call"
        )));
    }
    if !hashes
        .iter()
        .all(|h| h.len() == 64 && h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
    {
        return Err(DbError::InvalidData(
            "media hash must be 64 lowercase hex".into(),
        ));
    }
    Ok(())
}

fn check_key(name: &str, key: &[u8]) -> Result<()> {
    if key.len() != 32 {
        return Err(DbError::InvalidData(format!("{name} must be 32 bytes")));
    }
    Ok(())
}

impl Db {
    /// Record that `uploader` uploaded the blob `sha256`. Uploading the same
    /// bytes again refreshes the time, so the unused-file cleanup waits its
    /// grace period from the latest upload.
    #[datastore_span(name = "record_media_upload", system = "postgresql")]
    pub async fn record_media_upload(
        &self,
        community: CommunityId,
        sha256: &str,
        uploader: &[u8],
    ) -> Result<()> {
        check_hashes(std::slice::from_ref(&sha256.to_owned()))?;
        check_key("uploader", uploader)?;
        sqlx::query(
            "INSERT INTO media_uploads (community_id, sha256, uploader) VALUES ($1, $2, $3) \
             ON CONFLICT (community_id, sha256, uploader) DO UPDATE SET created_at = now()",
        )
        .bind(community.as_uuid())
        .bind(sha256)
        .bind(uploader)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Link each blob in `hashes` to a stored event. `channel_id` NULL marks a
    /// community-wide event. Idempotent.
    #[datastore_span(name = "record_media_references", system = "postgresql")]
    pub async fn record_media_references(
        &self,
        community: CommunityId,
        event_id: &[u8],
        event_created_at: DateTime<Utc>,
        channel_id: Option<Uuid>,
        hashes: &[String],
    ) -> Result<()> {
        if hashes.is_empty() {
            return Ok(());
        }
        check_hashes(hashes)?;
        check_key("event id", event_id)?;
        sqlx::query(
            "INSERT INTO media_references \
               (community_id, sha256, event_id, event_created_at, channel_id) \
             SELECT $1, h, $3, $4, $5 FROM unnest($2::text[]) AS h \
             ON CONFLICT DO NOTHING",
        )
        .bind(community.as_uuid())
        .bind(hashes)
        .bind(event_id)
        .bind(event_created_at)
        .bind(channel_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The subset of `hashes` that `reader` may open right now.
    #[datastore_span(name = "media_hashes_readable_by", system = "postgresql")]
    pub async fn media_hashes_readable_by(
        &self,
        community: CommunityId,
        reader: &[u8],
        hashes: &[String],
    ) -> Result<Vec<String>> {
        if hashes.is_empty() {
            return Ok(Vec::new());
        }
        check_hashes(hashes)?;
        check_key("reader", reader)?;
        let readable: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT h.sha FROM unnest($2::text[]) AS h(sha)
            WHERE EXISTS (
                    SELECT 1 FROM media_uploads u
                    WHERE u.community_id = $1 AND u.sha256 = h.sha
                      AND (u.uploader = $3
                           OR u.uploader = (
                                SELECT us.agent_owner_pubkey FROM users us
                                WHERE us.community_id = $1 AND us.pubkey = $3)))
               OR EXISTS (
                    SELECT 1
                    FROM media_references r
                    JOIN events e
                      ON e.community_id = r.community_id
                     AND e.created_at = r.event_created_at
                     AND e.id = r.event_id
                     AND e.deleted_at IS NULL
                    LEFT JOIN channels c
                      ON c.community_id = r.community_id AND c.id = r.channel_id
                    WHERE r.community_id = $1 AND r.sha256 = h.sha
                      AND (r.channel_id IS NULL
                           OR (c.id IS NOT NULL AND c.deleted_at IS NULL
                               AND (c.visibility = 'open'
                                    OR EXISTS (
                                        SELECT 1 FROM channel_members cm
                                        WHERE cm.community_id = $1
                                          AND cm.channel_id = r.channel_id
                                          AND cm.pubkey = $3
                                          AND cm.removed_at IS NULL))))
                      -- A message from before the reader's join does not
                      -- grant the file (since_join rooms).
                      AND NOT channel_event_before_join($1, r.channel_id,
                                                        event_history_sequence(e.channel_sequence,
                                                            e.thread_root_sequence), $3)
                      AND NOT channel_event_targets_before_join($1, r.channel_id, e.kind,
                                                                e.tags, $3))
               OR EXISTS (
                    SELECT 1 FROM communities co
                    WHERE co.id = $1 AND co.icon LIKE '%/media/' || h.sha || '%')
               OR EXISTS (
                    -- A group photo is shown to people who are not members yet
                    -- (invitations, invite links), so any signed-in reader may
                    -- open it; the hash is unguessable.
                    SELECT 1 FROM channels gc
                    WHERE gc.community_id = $1 AND gc.deleted_at IS NULL
                      AND gc.picture LIKE '%/media/' || h.sha || '%')
            "#,
        )
        .bind(community.as_uuid())
        .bind(hashes)
        .bind(reader)
        .fetch_all(&self.pool)
        .await?;
        Ok(readable)
    }

    /// Whether `reader` may open the blob `sha256` right now.
    pub async fn media_readable_by(
        &self,
        community: CommunityId,
        reader: &[u8],
        sha256: &str,
    ) -> Result<bool> {
        let readable = self
            .media_hashes_readable_by(community, reader, &[sha256.to_owned()])
            .await?;
        Ok(!readable.is_empty())
    }
}

impl Db {
    /// The subset of `hashes` that is still in use in `community`, for the
    /// unused-file cleanup. Errs toward keeping: a blob is in use if it was
    /// uploaded after `grace_cutoff`, or any event that is live or was deleted
    /// after `grace_cutoff` names it anywhere in its content or tags (every
    /// kind, including private ones the read links skip), or a reported event,
    /// a profile picture, the community icon, a group photo, a blob report, a feedback
    /// attachment or a workflow definition names it.
    ///
    /// The indexed checks run first; only the hashes they leave are matched
    /// against event text, in one pass over the community's events that pulls
    /// out every run of 64 or more hex digits and joins it to those hashes.
    #[datastore_span(name = "media_hashes_in_use", system = "postgresql")]
    pub async fn media_hashes_in_use(
        &self,
        community: CommunityId,
        hashes: &[String],
        grace_cutoff: DateTime<Utc>,
    ) -> Result<Vec<String>> {
        if hashes.is_empty() {
            return Ok(Vec::new());
        }
        if hashes.len() > MAX_CLEANUP_HASHES_PER_CALL {
            return Err(DbError::InvalidData(format!(
                "at most {MAX_CLEANUP_HASHES_PER_CALL} media hashes per cleanup check"
            )));
        }
        for chunk in hashes.chunks(MAX_MEDIA_HASHES_PER_EVENT) {
            check_hashes(chunk)?;
        }
        let mut tx = self.pool.begin().await?;
        // A background job must never hold the database for long.
        sqlx::query("SET LOCAL statement_timeout = '120s'")
            .execute(&mut *tx)
            .await?;
        let in_use: Vec<String> = sqlx::query_scalar(
            r#"
            WITH h AS (SELECT DISTINCT unnest($2::text[]) AS sha),
            cheap AS (
                SELECT h.sha FROM h
                WHERE EXISTS (
                        SELECT 1 FROM media_uploads u
                        WHERE u.community_id = $1 AND u.sha256 = h.sha AND u.created_at > $3)
                   OR EXISTS (
                        SELECT 1 FROM media_references r
                        JOIN events e
                          ON e.community_id = r.community_id
                         AND e.created_at = r.event_created_at
                         AND e.id = r.event_id
                        WHERE r.community_id = $1 AND r.sha256 = h.sha
                          AND (e.deleted_at IS NULL OR e.deleted_at > $3))
                   OR EXISTS (
                        SELECT 1 FROM moderation_reports m
                        WHERE m.community_id = $1
                          AND m.target_blob_sha256 = decode(h.sha, 'hex'))
                   OR EXISTS (
                        SELECT 1 FROM moderation_reports m
                        JOIN events e
                          ON e.community_id = m.community_id AND e.id = m.target_event_id
                        WHERE m.community_id = $1
                          AND (strpos(e.content, h.sha) > 0 OR strpos(e.tags::text, h.sha) > 0))
                   OR EXISTS (
                        SELECT 1 FROM communities co
                        WHERE co.id = $1 AND strpos(COALESCE(co.icon, ''), h.sha) > 0)
                   OR EXISTS (
                        SELECT 1 FROM channels gc
                        WHERE gc.community_id = $1
                          AND strpos(COALESCE(gc.picture, ''), h.sha) > 0)
                   OR EXISTS (
                        SELECT 1 FROM users us
                        WHERE us.community_id = $1
                          AND strpos(COALESCE(us.avatar_url, ''), h.sha) > 0)
                   OR EXISTS (
                        SELECT 1 FROM principals p
                        WHERE strpos(COALESCE(p.avatar_url, ''), h.sha) > 0)
                   OR EXISTS (
                        SELECT 1 FROM product_feedback f
                        WHERE f.community_id = $1 AND strpos(f.tags::text, h.sha) > 0)
                   OR EXISTS (
                        SELECT 1 FROM workflows w
                        WHERE w.community_id = $1 AND strpos(w.definition::text, h.sha) > 0)
            ),
            rest AS (SELECT sha FROM h EXCEPT SELECT sha FROM cheap),
            runs AS (
                SELECT DISTINCT m.run[1] AS run
                FROM events e
                CROSS JOIN LATERAL regexp_matches(
                    e.content || ' ' || e.tags::text, '[0-9a-f]{64,}', 'g') AS m(run)
                WHERE e.community_id = $1
                  AND (e.deleted_at IS NULL OR e.deleted_at > $3)
                  AND EXISTS (SELECT 1 FROM rest)
            ),
            scanned AS (
                SELECT rest.sha FROM rest JOIN runs ON runs.run = rest.sha
                UNION
                SELECT rest.sha FROM rest
                JOIN runs ON length(runs.run) > 64 AND strpos(runs.run, rest.sha) > 0
            )
            SELECT sha FROM cheap UNION SELECT sha FROM scanned
            "#,
        )
        .bind(community.as_uuid())
        .bind(hashes)
        .bind(grace_cutoff)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(in_use)
    }

    /// Drop `community`'s stale uploader and reference rows for a cleaned-up
    /// blob: uploads from before `grace_cutoff`, and references whose event is
    /// gone or was deleted before it. Rows written meanwhile stay.
    #[datastore_span(name = "forget_media", system = "postgresql")]
    pub async fn forget_media(
        &self,
        community: CommunityId,
        sha256: &str,
        grace_cutoff: DateTime<Utc>,
    ) -> Result<()> {
        check_hashes(std::slice::from_ref(&sha256.to_owned()))?;
        let mut tx = self.pool.begin().await?;
        for statement in [
            "DELETE FROM media_uploads \
             WHERE community_id = $1 AND sha256 = $2 AND created_at <= $3",
            "DELETE FROM media_references r \
             WHERE r.community_id = $1 AND r.sha256 = $2 \
               AND NOT EXISTS ( \
                   SELECT 1 FROM events e \
                   WHERE e.community_id = r.community_id \
                     AND e.created_at = r.event_created_at \
                     AND e.id = r.event_id \
                     AND (e.deleted_at IS NULL OR e.deleted_at > $3))",
        ] {
            sqlx::query(statement)
                .bind(community.as_uuid())
                .bind(sha256)
                .bind(grace_cutoff)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Every community id, whatever its state. The cleanup checks each one's
    /// sidecar before removing shared blob bytes.
    #[datastore_span(name = "all_community_ids", system = "postgresql")]
    pub async fn all_community_ids(&self) -> Result<Vec<Uuid>> {
        Ok(sqlx::query_scalar("SELECT id FROM communities")
            .fetch_all(&self.pool)
            .await?)
    }
}
