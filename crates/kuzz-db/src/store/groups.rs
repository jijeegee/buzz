//! Groups: public-group search and addresses (`channels.handle`), invite
//! links (`group_invite_links`), "who can add me" (`group_add_settings`) and
//! invitations (`group_invitations`).
//!
//! A group is a `stream` or `forum` channel; DMs and workflow channels never
//! appear here. Member counts are active rows (`removed_at IS NULL`), humans
//! and agents alike.

use chrono::{DateTime, Utc};
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::Db;

/// Most groups one search returns.
pub const MAX_GROUP_SEARCH: i64 = 20;

/// Days a pending invitation stays usable.
pub const INVITATION_TTL_DAYS: i32 = 30;

/// Days after a decline during which the same group cannot re-invite.
pub const DECLINE_COOLDOWN_DAYS: i32 = 7;

/// Most invitations one inviter may create in one community per 24 hours.
pub const INVITATIONS_PER_DAY: i64 = 50;

/// One group as the groups API shows it to `viewer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSummary {
    /// Channel id.
    pub id: Uuid,
    /// Group name.
    pub name: String,
    /// Description, if any.
    pub description: Option<String>,
    /// Public address (open groups only).
    pub handle: Option<String>,
    /// Group photo URL (a `/media` blob on this relay), if set.
    pub picture: Option<String>,
    /// `stream` or `forum` (or `dm`/`workflow` for direct lookups).
    pub channel_type: String,
    /// `open` or `private`.
    pub visibility: String,
    /// `members` or `admins`.
    pub invite_policy: String,
    /// Active members.
    pub member_count: i64,
    /// Whether the viewer is an active member.
    pub is_member: bool,
    /// When the group was archived, if it is.
    pub archived_at: Option<DateTime<Utc>>,
    /// Ownerless group chat (never searchable, no address or invite link).
    pub group_chat: bool,
}

/// The active invite link of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInviteLink {
    /// Group (channel) id.
    pub channel_id: Uuid,
    /// URL-safe code.
    pub code: String,
    /// Who created it.
    pub created_by: Vec<u8>,
    /// When it was created.
    pub created_at: DateTime<Utc>,
}

/// A pending invitation addressed to the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInvitation {
    /// Invitation id.
    pub id: Uuid,
    /// The group.
    pub channel: GroupSummary,
    /// Who invited.
    pub inviter: Vec<u8>,
    /// When the invitation was (last) sent.
    pub created_at: DateTime<Utc>,
    /// Group chats only: up to [`INVITATION_MEMBER_PREVIEW`] active members
    /// (never the invitee), oldest first, so an unnamed chat has a title.
    pub member_pubkeys: Vec<Vec<u8>>,
}

/// Most members an invitation to a group chat previews.
pub const INVITATION_MEMBER_PREVIEW: i64 = 4;

/// See [`Db::group_invitation_write_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvitationWriteState {
    /// The invitee already has an unexpired pending invitation to the group.
    pub already_pending: bool,
    /// The invitee declined one to this group within the cooldown.
    pub declined_recently: bool,
    /// Invitations the inviter created or refreshed in the last 24 hours.
    pub sent_last_day: i64,
}

/// One pending invitation row, before the group is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInvitation {
    /// Group (channel) id.
    pub channel_id: Uuid,
    /// Who invited.
    pub inviter: Vec<u8>,
}

/// Outcome of setting a group's public address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetHandleOutcome {
    /// Stored (or cleared).
    Set,
    /// Another live group of the community already uses it.
    Taken,
    /// The group does not exist or is deleted.
    NotFound,
}

/// Escape `%`, `_` and `\` and wrap for a case-insensitive substring match.
fn like_contains(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('%');
    for ch in text.to_lowercase().chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}

fn check_key(name: &str, key: &[u8]) -> Result<()> {
    if key.len() != 32 {
        return Err(DbError::InvalidData(format!("{name} must be 32 bytes")));
    }
    Ok(())
}

/// Columns of [`GroupSummary`] for channel alias `c`; `$2` is the viewer.
macro_rules! summary_columns {
    () => {
        "c.id, c.name, c.description, c.handle, c.picture, c.channel_type::text AS channel_type, \
         c.visibility::text AS visibility, c.invite_policy, c.archived_at, c.created_at,          c.group_chat, \
         (SELECT count(*) FROM channel_members m \
           WHERE m.community_id = c.community_id AND m.channel_id = c.id \
             AND m.removed_at IS NULL) AS member_count, \
         EXISTS (SELECT 1 FROM channel_members m \
           WHERE m.community_id = c.community_id AND m.channel_id = c.id \
             AND m.pubkey = $2 AND m.removed_at IS NULL) AS is_member"
    };
}

