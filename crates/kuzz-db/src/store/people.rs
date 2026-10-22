//! Finding people to add as friends: exact username/email lookup and
//! name-prefix search among people the reader shares a channel with.
//!
//! Results carry a messaging pubkey and public profile fields only; email
//! addresses are matched but never returned.

use kuzz_core::principal::PrincipalId;
use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;

use crate::error::Result;
use crate::Db;

/// Most people one shared-channel search returns.
pub const MAX_SHARED_PEOPLE: i64 = 50;

/// A person found by lookup or search. Never carries an email address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonRecord {
    /// 32-byte messaging pubkey (the key their events are signed with).
    pub pubkey: Vec<u8>,
    /// Display name (community profile first, then the account's).
    pub display_name: String,
    /// Avatar URL, if any.
    pub avatar_url: Option<String>,
    /// `@username`, if set.
    pub username: Option<String>,
}

/// What an exact lookup matches against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonLookup<'a> {
    /// Exact `@username`, case-insensitive, without the leading `@`.
    Username(&'a str),
    /// Exact sign-in email, case-insensitive.
    Email(&'a str),
}

fn person_from_row(row: &sqlx::postgres::PgRow) -> Result<PersonRecord> {
    Ok(PersonRecord {
        pubkey: row.try_get("pubkey")?,
        display_name: row.try_get("display_name")?,
        avatar_url: row.try_get("avatar_url")?,
        username: row.try_get("username")?,
    })
}

/// Escape `%`, `_` and `\` for a `LIKE ... ESCAPE '\'` pattern.
fn like_prefix(prefix: &str) -> String {
    let mut out = String::with_capacity(prefix.len() + 1);
    for ch in prefix.to_lowercase().chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}

/// Messaging pubkey of principal `p`: the backed-up account key for
/// key-backup accounts, the principal id for token accounts.
macro_rules! messaging_pubkey {
    () => {
        "CASE WHEN p.identity_mode = 'key_backup' THEN kb.pubkey ELSE p.id END"
    };
}

/// Exact lookup over discoverable, active users; `$2` is the needle and the
/// macro argument is the match predicate.
macro_rules! lookup_sql {
    ($matcher:expr) => {
        concat!(
            "SELECT ",
            messaging_pubkey!(),
            " AS pubkey, \
                    COALESCE(NULLIF(u.display_name, ''), p.display_name) AS display_name, \
                    COALESCE(u.avatar_url, p.avatar_url) AS avatar_url, \
                    p.username \
               FROM principals p \
               LEFT JOIN account_key_backups kb ON kb.account_id = p.id \
               LEFT JOIN users u ON u.community_id = $1 AND u.pubkey = ",
            messaging_pubkey!(),
            " WHERE ",
            $matcher,
            " AND p.kind = 'user' AND p.discoverable AND p.disabled_at IS NULL \
                AND ",
            messaging_pubkey!(),
            " IS NOT NULL \
              ORDER BY p.created_at, p.id \
              LIMIT 1"
        )
    };
}

const MESSAGING_PUBKEY_SQL: &str = concat!(
    "SELECT ",
    messaging_pubkey!(),
    " AS pubkey FROM principals p \
       LEFT JOIN account_key_backups kb ON kb.account_id = p.id \
      WHERE p.id = $1"
);

/// App language of the person account whose messaging key is `$1`. Both
/// sides of the `OR` are indexed (primary key, unique backup pubkey).
const LOCALE_BY_PUBKEY_SQL: &str = concat!(
    "SELECT p.locale FROM principals p \
       LEFT JOIN account_key_backups kb ON kb.account_id = p.id \
      WHERE (p.id = $1 OR kb.pubkey = $1) AND p.kind = 'user' AND ",
    messaging_pubkey!(),
    " = $1 LIMIT 1"
);

const LOOKUP_BY_USERNAME_SQL: &str = lookup_sql!("lower(p.username) = lower($2)");

const LOOKUP_BY_EMAIL_SQL: &str = lookup_sql!(
    "p.id IN (SELECT i.principal_id FROM identities i \
               WHERE lower(i.email) = lower($2) AND i.email IS NOT NULL)"
);

impl Db {
    /// The pubkey a principal's events are signed with, if it has one.
    #[datastore_span(name = "principal_messaging_pubkey", system = "postgresql")]
    pub async fn principal_messaging_pubkey(&self, id: &PrincipalId) -> Result<Option<Vec<u8>>> {
        let pubkey: Option<Option<Vec<u8>>> = sqlx::query_scalar(MESSAGING_PUBKEY_SQL)
            .bind(id.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await?;
        Ok(pubkey.flatten())
    }

    /// The app language of the person whose messaging key is `pubkey`, for
    /// text the server writes to them. `None` when unset or not an account.
    #[datastore_span(name = "locale_for_pubkey", system = "postgresql")]
    pub async fn locale_for_pubkey(&self, pubkey: &[u8]) -> Result<Option<String>> {
        let locale: Option<Option<String>> = sqlx::query_scalar(LOCALE_BY_PUBKEY_SQL)
            .bind(pubkey)
            .fetch_optional(&self.pool)
            .await?;
        Ok(locale.flatten())
    }

    /// Find one discoverable, active user by exact username or email.
    ///
    /// Bots, relay principals, disabled accounts, accounts that turned
    /// discovery off and accounts without a messaging key all miss.
    #[datastore_span(name = "lookup_person", system = "postgresql")]
    pub async fn lookup_person(
        &self,
        community: CommunityId,
        lookup: PersonLookup<'_>,
    ) -> Result<Option<PersonRecord>> {
        let (sql, needle) = match lookup {
            PersonLookup::Username(name) => (LOOKUP_BY_USERNAME_SQL, name),
            PersonLookup::Email(email) => (LOOKUP_BY_EMAIL_SQL, email),
        };
        let row = sqlx::query(sql)
            .bind(community.as_uuid())
            .bind(needle)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(person_from_row).transpose()
    }

    /// Whether `a` and `b` are both current members of some live room.
    #[datastore_span(name = "share_a_room", system = "postgresql")]
    pub async fn share_a_room(&self, community: CommunityId, a: &[u8], b: &[u8]) -> Result<bool> {
        let shared: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
               SELECT 1 FROM channel_members ma \
                 JOIN channel_members mb ON mb.community_id = ma.community_id \
                      AND mb.channel_id = ma.channel_id AND mb.pubkey = $3 \
                      AND mb.removed_at IS NULL \
                 JOIN channels c ON c.community_id = ma.community_id AND c.id = ma.channel_id \
                WHERE ma.community_id = $1 AND ma.pubkey = $2 AND ma.removed_at IS NULL \
                  AND c.deleted_at IS NULL)",
        )
        .bind(community.as_uuid())
        .bind(a)
        .bind(b)
        .fetch_one(&self.pool)
        .await?;
        Ok(shared)
    }

    /// People sharing a live channel with `reader` whose display name or
    /// username starts with `prefix` (case-insensitive).
    ///
    /// Excludes the reader, agents, bots, disabled accounts and everyone the
    /// reader blocked. At most `limit` (capped at [`MAX_SHARED_PEOPLE`]).
    #[datastore_span(name = "shared_channel_people", system = "postgresql")]
    pub async fn shared_channel_people(
        &self,
        community: CommunityId,
        reader: &[u8],
        prefix: &str,
        limit: i64,
    ) -> Result<Vec<PersonRecord>> {
        let limit = limit.clamp(1, MAX_SHARED_PEOPLE);
        let rows = sqlx::query(
            "WITH mine AS ( \
               SELECT cm.channel_id FROM channel_members cm \
                 JOIN channels c ON c.community_id = cm.community_id AND c.id = cm.channel_id \
                WHERE cm.community_id = $1 AND cm.pubkey = $2 \
                  AND cm.removed_at IS NULL AND c.deleted_at IS NULL \
             ), peers AS ( \
               SELECT DISTINCT cm.pubkey FROM channel_members cm \
                 JOIN mine ON mine.channel_id = cm.channel_id \
                WHERE cm.community_id = $1 AND cm.removed_at IS NULL AND cm.pubkey <> $2 \
             ), people AS ( \
               SELECT peers.pubkey, \
                      COALESCE(NULLIF(u.display_name, ''), pr.display_name, '') AS display_name, \
                      COALESCE(u.avatar_url, pr.avatar_url) AS avatar_url, \
                      pr.username, pr.kind, pr.disabled_at, pr.id AS principal_id, \
                      u.agent_owner_pubkey, u.deactivated_at \
                 FROM peers \
                 LEFT JOIN users u ON u.community_id = $1 AND u.pubkey = peers.pubkey \
                 LEFT JOIN account_key_backups kb ON kb.pubkey = peers.pubkey \
                 LEFT JOIN principals pr ON pr.id = COALESCE(kb.account_id, peers.pubkey) \
                      AND (kb.account_id IS NOT NULL OR pr.identity_mode = 'token') \
             ) \
             SELECT pubkey, display_name, avatar_url, username FROM people \
              WHERE agent_owner_pubkey IS NULL AND deactivated_at IS NULL \
                AND (principal_id IS NULL OR (kind = 'user' AND disabled_at IS NULL)) \
                AND NOT EXISTS (SELECT 1 FROM user_blocks ub \
                                 WHERE ub.community_id = $1 AND ub.blocker = $2 \
                                   AND ub.blocked = people.pubkey) \
                AND (lower(display_name) LIKE $3 ESCAPE '\\' \
                     OR lower(username) LIKE $3 ESCAPE '\\') \
              ORDER BY lower(display_name), pubkey \
              LIMIT $4",
        )
        .bind(community.as_uuid())
        .bind(reader)
        .bind(like_prefix(prefix))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(person_from_row).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::like_prefix;

    #[test]
    fn like_prefix_escapes_wildcards_and_lowercases() {
        assert_eq!(like_prefix("Ada"), "ada%");
        assert_eq!(like_prefix("a_b%c\\"), "a\\_b\\%c\\\\%");
    }
}
