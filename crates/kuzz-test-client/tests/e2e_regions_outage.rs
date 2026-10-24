//! Signing in stays alive while the primary is down (global regions S5-6),
//! end to end: with kr's relay stopped, jp answers a refresh itself from its
//! copy, the access token it gives out works in jp, and a logged-out session
//! is still refused. Once kr is back, refreshes rotate at kr again.
//!
//! `scripts/regions-two-region-test.sh` runs this file three times, one
//! phase each, and stops and starts kr's relay in between:
//!
//! ```text
//! REGION_OUTAGE_PHASE=prepare|down|after REGION_OUTAGE_STATE=<file> \
//! REGION_JP_URL=ws://localhost:3042 \
//! REGION_JP_DATABASE_URL=postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz \
//! cargo test -p kuzz-test-client --test e2e_regions_outage -- --ignored --test-threads=1
//! ```

use std::time::Duration;

use serde_json::{json, Value};

fn jp_url() -> String {
    std::env::var("REGION_JP_URL").unwrap_or_else(|_| "ws://localhost:3042".into())
}

fn http(url: &str) -> String {
    url.replace("ws://", "http://")
        .replace("wss://", "https://")
}

async fn jp_db() -> sqlx::PgPool {
    let url = std::env::var("REGION_JP_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kuzz:kuzz_dev@127.0.0.1:5492/kuzz".into());
    sqlx::PgPool::connect(&url).await.expect("jp database")
}

fn state_file() -> String {
    std::env::var("REGION_OUTAGE_STATE").expect("REGION_OUTAGE_STATE")
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

/// Sign a new account in through jp (the fake provider); returns its tokens.
async fn sign_in() -> Value {
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
        .json(&json!({"login_code": code, "code_verifier": verifier}))
        .send()
        .await
        .expect("complete");
    assert_eq!(complete.status(), 200);
    complete.json().await.expect("tokens")
}

async fn refresh(token: &str) -> (u16, Value) {
    let response = reqwest::Client::new()
        .post(format!("{}/auth/refresh", http(&jp_url())))
        .json(&json!({ "refresh": token }))
        .send()
        .await
        .expect("refresh");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

/// Status of a signed-in read that jp serves itself (blocks are kept in
/// every region).
async fn jp_read(access: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("{}/auth/blocks", http(&jp_url())))
        .bearer_auth(access)
        .send()
        .await
        .expect("blocks")
        .status()
        .as_u16()
}

/// Wait until `sql` (one `$1` text parameter) is true in jp's copy.
async fn jp_copy_shows(sql: &'static str, arg: &str) -> bool {
    let db = jp_db().await;
    for _ in 0..60 {
        let shown: bool = sqlx::query_scalar(sql)
            .bind(arg)
            .fetch_one(&db)
            .await
            .unwrap_or(false);
        if shown {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

#[tokio::test]
#[ignore = "requires two region relays (scripts/regions-two-region-test.sh)"]
async fn sign_in_survives_a_primary_outage() {
    let phase = std::env::var("REGION_OUTAGE_PHASE").expect("REGION_OUTAGE_PHASE");
    match phase.as_str() {
        "prepare" => prepare().await,
        "down" => while_down().await,
        "after" => after().await,
        other => panic!("unknown phase {other}"),
    }
}

/// kr is up: sign in two accounts through jp and log the second one out.
async fn prepare() {
    let alive = sign_in().await;
    let gone = sign_in().await;
    let logout = reqwest::Client::new()
        .post(format!("{}/auth/logout", http(&jp_url())))
        .bearer_auth(gone["access"].as_str().unwrap())
        .send()
        .await
        .expect("logout");
    assert_eq!(logout.status(), 204);
    let gone_refresh = gone["refresh"].as_str().unwrap();
    // jp's copy has both sessions, the second one revoked.
    assert!(
        jp_copy_shows(
            "SELECT EXISTS (SELECT 1 FROM refresh_tokens rt JOIN sessions s ON s.id = rt.session_id \
               WHERE rt.token_hash = sha256(convert_to($1, 'UTF8')) AND s.revoked_at IS NOT NULL)",
            gone_refresh,
        )
        .await,
        "the logout did not reach jp's copy"
    );
    assert!(
        jp_copy_shows(
            "SELECT EXISTS (SELECT 1 FROM refresh_tokens \
               WHERE token_hash = sha256(convert_to($1, 'UTF8')))",
            alive["refresh"].as_str().unwrap(),
        )
        .await
    );
    std::fs::write(
        state_file(),
        json!({ "alive": alive["refresh"], "gone": gone_refresh }).to_string(),
    )
    .expect("state file");
}

/// kr's relay is stopped and jp's copy has fallen behind.
async fn while_down() {
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(state_file()).unwrap()).unwrap();
    let alive = saved["alive"].as_str().unwrap();

    let (status, answer) = refresh(alive).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        answer["refresh"].as_str(),
        Some(alive),
        "the refresh token is not rotated while kr is down"
    );
    let access = answer["access"].as_str().expect("access").to_owned();
    let expires_in = answer["expires_in"].as_u64().expect("expires_in");
    assert!(expires_in > 0 && expires_in <= 3600, "{expires_in}");
    let read = jp_read(&access).await;
    assert_ne!(read, 401, "jp does not take its own outage token");
    assert!(matches!(read, 200 | 403), "{read}");
    let recorded: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM outage_access_tokens \
           WHERE token_hash = sha256(convert_to($1, 'UTF8')))",
    )
    .bind(&access)
    .fetch_one(&jp_db().await)
    .await
    .unwrap();
    assert!(recorded, "the token is recorded in jp's own table");

    // The app may refresh again with the same token.
    let (again, answer) = refresh(alive).await;
    assert_eq!(again, 200, "{answer}");
    assert_ne!(answer["access"].as_str(), Some(access.as_str()));

    // A logged-out session stays logged out.
    let (status, answer) = refresh(saved["gone"].as_str().unwrap()).await;
    assert_eq!(
        (status, answer["code"].as_str()),
        (401, Some("token_revoked")),
        "{answer}"
    );

    // Other sign-in requests still fail rather than split.
    let me = reqwest::Client::new()
        .get(format!("{}/auth/me", http(&jp_url())))
        .bearer_auth(&access)
        .send()
        .await
        .expect("me");
    assert_eq!(me.status(), 503);
}

/// kr is back: the same refresh token rotates at kr again.
async fn after() {
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(state_file()).unwrap()).unwrap();
    let alive = saved["alive"].as_str().unwrap();
    let mut last = (0, Value::Null);
    for _ in 0..60 {
        last = refresh(alive).await;
        if last.0 == 200 && last.1["refresh"].as_str() != Some(alive) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let (status, answer) = last;
    assert_eq!(status, 200, "{answer}");
    let rotated = answer["refresh"].as_str().expect("refresh");
    assert_ne!(rotated, alive, "kr rotates the refresh token again");
    let access = answer["access"].as_str().unwrap();
    let read = jp_read(access).await;
    assert!(matches!(read, 200 | 403), "{read}");
}