/// Live open groups (search and address lookups).
macro_rules! open_group_filter {
    () => {
        "c.community_id = $1 AND c.visibility = 'open' AND c.archived_at IS NULL \
         AND c.deleted_at IS NULL AND c.channel_type IN ('stream', 'forum')          AND NOT c.group_chat"
    };
}

const SEARCH_SQL: &str = concat!(
    "SELECT ",
    summary_columns!(),
    " FROM channels c WHERE ",
    open_group_filter!(),
    " AND (lower(c.name) LIKE $3 ESCAPE '\\' \
        OR lower(coalesce(c.description, '')) LIKE $3 ESCAPE '\\' \
        OR coalesce(c.handle, '') LIKE $3 ESCAPE '\\') \
      ORDER BY (lower(c.name) = $4 OR coalesce(c.handle, '') = $4) DESC, \
               member_count DESC, c.created_at DESC, c.id \
      LIMIT $5"
);

const BY_HANDLE_SQL: &str = concat!(
    "SELECT ",
    summary_columns!(),
    " FROM channels c WHERE ",
    open_group_filter!(),
    " AND c.handle IS NOT NULL AND lower(c.handle) = lower($3)"
);

const BY_ID_SQL: &str = concat!(
    "SELECT ",
    summary_columns!(),
    " FROM channels c WHERE c.community_id = $1 AND c.id = $3 AND c.deleted_at IS NULL"
);

const INVITATIONS_SQL: &str = concat!(
    "SELECT i.id AS invitation_id, i.inviter, i.created_at AS invited_at,        CASE WHEN c.group_chat THEN ARRAY(SELECT m.pubkey FROM channel_members m            WHERE m.community_id = c.community_id AND m.channel_id = c.id              AND m.removed_at IS NULL AND m.pubkey <> $2            ORDER BY m.joined_at, m.pubkey LIMIT $4)        ELSE ARRAY[]::bytea[] END AS member_pubkeys, ",
    summary_columns!(),
    " FROM group_invitations i \
      JOIN channels c ON c.community_id = i.community_id AND c.id = i.channel_id \
     WHERE i.community_id = $1 AND i.invitee = $2 AND i.status = 'pending' \
       AND i.created_at > now() - make_interval(days => $3) \
       AND c.deleted_at IS NULL AND c.archived_at IS NULL \
       AND NOT EXISTS (SELECT 1 FROM user_blocks b \
             WHERE b.community_id = i.community_id AND b.blocker = i.invitee \
               AND b.blocked = i.inviter) \
     ORDER BY i.created_at DESC, i.id \
     LIMIT 200"
);

fn summary_from_row(row: &sqlx::postgres::PgRow) -> Result<GroupSummary> {
    Ok(GroupSummary {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        handle: row.try_get("handle")?,
        picture: row.try_get("picture")?,
        channel_type: row.try_get("channel_type")?,
        visibility: row.try_get("visibility")?,
        invite_policy: row.try_get("invite_policy")?,
        member_count: row.try_get("member_count")?,
        is_member: row.try_get("is_member")?,
        archived_at: row.try_get("archived_at")?,
        group_chat: row.try_get("group_chat")?,
    })
}

