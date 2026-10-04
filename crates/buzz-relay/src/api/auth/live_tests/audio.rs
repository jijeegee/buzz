//! Live huddle-audio token AUTH (plan §3.4, Phase 2): the production
//! `/huddle/{channel}/audio` route behind `build_router`, real Postgres +
//! Redis, tokens minted through the real `/auth/*` flow.

use super::*;

type AudioWs = Ws;

/// The community id behind `inst.host`.
async fn community_id(inst: &Instance) -> uuid::Uuid {
    sqlx::query_scalar("SELECT id FROM communities WHERE host = $1")
        .bind(&inst.host)
        .fetch_one(inst.state.db.pool())
        .await
        .expect("community id")
}

/// An open stream channel whose members are `members` (hex principal ids).
async fn huddle_channel(inst: &Instance, members: &[&str]) -> uuid::Uuid {
    let community = community_id(inst).await;
    let channel = uuid::Uuid::new_v4();
    let creator = hex::decode(members[0]).expect("hex");
    sqlx::query(
        "INSERT INTO channels (id, community_id, name, channel_type, visibility, created_by) \
         VALUES ($1, $2, $3, 'stream', 'open', $4)",
    )
    .bind(channel)
    .bind(community)
    .bind(format!("huddle-{}", channel.simple()))
    .bind(&creator)
    .execute(inst.state.db.pool())
    .await
    .expect("seed channel");
    for member in members {
        sqlx::query(
            "INSERT INTO channel_members (community_id, channel_id, pubkey, role, invited_by) \
             VALUES ($1, $2, $3, 'member', $4)",
        )
        .bind(community)
        .bind(channel)
        .bind(hex::decode(member).expect("hex"))
        .bind(&creator)
        .execute(inst.state.db.pool())
        .await
        .expect("seed member");
    }
    channel
}

/// Open the audio socket and consume the server's challenge.
async fn audio_ws(inst: &Instance, channel: uuid::Uuid) -> AudioWs {
    let mut request = format!("ws://{}/huddle/{channel}/audio", inst.addr)
        .into_client_request()
        .expect("ws request");
    request
        .headers_mut()
        .insert("host", inst.host.parse().expect("host header"));
    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("audio ws connect");
    let challenge = next_json(&mut ws).await;
    assert_eq!(challenge["type"], "challenge");
    ws
}

async fn send_token(ws: &mut AudioWs, token: &str) {
    send(
        ws,
        json!({"type": "auth", "token": token, "protocol_version": 2}),
    )
    .await;
}

/// Next text frame whose `type` is not roster/presence chatter.
async fn next_control(ws: &mut AudioWs) -> Value {
    loop {
        let frame = next_json(ws).await;
        if !matches!(frame["type"].as_str(), Some("roster" | "speaking")) {
            return frame;
        }
    }
}

