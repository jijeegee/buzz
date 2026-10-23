//! Handing the primary to another region (global regions S6-5), end to end:
//! after `kuzz-admin primary hand-over --to jp` and a restart of every relay
//! with `KUZZ_PRIMARY_REGION=jp`, jp holds the global tables and kr copies
//! them, and both regions keep working.
//!
//! Needs the relays restarted by `scripts/regions-two-region-test.sh`, which
//! runs this file after the handover with:
//!
//! ```text
//! REGION_KR_URL=ws://localhost:3041 REGION_JP_URL=ws://localhost:3042 \
//! REGION_KR_DATABASE_URL=postgres://kuzz:kuzz_dev@127.0.0.1:5491/kuzz \
//! REGION_JP_DATABASE_URL=postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz \
//! cargo test -p kuzz-test-client --test e2e_regions_handover -- --ignored --test-threads=1
//! ```

use std::time::Duration;

use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::Value;

fn kr_url() -> String {
    std::env::var("REGION_KR_URL").unwrap_or_else(|_| "ws://localhost:3041".into())
}

fn jp_url() -> String {
    std::env::var("REGION_JP_URL").unwrap_or_else(|_| "ws://localhost:3042".into())
}

async fn region_db(region: &str) -> sqlx::PgPool {
    let (var, port) = match region {
        "kr" => ("REGION_KR_DATABASE_URL", 5491),
        _ => ("REGION_JP_DATABASE_URL", 5492),
    };
    let url = std::env::var(var)
        .unwrap_or_else(|_| format!("postgres://kuzz:kuzz_dev@127.0.0.1:{port}/kuzz"));
    sqlx::PgPool::connect(&url).await.expect("region database")
}

fn http(url: &str) -> String {
    url.replace("ws://", "http://")
        .replace("wss://", "https://")
}

/// POST an event over HTTP (dev-mode `X-Pubkey`); returns status and body.
async fn post(url: &str, keys: &Keys, event: &Event) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/events", http(url)))
        .header("X-Pubkey", keys.public_key().to_hex())
        .json(event)
        .send()
        .await
        .expect("event request");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn assert_accepted(answer: &(u16, Value)) {
    assert_eq!(
        (answer.0, &answer.1["accepted"]),
        (200, &Value::Bool(true)),
        "{}",
        answer.1
    );
}

fn create_room(keys: &Keys, id: &str) -> Event {
    EventBuilder::new(Kind::Custom(9007), "")
        .tags([
            Tag::parse(["h", id]).unwrap(),
            Tag::parse(["name", &format!("handover-{id}")]).unwrap(),
            Tag::parse(["channel_type", "stream"]).unwrap(),
            Tag::parse(["visibility", "open"]).unwrap(),
        ])
        .sign_with_keys(keys)
        .unwrap()
}

fn message(keys: &Keys, room: &str, text: &str) -> Event {
    EventBuilder::new(Kind::Custom(9), text)
        .tags([Tag::parse(["h", room]).unwrap()])
        .sign_with_keys(keys)
        .unwrap()
}

/// `room`'s home in `region`'s directory.
async fn directory_home(region: &str, room: &str) -> Option<String> {
    sqlx::query_scalar("SELECT home_region FROM room_directory WHERE channel_id = $1::uuid")
        .bind(room)
        .fetch_optional(&region_db(region).await)
        .await
        .expect("directory row")
}

