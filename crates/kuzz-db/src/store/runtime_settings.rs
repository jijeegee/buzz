//! Relay-wide runtime settings (`relay_runtime_settings`): values an operator
//! changes with `kuzz-admin settings set` and relays pick up without a
//! restart through a short cache. Deployment-global, not per community.

use kuzz_datastore_tracing::datastore_span;

use crate::error::Result;
use crate::Db;

impl Db {
    /// The stored value of runtime setting `key`, if any.
    #[datastore_span(name = "get_runtime_setting", system = "postgresql")]
    pub async fn get_runtime_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM relay_runtime_settings WHERE key = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Store `value` for runtime setting `key` (insert or replace).
    #[datastore_span(name = "set_runtime_setting", system = "postgresql")]
    pub async fn set_runtime_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO relay_runtime_settings (key, value) VALUES ($1, $2) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod postgres_tests {
    use crate::Db;
    use sqlx::PgPool;

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn runtime_settings_round_trip_and_replace() {
        let url = crate::test_support::database_url();
        let db = Db::from_pool(PgPool::connect(&url).await.expect("connect"));
        let key = format!("test_setting_{}", uuid::Uuid::new_v4().simple());
        assert_eq!(db.get_runtime_setting(&key).await.unwrap(), None);
        db.set_runtime_setting(&key, "shadow").await.unwrap();
        assert_eq!(
            db.get_runtime_setting(&key).await.unwrap().as_deref(),
            Some("shadow")
        );
        db.set_runtime_setting(&key, "on").await.unwrap();
        assert_eq!(
            db.get_runtime_setting(&key).await.unwrap().as_deref(),
            Some("on")
        );
        sqlx::query("DELETE FROM relay_runtime_settings WHERE key = $1")
            .bind(&key)
            .execute(&db.pool)
            .await
            .unwrap();
    }
}
