//! Room history cut (`channels.history_policy = 'since_join'`).
//!
//! In a members-only room with `since_join`, a reader sees only channel
//! events after their current join (`channel_members.join_sequence`). Open
//! rooms and legacy-stamped memberships keep full history. The rule lives in
//! SQL (`channel_history_floor`, `channel_event_before_join`, migration
//! 0081); these lookups serve read paths that cannot fold it into their own
//! query (lookups by id, stored summary counters).

use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::Db;

/// Most event ids one [`Db::history_hidden_event_ids`] call may name.
pub const MAX_HISTORY_EVENT_LOOKUP: usize = 5000;

/// Current room state (never cut). Mirrors SQL `channel_kind_is_room_state`
/// (migration 0081).
pub fn is_room_state_kind(kind: i32) -> bool {
    matches!(kind, 9007 | 40100 | 40110 | 10_000..=19_999 | 30_000..=39_999)
}

/// A quoted original for the quote box ([`Db::quote_preview`]).
#[derive(Debug, Clone)]
pub struct QuotePreview {
    /// Author pubkey of the quoted message.
    pub author: Vec<u8>,
    /// Its kind.
    pub kind: i32,
    /// When it was signed.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Its tags (attachments decide a text-less excerpt).
    pub tags: serde_json::Value,
    /// Its current text (the latest edit by its author).
    pub content: String,
}

impl Db {
    /// The channel sequence at or below which `reader` sees nothing in
    /// `channel`; `0` when the whole history is visible.
    #[datastore_span(name = "channel_history_floor", system = "postgresql")]
    pub async fn channel_history_floor(
        &self,
        community: CommunityId,
        channel: Uuid,
        reader: &[u8],
    ) -> Result<i64> {
        let floor: i64 = sqlx::query_scalar("SELECT channel_history_floor($1, $2, $3)")
            .bind(community.as_uuid())
            .bind(channel)
            .bind(reader)
            .fetch_one(&self.pool)
            .await?;
        Ok(floor)
    }

