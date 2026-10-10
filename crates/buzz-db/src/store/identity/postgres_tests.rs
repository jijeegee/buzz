//! Postgres-backed tests for the centralized-identity store. `#[ignore]`d like
//! every other buzz-db Postgres test; run with `--ignored` against a migrated
//! database (`BUZZ_TEST_DATABASE_URL`).

use chrono::{Duration, Utc};
use sqlx::PgPool;

use buzz_core::principal::PrincipalId;

use super::*;
use crate::Db;

async fn pool() -> PgPool {
    PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("connect to test DB")
}

fn token(hours: i64) -> IssuedToken {
    IssuedToken {
        hash: rand::random(),
        expires_at: Some(Utc::now() + Duration::hours(hours)),
    }
}

async fn new_user(db: &Db) -> PrincipalId {
    let subject = uuid::Uuid::new_v4().to_string();
    db.login_identity("google", &subject, None, "user", None)
        .await
        .expect("login")
        .principal
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn bot_issuance_revalidates_device_and_owner() {
    let db = Db::from_pool(pool().await);
    let owner = new_user(&db).await;
    let session = db
        .complete_login(&owner, "host", "desktop", None, token(24), token(1))
        .await
        .unwrap();
    let bot = db
        .create_bot(&owner, "bot", Some(session.device_id))
        .await
        .unwrap();
    db.revoke_device(&owner, session.device_id, RevokeReason::Logout)
        .await
        .unwrap();
    assert!(
        db.issue_bot_token(&bot, session.device_id, token(1))
            .await
            .is_err(),
        "a logged-out host cannot issue a token"
    );
    let headless = db.create_bot(&owner, "headless", None).await.unwrap();
    db.disable_principal(&owner, None).await.unwrap();
    assert!(
        db.issue_headless_bot_token(&headless, token(1))
            .await
            .is_err(),
        "disabled owners cannot issue headless tokens"
    );
}

/// Force exchange to reach its token read before revocation starts. A token
/// UPDATE alone sees the pre-exchange snapshot and misses the new token.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn bot_token_creation_cannot_escape_concurrent_revocation() {
    let pool = pool().await;
    let db = Db::from_pool(pool.clone());
    for reissue in [false, true] {
        for reason in [
            RevokeReason::Stopped,
            RevokeReason::Logout,
            RevokeReason::RevokeAll,
            RevokeReason::AccountDisabled,
        ] {
            let owner = new_user(&db).await;
            let login = db
                .complete_login(&owner, "host", "desktop", None, token(24), token(1))
                .await
                .unwrap();
            let bot = db
                .create_bot(&owner, "bot", Some(login.device_id))
                .await
                .unwrap();
            let old = token(1);
            let new = token(1);
            db.issue_bot_token(&bot, login.device_id, old)
                .await
                .unwrap();
            let mut held = pool.begin().await.unwrap();
            sqlx::query("SELECT token_hash FROM access_tokens WHERE token_hash = $1 FOR UPDATE")
                .bind(old.hash.as_slice())
                .execute(&mut *held)
                .await
                .unwrap();
            let exchange = tokio::spawn({
                let db = db.clone();
                async move {
                    if reissue {
                        db.issue_bot_token(&bot, login.device_id, new)
                            .await
                            .unwrap();
                    } else {
                        assert!(matches!(
                            db.exchange_bot_token(
                                &old.hash,
                                new,
                                Duration::seconds(60),
                                Duration::seconds(10)
                            )
                            .await
                            .unwrap(),
                            ExchangeOutcome::Exchanged { .. }
                        ));
                    }
                }
            });
            wait_for_lock_waiters(&pool, 1).await;
            let revocation = tokio::spawn({
                let db = db.clone();
                async move {
                    match reason {
                        RevokeReason::Stopped => db.revoke_bot_tokens(&bot, &["bot"], reason).await,
                        RevokeReason::Logout => db
                            .revoke_device(&owner, login.device_id, reason)
                            .await
                            .map(|v| v.unwrap()),
                        RevokeReason::RevokeAll => db.revoke_all_owned_bot_tokens(&owner).await,
                        RevokeReason::AccountDisabled => db.disable_principal(&owner, None).await,
                        _ => unreachable!(),
                    }
                }
            });
            wait_for_lock_waiters(&pool, 2).await;
            held.commit().await.unwrap();
            exchange.await.unwrap();
            let hashes = revocation.await.unwrap().unwrap();
            assert!(
                hashes.contains(&new.hash),
                "{reason:?} (reissue={reissue}) must publish revocation of the new token"
            );
            assert!(db
                .lookup_access_token(&new.hash)
                .await
                .unwrap()
                .unwrap()
                .check(Utc::now())
                .is_err());
        }
    }
}