/// Wait for the server to close; returns every JSON text frame seen first.
async fn audio_closed(ws: &mut AudioWs, within: Duration) -> Vec<Value> {
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, ws.next()).await {
            Err(_) => panic!("audio socket still open after {within:?}; frames: {frames:?}"),
            Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_)))) => return frames,
            Ok(Some(Ok(Message::Text(text)))) => {
                frames.push(serde_json::from_str(&text).unwrap_or(Value::Null));
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

fn hash_of(token: &str) -> [u8; 32] {
    buzz_auth::TokenSecret::new(token.to_owned()).hash()
}

async fn refresh(inst: &Instance, login: &Login) -> Login {
    let response = inst
        .post("/auth/refresh")
        .json(&json!({"refresh": login.refresh}))
        .send()
        .await
        .expect("refresh");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("json");
    Login {
        principal: login.principal.clone(),
        access: body["access"].as_str().unwrap().to_owned(),
        refresh: body["refresh"].as_str().unwrap().to_owned(),
    }
}

/// A user and a hosted bot each join a huddle with their access token; the
/// user's socket is the user's principal (no NIP-42 proof involved).
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn audio_token_auth_joins_users_and_bots() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    let user = new_user(&inst).await;
    let bot = create_bot(&inst, &user, "this_device").await;
    let bot_access = bot_token(&inst, &user, &bot).await;
    let channel = huddle_channel(&inst, &[&user.principal, &bot]).await;

    let mut ws = audio_ws(&inst, channel).await;
    send_token(&mut ws, &user.access).await;
    let joined = next_control(&mut ws).await;
    assert_eq!(joined["type"], "joined", "{joined}");

    let mut bot_ws = audio_ws(&inst, channel).await;
    send_token(&mut bot_ws, &bot_access).await;
    let joined = next_control(&mut bot_ws).await;
    assert_eq!(joined["type"], "joined", "{joined}");
    let peers: Vec<&str> = joined["peers"]
        .as_array()
        .expect("peers")
        .iter()
        .filter_map(|peer| peer["pubkey"].as_str())
        .collect();
    assert!(
        peers.contains(&bot.as_str()) && peers.contains(&user.principal.as_str()),
        "roster must name both principals: {joined}"
    );
}

/// An unknown token is refused with the token code and the socket closes.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn audio_token_auth_rejects_invalid_token() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    let user = new_user(&inst).await;
    let channel = huddle_channel(&inst, &[&user.principal]).await;

    let mut ws = audio_ws(&inst, channel).await;
    send_token(&mut ws, "bzs_not-a-real-token-0000000000000000000000000").await;
    let frames = audio_closed(&mut ws, Duration::from_secs(10)).await;
    assert_eq!(
        frames.first().map(|f| f["message"].clone()),
        Some(json!("auth-required: invalid_token")),
        "{frames:?}"
    );
}

/// Flag off: the token form is ignored exactly like any frame without a
/// NIP-42 event — no admission, the 5 s auth window lapses and the socket
/// closes without a reply.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn audio_token_form_is_ignored_with_token_auth_off() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    // Mint a real token on an enabled instance, then present it to a
    // disabled one for the same community.
    let host = community(&pool).await;
    let enabled = instance(&host).await;
    let user = new_user(&enabled).await;
    let off = instance_with(&host, false, None).await;
    let channel = huddle_channel(&off, &[&user.principal]).await;

    let mut ws = audio_ws(&off, channel).await;
    send_token(&mut ws, &user.access).await;
    let frames = audio_closed(&mut ws, Duration::from_secs(10)).await;
    assert!(
        frames.is_empty(),
        "flag off must not answer a token AUTH: {frames:?}"
    );
}

/// The bound token's revocation closes the audio socket; after a same-
/// principal re-AUTH the socket follows the new token: revoking the old one
/// no longer affects it, revoking the new one closes it.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn audio_socket_follows_reauth_and_closes_on_revocation() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    let first = new_user(&inst).await;
    let other = new_user(&inst).await;
    let channel = huddle_channel(&inst, &[&first.principal]).await;

    let mut ws = audio_ws(&inst, channel).await;
    send_token(&mut ws, &first.access).await;
    assert_eq!(next_control(&mut ws).await["type"], "joined");

    // Same principal, refreshed access token → auth_ok, binding swapped.
    let second = refresh(&inst, &first).await;
    send_token(&mut ws, &second.access).await;
    assert_eq!(next_control(&mut ws).await["type"], "auth_ok");

    // An invalid re-AUTH keeps the binding.
    send_token(&mut ws, "bzs_bogus-000000000000000000000000000000000000").await;
    let reply = next_control(&mut ws).await;
    assert_eq!(reply["type"], "auth_error", "{reply}");

    // Revoking the token the socket no longer holds leaves it open.
    crate::identity::publish_revocations(&inst.state, &[hash_of(&first.access)], "test", None)
        .await;
    send_token(&mut ws, &second.access).await;
    assert_eq!(
        next_control(&mut ws).await["type"],
        "auth_ok",
        "socket must survive revocation of the token it re-AUTHed away from"
    );

    // Revoking the bound token closes it with the revocation notice.
    crate::identity::publish_revocations(&inst.state, &[hash_of(&second.access)], "test", None)
        .await;
    let frames = audio_closed(&mut ws, Duration::from_secs(10)).await;
    assert!(
        frames.iter().any(|f| f["message"] == "auth-revoked: test"),
        "{frames:?}"
    );

    // A different principal's token on re-AUTH closes the socket.
    let mut ws = audio_ws(&inst, huddle_channel(&inst, &[&other.principal]).await).await;
    send_token(&mut ws, &other.access).await;
    assert_eq!(next_control(&mut ws).await["type"], "joined");
    send_token(&mut ws, &second.access).await;
    let frames = audio_closed(&mut ws, Duration::from_secs(10)).await;
    assert!(
        frames
            .iter()
            .any(|f| f["message"] == "auth-required: principal mismatch"),
        "{frames:?}"
    );
}