    /// The room's stored history policy: `all` or `since_join`. Group chats
    /// always cut whatever this says (`channel_cuts_history`).
    #[datastore_span(name = "channel_history_policy", system = "postgresql")]
    pub async fn channel_history_policy(
        &self,
        community: CommunityId,
        channel: Uuid,
    ) -> Result<String> {
        let policy: String = sqlx::query_scalar(
            "SELECT history_policy FROM channels WHERE community_id = $1 AND id = $2",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .fetch_one(&self.pool)
        .await?;
        Ok(policy)
    }

    /// Switch a room's history policy (`all` / `since_join`). Switching to
    /// `since_join` keeps what current members can already read (their
    /// memberships become legacy: full history); only later joins are cut.
    /// Stored unread counts are recounted either way.
    #[datastore_span(name = "set_channel_history_policy", system = "postgresql")]
    pub async fn set_channel_history_policy(
        &self,
        community: CommunityId,
        channel: Uuid,
        policy: &str,
    ) -> Result<()> {
        if policy != "all" && policy != "since_join" {
            return Err(DbError::InvalidData(format!(
                "invalid history policy: {policy}"
            )));
        }
        let mut tx = self.pool.begin().await?;
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT history_policy FROM channels WHERE community_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(previous) = previous else {
            return Err(DbError::ChannelNotFound(channel));
        };
        if previous == policy {
            return Ok(());
        }
        sqlx::query("UPDATE channels SET history_policy = $3 WHERE community_id = $1 AND id = $2")
            .bind(community.as_uuid())
            .bind(channel)
            .bind(policy)
            .execute(&mut *tx)
            .await?;
        if policy == "since_join" {
            // The membership trigger recounts each promoted row.
            sqlx::query(
                "UPDATE channel_members SET join_cutoff_is_legacy = true \
                  WHERE community_id = $1 AND channel_id = $2 AND removed_at IS NULL \
                    AND NOT join_cutoff_is_legacy",
            )
            .bind(community.as_uuid())
            .bind(channel)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE channel_members cm SET (unread_count, unread_mentions) = ( \
                     SELECT u.unread, u.mentions \
                       FROM chat_room_unread(cm.community_id, cm.channel_id, cm.pubkey, cm.read_seq) u) \
                  WHERE cm.community_id = $1 AND cm.channel_id = $2 AND cm.removed_at IS NULL \
                    AND NOT cm.join_cutoff_is_legacy",
            )
            .bind(community.as_uuid())
            .bind(channel)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// The quoted original behind a quote the reader can see, for the quote
    /// box (plan §9: a pre-join original shows only there). `None` unless
    /// `quoting` is a live message in `channel` the reader can read, it
    /// carries `["q", quoted]`, and `quoted` is an earlier live message in
    /// the same channel. Content is the latest edit by its author.
    #[datastore_span(name = "quote_preview", system = "postgresql")]
    pub async fn quote_preview(
        &self,
        community: CommunityId,
        channel: Uuid,
        reader: &[u8],
        quoting: &[u8],
        quoted: &[u8],
    ) -> Result<Option<QuotePreview>> {
        let row = sqlx::query(
            "SELECT q.pubkey, q.kind, q.created_at, q.tags, \
                    COALESCE((SELECT ed.content FROM events ed \
                               WHERE ed.community_id = q.community_id \
                                 AND ed.channel_id = q.channel_id AND ed.kind = 40003 \
                                 AND ed.pubkey = q.pubkey AND ed.deleted_at IS NULL \
                                 AND ed.tags @> jsonb_build_array(jsonb_build_array('e', encode(q.id, 'hex'))) \
                               ORDER BY ed.created_at DESC, ed.id DESC LIMIT 1), q.content) AS content \
               FROM events m \
               JOIN events q ON q.community_id = m.community_id AND q.id = $5 \
              WHERE m.community_id = $1 AND m.id = $4 AND m.channel_id = $2 \
                AND m.deleted_at IS NULL \
                AND m.kind IN (9, 40002, 45001, 45003) \
                AND NOT channel_event_before_join(m.community_id, m.channel_id, \
                      event_history_sequence(m.channel_sequence, m.thread_root_sequence), $3) \
                AND EXISTS (SELECT 1 FROM jsonb_array_elements(m.tags) t \
                             WHERE t->>0 = 'q' AND lower(t->>1) = encode($5, 'hex')) \
                AND q.channel_id = m.channel_id AND q.deleted_at IS NULL \
                AND q.kind IN (9, 40002, 45001, 45003) \
                AND q.channel_sequence < m.channel_sequence \
              LIMIT 1",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .bind(reader)
        .bind(quoting)
        .bind(quoted)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(QuotePreview {
                author: row.try_get("pubkey")?,
                kind: row.try_get("kind")?,
                created_at: row.try_get("created_at")?,
                tags: row.try_get("tags")?,
                content: row.try_get("content")?,
            })
        })
        .transpose()
    }

    /// Which of `event_ids` were written before `reader` joined their room
    /// (and so are "not found" to `reader`).
    #[datastore_span(name = "history_hidden_event_ids", system = "postgresql")]
    pub async fn history_hidden_event_ids(
        &self,
        community: CommunityId,
        reader: &[u8],
        event_ids: &[Vec<u8>],
    ) -> Result<Vec<Vec<u8>>> {
        if event_ids.is_empty() {
            return Ok(Vec::new());
        }
        if event_ids.len() > MAX_HISTORY_EVENT_LOOKUP {
            return Err(DbError::InvalidData(format!(
                "at most {MAX_HISTORY_EVENT_LOOKUP} events per history lookup"
            )));
        }
        let rows = sqlx::query(
            "SELECT DISTINCT e.id FROM events e \
              WHERE e.community_id = $1 AND e.id = ANY($3) \
                AND channel_event_before_join(e.community_id, e.channel_id, \
                    event_history_sequence(e.channel_sequence, e.thread_root_sequence), $2)",
        )
        .bind(community.as_uuid())
        .bind(reader)
        .bind(event_ids)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| row.try_get::<Vec<u8>, _>("id").map_err(Into::into))
            .collect()
    }