async fn wait_for_lock_waiters(pool: &PgPool, count: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock'")
                .fetch_one(pool).await.unwrap();
            if waiting >= count { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("operations reached their database lock barriers");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn revoke_others_preserves_hosted_bot_exchange() {
    let db = Db::from_pool(pool().await);
    let owner = new_user(&db).await;
    let host = db
        .complete_login(&owner, "host", "desktop", None, token(24), token(1))
        .await
        .unwrap();
    let keep = db
        .complete_login(&owner, "keep", "mobile", None, token(24), token(1))
        .await
        .unwrap();
    let bot = db
        .create_bot(&owner, "bot", Some(host.device_id))
        .await
        .unwrap();
    let old = token(1);
    db.issue_bot_token(&bot, host.device_id, old).await.unwrap();
    db.revoke_other_sessions(&owner, keep.session_id)
        .await
        .unwrap();
    assert!(db
        .lookup_access_token(&old.hash)
        .await
        .unwrap()
        .unwrap()
        .check(Utc::now())
        .is_ok());
    assert!(matches!(
        db.exchange_bot_token(
            &old.hash,
            token(1),
            Duration::seconds(60),
            Duration::seconds(10)
        )
        .await
        .unwrap(),
        ExchangeOutcome::Exchanged { .. }
    ));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn revoked_exchange_replay_is_invalid() {
    let db = Db::from_pool(pool().await);
    let owner = new_user(&db).await;
    let login = db
        .complete_login(&owner, "host", "desktop", None, token(24), token(1))
        .await
        .unwrap();
    let bot = db
        .create_bot(&owner, "bot", Some(login.device_id))
        .await
        .unwrap();
    let old = token(1);
    db.issue_bot_token(&bot, login.device_id, old)
        .await
        .unwrap();
    db.exchange_bot_token(
        &old.hash,
        token(1),
        Duration::seconds(60),
        Duration::seconds(10),
    )
    .await
    .unwrap();
    db.revoke_bot_tokens(&bot, &["bot"], RevokeReason::Stopped)
        .await
        .unwrap();
    assert_eq!(
        db.exchange_bot_token(
            &old.hash,
            token(1),
            Duration::seconds(60),
            Duration::seconds(10)
        )
        .await
        .unwrap(),
        ExchangeOutcome::Invalid
    );
}

/// B1: concurrent first boots converge on one relay principal — the relay
/// key — and a changed key re-keys the single row.
#[tokio::test]
#[ignore = "requires Postgres — mutates the global relay principal"]
async fn concurrent_relay_principal_bootstrap_converges() {
    let pool = pool().await;
    let key = PrincipalId::generate();
    let (a, b) = tokio::join!(
        principal::ensure_relay_principal(&pool, &key),
        principal::ensure_relay_principal(&pool, &key)
    );
    let (a, b) = (a.expect("first"), b.expect("second"));
    assert_eq!(a, b, "both boots must observe the same relay principal");
    assert_eq!(a, key, "the relay principal is the relay key");
    // An operator the relay added under the old key follows the re-key.
    let operator = PrincipalId::generate();
    sqlx::query("INSERT INTO relay_operators (pubkey, role, added_by) VALUES ($1, 'operator', $2)")
        .bind(operator.as_bytes().as_slice())
        .bind(key.as_bytes().as_slice())
        .execute(&pool)
        .await
        .expect("seed operator");
    let rotated = PrincipalId::generate();
    let c = principal::ensure_relay_principal(&pool, &rotated)
        .await
        .expect("rotated");
    assert_eq!(c, rotated, "a new relay key re-keys the relay principal");
    let added_by: Vec<u8> =
        sqlx::query_scalar("SELECT added_by FROM relay_operators WHERE pubkey = $1")
            .bind(operator.as_bytes().as_slice())
            .fetch_one(&pool)
            .await
            .expect("added_by");
    sqlx::query("DELETE FROM relay_operators WHERE pubkey = $1")
        .bind(operator.as_bytes().as_slice())
        .execute(&pool)
        .await
        .expect("cleanup operator");
    assert_eq!(added_by, rotated.as_bytes().to_vec(), "added_by follows");

    // A relay key that already names a user is refused, and both rows stay.
    let db = Db::from_pool(pool.clone());
    let user = new_user(&db).await;
    let refused = principal::ensure_relay_principal(&pool, &user).await;
    assert!(
        matches!(&refused, Err(crate::DbError::InvalidData(m)) if m.contains("non-relay")),
        "relay key colliding with a user is refused up front: {refused:?}"
    );
    let relay_now: Vec<u8> = sqlx::query_scalar("SELECT id FROM principals WHERE kind = 'relay'")
        .fetch_one(&pool)
        .await
        .expect("relay row");
    assert_eq!(
        relay_now,
        rotated.as_bytes().to_vec(),
        "relay row unchanged"
    );
    assert!(db.get_principal(&user).await.expect("user").is_some());
    let relay_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM principals WHERE kind = 'relay'")
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(relay_rows, 1);
}

/// B1: the store refuses stored bytes that are not a valid x-only key, so an
/// invalid id can never be handed out as a principal.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn invalid_principal_bytes_are_rejected() {
    assert!(principal_from_bytes(&[0u8; 32]).is_err());
    assert!(principal_from_bytes(&[1u8; 31]).is_err());
}

/// Concurrent first login for the same subject yields one principal and one
/// identity row.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn concurrent_first_login_creates_one_principal() {
    let db = Db::from_pool(pool().await);
    let subject = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        db.login_identity("google", &subject, Some("a@example.test"), "A", None),
        db.login_identity("google", &subject, Some("a@example.test"), "A", None)
    );
    let (a, b) = (a.expect("a"), b.expect("b"));
    assert_eq!(a.principal, b.principal);
    assert!(
        a.created ^ b.created,
        "exactly one login created the account"
    );
    let identities: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM identities WHERE provider = 'google' AND subject = $1",
    )
    .bind(&subject)
    .fetch_one(db.pool())
    .await
    .expect("count");
    assert_eq!(identities, 1);
}

