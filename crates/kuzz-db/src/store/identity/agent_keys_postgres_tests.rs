//! Agent registration, key copies and the 0080 registry backfill, against an
//! isolated test database only.
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

/// A key-backup account whose own key is a fresh random key.
async fn account(db: &Db) -> (PrincipalId, [u8; 32]) {
    let subject = uuid::Uuid::new_v4().to_string();
    let account = db
        .login_key_identity("google", &subject, None, "owner", None)
        .await
        .unwrap()
        .principal;
    let pubkey = nostr::Keys::generate().public_key().to_bytes();
    seed_account_key(db, &account, pubkey).await;
    (account, pubkey)
}

async fn seed_account_key(db: &Db, account: &PrincipalId, pubkey: [u8; 32]) {
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
}

/// A signed-in device of `account` holding `key`.
async fn device(db: &Db, account: &PrincipalId, key: &[u8; 33]) -> Uuid {
    db.complete_login_with_device_key(
        account,
        "pc",
        "desktop",
        None,
        Some(DeviceKey {
            pubkey: key,
            kind: "software",
        }),
        token(),
        token(),
    )
    .await
    .unwrap()
    .device_id
}

fn copy(agent: [u8; 32], fill: u8) -> AgentKeyBackupRecord {
    AgentKeyBackupRecord {
        agent_pubkey: agent,
        version: 1,
        key_id: "test".into(),
        nonce: vec![fill; 12],
        ciphertext: vec![fill; 48],
    }
}

async fn registry_row(db: &Db, agent: &[u8; 32]) -> Option<(Vec<u8>, Option<Uuid>)> {
    sqlx::query_as("SELECT principal_id, host_device_id FROM key_registry WHERE pubkey = $1")
        .bind(agent.as_slice())
        .fetch_optional(db.pool())
        .await
        .unwrap()
}

