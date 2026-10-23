//! A copy region whose copy is behind (global regions S5-4): what the copy
//! lacks is checked with the primary.
//!
//! These tests stop jp's copy for a while, so they run apart from
//! `e2e_regions`, one at a time, after it
//! (`scripts/regions-two-region-test.sh`). jp runs with
//! `KUZZ_GLOBAL_COPY_MAX_LAG_SECS=3`.

use std::time::Duration;

use nostr::{Event, EventBuilder, Keys, Kind, Tag};
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

/// Sign up through jp (made in kr, S5-2); the new access token.
async fn sign_up_through_jp() -> String {
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
    let login: Value = complete.json().await.expect("tokens");
    login["access"].as_str().expect("access").to_owned()
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn a_token_minted_while_the_copy_waits_is_admitted_in_jp() {
    // The copy is stopped while the token is minted in kr, and started again
    // a moment after jp sees the token: jp asks kr, which knows it, and
    // waits for the copy instead of refusing it.
    jp_copy(false).await;
    let _back = CopyBack;
    let access = sign_up_through_jp().await;
    let restart = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        jp_copy(true).await;
    });
    // Blocks are kept in every region: jp checks the token itself.
    let blocks = reqwest::Client::new()
        .get(format!("{}/auth/blocks", http(&jp_url())))
        .bearer_auth(&access)
        .send()
        .await
        .expect("blocks");
    let status = blocks.status().as_u16();
    let body = blocks.text().await.unwrap_or_default();
    restart.await.expect("copy restarted");
    // A token-only account may have no messaging key yet (403); either way
    // jp accepted the token.
    assert!(
        status == 200 || (status == 403 && body.contains("messaging key")),
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
    let community: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM communities WHERE host = 'localhost:3041'")
            .fetch_one(&kr)
            .await
            .expect("kr community");
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