/// B2 (store seam): a consumed refresh row is reuse — the session and its
/// access tokens are revoked — while an unknown token changes nothing.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn refresh_reuse_revokes_session_unknown_does_not() {
    let db = Db::from_pool(pool().await);
    let user = new_user(&db).await;
    let r1 = token(24);
    let a1 = token(1);
    let login = db
        .complete_login(&user, "laptop", "desktop", None, r1, a1)
        .await
        .expect("login");

    assert_eq!(
        db.rotate_refresh_token(&rand::random(), token(24), token(1), Duration::zero())
            .await
            .expect("unknown"),
        RefreshOutcome::NotFound
    );

    let r2 = token(24);
    let a2 = token(1);
    let rotated = db
        .rotate_refresh_token(&r1.hash, r2, a2, Duration::zero())
        .await
        .expect("rotate");
    assert!(
        matches!(rotated, RefreshOutcome::Rotated { session_id, .. } if session_id == login.session_id)
    );

    assert_eq!(
        db.rotate_refresh_token(&r1.hash, token(24), token(1), Duration::seconds(10))
            .await
            .expect("retry inside grace"),
        RefreshOutcome::RecentlyRotated,
        "a retry inside the grace window must not revoke the session"
    );
    let reused = db
        .rotate_refresh_token(&r1.hash, token(24), token(1), Duration::zero())
        .await
        .expect("reuse");
    match reused {
        RefreshOutcome::Reused {
            session_id,
            revoked,
        } => {
            assert_eq!(session_id, login.session_id);
            assert!(revoked.contains(&a1.hash) && revoked.contains(&a2.hash));
        }
        other => panic!("expected reuse, got {other:?}"),
    }
    let revoked_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT revoked_at FROM sessions WHERE id = $1")
            .bind(login.session_id)
            .fetch_one(db.pool())
            .await
            .expect("session");
    assert!(
        revoked_at.is_some(),
        "refresh reuse must revoke the session"
    );
    assert_eq!(
        db.rotate_refresh_token(&r2.hash, token(24), token(1), Duration::zero())
            .await
            .expect("after revoke"),
        RefreshOutcome::SessionRevoked
    );
}

