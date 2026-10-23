//! Moving a room to another region (global regions S6-3, S6-4), end to end:
//! `kuzz-admin rooms move` takes a room from kr to jp while apps are
//! connected, and the room keeps its messages and order.
//!
//! Needs the two region relays started by `scripts/regions-two-region-test.sh`,
//! which runs this file last (it moves rooms) with:
//!
//! ```text
//! REGION_KR_URL=ws://localhost:3041 REGION_JP_URL=ws://localhost:3042 \
//! REGION_KR_DATABASE_URL=postgres://kuzz:kuzz_dev@127.0.0.1:5491/kuzz \
//! REGION_JP_DATABASE_URL=postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz \
//! REGION_ADMIN_BIN=target/debug/kuzz-admin \
//! REGION_REDIS_URLS=redis://127.0.0.1:6491,redis://127.0.0.1:6492 \
//! cargo test -p kuzz-test-client --test e2e_regions_move -- --ignored --test-threads=1
//! ```

use std::time::Duration;

use kuzz_test_client::KuzzTestClient;
use nostr::{Event, EventBuilder, Filter, Keys, Kind, Tag};
use serde_json::Value;

fn kr_url() -> String {
    std::env::var("REGION_KR_URL").unwrap_or_else(|_| "ws://localhost:3041".into())
}

fn jp_url() -> String {
    std::env::var("REGION_JP_URL").unwrap_or_else(|_| "ws://localhost:3042".into())
}

fn database_url(region: &str) -> String {
    let (var, port) = match region {
        "kr" => ("REGION_KR_DATABASE_URL", 5491),
        _ => ("REGION_JP_DATABASE_URL", 5492),
    };
    std::env::var(var).unwrap_or_else(|_| format!("postgres://kuzz:kuzz_dev@127.0.0.1:{port}/kuzz"))
}

