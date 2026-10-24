//! Account purge against a migrated test database (`KUZZ_TEST_DATABASE_URL`).
//! `#[ignore]`d like every other kuzz-db Postgres test.

use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use kuzz_core::principal::PrincipalId;

use super::*;
use crate::Db;

async fn database() -> (Db, PgPool) {
    let pool = PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("connect to test DB");
    (Db::from_pool(pool.clone()), pool)
}

fn token() -> IssuedToken {
    IssuedToken {
        hash: rand::random(),
        expires_at: Some(Utc::now() + Duration::hours(1)),
    }
}

fn bytes(id: &PrincipalId) -> Vec<u8> {
    id.as_bytes().to_vec()
}

/// Delete the account and move its window into the past.
async fn delete_and_expire(db: &Db, pool: &PgPool, account: &PrincipalId) {
    db.disable_principal(account, Some(Duration::days(30)))
        .await
        .unwrap();
    sqlx::query("UPDATE principals SET purge_after = now() - interval '1 second' WHERE id = $1")
        .bind(bytes(account))
        .execute(pool)
        .await
        .unwrap();
}

async fn community(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(id)
        .bind(format!("purge-{}.invalid", id.simple()))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn event(pool: &PgPool, community: Uuid, pubkey: &[u8], kind: i32) -> Vec<u8> {
    let id: [u8; 32] = rand::random();
    sqlx::query(
        "INSERT INTO events (community_id, id, pubkey, created_at, kind, tags, content, sig, received_at) \
         VALUES ($1, $2, $3, now(), $4, '[]', 'purge test', $5, now())",
    )
    .bind(community)
    .bind(id.to_vec())
    .bind(pubkey)
    .bind(kind)
    .bind(vec![0u8; 64])
    .execute(pool)
    .await
    .unwrap();
    id.to_vec()
}

async fn count(pool: &PgPool, sql: &'static str, key: &[u8]) -> i64 {
    sqlx::query_scalar(sql)
        .bind(key)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn purge_removes_account_data_and_keeps_messages() {
    let (db, pool) = database().await;
    let subject = Uuid::new_v4().to_string();
    let account = db
        .login_identity("google", &subject, Some("purge@example.com"), "user", None)
        .await
        .unwrap()
        .principal;
    let session = db
        .complete_login(&account, "laptop", "desktop", None, token(), token())
        .await
        .unwrap();
    let bot = db
        .create_bot(&account, "helper", Some(session.device_id))
        .await
        .unwrap();
    let other: [u8; 32] = rand::random();
    let cid = community(&pool).await;
    let key = bytes(&account);
    let profile = event(&pool, cid, &key, 0).await;
    event(&pool, cid, &key, 3).await;
    let message = event(&pool, cid, &key, 9).await;
    event(&pool, cid, &bytes(&bot), 0).await;
    let bot_message = event(&pool, cid, &bytes(&bot), 9).await;
    sqlx::query(
        "INSERT INTO users (community_id, pubkey, display_name, avatar_url, about, metadata_event_id) \
         VALUES ($1, $2, 'Purge Me', 'https://example.com/me.png', 'hi', $3)",
    )
    .bind(cid)
    .bind(&key)
    .bind(&profile)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_blocks (community_id, blocker, blocked) VALUES ($1, $2, $3)")
        .bind(cid)
        .bind(&key)
        .bind(other.to_vec())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_blocks (community_id, blocker, blocked) VALUES ($1, $2, $3)")
        .bind(cid)
        .bind(other.to_vec())
        .bind(&key)
        .execute(&pool)
        .await
        .unwrap();

    delete_and_expire(&db, &pool, &account).await;
    assert!(db
        .due_account_purges(10_000)
        .await
        .unwrap()
        .contains(&account));

    let PurgeOutcome::Purged(counts) = db.purge_account(&account, false).await.unwrap() else {
        panic!("due account was not purged");
    };
    assert_eq!(counts.identities, 1);
    assert_eq!(counts.devices, 1);
    assert_eq!(counts.bots, 1);
    assert_eq!(counts.events, 3, "own profile, friend list, agent profile");
    assert_eq!(counts.profiles, 1);
    assert_eq!(counts.community_rows, 1, "only the account's own block");

    // Account information is gone; the tombstone stays disabled.
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM identities WHERE principal_id = $1",
            &key
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM devices WHERE principal_id = $1",
            &key
        )
        .await,
        0
    );
    let tombstone = db.get_principal(&account).await.unwrap().unwrap();
    assert!(tombstone.disabled_at.is_some());
    assert_eq!(tombstone.display_name, "");
    assert!(tombstone.avatar_url.is_none());
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM principals WHERE id = $1 AND purged_at IS NOT NULL",
            &key
        )
        .await,
        1
    );
    let (name, avatar, about): (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT display_name, avatar_url, about FROM users WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(cid)
    .bind(&key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((name, avatar, about), (None, None, None));
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM events WHERE pubkey = $1 AND kind IN (0, 3)",
            &key
        )
        .await,
        0
    );
    let bot_record = db.get_bot(&bot).await.unwrap().unwrap();
    assert!(bot_record.deleted_at.is_some());
    assert_eq!(
        db.get_principal(&bot).await.unwrap().unwrap().display_name,
        ""
    );

    // Messages, and other people's block of the account, stay.
    for id in [&message, &bot_message] {
        assert_eq!(
            count(&pool, "SELECT count(*) FROM events WHERE id = $1", id).await,
            1
        );
    }
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM user_blocks WHERE blocked = $1",
            &key
        )
        .await,
        1
    );

    // Not purged twice; the same Google account signs up as a new account.
    assert!(!db
        .due_account_purges(10_000)
        .await
        .unwrap()
        .contains(&account));
    assert_eq!(
        db.purge_account(&account, false).await.unwrap(),
        PurgeOutcome::NotDue
    );
    let again = db
        .login_identity("google", &subject, Some("purge@example.com"), "user", None)
        .await
        .unwrap();
    assert!(again.created);
    assert_ne!(again.principal, account);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn purge_waits_for_the_window_and_dry_run_changes_nothing() {
    let (db, pool) = database().await;
    let subject = Uuid::new_v4().to_string();
    let account = db
        .login_identity("google", &subject, None, "user", None)
        .await
        .unwrap()
        .principal;
    db.complete_login(&account, "phone", "mobile", None, token(), token())
        .await
        .unwrap();

    // Inside the window nothing happens, and a login restores the account.
    db.disable_principal(&account, Some(Duration::days(30)))
        .await
        .unwrap();
    assert_eq!(
        db.purge_account(&account, false).await.unwrap(),
        PurgeOutcome::NotDue
    );
    let restored = db
        .login_identity("google", &subject, None, "user", None)
        .await
        .unwrap();
    assert_eq!(restored.principal, account);
    assert!(!restored.disabled);

    // An operator disable is never purged.
    db.disable_principal(&account, None).await.unwrap();
    assert_eq!(
        db.purge_account(&account, false).await.unwrap(),
        PurgeOutcome::NotDue
    );
    sqlx::query("UPDATE principals SET disabled_at = NULL WHERE id = $1")
        .bind(bytes(&account))
        .execute(&pool)
        .await
        .unwrap();

    // Dry run reports the real counts and keeps everything.
    delete_and_expire(&db, &pool, &account).await;
    let dry = db.purge_account(&account, true).await.unwrap();
    let PurgeOutcome::Purged(dry_counts) = dry else {
        panic!("dry run did not run");
    };
    assert_eq!(dry_counts.identities, 1);
    assert_eq!(dry_counts.devices, 1);
    let key = bytes(&account);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM identities WHERE principal_id = $1",
            &key
        )
        .await,
        1
    );
    assert!(db
        .due_account_purges(10_000)
        .await
        .unwrap()
        .contains(&account));
    assert_eq!(
        db.purge_account(&account, false).await.unwrap(),
        PurgeOutcome::Purged(dry_counts)
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn purged_key_account_key_stays_refused() {
    let (db, pool) = database().await;
    let subject = Uuid::new_v4().to_string();
    let account = db
        .login_key_identity("google", &subject, None, "user", None)
        .await
        .unwrap()
        .principal;
    let session = db
        .complete_login(&account, "laptop", "desktop", None, token(), token())
        .await
        .unwrap();
    let pubkey = nostr::Keys::generate().public_key().to_bytes();
    let backup = KeyBackupRecord {
        pubkey,
        version: KEY_BACKUP_VERSION,
        key_id: "test".into(),
        nonce: vec![3; 12],
        ciphertext: vec![4; 48],
    };
    assert_eq!(
        db.initialize_key_backup(&account, session.session_id, &backup)
            .await
            .unwrap(),
        InitializeKeyBackup::Created
    );

    delete_and_expire(&db, &pool, &account).await;
    assert!(matches!(
        db.purge_account(&account, false).await.unwrap(),
        PurgeOutcome::Purged(PurgeCounts { key_backups: 1, .. })
    ));
    assert!(db.get_key_backup(&account).await.unwrap().is_none());
    assert!(
        db.key_account_disabled(&[pubkey]).await.unwrap(),
        "the purged account's key is still refused"
    );

    // A new account cannot take the purged key over.
    let fresh = db
        .login_key_identity("google", &subject, None, "user", None)
        .await
        .unwrap();
    assert!(fresh.created);
    let fresh_session = db
        .complete_login(
            &fresh.principal,
            "laptop",
            "desktop",
            None,
            token(),
            token(),
        )
        .await
        .unwrap();
    assert_eq!(
        db.initialize_key_backup(&fresh.principal, fresh_session.session_id, &backup)
            .await
            .unwrap(),
        InitializeKeyBackup::PubkeyInUse
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn purge_skips_relay_operators() {
    let (db, pool) = database().await;
    let account = db
        .login_identity("google", &Uuid::new_v4().to_string(), None, "user", None)
        .await
        .unwrap()
        .principal;
    sqlx::query("INSERT INTO relay_operators (pubkey, role, added_by) VALUES ($1, 'operator', $1)")
        .bind(bytes(&account))
        .execute(&pool)
        .await
        .unwrap();
    delete_and_expire(&db, &pool, &account).await;
    assert_eq!(
        db.purge_account(&account, false).await.unwrap(),
        PurgeOutcome::SkippedOperator
    );
    sqlx::query("DELETE FROM relay_operators WHERE pubkey = $1")
        .bind(bytes(&account))
        .execute(&pool)
        .await
        .unwrap();
}
