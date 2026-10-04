//! Live invite mint/claim with Bearer tokens (plan §5.3, Phase 2): an
//! owner/admin user session mints, a user session claims and becomes a relay
//! member, and a bot token is refused.

use super::*;

async fn seed_relay_role(inst: &Instance, principal: &str, role: &str) {
    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role) \
         SELECT id, $2, $3 FROM communities WHERE host = $1",
    )
    .bind(&inst.host)
    .bind(principal)
    .bind(role)
    .execute(inst.state.db.pool())
    .await
    .expect("seed relay member");
}

async fn relay_role(inst: &Instance, principal: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT m.role FROM relay_members m JOIN communities c ON c.id = m.community_id \
         WHERE c.host = $1 AND m.pubkey = $2",
    )
    .bind(&inst.host)
    .bind(principal)
    .fetch_optional(inst.state.db.pool())
    .await
    .expect("relay role")
}

#[tokio::test]
#[ignore = "requires Postgres + Redis"]
async fn invites_mint_and_claim_with_bearer_tokens() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("pg");
    let inst = instance(&community(&pool).await).await;
    let owner = new_user(&inst).await;
    seed_relay_role(&inst, &owner.principal, "owner").await;

    // A plain member cannot mint.
    let member = new_user(&inst).await;
    seed_relay_role(&inst, &member.principal, "member").await;
    let denied = inst
        .post("/api/invites")
        .bearer_auth(&member.access)
        .json(&json!({}))
        .send()
        .await
        .expect("mint as member");
    assert_eq!(denied.status(), 403);

    let minted = inst
        .post("/api/invites")
        .bearer_auth(&owner.access)
        .json(&json!({}))
        .send()
        .await
        .expect("mint");
    assert_eq!(minted.status(), 200);
    let code = minted.json::<Value>().await.expect("json")["code"]
        .as_str()
        .expect("code")
        .to_owned();

    // A bot token cannot claim, even its owner's invite.
    let bot = create_bot(&inst, &owner, "this_device").await;
    let token = bot_token(&inst, &owner, &bot).await;
    let refused = inst
        .post("/api/invites/claim")
        .bearer_auth(&token)
        .json(&json!({"code": code}))
        .send()
        .await
        .expect("claim as bot");
    assert_eq!(refused.status(), 403);
    assert_eq!(relay_role(&inst, &bot).await, None);

    // A revoked/invalid token is a 401, not a NIP-98 fallthrough.
    let invalid = inst
        .post("/api/invites/claim")
        .bearer_auth("bzs_not-a-real-token")
        .json(&json!({"code": code}))
        .send()
        .await
        .expect("claim invalid");
    assert_eq!(invalid.status(), 401);

    let joiner = new_user(&inst).await;
    let claimed = inst
        .post("/api/invites/claim")
        .bearer_auth(&joiner.access)
        .json(&json!({"code": code}))
        .send()
        .await
        .expect("claim");
    assert_eq!(claimed.status(), 200, "{:?}", claimed.text().await);
    assert_eq!(
        relay_role(&inst, &joiner.principal).await.as_deref(),
        Some("member")
    );
}
