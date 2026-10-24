//! Encrypted provider revoke tokens (migration 0093): Apple's refresh token,
//! kept only to revoke the sign-in when the account is deleted. Rows are
//! keyed by the external sign-in; the server encrypts and decrypts them.

use kuzz_core::principal::PrincipalId;
use sqlx::Row as _;

use crate::error::Result;
use crate::Db;

/// Ciphertext-only database representation of a provider revoke token.
#[derive(Clone)]
pub struct ProviderRevokeTokenRecord {
    /// Identity provider (`identities.provider`).
    pub provider: String,
    /// Provider subject (`identities.subject`).
    pub subject: String,
    /// Nonsecret master-key label.
    pub key_id: String,
    /// Random 96-bit nonce.
    pub nonce: Vec<u8>,
    /// Encrypted token plus 16-byte authentication tag.
    pub ciphertext: Vec<u8>,
}

impl Db {
    /// Store (or replace) the revoke token of a sign-in.
    pub async fn store_provider_revoke_token(
        &self,
        record: &ProviderRevokeTokenRecord,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO provider_revoke_tokens (provider, subject, key_id, nonce, ciphertext) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (provider, subject) DO UPDATE SET key_id = EXCLUDED.key_id, \
               nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext, updated_at = now()",
        )
        .bind(&record.provider)
        .bind(&record.subject)
        .bind(&record.key_id)
        .bind(&record.nonce)
        .bind(&record.ciphertext)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The revoke tokens of every sign-in linked to `principal`.
    pub async fn principal_provider_revoke_tokens(
        &self,
        principal: &PrincipalId,
    ) -> Result<Vec<ProviderRevokeTokenRecord>> {
        let rows = sqlx::query(
            "SELECT t.provider, t.subject, t.key_id, t.nonce, t.ciphertext \
               FROM provider_revoke_tokens t \
               JOIN identities i ON i.provider = t.provider AND i.subject = t.subject \
              WHERE i.principal_id = $1 \
              ORDER BY t.provider, t.subject",
        )
        .bind(principal.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(ProviderRevokeTokenRecord {
                    provider: row.try_get("provider")?,
                    subject: row.try_get("subject")?,
                    key_id: row.try_get("key_id")?,
                    nonce: row.try_get("nonce")?,
                    ciphertext: row.try_get("ciphertext")?,
                })
            })
            .collect()
    }

    /// Forget the revoke token of a sign-in (after it was revoked).
    pub async fn delete_provider_revoke_token(&self, provider: &str, subject: &str) -> Result<()> {
        sqlx::query("DELETE FROM provider_revoke_tokens WHERE provider = $1 AND subject = $2")
            .bind(provider)
            .bind(subject)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
