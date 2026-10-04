//! Live server-published kind:0 profiles (plan §3.7, Phase 2): a token
//! principal's first write in a community publishes its kind:0 there, the
//! profile PATCH endpoints republish it, AUTH reconciles only where the
//! principal already has a `users` row, and concurrent publishers serialize on
//! the per-(community, principal) lock.

use super::*;

const DEADLINE: Duration = Duration::from_secs(5);

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn profile_retry_repairs_projection_after_event_commit() {
    use crate::identity::profile::{publish_profile_in_community, Scope};
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let user = new_user(&inst).await;
    let principal = buzz_core::principal::PrincipalId::from_hex(&user.principal).unwrap();
    let id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM communities WHERE host = $1")
        .bind(&inst.host)
        .fetch_one(&pool)
        .await
        .unwrap();
    let tenant = buzz_core::tenant::TenantContext::resolved(
        buzz_core::CommunityId::from_uuid(id),
        inst.host.clone(),
    );
    let trigger = format!("profile_fault_{}", uuid::Uuid::new_v4().simple());
    // Only generated UUID identifiers and a parsed principal's hex enter this DDL.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.pubkey = decode('{}','hex') THEN RAISE EXCEPTION 'projection fault'; END IF; RETURN NEW; END $$; CREATE TRIGGER {trigger} BEFORE INSERT OR UPDATE ON users FOR EACH ROW EXECUTE FUNCTION {trigger}();", user.principal
    ))).execute(&pool).await.unwrap();
    let failed =
        publish_profile_in_community(&inst.state, &tenant, &principal, Scope::Participant).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP TRIGGER {trigger} ON users; DROP FUNCTION {trigger}();"
    )))
    .execute(&pool)
    .await
    .unwrap();
    assert!(failed.is_err(), "projection fault must propagate");
    let (lock, snapshot) = inst
        .state
        .db
        .lock_principal_profile_publish(tenant.community(), &principal)
        .await
        .unwrap();
    assert!(
        snapshot.latest.is_some(),
        "event was durably committed before projection failed"
    );
    assert!(!snapshot.has_user_row);
    lock.release().await.unwrap();
    assert!(
        !publish_profile_in_community(&inst.state, &tenant, &principal, Scope::Member)
            .await
            .unwrap(),
        "retry repairs without a new event"
    );
    assert_eq!(
        users_display_name(&inst, &user.principal).await.as_deref(),
        Some("Test User")
    );
}

/// Distinct (by id) kind:0 events of `author` visible over `ws`. A REQ can
/// deliver the same event twice (backfill and live overlap); that is correct
/// relay behavior, so de-duplicate.
async fn profiles(ws: &mut Ws, author: &str) -> Vec<Value> {
    let sub = format!("p{}", uuid::Uuid::new_v4().simple());
    let mut events = req_eose(ws, &sub, json!({"kinds": [0], "authors": [author]})).await;
    send(ws, json!(["CLOSE", sub])).await;
    let mut seen = std::collections::HashSet::new();
    events.retain(|event| seen.insert(event["id"].as_str().unwrap_or_default().to_owned()));
    events
}

fn content(event: &Value) -> Value {
    serde_json::from_str(event["content"].as_str().expect("content")).expect("json content")
}

/// Poll until `author`'s single live kind:0 is named `name`.
async fn profile_named(ws: &mut Ws, author: &str, name: &str) -> Value {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        let events = profiles(ws, author).await;
        if let [event] = events.as_slice() {
            if content(event)["name"] == name {
                return event.clone();
            }
        }
        assert!(events.len() <= 1, "kind:0 is replaceable: {events:?}");
        assert!(
            tokio::time::Instant::now() < deadline,
            "no kind:0 named {name:?} for {author}; saw {events:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn users_display_name(inst: &Instance, principal: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT u.display_name FROM users u JOIN communities c ON c.id = u.community_id \
         WHERE c.host = $1 AND u.pubkey = $2",
    )
    .bind(&inst.host)
    .bind(hex::decode(principal).expect("hex"))
    .fetch_optional(inst.state.db.pool())
    .await
    .expect("users row")
    .flatten()
}