/// Exchange: superseded tokens can never be exchanged again, keep working for
/// their grace period, and at most two bot tokens are live.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn exchange_supersedes_and_bounds_live_tokens() {
    let db = Db::from_pool(pool().await);
    let owner = new_user(&db).await;
    let login = db
        .complete_login(&owner, "laptop", "desktop", None, token(24), token(1))
        .await
        .expect("login");
    let bot = db
        .create_bot(&owner, "helper", Some(login.device_id))
        .await
        .expect("bot");
    let t1 = token(1);
    db.issue_bot_token(&bot, login.device_id, t1)
        .await
        .expect("issue");

    let t2 = token(1);
    let outcome = db
        .exchange_bot_token(&t1.hash, t2, Duration::seconds(60), Duration::seconds(10))
        .await
        .expect("exchange");
    let ExchangeOutcome::Exchanged {
        old_grace_until, ..
    } = outcome
    else {
        panic!("expected exchange, got {outcome:?}");
    };
    assert!(old_grace_until <= Utc::now() + Duration::seconds(61));
    // A retry inside the replay window is not a conflict: the caller serves
    // it from the replay cache.
    assert_eq!(
        db.exchange_bot_token(
            &t1.hash,
            token(1),
            Duration::seconds(60),
            Duration::seconds(10)
        )
        .await
        .expect("retried exchange"),
        ExchangeOutcome::RecentlyExchanged
    );
    // Outside the window the superseded token conflicts.
    sqlx::query(
        "UPDATE access_tokens SET superseded_at = now() - interval '11 seconds'          WHERE token_hash = $1",
    )
    .bind(t1.hash.as_slice())
    .execute(db.pool())
    .await
    .expect("age superseded_at");
    assert_eq!(
        db.exchange_bot_token(
            &t1.hash,
            token(1),
            Duration::seconds(60),
            Duration::seconds(10)
        )
        .await
        .expect("second exchange"),
        ExchangeOutcome::Superseded
    );
    let old = db
        .lookup_access_token(&t1.hash)
        .await
        .expect("lookup")
        .expect("row");
    assert!(
        old.check(Utc::now()).is_ok(),
        "superseded token works during grace"
    );
    assert!(
        old.check(old_grace_until + Duration::seconds(1)).is_err(),
        "superseded token stops at the end of its grace"
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_tokens WHERE bot_id = $1 AND revoked_at IS NULL",
    )
    .bind(bot.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .expect("count");
    assert_eq!(live, 2, "old (grace) + new");
}

/// B8 bootstrap: granted only while the DB roster has no operator.
#[tokio::test]
#[ignore = "requires Postgres — mutates the global relay_operators roster"]
async fn bootstrap_grants_only_into_an_empty_roster() {
    let db = Db::from_pool(pool().await);
    sqlx::query("DELETE FROM relay_operators")
        .execute(db.pool())
        .await
        .expect("clear roster");
    let relay = db
        .ensure_relay_principal(&PrincipalId::generate())
        .await
        .expect("relay");
    let first = new_user(&db).await;
    let second = new_user(&db).await;
    assert!(db
        .bootstrap_relay_operator(first.as_bytes(), relay.as_bytes())
        .await
        .expect("bootstrap"));
    assert!(!db
        .bootstrap_relay_operator(second.as_bytes(), relay.as_bytes())
        .await
        .expect("second bootstrap"));
    let row = db
        .get_relay_operator(first.as_bytes())
        .await
        .expect("get")
        .expect("granted");
    assert_eq!(row.role, "operator");
    assert_eq!(row.added_by, relay.as_bytes().to_vec());
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relay_operator_audit WHERE target_pubkey = $1 AND op = 'grant'",
    )
    .bind(first.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .expect("audit");
    assert_eq!(audited, 1, "bootstrap goes through the audited upsert path");
    sqlx::query("DELETE FROM relay_operators")
        .execute(db.pool())
        .await
        .expect("clean roster");
}

