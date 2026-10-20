//! Deployment regions (migration 0078).
//!
//! Each relay records its own region at startup. Room and user home markers
//! refer to these ids.

use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;

use crate::error::Result;
use crate::Db;

/// One region row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionRecord {
    /// Short id such as `kr`.
    pub id: String,
    /// Address clients in this region connect to.
    pub connect_url: String,
    /// Whether this is the primary region.
    pub is_primary: bool,
}

impl Db {
    /// Record this relay's region: insert it, or refresh its address, primary
    /// flag and `last_seen_at`. Fails when it claims primary while another
    /// region is primary (unique index `regions_one_primary`), so two regions
    /// can never both run the primary-only jobs.
    #[datastore_span(name = "register_region", system = "postgresql")]
    pub async fn register_region(
        &self,
        id: &str,
        connect_url: &str,
        is_primary: bool,
    ) -> Result<RegionRecord> {
        let row = sqlx::query(
            r#"
            INSERT INTO regions (id, connect_url, is_primary)
            VALUES ($1, $2, $3)
            ON CONFLICT (id) DO UPDATE
               SET connect_url = EXCLUDED.connect_url,
                   is_primary = EXCLUDED.is_primary,
                   last_seen_at = now()
            RETURNING id, connect_url, is_primary
            "#,
        )
        .bind(id)
        .bind(connect_url)
        .bind(is_primary)
        .fetch_one(&self.pool)
        .await?;
        Ok(RegionRecord {
            id: row.try_get("id")?,
            connect_url: row.try_get("connect_url")?,
            is_primary: row.try_get("is_primary")?,
        })
    }

    /// Every region, primary first, then by id.
    #[datastore_span(name = "list_regions", system = "postgresql")]
    pub async fn list_regions(&self) -> Result<Vec<RegionRecord>> {
        let rows = sqlx::query(
            "SELECT id, connect_url, is_primary FROM regions ORDER BY is_primary DESC, id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(RegionRecord {
                    id: row.try_get("id")?,
                    connect_url: row.try_get("connect_url")?,
                    is_primary: row.try_get("is_primary")?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod postgres_tests {
    use sqlx::PgPool;

    use crate::Db;

    async fn db() -> Db {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("isolated test database");
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        sqlx::query("DELETE FROM regions")
            .execute(&pool)
            .await
            .expect("clear regions");
        Db::from_pool(pool)
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn regions_register_refresh_and_keep_one_primary() {
        let db = db().await;
        let kr = db
            .register_region("kr", "wss://kr.example", true)
            .await
            .expect("first region");
        assert!(kr.is_primary);
        // Restart with a new address: same row, refreshed.
        let kr = db
            .register_region("kr", "wss://kr2.example", true)
            .await
            .expect("re-register");
        assert_eq!(kr.connect_url, "wss://kr2.example");
        db.register_region("jp", "wss://jp.example", false)
            .await
            .expect("second region");
        // A second primary is refused.
        assert!(db
            .register_region("us", "wss://us.example", true)
            .await
            .is_err());
        // Malformed ids are refused by the table too.
        assert!(db
            .register_region("KR!", "wss://x.example", false)
            .await
            .is_err());
        let ids: Vec<_> = db
            .list_regions()
            .await
            .expect("list")
            .into_iter()
            .map(|r| (r.id, r.is_primary))
            .collect();
        assert_eq!(
            ids,
            vec![("kr".to_string(), true), ("jp".to_string(), false)]
        );
    }
}
