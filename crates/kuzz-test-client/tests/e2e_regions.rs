//! Two-region routing (global regions S3): a room is served only by its home
//! region; the other region answers "wrong region" with the home's address.
//! With the region link (S4), the other region forwards a logged-in user's
//! write to the home instead.
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

/// POST `/query` filters over HTTP (dev-mode `X-Pubkey`).
async fn query(url: &str, keys: &Keys, filters: Value) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/query", http(url)))
        .header("X-Pubkey", keys.public_key().to_hex())
        .json(&filters)
        .send()
        .await
        .expect("query request");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// The `chat_list` row of `room` in `page`.
fn room_row<'a>(page: &'a Value, room: &str) -> Option<&'a Value> {
    page["rows"]
        .as_array()?
        .iter()
        .find(|row| row["type"] == "room" && row["channel_id"] == room)
}

/// How many rows jp's database has for `room`.
async fn jp_room_rows(room: &str) -> i64 {
    let url = std::env::var("REGION_JP_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz".into());
    let pool = sqlx::PgPool::connect(&url).await.expect("jp database");
    sqlx::query_scalar("SELECT count(*) FROM channels WHERE id = $1::uuid")
        .bind(room)
        .fetch_one(&pool)
        .await
        .expect("channel rows")
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

    // jp forwards writes for the room to kr (S4-2) and never keeps its own
    // copy of the room: the answer is kr's, with kr's order number.
    let (status, body) = post(&jp_url(), &keys, &message(&keys, &room, "hi jp")).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    assert!(body["channel_sequence"].as_i64().is_some(), "{body}");
    let (status, body) = post(&jp_url(), &keys, &create_room(&keys, &room)).await;
    assert!(status < 500, "{status} {body}");
    assert_eq!(jp_room_rows(&room).await, 0, "jp made a copy of a kr room");

    // Over WS (NIP-42) jp forwards the write to kr (S4-2): accepted once,
    // stored in kr, and a resend is kr's duplicate answer.
    let mut jp = KuzzTestClient::connect(&jp_url(), &keys)
        .await
        .expect("jp connect");
    let forwarded = message(&keys, &room, "hi jp ws");
    let ok = jp.send_event(forwarded.clone()).await.expect("ok");
    assert!(ok.accepted, "{}", ok.message);
    let ok = jp.send_event(forwarded.clone()).await.expect("ok");
    assert!(ok.accepted, "{}", ok.message);
    assert!(ok.message.starts_with("duplicate:"), "{}", ok.message);
    let mut kr = KuzzTestClient::connect(&kr_url(), &keys)
        .await
        .expect("kr connect");
    let in_room = Filter::new()
        .id(forwarded.id)
        .custom_tag(SingleLetterTag::lowercase(nostr::Alphabet::H), room.clone());
    kr.subscribe("kr-room", vec![in_room]).await.expect("req");
    let stored = kr
        .collect_until_eose("kr-room", Duration::from_secs(5))
        .await
        .expect("kr read");
    assert_eq!(stored.len(), 1, "the forwarded write is stored in kr");
    assert_eq!(stored[0].content, "hi jp ws");

    // Reads of the kr room through jp are answered by kr (S4-3): REQ,
    // COUNT, and HTTP /query all see the messages written in both regions.
    let h = SingleLetterTag::lowercase(nostr::Alphabet::H);
    let in_kr_room = Filter::new()
        .kind(Kind::Custom(9))
        .custom_tag(h, room.clone());
    jp.subscribe("regions", vec![in_kr_room.clone()])
        .await
        .expect("req");
    let read = jp
        .collect_until_eose("regions", Duration::from_secs(10))
        .await
        .expect("jp read of a kr room");
    let mut texts: Vec<_> = read.iter().map(|event| event.content.as_str()).collect();
    texts.sort_unstable();
    assert_eq!(texts, ["hi jp", "hi jp ws", "hi kr"]);
    jp.send_raw(&serde_json::json!(["COUNT", "regions-count", in_kr_room]))
        .await
        .expect("count");
    loop {
        match jp
            .recv_event(Duration::from_secs(10))
            .await
            .expect("count reply")
        {
            RelayMessage::Count {
                subscription_id,
                count,
            } if subscription_id == "regions-count" => {
                assert_eq!(count, 3);
                break;
            }
            RelayMessage::Closed { message, .. } => panic!("count refused: {message}"),
            _ => {}
        }
    }
    let response = reqwest::Client::new()
        .post(format!("{}/query", http(&jp_url())))
        .header("X-Pubkey", keys.public_key().to_hex())
        .json(&serde_json::json!([{"kinds": [9], "#h": [room]}]))
        .send()
        .await
        .expect("query");
    assert_eq!(response.status().as_u16(), 200);
    let events: Value = response.json().await.expect("query body");
    assert_eq!(events.as_array().map(Vec::len), Some(3), "{events}");

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

    // Chats through jp list the rooms of both regions (S4-3b), merged
    // newest first, with a cursor apps send back unchanged.
    let (status, chats) = query(&jp_url(), &keys, serde_json::json!([{"chat_list": 1}])).await;
    assert_eq!(status, 200, "{chats}");
    let kr_row = room_row(&chats, &room).unwrap_or_else(|| panic!("kr room missing: {chats}"));
    assert!(
        room_row(&chats, &jp_room).is_some(),
        "jp room missing: {chats}"
    );
    assert!(chats.get("unavailable_regions").is_none(), "{chats}");
    let last_seq = kr_row["last_seq"].as_i64().expect("last_seq");
    let (status, page) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"chat_list": 1, "limit": 1}]),
    )
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["rows"].as_array().map(Vec::len), Some(1), "{page}");
    let (status, rest) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"chat_list": 1, "limit": 1, "before": page["next"]}]),
    )
    .await;
    assert_eq!(status, 200, "{rest}");
    assert_ne!(rest["rows"][0]["key"], page["rows"][0]["key"], "{rest}");
    let (status, inbox) = query(&jp_url(), &keys, serde_json::json!([{"inbox_summary": 1}])).await;
    assert_eq!(status, 200, "{inbox}");
    assert!(
        inbox["rows"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["channel_id"] == room)),
        "kr room missing from the inbox: {inbox}"
    );

    // A read position set through jp is kept by the room's home, kr.
    let (status, marked) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"mark_read": 1, "items": [
            {"channel": jp_room},
            {"channel": room, "seq": last_seq}
        ]}]),
    )
    .await;
    assert_eq!(status, 200, "{marked}");
    assert_eq!(marked["items"][1]["channel_id"], room, "{marked}");
    assert_eq!(marked["items"][1]["read_seq"], last_seq, "{marked}");
    assert_eq!(marked["items"][0]["channel_id"], jp_room, "{marked}");
    let (_, kr_chats) = query(&kr_url(), &keys, serde_json::json!([{"chat_list": 1}])).await;
    let kr_row = room_row(&kr_chats, &room).unwrap_or_else(|| panic!("{kr_chats}"));
    assert_eq!(kr_row["read_seq"], last_seq, "{kr_chats}");
    // kr merges in jp's rooms the same way.
    assert!(
        room_row(&kr_chats, &jp_room).is_some(),
        "kr did not list the jp room: {kr_chats}"
    );
    // A refused item is named by its place in the whole request.
    let stranger = uuid::Uuid::new_v4().to_string();
    tell_jp_room_lives_in_kr(&stranger).await;
    let (status, refused) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"mark_read": 1, "items": [
            {"channel": jp_room}, {"channel": room}, {"channel": stranger}
        ]}]),
    )
    .await;
    assert_eq!(status, 403, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .is_some_and(|error| error.starts_with("mark_read item 2:")),
        "{refused}"
    );

    // A read spanning both regions is merged from each home (S4-3c): REQ,
    // COUNT, HTTP /query and /count, and a read naming no room.
    let both = Filter::new()
        .kind(Kind::Custom(9))
        .custom_tag(SingleLetterTag::lowercase(nostr::Alphabet::H), room.clone())
        .custom_tag(
            SingleLetterTag::lowercase(nostr::Alphabet::H),
            jp_room.clone(),
        );
    jp.subscribe("mixed", vec![both.clone()])
        .await
        .expect("req");
    let read = jp
        .collect_until_eose("mixed", Duration::from_secs(10))
        .await
        .expect("jp read of both regions");
    let mut texts: Vec<_> = read.iter().map(|event| event.content.as_str()).collect();
    texts.sort_unstable();
    assert_eq!(texts, ["hi jp", "hi jp ws", "hi kr", "jp's own room"]);
    jp.send_raw(&serde_json::json!(["COUNT", "mixed-count", both]))
        .await
        .expect("count");
    loop {
        match jp
            .recv_event(Duration::from_secs(10))
            .await
            .expect("count reply")
        {
            RelayMessage::Count {
                subscription_id,
                count,
            } if subscription_id == "mixed-count" => {
                assert_eq!(count, 4);
                break;
            }
            RelayMessage::Closed { message, .. } => panic!("mixed count refused: {message}"),
            _ => {}
        }
    }
    let both_rooms = serde_json::json!([room, jp_room]);
    let (status, events) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"kinds": [9], "#h": both_rooms}]),
    )
    .await;
    assert_eq!(status, 200, "{events}");
    assert_eq!(events.as_array().map(Vec::len), Some(4), "{events}");
    let (status, page) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"kinds": [9], "#h": both_rooms, "limit": 2}]),
    )
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page.as_array().map(Vec::len), Some(2), "{page}");
    let (status, mine) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"kinds": [9], "authors": [keys.public_key().to_hex()]}]),
    )
    .await;
    assert_eq!(status, 200, "{mine}");
    assert_eq!(
        mine.as_array().map(Vec::len),
        Some(4),
        "a read naming no room: {mine}"
    );
    let response = reqwest::Client::new()
        .post(format!("{}/count", http(&jp_url())))
        .header("X-Pubkey", keys.public_key().to_hex())
        .json(&serde_json::json!([{"kinds": [9], "#h": both_rooms}]))
        .send()
        .await
        .expect("count");
    assert_eq!(response.status().as_u16(), 200);
    let counted: Value = response.json().await.expect("count body");
    assert_eq!(counted["count"], 4, "{counted}");

    // Search spanning both regions is merged by score (S4-3d).
    let (status, found) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"kinds": [9], "#h": both_rooms, "search": "hi"}]),
    )
    .await;
    assert_eq!(status, 200, "{found}");
    assert_eq!(found.as_array().map(Vec::len), Some(3), "{found}");
    let (status, page) = query(
        &jp_url(),
        &keys,
        serde_json::json!([{"kinds": [9], "search": "hi", "limit": 2, "page": 2}]),
    )
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(
        page.as_array().map(Vec::len),
        Some(1),
        "page 2 of a room-less search: {page}"
    );
    let searched = Filter::new().kind(Kind::Custom(9)).search("room");
    jp.subscribe("search", vec![searched]).await.expect("req");
    let hits = jp
        .collect_until_eose("search", Duration::from_secs(10))
        .await
        .expect("jp search");
    let texts: Vec<_> = hits.iter().map(|event| event.content.as_str()).collect();
    assert_eq!(texts, ["jp's own room"]);
}

