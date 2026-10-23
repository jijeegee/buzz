//! Moving a room (migration 0088) against real databases: the shared test
//! database is the old home, and a second database on the same server
//! (`<name>_move_target`, made on first use) is the new home.
use kuzz_core::CommunityId;
use sqlx::PgPool;
use tokio::sync::OnceCell;
use uuid::Uuid;

use super::global_copy::LOCAL_TABLES;
use super::room_move::{copy_room, RoomHere, ROOM_TABLES};
use crate::error::DbError;
use crate::Db;

async fn migrated(url: &str) -> Db {
    let pool = PgPool::connect(url).await.expect("test database");
    if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
        crate::migration::run_migrations(&pool)
            .await
            .expect("migrate");
    }
    Db::from_pool(pool)
}

async fn old_home() -> Db {
    migrated(&crate::test_support::database_url()).await
}

/// The new home: a second database next to the test database.
async fn new_home() -> Db {
    static MADE: OnceCell<String> = OnceCell::const_new();
    let url = MADE
        .get_or_init(|| async {
            let base = crate::test_support::database_url();
            let (server, name) = base.rsplit_once('/').expect("database url");
            let name = name.split('?').next().unwrap_or(name);
            let target = format!("{name}_move_target");
            let admin = PgPool::connect(&base).await.expect("test database");
            let made = sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {target}")))
                .execute(&admin)
                .await;
            if let Err(error) = made {
                assert!(
                    error.to_string().contains("already exists"),
                    "making {target}: {error}"
                );
            }
            let url = format!("{server}/{target}");
            let pool = PgPool::connect(&url).await.expect("target database");
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate the target database");
            url
        })
        .await;
    Db::from_pool(PgPool::connect(url).await.expect("target database"))
}

async fn community(dbs: &[&Db]) -> CommunityId {
    let id = CommunityId::from_uuid(Uuid::new_v4());
    for db in dbs {
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(id.as_uuid())
            .bind(format!("move-{}.test", id.as_uuid()))
            .execute(db.pool())
            .await
            .expect("fixture community");
    }
    id
}

async fn room(db: &Db, community: CommunityId) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channels (id, community_id, name, channel_type, visibility, created_by) \
         VALUES ($1, $2, $3, 'stream', 'open', $4)",
    )
    .bind(id)
    .bind(community.as_uuid())
    .bind(format!("room-{id}"))
    .bind([7u8; 32].as_slice())
    .execute(db.pool())
    .await
    .expect("fixture room");
    id
}

/// Insert a message in `channel`; its id.
async fn message(db: &Db, community: CommunityId, channel: Uuid) -> Result<Vec<u8>, DbError> {
    let id = Uuid::new_v4().as_bytes().repeat(2);
    sqlx::query(
        "INSERT INTO events (community_id, id, pubkey, created_at, kind, tags, content, channel_id) \
         VALUES ($1, $2, $3, now(), 9, '[]', 'hello', $4)",
    )
    .bind(community.as_uuid())
    .bind(&id)
    .bind([7u8; 32].as_slice())
    .bind(channel)
    .execute(db.pool())
    .await?;
    Ok(id)
}

/// A room with three messages, a member who has read one, a reaction and a
/// mention.
async fn busy_room(db: &Db, community: CommunityId) -> Uuid {
    let channel = room(db, community).await;
    sqlx::query(
        "INSERT INTO channel_members (community_id, channel_id, pubkey, role) \
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(community.as_uuid())
    .bind(channel)
    .bind([8u8; 32].as_slice())
    .execute(db.pool())
    .await
    .expect("member");
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(message(db, community, channel).await.expect("message"));
    }
    sqlx::query(
        "UPDATE channel_members SET read_seq = 1 \
          WHERE community_id = $1 AND channel_id = $2",
    )
    .bind(community.as_uuid())
    .bind(channel)
    .execute(db.pool())
    .await
    .expect("read position");
    sqlx::query(
        "INSERT INTO reactions (community_id, event_created_at, event_id, pubkey, emoji) \
         SELECT community_id, created_at, id, $3, '+' FROM events \
          WHERE community_id = $1 AND id = $2",
    )
    .bind(community.as_uuid())
    .bind(&ids[0])
    .bind([8u8; 32].as_slice())
    .execute(db.pool())
    .await
    .expect("reaction");
    sqlx::query(
        "INSERT INTO event_mentions (community_id, pubkey_hex, event_id, event_created_at, channel_id, event_kind) \
         SELECT community_id, $3, id, created_at, channel_id, kind FROM events \
          WHERE community_id = $1 AND id = $2",
    )
    .bind(community.as_uuid())
    .bind(&ids[1])
    .bind("08".repeat(32))
    .execute(db.pool())
    .await
    .expect("mention");
    channel
}

