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
    /// Record that `uploader` uploaded the blob `sha256`. Idempotent.
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
             ON CONFLICT DO NOTHING",
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
                                          AND cm.removed_at IS NULL)))))
               OR EXISTS (
                    SELECT 1 FROM communities co
                    WHERE co.id = $1 AND co.icon LIKE '%/media/' || h.sha || '%')
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
