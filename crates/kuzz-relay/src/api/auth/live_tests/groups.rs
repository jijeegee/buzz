//! Live groups surface: `/api/groups/*` search, public addresses and invite
//! links, `/api/me/group-add-policy`, `/api/group-invitations/*`, and the
//! kind:9000 "who can add me" rule over real WebSockets.

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

async fn get(inst: &Instance, who: &Login, path: &str) -> (u16, Value) {
    status(
        inst.get(path)
            .bearer_auth(&who.access)
            .send()
            .await
            .unwrap(),
    )
    .await
}

async fn post(inst: &Instance, who: &Login, path: &str) -> (u16, Value) {
    status(
        inst.post(path)
            .bearer_auth(&who.access)
            .send()
            .await
            .unwrap(),
    )
    .await
}

async fn put(inst: &Instance, who: &Login, path: &str, body: Value) -> (u16, Value) {
    status(
        inst.http
            .put(inst.url(path))
            .header("host", &inst.host)
            .bearer_auth(&who.access)
            .json(&body)
            .send()
            .await
            .unwrap(),
    )
    .await
}

async fn search(inst: &Instance, who: &Login, q: &str) -> (u16, Value) {
    status(
        inst.get_with("/api/groups/search", &[("q", q)])
            .bearer_auth(&who.access)
            .send()
            .await
            .unwrap(),
    )
    .await
}

async fn group(
    pool: &sqlx::PgPool,
    scope: kuzz_core::CommunityId,
    owner: &Login,
    name: &str,
    visibility: kuzz_db::channel::ChannelVisibility,
) -> uuid::Uuid {
    kuzz_db::channel::create_channel(
        pool,
        scope,
        name,
        kuzz_db::channel::ChannelType::Stream,
        visibility,
        None,
        &key(&owner.principal),
        None,
    )
    .await
    .unwrap()
    .id
}

async fn add(
    pool: &sqlx::PgPool,
    scope: kuzz_core::CommunityId,
    room: uuid::Uuid,
    who: &Login,
    role: kuzz_db::channel::MemberRole,
) {
    // The creator (an active owner) invites, so private groups accept it.
    let owner: Vec<u8> =
        sqlx::query_scalar("SELECT created_by FROM channels WHERE community_id = $1 AND id = $2")
            .bind(scope.as_uuid())
            .bind(room)
            .fetch_one(pool)
            .await
            .unwrap();
    kuzz_db::channel_members::add_member(
        pool,
        scope,
        room,
        &key(&who.principal),
        role,
        Some(&owner),
    )
    .await
    .unwrap();
}

async fn is_member(
    pool: &sqlx::PgPool,
    scope: kuzz_core::CommunityId,
    room: uuid::Uuid,
    who: &str,
) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM channel_members WHERE community_id = $1 \
         AND channel_id = $2 AND pubkey = $3 AND removed_at IS NULL)",
    )
    .bind(scope.as_uuid())
    .bind(room)
    .bind(key(who))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn authed_ws(inst: &Instance, who: &Login) -> Ws {
    let mut ws = inst.ws().await;
    assert_eq!(auth_ws(&mut ws, &who.access).await[2], true);
    ws
}

/// Publish a draft over `ws` and return its `OK` frame.
async fn publish(ws: &mut Ws, kind: u32, tags: Value) -> Value {
    send(
        ws,
        json!(["EVENT", {"kind": kind, "content": "", "tags": tags}]),
    )
    .await;
    next_ok(ws).await
}

/// kind:9000 PUT_USER of `target` into `room`.
async fn put_user(ws: &mut Ws, room: uuid::Uuid, target: &str) -> Value {
    publish(ws, 9000, json!([["h", room.to_string()], ["p", target]])).await
}

fn ok_message(ok: &Value) -> &str {
    ok[3].as_str().unwrap_or_default()
}

