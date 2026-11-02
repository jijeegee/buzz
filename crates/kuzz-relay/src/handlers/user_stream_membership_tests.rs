//! User-stream rooms following membership (U1b), against Postgres.

use std::collections::BTreeSet;

use kuzz_core::channel::MemberRole;
use kuzz_pubsub::cache_invalidation::CacheInvalidation;

use super::super::*;
use super::db::*;

/// Open a stream on `s` through the REQ and drain its parts and EOSE.
async fn live_stream(h: &mut Harness) {
    h.req("s", vec![request()], None).await;
    let frames = h.frames();
    assert_eq!(frames.last().expect("frames")[0], "EOSE", "{frames:?}");
}

fn carried(h: &Harness) -> BTreeSet<Uuid> {
    h.state
        .sub_registry
        .stream_rooms(h.conn.conn_id)
        .into_iter()
        .collect()
}

async fn set_removed(h: &Harness, room: Uuid, removed: bool) {
    sqlx::query(
        "UPDATE channel_members SET removed_at = CASE WHEN $4 THEN now() ELSE NULL END
         WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3",
    )
    .bind(h.community.as_uuid())
    .bind(room)
    .bind(h.pubkey())
    .bind(removed)
    .execute(&h.pool)
    .await
    .expect("update membership");
}

async fn add_member_row(h: &Harness, room: Uuid) {
    sqlx::query(
        "INSERT INTO channel_members (community_id, channel_id, pubkey, role)
         VALUES ($1, $2, $3, 'member')",
    )
    .bind(h.community.as_uuid())
    .bind(room)
    .bind(h.pubkey())
    .execute(&h.pool)
    .await
    .expect("add member");
}

