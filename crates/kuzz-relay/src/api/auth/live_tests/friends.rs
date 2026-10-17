//! Live friends/block surface: `/auth/blocks`, `/auth/people/*`, the
//! `discoverable` profile field, silent DM blocking over real WebSockets, and
//! author-only contact lists.

use super::*;

async fn community_id(inst: &Instance) -> kuzz_core::CommunityId {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM communities WHERE host = $1")
        .bind(&inst.host)
        .fetch_one(&pool)
        .await
        .unwrap();
    kuzz_core::CommunityId::from_uuid(id)
}

fn key(principal: &str) -> Vec<u8> {
    hex::decode(principal).unwrap()
}

async fn status(response: reqwest::Response) -> (u16, Value) {
    let code = response.status().as_u16();
    (code, response.json().await.unwrap_or(Value::Null))
}

async fn put_block(inst: &Instance, who: &Login, target: &str) -> u16 {
    inst.http
        .put(inst.url(&format!("/auth/blocks/{target}")))
        .header("host", &inst.host)
        .bearer_auth(&who.access)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn blocked_list(inst: &Instance, who: &Login) -> Vec<String> {
    let (code, body) = status(
        inst.get("/auth/blocks")
            .bearer_auth(&who.access)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, 200, "{body}");
    body["blocked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            assert!(b["created_at"].as_str().is_some_and(|t| t.ends_with('Z')));
            b["pubkey"].as_str().unwrap().to_owned()
        })
        .collect()
}

async fn lookup(inst: &Instance, who: &Login, q: &str) -> (u16, Value) {
    status(
        inst.get_with("/auth/people/lookup", &[("q", q)])
            .bearer_auth(&who.access)
            .send()
            .await
            .unwrap(),
    )
    .await
}

