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
//! Since S5 jp's global tables (regions, the room directory, communities,
//! accounts, room-less events) are a copy of kr's, the primary's: tests
//! write global rows in kr and wait for jp's copy.

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

/// Wait until jp's copy of the directory knows `room` lives in kr.
async fn tell_jp_room_lives_in_kr(room: &str) {
    let home = copied_directory_home(room).await;
    if home.as_deref() != Some("kr") {
        let in_kr: Option<String> = sqlx::query_scalar(
            "SELECT home_region FROM room_directory WHERE channel_id = $1::uuid",
        )
        .bind(room)
        .fetch_optional(&region_db("kr").await)
        .await
        .expect("kr directory");
        let lag: Option<f64> = sqlx::query_scalar(
            "SELECT EXTRACT(EPOCH FROM clock_timestamp() - beat_at)::float8 FROM global_heartbeat",
        )
        .fetch_optional(&region_db("jp").await)
        .await
        .expect("jp heartbeat");
        panic!("jp's copy never learned room {room}: jp {home:?}, kr {in_kr:?}, jp lag {lag:?} s");
    }
}

/// `room`'s home in jp's copy of the directory, waiting up to 30 s (a busy test machine) for it.
async fn copied_directory_home(room: &str) -> Option<String> {
    let jp = region_db("jp").await;
    for _ in 0..300 {
        let home: Option<String> = sqlx::query_scalar(
            "SELECT home_region FROM room_directory WHERE channel_id = $1::uuid",
        )
        .bind(room)
        .fetch_optional(&jp)
        .await
        .expect("directory row");
        if home.is_some() {
            return home;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

/// Wait up to 30 s until `sql` (one boolean) is true in jp's database.
async fn jp_copy_shows(sql: &str, bind: &str) -> bool {
    let jp = region_db("jp").await;
    for _ in 0..300 {
        let shown: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .bind(bind)
            .fetch_one(&jp)
            .await
            .expect("copy query");
        if shown {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// The next live event of `sub_id`, skipping other messages.
async fn live_event(client: &mut KuzzTestClient, sub_id: &str) -> Event {
    loop {
        match client
            .recv_event(Duration::from_secs(10))
            .await
            .expect("live event")
        {
            RelayMessage::Event {
                subscription_id,
                event,
            } if subscription_id == sub_id => return *event,
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == sub_id => panic!("{sub_id} closed: {message}"),
            _ => {}
        }
    }
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
    // The REQ stays open: a message written in kr arrives live through jp,
    // with kr's order number (S4-4b).
    let live = message(&keys, &room, "live in kr");
    let (status, body) = post(&kr_url(), &keys, &live).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        live_event(&mut jp, "regions").await.id,
        live.id,
        "live event through jp"
    );
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
                assert_eq!(count, 4);
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
    assert_eq!(events.as_array().map(Vec::len), Some(4), "{events}");

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
    // (a room only the directory knows: listed in kr, copied to jp)
    let stranger = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO room_directory (community_id, channel_id, home_region) \
         SELECT id, $1::uuid, 'kr' FROM communities ON CONFLICT DO NOTHING",
    )
    .bind(&stranger)
    .execute(&region_db("kr").await)
    .await
    .expect("directory row");
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
    assert_eq!(
        texts,
        ["hi jp", "hi jp ws", "hi kr", "jp's own room", "live in kr"]
    );
    // Live events of the kr room reach the merged REQ too (S4-4b).
    let live = message(&keys, &room, "live again");
    let (status, body) = post(&kr_url(), &keys, &live).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(live_event(&mut jp, "mixed").await.id, live.id);
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
                assert_eq!(count, 6);
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
    assert_eq!(events.as_array().map(Vec::len), Some(6), "{events}");
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
        Some(6),
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
    assert_eq!(counted["count"], 6, "{counted}");

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

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_reader_in_jp_is_told_live_when_added_to_a_kr_room() {
    // A private kr room; its owner adds someone who is connected to jp.
    let owner = Keys::generate();
    let invitee = Keys::generate();
    let room = uuid::Uuid::new_v4().to_string();
    let private_room = EventBuilder::new(Kind::Custom(9007), "")
        .tags([
            Tag::parse(["h", &room]).unwrap(),
            Tag::parse(["name", &format!("regions-{room}")]).unwrap(),
            Tag::parse(["channel_type", "stream"]).unwrap(),
            Tag::parse(["visibility", "private"]).unwrap(),
        ])
        .sign_with_keys(&owner)
        .unwrap();
    let (status, body) = post(&kr_url(), &owner, &private_room).await;
    assert_eq!(status, 200, "{body}");

    // The invitee's app listens for its own room-less notifications at jp.
    let mut jp = KuzzTestClient::connect(&jp_url(), &invitee)
        .await
        .expect("jp connect");
    let mine = Filter::new()
        .kind(Kind::Custom(44100))
        .pubkey(invitee.public_key())
        .limit(0);
    jp.subscribe("mine", vec![mine]).await.expect("req");
    jp.collect_until_eose("mine", Duration::from_secs(5))
        .await
        .expect("eose");
    // Reader watches are not acknowledged; give the link a moment to open.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let add = EventBuilder::new(Kind::Custom(9000), "")
        .tags([
            Tag::parse(["h", &room]).unwrap(),
            Tag::parse(["p", &invitee.public_key().to_hex()]).unwrap(),
        ])
        .sign_with_keys(&owner)
        .unwrap();
    let (status, body) = post(&kr_url(), &owner, &add).await;
    assert_eq!(status, 200, "{body}");

    let notice = live_event(&mut jp, "mine").await;
    assert_eq!(notice.kind, Kind::Custom(44100));
    assert!(
        notice
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["h".to_string(), room.clone()]),
        "{notice:?}"
    );
}

/// Whether the relay closes `client` within `within` (frames before the
/// close, such as the closing `OK`, are skipped).
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
async fn a_disconnect_queued_in_kr_closes_the_connection_in_jp() {
    // Someone banned or logged out in kr is connected to jp; a bystander too.
    let banned = Keys::generate();
    let bystander = Keys::generate();
    let mut jp = KuzzTestClient::connect(&jp_url(), &banned)
        .await
        .expect("jp connect");
    let mut other = KuzzTestClient::connect(&jp_url(), &bystander)
        .await
        .expect("jp connect");

    // What a ban in kr writes for jp (the worker in kr delivers it).
    let reason = format!("blocked: regions test {}", uuid::Uuid::new_v4());
    let payload = serde_json::json!({
        "op": "DisconnectPubkey",
        "pubkey": banned.public_key().to_bytes().to_vec(),
        "event_id": "0".repeat(64),
        "reason": reason,
    });
    let url = std::env::var("REGION_KR_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kuzz:kuzz_dev@127.0.0.1:5491/kuzz".into());
    let kr = sqlx::PgPool::connect(&url).await.expect("kr database");
    sqlx::query(
        "INSERT INTO region_outbox (target_region, community_id, topic, payload) \
         VALUES ('jp', $1::uuid, 'conn_control', $2)",
    )
    .bind(kr_community().await)
    .bind(&payload)
    .execute(&kr)
    .await
    .expect("outbox row");

    assert!(
        closed_within(&mut jp, Duration::from_secs(20)).await,
        "the banned connection in jp was not closed"
    );
    // kr deletes the row once jp has answered, just after the close.
    let mut left: i64 = 1;
    for _ in 0..50 {
        left =
            sqlx::query_scalar("SELECT count(*) FROM region_outbox WHERE payload->>'reason' = $1")
                .bind(&reason)
                .fetch_one(&kr)
                .await
                .expect("outbox rows");
        if left == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(left, 0, "a delivered row is deleted");

    // The bystander is still served.
    other
        .subscribe("still", vec![Filter::new().kind(Kind::Custom(1)).limit(1)])
        .await
        .expect("req");
    other
        .collect_until_eose("still", Duration::from_secs(5))
        .await
        .expect("bystander still connected");
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_member_removed_in_kr_loses_the_live_room_in_jp() {
    // A private kr room with a member who reads it live from jp.
    let owner = Keys::generate();
    let member = Keys::generate();
    let room = uuid::Uuid::new_v4().to_string();
    let private_room = EventBuilder::new(Kind::Custom(9007), "")
        .tags([
            Tag::parse(["h", &room]).unwrap(),
            Tag::parse(["name", &format!("regions-{room}")]).unwrap(),
            Tag::parse(["channel_type", "stream"]).unwrap(),
            Tag::parse(["visibility", "private"]).unwrap(),
        ])
        .sign_with_keys(&owner)
        .unwrap();
    let (status, body) = post(&kr_url(), &owner, &private_room).await;
    assert_eq!(status, 200, "{body}");
    let member_tag = |kind: u16| {
        EventBuilder::new(Kind::Custom(kind), "")
            .tags([
                Tag::parse(["h", &room]).unwrap(),
                Tag::parse(["p", &member.public_key().to_hex()]).unwrap(),
            ])
            .sign_with_keys(&owner)
            .unwrap()
    };
    let (status, body) = post(&kr_url(), &owner, &member_tag(9000)).await;
    assert_eq!(status, 200, "{body}");
    tell_jp_room_lives_in_kr(&room).await;

    let mut jp = KuzzTestClient::connect(&jp_url(), &member)
        .await
        .expect("jp connect");
    let live = Filter::new()
        .kind(Kind::Custom(9))
        .custom_tag(SingleLetterTag::lowercase(nostr::Alphabet::H), room.clone());
    jp.subscribe("room", vec![live]).await.expect("req");
    loop {
        match jp.recv_event(Duration::from_secs(10)).await.expect("eose") {
            RelayMessage::Eose { subscription_id } if subscription_id == "room" => break,
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == "room" => panic!("room read refused: {message}"),
            _ => {}
        }
    }
    // The watch is acknowledged before history is read, but give the
    // stream a moment like the other live tests.
    tokio::time::sleep(Duration::from_secs(1)).await;
    // The member reads it live.
    let (status, body) = post(&kr_url(), &owner, &message(&owner, &room, "hello")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(live_event(&mut jp, "room").await.content, "hello");

    let (status, body) = post(&kr_url(), &owner, &member_tag(9001)).await;
    assert_eq!(status, 200, "{body}");
    let closed = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match jp.recv_event(Duration::from_secs(15)).await {
                Ok(RelayMessage::Closed {
                    subscription_id,
                    message,
                }) if subscription_id == "room" => return message,
                Ok(_) => {}
                Err(error) => panic!("no CLOSED: {error}"),
            }
        }
    })
    .await
    .expect("the removed member's subscription in jp was not closed");
    assert!(closed.starts_with("restricted:"), "{closed}");
}

/// A region link call from jp to kr: signed with jp's link key, for kr's
/// community, this exact path and body. Built here independently of the
/// relay's own signer, so the wire format is pinned.
fn jp_attestation(keys: &Keys, community: &str, path: &str, body: &[u8], target: &str) -> String {
    jp_attestation_at(
        keys,
        community,
        "POST",
        path,
        body,
        target,
        nostr::Timestamp::now(),
    )
}

fn jp_attestation_at(
    keys: &Keys,
    community: &str,
    method: &str,
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
        "method": method,
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

/// POST a propagation from jp to kr, signed with jp's link key.
async fn propagate_to_kr(body: &Value) -> (u16, Value) {
    let jp_key = Keys::parse(&std::env::var("REGION_JP_LINK_KEY").expect("REGION_JP_LINK_KEY"))
        .expect("jp link key");
    let community = kr_community().await;
    let path = "/internal/region/propagate";
    let bytes = serde_json::to_vec(body).expect("body");
    let auth = jp_attestation(&jp_key, &community, path, &bytes, "kr");
    let response = reqwest::Client::new()
        .post(format!("{}{path}", http(&kr_url())))
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(bytes)
        .send()
        .await
        .expect("propagate request");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_block_from_jp_is_saved_in_kr_and_lifted_again() {
    let blocker = Keys::generate().public_key().to_hex();
    let blocked = Keys::generate().public_key().to_hex();
    let url = std::env::var("REGION_KR_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kuzz:kuzz_dev@127.0.0.1:5491/kuzz".into());
    let kr = sqlx::PgPool::connect(&url).await.expect("kr database");
    let state = |kr: sqlx::PgPool, blocker: String, blocked: String| async move {
        let blocks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM user_blocks WHERE blocker = decode($1, 'hex') AND blocked = decode($2, 'hex')",
        )
        .bind(&blocker)
        .bind(&blocked)
        .fetch_one(&kr)
        .await
        .expect("blocks");
        let open: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM user_block_periods WHERE blocker = decode($1, 'hex') \
               AND blocked = decode($2, 'hex') AND ended_at IS NULL",
        )
        .bind(&blocker)
        .bind(&blocked)
        .fetch_one(&kr)
        .await
        .expect("periods");
        (blocks, open)
    };

    let block = serde_json::json!({
        "topic": "block",
        "payload": {"blocker": blocker, "blocked": blocked, "on": true, "version": 10}
    });
    let (status, answer) = propagate_to_kr(&block).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["applied"], true, "{answer}");
    assert_eq!(
        state(kr.clone(), blocker.clone(), blocked.clone()).await,
        (1, 1)
    );
    // Again (a retry): still one block, one open period.
    assert_eq!(propagate_to_kr(&block).await.0, 200);
    assert_eq!(
        state(kr.clone(), blocker.clone(), blocked.clone()).await,
        (1, 1)
    );

    let unblock = serde_json::json!({
        "topic": "block",
        "payload": {"blocker": blocker, "blocked": blocked, "on": false, "version": 20}
    });
    let (status, answer) = propagate_to_kr(&unblock).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        state(kr.clone(), blocker.clone(), blocked.clone()).await,
        (0, 0)
    );

    // The block arriving again after the newer unblock (out of order) is
    // ignored, so kr stays unblocked like the region that sent both.
    let (status, answer) = propagate_to_kr(&block).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["applied"], true, "{answer}");
    assert_eq!(
        state(kr.clone(), blocker.clone(), blocked.clone()).await,
        (0, 0)
    );
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

/// `room`'s home in `region`'s directory.
async fn directory_home(region: &str, room: &str) -> Option<String> {
    sqlx::query_scalar("SELECT home_region FROM room_directory WHERE channel_id = $1::uuid")
        .bind(room)
        .fetch_optional(&region_db(region).await)
        .await
        .expect("directory row")
}

/// How many rooms `region`'s database has with this id.
async fn room_rows(region: &str, room: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM channels WHERE id = $1::uuid")
        .bind(room)
        .fetch_one(&region_db(region).await)
        .await
        .expect("channel rows")
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_room_is_registered_at_the_primary_and_found_from_the_other_region() {
    let keys = Keys::generate();

    // Created in jp: reserved in kr (the primary) for jp first.
    let jp_room = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(&jp_url(), &keys, &create_room(&keys, &jp_room)).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    assert_eq!(directory_home("kr", &jp_room).await.as_deref(), Some("jp"));
    assert_eq!(room_rows("jp", &jp_room).await, 1);
    assert_eq!(room_rows("kr", &jp_room).await, 0);

    // The same id created again in kr goes to jp, its holder: no second room.
    let (status, body) = post(&kr_url(), &keys, &create_room(&keys, &jp_room)).await;
    assert!(status < 500, "{status} {body}");
    assert_eq!(room_rows("kr", &jp_room).await, 0, "kr made a second room");

    // Created in kr, never told to jp: jp asks kr where it lives and
    // forwards the write there.
    let kr_room = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(&kr_url(), &keys, &create_room(&keys, &kr_room)).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    let (status, body) = post(&jp_url(), &keys, &message(&keys, &kr_room, "found")).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    assert!(body["channel_sequence"].as_i64().is_some(), "{body}");
    assert_eq!(copied_directory_home(&kr_room).await.as_deref(), Some("kr"));
    assert_eq!(room_rows("jp", &kr_room).await, 0);
}

fn open_dm(keys: &Keys, other: &Keys) -> Event {
    EventBuilder::new(Kind::Custom(41010), "")
        .tags([Tag::parse(["p", &other.public_key().to_hex()]).unwrap()])
        .sign_with_keys(keys)
        .unwrap()
}

/// DM rooms either of `a` / `b` created in `region`.
async fn dm_rows(region: &str, a: &Keys, b: &Keys) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM channels WHERE channel_type = 'dm' \
           AND created_by IN (decode($1, 'hex'), decode($2, 'hex'))",
    )
    .bind(a.public_key().to_hex())
    .bind(b.public_key().to_hex())
    .fetch_one(&region_db(region).await)
    .await
    .expect("dm rows")
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn two_regions_opening_the_same_dm_at_once_get_one_room() {
    let (a, b) = (Keys::generate(), Keys::generate());
    let (jp_open, kr_open) = (open_dm(&a, &b), open_dm(&b, &a));
    let (jp, kr) = (jp_url(), kr_url());
    let ((jp_status, jp_body), (kr_status, kr_body)) =
        tokio::join!(post(&jp, &a, &jp_open), post(&kr, &b, &kr_open),);
    assert_eq!(
        (jp_status, &jp_body["accepted"]),
        (200, &Value::Bool(true)),
        "{jp_body}"
    );
    assert_eq!(
        (kr_status, &kr_body["accepted"]),
        (200, &Value::Bool(true)),
        "{kr_body}"
    );
    assert_eq!(
        dm_rows("jp", &a, &b).await + dm_rows("kr", &a, &b).await,
        1,
        "one DM across both regions"
    );

    // Opening it again from either side keeps the one room.
    let again = EventBuilder::new(Kind::Custom(41010), "again")
        .tags([Tag::parse(["p", &b.public_key().to_hex()]).unwrap()])
        .sign_with_keys(&a)
        .unwrap();
    let (status, body) = post(&kr_url(), &a, &again).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(dm_rows("jp", &a, &b).await + dm_rows("kr", &a, &b).await, 1);
}

/// POST a shared-state operation from jp to kr, signed with jp's link key
/// and naming `community` (the nil one for shared state).
async fn shared_state_at_kr(body: &Value, community: &str) -> (u16, Value) {
    let jp_key = Keys::parse(&std::env::var("REGION_JP_LINK_KEY").expect("REGION_JP_LINK_KEY"))
        .expect("jp link key");
    let path = "/internal/region/shared-state";
    let bytes = serde_json::to_vec(body).expect("body");
    let auth = jp_attestation(&jp_key, community, path, &bytes, "kr");
    let response = reqwest::Client::new()
        .post(format!("{}{path}", http(&kr_url())))
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .body(bytes)
        .send()
        .await
        .expect("shared-state request");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

const NIL: &str = "00000000-0000-0000-0000-000000000000";

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn the_primary_keeps_sign_in_state_for_linked_regions() {
    let key = format!("kuzz:auth:e2e:{}", uuid::Uuid::new_v4());
    let put_new = serde_json::json!({ "op": "put_new", "key": key, "value": "v", "ttl": 60 });
    assert_eq!(
        shared_state_at_kr(&put_new, NIL).await,
        (200, serde_json::json!({ "set": true }))
    );
    assert_eq!(
        shared_state_at_kr(&put_new, NIL).await,
        (200, serde_json::json!({ "set": false }))
    );
    let take = serde_json::json!({ "op": "take", "key": key });
    assert_eq!(
        shared_state_at_kr(&take, NIL).await,
        (200, serde_json::json!({ "value": "v" }))
    );
    assert_eq!(
        shared_state_at_kr(&take, NIL).await,
        (200, serde_json::json!({ "value": null }))
    );

    // One account's sign-in limit counts in kr for every region.
    let count = serde_json::json!({
        "op": "count",
        "key": format!("auth:key-backup:{}", uuid::Uuid::new_v4()),
        "window": 60,
        "limit": 1,
    });
    let (status, first) = shared_state_at_kr(&count, NIL).await;
    assert_eq!((status, &first["count"]), (200, &Value::from(1)), "{first}");
    let (status, second) = shared_state_at_kr(&count, NIL).await;
    assert_eq!(
        (status, &second["count"]),
        (200, &Value::from(2)),
        "{second}"
    );

    // Nothing else in kr's Redis is reachable, nor its per-IP limits.
    let other = serde_json::json!({ "op": "get", "key": "kuzz:ratelimit:ip:127.0.0.1:conn" });
    assert_eq!(shared_state_at_kr(&other, NIL).await.0, 400);
    let per_ip = serde_json::json!({
        "op": "count", "key": "auth:login:start:global", "window": 60, "limit": 1,
    });
    assert_eq!(shared_state_at_kr(&per_ip, NIL).await.0, 400);
    // Shared state belongs to no community.
    let community = kr_community().await;
    assert_eq!(shared_state_at_kr(&take, &community).await.0, 401);
}

fn nip98_post(keys: &Keys, url: &str, body: &[u8]) -> (Event, String) {
    use base64::Engine as _;
    use sha2::Digest as _;
    let event = EventBuilder::new(Kind::Custom(27235), "")
        .tags([
            Tag::parse(["u", url]).unwrap(),
            Tag::parse(["method", "POST"]).unwrap(),
            Tag::parse(["payload", &hex::encode(sha2::Sha256::digest(body))]).unwrap(),
        ])
        .sign_with_keys(keys)
        .unwrap();
    let header = format!(
        "Nostr {}",
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&event).unwrap())
    );
    (event, header)
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_signed_query_in_jp_is_marked_in_jp_only() {
    let keys = Keys::generate();
    let url = format!("{}/query", http(&jp_url()));
    let body = br#"[{"kinds":[9],"limit":1}]"#;
    let (event, header) = nip98_post(&keys, &url, body);
    let send = || async {
        reqwest::Client::new()
            .post(&url)
            .header("Authorization", &header)
            .header("Content-Type", "application/json")
            .body(body.to_vec())
            .send()
            .await
            .expect("query request")
    };
    let first = send().await;
    assert_eq!(first.status().as_u16(), 200, "{:?}", first.text().await);
    let again = send().await;
    assert_eq!(again.status().as_u16(), 401);

    // Queries run on every screen: their mark stays in jp's seen-set, so
    // kr has none (and sets it now).
    let community = kr_community().await;
    let mark = serde_json::json!({
        "op": "put_new",
        "key": format!("kuzz:{community}:nip98:{}", event.id.to_hex()),
        "value": "1",
        "ttl": 120,
    });
    assert_eq!(
        shared_state_at_kr(&mark, NIL).await,
        (200, serde_json::json!({ "set": true }))
    );
}

/// A kr room with one message, created in kr and known to jp.
async fn kr_room(keys: &Keys) -> String {
    let room = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(&kr_url(), keys, &create_room(keys, &room)).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    tell_jp_room_lives_in_kr(&room).await;
    room
}

/// The order numbers kr gave the messages of `room`, in order.
async fn kr_sequences(room: &str) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT channel_sequence FROM events WHERE channel_id = $1::uuid \
           AND kind = 9 AND channel_sequence > 0 ORDER BY channel_sequence",
    )
    .bind(room)
    .fetch_all(&region_db("kr").await)
    .await
    .expect("sequences")
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn writes_from_two_regions_at_once_get_one_gapless_order() {
    let keys = Keys::generate();
    let room = kr_room(&keys).await;

    // Twenty writes at once, half through jp (forwarded), half straight to kr.
    let (jp, kr) = (jp_url(), kr_url());
    let events: Vec<(String, Event)> = (0..20)
        .map(|i| {
            let url = if i % 2 == 0 { jp.clone() } else { kr.clone() };
            (url, message(&keys, &room, &format!("at once {i}")))
        })
        .collect();
    let answers =
        futures_util::future::join_all(events.iter().map(|(url, event)| post(url, &keys, event)))
            .await;
    let mut numbers: Vec<i64> = answers
        .iter()
        .map(|(status, body)| {
            assert_eq!(
                (*status, &body["accepted"]),
                (200, &Value::Bool(true)),
                "{body}"
            );
            body["channel_sequence"].as_i64().expect("order number")
        })
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 20, "each write has its own number");

    // kr holds all of them, numbered without a gap; jp holds none.
    let stored = kr_sequences(&room).await;
    assert_eq!(stored.len(), 20);
    assert!(
        stored.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "gap in {stored:?}"
    );
    let jp_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE channel_id = $1::uuid")
            .bind(&room)
            .fetch_one(&region_db("jp").await)
            .await
            .expect("jp rows");
    assert_eq!(jp_rows, 0);
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_resend_after_a_lost_reply_is_stored_once() {
    let keys = Keys::generate();
    let room = kr_room(&keys).await;

    // The app sent through jp and lost the reply; it sends the same event
    // again, through jp and then straight to kr.
    let event = message(&keys, &room, "sent twice");
    let (status, first) = post(&jp_url(), &keys, &event).await;
    assert_eq!(
        (status, &first["accepted"]),
        (200, &Value::Bool(true)),
        "{first}"
    );
    for url in [jp_url(), kr_url()] {
        let (status, again) = post(&url, &keys, &event).await;
        assert_eq!(status, 200, "{again}");
        assert_eq!(again["accepted"], Value::Bool(true), "{again}");
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE id = decode($1, 'hex')")
        .bind(event.id.to_hex())
        .fetch_one(&region_db("kr").await)
        .await
        .expect("rows");
    assert_eq!(rows, 1, "stored once");
    assert_eq!(kr_sequences(&room).await.len(), 1, "numbered once");
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_moved_epoch_is_followed_once_and_kept() {
    let keys = Keys::generate();
    let room = kr_room(&keys).await;

    // kr's directory says the room is at epoch 2 now (as after a move back);
    // jp's copy may still hold epoch 0. Either way jp's forward lands in kr
    // once (on a stale epoch: "wrong region" with the new one, followed once).
    let kr = region_db("kr").await;
    sqlx::query(
        "INSERT INTO room_directory (community_id, channel_id, home_region, epoch) \
         SELECT id, $1::uuid, 'kr', 2 FROM communities WHERE host = 'localhost:3041' \
         ON CONFLICT (community_id, channel_id) DO UPDATE SET epoch = 2",
    )
    .bind(&room)
    .execute(&kr)
    .await
    .expect("epoch");
    let (status, body) = post(&jp_url(), &keys, &message(&keys, &room, "after a move")).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    assert_eq!(kr_sequences(&room).await.len(), 1);
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_link_down_fails_retryably_and_never_splits_the_room() {
    let keys = Keys::generate();
    let room = kr_room(&keys).await;

    // jp loses kr: kr's address in the region list (written in kr, copied
    // to jp) points at nothing.
    let jp = region_db("jp").await;
    let kr = region_db("kr").await;
    let address: String = sqlx::query_scalar("SELECT connect_url FROM regions WHERE id = 'kr'")
        .fetch_one(&kr)
        .await
        .expect("kr address");
    let set_address = |url: String| {
        let kr = kr.clone();
        async move {
            sqlx::query("UPDATE regions SET connect_url = $1 WHERE id = 'kr'")
                .bind(&url)
                .execute(&kr)
                .await
                .expect("kr address");
            assert!(
                jp_copy_shows(
                    "SELECT EXISTS (SELECT 1 FROM regions WHERE id = 'kr' AND connect_url = $1)",
                    &url,
                )
                .await,
                "jp's copy never got kr's address {url}"
            );
        }
    };
    set_address("ws://127.0.0.1:9".into()).await;

    let event = message(&keys, &room, "while the link is down");
    let (status, body) = post(&jp_url(), &keys, &event).await;

    set_address(address).await;

    // Not accepted, not "not found", and nothing written in jp.
    assert_ne!(body["accepted"], Value::Bool(true), "{status} {body}");
    let message_text = body["message"].as_str().unwrap_or_default();
    assert!(
        !message_text.contains("not found") && !message_text.starts_with("blocked"),
        "{status} {body}"
    );
    let jp_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE id = decode($1, 'hex')")
            .bind(event.id.to_hex())
            .fetch_one(&jp)
            .await
            .expect("jp rows");
    assert_eq!(jp_rows, 0, "jp wrote a kr room's message itself");

    // The app's retry goes through once the link is back.
    let (status, body) = post(&jp_url(), &keys, &event).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    assert_eq!(kr_sequences(&room).await.len(), 1);
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn global_tables_are_copied_from_the_primary() {
    // The region list: jp was added in kr and knows itself from the copy.
    assert!(
        jp_copy_shows(
            "SELECT EXISTS (SELECT 1 FROM regions WHERE id = $1 AND NOT is_primary \
               AND link_pubkey IS NOT NULL)",
            "jp",
        )
        .await
    );

    // A profile (a room-less event) saved in kr reaches jp's copy, and so
    // does the profile row its side effect writes in kr.
    let keys = Keys::generate();
    let profile = EventBuilder::new(Kind::Metadata, r#"{"name":"copied"}"#)
        .sign_with_keys(&keys)
        .unwrap();
    let (status, body) = post(&kr_url(), &keys, &profile).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    assert!(
        jp_copy_shows(
            "SELECT EXISTS (SELECT 1 FROM events WHERE id = decode($1, 'hex') \
               AND channel_id IS NULL)",
            &profile.id.to_hex(),
        )
        .await,
        "the profile event was not copied"
    );
    assert!(
        jp_copy_shows(
            "SELECT EXISTS (SELECT 1 FROM users WHERE pubkey = decode($1, 'hex'))",
            &keys.public_key().to_hex(),
        )
        .await,
        "the profile row was not copied"
    );

    // Room messages stay in their home: a kr room's message is not copied.
    let room = kr_room(&keys).await;
    let (_, body) = post(&kr_url(), &keys, &message(&keys, &room, "stays in kr")).await;
    assert_eq!(body["accepted"], Value::Bool(true), "{body}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let jp_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM events WHERE channel_id = $1::uuid")
            .bind(&room)
            .fetch_one(&region_db("jp").await)
            .await
            .expect("jp rows");
    assert_eq!(jp_rows, 0);
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_copy_refuses_local_writes_and_keeps_copying() {
    // A profile from kr, so jp's copy holds a user and a room-less event.
    let keys = Keys::generate();
    let profile = EventBuilder::new(Kind::Metadata, r#"{"name":"guarded"}"#)
        .sign_with_keys(&keys)
        .unwrap();
    let (_, body) = post(&kr_url(), &keys, &profile).await;
    assert_eq!(body["accepted"], Value::Bool(true), "{body}");
    assert!(
        jp_copy_shows(
            "SELECT EXISTS (SELECT 1 FROM users WHERE pubkey = decode($1, 'hex'))",
            &keys.public_key().to_hex(),
        )
        .await
    );

    let jp = region_db("jp").await;
    let pubkey = keys.public_key().to_hex();
    let id = profile.id.to_hex();
    for sql in [
        "INSERT INTO regions (id, connect_url) VALUES ('zz', 'ws://zz.test')".to_owned(),
        "UPDATE regions SET connect_url = connect_url WHERE id = 'kr'".to_owned(),
        "UPDATE communities SET host = host".to_owned(),
        format!("DELETE FROM users WHERE pubkey = decode('{pubkey}', 'hex')"),
        format!("UPDATE events SET deleted_at = now() WHERE id = decode('{id}', 'hex')"),
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(sql.clone()))
            .execute(&jp)
            .await
            .expect_err(&sql)
            .to_string();
        assert!(
            error.contains("written in the primary region"),
            "{sql}: {error}"
        );
    }

    // Replication is not disturbed: a new room in kr still reaches jp.
    let room = kr_room(&keys).await;
    assert_eq!(copied_directory_home(&room).await.as_deref(), Some("kr"));
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn the_copy_lag_is_measured_in_jp() {
    let metrics = reqwest::get("http://localhost:3062/metrics")
        .await
        .expect("jp metrics")
        .text()
        .await
        .expect("metrics body");
    let lag: f64 = metrics
        .lines()
        .find(|line| line.starts_with("kuzz_global_copy_lag_seconds"))
        .and_then(|line| line.split_whitespace().last())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("no lag gauge in jp's metrics"));
    assert!(lag < 5.0, "jp's copy is {lag} s behind");
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
    let expired = jp_attestation_at(&jp_key, &community, "POST", path, body, "kr", old);
    assert_eq!(ping_kr(&expired, body).await.0, 401);
    // No attestation at all.
    assert_eq!(ping_kr("Bearer nothing", body).await.0, 401);
}

/// The next text frame of a region stream, as JSON, skipping pings.
async fn next_stream_frame<S>(stream: &mut S) -> Option<Value>
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    use futures_util::StreamExt as _;
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .ok()??
            .ok()?;
        if let tokio_tungstenite::tungstenite::Message::Text(text) = message {
            return serde_json::from_str(text.as_str()).ok();
        }
    }
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn home_streams_a_watched_room_to_its_allowed_readers() {
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let jp_key = Keys::parse(&std::env::var("REGION_JP_LINK_KEY").expect("REGION_JP_LINK_KEY"))
        .expect("jp link key");
    let community = kr_community().await;
    let path = "/internal/region/stream";

    // A private kr room with one member; a stranger is watched for too.
    let member = Keys::generate();
    let stranger = Keys::generate();
    let room = uuid::Uuid::new_v4().to_string();
    let private_room = EventBuilder::new(Kind::Custom(9007), "")
        .tags([
            Tag::parse(["h", &room]).unwrap(),
            Tag::parse(["name", &format!("regions-{room}")]).unwrap(),
            Tag::parse(["channel_type", "stream"]).unwrap(),
            Tag::parse(["visibility", "private"]).unwrap(),
        ])
        .sign_with_keys(&member)
        .unwrap();
    let (status, body) = post(&kr_url(), &member, &private_room).await;
    assert_eq!(status, 200, "{body}");

    // Without a valid attestation the upgrade is refused.
    let mut request = format!("{}{path}", kr_url()).into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Authorization", "KuzzRegion e30=".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(request).await.is_err());

    let auth = jp_attestation_at(
        &jp_key,
        &community,
        "GET",
        path,
        b"",
        "kr",
        nostr::Timestamp::now(),
    );
    let mut request = format!("{}{path}", kr_url()).into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Authorization", auth.parse().unwrap());
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("stream upgrade");
    let (mut sink, mut stream) = socket.split();
    let watch = serde_json::json!({
        "op": "watch",
        "room": room,
        "readers": [member.public_key().to_hex(), stranger.public_key().to_hex()],
    });
    sink.send(tokio_tungstenite::tungstenite::Message::Text(
        watch.to_string().into(),
    ))
    .await
    .expect("watch");
    // kr answers once the watch is live; no sleep needed.
    let frame = next_stream_frame(&mut stream).await.expect("watching");
    assert_eq!(frame["op"], "watching", "{frame}");

    let sent = message(&member, &room, "live from kr");
    let (status, body) = post(&kr_url(), &member, &sent).await;
    assert_eq!(status, 200, "{body}");
    let seq = body["channel_sequence"].as_i64();

    let frame = next_stream_frame(&mut stream).await.expect("event frame");
    assert_eq!(frame["op"], "event", "{frame}");
    assert_eq!(frame["room"], room.as_str());
    assert_eq!(frame["event"]["id"], sent.id.to_hex());
    // Only the member may see a private room's message.
    assert_eq!(
        frame["to"],
        serde_json::json!([member.public_key().to_hex()])
    );
    assert!(seq.is_some(), "{body}");
    assert_eq!(frame["seq"].as_i64(), seq, "{frame}");

    // Watched only for the stranger, the room's messages are not streamed.
    let only_stranger = serde_json::json!({
        "op": "watch",
        "room": room,
        "readers": [stranger.public_key().to_hex()],
    });
    sink.send(tokio_tungstenite::tungstenite::Message::Text(
        only_stranger.to_string().into(),
    ))
    .await
    .expect("rewatch");
    let frame = next_stream_frame(&mut stream).await.expect("watching");
    assert_eq!(frame["op"], "watching", "{frame}");
    let (status, _) = post(&kr_url(), &member, &message(&member, &room, "private")).await;
    assert_eq!(status, 200);

    // After unwatching, nothing more arrives for the room.
    let unwatch = serde_json::json!({"op": "unwatch", "room": room});
    sink.send(tokio_tungstenite::tungstenite::Message::Text(
        unwatch.to_string().into(),
    ))
    .await
    .expect("unwatch");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (status, _) = post(&kr_url(), &member, &message(&member, &room, "after")).await;
    assert_eq!(status, 200);
    assert!(next_stream_frame(&mut stream).await.is_none());

    // A malformed reader is refused, per room.
    let bad = serde_json::json!({"op": "watch", "room": room, "readers": ["nope"]});
    sink.send(tokio_tungstenite::tungstenite::Message::Text(
        bad.to_string().into(),
    ))
    .await
    .expect("bad watch");
    let frame = next_stream_frame(&mut stream).await.expect("refusal");
    assert_eq!(frame["op"], "refused", "{frame}");
    // So is a room kr does not have.
    let unknown = serde_json::json!({
        "op": "watch",
        "room": uuid::Uuid::new_v4(),
        "readers": [member.public_key().to_hex()],
    });
    sink.send(tokio_tungstenite::tungstenite::Message::Text(
        unknown.to_string().into(),
    ))
    .await
    .expect("unknown watch");
    let frame = next_stream_frame(&mut stream).await.expect("refusal");
    assert_eq!(frame["op"], "refused", "{frame}");
    assert_eq!(frame["reason"], "invalid: unknown room");
}

/// PKCE S256 challenge of `verifier`.
fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier))
}

fn query_param(location: &str, name: &str) -> Option<String> {
    url::Url::parse(location)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_sign_up_in_jp_is_made_in_kr_and_lives_in_jp() {
    // jp holds a copy of the accounts and may not write them: the sign-in
    // steps sent to jp go to kr and the answers come back through jp.
    const REDIRECT: &str = "http://127.0.0.1:9999/cb";
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let jp = http(&jp_url());
    let verifier = hex::encode(rand::random::<[u8; 32]>());
    let state = verifier[..24].to_owned();
    let mut start_url = url::Url::parse(&format!("{jp}/auth/oidc/google/start")).unwrap();
    start_url.query_pairs_mut().extend_pairs([
        ("state", state.as_str()),
        ("code_challenge", pkce_challenge(&verifier).as_str()),
        ("client", "cli"),
        ("identity_mode", "token"),
        ("redirect_uri", REDIRECT),
        ("device_name", "e2e"),
    ]);
    let start = client.get(start_url).send().await.expect("start");
    assert_eq!(start.status(), 302, "{:?}", start.text().await);

    // The provider sends the browser back to the service's callback, which
    // here is kr's address: the account's home still comes from where the
    // sign-in began. The fake provider trusts any `fake:` code, so each run
    // makes a new account.
    let provider = start.headers()["location"].to_str().unwrap().to_owned();
    let mut callback_url = url::Url::parse(&provider).expect("provider redirect");
    let subject = format!("fake:{}@example.test", uuid::Uuid::new_v4().simple());
    callback_url
        .query_pairs_mut()
        .clear()
        .extend_pairs([("code", subject.as_str()), ("state", state.as_str())]);
    let callback = client.get(callback_url).send().await.expect("callback");
    assert_eq!(callback.status(), 302);
    let location = callback.headers()["location"].to_str().unwrap().to_owned();
    let code = query_param(&location, "code").unwrap_or_else(|| panic!("{location}"));

    let complete = client
        .post(format!("{jp}/auth/oidc/complete"))
        .json(&serde_json::json!({"login_code": code, "code_verifier": verifier}))
        .send()
        .await
        .expect("complete");
    assert_eq!(complete.status(), 200);
    let login: Value = complete.json().await.expect("tokens");
    let principal = login["principal_id"]
        .as_str()
        .expect("principal")
        .to_owned();
    let access = login["access"].as_str().expect("access").to_owned();

    // Made in kr, home jp, and copied back to jp.
    let home: Option<String> =
        sqlx::query_scalar("SELECT home_region FROM principals WHERE id = decode($1, 'hex')")
            .bind(&principal)
            .fetch_optional(&region_db("kr").await)
            .await
            .expect("kr principal");
    assert_eq!(home.as_deref(), Some("jp"));
    assert!(
        jp_copy_shows(
            "SELECT EXISTS (SELECT 1 FROM principals WHERE id = decode($1, 'hex') \
               AND home_region = 'jp')",
            &principal,
        )
        .await
    );

    // A signed-in request through jp is answered by kr.
    let me = client
        .get(format!("{jp}/auth/me"))
        .bearer_auth(&access)
        .send()
        .await
        .expect("me");
    assert_eq!(me.status(), 200, "{:?}", me.text().await);
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_profile_saved_in_jp_is_stored_in_kr_and_copied_back() {
    // A signed HTTP write, so jp can vouch for the author at kr.
    let keys = Keys::generate();
    let profile = EventBuilder::new(Kind::Metadata, r#"{"name":"from jp"}"#)
        .sign_with_keys(&keys)
        .unwrap();
    let url = format!("{}/events", http(&jp_url()));
    let body = serde_json::to_vec(&profile).unwrap();
    let (_, header) = nip98_post(&keys, &url, &body);
    let response = reqwest::Client::new()
        .post(&url)
        .header("Authorization", header)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("profile write");
    let status = response.status().as_u16();
    let answer: Value = response.json().await.unwrap_or(Value::Null);
    assert_eq!(
        (status, &answer["accepted"]),
        (200, &Value::Bool(true)),
        "{answer}"
    );

    // Stored and projected in kr, then copied to jp.
    let kr = region_db("kr").await;
    let stored: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE id = decode($1, 'hex') \
           AND channel_id IS NULL)",
    )
    .bind(profile.id.to_hex())
    .fetch_one(&kr)
    .await
    .expect("kr event");
    assert!(stored, "the profile was not stored in kr");
    assert!(
        jp_copy_shows(
            "SELECT EXISTS (SELECT 1 FROM users WHERE pubkey = decode($1, 'hex') \
               AND display_name = 'from jp')",
            &keys.public_key().to_hex(),
        )
        .await,
        "the profile row was not copied back to jp"
    );
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_dm_opened_in_jp_keeps_its_command_in_the_room_and_its_notice_in_kr() {
    let (a, b) = (Keys::generate(), Keys::generate());
    let open = open_dm(&a, &b);
    let (status, body) = post(&jp_url(), &a, &open).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );

    // The command's own row is kept in jp, in the DM's room: a room-less
    // row there is global data, written only in kr.
    let in_room: bool = sqlx::query_scalar(
        "SELECT channel_id IS NOT NULL FROM events WHERE id = decode($1, 'hex')",
    )
    .bind(open.id.to_hex())
    .fetch_one(&region_db("jp").await)
    .await
    .expect("command row in jp");
    assert!(in_room, "the DM open was stored room-less in jp");

    // The relay's "added" notice to the other person is stored in kr and
    // copied back to jp.
    let notice = "SELECT EXISTS (SELECT 1 FROM events WHERE kind = 44100 \
                    AND channel_id IS NULL \
                    AND tags @> jsonb_build_array(jsonb_build_array('p', $1::text)))";
    let mut in_kr = false;
    for _ in 0..100 {
        in_kr = sqlx::query_scalar(notice)
            .bind(b.public_key().to_hex())
            .fetch_one(&region_db("kr").await)
            .await
            .expect("kr notice");
        if in_kr {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(in_kr, "the membership notice was not stored in kr");
    assert!(
        jp_copy_shows(notice, &b.public_key().to_hex()).await,
        "the membership notice was not copied back to jp"
    );
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_profile_saved_through_jp_reaches_jp_watchers_live() {
    // Someone in jp watches a person's profile.
    let (person, watcher) = (Keys::generate(), Keys::generate());
    let mut jp = KuzzTestClient::connect(&jp_url(), &watcher)
        .await
        .expect("jp connect");
    let profiles = Filter::new()
        .kind(Kind::Metadata)
        .author(person.public_key())
        .limit(0);
    jp.subscribe("profiles", vec![profiles]).await.expect("req");
    jp.collect_until_eose("profiles", Duration::from_secs(5))
        .await
        .expect("eose");

    // The person saves it through jp: stored in kr, copied back, and told to
    // the watcher in jp as the copy brings it.
    let profile = EventBuilder::new(Kind::Metadata, r#"{"name":"live from kr"}"#)
        .sign_with_keys(&person)
        .unwrap();
    let url = format!("{}/events", http(&jp_url()));
    let body = serde_json::to_vec(&profile).unwrap();
    let (_, header) = nip98_post(&person, &url, &body);
    let response = reqwest::Client::new()
        .post(&url)
        .header("Authorization", header)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("profile write");
    assert_eq!(response.status().as_u16(), 200);

    let seen = live_event(&mut jp, "profiles").await;
    assert_eq!(seen.id, profile.id);
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn jp_answers_media_checks_only_for_a_linked_region() {
    // The media check (S6-2) is behind the region link like every internal
    // route: only a linked region learns which files jp's rooms use.
    let response = reqwest::Client::new()
        .post(format!("{}/internal/region/media-in-use", http(&jp_url())))
        .json(&serde_json::json!({ "hashes": [], "grace_cutoff": 0 }))
        .send()
        .await
        .expect("media check");
    assert_eq!(response.status().as_u16(), 401);
}
