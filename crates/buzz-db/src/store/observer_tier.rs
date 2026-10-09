//! Per-owner observer telemetry tier overrides.
//!
//! A row overrides the relay's configured default tier for one owner in one
//! community. Agents inherit their owner's tier; the relay resolves the owner
//! and applies the default when no row exists.

use buzz_datastore_tracing::datastore_span;
use sqlx::PgPool;

use crate::error::{DbError, Result};
use crate::{CommunityId, Db};

/// Tier names accepted by the `observer_tiers.tier` CHECK constraint.
pub const OBSERVER_TIER_NAMES: [&str; 3] = ["free", "standard", "premium"];

/// Return the stored tier override for `owner_pubkey`, if any.
pub async fn get_observer_tier(
    pool: &PgPool,
    community_id: CommunityId,
    owner_pubkey: &[u8],
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT tier FROM observer_tiers WHERE community_id = $1 AND owner_pubkey = $2",
    )
    .bind(community_id.as_uuid())
    .bind(owner_pubkey)
    .fetch_optional(pool)
    .await?)
}

/// Insert or replace the tier override for `owner_pubkey`.
///
/// Returns an error if `tier` is not one of [`OBSERVER_TIER_NAMES`].
pub async fn set_observer_tier(
    pool: &PgPool,
    community_id: CommunityId,
    owner_pubkey: &[u8],
    tier: &str,
) -> Result<()> {
    if !OBSERVER_TIER_NAMES.contains(&tier) {
        return Err(DbError::InvalidData(format!(
            "invalid observer tier: {tier}"
        )));
    }
    sqlx::query(
        "INSERT INTO observer_tiers (community_id, owner_pubkey, tier) VALUES ($1, $2, $3) \
         ON CONFLICT (community_id, owner_pubkey) \
         DO UPDATE SET tier = EXCLUDED.tier, updated_at = now()",
    )
    .bind(community_id.as_uuid())
    .bind(owner_pubkey)
    .bind(tier)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove the tier override for `owner_pubkey`, returning whether one existed.
pub async fn clear_observer_tier(
    pool: &PgPool,
    community_id: CommunityId,
    owner_pubkey: &[u8],
) -> Result<bool> {
    let result =
        sqlx::query("DELETE FROM observer_tiers WHERE community_id = $1 AND owner_pubkey = $2")
            .bind(community_id.as_uuid())
            .bind(owner_pubkey)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

impl Db {
    /// Return the stored observer tier override for an owner.
    #[datastore_span(name = "get_observer_tier", system = "postgresql")]
    pub async fn get_observer_tier(
        &self,
        community_id: CommunityId,
        owner_pubkey: &[u8],
    ) -> Result<Option<String>> {
        get_observer_tier(&self.pool, community_id, owner_pubkey).await
    }

    /// Insert or replace an owner's observer tier override.
    #[datastore_span(name = "set_observer_tier", system = "postgresql")]
    pub async fn set_observer_tier(
        &self,
        community_id: CommunityId,
        owner_pubkey: &[u8],
        tier: &str,
    ) -> Result<()> {
        set_observer_tier(&self.pool, community_id, owner_pubkey, tier).await
    }

    /// Remove an owner's observer tier override.
    #[datastore_span(name = "clear_observer_tier", system = "postgresql")]
    pub async fn clear_observer_tier(
        &self,
        community_id: CommunityId,
        owner_pubkey: &[u8],
    ) -> Result<bool> {
        clear_observer_tier(&self.pool, community_id, owner_pubkey).await
    }
}

#[cfg(test)]
mod postgres_tests {
    use super::*;
    use nostr::Keys;

    async fn setup_db() -> Db {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("connect to test DB");
        if std::env::var("BUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate test DB");
        }
        Db::from_pool(pool)
    }

    async fn make_community(pool: &PgPool) -> CommunityId {
        let id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("observer-tier-{}.example", id.simple()))
            .execute(pool)
            .await
            .expect("insert test community");
        CommunityId::from_uuid(id)
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn observer_tier_override_round_trips_per_community() {
        let db = setup_db().await;
        let community = make_community(&db.pool).await;
        let other = make_community(&db.pool).await;
        let owner = Keys::generate().public_key().to_bytes();

        assert_eq!(db.get_observer_tier(community, &owner).await.unwrap(), None);
        db.set_observer_tier(community, &owner, "free")
            .await
            .unwrap();
        db.set_observer_tier(community, &owner, "standard")
            .await
            .unwrap();
        assert_eq!(
            db.get_observer_tier(community, &owner).await.unwrap(),
            Some("standard".to_owned())
        );
        assert_eq!(
            db.get_observer_tier(other, &owner).await.unwrap(),
            None,
            "an override in one community must not leak into another"
        );
        assert!(matches!(
            db.set_observer_tier(community, &owner, "gold").await,
            Err(DbError::InvalidData(_))
        ));

        assert!(db.clear_observer_tier(community, &owner).await.unwrap());
        assert!(!db.clear_observer_tier(community, &owner).await.unwrap());
        assert_eq!(db.get_observer_tier(community, &owner).await.unwrap(), None);
    }
}