async fn region_db(region: &str) -> sqlx::PgPool {
    sqlx::PgPool::connect(&database_url(region))
        .await
        .expect("region database")
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

fn accepted(answer: &(u16, Value)) -> bool {
    answer.0 == 200 && answer.1["accepted"] == Value::Bool(true)
}

fn create_room(keys: &Keys, id: &str) -> Event {
    EventBuilder::new(Kind::Custom(9007), "")
        .tags([
            Tag::parse(["h", id]).unwrap(),
            Tag::parse(["name", &format!("move-{id}")]).unwrap(),
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

/// Run `kuzz-admin rooms move` for `room` to `to`; its exit status and output.
fn move_room(room: &str, from: &str, to: &str, extra: &[&str]) -> (bool, String) {
    let admin = std::env::var("REGION_ADMIN_BIN").expect("REGION_ADMIN_BIN");
    let output = std::process::Command::new(admin)
        .args(["rooms", "move", "--channel", room, "--to", to])
        .args(extra)
        .env("DATABASE_URL", database_url("kr"))
        .env("KUZZ_MOVE_FROM_DATABASE_URL", database_url(from))
        .env("KUZZ_MOVE_TO_DATABASE_URL", database_url(to))
        .env(
            "KUZZ_MOVE_REDIS_URLS",
            std::env::var("REGION_REDIS_URLS").unwrap_or_default(),
        )
        .env("RELAY_URL", kr_url())
        .output()
        .expect("kuzz-admin");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text)
}

/// `(accepted_sequence, moving_since set, moved_out_at set)` of `room` in `region`.
async fn room_state(region: &str, room: &str) -> Option<(i64, bool, bool)> {
    sqlx::query_as(
        "SELECT accepted_sequence, moving_since IS NOT NULL, moved_out_at IS NOT NULL \
           FROM channels WHERE id = $1::uuid",
    )
    .bind(room)
    .fetch_optional(&region_db(region).await)
    .await
    .expect("room row")
}

/// The order numbers of `room`'s messages in `region`, in order.
async fn sequences(region: &str, room: &str) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT channel_sequence FROM events WHERE channel_id = $1::uuid \
           AND kind = 9 ORDER BY channel_sequence",
    )
    .bind(room)
    .fetch_all(&region_db(region).await)
    .await
    .expect("sequences")
}

/// `(home, epoch)` of `room` in `region`'s directory.
async fn home(region: &str, room: &str) -> Option<(String, i32)> {
    sqlx::query_as("SELECT home_region, epoch FROM room_directory WHERE channel_id = $1::uuid")
        .bind(room)
        .fetch_optional(&region_db(region).await)
        .await
        .expect("directory row")
}

/// Wait until jp's copy of the directory knows `room`.
async fn jp_knows(room: &str) {
    for _ in 0..300 {
        if home("jp", room).await.is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("jp's copy never learned room {room}");
}

/// Whether the connection closes within `within`.
async fn closed_within(client: &mut KuzzTestClient, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        match client.recv_event(left).await {
            Ok(_) => {}
            Err(kuzz_test_client::TestClientError::Timeout) => return false,
            Err(_) => return true,
        }
    }
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_room_moves_to_jp_with_its_messages_and_order() {
    let keys = Keys::generate();
    let room = uuid::Uuid::new_v4().to_string();
    assert!(accepted(
        &post(&kr_url(), &keys, &create_room(&keys, &room)).await
    ));
    for i in 0..5 {
        let answer = post(
            &kr_url(),
            &keys,
            &message(&keys, &room, &format!("before {i}")),
        )
        .await;
        assert!(accepted(&answer), "{answer:?}");
    }
    jp_knows(&room).await;
    let before = sequences("kr", &room).await;
    assert_eq!(before.len(), 5);
    let (accepted_before, _, _) = room_state("kr", &room).await.expect("room in kr");

    // An app watching the room at its old home.
    let mut watcher = KuzzTestClient::connect(&kr_url(), &keys)
        .await
        .expect("kr connect");
    watcher
        .subscribe(
            "room",
            vec![Filter::new().kind(Kind::Custom(9)).custom_tag(
                nostr::SingleLetterTag::lowercase(nostr::Alphabet::H),
                room.clone(),
            )],
        )
        .await
        .expect("req");
    watcher
        .collect_until_eose("room", Duration::from_secs(5))
        .await
        .expect("history");

    let (done, output) = move_room(&room, "kr", "jp", &[]);
    assert!(done, "move failed:\n{output}");
    for step in [
        "frozen",
        "copied",
        "checked",
        "switched",
        "opened",
        "moved_out",
        "done",
    ] {
        assert!(
            output.contains(&format!("\"step\":\"{step}\"")),
            "no {step} step:\n{output}"
        );
    }
    assert!(
        closed_within(&mut watcher, Duration::from_secs(20)).await,
        "the app at the old home was not told to reconnect"
    );

    // The new home has every message, numbered as before, and takes events.
    assert_eq!(home("kr", &room).await, Some(("jp".into(), 1)));
    assert_eq!(sequences("jp", &room).await, before);
    assert_eq!(
        room_state("jp", &room).await,
        Some((accepted_before, false, false))
    );
    assert_eq!(
        room_state("kr", &room).await,
        Some((accepted_before, true, true)),
        "the old home keeps the room, frozen and moved out"
    );
    let answer = post(&jp_url(), &keys, &message(&keys, &room, "after, in jp")).await;
    assert!(accepted(&answer), "{answer:?}");
    assert_eq!(
        answer.1["channel_sequence"].as_i64(),
        Some(accepted_before + 1)
    );

    // A write sent to kr reaches jp once kr's route cache has expired; until
    // then kr refuses it as moving, never stores it.
    let event = message(&keys, &room, "after, through kr");
    let mut landed = None;
    for _ in 0..45 {
        let answer = post(&kr_url(), &keys, &event).await;
        if accepted(&answer) {
            landed = Some(answer);
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let answer = landed.expect("a write through kr never reached jp");
    assert_eq!(
        answer.1["channel_sequence"].as_i64(),
        Some(accepted_before + 2),
        "{answer:?}"
    );
    let after = sequences("jp", &room).await;
    assert!(
        after.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "gap in {after:?}"
    );
    assert_eq!(sequences("kr", &room).await, before, "kr took nothing new");

    // Running the command again changes nothing; aborting is refused.
    let (again, output) = move_room(&room, "kr", "jp", &[]);
    assert!(again, "a second run failed:\n{output}");
    assert_eq!(home("kr", &room).await, Some(("jp".into(), 1)));
    let (aborted, output) = move_room(&room, "kr", "jp", &["--abort"]);
    assert!(!aborted, "an abort after the switch ran:\n{output}");
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn an_aborted_move_leaves_the_room_where_it_was() {
    let keys = Keys::generate();
    let room = uuid::Uuid::new_v4().to_string();
    assert!(accepted(
        &post(&kr_url(), &keys, &create_room(&keys, &room)).await
    ));
    assert!(accepted(
        &post(&kr_url(), &keys, &message(&keys, &room, "stays")).await
    ));
    jp_knows(&room).await;

    // A move stopped after the copy, as if the operator's machine went away.
    let kr = region_db("kr").await;
    sqlx::query("UPDATE channels SET moving_since = now() WHERE id = $1::uuid")
        .bind(&room)
        .execute(&kr)
        .await
        .expect("freeze");
    let refused = post(&kr_url(), &keys, &message(&keys, &room, "while frozen")).await;
    // Over HTTP the refusal is a plain server error; over the socket the
    // `OK` frame says the room is moving.
    assert!(!accepted(&refused), "a frozen room took a message");
    assert!(refused.0 >= 500, "{refused:?}");

    let (aborted, output) = move_room(&room, "kr", "jp", &["--abort"]);
    assert!(aborted, "abort failed:\n{output}");
    assert_eq!(room_state("jp", &room).await, None, "no copy is left in jp");
    assert_eq!(home("kr", &room).await, Some(("kr".into(), 0)));
    let answer = post(&kr_url(), &keys, &message(&keys, &room, "open again")).await;
    assert!(accepted(&answer), "{answer:?}");
}