/// The bound token's deadline closes the socket (the binding watch).
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn audio_socket_closes_at_token_expiry() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    let user = new_user(&inst).await;
    let channel = huddle_channel(&inst, &[&user.principal]).await;
    sqlx::query(
        "UPDATE access_tokens SET expires_at = now() + interval '3 seconds' \
         WHERE token_hash = $1",
    )
    .bind(hash_of(&user.access).as_slice())
    .execute(&pool)
    .await
    .expect("shorten token");

    let mut ws = audio_ws(&inst, channel).await;
    send_token(&mut ws, &user.access).await;
    assert_eq!(next_control(&mut ws).await["type"], "joined");
    let frames = audio_closed(&mut ws, Duration::from_secs(15)).await;
    assert!(
        frames
            .iter()
            .any(|f| f["message"] == "auth-expired: token_expired"),
        "{frames:?}"
    );
}

/// The token ban gate: a banned principal is refused at audio join, and so
/// is a bot whose *owner* is banned (the cascade the root route applies).
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn audio_token_auth_refuses_banned_principals_and_bots_of_banned_owners() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    let community = buzz_core::CommunityId::from_uuid(community_id(&inst).await);
    let banned = new_user(&inst).await;
    let owner = new_user(&inst).await;
    let bot = create_bot(&inst, &owner, "this_device").await;
    let bot_access = bot_token(&inst, &owner, &bot).await;
    let channel = huddle_channel(&inst, &[&banned.principal, &owner.principal, &bot]).await;
    let actor = hex::decode(&owner.principal).expect("hex");
    for target in [&banned.principal, &owner.principal] {
        inst.state
            .db
            .ban_community_member(
                community,
                &hex::decode(target).expect("hex"),
                &actor,
                None,
                None,
            )
            .await
            .expect("seed ban");
    }

    for token in [banned.access.as_str(), bot_access.as_str()] {
        let mut ws = audio_ws(&inst, channel).await;
        send_token(&mut ws, token).await;
        let frames = audio_closed(&mut ws, Duration::from_secs(10)).await;
        assert!(
            frames
                .iter()
                .any(|f| f["message"] == "blocked: you are banned from this community"),
            "{frames:?}"
        );
        assert!(!frames.iter().any(|f| f["type"] == "joined"), "{frames:?}");
    }
    // The gate refuses before any admission side effect: the banned owner's
    // bot never gets its owner link (and with it a `users` row) recorded.
    let bot_users_row: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM users WHERE community_id = $1 AND pubkey = $2")
            .bind(community.as_uuid())
            .bind(hex::decode(&bot).expect("hex"))
            .fetch_optional(&pool)
            .await
            .expect("users lookup");
    assert_eq!(
        bot_users_row, None,
        "refused before the owner link is written"
    );
}
