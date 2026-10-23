//! External agent pairing (migration 0090), against an isolated test
//! database only.
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

fn device_key() -> [u8; 33] {
    let mut key = [2u8; 33];
    key[1..].copy_from_slice(&rand::random::<[u8; 32]>());
    key
}

fn agent_key() -> [u8; 32] {
    nostr::Keys::generate().public_key().to_bytes()
}

/// A key-backup account (own key random) signed in on one device.
async fn owner(db: &Db) -> (PrincipalId, [u8; 32], Uuid, [u8; 33]) {
    let subject = uuid::Uuid::new_v4().to_string();
    let account = db
        .login_key_identity("google", &subject, None, "owner", None)
        .await
        .unwrap()
        .principal;
    let pubkey = agent_key();
    sqlx::query(
        "INSERT INTO account_key_backups (account_id, pubkey, version, key_id, nonce, ciphertext) \
         VALUES ($1, $2, 2, 'k1', $3, $4)",
    )
    .bind(account.as_bytes().as_slice())
    .bind(pubkey.as_slice())
    .bind([0u8; 12].as_slice())
    .bind([0u8; 48].as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let key = device_key();
    let device = login(db, &account, &key).await;
    (account, pubkey, device, key)
}

async fn login(db: &Db, account: &PrincipalId, key: &[u8; 33]) -> Uuid {
    db.complete_login_with_device_key(
        account,
        "pc",
        "desktop",
        None,
        Some(DeviceKey {
            pubkey: key,
            kind: "hardware",
        }),
        token(),
        token(),
    )
    .await
    .unwrap()
    .device_id
}

async fn start(db: &Db, owner: &PrincipalId, device: Uuid) -> (Uuid, [u8; 32]) {
    let code_hash: [u8; 32] = rand::random();
    match db.start_pairing(owner, device, &code_hash).await.unwrap() {
        StartPairing::Started { id, .. } => (id, code_hash),
        other => panic!("start: {other:?}"),
    }
}

async fn claim(db: &Db, code_hash: &[u8; 32], agent: &[u8; 32], device: &[u8; 33]) -> ClaimPairing {
    db.claim_pairing(&PairingClaim {
        code_hash,
        agent_pubkey: agent,
        device_pubkey: device,
        device_key_kind: "software",
        device_name: Some("studio mac"),
    })
    .await
    .unwrap()
}

async fn device_row(db: &Db, id: Uuid) -> (String, String, bool) {
    sqlx::query_as("SELECT kind, platform, revoked_at IS NOT NULL FROM devices WHERE id = $1")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

/// Claim that must connect: the external device.
async fn connect(db: &Db, code_hash: &[u8; 32], agent: &[u8; 32], device: &[u8; 33]) -> Uuid {
    match claim(db, code_hash, agent, device).await {
        ClaimPairing::Connected { device_id, .. } => device_id,
        other => panic!("claim: {other:?}"),
    }
}

async fn external_devices(db: &Db, owner: &PrincipalId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM devices WHERE principal_id = $1 AND kind = 'external'")
        .bind(owner.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_claim_connects_the_agent_to_its_computer_only() {
    let db = database().await;
    let (owner, owner_key, pc, pc_key) = owner(&db).await;
    let (id, code) = start(&db, &owner, pc).await;
    let (agent, remote) = (agent_key(), device_key());

    let listed = db.recent_pairings(&owner).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].id, listed[0].state), (id, PairingState::Open));
    assert!(!listed[0].expired());
    assert_eq!(
        db.key_admission(&agent, Some(&remote)).await.unwrap(),
        KeyAdmission::Unknown
    );

    // The first valid claim connects at once.
    let ClaimPairing::Connected {
        pairing_id,
        device_id,
        owner_pubkey,
    } = claim(&db, &code, &agent, &remote).await
    else {
        panic!("claim");
    };
    assert_eq!(pairing_id, id);
    // The agent learns whose account key it now works for.
    assert_eq!(owner_pubkey, owner_key);
    assert_eq!(
        device_row(&db, device_id).await,
        ("external".into(), "external".into(), false)
    );
    let name: String = sqlx::query_scalar("SELECT name FROM devices WHERE id = $1")
        .bind(device_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(name, "studio mac");
    // The owner's list: who connected, from where, when.
    let listed = db.recent_pairings(&owner).await.unwrap();
    assert_eq!(listed[0].state, PairingState::Connected);
    assert_eq!(listed[0].agent_pubkey, Some(agent));
    assert_eq!(listed[0].device_pubkey, Some(remote));
    assert_eq!(listed[0].device_name.as_deref(), Some("studio mac"));
    assert_eq!(listed[0].device_id, Some(device_id));
    assert!(listed[0].claimed_at.is_some());
    assert!(!listed[0].expired());
    // Another account sees none of it.
    let (stranger, ..) = self::owner(&db).await;
    assert!(db.recent_pairings(&stranger).await.unwrap().is_empty());

    // Admitted from the paired computer with no login there; nowhere else.
    assert_eq!(
        db.key_admission(&agent, Some(&remote)).await.unwrap(),
        KeyAdmission::Admitted {
            role: KeyRole::External,
            principal: owner,
            device_id,
            owner_pubkey: Some(owner_key),
        }
    );
    for other in [Some(&pc_key), None] {
        assert!(matches!(
            db.key_admission(&agent, other).await.unwrap(),
            KeyAdmission::Refused {
                role: KeyRole::External,
                ..
            }
        ));
    }
    // The paired computer is not a login device: the owner's own key is not
    // admitted from it.
    assert_eq!(
        db.key_admission(&owner_key, Some(&remote)).await.unwrap(),
        KeyAdmission::Refused {
            role: KeyRole::Person,
            reason: KeyRefusal::DeviceUnknown,
        }
    );

    // A second agent on the same computer shares its external device.
    let (_, code2) = start(&db, &owner, pc).await;
    assert_eq!(connect(&db, &code2, &agent_key(), &remote).await, device_id);
    assert_eq!(external_devices(&db, &owner).await, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn codes_are_single_use_and_expire() {
    let db = database().await;
    let (owner, _, pc, _) = owner(&db).await;
    let (agent, remote) = (agent_key(), device_key());

    // Used: every later claim is refused, the same keys included, and
    // nothing more is written.
    let (_, code) = start(&db, &owner, pc).await;
    connect(&db, &code, &agent, &remote).await;
    let (thief, thief_device) = (agent_key(), device_key());
    for (key, device) in [
        (&thief, &thief_device),
        (&agent, &remote),
        (&agent, &thief_device),
    ] {
        assert_eq!(claim(&db, &code, key, device).await, ClaimPairing::Used);
    }
    assert_eq!(external_devices(&db, &owner).await, 1);
    assert_eq!(
        db.key_admission(&thief, Some(&thief_device)).await.unwrap(),
        KeyAdmission::Unknown
    );

    // An unknown or expired code connects nothing.
    assert_eq!(
        claim(&db, &rand::random(), &thief, &thief_device).await,
        ClaimPairing::Invalid
    );
    let (expired, code) = start(&db, &owner, pc).await;
    sqlx::query(
        "UPDATE pairing_requests SET expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(expired)
    .execute(db.pool())
    .await
    .unwrap();
    assert_eq!(
        claim(&db, &code, &thief, &thief_device).await,
        ClaimPairing::Invalid
    );
    assert_eq!(external_devices(&db, &owner).await, 1);
    // Still listed, as expired.
    let listed = db.recent_pairings(&owner).await.unwrap();
    let row = listed.iter().find(|pairing| pairing.id == expired).unwrap();
    assert!(row.expired());
    assert_eq!(row.state, PairingState::Open);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn account_and_registered_keys_are_never_paired() {
    let db = database().await;
    let (owner, owner_key, pc, _) = owner(&db).await;
    let (other, other_key, other_pc, _) = self::owner(&db).await;
    let remote = device_key();

    // Account keys, the owner's own included.
    let (_, code) = start(&db, &owner, pc).await;
    for key in [owner_key, other_key] {
        assert_eq!(
            claim(&db, &code, &key, &remote).await,
            ClaimPairing::KeyInUse
        );
    }
    // A registered Kuzz agent (any owner) or an already paired key.
    let registered = agent_key();
    db.register_agent_key(
        &other,
        other_pc,
        &AgentKeyBackupRecord {
            agent_pubkey: registered,
            version: 1,
            key_id: "test".into(),
            nonce: vec![1; 12],
            ciphertext: vec![1; 48],
        },
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        claim(&db, &code, &registered, &remote).await,
        ClaimPairing::KeyInUse
    );
    assert_eq!(external_devices(&db, &owner).await, 0);
    // The code stays usable for a valid claim after refused ones.
    let paired = agent_key();
    connect(&db, &code, &paired, &remote).await;
    let (_, code) = start(&db, &owner, pc).await;
    assert_eq!(
        claim(&db, &code, &paired, &device_key()).await,
        ClaimPairing::KeyInUse
    );

    // An agent the backfill bound to this owner (no host, no copy) is paired
    // in place; one bound to another owner is not.
    let unbound = agent_key();
    let foreign = agent_key();
    for (key, principal) in [(unbound, &owner), (foreign, &other)] {
        sqlx::query(
            "INSERT INTO key_registry (pubkey, principal_id, role) VALUES ($1, $2, 'agent')",
        )
        .bind(key.as_slice())
        .bind(principal.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    }
    assert_eq!(
        claim(&db, &code, &foreign, &remote).await,
        ClaimPairing::KeyInUse
    );
    let device_id = connect(&db, &code, &unbound, &remote).await;
    let row: (String, Option<Uuid>) =
        sqlx::query_as("SELECT role, host_device_id FROM key_registry WHERE pubkey = $1")
            .bind(unbound.as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(row, ("external".to_owned(), Some(device_id)));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_login_never_revives_or_reuses_an_external_device() {
    let db = database().await;
    let (owner, _, pc, _) = owner(&db).await;
    let (agent, remote) = (agent_key(), device_key());
    let (_, code) = start(&db, &owner, pc).await;
    let external = connect(&db, &code, &agent, &remote).await;

    // The owner signs in on the paired computer with the same device key:
    // a separate login row; the external row is untouched.
    let signed_in = login(&db, &owner, &remote).await;
    assert_ne!(signed_in, external);
    assert_eq!(
        device_row(&db, signed_in).await,
        ("login".into(), "desktop".into(), false)
    );
    assert_eq!(
        device_row(&db, external).await,
        ("external".into(), "external".into(), false)
    );

    // The owner disconnects the computer; signing in there again does not
    // revive the external row, and the agent stays refused.
    assert!(db
        .revoke_device(&owner, external, RevokeReason::DeviceRevoked)
        .await
        .unwrap()
        .is_some());
    assert_eq!(login(&db, &owner, &remote).await, signed_in);
    assert_eq!(
        device_row(&db, external).await,
        ("external".into(), "external".into(), true)
    );
    assert_eq!(
        db.key_admission(&agent, Some(&remote)).await.unwrap(),
        KeyAdmission::Refused {
            role: KeyRole::External,
            reason: KeyRefusal::DeviceRevoked,
        }
    );
    // A new pairing from the computer makes a new external row.
    let (_, code) = start(&db, &owner, pc).await;
    let device_id = connect(&db, &code, &agent_key(), &remote).await;
    assert_ne!(device_id, external);
    assert_eq!(
        device_row(&db, external).await,
        ("external".into(), "external".into(), true)
    );
    // Login devices are listed with external ones (the owner revokes either).
    let platforms: Vec<String> = db
        .list_devices(&owner)
        .await
        .unwrap()
        .into_iter()
        .map(|device| device.platform)
        .collect();
    assert_eq!(platforms.iter().filter(|p| *p == "external").count(), 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn open_pairings_are_capped_per_owner() {
    let db = database().await;
    let (owner, _, pc, _) = owner(&db).await;
    for _ in 0..MAX_OPEN_PAIRINGS {
        start(&db, &owner, pc).await;
    }
    assert_eq!(
        db.start_pairing(&owner, pc, &rand::random()).await.unwrap(),
        StartPairing::TooMany
    );
    // Expired ones free their slot.
    sqlx::query(
        "UPDATE pairing_requests SET expires_at = now() - interval '2 days' \
          WHERE principal_id = $1",
    )
    .bind(owner.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    start(&db, &owner, pc).await;
    // ... and are dropped a day after expiry.
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pairing_requests WHERE principal_id = $1")
            .bind(owner.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(rows, 1);
}
