//! App sign-in (`start?response=json` → `id-token` → `complete`) on the real
//! HTTP/Redis/Postgres seams. The fake provider's id_token is
//! `subject|email|verified|nonce`.

use super::*;

const MOBILE_REDIRECT: &str = "app.kuzz://auth/cb";

/// `start` for app sign-in; returns `(state, nonce)`.
async fn app_start(inst: &Instance, verifier: &str) -> (String, String) {
    let state = verifier[..24].to_owned();
    let challenge = kuzz_auth::token::pkce_s256_challenge(verifier);
    let response = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state.as_str()),
                ("code_challenge", challenge.as_str()),
                ("client", "mobile"),
                ("identity_mode", "token"),
                ("redirect_uri", MOBILE_REDIRECT),
                ("response", "json"),
            ],
        )
        .send()
        .await
        .expect("start");
    assert_eq!(response.status(), 200, "app sign-in start answers JSON");
    let body: Value = response.json().await.expect("json");
    let nonce = body["nonce"].as_str().expect("nonce").to_owned();
    assert_eq!(body["client_id"], "test-client");
    (state, nonce)
}

async fn post_id_token(inst: &Instance, state: &str, id_token: &str) -> reqwest::Response {
    inst.post("/auth/oidc/google/id-token")
        .json(&json!({"state": state, "id_token": id_token}))
        .send()
        .await
        .expect("id-token")
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn app_sign_in_issues_a_login_code_once_per_nonce() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    reset_login_limits(&inst).await;
    let subject = uuid::Uuid::new_v4().to_string();

    // A token minted for another nonce is refused, and the state is spent.
    let verifier_a = verifier();
    let (state, _nonce) = app_start(&inst, &verifier_a).await;
    let response = post_id_token(&inst, &state, &format!("{subject}||false|other")).await;
    assert_eq!(response.status(), 401);
    let response = post_id_token(&inst, &state, &format!("{subject}||false|other")).await;
    assert_eq!(response.status(), 400, "a state is good for one attempt");

    // The right nonce yields a login code that `complete` redeems.
    let verifier_b = verifier();
    let (state, nonce) = app_start(&inst, &verifier_b).await;
    let response = post_id_token(&inst, &state, &format!("{subject}||false|{nonce}")).await;
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json");
    let login_code = body["login_code"].as_str().expect("login_code").to_owned();
    let first = complete(&inst, &login_code, &verifier_b).await;

    // The same identity signs into the same account again.
    let verifier_c = verifier();
    let (state, nonce) = app_start(&inst, &verifier_c).await;
    let response = post_id_token(&inst, &state, &format!("{subject}||false|{nonce}")).await;
    let body: Value = response.json().await.expect("json");
    let again = complete(&inst, body["login_code"].as_str().unwrap(), &verifier_c).await;
    assert_eq!(again.principal, first.principal);
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn app_sign_in_and_browser_states_do_not_mix() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    reset_login_limits(&inst).await;

    // An app sign-in state cannot be finished through the browser callback.
    let verifier_a = verifier();
    let (state, _nonce) = app_start(&inst, &verifier_a).await;
    let response = inst
        .get_with(
            "/auth/oidc/google/callback",
            &[("code", "someone||false"), ("state", state.as_str())],
        )
        .send()
        .await
        .expect("callback");
    assert_eq!(response.status(), 400);

    // A browser state cannot be finished with an id_token.
    let verifier_b = verifier();
    let state = verifier_b[..24].to_owned();
    let challenge = kuzz_auth::token::pkce_s256_challenge(&verifier_b);
    let response = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state.as_str()),
                ("code_challenge", challenge.as_str()),
                ("client", "mobile"),
                ("identity_mode", "token"),
                ("redirect_uri", MOBILE_REDIRECT),
            ],
        )
        .send()
        .await
        .expect("start");
    assert_eq!(response.status(), 302);
    let response = post_id_token(&inst, &state, "someone||false|x").await;
    assert_eq!(response.status(), 400);

    // App sign-in is a mobile-only start mode.
    let verifier_c = verifier();
    let challenge = kuzz_auth::token::pkce_s256_challenge(&verifier_c);
    let response = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", &verifier_c[..24]),
                ("code_challenge", challenge.as_str()),
                ("client", "cli"),
                ("identity_mode", "token"),
                ("redirect_uri", REDIRECT_URI),
                ("response", "json"),
            ],
        )
        .send()
        .await
        .expect("start");
    assert_eq!(response.status(), 400);
}

async fn display_name(inst: &Instance, access: &str) -> String {
    let me: Value = inst
        .get("/auth/me")
        .bearer_auth(access)
        .send()
        .await
        .expect("me")
        .json()
        .await
        .expect("json");
    me["display_name"].as_str().unwrap_or_default().to_owned()
}

/// Sign in with Apple's `form_post` callback, with the name Apple sends
/// only on the first sign-in.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn form_post_callback_signs_in_and_keeps_the_first_sign_in_name() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    reset_login_limits(&inst).await;
    let verifier = verifier();
    let state = verifier[..24].to_owned();
    let challenge = kuzz_auth::token::pkce_s256_challenge(&verifier);
    let response = inst
        .get_with(
            "/auth/oidc/google/start",
            &[
                ("state", state.as_str()),
                ("code_challenge", challenge.as_str()),
                ("client", "cli"),
                ("identity_mode", "token"),
                ("redirect_uri", REDIRECT_URI),
            ],
        )
        .send()
        .await
        .expect("start");
    assert_eq!(response.status(), 302);
    let subject = format!("anon-{}", uuid::Uuid::new_v4());
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", &format!("{subject}||false"))
        .append_pair("state", &state)
        .append_pair("user", r#"{"name":{"firstName":"Ji","lastName":"Ho"}}"#)
        .finish();
    let response = inst
        .post("/auth/oidc/google/callback")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .expect("callback");
    assert_eq!(response.status(), 302);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    let code = query_param(&location, "code").expect("login code");
    let login = complete(&inst, &code, &verifier).await;
    assert_eq!(display_name(&inst, &login.access).await, "Ji Ho");
}

/// An app sign-in whose token carries no name takes the one the app sends.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn app_sign_in_takes_the_platform_name_when_the_token_has_none() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let host = community(&pool).await;
    let inst = instance(&host).await;
    reset_login_limits(&inst).await;
    let verifier = verifier();
    let (state, nonce) = app_start(&inst, &verifier).await;
    let subject = format!("anon-{}", uuid::Uuid::new_v4());
    let response = inst
        .post("/auth/oidc/google/id-token")
        .json(&json!({
            "state": state,
            "id_token": format!("{subject}||false|{nonce}"),
            "name": "  Ji Ho  ",
        }))
        .send()
        .await
        .expect("id-token");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json");
    let login = complete(&inst, body["login_code"].as_str().unwrap(), &verifier).await;
    assert_eq!(display_name(&inst, &login.access).await, "Ji Ho");
}