/// A region link call from jp to kr: signed with jp's link key, for kr's
/// community, this exact path and body. Built here independently of the
/// relay's own signer, so the wire format is pinned.
fn jp_attestation(keys: &Keys, community: &str, path: &str, body: &[u8], target: &str) -> String {
    jp_attestation_at(keys, community, path, body, target, nostr::Timestamp::now())
}

fn jp_attestation_at(
    keys: &Keys,
    community: &str,
    path: &str,
    body: &[u8],
    target: &str,
    now: nostr::Timestamp,
) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    let claims = serde_json::json!({
        "v": 1,
        "entry_region": "jp",
        "target_region": target,
        "community_id": community,
        "method": "POST",
        "path": path,
        "body_sha256": hex::encode(sha2::Sha256::digest(body)),
        "channel_id": null,
        "epoch": null,
        "principal": null,
        "nonce": uuid::Uuid::new_v4().simple().to_string(),
    });
    let event = EventBuilder::new(Kind::Custom(27250), claims.to_string())
        .tags([Tag::expiration(nostr::Timestamp::from(now.as_secs() + 30))])
        .custom_created_at(now)
        .sign_with_keys(keys)
        .unwrap();
    format!(
        "KuzzRegion {}",
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap())
    )
}

async fn kr_community() -> String {
    let url = std::env::var("REGION_KR_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kuzz:kuzz_dev@127.0.0.1:5491/kuzz".into());
    let pool = sqlx::PgPool::connect(&url).await.expect("kr database");
    let id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM communities WHERE host = 'localhost:3041'")
            .fetch_one(&pool)
            .await
            .expect("kr community");
    id.to_string()
}

