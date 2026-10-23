//! A copy region whose copy is behind (global regions S5-4, S5-5): what the
//! copy lacks is checked with the primary, and security checks fail closed
//! when the copy lags and the primary cannot answer.
//!
//! These tests stop jp's copy for a while, so they run apart from
//! `e2e_regions`, one at a time, after it
//! (`scripts/regions-two-region-test.sh`). jp runs with
//! `KUZZ_GLOBAL_COPY_MAX_LAG_SECS=3`, and a second jp relay on the same
//! database admits relay members only (`REGION_JP_MEMBERS_URL`).

use std::time::Duration;

use kuzz_test_client::KuzzTestClient;
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::Value;

fn kr_url() -> String {
    std::env::var("REGION_KR_URL").unwrap_or_else(|_| "ws://localhost:3041".into())
}

fn jp_url() -> String {
    std::env::var("REGION_JP_URL").unwrap_or_else(|_| "ws://localhost:3042".into())
}

fn jp_members_url() -> String {
    std::env::var("REGION_JP_MEMBERS_URL").unwrap_or_else(|_| "ws://localhost:3043".into())
}

fn http(url: &str) -> String {
    url.replace("ws://", "http://")
        .replace("wss://", "https://")
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

/// Stop or restart jp's copy of the global tables.
async fn jp_copy(on: bool) {
    let sql = if on {
        "ALTER SUBSCRIPTION kuzz_global ENABLE"
    } else {
        "ALTER SUBSCRIPTION kuzz_global DISABLE"
    };
    sqlx::query(sql)
        .execute(&region_db("jp").await)
        .await
        .expect("subscription");
}

/// Restarts jp's copy when a test ends, passed or not.
struct CopyBack;

impl Drop for CopyBack {
    fn drop(&mut self) {
        // No panics here: a failing test is already unwinding.
        let restarted = std::thread::spawn(|| {
            tokio::runtime::Runtime::new()
                .map(|runtime| runtime.block_on(jp_copy(true)))
                .is_ok()
        })
        .join();
        if !matches!(restarted, Ok(true)) {
            eprintln!("could not restart jp's copy");
        }
    }
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
            Tag::parse(["name", &format!("lag-{id}")]).unwrap(),
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

/// Sign up through jp (made in kr, S5-2); the sign-in answer with the
/// `access` and `refresh` tokens.
async fn sign_up_through_jp() -> Value {
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
    complete.json().await.expect("tokens")
}

/// jp's own answer to a token (blocks are kept in every region, so jp checks
/// the token itself): whether it accepted it, with the status and body.
async fn jp_accepts_token(access: &str) -> (bool, u16, String) {
    let blocks = reqwest::Client::new()
        .get(format!("{}/auth/blocks", http(&jp_url())))
        .bearer_auth(access)
        .send()
        .await
        .expect("blocks");
    let status = blocks.status().as_u16();
    let body = blocks.text().await.unwrap_or_default();
    // A token-only account may have no messaging key yet (403); either way
    // jp accepted the token.
    let accepted = status == 200 || (status == 403 && body.contains("messaging key"));
    (accepted, status, body)
}

/// The deployment community (the same in both regions).
async fn community() -> uuid::Uuid {
    sqlx::query_scalar("SELECT id FROM communities WHERE host = 'localhost:3041'")
        .fetch_one(&region_db("kr").await)
        .await
        .expect("kr community")
}

/// Point kr's address in the region list at `url` (written in kr) and wait
/// for jp's copy to have it.
async fn set_kr_address(url: &str) {
    sqlx::query("UPDATE regions SET connect_url = $1 WHERE id = 'kr'")
        .bind(url)
        .execute(&region_db("kr").await)
        .await
        .expect("kr address");
    let jp = region_db("jp").await;
    for _ in 0..100 {
        let copied: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM regions WHERE id = 'kr' AND connect_url = $1)",
        )
        .bind(url)
        .fetch_one(&jp)
        .await
        .expect("jp regions");
        if copied {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("jp's copy never got kr's address {url}");
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_token_minted_while_the_copy_waits_is_admitted_in_jp() {
    // The copy is stopped while the token is minted in kr, and started again
    // a moment after jp sees the token: jp asks kr, which knows it, and
    // waits for the copy instead of refusing it.
    jp_copy(false).await;
    let _back = CopyBack;
    let login = sign_up_through_jp().await;
    let access = login["access"].as_str().expect("access").to_owned();
    let restart = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        jp_copy(true).await;
    });
    let (accepted, status, body) = jp_accepts_token(&access).await;
    restart.await.expect("copy restarted");
    assert!(
        accepted,
        "jp refused a token kr had minted: {status} {body}"
    );
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_ban_made_while_the_copy_lags_is_enforced_in_jp() {
    let keys = Keys::generate();
    let room = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(&jp_url(), &keys, &create_room(&keys, &room)).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    let (status, body) = post(&jp_url(), &keys, &message(&keys, &room, "before")).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );

    // The copy stops and falls behind the limit; the ban is made in kr and
    // does not reach jp's copy.
    jp_copy(false).await;
    let _back = CopyBack;
    let kr = region_db("kr").await;
    let community = community().await;
    sqlx::query(
        "INSERT INTO community_bans (community_id, pubkey, banned, actor_pubkey) \
         VALUES ($1, $2, true, $2) \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET banned = true",
    )
    .bind(community)
    .bind(keys.public_key().to_bytes().to_vec())
    .execute(&kr)
    .await
    .expect("ban in kr");
    tokio::time::sleep(Duration::from_secs(5)).await;
    let copied: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM community_bans WHERE pubkey = $1 AND banned)",
    )
    .bind(keys.public_key().to_bytes().to_vec())
    .fetch_one(&region_db("jp").await)
    .await
    .expect("jp bans");
    assert!(!copied, "the stopped copy brought the ban");

    // jp's copy says clear, but it lags: jp asks kr, which says banned.
    let (status, body) = post(&jp_url(), &keys, &message(&keys, &room, "after")).await;
    assert!(
        body.to_string().contains("banned"),
        "a lagging jp let a banned author write: {status} {body}"
    );
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn kr_answers_reads_only_for_a_linked_region() {
    // The read route is behind the region link like every internal route.
    let response = reqwest::Client::new()
        .post(format!("{}/internal/region/global-read", http(&kr_url())))
        .json(&serde_json::json!({ "ask": "token_known", "hash": "00" }))
        .send()
        .await
        .expect("read");
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_member_added_in_kr_is_admitted_in_jp_at_once() {
    // The members-only relay in jp refuses a stranger.
    let stranger = Keys::generate();
    assert!(
        KuzzTestClient::connect(&jp_members_url(), &stranger)
            .await
            .is_err(),
        "the members-only jp relay admitted a stranger"
    );

    // A member added in kr while jp's copy waits: jp asks kr.
    jp_copy(false).await;
    let _back = CopyBack;
    let member = Keys::generate();
    sqlx::query("INSERT INTO relay_members (community_id, pubkey, role) VALUES ($1, $2, 'member')")
        .bind(community().await)
        .bind(member.public_key().to_hex())
        .execute(&region_db("kr").await)
        .await
        .expect("member in kr");
    let copied: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM relay_members WHERE pubkey = $1)")
            .bind(member.public_key().to_hex())
            .fetch_one(&region_db("jp").await)
            .await
            .expect("jp members");
    assert!(!copied, "the stopped copy brought the member");
    let admitted = KuzzTestClient::connect(&jp_members_url(), &member).await;
    assert!(
        admitted.is_ok(),
        "jp refused a member added in kr: {:?}",
        admitted.err()
    );
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_refresh_through_jp_while_the_copy_waits_gives_a_token_jp_accepts() {
    let login = sign_up_through_jp().await;
    let refresh = login["refresh"].as_str().expect("refresh").to_owned();

    // Refresh runs at kr (S5-2); the new token is not in jp's copy yet.
    jp_copy(false).await;
    let _back = CopyBack;
    let refreshed = reqwest::Client::new()
        .post(format!("{}/auth/refresh", http(&jp_url())))
        .json(&serde_json::json!({ "refresh": refresh }))
        .send()
        .await
        .expect("refresh");
    assert_eq!(refreshed.status().as_u16(), 200);
    let tokens: Value = refreshed.json().await.expect("tokens");
    let access = tokens["access"].as_str().expect("access").to_owned();
    assert_ne!(Some(access.as_str()), login["access"].as_str());

    let restart = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        jp_copy(true).await;
    });
    let (accepted, status, body) = jp_accepts_token(&access).await;
    restart.await.expect("copy restarted");
    assert!(accepted, "jp refused a refreshed token: {status} {body}");
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_room_made_in_kr_while_the_copy_waits_is_reached_from_jp() {
    // jp's copy of the room directory does not have the room: jp asks the
    // primary where it lives (S4-6) and forwards the write there.
    jp_copy(false).await;
    let _back = CopyBack;
    let keys = Keys::generate();
    let room = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(&kr_url(), &keys, &create_room(&keys, &room)).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    let event = message(&keys, &room, "through jp");
    let (status, body) = post(&jp_url(), &keys, &event).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );
    let in_kr: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM events WHERE id = decode($1, 'hex'))")
            .bind(event.id.to_hex())
            .fetch_one(&region_db("kr").await)
            .await
            .expect("kr events");
    assert!(in_kr, "the message did not reach kr");
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_lagging_jp_that_cannot_reach_kr_refuses_writes() {
    let keys = Keys::generate();
    let room = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(&jp_url(), &keys, &create_room(&keys, &room)).await;
    assert_eq!(
        (status, &body["accepted"]),
        (200, &Value::Bool(true)),
        "{body}"
    );

    // kr becomes unreachable from jp, then jp's copy stops and falls behind
    // the limit: jp can neither trust its copy nor ask kr about bans.
    let kr = region_db("kr").await;
    let address: String = sqlx::query_scalar("SELECT connect_url FROM regions WHERE id = 'kr'")
        .fetch_one(&kr)
        .await
        .expect("kr address");
    set_kr_address("ws://127.0.0.1:9").await;
    jp_copy(false).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let event = message(&keys, &room, "while jp cannot check bans");
    let (status, body) = post(&jp_url(), &keys, &event).await;

    // Back to normal before asserting, so a failure leaves the stack usable.
    jp_copy(true).await;
    set_kr_address(&address).await;

    assert_ne!(
        body["accepted"],
        Value::Bool(true),
        "a lagging jp accepted a write it could not check: {status} {body}"
    );
    assert!(
        !body.to_string().contains("banned"),
        "refused as banned, not as unchecked: {body}"
    );
    // The app's retry goes through once jp can check again (after a failed
    // call jp waits 5 s before asking kr again).
    let mut last = (0, Value::Null);
    for _ in 0..30 {
        last = post(&jp_url(), &keys, &event).await;
        if last.1["accepted"] == Value::Bool(true) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("jp still refused the retry: {} {}", last.0, last.1);
}