/// Wait (bounded) until `check` holds.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_join_rides_the_stream_from_its_next_message() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(false, 0).await;
    live_stream(&mut h).await;
    assert!(carried(&h).is_empty());

    let owner = vec![0x41u8; 32];
    crate::handlers::side_effects::add_channel_member(
        &h.conn.tenant,
        &h.state,
        room,
        &h.pubkey(),
        MemberRole::Member,
        None,
        &owner,
    )
    .await
    .expect("join");
    // Carried by the time the join returns (the hook runs before the 44100).
    assert_eq!(carried(&h), BTreeSet::from([room]));
    h.frames();

    let id = h.message(room, "first after join").await;
    let frames = h.frames();
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0][1], "s");
    assert_eq!(frames[0][2]["id"], id);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_kick_drops_the_room_without_a_closed() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(true, 0).await;
    let other = h.room(true, 0).await;
    live_stream(&mut h).await;
    assert_eq!(carried(&h), BTreeSet::from([room, other]));

    set_removed(&h, room, true).await;
    let outcome = crate::handlers::side_effects::apply_kick_live_side_effects(
        &h.conn.tenant,
        &h.state,
        room,
        &h.pubkey(),
    )
    .await
    .expect("kick effects");
    assert_eq!(
        outcome,
        crate::handlers::side_effects::KickOutcome::Enforced
    );
    assert_eq!(carried(&h), BTreeSet::from([other]));
    h.message(room, "after the kick").await;
    let frames = h.frames();
    assert!(frames.is_empty(), "no CLOSED, nothing after: {frames:?}");
    assert!(h
        .state
        .sub_registry
        .stream_for(h.conn.conn_id, "s")
        .is_some());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn an_archive_eviction_keeps_the_room() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(true, 0).await;
    live_stream(&mut h).await;
    crate::handlers::side_effects::evict_all_channel_subscriptions(&h.conn.tenant, &h.state, room)
        .await;
    assert_eq!(carried(&h), BTreeSet::from([room]));
    let id = h.message(room, "still here").await;
    assert_eq!(h.frames()[0][2]["id"], id);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn changes_during_registration_replay_on_query_a() {
    let h = harness(64, |_| on()).await;
    let kicked = h.room(true, 0).await;
    let kept = h.room(true, 0).await;
    let joined = h.room(false, 0).await;

    // Registered with no rooms, as the REQ does before query A.
    let owner = crate::handlers::close::next_owner();
    h.conn.subscriptions.lock().await.insert("s".into(), owner);
    let handle = Arc::new(StreamHandle::new(
        "s".into(),
        owner,
        StreamMode::Live,
        h.conn.tenant.clone(),
        queue_cap(h.state.config.send_buffer_size),
    ));
    h.state.sub_registry.register_stream(
        h.community,
        h.conn.conn_id,
        Arc::clone(&handle),
        vec![stream_filter()],
        Vec::new(),
    );
    // Query A read before both changes.
    let query_a = vec![kicked, kept];

    set_removed(&h, kicked, true).await;
    let generation = h
        .state
        .invalidate_membership(&h.conn.tenant, kicked, &h.pubkey());
    remove_room(&h.state, h.community, kicked, &h.pubkey(), generation).await;
    add_member_row(&h, joined).await;
    h.state
        .invalidate_membership(&h.conn.tenant, joined, &h.pubkey());
    recheck_room(&h.state, h.community, joined, &h.pubkey()).await;
    assert!(carried(&h).is_empty(), "held until the rooms are set");

    assert_eq!(
        set_stream_rooms(&h.state, &h.conn, &handle, query_a).await,
        SetRooms::Set
    );
    assert_eq!(carried(&h), BTreeSet::from([kept, joined]));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_removal_read_before_a_later_change_is_not_applied() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(true, 0).await;
    live_stream(&mut h).await;

    // A leave (generation g1), then a re-add before the leave's hook runs.
    let g1 = h
        .state
        .invalidate_membership(&h.conn.tenant, room, &h.pubkey());
    let g2 = h
        .state
        .invalidate_membership(&h.conn.tenant, room, &h.pubkey());
    assert_eq!(g2, g1 + 1);
    remove_room(&h.state, h.community, room, &h.pubkey(), g1).await;
    // The stale removal fell back to a re-check, which reads "member".
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(carried(&h), BTreeSet::from([room]));

    // The current generation applies directly.
    remove_room(&h.state, h.community, room, &h.pubkey(), g2).await;
    assert!(carried(&h).is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn racing_rechecks_settle_on_the_latest_membership() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(true, 0).await;
    live_stream(&mut h).await;
    // Two drops of the key back to back; the account ends removed.
    set_removed(&h, room, true).await;
    h.state
        .invalidate_membership(&h.conn.tenant, room, &h.pubkey());
    h.state
        .invalidate_membership(&h.conn.tenant, room, &h.pubkey());
    let pubkey = h.pubkey();
    tokio::join!(
        recheck_room(&h.state, h.community, room, &pubkey),
        recheck_room(&h.state, h.community, room, &pubkey),
    );
    assert!(carried(&h).is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_channel_delete_drops_the_room_for_every_stream() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(true, 0).await;
    let other = h.room(true, 0).await;
    live_stream(&mut h).await;
    let delete = EventBuilder::new(Kind::Custom(9008), "")
        .tags([Tag::parse(["h", &room.to_string()]).unwrap()])
        .sign_with_keys(&h.keys)
        .unwrap();
    crate::handlers::side_effects::handle_side_effects(&h.conn.tenant, 9008, &delete, &h.state)
        .await
        .expect("delete group");
    assert_eq!(carried(&h), BTreeSet::from([other]));

    // The ingest compensation path calls the same hook.
    remove_deleted_room(&h.state, h.community, other).await;
    assert!(carried(&h).is_empty());
    assert!(closed(&h.frames(), "s").is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn accessible_all_from_another_pod_reruns_query_a() {
    let mut h = harness(64, |_| on()).await;
    let left = h.room(true, 0).await;
    let kept = h.room(true, 0).await;
    let joined = h.room(false, 0).await;
    live_stream(&mut h).await;
    // Changes whose drops this pod never saw.
    set_removed(&h, left, true).await;
    add_member_row(&h, joined).await;

    after_cache_invalidation(&h.state, h.community, &CacheInvalidation::AccessibleAll);
    eventually("the re-run", || {
        carried(&h) == BTreeSet::from([kept, joined])
    })
    .await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_membership_drop_from_another_pod_is_rechecked() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(false, 0).await;
    live_stream(&mut h).await;
    add_member_row(&h, room).await;

    // Its own loopback: this pod's hook already ran, nothing to do.
    after_cache_invalidation(
        &h.state,
        h.community,
        &CacheInvalidation::Membership {
            channel_id: room,
            pubkey: h.pubkey(),
            origin: Some(h.state.membership_gen.pod_id()),
        },
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(carried(&h).is_empty());

    after_cache_invalidation(
        &h.state,
        h.community,
        &CacheInvalidation::Membership {
            channel_id: room,
            pubkey: h.pubkey(),
            origin: Some(Uuid::new_v4()),
        },
    );
    eventually("the re-check", || carried(&h) == BTreeSet::from([room])).await;
}

#[test]
fn reruns_coalesce_per_community() {
    let registry = crate::subscription::SubscriptionRegistry::new();
    let c = CommunityId::from_uuid(Uuid::new_v4());
    assert!(registry.begin_stream_rerun(c));
    assert!(!registry.begin_stream_rerun(c), "in flight");
    assert!(!registry.begin_stream_rerun(c));
    assert!(registry.finish_stream_rerun(c), "asked again meanwhile");
    assert!(!registry.finish_stream_rerun(c));
    assert!(registry.begin_stream_rerun(c), "released");
}
