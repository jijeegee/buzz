use super::*;
use crate::managed_agents::persona_events::monotonic_created_at;
use crate::managed_agents::retention::{
    get_pending_sync, get_retained_event, open_retention_db, retain_event,
    scoped_retention_db_path, tombstone_retention_d_tag, RetainedEvent,
};
use crate::managed_agents::team_events::build_team_event;
use kuzz_core_pkg::kind::KIND_TEAM;
use nostr::JsonUtil;
use std::path::{Path, PathBuf};

fn team() -> TeamRecord {
    TeamRecord {
        id: "team-abc".to_string(),
        name: "Catalog Team".to_string(),
        description: Some("A shared team".to_string()),
        instructions: None,
        persona_ids: vec!["m1".to_string()],
        is_builtin: false,
        shared: false,
        catalog_source: None,
        source_dir: Some(PathBuf::from("/local/only/path")),
        is_symlink: false,
        symlink_target: None,
        version: None,
        created_at: "2026-07-30T00:00:00Z".to_string(),
        updated_at: "2026-07-30T00:00:00Z".to_string(),
    }
}

fn scoped_db(dir: &Path, relay_url: &str, owner: &str) -> PathBuf {
    let db_path = scoped_retention_db_path(dir, relay_url, owner);
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    db_path
}

/// Seed a retained 30176 head dated `created_at` seconds since epoch.
fn seed_team_head(db_path: &Path, keys: &nostr::Keys, created_at: i64) {
    let event = build_team_event(&team())
        .unwrap()
        .custom_created_at(nostr::Timestamp::from(created_at as u64))
        .sign_with_keys(keys)
        .unwrap();
    let conn = open_retention_db(db_path).unwrap();
    retain_event(
        &conn,
        &RetainedEvent {
            kind: KIND_TEAM,
            pubkey: keys.public_key().to_hex(),
            d_tag: "team-abc".to_string(),
            content: event.content.to_string(),
            created_at,
            raw_event: event.as_json(),
            pending_sync: false,
        },
    )
    .unwrap();
}

#[test]
fn test_team_tombstone_created_at_strictly_dominates_a_future_dated_head() {
    // 30176 analog of the 30178 defect (Wes P1): retain_team_pending signs the
    // team head with monotonic_created_at, so it can be future-dated. The kind:5
    // must dominate it or the relay's created_at <= gate leaves the head live.
    let dir = tempfile::tempdir().unwrap();
    let keys = nostr::Keys::generate();
    let owner = keys.public_key().to_hex();
    let db_path = scoped_db(dir.path(), "wss://a.example", &owner);

    let future = nostr::Timestamp::now().as_secs() as i64 + 86_400;
    seed_team_head(&db_path, &keys, future);

    tombstone_team_at(&db_path, &keys, "team-abc").unwrap();

    let conn = open_retention_db(&db_path).unwrap();
    assert!(
        get_retained_event(&conn, KIND_TEAM, &owner, "team-abc")
            .unwrap()
            .is_none(),
        "the 30176 head is purged"
    );
    let tombstone = get_pending_sync(&conn)
        .unwrap()
        .into_iter()
        .find(|row| row.kind == 5)
        .expect("a kind:5 tombstone is enqueued");
    assert_eq!(
        tombstone.d_tag,
        tombstone_retention_d_tag(KIND_TEAM, "team-abc")
    );
    assert!(
        tombstone.created_at > future,
        "tombstone created_at ({}) must strictly dominate the future-dated 30176 head ({future})",
        tombstone.created_at
    );
}

#[test]
fn test_team_tombstone_with_no_head_falls_back_to_wall_clock() {
    let dir = tempfile::tempdir().unwrap();
    let keys = nostr::Keys::generate();
    let owner = keys.public_key().to_hex();
    let db_path = scoped_db(dir.path(), "wss://a.example", &owner);

    let before = nostr::Timestamp::now().as_secs() as i64;
    tombstone_team_at(&db_path, &keys, "team-abc").unwrap();
    let after = nostr::Timestamp::now().as_secs() as i64;

    let conn = open_retention_db(&db_path).unwrap();
    let tombstone = get_pending_sync(&conn)
        .unwrap()
        .into_iter()
        .find(|row| row.kind == 5)
        .expect("a kind:5 tombstone is enqueued even with no head");
    assert!(
        tombstone.created_at >= before && tombstone.created_at <= after,
        "no-head 30176 tombstone is dated at wall clock; got {}",
        tombstone.created_at
    );
    // Sanity: with no head, the floor is 0 so the result is exactly `now`.
    assert!(monotonic_created_at(None).as_secs() as i64 >= before);
}

#[test]
fn test_team_tombstone_rolls_back_head_purge_when_enqueue_fails() {
    // P1-2: the head purge and the kind:5 enqueue run in one `BEGIN IMMEDIATE`
    // transaction. A crash/failure between them must not leave the 30176 head
    // gone with no local retry witness. A `BEFORE INSERT` trigger blocks the
    // tombstone enqueue (which follows the head DELETE); the whole transaction
    // must roll back so the head survives.
    let dir = tempfile::tempdir().unwrap();
    let keys = nostr::Keys::generate();
    let owner = keys.public_key().to_hex();
    let db_path = scoped_db(dir.path(), "wss://a.example", &owner);

    let future = nostr::Timestamp::now().as_secs() as i64 + 86_400;
    seed_team_head(&db_path, &keys, future);

    let conn = open_retention_db(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER block_all_inserts BEFORE INSERT ON persona_events
         BEGIN
             SELECT RAISE(ABORT, 'insert blocked by test trigger');
         END;",
    )
    .unwrap();
    drop(conn);

    let result = tombstone_team_at(&db_path, &keys, "team-abc");
    assert!(result.is_err(), "tombstone with INSERT trigger must fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("insert blocked by test trigger") || err.contains("blocked"),
        "error must name the trigger cause; got: {err}"
    );

    let conn = open_retention_db(&db_path).unwrap();
    assert!(
        get_retained_event(&conn, KIND_TEAM, &owner, "team-abc")
            .unwrap()
            .is_some(),
        "the 30176 head must survive when the tombstone enqueue fails"
    );
}
