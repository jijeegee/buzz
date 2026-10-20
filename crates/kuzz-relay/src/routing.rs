//! Room routing (global regions, stage S3).
//!
//! Every room lives in one home region (`room_directory`, migration 0079).
//! A room operation that reaches another region is not served from that
//! region's database: until regions forward to each other (S4), the client
//! is told where the room lives instead.
//!
//! With one registered region every room is local and nothing is looked up,
//! so a single-region deployment behaves exactly as before.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::Json;
use kuzz_core::CommunityId;
use kuzz_db::region::{RegionRecord, RoomHome};
use kuzz_db::Db;
use uuid::Uuid;

/// How long a room's home and the region list are cached. Homes change only
/// when a room moves (S6), which will drop the cached entry explicitly.
const CACHE_TTL: Duration = Duration::from_secs(30);

/// Prefix of WS `OK`/`CLOSED` messages for a room that lives elsewhere.
pub const WRONG_REGION_PREFIX: &str = "wrong-region:";

/// Where a room operation must be served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomRoute {
    /// The room lives here, or this region does not know it (the ordinary
    /// "not found" path applies).
    Here,
    /// The room lives in another region.
    Elsewhere(RemoteHome),
}

/// The home of a room that lives in another region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHome {
    /// The home region's id.
    pub region: String,
    /// Address clients in that region connect to; empty when the region is
    /// not registered here yet.
    pub connect_url: String,
    /// The room's epoch (bumped when it moves, S6).
    pub epoch: i32,
}

impl RemoteHome {
    /// The WS refusal: `wrong-region: room lives in <region> <connect_url>`.
    pub fn ws_message(&self) -> String {
        format!(
            "{WRONG_REGION_PREFIX} room lives in {} {}",
            self.region, self.connect_url
        )
        .trim_end()
        .to_string()
    }

    /// The HTTP refusal: 421 Misdirected Request with the home region.
    pub fn http_error(&self) -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::MISDIRECTED_REQUEST,
            Json(serde_json::json!({
                "error": "wrong_region",
                "region": self.region,
                "connect_url": self.connect_url,
            })),
        )
    }
}

/// Decide where a room is served from the region list and the room's
/// directory row.
pub fn route_for(local: &str, regions: &[RegionRecord], home: Option<&RoomHome>) -> RoomRoute {
    match home {
        Some(home) if regions.len() > 1 && home.region != local => {
            let connect_url = regions
                .iter()
                .find(|region| region.id == home.region)
                .map(|region| region.connect_url.clone())
                .unwrap_or_default();
            RoomRoute::Elsewhere(RemoteHome {
                region: home.region.clone(),
                connect_url,
                epoch: home.epoch,
            })
        }
        _ => RoomRoute::Here,
    }
}

/// The region and address an account's apps connect to: its home region's
/// `connect_url`, or this relay's own when the home is unset or not
/// registered here.
pub fn home_address(
    home: Option<&str>,
    regions: &[RegionRecord],
    local: &str,
    local_url: &str,
) -> (String, String) {
    home.and_then(|home| regions.iter().find(|region| region.id == home))
        .filter(|region| !region.connect_url.is_empty())
        .map(|region| (region.id.clone(), region.connect_url.clone()))
        .unwrap_or_else(|| (local.to_string(), local_url.to_string()))
}

/// Rooms split by the region that serves them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoomSplit {
    /// Rooms served here, in the order given.
    pub here: Vec<Uuid>,
    /// Rooms other regions serve, by region. Multi-room reads query these
    /// in their home regions once regions are linked (S4); until then they
    /// are left out.
    pub elsewhere: BTreeMap<String, Vec<Uuid>>,
}

/// Split `channels` using the directory rows of the rooms homed elsewhere.
pub fn split_rooms(channels: Vec<Uuid>, homed_elsewhere: Vec<(Uuid, String)>) -> RoomSplit {
    if homed_elsewhere.is_empty() {
        return RoomSplit {
            here: channels,
            elsewhere: BTreeMap::new(),
        };
    }
    let homes: HashMap<Uuid, String> = homed_elsewhere.into_iter().collect();
    let mut split = RoomSplit::default();
    for channel in channels {
        match homes.get(&channel) {
            Some(region) => split
                .elsewhere
                .entry(region.clone())
                .or_default()
                .push(channel),
            None => split.here.push(channel),
        }
    }
    split
}

