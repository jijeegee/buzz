//! Deployment regions (migration 0078) and the room directory (0079).
//!
//! Each relay records its own region at startup and claims this database for
//! it. Room and user home markers refer to these ids.

use kuzz_core::principal::PrincipalId;
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

/// The directory row a reservation ended on ([`Db::reserve_room_home`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedRoom {
    /// The room id the directory holds: the one asked for, or for a DM the
    /// room already holding that participant set.
    pub channel_id: Uuid,
    /// Its home.
    pub home: RoomHome,
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
    /// claimed. Communities being deleted are skipped through the write-fence
    /// predicate, so one community entering deletion mid-run does not abort
    /// the rest. Returns the number of rows added.
    #[datastore_span(name = "backfill_room_directory", system = "postgresql")]
    pub async fn backfill_room_directory(&self) -> Result<u64> {
        let added = sqlx::query(
            r#"
            INSERT INTO room_directory (community_id, channel_id, home_region, participant_hash)
            SELECT c.community_id, c.id, l.region_id, c.participant_hash
              FROM channels c
             CROSS JOIN local_region l
             WHERE NOT EXISTS (
                   SELECT 1 FROM room_directory d
                    WHERE d.community_id = c.community_id AND d.channel_id = c.id)
               AND community_write_allowed(c.community_id)
            ON CONFLICT (community_id, channel_id) DO NOTHING
            "#,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(added)
    }

    /// Give every account without a home region the local region (accounts
    /// from before migration 0080, or created while no region was claimed).
    /// Returns the number of accounts updated.
    #[datastore_span(name = "backfill_principal_homes", system = "postgresql")]
    pub async fn backfill_principal_homes(&self) -> Result<u64> {
        let updated = sqlx::query(
            "UPDATE principals p SET home_region = l.region_id \
               FROM local_region l WHERE p.home_region IS NULL",
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(updated)
    }

    /// Record the country an account signed up from, unless one is recorded.
    /// `country` must be two uppercase letters.
    #[datastore_span(name = "record_signup_country", system = "postgresql")]
    pub async fn record_signup_country(
        &self,
        principal: &PrincipalId,
        country: &str,
    ) -> Result<()> {
        sqlx::query("UPDATE principals SET country = $2 WHERE id = $1 AND country IS NULL")
            .bind(principal.as_bytes().as_slice())
            .bind(country)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The account's home region and sign-up country.
    #[datastore_span(name = "principal_home", system = "postgresql")]
    pub async fn principal_home(
        &self,
        principal: &PrincipalId,
    ) -> Result<Option<(Option<String>, Option<String>)>> {
        let row = sqlx::query("SELECT home_region, country FROM principals WHERE id = $1")
            .bind(principal.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| Ok((row.try_get("home_region")?, row.try_get("country")?)))
            .transpose()
    }

    /// Reserve `channel_id` for `home` unless the directory already holds
    /// the room or, for a DM (`participant_hash`), its participant set.
    /// Returns the row that holds it now, so two regions creating the same
    /// room or DM at once end on one (global regions S4-6, run at the
    /// primary). Safe to repeat.
    #[datastore_span(name = "reserve_room_home", system = "postgresql")]
    pub async fn reserve_room_home(
        &self,
        community_id: CommunityId,
        channel_id: Uuid,
        home: &str,
        participant_hash: Option<&[u8]>,
    ) -> Result<ReservedRoom> {
        sqlx::query(
            "INSERT INTO room_directory (community_id, channel_id, home_region, participant_hash) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(community_id.as_uuid())
        .bind(channel_id)
        .bind(home)
        .bind(participant_hash)
        .execute(&self.pool)
        .await?;
        let row = match participant_hash {
            Some(hash) => {
                sqlx::query(
                    "SELECT channel_id, home_region, epoch FROM room_directory \
                      WHERE community_id = $1 AND participant_hash = $2",
                )
                .bind(community_id.as_uuid())
                .bind(hash)
                .fetch_optional(&self.pool)
                .await?
            }
            None => {
                sqlx::query(
                    "SELECT channel_id, home_region, epoch FROM room_directory \
                      WHERE community_id = $1 AND channel_id = $2",
                )
                .bind(community_id.as_uuid())
                .bind(channel_id)
                .fetch_optional(&self.pool)
                .await?
            }
        };
        // Only a room id already used by a room that is not this DM leaves
        // no row for the participant set.
        let row =
            row.ok_or_else(|| DbError::InvalidData("room id already used by another room".into()))?;
        Ok(ReservedRoom {
            channel_id: row.try_get("channel_id")?,
            home: RoomHome {
                region: row.try_get("home_region")?,
                epoch: row.try_get("epoch")?,
            },
        })
    }

    /// Remember where another region's room lives (learned from the
    /// primary). Keeps an existing row.
    #[datastore_span(name = "remember_room_home", system = "postgresql")]
    pub async fn remember_room_home(
        &self,
        community_id: CommunityId,
        channel_id: Uuid,
        home: &RoomHome,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO room_directory (community_id, channel_id, home_region, epoch) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(community_id.as_uuid())
        .bind(channel_id)
        .bind(&home.region)
        .bind(home.epoch)
        .execute(&self.pool)
        .await?;
        Ok(())
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

    /// Of `channels`, the rooms the directory homes outside `local_region`,
    /// with their home region. Rooms without a directory row are not listed.
    #[datastore_span(name = "rooms_homed_elsewhere", system = "postgresql")]
    pub async fn rooms_homed_elsewhere(
        &self,
        community_id: CommunityId,
        channels: &[Uuid],
        local_region: &str,
    ) -> Result<Vec<(Uuid, String)>> {
        if channels.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT channel_id, home_region FROM room_directory \
              WHERE community_id = $1 AND channel_id = ANY($2) AND home_region <> $3",
        )
        .bind(community_id.as_uuid())
        .bind(channels)
        .bind(local_region)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| Ok((row.try_get("channel_id")?, row.try_get("home_region")?)))
            .collect()
    }

    /// Record this region's link key (x-only public key, lowercase hex).
    #[datastore_span(name = "set_region_link_key", system = "postgresql")]
    pub async fn set_region_link_key(&self, id: &str, link_pubkey: &str) -> Result<()> {
        let done = sqlx::query("UPDATE regions SET link_pubkey = $2 WHERE id = $1")
            .bind(id)
            .bind(link_pubkey)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(DbError::NotFound(format!("region {id}")));
        }
        Ok(())
    }

    /// A region's address and link key, or `None` when the region is not
    /// registered here. The key is `None` when the region has no link.
    #[datastore_span(name = "region_link", system = "postgresql")]
    pub async fn region_link(&self, id: &str) -> Result<Option<(String, Option<String>)>> {
        let row = sqlx::query("SELECT connect_url, link_pubkey FROM regions WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| Ok((row.try_get("connect_url")?, row.try_get("link_pubkey")?)))
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
    use kuzz_core::principal::PrincipalId;

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
    async fn region_link_keys_are_recorded_per_region() {
        let db = db().await;
        db.register_region("kr", "wss://kr.example", true)
            .await
            .expect("region");
        assert_eq!(
            db.region_link("kr").await.expect("read"),
            Some(("wss://kr.example".to_string(), None))
        );
        let key = "ab".repeat(32);
        db.set_region_link_key("kr", &key).await.expect("set key");
        // A restart refreshes the address and keeps the key.
        db.register_region("kr", "wss://kr2.example", true)
            .await
            .expect("re-register");
        assert_eq!(
            db.region_link("kr").await.expect("read"),
            Some(("wss://kr2.example".to_string(), Some(key)))
        );
        assert_eq!(db.region_link("jp").await.expect("read"), None);
        assert!(db
            .set_region_link_key("jp", &"cd".repeat(32))
            .await
            .is_err());
        // Only a lowercase 64-hex key fits the column.
        assert!(db.set_region_link_key("kr", "not-a-key").await.is_err());
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

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn reservations_keep_the_first_home_per_room_and_dm_set() {
        let db = db().await;
        let scope = community(&db).await;
        let home = |region: &str| RoomHome {
            region: region.to_string(),
            epoch: 0,
        };

        // A room id: the first region keeps it; a repeat or a rival gets it.
        let room = Uuid::new_v4();
        let first = db
            .reserve_room_home(scope, room, "jp", None)
            .await
            .expect("reserve");
        assert_eq!((first.channel_id, first.home.clone()), (room, home("jp")));
        let rival = db
            .reserve_room_home(scope, room, "kr", None)
            .await
            .expect("rival");
        assert_eq!(rival, first);

        // A DM set: a rival's own id is ignored; it gets the first DM's id.
        let hash = [3u8; 32];
        let (dm, other) = (Uuid::new_v4(), Uuid::new_v4());
        let first = db
            .reserve_room_home(scope, dm, "kr", Some(&hash))
            .await
            .expect("reserve dm");
        assert_eq!((first.channel_id, first.home.clone()), (dm, home("kr")));
        let rival = db
            .reserve_room_home(scope, other, "jp", Some(&hash))
            .await
            .expect("rival dm");
        assert_eq!(rival, first);
        assert_eq!(db.room_home(scope, other).await.expect("home"), None);

        // A DM set reserved with an id another room already uses is refused.
        assert!(db
            .reserve_room_home(scope, room, "jp", Some(&[4u8; 32]))
            .await
            .is_err());

        // A learned home is kept, never overwritten.
        let learned = Uuid::new_v4();
        db.remember_room_home(scope, learned, &home("kr"))
            .await
            .expect("remember");
        db.remember_room_home(scope, learned, &home("jp"))
            .await
            .expect("remember again");
        assert_eq!(
            db.room_home(scope, learned).await.expect("home"),
            Some(home("kr"))
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn accounts_record_their_home_region_and_signup_country() {
        let db = db().await;
        let insert = |id: PrincipalId| {
            let pool = db.pool.clone();
            async move {
                sqlx::query("INSERT INTO principals (id, kind) VALUES ($1, 'user')")
                    .bind(id.as_bytes().as_slice())
                    .execute(&pool)
                    .await
                    .expect("principal");
            }
        };
        let early = PrincipalId::generate();
        insert(early).await;
        assert_eq!(
            db.principal_home(&early).await.expect("home"),
            Some((None, None))
        );

        db.register_region("kr", "wss://kr.example", true)
            .await
            .expect("region");
        db.claim_local_region("kr").await.expect("claim");
        assert!(db.backfill_principal_homes().await.expect("backfill") >= 1);
        assert_eq!(
            db.principal_home(&early).await.expect("home"),
            Some((Some("kr".to_string()), None))
        );

        // New accounts get the local region from the trigger.
        let later = PrincipalId::generate();
        insert(later).await;
        db.record_signup_country(&later, "JP")
            .await
            .expect("country");
        // The first recorded country stays.
        db.record_signup_country(&later, "US").await.expect("again");
        assert_eq!(
            db.principal_home(&later).await.expect("home"),
            Some((Some("kr".to_string()), Some("JP".to_string())))
        );
        assert!(db.record_signup_country(&early, "jp").await.is_err());
    }
}