/// S2: handles (validation, reserved, taken, owner/admin only, open only),
/// search (filtering, exact-first ordering, length limits), by-handle, and
/// the handle cleared when the group turns private.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn search_handles_and_private_clears_handle() {
    use kuzz_db::channel::{ChannelVisibility, MemberRole};
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let scope = community_id(&inst).await;
    let alice = new_user(&inst).await;
    let bob = new_user(&inst).await;
    let carol = new_user(&inst).await;

    let crew = group(&pool, scope, &alice, "Design Crew", ChannelVisibility::Open).await;
    let exact = group(&pool, scope, &bob, "design", ChannelVisibility::Open).await;
    let hidden = group(
        &pool,
        scope,
        &alice,
        "Design Secret",
        ChannelVisibility::Private,
    )
    .await;
    add(&pool, scope, crew, &bob, MemberRole::Member).await;

    // Address rules.
    let handle_path = format!("/api/groups/{crew}/handle");
    let (code, body) = put(
        &inst,
        &alice,
        &handle_path,
        json!({"handle": " @Design_Crew "}),
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["handle"], "design_crew");
    assert_eq!(body["memberCount"], 2);
    for bad in ["ab", "has space", "dash-name", "admin", "invite"] {
        let (code, _) = put(&inst, &alice, &handle_path, json!({"handle": bad})).await;
        assert_eq!(code, 400, "{bad}");
    }
    assert_eq!(
        put(&inst, &alice, &handle_path, json!({"handle": 5}))
            .await
            .0,
        400
    );
    let (code, body) = put(
        &inst,
        &bob,
        &format!("/api/groups/{exact}/handle"),
        json!({"handle": "DESIGN_CREW"}),
    )
    .await;
    assert_eq!(code, 409, "{body}");
    // A plain member and an outsider may not set it.
    assert_eq!(
        put(&inst, &bob, &handle_path, json!({"handle": "x_crew"}))
            .await
            .0,
        403
    );
    assert_eq!(
        put(&inst, &carol, &handle_path, json!({"handle": "x_crew"}))
            .await
            .0,
        403
    );
    // Private groups cannot have one.
    let (code, _) = put(
        &inst,
        &alice,
        &format!("/api/groups/{hidden}/handle"),
        json!({"handle": "secret_room"}),
    )
    .await;
    assert_eq!(code, 400);

    // Search: open groups only, exact match first, then member count.
    let (code, body) = search(&inst, &carol, "design").await;
    assert_eq!(code, 200, "{body}");
    let found = body["groups"].as_array().unwrap();
    let ids: Vec<&str> = found.iter().map(|g| g["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![exact.to_string(), crew.to_string()]);
    assert_eq!(found[1]["handle"], "design_crew");
    assert_eq!(found[1]["channelType"], "stream");
    assert_eq!(found[1]["memberCount"], 2);
    assert_eq!(found[1]["isMember"], false);
    let (_, body) = search(&inst, &bob, "crew").await;
    assert_eq!(body["groups"][0]["isMember"], true);
    assert_eq!(search(&inst, &carol, "d").await.0, 400);
    assert_eq!(search(&inst, &carol, &"d".repeat(65)).await.0, 400);
    let (code, _) = status(
        inst.get_with("/api/groups/search", &[("q", "design"), ("limit", "0")])
            .bearer_auth(&carol.access)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(code, 400);
    assert_eq!(
        inst.get("/api/groups/search?q=design")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    // By address, and the group view.
    let (code, body) = get(&inst, &carol, "/api/groups/by-handle/@Design_Crew").await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["id"], crew.to_string());
    assert_eq!(
        get(&inst, &carol, "/api/groups/by-handle/nobody_here")
            .await
            .0,
        404
    );
    let (code, body) = get(&inst, &carol, &format!("/api/groups/{crew}")).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["visibility"], "open");
    assert_eq!(body["invitePolicy"], "members");
    assert_eq!(
        get(&inst, &carol, &format!("/api/groups/{hidden}")).await.0,
        404
    );

    // Turning the group private (kind:9002) clears its address.
    let mut alice_ws = authed_ws(&inst, &alice).await;
    let ok = publish(
        &mut alice_ws,
        9002,
        json!([["h", crew.to_string()], ["visibility", "private"]]),
    )
    .await;
    assert_eq!(ok[2], true, "{ok}");
    let handle: Option<String> =
        sqlx::query_scalar("SELECT handle FROM channels WHERE community_id = $1 AND id = $2")
            .bind(scope.as_uuid())
            .bind(crew)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(handle, None);
    assert_eq!(
        get(&inst, &carol, "/api/groups/by-handle/design_crew")
            .await
            .0,
        404
    );
    let (_, body) = search(&inst, &carol, "crew").await;
    assert!(body["groups"].as_array().unwrap().is_empty(), "{body}");
    // The freed address can be taken again; clearing with null works.
    let exact_handle = format!("/api/groups/{exact}/handle");
    assert_eq!(
        put(&inst, &bob, &exact_handle, json!({"handle": "design_crew"}))
            .await
            .0,
        200
    );
    let (code, body) = put(&inst, &bob, &exact_handle, json!({"handle": null})).await;
    assert_eq!(code, 200);
    assert_eq!(body["handle"], Value::Null);
}

/// S3 invite links: create-on-first-call, preview and join (private group,
/// idempotent), reset invalidates the old code, the admins policy, and the
/// refusals (banned, removed by an admin, agents, unknown code).
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn invite_links_preview_join_reset_and_policy() {
    use kuzz_db::channel::{ChannelVisibility, MemberRole};
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let scope = community_id(&inst).await;
    let alice = new_user(&inst).await;
    let bob = new_user(&inst).await;
    let carol = new_user(&inst).await;
    let dave = new_user(&inst).await;
    let erin = new_user(&inst).await;
    let room = group(
        &pool,
        scope,
        &alice,
        "Book Club",
        ChannelVisibility::Private,
    )
    .await;
    add(&pool, scope, room, &bob, MemberRole::Member).await;

    // Non-members of a private group: 404.
    let link_path = format!("/api/groups/{room}/invite-link");
    assert_eq!(get(&inst, &carol, &link_path).await.0, 404);
    // A member gets the link; the second call returns the same one.
    let (code, link) = get(&inst, &bob, &link_path).await;
    assert_eq!(code, 200, "{link}");
    let code_a = link["code"].as_str().unwrap().to_owned();
    assert!(code_a.len() >= 22, "{code_a}");
    assert_eq!(
        link["url"],
        format!("http://{}/g/{code_a}", inst.host),
        "url uses the community host"
    );
    assert_eq!(
        get(&inst, &bob, &link_path).await.1["code"],
        code_a.as_str()
    );
    assert_eq!(
        get(&inst, &alice, &link_path).await.1["code"],
        code_a.as_str()
    );

    // Preview works for a non-member of the private group.
    let (code, preview) = get(&inst, &carol, &format!("/api/groups/invite/{code_a}")).await;
    assert_eq!(code, 200, "{preview}");
    assert_eq!(preview["id"], room.to_string());
    assert_eq!(preview["name"], "Book Club");
    assert_eq!(preview["visibility"], "private");
    assert_eq!(preview["memberCount"], 2);
    assert_eq!(preview["isMember"], false);
    assert_eq!(preview["channelType"], "stream");

    // Join, then join again (idempotent).
    let join_a = format!("/api/groups/invite/{code_a}/join");
    let (code, body) = post(&inst, &carol, &join_a).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body, json!({"channelId": room.to_string(), "joined": true}));
    assert!(is_member(&pool, scope, room, &carol.principal).await);
    let (code, body) = post(&inst, &carol, &join_a).await;
    assert_eq!(code, 200);
    assert_eq!(body["joined"], false);
    assert_eq!(
        get(&inst, &carol, &format!("/api/groups/invite/{code_a}"))
            .await
            .1["isMember"],
        true
    );

    // Only owners/admins reset; reset kills the old code.
    let reset = format!("/api/groups/{room}/invite-link/reset");
    assert_eq!(post(&inst, &bob, &reset).await.0, 403);
    let (code, fresh) = post(&inst, &alice, &reset).await;
    assert_eq!(code, 200, "{fresh}");
    let code_b = fresh["code"].as_str().unwrap().to_owned();
    assert_ne!(code_a, code_b);
    assert_eq!(
        get(&inst, &dave, &format!("/api/groups/invite/{code_a}"))
            .await
            .0,
        404
    );
    assert_eq!(post(&inst, &dave, &join_a).await.0, 404);
    assert_eq!(
        get(&inst, &bob, &link_path).await.1["code"],
        code_b.as_str()
    );
    assert_eq!(
        get(&inst, &dave, "/api/groups/invite/not-a-real-code-at-all-xx")
            .await
            .0,
        404
    );

    // Invite policy: owner/admin only; admins-only hides the link from members.
    let policy_path = format!("/api/groups/{room}/invite-policy");
    assert_eq!(get(&inst, &bob, &policy_path).await.1["policy"], "members");
    assert_eq!(
        put(&inst, &bob, &policy_path, json!({"policy": "admins"}))
            .await
            .0,
        403
    );
    assert_eq!(
        put(&inst, &alice, &policy_path, json!({"policy": "nobody"}))
            .await
            .0,
        400
    );
    let (code, body) = put(&inst, &alice, &policy_path, json!({"policy": "admins"})).await;
    assert_eq!((code, body), (200, json!({"policy": "admins"})));
    assert_eq!(get(&inst, &bob, &policy_path).await.1["policy"], "admins");
    assert_eq!(get(&inst, &bob, &link_path).await.0, 403);
    // The owner's link keeps working under the admins policy.
    let join_b = format!("/api/groups/invite/{code_b}/join");
    let (code, body) = post(&inst, &dave, &join_b).await;
    assert_eq!((code, body["joined"].clone()), (200, json!(true)));

    // Removed by an admin: no way back through the link.
    let mut alice_ws = authed_ws(&inst, &alice).await;
    let ok = publish(
        &mut alice_ws,
        9001,
        json!([["h", room.to_string()], ["p", dave.principal]]),
    )
    .await;
    assert_eq!(ok[2], true, "{ok}");
    assert_eq!(post(&inst, &dave, &join_b).await.0, 403);

    // Community-banned people are refused at authentication.
    sqlx::query(
        "INSERT INTO community_bans (community_id, pubkey, banned, actor_pubkey) \
         VALUES ($1, $2, true, $3)",
    )
    .bind(scope.as_uuid())
    .bind(key(&erin.principal))
    .bind(key(&alice.principal))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(post(&inst, &erin, &join_b).await.0, 403);
    assert!(!is_member(&pool, scope, room, &erin.principal).await);
}