async fn copy_row(db: &Db, agent: &[u8; 32]) -> Option<(Option<Uuid>, Vec<u8>)> {
    sqlx::query_as("SELECT host_device_id, nonce FROM agent_key_backups WHERE agent_pubkey = $1")
        .bind(agent.as_slice())
        .fetch_optional(db.pool())
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn register_binds_owner_and_host_and_moves_only_when_asked() {
    let db = database().await;
    let (owner, _) = account(&db).await;
    let (pc_key, laptop_key) = (device_key(), device_key());
    let pc = device(&db, &owner, &pc_key).await;
    let laptop = device(&db, &owner, &laptop_key).await;
    let agent = nostr::Keys::generate().public_key().to_bytes();

    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(agent, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::Registered
    );
    assert_eq!(
        registry_row(&db, &agent).await,
        Some((owner.as_bytes().to_vec(), Some(pc)))
    );
    assert_eq!(copy_row(&db, &agent).await, Some((Some(pc), vec![1; 12])));
    // Same owner and host: idempotent; the copy is renewed.
    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(agent, 2), false)
            .await
            .unwrap(),
        RegisterAgentKey::Unchanged
    );
    assert_eq!(copy_row(&db, &agent).await, Some((Some(pc), vec![2; 12])));
    // Another of the owner's devices: refused without `move`, nothing written.
    assert_eq!(
        db.register_agent_key(&owner, laptop, &copy(agent, 3), false)
            .await
            .unwrap(),
        RegisterAgentKey::OtherHost
    );
    assert_eq!(copy_row(&db, &agent).await, Some((Some(pc), vec![2; 12])));
    assert_eq!(
        db.register_agent_key(&owner, laptop, &copy(agent, 4), true)
            .await
            .unwrap(),
        RegisterAgentKey::Moved
    );
    assert_eq!(
        registry_row(&db, &agent).await,
        Some((owner.as_bytes().to_vec(), Some(laptop)))
    );
    assert_eq!(
        copy_row(&db, &agent).await,
        Some((Some(laptop), vec![4; 12]))
    );

    // The copy is listed for its host only.
    assert!(db.device_agent_keys(&owner, pc).await.unwrap().is_empty());
    let listed = db.device_agent_keys(&owner, laptop).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].agent_pubkey, agent);
    assert_eq!(listed[0].nonce, vec![4; 12]);

    // A revoked agent is not registered again and not listed.
    sqlx::query("UPDATE key_registry SET revoked_at = now() WHERE pubkey = $1")
        .bind(agent.as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.register_agent_key(&owner, laptop, &copy(agent, 5), true)
            .await
            .unwrap(),
        RegisterAgentKey::Revoked
    );
    assert!(db
        .device_agent_keys(&owner, laptop)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn register_never_reowns_or_changes_a_role() {
    let db = database().await;
    let (owner, owner_key) = account(&db).await;
    let (other, _) = account(&db).await;
    let pc = device(&db, &owner, &device_key()).await;
    let other_pc = device(&db, &other, &device_key()).await;
    let agent = nostr::Keys::generate().public_key().to_bytes();
    db.register_agent_key(&owner, pc, &copy(agent, 1), false)
        .await
        .unwrap();

    // Another account, even with `move`.
    assert_eq!(
        db.register_agent_key(&other, other_pc, &copy(agent, 2), true)
            .await
            .unwrap(),
        RegisterAgentKey::OtherOwner
    );
    assert_eq!(
        registry_row(&db, &agent).await,
        Some((owner.as_bytes().to_vec(), Some(pc)))
    );
    // An account key (the owner's own, too) is never an agent.
    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(owner_key, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::OtherRole
    );
    // An external agent key stays external.
    let external = nostr::Keys::generate().public_key().to_bytes();
    sqlx::query(
        "INSERT INTO key_registry (pubkey, principal_id, role, host_device_id) \
         VALUES ($1, $2, 'external', $3)",
    )
    .bind(external.as_slice())
    .bind(owner.as_bytes().as_slice())
    .bind(pc)
    .execute(db.pool())
    .await
    .unwrap();
    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(external, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::OtherRole
    );
    // Another account's device, or a revoked one, is not a host.
    let fresh = nostr::Keys::generate().public_key().to_bytes();
    assert_eq!(
        db.register_agent_key(&owner, other_pc, &copy(fresh, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::DeviceGone
    );
    sqlx::query("UPDATE devices SET revoked_at = now() WHERE id = $1")
        .bind(pc)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(fresh, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::DeviceGone
    );
    assert_eq!(registry_row(&db, &fresh).await, None);
    assert_eq!(copy_row(&db, &fresh).await, None);
}

/// The S1 backfill statement of migration 0080, run again on seeded rows.
fn backfill_sql() -> &'static str {
    let migration = include_str!("../../../../../migrations/0080_agent_key_backups.sql");
    let start = migration.find("-- S1 backfill").expect("backfill marker");
    &migration[start..]
}

async fn seed_user(
    db: &Db,
    community: Uuid,
    pubkey: &[u8; 32],
    owner: Option<&[u8; 32]>,
    age: i32,
) {
    sqlx::query(
        "INSERT INTO users (community_id, pubkey, agent_owner_pubkey, created_at) \
         VALUES ($1, $2, $3, now() - make_interval(days => $4))",
    )
    .bind(community)
    .bind(pubkey.as_slice())
    .bind(owner.map(|owner| owner.to_vec()))
    .bind(age)
    .execute(db.pool())
    .await
    .unwrap();
}

async fn seed_community(db: &Db) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(id)
        .bind(format!("{}.agents.test", id.simple()))
        .execute(db.pool())
        .await
        .unwrap();
    id
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_binds_attested_agents_unbound_until_registered() {
    let db = database().await;
    let (owner_a, key_a) = account(&db).await;
    let (owner_b, key_b) = account(&db).await;
    let (c1, c2) = (seed_community(&db).await, seed_community(&db).await);
    for community in [c1, c2] {
        seed_user(&db, community, &key_a, None, 10).await;
        seed_user(&db, community, &key_b, None, 10).await;
    }
    let agent = nostr::Keys::generate().public_key().to_bytes();
    seed_user(&db, c1, &agent, Some(&key_a), 1).await;
    // Two communities name different owners: skipped, registered explicitly.
    let disputed = nostr::Keys::generate().public_key().to_bytes();
    seed_user(&db, c1, &disputed, Some(&key_a), 1).await;
    seed_user(&db, c2, &disputed, Some(&key_b), 5).await;
    // An owner without a key-backup account binds nothing.
    let stranger = nostr::Keys::generate().public_key().to_bytes();
    seed_user(&db, c1, &stranger, None, 10).await;
    let orphan = nostr::Keys::generate().public_key().to_bytes();
    seed_user(&db, c1, &orphan, Some(&stranger), 1).await;
    // An account key attested as an agent stays a person key.
    let (_, person_key) = account(&db).await;
    seed_user(&db, c1, &person_key, Some(&key_a), 1).await;

    for _ in 0..2 {
        // Idempotent: the second run changes nothing.
        sqlx::raw_sql(backfill_sql())
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(
            registry_row(&db, &agent).await,
            Some((owner_a.as_bytes().to_vec(), None))
        );
        assert_eq!(registry_row(&db, &disputed).await, None);
        assert_eq!(registry_row(&db, &orphan).await, None);
        assert_eq!(registry_row(&db, &person_key).await, None);
    }

    // Unbound: refused even on the owner's signed-in device.
    let pc_key = device_key();
    let pc = device(&db, &owner_a, &pc_key).await;
    assert_eq!(
        db.key_admission(&agent, Some(&pc_key)).await.unwrap(),
        KeyAdmission::Refused {
            role: KeyRole::Agent,
            reason: KeyRefusal::Unbound,
        }
    );
    // Another account cannot claim it; its owner binds it from the device.
    let pc_b = device(&db, &owner_b, &device_key()).await;
    assert_eq!(
        db.register_agent_key(&owner_b, pc_b, &copy(agent, 1), true)
            .await
            .unwrap(),
        RegisterAgentKey::OtherOwner
    );
    assert_eq!(
        db.register_agent_key(&owner_a, pc, &copy(agent, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::Registered
    );
    assert_eq!(
        db.key_admission(&agent, Some(&pc_key)).await.unwrap(),
        KeyAdmission::Admitted {
            role: KeyRole::Agent,
            principal: owner_a,
            device_id: pc,
            owner_pubkey: Some(key_a),
        }
    );
    // The disputed agent: neither named owner may register it while any
    // profile names the other.
    for (owner, device) in [(&owner_a, pc), (&owner_b, pc_b)] {
        assert_eq!(
            db.register_agent_key(owner, device, &copy(disputed, 1), false)
                .await
                .unwrap(),
            RegisterAgentKey::OtherOwner
        );
    }
    assert_eq!(registry_row(&db, &disputed).await, None);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn register_refuses_an_owner_other_than_the_profiles_name() {
    let db = database().await;
    let (owner_a, key_a) = account(&db).await;
    let (owner_b, _) = account(&db).await;
    let community = seed_community(&db).await;
    seed_user(&db, community, &key_a, None, 10).await;
    // Attested after the one-shot backfill: no registry row yet.
    let agent = nostr::Keys::generate().public_key().to_bytes();
    seed_user(&db, community, &agent, Some(&key_a), 1).await;
    let pc_a = device(&db, &owner_a, &device_key()).await;
    let pc_b = device(&db, &owner_b, &device_key()).await;

    assert_eq!(
        db.register_agent_key(&owner_b, pc_b, &copy(agent, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::OtherOwner
    );
    assert_eq!(registry_row(&db, &agent).await, None);
    assert_eq!(copy_row(&db, &agent).await, None);
    assert_eq!(
        db.register_agent_key(&owner_a, pc_a, &copy(agent, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::Registered
    );
    // A profile with no owner named does not block anyone.
    let unattested = nostr::Keys::generate().public_key().to_bytes();
    seed_user(&db, community, &unattested, None, 1).await;
    assert_eq!(
        db.register_agent_key(&owner_b, pc_b, &copy(unattested, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::Registered
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn register_caps_live_agents_per_device() {
    let db = database().await;
    let (owner, _) = account(&db).await;
    let pc = device(&db, &owner, &device_key()).await;
    let laptop = device(&db, &owner, &device_key()).await;
    let kept = nostr::Keys::generate().public_key().to_bytes();
    db.register_agent_key(&owner, pc, &copy(kept, 1), false)
        .await
        .unwrap();
    // Fill the device up to the cap with live agents.
    sqlx::query(
        "INSERT INTO key_registry (pubkey, principal_id, role, host_device_id) \
         SELECT decode(md5(random()::text || g::text) || md5(g::text || random()::text), 'hex'), \
                $1, 'agent', $2 \
           FROM generate_series(1, $3) AS g",
    )
    .bind(owner.as_bytes().as_slice())
    .bind(pc)
    .bind((MAX_AGENT_KEYS_PER_DEVICE - 1) as i32)
    .execute(db.pool())
    .await
    .unwrap();

    let extra = nostr::Keys::generate().public_key().to_bytes();
    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(extra, 1), false)
            .await
            .unwrap(),
        RegisterAgentKey::DeviceFull
    );
    assert_eq!(registry_row(&db, &extra).await, None);
    // Renewing a hosted agent's copy is not a new slot.
    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(kept, 2), false)
            .await
            .unwrap(),
        RegisterAgentKey::Unchanged
    );
    // Moving in is refused too; another device still has room.
    db.register_agent_key(&owner, laptop, &copy(extra, 1), false)
        .await
        .unwrap();
    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(extra, 2), true)
            .await
            .unwrap(),
        RegisterAgentKey::DeviceFull
    );
    // A removed agent frees its slot.
    sqlx::query("UPDATE key_registry SET revoked_at = now() WHERE pubkey = $1")
        .bind(kept.as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.register_agent_key(&owner, pc, &copy(extra, 3), true)
            .await
            .unwrap(),
        RegisterAgentKey::Moved
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn owned_keys_show_only_the_owners_live_keys() {
    let db = database().await;
    let (owner, owner_key) = account(&db).await;
    let (other, other_key) = account(&db).await;
    let pc_key = device_key();
    let pc = device(&db, &owner, &pc_key).await;
    let agent = nostr::Keys::generate().public_key().to_bytes();
    db.register_agent_key(&owner, pc, &copy(agent, 1), false)
        .await
        .unwrap();
    let foreign = nostr::Keys::generate().public_key().to_bytes();
    let other_pc = device(&db, &other, &device_key()).await;
    db.register_agent_key(&other, other_pc, &copy(foreign, 1), false)
        .await
        .unwrap();
    let unknown = nostr::Keys::generate().public_key().to_bytes();

    let mut owned = db
        .owned_keys(&owner, &[owner_key, agent, foreign, other_key, unknown])
        .await
        .unwrap();
    owned.sort_by_key(|key| key.role.as_str());
    assert_eq!(
        owned,
        [
            OwnedKey {
                pubkey: agent,
                role: KeyRole::Agent,
                host_device_pubkey: Some(pc_key),
            },
            OwnedKey {
                pubkey: owner_key,
                role: KeyRole::Person,
                host_device_pubkey: None,
            },
        ]
    );
    assert_eq!(db.account_pubkey(&owner).await.unwrap(), Some(owner_key));
}