/// Poll until the `users` projection says `name` (the kind:0 side effect
/// commits after the event becomes visible to REQ).
async fn users_named(inst: &Instance, principal: &str, name: &str) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    loop {
        let current = users_display_name(inst, principal).await;
        if current.as_deref() == Some(name) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "users.display_name is {current:?}, want {name:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn patch(inst: &Instance, path: &str, token: &str, body: Value) {
    let response = inst
        .http
        .patch(inst.url(path))
        .header("host", &inst.host)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .expect("patch");
    assert_eq!(response.status(), 200, "{path}");
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn server_publishes_kind0_for_users_and_bots() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    // The relay principal is the relay key, so relay-authored events keep
    // one author across signing and stamping.
    assert_eq!(
        inst.state.identity.relay_principal().map(|p| p.to_hex()),
        Some(inst.state.relay_keypair.public_key().to_hex())
    );

    // A user who only reads is not announced: AUTH reconciles members only.
    let user = new_user(&inst).await;
    let mut ws = inst.ws().await;
    assert_eq!(auth_ws(&mut ws, &user.access).await[2], true);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(profiles(&mut ws, &user.principal).await.is_empty());

    // The first accepted write publishes the Google profile name.
    send(
        &mut ws,
        json!(["EVENT", {"kind": 1, "content": "hi", "tags": []}]),
    )
    .await;
    assert_eq!(next_ok(&mut ws).await[2], true);
    let event = profile_named(&mut ws, &user.principal, "Test User").await;
    assert_eq!(event["pubkey"], user.principal.as_str());
    assert_eq!(content(&event)["display_name"], "Test User");
    users_named(&inst, &user.principal, "Test User").await;

    // PATCH /auth/profile republishes (fan-out spawned after the commit).
    patch(
        &inst,
        "/auth/profile",
        &user.access,
        json!({"display_name": "Renamed", "avatar_url": "https://img.test/a.png"}),
    )
    .await;
    let renamed = profile_named(&mut ws, &user.principal, "Renamed").await;
    assert_eq!(content(&renamed)["picture"], "https://img.test/a.png");
    users_named(&inst, &user.principal, "Renamed").await;

    // A repeated AUTH with an unchanged profile publishes nothing new.
    let mut again = inst.ws().await;
    assert_eq!(auth_ws(&mut again, &user.access).await[2], true);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let events = profiles(&mut again, &user.principal).await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0]["id"], renamed["id"],
        "no republish without a change"
    );

    // Bot: AUTH records its owner link (creating its users row), so the
    // member-scoped reconcile publishes its name with the bot flag.
    let bot = create_bot(&inst, &user, "this_device").await;
    let token = bot_token(&inst, &user, &bot).await;
    let mut bot_ws = inst.ws().await;
    assert_eq!(auth_ws(&mut bot_ws, &token).await[2], true);
    let event = profile_named(&mut bot_ws, &bot, "helper").await;
    assert_eq!(content(&event)["bot"], true);

    patch(
        &inst,
        &format!("/auth/bots/{bot}/profile"),
        &user.access,
        json!({"display_name": "Helper Two"}),
    )
    .await;
    profile_named(&mut ws, &bot, "Helper Two").await;

    // A publish that never happened (simulated: the row changes with no
    // publish) converges on the next same-connection re-AUTH (Rule 1).
    sqlx::query("UPDATE principals SET display_name = 'Missed' WHERE id = $1")
        .bind(hex::decode(&user.principal).expect("hex"))
        .execute(&pool)
        .await
        .expect("rename without publish");
    let refreshed: Value = inst
        .post("/auth/refresh")
        .json(&json!({"refresh": user.refresh}))
        .send()
        .await
        .expect("refresh")
        .json()
        .await
        .expect("refresh json");
    let access = refreshed["access"].as_str().expect("access");
    // `next_ok`: skip CLOSED frames from the subscriptions closed above.
    send(&mut ws, json!(["AUTH", {"token": access}])).await;
    assert_eq!(next_ok(&mut ws).await[2], true, "re-AUTH");
    profile_named(&mut ws, &user.principal, "Missed").await;
}