/// S3 "who can add me": friend vs stranger (invite-sent + invitation),
/// accept/decline, `everyone`, agents untouched, the admins invite policy,
/// and accept re-checking the inviter.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn put_user_respects_who_can_add_me_and_invitations() {
    use kuzz_db::channel::ChannelVisibility;
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let scope = community_id(&inst).await;
    let alice = new_user(&inst).await;
    let bob = new_user(&inst).await;
    let carol = new_user(&inst).await;
    let dave = new_user(&inst).await;
    let erin = new_user(&inst).await;
    let frank = new_user(&inst).await;
    let george = new_user(&inst).await;
    let room = group(&pool, scope, &alice, "Hikers", ChannelVisibility::Open).await;
    let mut alice_ws = authed_ws(&inst, &alice).await;

    // Default policy is friends.
    assert_eq!(
        get(&inst, &bob, "/api/me/group-add-policy").await.1["policy"],
        "friends"
    );

    // Stranger: not added, an invitation instead.
    let ok = put_user(&mut alice_ws, room, &bob.principal).await;
    assert_eq!(ok[2], false, "{ok}");
    assert!(ok_message(&ok).starts_with("invite-sent:"), "{ok}");
    assert!(!is_member(&pool, scope, room, &bob.principal).await);
    // Adding again refreshes the same invitation.
    assert_eq!(
        put_user(&mut alice_ws, room, &bob.principal).await[2],
        false
    );
    let (code, list) = get(&inst, &bob, "/api/group-invitations").await;
    assert_eq!(code, 200, "{list}");
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["channel"]["id"], room.to_string());
    assert_eq!(list[0]["channel"]["name"], "Hikers");
    assert_eq!(list[0]["channel"]["visibility"], "open");
    assert_eq!(list[0]["channel"]["memberCount"], 1);
    assert_eq!(list[0]["inviter"], alice.principal.as_str());
    assert!(list[0]["createdAt"].as_str().unwrap().ends_with('Z'));
    let invitation = list[0]["id"].as_str().unwrap().to_owned();
    // Only the invitee can answer it.
    assert_eq!(
        post(
            &inst,
            &carol,
            &format!("/api/group-invitations/{invitation}/accept")
        )
        .await
        .0,
        404
    );
    let (code, body) = post(
        &inst,
        &bob,
        &format!("/api/group-invitations/{invitation}/accept"),
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body, json!({"channelId": room.to_string(), "joined": true}));
    assert!(is_member(&pool, scope, room, &bob.principal).await);
    assert!(get(&inst, &bob, "/api/group-invitations")
        .await
        .1
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        post(
            &inst,
            &bob,
            &format!("/api/group-invitations/{invitation}/accept")
        )
        .await
        .0,
        404
    );

    // Friend (the target lists the actor in kind:3): added directly.
    let mut carol_ws = authed_ws(&inst, &carol).await;
    let ok = publish(&mut carol_ws, 3, json!([["p", alice.principal]])).await;
    assert_eq!(ok[2], true, "{ok}");
    let ok = put_user(&mut alice_ws, room, &carol.principal).await;
    assert_eq!(ok[2], true, "{ok}");
    assert!(is_member(&pool, scope, room, &carol.principal).await);

    // `everyone`: added directly.
    let (code, body) = put(
        &inst,
        &dave,
        "/api/me/group-add-policy",
        json!({"policy": "everyone"}),
    )
    .await;
    assert_eq!((code, body), (200, json!({"policy": "everyone"})));
    assert_eq!(
        get(&inst, &dave, "/api/me/group-add-policy").await.1["policy"],
        "everyone"
    );
    assert_eq!(
        put(
            &inst,
            &dave,
            "/api/me/group-add-policy",
            json!({"policy": "nobody"})
        )
        .await
        .0,
        400
    );
    let ok = put_user(&mut alice_ws, room, &dave.principal).await;
    assert_eq!(ok[2], true, "{ok}");

    // Decline.
    assert_eq!(
        put_user(&mut alice_ws, room, &erin.principal).await[2],
        false
    );
    let list = get(&inst, &erin, "/api/group-invitations").await.1;
    let invitation = list[0]["id"].as_str().unwrap().to_owned();
    let (code, body) = post(
        &inst,
        &erin,
        &format!("/api/group-invitations/{invitation}/decline"),
    )
    .await;
    assert_eq!((code, body), (200, json!({"status": "declined"})));
    assert!(get(&inst, &erin, "/api/group-invitations")
        .await
        .1
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        post(
            &inst,
            &erin,
            &format!("/api/group-invitations/{invitation}/accept")
        )
        .await
        .0,
        404
    );
    assert!(!is_member(&pool, scope, room, &erin.principal).await);

    // Agents keep their own add policy: the owner adds their agent directly.
    let bot = create_bot(&inst, &alice, "headless").await;
    let ok = put_user(&mut alice_ws, room, &bot).await;
    assert_eq!(ok[2], true, "{ok}");
    assert!(is_member(&pool, scope, room, &bot).await);

    // A member (bob) invites george while every member may add people.
    let mut bob_ws = authed_ws(&inst, &bob).await;
    let ok = put_user(&mut bob_ws, room, &george.principal).await;
    assert!(ok_message(&ok).starts_with("invite-sent:"), "{ok}");
    let george_invite = get(&inst, &george, "/api/group-invitations").await.1[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Admins-only: a plain member is denied outright even for `everyone`
    // targets; the owner still adds.
    let policy_path = format!("/api/groups/{room}/invite-policy");
    assert_eq!(
        put(&inst, &alice, &policy_path, json!({"policy": "admins"}))
            .await
            .0,
        200
    );
    assert_eq!(
        put(
            &inst,
            &frank,
            "/api/me/group-add-policy",
            json!({"policy": "everyone"})
        )
        .await
        .0,
        200
    );
    let ok = put_user(&mut bob_ws, room, &frank.principal).await;
    assert_eq!(ok[2], false, "{ok}");
    assert!(ok_message(&ok).starts_with("invalid:"), "{ok}");
    assert!(!is_member(&pool, scope, room, &frank.principal).await);
    assert_eq!(
        put_user(&mut alice_ws, room, &frank.principal).await[2],
        true
    );

    // George's invitation came from a member who may no longer add: 409.
    let (code, body) = post(
        &inst,
        &george,
        &format!("/api/group-invitations/{george_invite}/accept"),
    )
    .await;
    assert_eq!(code, 409, "{body}");
    assert!(!is_member(&pool, scope, room, &george.principal).await);
    assert!(get(&inst, &george, "/api/group-invitations")
        .await
        .1
        .as_array()
        .unwrap()
        .is_empty());

    // Role changes of existing members are unaffected by the rule.
    let ok = publish(
        &mut alice_ws,
        9000,
        json!([
            ["h", room.to_string()],
            ["p", bob.principal],
            ["role", "admin"]
        ]),
    )
    .await;
    assert_eq!(ok[2], true, "{ok}");
}