async fn reset_people_limits(inst: &Instance, who: &Login) {
    let mut redis = inst.state.redis_pool.get().await.expect("redis");
    let _: () = redis::cmd("DEL")
        .arg(format!(
            "kuzz:ratelimit:auth:people:lookup:{}",
            who.principal
        ))
        .arg(format!(
            "kuzz:ratelimit:auth:people:shared:{}",
            who.principal
        ))
        .query_async(&mut redis)
        .await
        .expect("reset people limits");
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn blocks_and_people_endpoints_follow_the_contract() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..12];
    let alice = new_user(&inst).await;
    let bob_email = format!("Bob.{tag}@Example.com");
    let bob = login(&inst, &uuid::Uuid::new_v4().to_string(), &bob_email, true).await;

    // `discoverable` defaults on and is settable with POST as well as PATCH.
    let (code, me) = me_status(&inst, &bob.access).await;
    assert_eq!(code, 200);
    assert_eq!(me["discoverable"], true);
    let bob_name = format!("bob_{tag}");
    let (code, body) = status(
        inst.post("/auth/profile")
            .bearer_auth(&bob.access)
            .json(&json!({"username": bob_name}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["discoverable"], true);
    let (code, _) = status(
        inst.patch("/auth/profile")
            .bearer_auth(&bob.access)
            .json(&json!({"discoverable": "no"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, 400, "discoverable must be a boolean");

    // Exact lookup by username (with or without @, any case) and by email.
    for q in [
        bob_name.clone(),
        format!("@{bob_name}"),
        bob_name.to_uppercase(),
        format!("  {}  ", bob_email.to_lowercase()),
    ] {
        let (code, found) = lookup(&inst, &alice, &q).await;
        assert_eq!(code, 200, "{q}: {found}");
        assert_eq!(found["pubkey"], bob.principal.as_str());
        assert_eq!(found["username"], bob_name.as_str());
        assert!(found.get("email").is_none(), "never returns email: {found}");
        assert!(!found.to_string().to_lowercase().contains("example.com"));
    }
    assert_eq!(lookup(&inst, &alice, &bob_name[..5]).await.0, 404);
    assert_eq!(lookup(&inst, &alice, "").await.0, 400);

    // Discovery off: both routes miss.
    let (code, body) = status(
        inst.patch("/auth/profile")
            .bearer_auth(&bob.access)
            .json(&json!({"discoverable": false}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(me_status(&inst, &bob.access).await.1["discoverable"], false);
    assert_eq!(lookup(&inst, &alice, &bob_name).await.0, 404);
    assert_eq!(lookup(&inst, &alice, &bob_email).await.0, 404);

    // 20 lookups per minute per person.
    reset_people_limits(&inst, &alice).await;
    for _ in 0..20 {
        assert_eq!(lookup(&inst, &alice, "nobody_here_x").await.0, 404);
    }
    assert_eq!(lookup(&inst, &alice, "nobody_here_x").await.0, 429);
    reset_people_limits(&inst, &alice).await;

    // Blocks: self → 400, idempotent PUT/DELETE, private list.
    assert_eq!(put_block(&inst, &alice, &alice.principal).await, 400);
    assert_eq!(put_block(&inst, &alice, "not-hex").await, 400);
    assert_eq!(put_block(&inst, &alice, &bob.principal).await, 204);
    assert_eq!(put_block(&inst, &alice, &bob.principal).await, 204);
    assert_eq!(
        blocked_list(&inst, &alice).await,
        vec![bob.principal.clone()]
    );
    assert!(
        blocked_list(&inst, &bob).await.is_empty(),
        "blocked side sees nothing"
    );
    for _ in 0..2 {
        let code = inst
            .delete(&format!("/auth/blocks/{}", bob.principal))
            .bearer_auth(&alice.access)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(code, 204);
    }
    assert!(blocked_list(&inst, &alice).await.is_empty());
    let unauthenticated = inst.get("/auth/blocks").send().await.unwrap().status();
    assert_eq!(unauthenticated, 401);

    // Shared-channel search: prefix among channel mates, minus blocked people.
    let scope = community_id(&inst).await;
    let carol = new_user(&inst).await;
    let outsider = new_user(&inst).await;
    for (who, name) in [
        (&alice, "Alice"),
        (&bob, "Bobby"),
        (&carol, "Bobcat"),
        (&outsider, "Bob Outside"),
    ] {
        sqlx::query(
            "INSERT INTO users (community_id, pubkey, display_name) VALUES ($1, $2, $3) \
             ON CONFLICT (community_id, pubkey) DO UPDATE SET display_name = EXCLUDED.display_name",
        )
        .bind(scope.as_uuid())
        .bind(key(&who.principal))
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }
    let room = kuzz_db::channel::create_channel(
        &pool,
        scope,
        "friends",
        kuzz_db::channel::ChannelType::Stream,
        kuzz_db::channel::ChannelVisibility::Open,
        None,
        &key(&alice.principal),
        None,
    )
    .await
    .unwrap()
    .id;
    for who in [&bob, &carol] {
        kuzz_db::channel_members::add_member(
            &pool,
            scope,
            room,
            &key(&who.principal),
            kuzz_db::channel::MemberRole::Member,
            None,
        )
        .await
        .unwrap();
    }
    let shared = |q: &'static str| {
        inst.get_with("/auth/people/shared", &[("q", q), ("limit", "50")])
            .bearer_auth(&alice.access)
            .send()
    };
    let (code, body) = status(shared("bob").await.unwrap()).await;
    assert_eq!(code, 200, "{body}");
    let names: Vec<&str> = body["people"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Bobby", "Bobcat"]);
    assert_eq!(put_block(&inst, &alice, &carol.principal).await, 204);
    let (_, body) = status(shared("bob").await.unwrap()).await;
    assert_eq!(body["people"].as_array().unwrap().len(), 1);
    assert_eq!(body["people"][0]["pubkey"], bob.principal.as_str());
    assert!(body["people"][0].get("email").is_none());
    assert_eq!(status(shared("").await.unwrap()).await.0, 400);
}

/// Wait briefly for an EVENT on `sub`; `None` when nothing arrives.
async fn maybe_event(ws: &mut Ws, sub: &str, within: Duration) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = match tokio::time::timeout(remaining, ws.next()).await {
            Err(_) => return None,
            Ok(frame) => frame.expect("stream open").expect("frame ok"),
        };
        if let Message::Text(text) = frame {
            let value: Value = serde_json::from_str(&text).unwrap();
            if value[0] == "EVENT" && value[1] == sub {
                return Some(value[2].clone());
            }
        }
    }
}

async fn send_message(ws: &mut Ws, channel: uuid::Uuid, text: &str) -> String {
    send(
        ws,
        json!(["EVENT", {"kind": 9, "content": text, "tags": [["h", channel.to_string()]]}]),
    )
    .await;
    let ok = next_ok(ws).await;
    assert_eq!(ok[2], true, "blocked sender still gets a normal OK: {ok}");
    ok[1].as_str().unwrap().to_owned()
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn silent_block_hides_dm_events_from_the_blocker_only() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let scope = community_id(&inst).await;
    let alice = new_user(&inst).await;
    let bob = new_user(&inst).await;
    let dm = kuzz_db::dm::create_dm(
        &pool,
        scope,
        &[&key(&alice.principal), &key(&bob.principal)],
        &key(&alice.principal),
    )
    .await
    .unwrap()
    .id;
    let room = kuzz_db::channel::create_channel(
        &pool,
        scope,
        "shared",
        kuzz_db::channel::ChannelType::Stream,
        kuzz_db::channel::ChannelVisibility::Open,
        None,
        &key(&alice.principal),
        None,
    )
    .await
    .unwrap()
    .id;
    kuzz_db::channel_members::add_member(
        &pool,
        scope,
        room,
        &key(&bob.principal),
        kuzz_db::channel::MemberRole::Member,
        None,
    )
    .await
    .unwrap();
    assert_eq!(put_block(&inst, &alice, &bob.principal).await, 204);

    let mut alice_ws = inst.ws().await;
    assert_eq!(auth_ws(&mut alice_ws, &alice.access).await[2], true);
    let mut bob_ws = inst.ws().await;
    assert_eq!(auth_ws(&mut bob_ws, &bob.access).await[2], true);
    let filter = json!({"kinds": [9], "#h": [dm.to_string(), room.to_string()]});
    req_eose(&mut alice_ws, "live", filter.clone()).await;
    req_eose(&mut bob_ws, "live", filter.clone()).await;

    // Bob's DM message: accepted as usual, never delivered to Alice.
    let hidden = send_message(&mut bob_ws, dm, "you cannot see this").await;
    assert!(
        maybe_event(&mut alice_ws, "live", Duration::from_millis(1500))
            .await
            .is_none(),
        "blocked DM message must not reach the blocker"
    );
    // Bob's shared-channel message is delivered (the client folds it).
    let shared = send_message(&mut bob_ws, room, "shared room").await;
    assert_eq!(
        maybe_event(&mut alice_ws, "live", Duration::from_secs(5))
            .await
            .unwrap()["id"],
        shared.as_str()
    );

    // History, COUNT and the HTTP bridge agree.
    let history = req_eose(&mut alice_ws, "hist", filter.clone()).await;
    let ids: Vec<&str> = history.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&shared.as_str()) && !ids.contains(&hidden.as_str()));
    let bob_history = req_eose(&mut bob_ws, "hist", filter.clone()).await;
    assert!(bob_history.iter().any(|e| e["id"] == hidden.as_str()));
    send(
        &mut alice_ws,
        json!(["COUNT", "c", {"kinds": [9], "#h": [dm.to_string()]}]),
    )
    .await;
    loop {
        let frame = next_json(&mut alice_ws).await;
        if frame[0] == "COUNT" {
            assert_eq!(frame[2]["count"], 0, "{frame}");
            break;
        }
    }
    let queried: Value = inst
        .post("/query")
        .bearer_auth(&alice.access)
        .json(&json!([{"kinds": [9], "#h": [dm.to_string()]}]))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queried, json!([]), "bridge hides the DM message too");

    // Unblock: delivery resumes.
    let code = inst
        .delete(&format!("/auth/blocks/{}", bob.principal))
        .bearer_auth(&alice.access)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(code, 204);
    let visible = send_message(&mut bob_ws, dm, "visible again").await;
    assert_eq!(
        maybe_event(&mut alice_ws, "live", Duration::from_secs(5))
            .await
            .unwrap()["id"],
        visible.as_str()
    );
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn contact_lists_are_readable_only_by_their_author() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let alice = new_user(&inst).await;
    let bob = new_user(&inst).await;
    let mut alice_ws = inst.ws().await;
    assert_eq!(auth_ws(&mut alice_ws, &alice.access).await[2], true);
    send(
        &mut alice_ws,
        json!(["EVENT", {"kind": 3, "content": "", "tags": [["p", bob.principal]]}]),
    )
    .await;
    let ok = next_ok(&mut alice_ws).await;
    assert_eq!(ok[2], true, "{ok}");
    let own = req_eose(
        &mut alice_ws,
        "mine",
        json!({"kinds": [3], "authors": [alice.principal]}),
    )
    .await;
    assert_eq!(own.len(), 1);

    let mut bob_ws = inst.ws().await;
    assert_eq!(auth_ws(&mut bob_ws, &bob.access).await[2], true);
    send(
        &mut bob_ws,
        json!(["REQ", "theirs", {"kinds": [3], "authors": [alice.principal]}]),
    )
    .await;
    loop {
        let frame = next_json(&mut bob_ws).await;
        assert_ne!(
            frame[0], "EVENT",
            "another person's contact list leaked: {frame}"
        );
        if frame[0] == "CLOSED" || frame[0] == "EOSE" {
            break;
        }
    }
    let by_id = req_eose(&mut bob_ws, "byid", json!({"ids": [ok[1]]})).await;
    assert!(by_id.is_empty(), "id lookup must not leak it either");
}
