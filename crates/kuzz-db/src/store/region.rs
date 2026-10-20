//! Deployment regions (migration 0078) and the room directory (0079).
//!
//! Each relay records its own region at startup and claims this database for
//! it. Room and user home markers refer to these ids.

use kuzz_core::CommunityId;
use kuzz_datastore_tracing::datastore_span;
use sqlx::Row;
use uuid::Uuid;

use crate::error::{DbError, Result};
use crate::Db;

/// A room's home as the directory records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomHome {
    /// Region that owns the room.
    pub region: String,
    /// Bumped each time the room moves to another region.
    pub epoch: i32,
}

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

    /// The region this database belongs to, if one has claimed it.
    #[datastore_span(name = "local_region", system = "postgresql")]
    pub async fn local_region(&self) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT region_id FROM local_region")
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Claim this database for `region` (`local_region`), which must already
    /// be recorded. Idempotent for the same region. Fails when the database
    /// already belongs to another region: one database never serves two.
    #[datastore_span(name = "claim_local_region", system = "postgresql")]
    pub async fn claim_local_region(&self, region: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO local_region (region_id) VALUES ($1) ON CONFLICT (singleton) DO NOTHING",
        )
        .bind(region)
        .execute(&self.pool)
        .await?;
        let claimed: String = sqlx::query_scalar("SELECT region_id FROM local_region")
            .fetch_one(&self.pool)
            .await?;
        if claimed != region {
            return Err(DbError::InvalidData(format!(
                "this database belongs to region {claimed:?}, not {region:?}"
            )));
        }
        Ok(())
    }

    /// Give every room without a directory row the local region as its home.
    /// Covers rooms created before migration 0079 and while no region was
    /// claimed. Communities being deleted are skipped (their writes are
    /// fenced). Returns the number of rows added.
    #[datastore_span(name = "backfill_room_directory", system = "postgresql")]
    pub async fn backfill_room_directory(&self) -> Result<u64> {
        let added = sqlx::query(
            r#"
            INSERT INTO room_directory (community_id, channel_id, home_region, participant_hash)
            SELECT c.community_id, c.id, l.region_id, c.participant_hash
              FROM channels c
              JOIN communities co ON co.id = c.community_id AND co.deletion_state = 'active'
             CROSS JOIN local_region l
             WHERE NOT EXISTS (
                   SELECT 1 FROM room_directory d
                    WHERE d.community_id = c.community_id AND d.channel_id = c.id)
            ON CONFLICT (community_id, channel_id) DO NOTHING
            "#,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(added)
    }

    /// The room's home, or `None` when the directory has no row for it.
    #[datastore_span(name = "room_home", system = "postgresql")]
    pub async fn room_home(
        &self,
        community_id: CommunityId,
        channel_id: Uuid,
    ) -> Result<Option<RoomHome>> {
        let row = sqlx::query(
            "SELECT home_region, epoch FROM room_directory WHERE community_id = $1 AND channel_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(RoomHome {
                region: row.try_get("home_region")?,
                epoch: row.try_get("epoch")?,
            })
        })
        .transpose()
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
    use kuzz_core::CommunityId;
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::RoomHome;
    use crate::Db;

    async fn community(db: &Db) -> CommunityId {
        let id = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(id.as_uuid())
            .bind(format!("region-{}.test", id.as_uuid()))
            .execute(&db.pool)
            .await
            .expect("fixture community");
        id
    }

    async fn room(db: &Db, community: CommunityId, dm_hash: Option<&[u8]>) -> Uuid {
        let id = Uuid::new_v4();
        let (kind, visibility) = if dm_hash.is_some() {
            ("dm", "private")
        } else {
            ("stream", "open")
        };
        sqlx::query(
            "INSERT INTO channels (id, community_id, name, channel_type, visibility, created_by, participant_hash) \
             VALUES ($1, $2, $3, $4::channel_type, $5::channel_visibility, $6, $7)",
        )
        .bind(id)
        .bind(community.as_uuid())
        .bind(format!("room-{id}"))
        .bind(kind)
        .bind(visibility)
        .bind([7u8; 32].as_slice())
        .bind(dm_hash)
        .execute(&db.pool)
        .await
        .expect("fixture room");
        id
    }

    async fn db() -> Db {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("isolated test database");
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        for clear in [
            "DELETE FROM room_directory",
            "DELETE FROM local_region",
            "DELETE FROM regions",
        ] {
            sqlx::query(clear)
                .execute(&pool)
                .await
                .expect("clear region tables");
        }
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

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn rooms_record_their_home_and_dm_pairs_are_reserved() {
        let db = db().await;
        let scope = community(&db).await;
        // Created before any region is claimed: no row yet.
        let early = room(&db, scope, None).await;
        assert_eq!(db.room_home(scope, early).await.expect("home"), None);

        db.register_region("kr", "wss://kr.example", true)
            .await
            .expect("region");
        assert_eq!(db.local_region().await.expect("unclaimed"), None);
        db.claim_local_region("kr").await.expect("claim");
        assert_eq!(
            db.local_region().await.expect("claimed").as_deref(),
            Some("kr")
        );
        db.claim_local_region("kr")
            .await
            .expect("claim is idempotent");
        db.register_region("jp", "wss://jp.example", false)
            .await
            .expect("second region");
        assert!(
            db.claim_local_region("jp").await.is_err(),
            "a database never serves two regions"
        );

        // Startup backfill gives the early room the local region.
        assert!(db.backfill_room_directory().await.expect("backfill") >= 1);
        let kr = Some(RoomHome {
            region: "kr".to_string(),
            epoch: 0,
        });
        assert_eq!(db.room_home(scope, early).await.expect("home"), kr);
        assert_eq!(db.backfill_room_directory().await.expect("again"), 0);

        // New rooms get their row from the trigger, in the same transaction.
        let later = room(&db, scope, None).await;
        assert_eq!(db.room_home(scope, later).await.expect("home"), kr);
        let hash = [9u8; 32];
        let dm = room(&db, scope, Some(&hash)).await;
        assert_eq!(db.room_home(scope, dm).await.expect("home"), kr);

        // The DM pair is reserved in the directory as well.
        let other_region_dm = sqlx::query(
            "INSERT INTO room_directory (community_id, channel_id, home_region, participant_hash) \
             VALUES ($1, $2, 'jp', $3)",
        )
        .bind(scope.as_uuid())
        .bind(Uuid::new_v4())
        .bind(hash.as_slice())
        .execute(&db.pool)
        .await;
        assert!(other_region_dm.is_err(), "one DM per pair across regions");

        // A removed room frees its row (and so its DM pair).
        sqlx::query("DELETE FROM channels WHERE community_id = $1 AND id = $2")
            .bind(scope.as_uuid())
            .bind(dm)
            .execute(&db.pool)
            .await
            .expect("delete room");
        assert_eq!(db.room_home(scope, dm).await.expect("home"), None);
    }
}