/// Cached room-home lookups for one relay.
pub struct RoomRouter {
    local: String,
    regions: moka::sync::Cache<(), Arc<Vec<RegionRecord>>>,
    homes: moka::sync::Cache<(CommunityId, Uuid), Option<RoomHome>>,
}

impl RoomRouter {
    /// A router for a relay in `local_region`.
    pub fn new(local_region: impl Into<String>) -> Self {
        Self {
            local: local_region.into(),
            regions: moka::sync::Cache::builder()
                .max_capacity(1)
                .time_to_live(CACHE_TTL)
                .build(),
            homes: moka::sync::Cache::builder()
                .max_capacity(100_000)
                .time_to_live(CACHE_TTL)
                .build(),
        }
    }

    /// This relay's region.
    pub fn local_region(&self) -> &str {
        &self.local
    }

    /// Where an operation on `channel_id` must be served.
    pub async fn route(
        &self,
        db: &Db,
        community_id: CommunityId,
        channel_id: Uuid,
    ) -> Result<RoomRoute, kuzz_db::DbError> {
        let regions = self.region_list(db).await?;
        if regions.len() <= 1 {
            return Ok(RoomRoute::Here);
        }
        let key = (community_id, channel_id);
        let home = match self.homes.get(&key) {
            Some(home) => home,
            None => {
                let home = db.room_home(community_id, channel_id).await?;
                self.homes.insert(key, home.clone());
                home
            }
        };
        Ok(route_for(&self.local, &regions, home.as_ref()))
    }

    /// Split a multi-room read by the region that serves each room. With one
    /// registered region every room is here and nothing is looked up.
    pub async fn split(
        &self,
        db: &Db,
        community_id: CommunityId,
        channels: Vec<Uuid>,
    ) -> Result<RoomSplit, kuzz_db::DbError> {
        if self.region_list(db).await?.len() <= 1 {
            return Ok(split_rooms(channels, Vec::new()));
        }
        let homed_elsewhere = db
            .rooms_homed_elsewhere(community_id, &channels, &self.local)
            .await?;
        Ok(split_rooms(channels, homed_elsewhere))
    }

    async fn region_list(&self, db: &Db) -> Result<Arc<Vec<RegionRecord>>, kuzz_db::DbError> {
        if let Some(regions) = self.regions.get(&()) {
            return Ok(regions);
        }
        let regions = Arc::new(db.list_regions().await?);
        self.regions.insert((), Arc::clone(&regions));
        Ok(regions)
    }

    /// Where an account whose home region is `home` connects (see
    /// [`home_address`]); `local_url` is this relay's own address.
    pub async fn account_address(
        &self,
        db: &Db,
        home: Option<&str>,
        local_url: &str,
    ) -> Result<(String, String), kuzz_db::DbError> {
        let regions = self.region_list(db).await?;
        Ok(home_address(home, &regions, &self.local, local_url))
    }