fn link_from_row(row: &sqlx::postgres::PgRow) -> Result<GroupInviteLink> {
    Ok(GroupInviteLink {
        channel_id: row.try_get("channel_id")?,
        code: row.try_get("code")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
    })
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

impl Db {
    /// Live open groups whose name, description or address contains `text`
    /// (case-insensitive). Exact name/address matches first, then larger
    /// groups, then newer ones.
    #[datastore_span(name = "search_open_groups", system = "postgresql")]
    pub async fn search_open_groups(
        &self,
        community: CommunityId,
        viewer: &[u8],
        text: &str,
        limit: i64,
    ) -> Result<Vec<GroupSummary>> {
        let rows = sqlx::query(SEARCH_SQL)
            .bind(community.as_uuid())
            .bind(viewer)
            .bind(like_contains(text))
            .bind(text.to_lowercase())
            .bind(limit.clamp(1, MAX_GROUP_SEARCH))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(summary_from_row).collect()
    }

    /// The live open group with this address, if any.
    #[datastore_span(name = "open_group_by_handle", system = "postgresql")]
    pub async fn open_group_by_handle(
        &self,
        community: CommunityId,
        viewer: &[u8],
        handle: &str,
    ) -> Result<Option<GroupSummary>> {
        let row = sqlx::query(BY_HANDLE_SQL)
            .bind(community.as_uuid())
            .bind(viewer)
            .bind(handle)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(summary_from_row).transpose()
    }

    /// Any non-deleted channel as a group summary for `viewer`.
    #[datastore_span(name = "group_summary", system = "postgresql")]
    pub async fn group_summary(
        &self,
        community: CommunityId,
        viewer: &[u8],
        channel_id: Uuid,
    ) -> Result<Option<GroupSummary>> {
        let row = sqlx::query(BY_ID_SQL)
            .bind(community.as_uuid())
            .bind(viewer)
            .bind(channel_id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(summary_from_row).transpose()
    }

    /// Set or clear a group's public address. The caller validates the text;
    /// setting one on a group that is not open reports `NotFound`.
    #[datastore_span(name = "set_group_handle", system = "postgresql")]
    pub async fn set_group_handle(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        handle: Option<&str>,
    ) -> Result<SetHandleOutcome> {
        let result = sqlx::query(
            "UPDATE channels SET handle = $3 \
              WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL \
                AND ($3::text IS NULL OR visibility = 'open')",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .bind(handle)
        .execute(&self.pool)
        .await;
        match result {
            Ok(done) if done.rows_affected() == 0 => Ok(SetHandleOutcome::NotFound),
            Ok(_) => Ok(SetHandleOutcome::Set),
            Err(error) if is_unique_violation(&error) => Ok(SetHandleOutcome::Taken),
            Err(error) => Err(error.into()),
        }
    }

    /// Set who may share the invite link and add people (`members`/`admins`).
    /// Returns false when the group does not exist.
    #[datastore_span(name = "set_group_invite_policy", system = "postgresql")]
    pub async fn set_group_invite_policy(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        policy: &str,
    ) -> Result<bool> {
        let done = sqlx::query(
            "UPDATE channels SET invite_policy = $3 \
              WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .bind(policy)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// A group's invite policy; `None` when the group does not exist.
    #[datastore_span(name = "group_invite_policy", system = "postgresql")]
    pub async fn group_invite_policy(
        &self,
        community: CommunityId,
        channel_id: Uuid,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT invite_policy FROM channels \
              WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// The group's active (non-revoked) invite link.
    #[datastore_span(name = "active_group_invite_link", system = "postgresql")]
    pub async fn active_group_invite_link(
        &self,
        community: CommunityId,
        channel_id: Uuid,
    ) -> Result<Option<GroupInviteLink>> {
        let row = sqlx::query(
            "SELECT channel_id, code, created_by, created_at FROM group_invite_links \
              WHERE community_id = $1 AND channel_id = $2 AND revoked_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(link_from_row).transpose()
    }

    /// The active invite link with this code.
    #[datastore_span(name = "group_invite_link_by_code", system = "postgresql")]
    pub async fn group_invite_link_by_code(
        &self,
        community: CommunityId,
        code: &str,
    ) -> Result<Option<GroupInviteLink>> {
        let row = sqlx::query(
            "SELECT channel_id, code, created_by, created_at FROM group_invite_links \
              WHERE community_id = $1 AND code = $2 AND revoked_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(code)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(link_from_row).transpose()
    }

    /// Revoke the group's active link (when `expected_code` is given, only if
    /// it is still that code), then make `code` the active link unless a
    /// concurrent caller already made one. Returns the active link.
    #[datastore_span(name = "replace_group_invite_link", system = "postgresql")]
    pub async fn replace_group_invite_link(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        revoke: Option<&str>,
        code: &str,
        created_by: &[u8],
    ) -> Result<GroupInviteLink> {
        check_key("created_by", created_by)?;
        let mut tx = self.pool.begin().await?;
        if let Some(old) = revoke {
            sqlx::query(
                "UPDATE group_invite_links SET revoked_at = now() \
                  WHERE community_id = $1 AND channel_id = $2 AND code = $3 \
                    AND revoked_at IS NULL",
            )
            .bind(community.as_uuid())
            .bind(channel_id)
            .bind(old)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO group_invite_links (community_id, channel_id, code, created_by) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (community_id, channel_id) WHERE revoked_at IS NULL DO NOTHING",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .bind(code)
        .bind(created_by)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(
            "SELECT channel_id, code, created_by, created_at FROM group_invite_links \
              WHERE community_id = $1 AND channel_id = $2 AND revoked_at IS NULL",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        link_from_row(&row)
    }

    /// The reader's "who can add me to groups" setting (`friends` by default).
    #[datastore_span(name = "group_add_policy", system = "postgresql")]
    pub async fn group_add_policy(&self, community: CommunityId, pubkey: &[u8]) -> Result<String> {
        let policy: Option<String> = sqlx::query_scalar(
            "SELECT policy FROM group_add_settings WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community.as_uuid())
        .bind(pubkey)
        .fetch_optional(&self.pool)
        .await?;
        Ok(policy.unwrap_or_else(|| "friends".to_owned()))
    }

    /// Store the reader's "who can add me to groups" setting.
    #[datastore_span(name = "set_group_add_policy", system = "postgresql")]
    pub async fn set_group_add_policy(
        &self,
        community: CommunityId,
        pubkey: &[u8],
        policy: &str,
    ) -> Result<()> {
        check_key("pubkey", pubkey)?;
        sqlx::query(
            "INSERT INTO group_add_settings (community_id, pubkey, policy) VALUES ($1, $2, $3) \
             ON CONFLICT (community_id, pubkey) \
             DO UPDATE SET policy = EXCLUDED.policy, updated_at = now()",
        )
        .bind(community.as_uuid())
        .bind(pubkey)
        .bind(policy)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Create the pending invitation of `invitee` to the group, or refresh
    /// the existing one (new inviter, sent now). Returns its id.
    #[datastore_span(name = "upsert_group_invitation", system = "postgresql")]
    pub async fn upsert_group_invitation(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        inviter: &[u8],
        invitee: &[u8],
    ) -> Result<Uuid> {
        check_key("inviter", inviter)?;
        check_key("invitee", invitee)?;
        Ok(sqlx::query_scalar(
            "INSERT INTO group_invitations (community_id, channel_id, inviter, invitee) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (community_id, channel_id, invitee) WHERE status = 'pending' \
             DO UPDATE SET inviter = EXCLUDED.inviter, created_at = now() \
             RETURNING id",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .bind(inviter)
        .bind(invitee)
        .fetch_one(&self.pool)
        .await?)
    }

    /// What decides whether a refused add may write an invitation, read in
    /// one round trip: whether `invitee` already has an unexpired pending
    /// invitation to the group, whether they declined one to the group within
    /// [`DECLINE_COOLDOWN_DAYS`], and how many invitations `inviter` created
    /// or refreshed in this community in the last 24 hours.
    #[datastore_span(name = "group_invitation_write_state", system = "postgresql")]
    pub async fn group_invitation_write_state(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        inviter: &[u8],
        invitee: &[u8],
    ) -> Result<InvitationWriteState> {
        let (already_pending, declined_recently, sent_last_day): (bool, bool, i64) =
            sqlx::query_as(
                "SELECT \
                   EXISTS (SELECT 1 FROM group_invitations \
                            WHERE community_id = $1 AND channel_id = $2 AND invitee = $4 \
                              AND status = 'pending' \
                              AND created_at > now() - make_interval(days => $5)), \
                   EXISTS (SELECT 1 FROM group_invitations \
                            WHERE community_id = $1 AND channel_id = $2 AND invitee = $4 \
                              AND status = 'declined' \
                              AND responded_at > now() - make_interval(days => $6)), \
                   (SELECT count(*) FROM group_invitations \
                     WHERE community_id = $1 AND inviter = $3 \
                       AND created_at > now() - interval '24 hours')",
            )
            .bind(community.as_uuid())
            .bind(channel_id)
            .bind(inviter)
            .bind(invitee)
            .bind(INVITATION_TTL_DAYS)
            .bind(DECLINE_COOLDOWN_DAYS)
            .fetch_one(&self.pool)
            .await?;
        Ok(InvitationWriteState {
            already_pending,
            declined_recently,
            sent_last_day,
        })
    }

    /// The reader's usable pending invitations, newest first: not expired,
    /// group live and not archived, inviter not blocked by the reader.
    #[datastore_span(name = "pending_group_invitations", system = "postgresql")]
    pub async fn pending_group_invitations(
        &self,
        community: CommunityId,
        invitee: &[u8],
    ) -> Result<Vec<GroupInvitation>> {
        let rows = sqlx::query(INVITATIONS_SQL)
            .bind(community.as_uuid())
            .bind(invitee)
            .bind(INVITATION_TTL_DAYS)
            .bind(INVITATION_MEMBER_PREVIEW)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| {
                Ok(GroupInvitation {
                    id: row.try_get("invitation_id")?,
                    channel: summary_from_row(row)?,
                    inviter: row.try_get("inviter")?,
                    created_at: row.try_get("invited_at")?,
                    member_pubkeys: row.try_get("member_pubkeys")?,
                })
            })
            .collect()
    }

    /// One of the reader's pending, unexpired invitations.
    #[datastore_span(name = "pending_group_invitation", system = "postgresql")]
    pub async fn pending_group_invitation(
        &self,
        community: CommunityId,
        id: Uuid,
        invitee: &[u8],
    ) -> Result<Option<PendingInvitation>> {
        let row = sqlx::query(
            "SELECT channel_id, inviter FROM group_invitations \
              WHERE community_id = $1 AND id = $2 AND invitee = $3 AND status = 'pending' \
                AND created_at > now() - make_interval(days => $4)",
        )
        .bind(community.as_uuid())
        .bind(id)
        .bind(invitee)
        .bind(INVITATION_TTL_DAYS)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(PendingInvitation {
                channel_id: row.try_get("channel_id")?,
                inviter: row.try_get("inviter")?,
            })
        })
        .transpose()
    }

    /// Answer one pending invitation (`accepted`, `declined` or `expired`).
    /// Returns false when it was not pending.
    #[datastore_span(name = "respond_group_invitation", system = "postgresql")]
    pub async fn respond_group_invitation(
        &self,
        community: CommunityId,
        id: Uuid,
        invitee: &[u8],
        status: &str,
    ) -> Result<bool> {
        let done = sqlx::query(
            "UPDATE group_invitations SET status = $4, responded_at = now() \
              WHERE community_id = $1 AND id = $2 AND invitee = $3 AND status = 'pending'",
        )
        .bind(community.as_uuid())
        .bind(id)
        .bind(invitee)
        .bind(status)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Mark the member's pending invitation to this group accepted: they are
    /// in, however they got there.
    #[datastore_span(name = "settle_group_invitations", system = "postgresql")]
    pub async fn settle_group_invitations(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        invitee: &[u8],
    ) -> Result<()> {
        sqlx::query(
            "UPDATE group_invitations SET status = 'accepted', responded_at = now() \
              WHERE community_id = $1 AND channel_id = $2 AND invitee = $3 \
                AND status = 'pending'",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .bind(invitee)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Whether someone other than `pubkey` removed them from the group (an
    /// owner/admin kick), so a shared link must not bring them back.
    #[datastore_span(name = "removed_from_group_by_other", system = "postgresql")]
    pub async fn removed_from_group_by_other(
        &self,
        community: CommunityId,
        channel_id: Uuid,
        pubkey: &[u8],
    ) -> Result<bool> {
        let removed: Option<bool> = sqlx::query_scalar(
            "SELECT removed_at IS NOT NULL AND removed_by IS NOT NULL AND removed_by <> pubkey \
               FROM channel_members \
              WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
        )
        .bind(community.as_uuid())
        .bind(channel_id)
        .bind(pubkey)
        .fetch_optional(&self.pool)
        .await?;
        Ok(removed.unwrap_or(false))
    }

    /// Whether `pubkey` is a bot principal (token-mode agent), even one that
    /// has not connected yet and so has no `users.agent_owner_pubkey`.
    #[datastore_span(name = "is_bot_principal", system = "postgresql")]
    pub async fn is_bot_principal(&self, pubkey: &[u8]) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM principals WHERE id = $1 AND kind = 'bot')",
        )
        .bind(pubkey)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Whether `blocker` blocks `blocked`.
    #[datastore_span(name = "is_blocked_by", system = "postgresql")]
    pub async fn is_blocked_by(
        &self,
        community: CommunityId,
        blocker: &[u8],
        blocked: &[u8],
    ) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_blocks \
              WHERE community_id = $1 AND blocker = $2 AND blocked = $3)",
        )
        .bind(community.as_uuid())
        .bind(blocker)
        .bind(blocked)
        .fetch_one(&self.pool)
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::like_contains;

    #[test]
    fn like_pattern_escapes_wildcards_and_lowercases() {
        assert_eq!(like_contains("Design"), "%design%");
        assert_eq!(like_contains("50%_off\\"), "%50\\%\\_off\\\\%");
    }
}
