//! `/query` order-number reads through the real router and database:
//! `after_seq` pages and the `top_level` window's `order: "seq"` mode.
use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request as HttpRequest,
};
use base64::Engine;
use kuzz_core::{
    channel::{ChannelType, ChannelVisibility},
    CommunityId,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

/// Noon UTC on the first of a month, one per `events` partition.
const JAN: u64 = 1_767_268_800;
const FEB: u64 = 1_769_947_200;
const MAR: u64 = 1_772_366_400;
const MAY: u64 = 1_777_636_800;

struct Fixture {
    state: Arc<AppState>,
    host: String,
    community: CommunityId,
    channel: Uuid,
    owner: Keys,
    pool: sqlx::PgPool,
}

impl Fixture {
    async fn new() -> Self {
        let state = super::postgres_tests::bridge_handler_test_state()
            .await
            .expect("Postgres and Redis");
        let mut state = (*state).clone();
        Arc::make_mut(&mut state.config).require_auth_token = true;
        let state = Arc::new(state);
        let host = format!("seq-{}.local", Uuid::new_v4());
        let community = state
            .db
            .ensure_configured_community(&host)
            .await
            .unwrap()
            .id;
        let channel = Uuid::new_v4();
        let owner = Keys::generate();
        state
            .db
            .create_channel_with_id(
                community,
                channel,
                "seq-page",
                ChannelType::Stream,
                ChannelVisibility::Private,
                None,
                &owner.public_key().to_bytes(),
                None,
            )
            .await
            .unwrap();
        let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
            .await
            .unwrap();
        Self {
            state,
            host,
            community,
            channel,
            owner,
            pool,
        }
    }

    async fn send_to(&self, channel: Uuid, keys: &Keys, ts: u64) -> (Event, i64) {
        let event = EventBuilder::new(Kind::Custom(9), format!("at {ts}"))
            .tags([Tag::parse(["h", &channel.to_string()]).unwrap()])
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .unwrap();
        let (stored, _) = self
            .state
            .db
            .insert_event(self.community, &event, Some(channel))
            .await
            .unwrap();
        (event, stored.channel_sequence.unwrap())
    }

    async fn send(&self, ts: u64) -> (Event, i64) {
        self.send_to(self.channel, &self.owner, ts).await
    }

    async fn post(&self, keys: &Keys, filters: Value) -> (StatusCode, Value) {
        let body = serde_json::to_vec(&filters).unwrap();
        let proof = EventBuilder::new(Kind::Custom(27235), "")
            .tags([
                Tag::parse(["u", &format!("https://{}/query", self.host)]).unwrap(),
                Tag::parse(["method", "POST"]).unwrap(),
                Tag::parse(["payload", &hex::encode(Sha256::digest(&body))]).unwrap(),
                Tag::parse(["nonce", &Uuid::new_v4().to_string()]).unwrap(),
            ])
            .sign_with_keys(keys)
            .unwrap();
        let auth = format!(
            "Nostr {}",
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&proof).unwrap())
        );
        let response = crate::router::build_router(self.state.clone())
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/query")
                    .header("host", &self.host)
                    .header("authorization", auth)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn query(&self, keys: &Keys, filter: Value) -> Value {
        let (status, body) = self.post(keys, json!([filter])).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    /// Row ids (kind 9) and the single verified relay-signed 39006 bounds.
    fn split(&self, response: &Value, d: &str) -> (Vec<String>, Value) {
        let all = response.as_array().unwrap();
        let rows = all
            .iter()
            .filter(|v| v["kind"] == 9)
            .map(|v| v["id"].as_str().unwrap().to_owned())
            .collect();
        let bounds: Vec<&Value> = all.iter().filter(|v| v["kind"] == 39006).collect();
        assert_eq!(bounds.len(), 1, "{response}");
        assert_eq!(all.last(), Some(bounds[0]), "bounds close the page");
        let event: Event = serde_json::from_value(bounds[0].clone()).unwrap();
        event.verify().unwrap();
        assert_eq!(event.pubkey, self.state.relay_keypair.public_key());
        assert_eq!(
            serde_json::to_value(&event.tags).unwrap(),
            json!([["d", d], ["h", self.channel.to_string()]])
        );
        (rows, serde_json::from_str(&event.content).unwrap())
    }
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn after_seq_pages_in_acceptance_order_with_signed_bounds() {
    let f = Fixture::new().await;
    let mut sent = Vec::new();
    for ts in [MAR, JAN, MAY, FEB] {
        sent.push(f.send(ts).await);
    }
    let id = |n: usize| sent[n].0.id.to_hex();
    let seq = |n: usize| sent[n].1;
    let ch = f.channel.to_string();
    let page = |after: i64| json!({"kinds": [9], "#h": [ch], "after_seq": after, "limit": 2});

    let first = f.query(&f.owner, page(0)).await;
    let (rows, bounds) = f.split(&first, &format!("{ch}:after_seq:0"));
    assert_eq!(rows, [id(0), id(1)]);
    assert_eq!(
        bounds,
        json!({"order": "seq", "after_seq": 0, "last_seq": seq(1), "has_more": true,
               "sequences": [[id(0), seq(0)], [id(1), seq(1)]]})
    );

    // Exact multiple: the full final page reports exhaustion.
    let second = f.query(&f.owner, page(seq(1))).await;
    let (rows, bounds) = f.split(&second, &format!("{ch}:after_seq:{}", seq(1)));
    assert_eq!(rows, [id(2), id(3)]);
    assert_eq!(bounds["has_more"], false);
    assert_eq!(bounds["last_seq"], seq(3));

    // Past the end: only the bounds, cursor unchanged.
    let empty = f.query(&f.owner, page(seq(3))).await;
    let (rows, bounds) = f.split(&empty, &format!("{ch}:after_seq:{}", seq(3)));
    assert!(rows.is_empty());
    assert_eq!(
        bounds,
        json!({"order": "seq", "after_seq": seq(3), "last_seq": seq(3), "has_more": false,
               "sequences": []})
    );

    // A deleted row stays hidden and never gets listed.
    sqlx::query("UPDATE events SET deleted_at = now() WHERE community_id = $1 AND id = $2")
        .bind(f.community.as_uuid())
        .bind(sent[1].0.id.to_bytes().to_vec())
        .execute(&f.pool)
        .await
        .unwrap();
    let after_delete = f.query(&f.owner, page(0)).await;
    let (rows, bounds) = f.split(&after_delete, &format!("{ch}:after_seq:0"));
    assert_eq!(rows, [id(0), id(2)]);
    assert_eq!(
        bounds["sequences"],
        json!([[id(0), seq(0)], [id(2), seq(2)]])
    );

    // Old requests are untouched: newest first by created_at, no bounds.
    let legacy = f
        .query(&f.owner, json!({"kinds": [9], "#h": [ch], "limit": 10}))
        .await;
    let legacy_ids: Vec<&str> = legacy
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(legacy_ids, [id(2), id(0), id(3)]);
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn after_seq_rejects_outsiders_and_invalid_filters() {
    let f = Fixture::new().await;
    f.send(JAN).await;
    let ch = f.channel.to_string();

    let (status, body) = f
        .post(
            &Keys::generate(),
            json!([{"kinds": [9], "#h": [ch], "after_seq": 0}]),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = f
        .post(
            &Keys::generate(),
            json!([{"kinds": [9], "#h": [ch], "top_level": true, "order": "seq"}]),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Another query mode, even in a sibling filter, would drop the cursor.
    for request in [
        json!([{"kinds": [9], "#h": [ch], "after_seq": 1, "resolve_thread_roots": true}]),
        json!([{"kinds": [9], "#h": [ch], "after_seq": 1}, {"inbox_summary": true}]),
        json!([{"kinds": [9], "#h": [ch], "after_seq": 1}, {"kinds": [9], "#h": [ch], "resolve_thread_roots": true}]),
    ] {
        let (status, body) = f.post(&f.owner, request.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{request} -> {body}");
    }

    let other = "22222222-2222-4222-8222-222222222222";
    for filter in [
        json!({"kinds": [9], "#h": [ch], "after_seq": -1}),
        json!({"kinds": [9], "#h": [ch], "after_seq": "1"}),
        json!({"kinds": [9], "after_seq": 1}),
        json!({"kinds": [9], "#h": [ch, other], "after_seq": 1}),
        json!({"kinds": [9], "#h": [ch], "after_seq": 1, "since": 1}),
        json!({"kinds": [9], "#h": [ch], "after_seq": 1, "until": 2}),
        json!({"kinds": [9], "#h": [ch], "after_seq": 1, "search": "x"}),
        json!({"kinds": [9], "#h": [ch], "top_level": true, "after_seq": 1}),
        json!({"kinds": [9], "#h": [ch], "top_level": true, "before_seq": 1}),
        json!({"kinds": [9], "#h": [ch], "top_level": true, "order": "time"}),
        json!({"kinds": [9], "#h": [ch], "top_level": true, "order": "seq", "until": 2}),
        json!({"kinds": [9], "#h": [ch], "top_level": true, "order": "seq",
               "before_seq": 3, "after_seq": 1}),
        json!({"kinds": [9], "#h": [ch], "top_level": true, "order": "seq", "after_seq": -2}),
    ] {
        let (status, body) = f.post(&f.owner, json!([filter])).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{filter} -> {body}");
    }
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn after_seq_hides_dm_rows_received_while_blocked() {
    let f = Fixture::new().await;
    let (a, b) = (Keys::generate(), Keys::generate());
    let pk = |k: &Keys| k.public_key().to_bytes().to_vec();
    let direct = kuzz_db::dm::create_dm(&f.pool, f.community, &[&pk(&a), &pk(&b)], &pk(&a))
        .await
        .unwrap()
        .id;
    let (before, _) = f.send_to(direct, &b, JAN).await;
    f.state
        .db
        .block_user(f.community, &pk(&a), &pk(&b))
        .await
        .unwrap();
    for ts in [JAN + 1, JAN + 2] {
        f.send_to(direct, &b, ts).await;
    }
    let (mine, _) = f.send_to(direct, &a, FEB).await;

    let page = f
        .query(
            &a,
            json!({"kinds": [9], "#h": [direct.to_string()], "after_seq": 0, "limit": 2}),
        )
        .await;
    let ids: Vec<&str> = page
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["kind"] == 9)
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [before.id.to_hex(), mine.id.to_hex()]);
    let bounds = page.as_array().unwrap().last().unwrap();
    let content: Value = serde_json::from_str(bounds["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["has_more"], false);
    assert_eq!(content["sequences"].as_array().unwrap().len(), 2);
}

#[tokio::test]
#[ignore = "requires Postgres and Redis"]
async fn top_level_seq_order_pages_and_time_window_is_unchanged() {
    let f = Fixture::new().await;
    let mut sent = Vec::new();
    for ts in [MAR, JAN, MAY] {
        sent.push(f.send(ts).await);
    }
    let id = |n: usize| sent[n].0.id.to_hex();
    let seq = |n: usize| sent[n].1;
    let ch = f.channel.to_string();
    let window = |extra: Value| {
        let mut filter = json!({"top_level": true, "#h": [ch], "kinds": [9], "limit": 2});
        for (k, v) in extra.as_object().unwrap() {
            filter[k] = v.clone();
        }
        filter
    };

    let head = f.query(&f.owner, window(json!({"order": "seq"}))).await;
    let (rows, bounds) = f.split(&head, &format!("{ch}:seq:head"));
    assert_eq!(rows, [id(2), id(1)]);
    assert_eq!(
        bounds,
        json!({"order": "seq", "direction": "older", "has_more": true, "next_seq": seq(1),
               "sequences": [[id(2), seq(2)], [id(1), seq(1)]]})
    );
    let older = f
        .query(
            &f.owner,
            window(json!({"order": "seq", "before_seq": seq(1)})),
        )
        .await;
    let (rows, bounds) = f.split(&older, &format!("{ch}:seq:before:{}", seq(1)));
    assert_eq!(rows, [id(0)]);
    assert_eq!(bounds["has_more"], false);
    assert_eq!(bounds["next_seq"], Value::Null);
    let newer = f
        .query(
            &f.owner,
            window(json!({"order": "seq", "after_seq": seq(0)})),
        )
        .await;
    let (rows, bounds) = f.split(&newer, &format!("{ch}:seq:after:{}", seq(0)));
    assert_eq!(rows, [id(1), id(2)]);
    assert_eq!(bounds["direction"], "newer");
    assert_eq!(bounds["has_more"], false);

    // Without `order`, the (created_at, id) window and its bounds are as before.
    let by_time = f.query(&f.owner, window(json!({}))).await;
    let (rows, bounds) = f.split(&by_time, &format!("{ch}:head"));
    assert_eq!(rows, [id(2), id(0)]);
    assert_eq!(
        bounds,
        json!({"has_more": true, "next_cursor": {"created_at": MAR, "id": id(0)}})
    );
}