/// An agent adds its own owner directly (no invitation), and the invitation
/// spam limits: a recent decline suppresses re-invites for 7 days, and one
/// inviter may create at most 50 invitations per community per 24 hours.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn agent_adds_owner_and_invitation_spam_limits() {
    use kuzz_db::channel::ChannelVisibility;
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let scope = community_id(&inst).await;
    let alice = new_user(&inst).await;
    let erin = new_user(&inst).await;
    let zed = new_user(&inst).await;

    // The agent's own room; adding its owner (default `friends`, and the
    // owner never listed the agent in kind:3) goes straight through.
    let bot = create_bot(&inst, &alice, "this_device").await;
    let token = bot_token(&inst, &alice, &bot).await;
    let mut bot_ws = inst.ws().await;
    assert_eq!(auth_ws(&mut bot_ws, &token).await[2], true);
    let bot_room = kuzz_db::channel::create_channel(
        &pool,
        scope,
        "agent room",
        kuzz_db::channel::ChannelType::Stream,
        ChannelVisibility::Open,
        None,
        &key(&bot),
        None,
    )
    .await
    .unwrap()
    .id;
    let ok = put_user(&mut bot_ws, bot_room, &alice.principal).await;
    assert_eq!(ok[2], true, "{ok}");
    assert!(is_member(&pool, scope, bot_room, &alice.principal).await);
    // Someone else's person is still protected from the agent.
    let ok = put_user(&mut bot_ws, bot_room, &erin.principal).await;
    assert!(ok_message(&ok).starts_with("invite-sent:"), "{ok}");
    assert!(!is_member(&pool, scope, bot_room, &erin.principal).await);

    // As a plain member of someone else's group the agent cannot pull its
    // owner in: a stranger could otherwise add the agent and have it add V.
    let mallory = new_user(&inst).await;
    let trap = group(&pool, scope, &mallory, "Trap", ChannelVisibility::Open).await;
    kuzz_db::channel_members::add_member(
        &pool,
        scope,
        trap,
        &key(&bot),
        kuzz_db::channel::MemberRole::Member,
        Some(&key(&mallory.principal)),
    )
    .await
    .unwrap();
    let ok = put_user(&mut bot_ws, trap, &alice.principal).await;
    assert!(ok_message(&ok).starts_with("invite-sent:"), "{ok}");
    assert!(!is_member(&pool, scope, trap, &alice.principal).await);

    // A block wins over `everyone`: not added, silent invite-sent, no row.
    let mut mallory_ws = authed_ws(&inst, &mallory).await;
    assert_eq!(
        put(
            &inst,
            &zed,
            "/api/me/group-add-policy",
            json!({"policy": "everyone"})
        )
        .await
        .0,
        200
    );
    let blocked = inst
        .http
        .put(inst.url(&format!("/auth/blocks/{}", mallory.principal)))
        .header("host", &inst.host)
        .bearer_auth(&zed.access)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(blocked, 204);
    let ok = put_user(&mut mallory_ws, trap, &zed.principal).await;
    assert_eq!(ok[2], false, "{ok}");
    assert!(ok_message(&ok).starts_with("invite-sent:"), "{ok}");
    assert!(!is_member(&pool, scope, trap, &zed.principal).await);
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM group_invitations WHERE community_id = $1 AND invitee = $2",
    )
    .bind(scope.as_uuid())
    .bind(key(&zed.principal))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 0);

    // Decline cooldown.
    let room = group(&pool, scope, &alice, "Runners", ChannelVisibility::Open).await;
    let mut alice_ws = authed_ws(&inst, &alice).await;
    assert_eq!(
        put_user(&mut alice_ws, room, &erin.principal).await[2],
        false
    );
    let pending = |who: &Login| {
        let who = who.access.clone();
        let inst = &inst;
        async move {
            let response = inst
                .get("/api/group-invitations")
                .bearer_auth(who)
                .send()
                .await
                .unwrap();
            response.json::<Value>().await.unwrap()
        }
    };
    let list = pending(&erin).await;
    let mine: Vec<&Value> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["channel"]["id"] == room.to_string())
        .collect();
    let invitation = mine[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        post(
            &inst,
            &erin,
            &format!("/api/group-invitations/{invitation}/decline")
        )
        .await
        .0,
        200
    );
    let ok = put_user(&mut alice_ws, room, &erin.principal).await;
    assert!(ok_message(&ok).starts_with("invite-sent:"), "{ok}");
    let count_for_room = |list: &Value| {
        list.as_array()
            .unwrap()
            .iter()
            .filter(|i| i["channel"]["id"] == room.to_string())
            .count()
    };
    assert_eq!(
        count_for_room(&pending(&erin).await),
        0,
        "no re-invite spam"
    );
    sqlx::query(
        "UPDATE group_invitations SET responded_at = now() - interval '8 days' \
         WHERE community_id = $1 AND id = $2",
    )
    .bind(scope.as_uuid())
    .bind(uuid::Uuid::parse_str(&invitation).unwrap())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        put_user(&mut alice_ws, room, &erin.principal).await[2],
        false
    );
    assert_eq!(
        count_for_room(&pending(&erin).await),
        1,
        "allowed after 7 days"
    );

    // Daily cap: fill alice's 24h budget to exactly 50.
    let yan = new_user(&inst).await;
    let sent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM group_invitations WHERE community_id = $1 AND inviter = $2 \
         AND created_at > now() - interval '24 hours'",
    )
    .bind(scope.as_uuid())
    .bind(key(&alice.principal))
    .fetch_one(&pool)
    .await
    .unwrap();
    for _ in sent..kuzz_db::groups::INVITATIONS_PER_DAY {
        sqlx::query(
            "INSERT INTO group_invitations (community_id, channel_id, inviter, invitee) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(scope.as_uuid())
        .bind(room)
        .bind(key(&alice.principal))
        .bind(nostr::Keys::generate().public_key().to_bytes().to_vec())
        .execute(&pool)
        .await
        .unwrap();
    }
    let ok = put_user(&mut alice_ws, room, &yan.principal).await;
    assert_eq!(ok[2], false, "{ok}");
    assert_eq!(
        ok_message(&ok),
        "rate-limited: too many invitations, try later"
    );
    assert!(pending(&yan).await.as_array().unwrap().is_empty());
    // Refreshing an existing pending invitation is not a new one.
    let ok = put_user(&mut alice_ws, room, &erin.principal).await;
    assert!(ok_message(&ok).starts_with("invite-sent:"), "{ok}");
}

