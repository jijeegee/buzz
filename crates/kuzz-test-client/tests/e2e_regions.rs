//! Two-region routing (global regions S3): a room is served only by its home
//! region; the other region answers "wrong region" with the home's address.
//!
//! Needs two relays with separate databases, started by
//! `scripts/regions-two-region-test.sh`, which also runs this file:
//!
//! ```text
//! REGION_KR_URL=ws://localhost:3041 REGION_JP_URL=ws://localhost:3042 \
//! REGION_JP_DATABASE_URL=postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz \
//! cargo test -p kuzz-test-client --test e2e_regions -- --ignored --test-threads=1
//! ```
//!
//! Regions do not copy their room directories to each other until stage S5,
//! so the test writes the other region's directory row itself.

use std::time::Duration;

use kuzz_test_client::{KuzzTestClient, RelayMessage};
use nostr::{Event, EventBuilder, Filter, Keys, Kind, SingleLetterTag, Tag};
use serde_json::Value;

fn kr_url() -> String {
    std::env::var("REGION_KR_URL").unwrap_or_else(|_| "ws://localhost:3041".into())
}

fn jp_url() -> String {
    std::env::var("REGION_JP_URL").unwrap_or_else(|_| "ws://localhost:3042".into())
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

fn create_room(keys: &Keys, id: &str) -> Event {
    EventBuilder::new(Kind::Custom(9007), "")
        .tags([
            Tag::parse(["h", id]).unwrap(),
            Tag::parse(["name", &format!("regions-{id}")]).unwrap(),
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

/// Record in the jp database that `room` lives in kr.
async fn tell_jp_room_lives_in_kr(room: &str) {
    let url = std::env::var("REGION_JP_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz".into());
    let pool = sqlx::PgPool::connect(&url).await.expect("jp database");
    sqlx::query(
        "INSERT INTO room_directory (community_id, channel_id, home_region) \
         SELECT id, $1::uuid, 'kr' FROM communities",
    )
    .bind(room)
    .execute(&pool)
    .await
    .expect("directory row");
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn rooms_are_served_only_by_their_home_region() {
    let keys = Keys::generate();

    // A room created in kr is served by kr.
    let room = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(&kr_url(), &keys, &create_room(&keys, &room)).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    let (status, body) = post(&kr_url(), &keys, &message(&keys, &room, "hi kr")).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );

    tell_jp_room_lives_in_kr(&room).await;

    // jp neither writes the room nor creates a second copy of it.
    for event in [message(&keys, &room, "hi jp"), create_room(&keys, &room)] {
        let (status, body) = post(&jp_url(), &keys, &event).await;
        assert_eq!(status, 421, "{body}");
        assert_eq!(body["error"], "wrong_region");
        assert_eq!(body["region"], "kr");
        assert_eq!(body["connect_url"], kr_url());
    }

    // Over WS: writes get OK false, reads get CLOSED, both naming kr.
    let mut jp = KuzzTestClient::connect(&jp_url(), &keys)
        .await
        .expect("jp connect");
    let ok = jp
        .send_event(message(&keys, &room, "hi jp ws"))
        .await
        .expect("ok");
    assert!(!ok.accepted);
    assert_eq!(
        ok.message,
        format!("wrong-region: room lives in kr {}", kr_url())
    );
    let filter =
        Filter::new().custom_tag(SingleLetterTag::lowercase(nostr::Alphabet::H), room.clone());
    jp.subscribe("regions", vec![filter]).await.expect("req");
    loop {
        match jp.recv_event(Duration::from_secs(5)).await.expect("reply") {
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == "regions" => {
                assert!(
                    message.starts_with("wrong-region: room lives in kr"),
                    "{message}"
                );
                break;
            }
            RelayMessage::Event { .. } | RelayMessage::Eose { .. } => {
                panic!("jp served a kr room")
            }
            _ => {}
        }
    }

    // A room created in jp is jp's own.
    let jp_room = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(&jp_url(), &keys, &create_room(&keys, &jp_room)).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    let ok = jp
        .send_event(message(&keys, &jp_room, "jp's own room"))
        .await
        .expect("ok");
    assert!(ok.accepted, "{}", ok.message);
}