    /// Drop a cached home (the room moved or was deleted).
    pub fn forget(&self, community_id: CommunityId, channel_id: Uuid) {
        self.homes.invalidate(&(community_id, channel_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: &str, url: &str, is_primary: bool) -> RegionRecord {
        RegionRecord {
            id: id.to_string(),
            connect_url: url.to_string(),
            is_primary,
        }
    }

    fn home(region: &str) -> RoomHome {
        RoomHome {
            region: region.to_string(),
            epoch: 2,
        }
    }

    #[test]
    fn one_region_serves_every_room() {
        let regions = [region("kr", "wss://kr.example", true)];
        assert_eq!(
            route_for("kr", &regions, Some(&home("jp"))),
            RoomRoute::Here
        );
        assert_eq!(route_for("kr", &regions, None), RoomRoute::Here);
    }

    #[test]
    fn rooms_of_another_region_are_sent_there() {
        let regions = [
            region("kr", "wss://kr.example", true),
            region("jp", "wss://jp.example", false),
        ];
        assert_eq!(
            route_for("kr", &regions, Some(&home("kr"))),
            RoomRoute::Here
        );
        assert_eq!(route_for("kr", &regions, None), RoomRoute::Here);
        let RoomRoute::Elsewhere(remote) = route_for("kr", &regions, Some(&home("jp"))) else {
            panic!("jp room is not served by kr");
        };
        assert_eq!(
            remote,
            RemoteHome {
                region: "jp".to_string(),
                connect_url: "wss://jp.example".to_string(),
                epoch: 2,
            }
        );
        assert_eq!(
            remote.ws_message(),
            "wrong-region: room lives in jp wss://jp.example"
        );
        // A home region that is not registered here yet still refuses.
        let RoomRoute::Elsewhere(unknown) = route_for("kr", &regions, Some(&home("sg"))) else {
            panic!("sg room is not served by kr");
        };
        assert_eq!(unknown.ws_message(), "wrong-region: room lives in sg");
    }

    #[test]
    fn multi_room_reads_keep_only_rooms_served_here() {
        let (a, b, c, d) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        let split = split_rooms(
            vec![a, b, c, d],
            vec![(b, "jp".to_string()), (d, "jp".to_string())],
        );
        assert_eq!(split.here, vec![a, c]);
        assert_eq!(
            split.elsewhere,
            BTreeMap::from([("jp".to_string(), vec![b, d])])
        );
        assert_eq!(split_rooms(vec![a, b], Vec::new()).here, vec![a, b]);
    }

    #[test]
    fn accounts_connect_to_their_home_region() {
        let regions = [
            region("kr", "wss://kr.example", true),
            region("jp", "wss://jp.example", false),
            region("sg", "", false),
        ];
        let here = |home| home_address(home, &regions, "kr", "wss://local.example");
        assert_eq!(here(Some("jp")), ("jp".into(), "wss://jp.example".into()));
        assert_eq!(here(Some("kr")), ("kr".into(), "wss://kr.example".into()));
        // Unset, unregistered or address-less homes use this relay.
        for home in [None, Some("us"), Some("sg")] {
            assert_eq!(here(home), ("kr".into(), "wss://local.example".into()));
        }
    }

    #[test]
    fn wrong_region_is_http_421_with_the_home() {
        let remote = RemoteHome {
            region: "jp".to_string(),
            connect_url: "wss://jp.example".to_string(),
            epoch: 0,
        };
        let (status, Json(body)) = remote.http_error();
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
        assert_eq!(
            body,
            serde_json::json!({
                "error": "wrong_region",
                "region": "jp",
                "connect_url": "wss://jp.example",
            })
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn writes_and_reads_of_another_regions_room_are_refused() {
        let state =
            crate::state::tests::test_state_with_database_url(&crate::test_support::database_url())
                .await;
        assert_eq!(state.room_router.local_region(), "kr");
        let pool = state.db.pool();
        sqlx::query(
            "INSERT INTO regions (id, connect_url, is_primary) VALUES              ('kr', 'wss://kr.test', NOT EXISTS (SELECT 1 FROM regions WHERE is_primary)),              ('jp', 'wss://jp.test', false) ON CONFLICT (id) DO NOTHING",
        )
        .execute(pool)
        .await
        .expect("regions");
        let community = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community.as_uuid())
            .bind(format!("routing-{}.test", community.as_uuid()))
            .execute(pool)
            .await
            .expect("community");
        let (kr_room, jp_room, unknown) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for (room, home) in [(kr_room, "kr"), (jp_room, "jp")] {
            sqlx::query(
                "INSERT INTO room_directory (community_id, channel_id, home_region)                  VALUES ($1, $2, $3)",
            )
            .bind(community.as_uuid())
            .bind(room)
            .bind(home)
            .execute(pool)
            .await
            .expect("directory row");
        }

        assert_eq!(
            state.route_room(community, kr_room).await.expect("route"),
            RoomRoute::Here
        );
        assert_eq!(
            state.route_room(community, unknown).await.expect("route"),
            RoomRoute::Here
        );
        let tenant = kuzz_core::tenant::TenantContext::resolved(community, "routing.test");
        match crate::handlers::ingest::ensure_room_here(&state, &tenant, jp_room).await {
            Err(crate::handlers::ingest::IngestError::WrongRegion(home)) => {
                assert_eq!(home.region, "jp");
                assert!(home.ws_message().starts_with(WRONG_REGION_PREFIX));
            }
            other => panic!("expected wrong region, got {other:?}"),
        }
        let first = state
            .first_room_elsewhere(community, [kr_room, unknown, jp_room])
            .await
            .expect("route");
        assert_eq!(first.map(|home| home.region), Some("jp".to_string()));

        let split = state
            .room_router
            .split(&state.db, community, vec![kr_room, jp_room, unknown])
            .await
            .expect("split");
        assert_eq!(split.here, vec![kr_room, unknown]);
        assert_eq!(split.elsewhere["jp"], vec![jp_room]);
    }
}
