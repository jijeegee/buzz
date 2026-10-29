//! The backfill command against PostgreSQL: dry run, then two real runs.
use super::{backfill, verify_relay_pubkey, DeletionRecordBackfillCounts};
use kuzz_core::CommunityId;
use kuzz_db::Db;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

async fn store(
    db: &Db,
    community: CommunityId,
    keys: &Keys,
    kind: u16,
    tags: &[[&str; 2]],
) -> Event {
    store_at(db, community, keys, kind, tags, Timestamp::now()).await
}

async fn store_at(
    db: &Db,
    community: CommunityId,
    keys: &Keys,
    kind: u16,
    tags: &[[&str; 2]],
    created_at: Timestamp,
) -> Event {
    let channel: Option<Uuid> = tags
        .iter()
        .find(|t| t[0] == "h")
        .and_then(|t| t[1].parse().ok());
    let event = EventBuilder::new(Kind::Custom(kind), Uuid::new_v4().to_string())
        .tags(tags.iter().map(|t| Tag::parse(*t).expect("tag")))
        .custom_created_at(created_at)
        .sign_with_keys(keys)
        .expect("sign");
    db.insert_event(community, &event, channel)
        .await
        .expect("insert");
    event
}

async fn setup() -> (PgPool, Db, CommunityId) {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool = PgPool::connect(&url).await.expect("pool");
    let db = Db::from_pool(pool.clone());
    let community = CommunityId::from_uuid(Uuid::new_v4());
    sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
        .bind(community.as_uuid())
        .bind(format!("backfill-{}.test", community.as_uuid()))
        .execute(&pool)
        .await
        .expect("community");
    (pool, db, community)
}

async fn staff_action(
    pool: &PgPool,
    community: CommunityId,
    actor: &Keys,
    target: &Event,
    state: &str,
    step_marker: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO relay_admin_actions (report_community_id, request_id, actor_pubkey,            actor_role, action, state, step_marker, enforcement_target_event_id,            enforcement_target_pubkey)          VALUES ($1, $2, $3, 'operator', 'delete', $4, $5, $6, $7)",
    )
    .bind(community.as_uuid())
    .bind(Uuid::new_v4())
    .bind(actor.public_key().to_bytes().as_slice())
    .bind(state)
    .bind(step_marker)
    .bind(target.id.as_bytes().as_slice())
    .bind(target.pubkey.to_bytes().as_slice())
    .execute(pool)
    .await
    .expect("action");
}

async fn records(pool: &PgPool, community: CommunityId, relay: &Keys) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE community_id=$1 AND kind=9005 AND pubkey=$2",
    )
    .bind(community.as_uuid())
    .bind(relay.public_key().to_bytes().as_slice())
    .fetch_one(pool)
    .await
    .expect("count")
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_is_idempotent_and_dry_run_writes_nothing() {
    let (pool, db, community) = setup().await;
    let (author, owner, relay) = (Keys::generate(), Keys::generate(), Keys::generate());
    let channel = Uuid::new_v4().to_string();
    let h = ["h", channel.as_str()];

    // A staff delete (direct) and an agent-owner kind 5, both without records.
    let staffed = store(&db, community, &author, 9, &[h]).await;
    let owned = store(&db, community, &author, 9, &[h]).await;
    let owned_hex = owned.id.to_hex();
    store(&db, community, &owner, 5, &[h, ["e", owned_hex.as_str()]]).await;
    for event in [&staffed, &owned] {
        db.soft_delete_event_and_update_thread(community, event.id.as_bytes(), None, None)
            .await
            .expect("soft delete");
    }
    staff_action(&pool, community, &owner, &staffed, "succeeded", None).await;

    let dry = backfill(&db, community, &relay.public_key(), None, 1, |_, _| {})
        .await
        .expect("dry run");
    assert_eq!(
        dry,
        DeletionRecordBackfillCounts {
            candidates: 2,
            written: 0,
            skipped: 0
        }
    );
    assert_eq!(records(&pool, community, &relay).await, 0);

    let mut pages = 0;
    let first = backfill(
        &db,
        community,
        &relay.public_key(),
        Some(&relay),
        1,
        |_, _| pages += 1,
    )
    .await
    .expect("first run");
    assert_eq!(
        first,
        DeletionRecordBackfillCounts {
            candidates: 2,
            written: 2,
            skipped: 0
        }
    );
    assert_eq!(pages, 2, "one progress line per page");
    assert_eq!(records(&pool, community, &relay).await, 2);

    let second = backfill(
        &db,
        community,
        &relay.public_key(),
        Some(&relay),
        1,
        |_, _| {},
    )
    .await
    .expect("second run");
    assert_eq!(second, DeletionRecordBackfillCounts::default());
    assert_eq!(records(&pool, community, &relay).await, 2);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn backfill_covers_committed_failed_actions_and_future_dated_targets_once() {
    let (pool, db, community) = setup().await;
    let (author, owner, relay) = (Keys::generate(), Keys::generate(), Keys::generate());
    let channel = Uuid::new_v4().to_string();
    let h = ["h", channel.as_str()];
    // The mutation committed but a later step failed the action.
    let marked = store(&db, community, &author, 9, &[h]).await;
    // A client clock two days ahead.
    let ahead = Timestamp::from(Timestamp::now().as_secs() + 2 * 86_400);
    let future = store_at(&db, community, &author, 9, &[h], ahead).await;
    // Failed before the mutation: not a staff removal.
    let unmarked = store(&db, community, &author, 9, &[h]).await;
    for event in [&marked, &future, &unmarked] {
        db.soft_delete_event_and_update_thread(community, event.id.as_bytes(), None, None)
            .await
            .expect("soft delete");
    }
    staff_action(
        &pool,
        community,
        &owner,
        &marked,
        "failed",
        Some("mutation_committed"),
    )
    .await;
    staff_action(&pool, community, &owner, &future, "succeeded", None).await;
    staff_action(&pool, community, &owner, &unmarked, "failed", None).await;

    let relay_pubkey = relay.public_key();
    let run = || backfill(&db, community, &relay_pubkey, Some(&relay), 10, |_, _| {});
    let first = run().await.expect("first run");
    assert_eq!((first.candidates, first.written), (2, 2));
    let second = run().await.expect("second run");
    assert_eq!(second, DeletionRecordBackfillCounts::default());
    assert_eq!(records(&pool, community, &relay).await, 2);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn relay_pubkey_mismatch_is_refused() {
    let (_pool, db, community) = setup().await;
    let (relay, other) = (Keys::generate(), Keys::generate());
    let (expected, wrong) = (relay.public_key(), other.public_key());

    // No group state yet: the expected key and the signing key decide.
    assert_eq!(
        verify_relay_pubkey(&db, community, &expected, Some(&expected))
            .await
            .expect("match"),
        None
    );
    verify_relay_pubkey(&db, community, &expected, Some(&wrong))
        .await
        .expect_err("signing key differs from --expect-pubkey");

    let channel = Uuid::new_v4().to_string();
    store(&db, community, &relay, 39000, &[["d", channel.as_str()]]).await;
    assert_eq!(
        verify_relay_pubkey(&db, community, &expected, None)
            .await
            .expect("dry run reports the derived pubkey"),
        Some(expected)
    );
    let err = verify_relay_pubkey(&db, community, &wrong, Some(&wrong))
        .await
        .expect_err("group state is signed by another key");
    assert!(err.to_string().contains(&expected.to_hex()), "{err}");
    backfill(&db, community, &wrong, Some(&relay), 10, |_, _| {})
        .await
        .expect_err("writer must be the relay key");
}