/// The public `/g/<code>` and `/g/@<handle>` landing page: no auth, HTML,
/// escaped user text, private groups without description, 404 page for
/// unknown and reset codes.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn landing_page_for_codes_and_handles() {
    use kuzz_db::channel::ChannelVisibility;
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let scope = community_id(&inst).await;
    let alice = new_user(&inst).await;
    let open = group(
        &pool,
        scope,
        &alice,
        "<b>Bold</b> & \"Co\"",
        ChannelVisibility::Open,
    )
    .await;
    sqlx::query("UPDATE channels SET description = $3 WHERE community_id = $1 AND id = $2")
        .bind(scope.as_uuid())
        .bind(open)
        .bind("<img src=x onerror=alert(1)> hikes")
        .execute(&pool)
        .await
        .unwrap();
    let (code, _) = put(
        &inst,
        &alice,
        &format!("/api/groups/{open}/handle"),
        json!({"handle": "landing_crew"}),
    )
    .await;
    assert_eq!(code, 200);
    let open_code = get(&inst, &alice, &format!("/api/groups/{open}/invite-link"))
        .await
        .1["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let secret = group(
        &pool,
        scope,
        &alice,
        "Secret Room",
        ChannelVisibility::Private,
    )
    .await;
    sqlx::query(
        "UPDATE channels SET description = 'hidden plans' WHERE community_id = $1 AND id = $2",
    )
    .bind(scope.as_uuid())
    .bind(secret)
    .execute(&pool)
    .await
    .unwrap();
    let secret_code = get(&inst, &alice, &format!("/api/groups/{secret}/invite-link"))
        .await
        .1["code"]
        .as_str()
        .unwrap()
        .to_owned();

    let page = |path: String| {
        let inst = &inst;
        async move {
            let response = inst.get(&path).send().await.unwrap();
            let status = response.status().as_u16();
            let content_type = response
                .headers()
                .get("content-type")
                .map(|v| v.to_str().unwrap().to_owned())
                .unwrap_or_default();
            let robots = response
                .headers()
                .get("x-robots-tag")
                .map(|v| v.to_str().unwrap().to_owned())
                .unwrap_or_default();
            (status, content_type, robots, response.text().await.unwrap())
        }
    };

    let (status, content_type, robots, body) = page(format!("/g/{open_code}")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert!(robots.contains("noindex"));
    assert!(
        body.contains("&lt;b&gt;Bold&lt;/b&gt; &amp; &quot;Co&quot;"),
        "{body}"
    );
    assert!(!body.contains("<b>Bold"), "{body}");
    assert!(!body.contains("<img"), "{body}");
    assert!(body.contains("&lt;img src=x onerror=alert(1)&gt; hikes"));
    assert!(body.contains("1 member<"));
    assert!(body.contains(&format!("href=\"kuzz://join/{open_code}\"")));
    assert!(body.contains("Don&#39;t have Kuzz yet? Ask the person who shared this link."));

    let (status, _, _, body) = page("/g/@Landing_Crew".to_owned()).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("href=\"kuzz://join/@landing_crew\""),
        "{body}"
    );
    assert!(body.contains("hikes"));

    let (status, _, _, body) = page(format!("/g/{secret_code}")).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Secret Room"));
    assert!(!body.contains("hidden plans"), "private: name + count only");
    // Private groups have no public address page.
    assert_eq!(page("/g/@secret_room".to_owned()).await.0, 404);

    let (status, content_type, _, body) = page("/g/not-a-real-code-at-all-xx".to_owned()).await;
    assert_eq!(status, 404);
    assert_eq!(content_type, "text/html; charset=utf-8");
    assert!(body.contains("This link doesn&#39;t work"));
    assert_eq!(page("/g/@nobody_here".to_owned()).await.0, 404);
    assert_eq!(
        post(
            &inst,
            &alice,
            &format!("/api/groups/{open}/invite-link/reset")
        )
        .await
        .0,
        200
    );
    assert_eq!(
        page(format!("/g/{open_code}")).await.0,
        404,
        "reset kills it"
    );
}