/// One member row as a comparable tuple.
async fn member(db: &Db, community: CommunityId, channel: Uuid) -> (i64, Option<i64>, i32) {
    sqlx::query_as(
        "SELECT join_sequence, read_seq, unread_count FROM channel_members \
          WHERE community_id = $1 AND channel_id = $2",
    )
    .bind(community.as_uuid())
    .bind(channel)
    .fetch_one(db.pool())
    .await
    .expect("member row")
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_frozen_room_takes_no_events() {
    let db = old_home().await;
    let community = community(&[&db]).await;
    let channel = room(&db, community).await;
    message(&db, community, channel).await.expect("open room");

    let since = db.freeze_room(community, channel).await.expect("freeze");
    assert_eq!(
        db.freeze_room(community, channel).await.expect("again"),
        since,
        "freezing again keeps the first time"
    );
    let refused = message(&db, community, channel).await.expect_err("frozen");
    assert!(refused.is_room_moving(), "{refused}");
    assert_eq!(
        db.room_here(community, channel).await.unwrap(),
        RoomHere::Frozen(since)
    );

    assert!(db.unfreeze_room(community, channel).await.unwrap());
    message(&db, community, channel).await.expect("open again");

    // A room that moved out stays frozen.
    db.freeze_room(community, channel).await.unwrap();
    sqlx::query("UPDATE channels SET moved_out_at = now() WHERE community_id = $1 AND id = $2")
        .bind(community.as_uuid())
        .bind(channel)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(!db.unfreeze_room(community, channel).await.unwrap());
    assert!(matches!(
        db.freeze_room(community, channel).await,
        Err(DbError::NotFound(_))
    ));
    assert!(message(&db, community, channel)
        .await
        .expect_err("moved out")
        .is_room_moving());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_room_is_copied_as_it_is() {
    let (from, to) = (old_home().await, new_home().await);
    let community = community(&[&from, &to]).await;
    let channel = busy_room(&from, community).await;

    let open = copy_room(&from, &to, community, channel)
        .await
        .expect_err("an open room is not copied");
    assert!(open.to_string().contains("freeze it first"), "{open}");

    from.freeze_room(community, channel).await.unwrap();
    let copied = copy_room(&from, &to, community, channel)
        .await
        .expect("copy");
    let rows = |table: &str| copied.iter().find(|c| c.table == table).unwrap().rows;
    assert_eq!(rows("channels"), 1);
    assert_eq!(rows("events"), 3);
    assert_eq!(rows("reactions"), 1);
    assert_eq!(rows("event_mentions"), 1);

    let old = from.room_fingerprint(community, channel).await.unwrap();
    assert_eq!(old.accepted_sequence, 3);
    assert_eq!(old.last_sequence, 3);
    assert_eq!(to.room_fingerprint(community, channel).await.unwrap(), old);
    assert_eq!(
        member(&to, community, channel).await,
        member(&from, community, channel).await,
        "join cutoff, read position and unread count are kept"
    );
    assert!(
        matches!(
            to.room_here(community, channel).await.unwrap(),
            RoomHere::Frozen(_)
        ),
        "the copy stays frozen until it is opened"
    );
    assert!(message(&to, community, channel)
        .await
        .expect_err("copy frozen")
        .is_room_moving());

    let again = copy_room(&from, &to, community, channel)
        .await
        .expect_err("copied once");
    assert!(again.to_string().contains("already holds"), "{again}");

    assert!(to.delete_room_copy(community, channel).await.unwrap() >= 7);
    assert_eq!(
        to.room_here(community, channel).await.unwrap(),
        RoomHere::Absent
    );
    assert_eq!(
        to.room_fingerprint(community, channel)
            .await
            .map(|f| f.events)
            .ok(),
        None,
        "nothing of the room is left"
    );
    // The old home still has everything.
    assert_eq!(
        from.room_fingerprint(community, channel).await.unwrap(),
        old
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn side_effects_left_hold_the_copy() {
    let (from, to) = (old_home().await, new_home().await);
    let community = community(&[&from, &to]).await;
    let channel = room(&from, community).await;
    let id = message(&from, community, channel).await.unwrap();
    sqlx::query("INSERT INTO event_side_effects (community_id, event_id, kind) VALUES ($1, $2, 9)")
        .bind(community.as_uuid())
        .bind(&id)
        .execute(from.pool())
        .await
        .unwrap();
    from.freeze_room(community, channel).await.unwrap();
    assert_eq!(
        from.room_side_effects_pending(community, channel)
            .await
            .unwrap(),
        1
    );
    let waiting = copy_room(&from, &to, community, channel)
        .await
        .expect_err("side effects left");
    assert!(waiting.to_string().contains("side effects"), "{waiting}");
    assert_eq!(
        to.room_here(community, channel).await.unwrap(),
        RoomHere::Absent
    );

    sqlx::query("DELETE FROM event_side_effects WHERE community_id = $1")
        .bind(community.as_uuid())
        .execute(from.pool())
        .await
        .unwrap();
    copy_room(&from, &to, community, channel)
        .await
        .expect("copy once drained");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn an_open_room_is_never_removed_as_a_copy() {
    let db = old_home().await;
    let community = community(&[&db]).await;
    let channel = room(&db, community).await;
    let refused = db
        .delete_room_copy(community, channel)
        .await
        .expect_err("open room");
    assert!(
        refused.to_string().contains("not an unopened copy"),
        "{refused}"
    );
    assert_eq!(
        db.room_here(community, channel).await.unwrap(),
        RoomHere::Open
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn every_local_table_with_a_room_column_moves_with_the_room() {
    let db = old_home().await;
    let with_room: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.columns \
          WHERE table_schema = current_schema() AND column_name = 'channel_id'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    for table in with_room {
        if !LOCAL_TABLES.contains(&table.as_str()) {
            continue; // copied from the primary, or a partition
        }
        assert!(
            ROOM_TABLES.iter().any(|(listed, _)| *listed == table),
            "{table} has a room column but is not moved with the room: add it to ROOM_TABLES"
        );
    }
}

/// Insert an event of `kind` with `id` in `channel`.
async fn event(
    db: &Db,
    community: CommunityId,
    channel: Uuid,
    id: &[u8],
    kind: i32,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO events (community_id, id, pubkey, created_at, kind, tags, content, channel_id) \
         VALUES ($1, $2, $3, '2026-10-24T00:00:00Z', $4, '[]', 'x', $5)",
    )
    .bind(community.as_uuid())
    .bind(id)
    .bind([7u8; 32].as_slice())
    .bind(kind)
    .bind(channel)
    .execute(db.pool())
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_retried_event_is_refused_once_frozen() {
    let db = old_home().await;
    let community = community(&[&db]).await;
    let channel = room(&db, community).await;
    let id = Uuid::new_v4().as_bytes().repeat(2);
    event(&db, community, channel, &id, 7).await.expect("open");
    db.freeze_room(community, channel).await.unwrap();
    let refused = event(&db, community, channel, &id, 7)
        .await
        .expect_err("a retry is refused too");
    assert!(refused.is_room_moving(), "{refused}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn side_effects_drain_through_the_freeze() {
    let db = old_home().await;
    let community = community(&[&db]).await;
    let channel = room(&db, community).await;
    let trigger = message(&db, community, channel).await.unwrap();
    sqlx::query(
        "INSERT INTO event_side_effects (community_id, event_id, kind) VALUES ($1, $2, 9000)",
    )
    .bind(community.as_uuid())
    .bind(&trigger)
    .execute(db.pool())
    .await
    .unwrap();
    db.freeze_room(community, channel).await.unwrap();

    let system = |n: u8| {
        let mut id = Uuid::new_v4().as_bytes().repeat(2);
        id[0] = n;
        id
    };
    event(&db, community, channel, &system(1), 40099)
        .await
        .expect("the relay's room message lands while side effects drain");
    event(&db, community, channel, &system(2), 39002)
        .await
        .expect("so does the group snapshot");
    assert!(event(&db, community, channel, &system(3), 9)
        .await
        .expect_err("a member's message does not")
        .is_room_moving());

    sqlx::query("DELETE FROM event_side_effects WHERE community_id = $1")
        .bind(community.as_uuid())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(event(&db, community, channel, &system(4), 40099)
        .await
        .expect_err("drained: nothing lands")
        .is_room_moving());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn the_old_home_is_never_removed_as_a_copy() {
    let db = old_home().await;
    let community = community(&[&db]).await;
    let channel = busy_room(&db, community).await;
    db.freeze_room(community, channel).await.unwrap();
    let refused = db
        .delete_room_copy(community, channel)
        .await
        .expect_err("frozen, but not a copy");
    assert!(
        refused.to_string().contains("not an unopened copy"),
        "{refused}"
    );
    assert_eq!(
        db.room_fingerprint(community, channel)
            .await
            .unwrap()
            .events,
        3
    );
}

/// The new home's database as region `mvb`, the primary; `mva` is the
/// other region. Only these tests use that database.
async fn claimed_new_home() -> Db {
    let db = new_home().await;
    for sql in [
        "INSERT INTO regions (id, connect_url, is_primary) VALUES ('mvb', 'wss://mvb.test', true) \
         ON CONFLICT (id) DO NOTHING",
        "INSERT INTO regions (id, connect_url) VALUES ('mva', 'wss://mva.test') \
         ON CONFLICT (id) DO NOTHING",
        "INSERT INTO local_region (region_id) VALUES ('mvb') ON CONFLICT (singleton) DO NOTHING",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(db.pool())
            .await
            .expect(sql);
    }
    assert_eq!(db.local_region().await.unwrap().as_deref(), Some("mvb"));
    db
}

/// Set the directory's home of `channel`.
async fn homed(db: &Db, community: CommunityId, channel: Uuid, region: &str, epoch: i32) {
    sqlx::query(
        "INSERT INTO room_directory (community_id, channel_id, home_region, epoch) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (community_id, channel_id) \
         DO UPDATE SET home_region = EXCLUDED.home_region, epoch = EXCLUDED.epoch",
    )
    .bind(community.as_uuid())
    .bind(channel)
    .bind(region)
    .bind(epoch)
    .execute(db.pool())
    .await
    .expect("directory row");
}

/// Mark `channel` as a move's copy not opened yet.
async fn as_copy(db: &Db, community: CommunityId, channel: Uuid) {
    db.freeze_room(community, channel).await.unwrap();
    sqlx::query("UPDATE channels SET moved_in_at = now() WHERE community_id = $1 AND id = $2")
        .bind(community.as_uuid())
        .bind(channel)
        .execute(db.pool())
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_moved_room_opens_only_where_the_directory_homes_it() {
    use super::region::RoomHome;

    let db = claimed_new_home().await;
    let community = community(&[&db]).await;
    let channel = room(&db, community).await;
    homed(&db, community, channel, "mva", 0).await;
    as_copy(&db, community, channel).await;

    let elsewhere = db
        .unfreeze_room(community, channel)
        .await
        .expect_err("homed in mva");
    assert!(elsewhere.to_string().contains("region mva"), "{elsewhere}");

    let from = RoomHome {
        region: "mva".into(),
        epoch: 0,
    };
    let moved = db
        .switch_room_home(community, channel, &from, "mvb")
        .await
        .expect("switch");
    assert_eq!((moved.region.as_str(), moved.epoch), ("mvb", 1));
    db.switch_room_home(community, channel, &from, "mvb")
        .await
        .expect_err("a stale home does not switch again");
    db.switch_room_home(community, channel, &moved, "nowhere")
        .await
        .expect_err("an unknown region");

    assert!(db.unfreeze_room(community, channel).await.expect("open"));
    assert_eq!(
        db.room_here(community, channel).await.unwrap(),
        RoomHere::Open
    );
    let opened = db
        .delete_room_copy(community, channel)
        .await
        .expect_err("an opened copy is the room");
    assert!(
        opened.to_string().contains("not an unopened copy"),
        "{opened}"
    );
    message(&db, community, channel)
        .await
        .expect("takes events");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_room_moved_out_is_kept_then_forgotten() {
    let db = claimed_new_home().await;
    let community = community(&[&db]).await;
    let channel = room(&db, community).await;
    message(&db, community, channel).await.unwrap();
    homed(&db, community, channel, "mvb", 0).await;
    db.freeze_room(community, channel).await.unwrap();

    let here = db
        .mark_room_moved_out(community, channel)
        .await
        .expect_err("still homed here");
    assert!(
        here.to_string().contains("this database's region"),
        "{here}"
    );

    homed(&db, community, channel, "mva", 1).await;
    db.mark_room_moved_out(community, channel)
        .await
        .expect("moved out");
    db.mark_room_moved_out(community, channel)
        .await
        .expect("again changes nothing");
    assert!(matches!(
        db.room_here(community, channel).await.unwrap(),
        RoomHere::MovedOut(_)
    ));
    assert!(message(&db, community, channel)
        .await
        .expect_err("read-only")
        .is_room_moving());

    let kept = db
        .forget_moved_room(community, channel, 7)
        .await
        .expect_err("kept a week");
    assert!(kept.to_string().contains("kept until"), "{kept}");
    assert!(db.forget_moved_room(community, channel, 0).await.unwrap() >= 2);
    assert_eq!(
        db.room_here(community, channel).await.unwrap(),
        RoomHere::Absent
    );

    // A copy the directory homes here is the room: never removed.
    let other = room(&db, community).await;
    homed(&db, community, other, "mvb", 0).await;
    as_copy(&db, community, other).await;
    let refused = db
        .delete_room_copy(community, other)
        .await
        .expect_err("homed here");
    assert!(
        refused.to_string().contains("this database's region"),
        "{refused}"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_frozen_room_runs_no_schedules() {
    let db = claimed_new_home().await;
    let community = community(&[&db]).await;
    let channel = room(&db, community).await;
    sqlx::query("INSERT INTO users (community_id, pubkey) VALUES ($1, $2)")
        .bind(community.as_uuid())
        .bind([7u8; 32].as_slice())
        .execute(db.pool())
        .await
        .expect("owner");
    let workflow: Uuid = sqlx::query_scalar(
        r#"INSERT INTO workflows (community_id, name, owner_pubkey, channel_id, definition, definition_hash)
           VALUES ($1, 'tick', $2, $3, '{"trigger": {"on": "schedule"}}', '\x00') RETURNING id"#,
    )
    .bind(community.as_uuid())
    .bind([7u8; 32].as_slice())
    .bind(channel)
    .fetch_one(db.pool())
    .await
    .expect("workflow");
    let listed = || async {
        super::workflow::list_all_enabled_workflows(db.pool())
            .await
            .unwrap()
            .iter()
            .any(|w| w.id == workflow)
    };
    assert!(listed().await);
    db.freeze_room(community, channel).await.unwrap();
    assert!(!listed().await, "frozen: no schedule fires");
}