/// Phase 0 risk (a): a server-stamped event (sentinel sig) and a NULL-sig row
/// both come back from the production row mapper as `Some`.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn stamped_and_null_sig_events_read_back() {
    let db = Db::from_pool(pool().await);
    let community = buzz_core::CommunityId::from_uuid(uuid::Uuid::new_v4());
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(*community.as_uuid())
        .bind(format!("{}.stamp.test", uuid::Uuid::new_v4().simple()))
        .execute(db.pool())
        .await
        .expect("community");
    let principal = PrincipalId::generate();
    let draft = serde_json::json!({"kind": 1, "content": "stamped", "tags": []});
    let event =
        buzz_core::draft::stamp_draft(&draft, &principal, Utc::now().timestamp()).expect("stamp");
    let (_, inserted) = db
        .insert_event(community, &event, None)
        .await
        .expect("insert");
    assert!(inserted);

    let fetched = db
        .get_event_by_id(community, event.id.as_bytes())
        .await
        .expect("get")
        .expect("stamped event must not vanish at row_to_stored_event");
    assert_eq!(fetched.event.pubkey, principal.as_public_key());

    sqlx::query("UPDATE events SET sig = NULL WHERE community_id = $1 AND id = $2")
        .bind(*community.as_uuid())
        .bind(event.id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .expect("null sig");
    let fetched = db
        .get_event_by_id(community, event.id.as_bytes())
        .await
        .expect("get null sig")
        .expect("NULL sig row must not vanish");
    assert!(buzz_core::draft::is_sentinel_sig(
        fetched.event.sig.as_ref()
    ));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn relogin_with_install_id_reuses_device() {
    let db = Db::from_pool(pool().await);
    let owner = new_user(&db).await;
    let install = Some("5f0c6b1e-2a4d-4c8e-9b7a-1d2e3f4a5b6c");
    let first_access = token(1);
    let first = db
        .complete_login(
            &owner,
            "Buzz Desktop",
            "desktop",
            install,
            token(24),
            first_access,
        )
        .await
        .unwrap();
    db.rename_device(&owner, first.device_id, "Home PC")
        .await
        .unwrap();

    // Same install, still signed in: same device, a new session, and the
    // earlier one stays usable (the client may still discard this grant).
    let second = db
        .complete_login(
            &owner,
            "Buzz Desktop",
            "desktop",
            install,
            token(24),
            token(1),
        )
        .await
        .unwrap();
    assert_eq!(second.device_id, first.device_id);
    assert_ne!(second.session_id, first.session_id);
    assert!(db
        .lookup_access_token(&first_access.hash)
        .await
        .unwrap()
        .unwrap()
        .check(Utc::now())
        .is_ok());

    // Logged out, then back in: the device comes back with its name.
    db.revoke_device(&owner, second.device_id, RevokeReason::Logout)
        .await
        .unwrap();
    let third = db
        .complete_login(
            &owner,
            "Buzz Desktop",
            "desktop",
            install,
            token(24),
            token(1),
        )
        .await
        .unwrap();
    assert_eq!(third.device_id, first.device_id);
    let devices = db.list_devices(&owner).await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].id, first.device_id);
    assert_eq!(devices[0].name, "Home PC");

    // No install id, or another one: a new device.
    let legacy = db
        .complete_login(&owner, "Buzz Desktop", "desktop", None, token(24), token(1))
        .await
        .unwrap();
    assert_ne!(legacy.device_id, first.device_id);
    let other = db
        .complete_login(
            &owner,
            "Buzz Desktop",
            "desktop",
            Some("0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"),
            token(24),
            token(1),
        )
        .await
        .unwrap();
    assert_ne!(other.device_id, first.device_id);

    // Install ids are per principal: another account gets its own device.
    let stranger = new_user(&db).await;
    let theirs = db
        .complete_login(
            &stranger,
            "Buzz Desktop",
            "desktop",
            install,
            token(24),
            token(1),
        )
        .await
        .unwrap();
    assert_ne!(theirs.device_id, first.device_id);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn relogin_with_install_id_is_not_blocked_by_the_device_limit() {
    let db = Db::from_pool(pool().await);
    let owner = new_user(&db).await;
    let install = Some("7c9e6679-7425-40de-944b-e07fc1f90ae7");
    let mine = db
        .complete_login(&owner, "Phone", "mobile", install, token(24), token(1))
        .await
        .unwrap();
    for _ in 1..MAX_DEVICES_PER_PRINCIPAL {
        db.complete_login(&owner, "other", "desktop", None, token(24), token(1))
            .await
            .unwrap();
    }
    assert!(db
        .complete_login(&owner, "one more", "desktop", None, token(24), token(1))
        .await
        .is_err());
    // At the limit, live or logged out, the known install still signs in.
    let again = db
        .complete_login(&owner, "Phone", "mobile", install, token(24), token(1))
        .await
        .unwrap();
    assert_eq!(again.device_id, mine.device_id);
    db.revoke_device(&owner, mine.device_id, RevokeReason::Logout)
        .await
        .unwrap();
    let back = db
        .complete_login(&owner, "Phone", "mobile", install, token(24), token(1))
        .await
        .unwrap();
    assert_eq!(back.device_id, mine.device_id);
}