/// Poll `sql` (bound to the community and `arg`) until it returns true.
async fn eventually(pool: &sqlx::PgPool, scope: kuzz_core::CommunityId, sql: &str, arg: &str) {
    for _ in 0..50 {
        let ok: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .bind(scope.as_uuid())
            .bind(arg)
            .fetch_one(pool)
            .await
            .unwrap();
        if ok {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("never true: {sql} ({arg})");
}

/// S4: group photo via kind:9002 `picture` — owner/admin only, URL rules,
/// stored and shown in kind:39000, the groups API, invitations and the
/// landing page; removal clears it everywhere.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn group_photo_set_shown_and_removed() {
    use kuzz_db::channel::{ChannelVisibility, MemberRole};
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let scope = community_id(&inst).await;
    let alice = new_user(&inst).await;
    let bob = new_user(&inst).await;
    let carol = new_user(&inst).await;
    let room = group(
        &pool,
        scope,
        &alice,
        "Photo Club",
        ChannelVisibility::Private,
    )
    .await;
    add(&pool, scope, room, &bob, MemberRole::Member).await;
    let hash = "ab".repeat(32);
    let url = format!("http://{}/media/{hash}.jpg", inst.host);
    let h = room.to_string();

    // A plain member may not change it.
    let mut bob_ws = authed_ws(&inst, &bob).await;
    let ok = publish(&mut bob_ws, 9002, json!([["h", h], ["picture", url]])).await;
    assert_eq!(ok[2], false, "{ok}");
    assert!(ok_message(&ok).contains("not authorized"), "{ok}");

    // Only blob URLs on this relay.
    let mut alice_ws = authed_ws(&inst, &alice).await;
    for bad in [
        format!("https://evil.example/media/{hash}.jpg"),
        format!("http://{}/media/{hash}.jpg?x=1", inst.host),
        format!("http://{}/media/not-a-hash.jpg", inst.host),
        format!("http://{}/media/{hash}.{}", inst.host, "a".repeat(600)),
    ] {
        let ok = publish(&mut alice_ws, 9002, json!([["h", h], ["picture", bad]])).await;
        assert_eq!(ok[2], false, "{bad}: {ok}");
        assert!(ok_message(&ok).contains("invalid picture"), "{ok}");
    }

    // The owner sets it.
    let ok = publish(&mut alice_ws, 9002, json!([["h", h], ["picture", url]])).await;
    assert_eq!(ok[2], true, "{ok}");
    eventually(
        &pool,
        scope,
        "SELECT EXISTS (SELECT 1 FROM channels WHERE community_id = $1 \
         AND id::text = $2 AND picture IS NOT NULL)",
        &h,
    )
    .await;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT picture FROM channels WHERE community_id = $1 AND id = $2")
            .bind(scope.as_uuid())
            .bind(room)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some(url.as_str()));
    let picture_tag = json!([["picture", url]]).to_string();
    eventually(
        &pool,
        scope,
        &format!(
            "SELECT EXISTS (SELECT 1 FROM events WHERE community_id = $1 AND kind = 39000 \
             AND deleted_at IS NULL AND tags @> jsonb_build_array(jsonb_build_array('d', $2::text)) \
             AND tags @> '{}'::jsonb)",
            picture_tag.replace('\'', "''")
        ),
        &h,
    )
    .await;
    eventually(
        &pool,
        scope,
        "SELECT EXISTS (SELECT 1 FROM events WHERE community_id = $1 AND deleted_at IS NULL \
         AND content LIKE '%\"picture_changed\"%' AND content LIKE '%' || $2 || '%')",
        &url,
    )
    .await;

    // Shown to a non-member: invite preview and the invitation list.
    let code = get(&inst, &alice, &format!("/api/groups/{room}/invite-link"))
        .await
        .1["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, preview) = get(&inst, &carol, &format!("/api/groups/invite/{code}")).await;
    assert_eq!(status, 200, "{preview}");
    assert_eq!(preview["picture"], url.as_str());
    // A stranger gets an invitation instead of being added.
    let ok = put_user(&mut alice_ws, room, &carol.principal).await;
    assert!(ok_message(&ok).starts_with("invite-sent:"), "{ok}");
    let (status, invitations) = get(&inst, &carol, "/api/group-invitations").await;
    assert_eq!(status, 200, "{invitations}");
    assert_eq!(
        invitations[0]["channel"]["picture"],
        url.as_str(),
        "{invitations}"
    );

    // Landing page shows it from the same origin; the photo route answers
    // without sign-in (404 here: the test relay stores no blob bytes).
    let response = inst.get(&format!("/g/{code}")).send().await.unwrap();
    let csp = response
        .headers()
        .get("content-security-policy")
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let body = response.text().await.unwrap();
    assert!(
        body.contains(&format!("<img class=\"photo\" src=\"/g/{code}/picture\"")),
        "{body}"
    );
    assert!(csp.contains("img-src 'self'"), "{csp}");
    let status = inst
        .get(&format!("/g/{code}/picture"))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(status, 404);

    // Removing clears it everywhere.
    let ok = publish(&mut alice_ws, 9002, json!([["h", h], ["picture", ""]])).await;
    assert_eq!(ok[2], true, "{ok}");
    eventually(
        &pool,
        scope,
        "SELECT EXISTS (SELECT 1 FROM channels WHERE community_id = $1 \
         AND id::text = $2 AND picture IS NULL)",
        &h,
    )
    .await;
    eventually(
        &pool,
        scope,
        "SELECT EXISTS (SELECT 1 FROM events WHERE community_id = $1 AND kind = 39000 \
         AND deleted_at IS NULL AND tags @> jsonb_build_array(jsonb_build_array('d', $2::text)) \
         AND NOT tags::text LIKE '%\"picture\"%')",
        &h,
    )
    .await;
    let (_, preview) = get(&inst, &carol, &format!("/api/groups/invite/{code}")).await;
    assert_eq!(preview["picture"], Value::Null);
    let body = inst
        .get(&format!("/g/{code}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!body.contains("<img"), "{body}");
}

/// Group chats have no invite link, address or invite policy. Every such
/// route refuses a group chat with its own error, ahead of the owner/admin
/// check (a group chat has no owner, so that check alone would also refuse),
/// and a link planted for a group chat neither previews nor joins, while the
/// same planted link on a managed group works.
#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn group_chats_refuse_links_addresses_and_invite_policy() {
    use kuzz_db::channel::{ChannelVisibility, MemberRole};
    const CHAT_REFUSAL: &str = "chats have no invite link, address or invite settings";
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    let inst = instance(&community(&pool).await).await;
    let scope = community_id(&inst).await;
    let alice = new_user(&inst).await;
    let bob = new_user(&inst).await;
    let carol = new_user(&inst).await;

    let chat = uuid::Uuid::new_v4();
    let (_, created) = kuzz_db::channel::create_group_chat_with_id(
        &pool,
        scope,
        chat,
        "Trip",
        &key(&alice.principal),
        &[(key(&bob.principal), MemberRole::Member)],
        &[],
    )
    .await
    .unwrap();
    assert!(created);
    let control = group(&pool, scope, &alice, "Control", ChannelVisibility::Private).await;

    // Members of the chat (the creator included) get the chat refusal, not
    // a link and not the owner/admin refusal.
    for who in [&alice, &bob] {
        let (code, body) = get(&inst, who, &format!("/api/groups/{chat}/invite-link")).await;
        assert_eq!((code, body["error"].as_str()), (400, Some(CHAT_REFUSAL)));
        let (code, body) = post(&inst, who, &format!("/api/groups/{chat}/invite-link/reset")).await;
        assert_eq!((code, body["error"].as_str()), (400, Some(CHAT_REFUSAL)));
        let (code, body) = put(
            &inst,
            who,
            &format!("/api/groups/{chat}/handle"),
            json!({"handle": "tripchat"}),
        )
        .await;
        assert_eq!((code, body["error"].as_str()), (400, Some(CHAT_REFUSAL)));
        let policy_path = format!("/api/groups/{chat}/invite-policy");
        let (code, body) = get(&inst, who, &policy_path).await;
        assert_eq!((code, body["error"].as_str()), (400, Some(CHAT_REFUSAL)));
        let (code, body) = put(&inst, who, &policy_path, json!({"policy": "admins"})).await;
        assert_eq!((code, body["error"].as_str()), (400, Some(CHAT_REFUSAL)));
    }
    // A non-member learns nothing about the private chat.
    assert_eq!(
        get(&inst, &carol, &format!("/api/groups/{chat}/invite-link"))
            .await
            .0,
        404
    );
    assert!(sqlx::query_scalar::<_, Option<String>>(
        "SELECT handle FROM channels WHERE community_id = $1 AND id = $2"
    )
    .bind(scope.as_uuid())
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap()
    .is_none());

    // Plant usable links (an active member created each) directly.
    let chat_code = "groupchatplantedcode00000001";
    let control_code = "groupchatplantedcode00000002";
    for (room, code) in [(chat, chat_code), (control, control_code)] {
        sqlx::query(
            "INSERT INTO group_invite_links (community_id, channel_id, code, created_by) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(scope.as_uuid())
        .bind(room)
        .bind(code)
        .bind(key(&alice.principal))
        .execute(&pool)
        .await
        .unwrap();
    }
    let (code, preview) = get(&inst, &carol, &format!("/api/groups/invite/{control_code}")).await;
    assert_eq!(
        code, 200,
        "the planted link works on a managed group: {preview}"
    );
    assert_eq!(preview["id"], control.to_string());

    let (code, body) = get(&inst, &carol, &format!("/api/groups/invite/{chat_code}")).await;
    assert_eq!(code, 404, "{body}");
    let (code, body) = post(
        &inst,
        &carol,
        &format!("/api/groups/invite/{chat_code}/join"),
    )
    .await;
    assert_eq!(code, 404, "{body}");
    assert!(!is_member(&pool, scope, chat, &carol.principal).await);
}