async fn ping_kr(auth: &str, body: &'static [u8]) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/internal/region/ping", http(&kr_url())))
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("ping request");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn region_link_accepts_only_a_fresh_call_signed_by_a_known_region() {
    let jp_key = Keys::parse(&std::env::var("REGION_JP_LINK_KEY").expect("REGION_JP_LINK_KEY"))
        .expect("jp link key");
    let community = kr_community().await;
    let path = "/internal/region/ping";
    let body: &'static [u8] = b"{}";

    let auth = jp_attestation(&jp_key, &community, path, body, "kr");
    let (status, answer) = ping_kr(&auth, body).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["region"], "kr");
    assert_eq!(answer["from"], "jp");

    // The same attestation again is a replay.
    assert_eq!(ping_kr(&auth, body).await.0, 401);
    // Another key claiming to be jp.
    let forged = jp_attestation(&Keys::generate(), &community, path, body, "kr");
    assert_eq!(ping_kr(&forged, body).await.0, 401);
    // Signed for another body, or for another region.
    let other_body = jp_attestation(&jp_key, &community, path, b"{\"x\":1}", "kr");
    assert_eq!(ping_kr(&other_body, body).await.0, 401);
    let other_target = jp_attestation(&jp_key, &community, path, body, "us");
    assert_eq!(ping_kr(&other_target, body).await.0, 401);
    // A community kr does not serve.
    let elsewhere = uuid::Uuid::new_v4().to_string();
    let other_community = jp_attestation(&jp_key, &elsewhere, path, body, "kr");
    assert_eq!(ping_kr(&other_community, body).await.0, 401);
    // Signed a minute ago: expired.
    let old = nostr::Timestamp::from(nostr::Timestamp::now().as_secs() - 60);
    let expired = jp_attestation_at(&jp_key, &community, path, body, "kr", old);
    assert_eq!(ping_kr(&expired, body).await.0, 401);
    // No attestation at all.
    assert_eq!(ping_kr("Bearer nothing", body).await.0, 401);
}
