//! Binding/transaction regressions, run against an isolated test database only.
use super::*;
use crate::Db;
use chrono::{Duration, Utc};

async fn database() -> Db {
    Db::from_pool(
        PgPool::connect(&crate::test_support::database_url())
            .await
            .unwrap(),
    )
}

fn token() -> IssuedToken {
    IssuedToken {
        hash: rand::random(),
        expires_at: Some(Utc::now() + Duration::hours(1)),
    }
}

async fn account(db: &Db, subject: &str) -> PrincipalId {
    db.login_key_identity("google", subject, None, "user", None)
        .await
        .unwrap()
        .principal
}

async fn session(db: &Db, account: &PrincipalId) -> LoginSession {
    db.complete_login(account, "test device", "desktop", token(), token())
        .await
        .unwrap()
}

fn envelope(pubkey: [u8; 32]) -> KeyBackupRecord {
    KeyBackupRecord {
        pubkey,
        version: 1,
        key_id: "test".into(),
        nonce: vec![3; 12],
        ciphertext: vec![4; 48],
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn key_backup_concurrent_initialization_never_replaces_binding() {
    let db = database().await;
    let subject = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(account(&db, &subject), account(&db, &subject));
    assert_eq!(a, b);
    assert!(db.account_key_mode(&a).await.unwrap());
    let s1 = session(&db, &a).await;
    let s2 = session(&db, &a).await;
    let k1 = envelope(nostr::Keys::generate().public_key().to_bytes());
    let k2 = envelope(nostr::Keys::generate().public_key().to_bytes());
    let (r1, r2) = tokio::join!(
        db.initialize_key_backup(&a, s1.session_id, &k1),
        db.initialize_key_backup(&a, s2.session_id, &k2)
    );
    let (r1, r2) = (r1.unwrap(), r2.unwrap());
    assert!(matches!(
        (r1, r2),
        (
            InitializeKeyBackup::Created,
            InitializeKeyBackup::BackupExists
        ) | (
            InitializeKeyBackup::BackupExists,
            InitializeKeyBackup::Created
        )
    ));
    let winner = db.get_key_backup(&a).await.unwrap().unwrap();
    let expected = if r1 == InitializeKeyBackup::Created {
        &k1
    } else {
        &k2
    };
    assert_eq!(winner.pubkey, expected.pubkey);
    assert_eq!(
        db.initialize_key_backup(&a, s1.session_id, &k1)
            .await
            .unwrap(),
        InitializeKeyBackup::BackupExists
    );
    assert_eq!(
        db.get_key_backup(&a).await.unwrap().unwrap().pubkey,
        winner.pubkey
    );
    let other = account(&db, &uuid::Uuid::new_v4().to_string()).await;
    assert!(db.get_key_backup(&other).await.unwrap().is_none());
    let other_session = session(&db, &other).await;
    assert_eq!(
        db.initialize_key_backup(&other, other_session.session_id, expected)
            .await
            .unwrap(),
        InitializeKeyBackup::PubkeyInUse
    );
    assert!(!db
        .key_backup_session_fresh(&other, s1.session_id)
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn key_backup_token_principals_conflict_without_migration_or_email_merging() {
    let db = database().await;
    let sub = uuid::Uuid::new_v4().to_string();
    let token_user = db
        .login_identity("google", &sub, Some("same@example.test"), "old", None)
        .await
        .unwrap()
        .principal;
    assert!(db
        .login_key_identity("google", &sub, None, "new", None)
        .await
        .is_err());
    assert!(!db.account_key_mode(&token_user).await.unwrap());
    assert_eq!(
        db.login_existing_token_identity("google", &sub, None, "old", None)
            .await
            .unwrap()
            .principal,
        token_user
    );
    assert!(db
        .login_existing_token_identity(
            "google",
            &uuid::Uuid::new_v4().to_string(),
            None,
            "new",
            None
        )
        .await
        .is_err());
    assert_eq!(
        db.login_identity("google", &sub, None, "old", None)
            .await
            .unwrap()
            .principal,
        token_user
    );
    let key_sub = uuid::Uuid::new_v4().to_string();
    let key_user = db
        .login_key_identity("google", &key_sub, Some("same@example.test"), "new", None)
        .await
        .unwrap()
        .principal;
    assert_ne!(key_user, token_user);
    assert!(db
        .login_identity("google", &key_sub, None, "new", None)
        .await
        .is_err());
    assert!(db.get_key_backup(&key_user).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn key_backup_refresh_keeps_identity_but_does_not_renew_export_freshness() {
    let db = database().await;
    let a = account(&db, &uuid::Uuid::new_v4().to_string()).await;
    let refresh = token();
    let session = db
        .complete_login(&a, "device", "mobile", refresh, token())
        .await
        .unwrap();
    let key = envelope(nostr::Keys::generate().public_key().to_bytes());
    assert_eq!(
        db.initialize_key_backup(&a, session.session_id, &key)
            .await
            .unwrap(),
        InitializeKeyBackup::Created
    );
    sqlx::query("UPDATE sessions SET created_at = now() - interval '6 minutes' WHERE id = $1")
        .bind(session.session_id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        matches!(db.rotate_refresh_token(&refresh.hash,token(),token(),Duration::seconds(10)).await.unwrap(),RefreshOutcome::Rotated{principal,..} if principal==a)
    );
    assert!(!db
        .key_backup_session_fresh(&a, session.session_id)
        .await
        .unwrap());
    assert_eq!(
        db.get_key_backup(&a).await.unwrap().unwrap().pubkey,
        key.pubkey
    );
    assert_eq!(
        db.initialize_key_backup(&a, session.session_id, &key)
            .await
            .unwrap(),
        InitializeKeyBackup::SessionNotFresh
    );
}