    /// Of the `(event, reader)` pairs, those where `reader` cannot read the
    /// stored channel event: before their join, a reply in a thread from
    /// before it, or a reaction/edit/deletion/summary about such an event.
    /// Push wakes use it.
    #[datastore_span(name = "history_hidden_pairs", system = "postgresql")]
    pub async fn history_hidden_pairs(
        &self,
        community: CommunityId,
        event_ids: &[Vec<u8>],
        readers: &[Vec<u8>],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        if event_ids.is_empty() || event_ids.len() != readers.len() {
            return Ok(Vec::new());
        }
        if event_ids.len() > MAX_HISTORY_EVENT_LOOKUP {
            return Err(DbError::InvalidData(format!(
                "at most {MAX_HISTORY_EVENT_LOOKUP} pairs per history lookup"
            )));
        }
        let rows = sqlx::query(
            "SELECT p.id, p.pk FROM unnest($2::bytea[], $3::bytea[]) AS p(id, pk) \
              WHERE EXISTS (SELECT 1 FROM events e \
                     WHERE e.community_id = $1 AND e.id = p.id AND e.channel_id IS NOT NULL \
                       AND NOT channel_kind_is_room_state(e.kind) \
                       AND (channel_event_before_join(e.community_id, e.channel_id, \
                              event_history_sequence(e.channel_sequence, e.thread_root_sequence), \
                              p.pk) \
                            OR channel_event_targets_before_join(e.community_id, e.channel_id, \
                              e.kind, e.tags, p.pk)))",
        )
        .bind(community.as_uuid())
        .bind(event_ids)
        .bind(readers)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get::<Vec<u8>, _>("id")?,
                    row.try_get::<Vec<u8>, _>("pk")?,
                ))
            })
            .collect()
    }

    /// Of `readers`, those for whom any of `targets` that lives in `channel`
    /// is hidden: written before their join, or a reply in a thread whose
    /// root was (migration 0082). Non-members of an open room that cuts
    /// history are hidden from everything. Live delivery uses it with the
    /// event itself and, for a reaction, edit, deletion or summary, what it
    /// is about. Targets outside `channel` are ignored; none in it hides
    /// nothing.
    #[datastore_span(name = "readers_blind_to_targets", system = "postgresql")]
    pub async fn readers_blind_to_targets(
        &self,
        community: CommunityId,
        channel: Uuid,
        targets: &[Vec<u8>],
        readers: &[Vec<u8>],
    ) -> Result<Vec<Vec<u8>>> {
        if targets.is_empty() || readers.is_empty() {
            return Ok(Vec::new());
        }
        if targets.len() > MAX_HISTORY_EVENT_LOOKUP || readers.len() > MAX_HISTORY_EVENT_LOOKUP {
            return Err(DbError::InvalidData(format!(
                "at most {MAX_HISTORY_EVENT_LOOKUP} targets and readers per history lookup"
            )));
        }
        let rows = sqlx::query(
            "WITH t AS (SELECT min(event_history_sequence(e.channel_sequence, e.thread_root_sequence)) AS seq FROM events e \
                         WHERE e.community_id = $1 AND e.channel_id = $2 AND e.id = ANY($3)) \
             SELECT r.pk FROM unnest($4::bytea[]) AS r(pk), t \
              WHERE t.seq IS NOT NULL AND t.seq <= channel_history_floor($1, $2, r.pk)",
        )
        .bind(community.as_uuid())
        .bind(channel)
        .bind(targets)
        .bind(readers)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| row.try_get::<Vec<u8>, _>("pk").map_err(Into::into))
            .collect()
    }
}
