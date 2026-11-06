//! Owner tools switch, agent key revocation and the agent action log
//! (migration 0100), against an isolated test database only.
use super::*;
use crate::Db;
use chrono::{Duration, Utc};
use kuzz_core::CommunityId;

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

/// A key-backup account whose own key is `keys`.
async fn account(db: &Db, keys: &nostr::Keys) -> PrincipalId {
    let subject = uuid::Uuid::new_v4().to_string();
    let account = db
        .login_key_identity("google", &subject, None, "owner", None)
        .await
        .unwrap()
        .principal;
    sqlx::query(
        "INSERT INTO account_key_backups (account_id, pubkey, version, key_id, nonce, ciphertext) \
         VALUES ($1, $2, 2, 'k1', $3, $4)",
    )
    .bind(account.as_bytes().as_slice())
    .bind(keys.public_key().to_bytes().as_slice())
    .bind([0u8; 12].as_slice())
    .bind([0u8; 48].as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    account
}

/// A signed-in device of `account`.
async fn device(db: &Db, account: &PrincipalId) -> Uuid {
    db.complete_login_with_device_key(
        account,
        "pc",
        "desktop",
        None,
        Some(DeviceKey {
            pubkey: &device_key(),
            kind: "software",
        }),
        token(),
        token(),
    )
    .await
    .unwrap()
    .device_id
}

/// `agent` registered as `owner`'s app agent on `device`.
async fn agent_on(db: &Db, owner: &PrincipalId, device: Uuid) -> [u8; 32] {
    let agent = nostr::Keys::generate().public_key().to_bytes();
    register(db, owner, device, agent).await;
    agent
}

async fn register(db: &Db, owner: &PrincipalId, device: Uuid, agent: [u8; 32]) -> RegisterAgentKey {
    db.register_agent_key(
        owner,
        device,
        &AgentKeyBackupRecord {
            agent_pubkey: agent,
            version: 1,
            key_id: "test".into(),
            nonce: vec![1; 12],
            ciphertext: vec![1; 48],
        },
        true,
    )
    .await
    .unwrap()
}

async fn community(db: &Db) -> CommunityId {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(id)
        .bind(format!("tools-{}.invalid", id.simple()))
        .execute(db.pool())
        .await
        .unwrap();
    CommunityId::from_uuid(id)
}

fn write(
    level: ToolLevel,
    class: RespondClass,
    community: CommunityId,
    device: Uuid,
) -> ToolLevelWrite {
    ToolLevelWrite {
        level,
        respond_class: class,
        default_only: false,
        community,
        device,
    }
}

async fn stored(db: &Db, agent: &[u8; 32]) -> (Option<String>, Option<String>, Option<Uuid>) {
    sqlx::query_as(
        "SELECT tool_level, respond_class, tool_community_id FROM key_registry WHERE pubkey = $1",
    )
    .bind(agent.as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap()
}

fn some(
    level: &str,
    class: &str,
    community: CommunityId,
) -> (Option<String>, Option<String>, Option<Uuid>) {
    (
        Some(level.into()),
        Some(class.into()),
        Some(*community.as_uuid()),
    )
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn the_owner_sets_the_switch_and_a_default_never_overwrites_it() {
    let db = database().await;
    let owner = account(&db, &nostr::Keys::generate()).await;
    let (pc, phone) = (device(&db, &owner).await, device(&db, &owner).await);
    let here = community(&db).await;
    let agent = agent_on(&db, &owner, pc).await;
    use RespondClass::*;
    use ToolLevel::*;

    // A default write over a level never set is stored.
    let mut default = write(Full, OwnerOnly, here, pc);
    default.default_only = true;
    assert_eq!(
        db.set_agent_tool_level(&owner, &agent, &default)
            .await
            .unwrap(),
        SetToolLevel::Set(Full)
    );
    assert_eq!(stored(&db, &agent).await, some("full", "owner-only", here));

    // The owner turns it off; a later default (another PC, a reinstall)
    // reads the stored answer and changes nothing.
    assert_eq!(
        db.set_agent_tool_level(&owner, &agent, &write(Limited, OwnerOnly, here, pc))
            .await
            .unwrap(),
        SetToolLevel::Set(Limited)
    );
    assert_eq!(
        db.set_agent_tool_level(&owner, &agent, &default)
            .await
            .unwrap(),
        SetToolLevel::AlreadySet(Limited)
    );
    assert_eq!(
        stored(&db, &agent).await,
        some("limited", "owner-only", here)
    );

    // Full never with anyone.
    assert_eq!(
        db.set_agent_tool_level(&owner, &agent, &write(Full, Anyone, here, pc))
            .await
            .unwrap(),
        SetToolLevel::FullAnyone
    );

    // Another owner's key and an unknown key are not found.
    let stranger = account(&db, &nostr::Keys::generate()).await;
    assert_eq!(
        db.set_agent_tool_level(&stranger, &agent, &write(Full, OwnerOnly, here, pc))
            .await
            .unwrap(),
        SetToolLevel::NotFound
    );
    let unknown = nostr::Keys::generate().public_key().to_bytes();
    assert_eq!(
        db.set_agent_tool_level(&owner, &unknown, &write(Full, OwnerOnly, here, pc))
            .await
            .unwrap(),
        SetToolLevel::NotFound
    );

    // The phone is not the app agent's host: it may turn it on only with the
    // class the relay already has, and never changes that class.
    assert_eq!(
        db.set_agent_tool_level(&owner, &agent, &write(Full, Allowlist, here, phone))
            .await
            .unwrap(),
        SetToolLevel::ClassStale
    );
    assert_eq!(
        db.set_agent_tool_level(&owner, &agent, &write(Limited, Allowlist, here, phone))
            .await
            .unwrap(),
        SetToolLevel::Set(Limited)
    );
    assert_eq!(
        stored(&db, &agent).await,
        some("limited", "owner-only", here)
    );
    assert_eq!(
        db.set_agent_tool_level(&owner, &agent, &write(Full, OwnerOnly, here, phone))
            .await
            .unwrap(),
        SetToolLevel::Set(Full)
    );

    // A never-set agent has no stored class: the phone cannot turn it on.
    let fresh = agent_on(&db, &owner, pc).await;
    assert_eq!(
        db.set_agent_tool_level(&owner, &fresh, &write(Full, OwnerOnly, here, phone))
            .await
            .unwrap(),
        SetToolLevel::ClassStale
    );

    // A paired agent's class is only its owner's signed record: no device
    // supplies it, and `full` needs the class sent to equal the stored one.
    let paired = nostr::Keys::generate().public_key().to_bytes();
    sqlx::query(
        "INSERT INTO key_registry (pubkey, principal_id, role, host_device_id) \
         VALUES ($1, $2, 'external', $3)",
    )
    .bind(paired.as_slice())
    .bind(owner.as_bytes().as_slice())
    .bind(pc)
    .execute(db.pool())
    .await
    .unwrap();
    // Unknown class: refused even from the paired computer; limited works
    // and stores no class.
    assert_eq!(
        db.set_agent_tool_level(&owner, &paired, &write(Full, OwnerOnly, here, pc))
            .await
            .unwrap(),
        SetToolLevel::ClassStale
    );
    assert_eq!(
        db.set_agent_tool_level(&owner, &paired, &write(Limited, OwnerOnly, here, pc))
            .await
            .unwrap(),
        SetToolLevel::Set(Limited)
    );
    assert_eq!(
        stored(&db, &paired).await,
        (Some("limited".into()), None, Some(*here.as_uuid()))
    );
    // The desktop records anyone (30177); a stale client still asks for
    // full + owner-only: refused, nothing changes.
    sqlx::query("UPDATE key_registry SET respond_class = 'anyone' WHERE pubkey = $1")
        .bind(paired.as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.set_agent_tool_level(&owner, &paired, &write(Full, OwnerOnly, here, phone))
            .await
            .unwrap(),
        SetToolLevel::ClassStale
    );
    assert_eq!(stored(&db, &paired).await, some("limited", "anyone", here));
    // Once the record says allowlist, full with allowlist is set.
    sqlx::query("UPDATE key_registry SET respond_class = 'allowlist' WHERE pubkey = $1")
        .bind(paired.as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        db.set_agent_tool_level(&owner, &paired, &write(Full, Allowlist, here, phone))
            .await
            .unwrap(),
        SetToolLevel::Set(Full)
    );
    assert_eq!(stored(&db, &paired).await, some("full", "allowlist", here));

    // The batch read: the owner's live keys only.
    let mut levels = db
        .agent_tool_levels(&owner, &[agent, fresh, unknown, paired])
        .await
        .unwrap();
    levels.sort_by_key(|state| state.pubkey != agent);
    assert_eq!(levels.len(), 3);
    assert_eq!(levels[0].pubkey, agent);
    assert_eq!(levels[0].level, Some(Full));
    assert_eq!(levels[0].community, Some(here));
    assert_eq!(levels[0].host_device_id, Some(pc));
    assert_eq!(levels[0].host_device_name.as_deref(), Some("pc"));
    assert!(db
        .agent_tool_levels(&stranger, &[agent])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn register_keeps_the_level_and_re_pairing_resets_it() {
    let db = database().await;
    let owner = account(&db, &nostr::Keys::generate()).await;
    let (pc, laptop) = (device(&db, &owner).await, device(&db, &owner).await);
    let here = community(&db).await;
    let agent = agent_on(&db, &owner, pc).await;
    db.set_agent_tool_level(
        &owner,
        &agent,
        &write(ToolLevel::Full, RespondClass::OwnerOnly, here, pc),
    )
    .await
    .unwrap();
    assert_eq!(
        register(&db, &owner, pc, agent).await,
        RegisterAgentKey::Unchanged
    );
    assert_eq!(
        register(&db, &owner, laptop, agent).await,
        RegisterAgentKey::Moved
    );
    assert_eq!(stored(&db, &agent).await, some("full", "owner-only", here));

    // A paired agent set on, disconnected, then paired again starts over.
    let paired = nostr::Keys::generate().public_key().to_bytes();
    let computer = device_key();
    let pair = |code_hash: [u8; 32]| {
        let db = db.clone();
        async move {
            db.claim_pairing(&PairingClaim {
                code_hash: &code_hash,
                agent_pubkey: &paired,
                device_pubkey: &computer,
                device_key_kind: "software",
                device_name: None,
                runtime: None,
            })
            .await
            .unwrap()
        }
    };
    let code_hash: [u8; 32] = rand::random();
    db.start_pairing(&owner, pc, &code_hash, false)
        .await
        .unwrap();
    let ClaimPairing::Connected { device_id, .. } = pair(code_hash).await else {
        panic!("first pairing");
    };
    assert_eq!(stored(&db, &paired).await, (None, None, None));
    assert_eq!(
        db.set_agent_tool_level(
            &owner,
            &paired,
            &write(ToolLevel::Limited, RespondClass::OwnerOnly, here, pc),
        )
        .await
        .unwrap(),
        SetToolLevel::Set(ToolLevel::Limited)
    );
    db.disconnect_external_agent(&owner, device_id, &paired)
        .await
        .unwrap()
        .expect("disconnected");
    let code_hash: [u8; 32] = rand::random();
    db.start_pairing(&owner, pc, &code_hash, false)
        .await
        .unwrap();
    assert!(matches!(
        pair(code_hash).await,
        ClaimPairing::Connected { .. }
    ));
    assert_eq!(stored(&db, &paired).await, (None, None, None));

    // A backfilled agent that becomes external by pairing starts over too.
    let backfilled = nostr::Keys::generate().public_key().to_bytes();
    sqlx::query(
        "INSERT INTO key_registry (pubkey, principal_id, role, tool_level, tool_community_id, \
                respond_class) VALUES ($1, $2, 'agent', 'full', $3, 'owner-only')",
    )
    .bind(backfilled.as_slice())
    .bind(owner.as_bytes().as_slice())
    .bind(here.as_uuid())
    .execute(db.pool())
    .await
    .unwrap();
    let code_hash: [u8; 32] = rand::random();
    db.start_pairing(&owner, pc, &code_hash, false)
        .await
        .unwrap();
    let claimed = db
        .claim_pairing(&PairingClaim {
            code_hash: &code_hash,
            agent_pubkey: &backfilled,
            device_pubkey: &computer,
            device_key_kind: "software",
            device_name: None,
            runtime: None,
        })
        .await
        .unwrap();
    assert!(
        matches!(claimed, ClaimPairing::Connected { .. }),
        "{claimed:?}"
    );
    assert_eq!(stored(&db, &backfilled).await, (None, None, None));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn revoke_is_for_the_host_only_and_for_good() {
    let db = database().await;
    let owner = account(&db, &nostr::Keys::generate()).await;
    let (pc, laptop) = (device(&db, &owner).await, device(&db, &owner).await);
    let here = community(&db).await;
    let agent = agent_on(&db, &owner, pc).await;
    db.set_agent_tool_level(
        &owner,
        &agent,
        &write(ToolLevel::Full, RespondClass::OwnerOnly, here, pc),
    )
    .await
    .unwrap();
    let stranger = account(&db, &nostr::Keys::generate()).await;
    let stranger_pc = device(&db, &stranger).await;

    assert_eq!(
        db.revoke_agent_key(&owner, &agent, laptop).await.unwrap(),
        RevokeAgentKey::NotHost
    );
    assert_eq!(
        db.revoke_agent_key(&stranger, &agent, stranger_pc)
            .await
            .unwrap(),
        RevokeAgentKey::NotFound
    );
    assert_eq!(stored(&db, &agent).await, some("full", "owner-only", here));

    assert_eq!(
        db.revoke_agent_key(&owner, &agent, pc).await.unwrap(),
        RevokeAgentKey::Revoked
    );
    assert_eq!(stored(&db, &agent).await, (None, None, None));
    let copies: i64 =
        sqlx::query_scalar("SELECT count(*) FROM agent_key_backups WHERE agent_pubkey = $1")
            .bind(agent.as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(copies, 0);
    assert_eq!(
        db.revoke_agent_key(&owner, &agent, pc).await.unwrap(),
        RevokeAgentKey::AlreadyRevoked
    );
    // Never registered again, and the switch is gone with it.
    assert_eq!(
        register(&db, &owner, pc, agent).await,
        RegisterAgentKey::Revoked
    );
    assert_eq!(
        db.set_agent_tool_level(
            &owner,
            &agent,
            &write(ToolLevel::Full, RespondClass::OwnerOnly, here, pc)
        )
        .await
        .unwrap(),
        SetToolLevel::NotFound
    );
    let authority = db.key_authority(&agent, None).await.unwrap();
    assert!(matches!(
        authority.admission,
        KeyAdmission::Refused {
            reason: KeyRefusal::KeyRevoked,
            ..
        }
    ));
    assert!(authority.tool.expect("registry row").revoked);
}

fn managed_agent(
    author: &nostr::Keys,
    agent: &[u8; 32],
    respond_to: &str,
    created_at: u64,
) -> nostr::Event {
    nostr::EventBuilder::new(
        nostr::Kind::Custom(30177),
        format!(r#"{{"display_name":"x","respond_to":"{respond_to}"}}"#),
    )
    .tags([nostr::Tag::identifier(hex::encode(agent))])
    .custom_created_at(nostr::Timestamp::from(created_at))
    .sign_with_keys(author)
    .unwrap()
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn the_owners_live_record_keeps_the_class_and_anyone_turns_it_off() {
    let db = database().await;
    let owner_keys = nostr::Keys::generate();
    let owner = account(&db, &owner_keys).await;
    let pc = device(&db, &owner).await;
    let (here, elsewhere) = (community(&db).await, community(&db).await);
    let agent = agent_on(&db, &owner, pc).await;
    let d = hex::encode(agent);
    let now = Utc::now().timestamp() as u64;
    db.set_agent_tool_level(
        &owner,
        &agent,
        &write(ToolLevel::Full, RespondClass::OwnerOnly, here, pc),
    )
    .await
    .unwrap();
    let store = |event: nostr::Event, community: CommunityId| {
        let db = db.clone();
        let d = d.clone();
        async move {
            db.replace_managed_agent_event(community, &event, &d, None)
                .await
                .unwrap()
                .1
        }
    };

    // Someone else's record names the agent: nothing changes.
    let stranger = nostr::Keys::generate();
    assert!(store(managed_agent(&stranger, &agent, "anyone", now - 50), here).await);
    assert_eq!(stored(&db, &agent).await, some("full", "owner-only", here));

    // The owner's allowlist record in another community does not move it.
    assert!(
        store(
            managed_agent(&owner_keys, &agent, "allowlist", now - 40),
            elsewhere
        )
        .await
    );
    assert_eq!(stored(&db, &agent).await, some("full", "owner-only", here));

    // Here it does, and the level stays.
    assert!(
        store(
            managed_agent(&owner_keys, &agent, "allowlist", now - 30),
            here
        )
        .await
    );
    assert_eq!(stored(&db, &agent).await, some("full", "allowlist", here));

    // Anyone, in any community, turns it off.
    assert!(
        store(
            managed_agent(&owner_keys, &agent, "anyone", now - 20),
            elsewhere
        )
        .await
    );
    assert_eq!(stored(&db, &agent).await, some("limited", "anyone", here));

    // An older record is not stored and changes nothing.
    assert!(
        !store(
            managed_agent(&owner_keys, &agent, "owner-only", now - 25),
            elsewhere
        )
        .await
    );
    assert_eq!(stored(&db, &agent).await, some("limited", "anyone", here));

    // The owner narrows it again: the class follows, the level stays off.
    assert!(
        store(
            managed_agent(&owner_keys, &agent, "owner-only", now - 10),
            here
        )
        .await
    );
    assert_eq!(
        stored(&db, &agent).await,
        some("limited", "owner-only", here)
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn agent_actions_are_kept_ninety_days_and_go_with_the_account() {
    let db = database().await;
    let owner_keys = nostr::Keys::generate();
    let owner = account(&db, &owner_keys).await;
    let pc = device(&db, &owner).await;
    let here = community(&db).await;
    let agent = agent_on(&db, &owner, pc).await;
    let other_owner = nostr::Keys::generate().public_key().to_bytes();
    let other_agent = nostr::Keys::generate().public_key().to_bytes();
    let log = |owner_pubkey: [u8; 32], agent_pubkey: [u8; 32]| {
        let db = db.clone();
        async move {
            db.insert_agent_action(&NewAgentAction {
                community: here,
                owner_pubkey,
                agent_pubkey,
                action: "tool_level.set",
                target: Some("full"),
                computer: Some("pc"),
                status: ActionStatus::Ok,
                source: ActionSource::Relay,
            })
            .await
            .unwrap()
        }
    };
    let owner_key = owner_keys.public_key().to_bytes();
    log(owner_key, agent).await;
    log(owner_key, agent).await;
    log(other_owner, other_agent).await;
    let count = |key: [u8; 32]| {
        let db = db.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM agent_actions WHERE owner_pubkey = $1 OR agent_pubkey = $1",
            )
            .bind(key.as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap()
        }
    };

    // Retention: only rows past the cutoff go, in small batches.
    sqlx::query(
        "UPDATE agent_actions SET created_at = now() - interval '91 days' \
          WHERE community_id = $1 AND owner_pubkey = $2",
    )
    .bind(here.as_uuid())
    .bind(other_owner.as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let purged = db
        .purge_agent_actions_before(Utc::now() - Duration::days(90), 1)
        .await
        .unwrap();
    assert!(purged >= 1);
    assert_eq!(count(other_owner).await, 0);
    assert_eq!(count(owner_key).await, 2);

    // Account purge: the owner's log goes, the level with the key.
    db.set_agent_tool_level(
        &owner,
        &agent,
        &write(ToolLevel::Full, RespondClass::OwnerOnly, here, pc),
    )
    .await
    .unwrap();
    log(other_owner, other_agent).await;
    db.disable_principal(&owner, Some(Duration::days(30)))
        .await
        .unwrap();
    sqlx::query("UPDATE principals SET purge_after = now() - interval '1 second' WHERE id = $1")
        .bind(owner.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    assert!(matches!(
        db.purge_account(&owner, false).await.unwrap(),
        PurgeOutcome::Purged(_)
    ));
    assert_eq!(count(owner_key).await, 0);
    assert_eq!(count(agent).await, 0);
    assert_eq!(count(other_owner).await, 1);
    assert_eq!(stored(&db, &agent).await, (None, None, None));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn agent_actions_are_community_scoped_and_fenced() {
    let db = database().await;
    for catalog in [
        crate::deletion::EXPECTED_SCOPED_TABLES,
        crate::deletion::PURGE_SCOPED_TABLES,
    ] {
        assert!(catalog.contains(&"agent_actions"));
    }
    let fenced: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_trigger \
          WHERE tgrelid = 'agent_actions'::regclass \
            AND tgname = 'community_write_fence_agent_actions')",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(fenced);
    // key_registry stays operator-global: its community column is not
    // `community_id`, so no fence and no scoped-table entry.
    let registry_scoped: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
          WHERE table_name = 'key_registry' AND column_name = 'community_id')",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(!registry_scoped);
}
