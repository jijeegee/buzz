//! End-to-end tests for the user stream REQ (`docs/message-sync.md#user-stream`).
//!
//! They need a running relay started with the stream on and one account in
//! `KUZZ_USER_STREAM_LIVE`:
//!
//! ```text
//! KUZZ_USER_STREAM=on
//! KUZZ_USER_STREAM_LIVE=allowlist:<pubkey of KUZZ_TEST_STREAM_LIVE_KEY>
//! KUZZ_STREAM_EPOCH=e2e
//! ```
//!
//! then:
//!
//! ```text
//! KUZZ_TEST_STREAM_LIVE_KEY=<hex secret> RELAY_URL=ws://127.0.0.1:3000 \
//!   cargo test -p kuzz-test-client --test e2e_user_stream -- --ignored
//! ```

use std::time::Duration;

use kuzz_test_client::{KuzzTestClient, RelayMessage};
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};

const FROZEN_KINDS: [u16; 17] = [
    5, 7, 9, 9005, 39005, 40001, 40002, 40003, 40008, 40009, 40099, 45001, 45003, 48100, 48101,
    48102, 48103,
];

fn relay_url() -> String {
    std::env::var("RELAY_URL").unwrap_or_else(|_| "ws://localhost:3000".to_string())
}

fn relay_http_url() -> String {
    relay_url()
        .replace("wss://", "https://")
        .replace("ws://", "http://")
        .trim_end_matches('/')
        .to_string()
}

fn live_keys() -> Keys {
    let secret = std::env::var("KUZZ_TEST_STREAM_LIVE_KEY")
        .expect("KUZZ_TEST_STREAM_LIVE_KEY (an account in KUZZ_USER_STREAM_LIVE)");
    Keys::parse(&secret).expect("KUZZ_TEST_STREAM_LIVE_KEY")
}

fn stream_req(sub: &str) -> Value {
    json!(["REQ", sub, {"#h": ["$joined"], "kinds": FROZEN_KINDS, "limit": 0}])
}

/// Create an open channel owned by `keys` (signed kind 9007 via POST /events).
async fn create_channel(keys: &Keys) -> String {
    let channel = uuid::Uuid::new_v4().to_string();
    let event = EventBuilder::new(Kind::Custom(9007), "")
        .tags(vec![
            Tag::parse(["h", &channel]).unwrap(),
            Tag::parse(["name", &format!("stream-e2e-{channel}")]).unwrap(),
            Tag::parse(["channel_type", "stream"]).unwrap(),
            Tag::parse(["visibility", "open"]).unwrap(),
        ])
        .sign_with_keys(keys)
        .unwrap();
    let response = reqwest::Client::new()
        .post(format!("{}/events", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&event).unwrap())
        .send()
        .await
        .expect("submit create-channel event");
    assert!(response.status().is_success(), "{}", response.status());
    channel
}

/// Open the stream on `sub`; returns every snapshot part (up to the EOSE).
async fn open_stream(client: &mut KuzzTestClient, sub: &str) -> Vec<Event> {
    client.send_raw(&stream_req(sub)).await.expect("send REQ");
    let mut parts = Vec::new();
    loop {
        match client
            .recv_event(Duration::from_secs(10))
            .await
            .expect("stream frame")
        {
            RelayMessage::Event {
                subscription_id,
                event,
            } if subscription_id == sub => {
                assert_eq!(
                    event.kind,
                    Kind::Custom(24220),
                    "only snapshot parts before the EOSE"
                );
                parts.push(*event);
            }
            RelayMessage::Eose { subscription_id } if subscription_id == sub => return parts,
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == sub => panic!("stream refused: {message}"),
            _ => {}
        }
    }
}

fn part_one(parts: &[Event]) -> Value {
    serde_json::from_str(&parts[0].content).expect("part 1 JSON")
}

#[tokio::test]
#[ignore]
async fn live_account_gets_snapshot_then_live_messages() {
    let url = relay_url();
    let keys = live_keys();
    let channel = create_channel(&keys).await;

    let mut client = KuzzTestClient::connect(&url, &keys).await.expect("connect");
    let parts = open_stream(&mut client, "us").await;
    let part = part_one(&parts);
    assert_eq!(part["v"], 1);
    assert_eq!(part["op"], "snapshot");
    assert_eq!(part["mode"], "live");
    assert_eq!(part["epoch"], "e2e");
    assert_eq!(part["kinds"], json!(FROZEN_KINDS));
    assert_eq!(part["parts"].as_u64().unwrap() as usize, parts.len());
    let rooms: serde_json::Map<String, Value> = parts
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            let content: Value = serde_json::from_str(&p.content).unwrap();
            if i > 0 {
                assert_eq!(
                    content.as_object().unwrap().keys().collect::<Vec<_>>(),
                    ["rooms"]
                );
            }
            content["rooms"].as_object().unwrap().clone()
        })
        .collect();
    assert!(rooms.contains_key(&channel), "new room in the snapshot");
    for part in &parts {
        assert_eq!(
            part.tags.iter().next().unwrap().as_slice(),
            ["p".to_string(), keys.public_key().to_hex()]
        );
    }

    // A message in the room arrives on the stream.
    let mut sender = KuzzTestClient::connect(&url, &keys)
        .await
        .expect("sender connect");
    let content = format!("stream e2e {}", uuid::Uuid::new_v4());
    let ok = sender
        .send_text_message(&keys, &channel, &content, 9)
        .await
        .expect("send");
    assert!(ok.accepted, "{}", ok.message);
    loop {
        match client
            .recv_event(Duration::from_secs(10))
            .await
            .expect("live frame")
        {
            RelayMessage::Event {
                subscription_id,
                event,
            } if subscription_id == "us" && event.content == content => break,
            _ => {}
        }
    }

    // The same id again within 5 s ends the stream.
    client.send_raw(&stream_req("us")).await.unwrap();
    loop {
        match client.recv_event(Duration::from_secs(10)).await.unwrap() {
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == "us" => {
                assert_eq!(message, "invalid: user-stream repeat");
                break;
            }
            _ => {}
        }
    }
    client.disconnect().await.unwrap();
    sender.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore]
async fn other_accounts_get_shadow_and_bad_requests_are_refused() {
    let url = relay_url();
    let keys = Keys::generate();
    let mut client = KuzzTestClient::connect(&url, &keys).await.expect("connect");
    let parts = open_stream(&mut client, "us").await;
    assert_eq!(part_one(&parts)["mode"], "shadow");

    client
        .send_raw(&json!(["REQ", "other", {"#h": ["$joined"], "kinds": [9], "limit": 0}]))
        .await
        .unwrap();
    loop {
        match client.recv_event(Duration::from_secs(10)).await.unwrap() {
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == "other" => {
                // An active stream under another id is refused first.
                assert_eq!(message, "invalid: user-stream second");
                break;
            }
            _ => {}
        }
    }
    client.disconnect().await.unwrap();
}