/// B2 regression: a publish waits for the per-(community, principal) lock and
/// re-reads the principal row under it. A publisher that had read the row
/// before the lock (the race between an AUTH reconcile and a PATCH) would
/// publish the stale name after the newer one.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn profile_publish_serializes_and_reads_the_row_under_the_lock() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    let user = new_user(&inst).await;
    let principal = buzz_core::principal::PrincipalId::from_hex(&user.principal).expect("id");
    let community_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM communities WHERE host = $1")
        .bind(&inst.host)
        .fetch_one(&pool)
        .await
        .expect("community");
    let community = buzz_core::CommunityId::from_uuid(community_id);
    let tenant = buzz_core::tenant::TenantContext::resolved(community, inst.host.clone());

    // Hold the lock, as a concurrent publisher would.
    let (held, _) = inst
        .state
        .db
        .lock_principal_profile_publish(community, &principal)
        .await
        .expect("lock");
    let publish = tokio::spawn({
        let state = Arc::clone(&inst.state);
        let tenant = tenant.clone();
        async move {
            crate::identity::profile::publish_profile_in_community(
                &state,
                &tenant,
                &principal,
                crate::identity::profile::Scope::Participant,
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !publish.is_finished(),
        "a second publisher must wait for the lock"
    );

    // The profile changes while the publisher waits; it must publish this.
    sqlx::query("UPDATE principals SET display_name = 'Newest' WHERE id = $1")
        .bind(principal.as_bytes().as_slice())
        .execute(&pool)
        .await
        .expect("rename");
    held.release().await.expect("release");
    assert!(publish.await.expect("join").expect("publish"));

    // What that publisher itself wrote (read before any AUTH could reconcile).
    let latest = inst
        .state
        .db
        .lock_principal_profile_publish(community, &principal)
        .await
        .expect("read latest");
    let content: Value =
        serde_json::from_str(&latest.1.latest.expect("published").content).expect("json");
    latest.0.release().await.expect("release");
    assert_eq!(content["name"], "Newest", "publisher used a pre-lock read");
}

/// Pool-starvation guard: with every publish slot taken, further publishers
/// queue on the semaphore *without* checking out a pool connection. Without
/// the slots each would pin its lock connection while waiting on the
/// advisory lock (held here), draining the pool under a reconnect storm.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn profile_publishers_queue_without_holding_connections() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    let user = new_user(&inst).await;
    let principal = buzz_core::principal::PrincipalId::from_hex(&user.principal).expect("id");
    let community_id: uuid::Uuid = sqlx::query_scalar("SELECT id FROM communities WHERE host = $1")
        .bind(&inst.host)
        .fetch_one(&pool)
        .await
        .expect("community");
    let community = buzz_core::CommunityId::from_uuid(community_id);
    let tenant = buzz_core::tenant::TenantContext::resolved(community, inst.host.clone());

    let relay_pool = inst.state.db.pool().clone();
    let slots = inst
        .state
        .identity
        .publish_slots
        .semaphore(relay_pool.options().get_max_connections());
    let all = u32::try_from(slots.available_permits()).expect("permits");
    let held_slots = Arc::clone(&slots)
        .acquire_many_owned(all)
        .await
        .expect("slots");
    // Also hold the advisory lock, so an unthrottled publisher would park on
    // it with its lock connection checked out.
    let (held_lock, _) = inst
        .state
        .db
        .lock_principal_profile_publish(community, &principal)
        .await
        .expect("lock");

    let in_use = |pool: &sqlx::PgPool| pool.size() as usize - pool.num_idle();
    let before = in_use(&relay_pool);
    let publishers: Vec<_> = (0..3)
        .map(|_| {
            let state = Arc::clone(&inst.state);
            let tenant = tenant.clone();
            tokio::spawn(async move {
                crate::identity::profile::publish_profile_in_community(
                    &state,
                    &tenant,
                    &principal,
                    crate::identity::profile::Scope::Participant,
                )
                .await
            })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let during = in_use(&relay_pool);
    assert!(publishers.iter().all(|p| !p.is_finished()));
    assert!(
        during < before + 3,
        "queued publishers must not pin connections: {before} -> {during}"
    );

    held_lock.release().await.expect("release lock");
    drop(held_slots);
    for publisher in publishers {
        publisher.await.expect("join").expect("publish");
    }
}
