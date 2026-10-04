//! Token operator grants must preserve a usable human roster.
use super::*;

#[tokio::test]
#[ignore = "requires Postgres + Redis — owns global operator roster"]
async fn operators_reject_unusable_targets_and_preserve_last_human() {
    let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
        .await
        .unwrap();
    sqlx::query("DELETE FROM relay_operators")
        .execute(&pool)
        .await
        .unwrap();
    let inst = instance_with(&community(&pool).await, true, Some("operator@test.invalid")).await;
    let operator = login(
        &inst,
        &uuid::Uuid::new_v4().to_string(),
        "operator@test.invalid",
        true,
    )
    .await;
    let bot = create_bot(&inst, &operator, "this_device").await;
    let disabled = new_user(&inst).await;
    let disabled_id = buzz_core::principal::PrincipalId::from_hex(&disabled.principal).unwrap();
    inst.state
        .db
        .disable_principal(&disabled_id, None)
        .await
        .unwrap();
    let relay = inst.state.identity.relay_principal().unwrap().to_hex();
    for target in [&bot, &disabled.principal, &relay] {
        let response = inst
            .http
            .put(inst.url(&format!("/auth/operators/{target}")))
            .header("host", &inst.host)
            .bearer_auth(&operator.access)
            .json(&json!({"role":"operator"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "unusable target {target}");
        // Simulate a roster left by an older version, including unknown legacy keys.
        sqlx::query(
            "INSERT INTO relay_operators (pubkey, role, added_by) VALUES ($1, 'operator', $2)",
        )
        .bind(hex::decode(target).unwrap())
        .bind(hex::decode(&operator.principal).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    }
    let legacy = buzz_core::principal::PrincipalId::generate();
    sqlx::query("INSERT INTO relay_operators (pubkey, role, added_by) VALUES ($1, 'operator', $1)")
        .bind(legacy.as_bytes().as_slice())
        .execute(&pool)
        .await
        .unwrap();
    let response = inst
        .delete(&format!("/auth/operators/{}", operator.principal))
        .bearer_auth(&operator.access)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        409,
        "unusable roster entries cannot replace the last human"
    );
    let response = inst
        .http
        .put(inst.url(&format!("/auth/operators/{}", operator.principal)))
        .header("host", &inst.host)
        .bearer_auth(&operator.access)
        .json(&json!({"role":"moderator"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        409,
        "demotion also preserves a usable operator"
    );
    // An externally disabled last operator must not permanently block recovery.
    let operator_id = buzz_core::principal::PrincipalId::from_hex(&operator.principal).unwrap();
    inst.state
        .db
        .disable_principal(&operator_id, None)
        .await
        .unwrap();
    let recovery = login(
        &inst,
        &uuid::Uuid::new_v4().to_string(),
        "operator@test.invalid",
        true,
    )
    .await;
    assert_eq!(
        inst.get("/auth/operators")
            .bearer_auth(&recovery.access)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    sqlx::query("DELETE FROM relay_operators")
        .execute(&pool)
        .await
        .unwrap();
}