/// Wait until kr's copy shows `room` homed in `home`.
async fn kr_copy_homes(room: &str, home: &str) -> bool {
    for _ in 0..300 {
        if directory_home("kr", room).await.as_deref() == Some(home) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn room_rows(region: &str, room: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM channels WHERE id = $1::uuid")
        .bind(room)
        .fetch_one(&region_db(region).await)
        .await
        .expect("channel rows")
}

#[tokio::test]
#[ignore = "requires the relays restarted after a handover (scripts/regions-two-region-test.sh)"]
async fn jp_is_the_primary_and_kr_copies_it() {
    for region in ["kr", "jp"] {
        let db = region_db(region).await;
        let primaries: Vec<String> =
            sqlx::query_scalar("SELECT id FROM regions WHERE is_primary ORDER BY id")
                .fetch_all(&db)
                .await
                .expect("regions");
        assert_eq!(primaries, ["jp"], "{region}'s list of regions");
    }
    let jp_copy: bool = sqlx::query_scalar("SELECT global_copy FROM local_region")
        .fetch_one(&region_db("jp").await)
        .await
        .expect("jp local_region");
    assert!(!jp_copy, "jp is still a copy");
    let kr_copy: bool = sqlx::query_scalar("SELECT global_copy FROM local_region")
        .fetch_one(&region_db("kr").await)
        .await
        .expect("kr local_region");
    assert!(kr_copy, "kr is not a copy");

    // kr's copy hears jp's heartbeat.
    let mut lag = f64::MAX;
    for _ in 0..30 {
        let metrics = reqwest::get("http://localhost:3061/metrics")
            .await
            .expect("kr metrics")
            .text()
            .await
            .expect("metrics body");
        lag = metrics
            .lines()
            .find(|line| line.starts_with("kuzz_global_copy_lag_seconds"))
            .and_then(|line| line.split_whitespace().last())
            .and_then(|value| value.parse().ok())
            .unwrap_or(f64::MAX);
        if lag < 5.0 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(lag < 5.0, "kr's copy is {lag} s behind");
}

#[tokio::test]
#[ignore = "requires the relays restarted after a handover (scripts/regions-two-region-test.sh)"]
async fn rooms_are_registered_at_the_new_primary() {
    let keys = Keys::generate();

    // Created through kr: reserved at jp for kr, copied back to kr.
    let kr_room = uuid::Uuid::new_v4().to_string();
    assert_accepted(&post(&kr_url(), &keys, &create_room(&keys, &kr_room)).await);
    assert_eq!(directory_home("jp", &kr_room).await.as_deref(), Some("kr"));
    assert!(
        kr_copy_homes(&kr_room, "kr").await,
        "kr's copy lacks its room"
    );
    assert_eq!(room_rows("kr", &kr_room).await, 1);

    // A message sent through jp reaches the kr room.
    let answer = post(&jp_url(), &keys, &message(&keys, &kr_room, "via jp")).await;
    assert_accepted(&answer);
    assert!(
        answer.1["channel_sequence"].as_i64().is_some(),
        "{}",
        answer.1
    );
    assert_eq!(room_rows("jp", &kr_room).await, 0);

    // Created through jp: lives in jp, kr's copy of the directory knows it.
    let jp_room = uuid::Uuid::new_v4().to_string();
    assert_accepted(&post(&jp_url(), &keys, &create_room(&keys, &jp_room)).await);
    assert_eq!(room_rows("jp", &jp_room).await, 1);
    assert!(
        kr_copy_homes(&jp_room, "jp").await,
        "kr's copy lacks jp's room"
    );
    assert_eq!(room_rows("kr", &jp_room).await, 0);
}

#[tokio::test]
#[ignore = "requires the relays restarted after a handover (scripts/regions-two-region-test.sh)"]
async fn global_writes_go_to_jp_and_kr_refuses_them_locally() {
    // A profile saved in jp is copied to kr.
    let keys = Keys::generate();
    let profile = EventBuilder::new(Kind::Metadata, r#"{"name":"after handover"}"#)
        .sign_with_keys(&keys)
        .unwrap();
    assert_accepted(&post(&jp_url(), &keys, &profile).await);
    let kr = region_db("kr").await;
    let mut copied = false;
    for _ in 0..300 {
        copied = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users WHERE pubkey = decode($1, 'hex'))",
        )
        .bind(keys.public_key().to_hex())
        .fetch_one(&kr)
        .await
        .expect("kr users");
        if copied {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(copied, "the profile row was not copied to kr");

    // kr, the old primary, now refuses global writes of its own.
    for sql in [
        "INSERT INTO regions (id, connect_url) VALUES ('zz', 'ws://zz.test')",
        "UPDATE regions SET connect_url = connect_url WHERE id = 'jp'",
        "UPDATE communities SET host = host",
    ] {
        let error = sqlx::query(sql)
            .execute(&kr)
            .await
            .expect_err(sql)
            .to_string();
        assert!(
            error.contains("written in the primary region"),
            "{sql}: {error}"
        );
    }

    // ... and jp, the new primary, takes them.
    sqlx::query("UPDATE communities SET host = host")
        .execute(&region_db("jp").await)
        .await
        .expect("jp writes its global tables");
}
