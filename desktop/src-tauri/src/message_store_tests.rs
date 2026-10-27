use super::*;
fn fixture() -> Value {
    serde_json::from_str(include_str!("../../../test-fixtures/message-store.json")).unwrap()
}

#[test]
fn history_queue_is_bounded_before_spawning_disk_work() {
    let store = MessageStore::default();
    store.reserve(4096, 1024).unwrap();
    assert!(store.reserve(1, 1).is_err());
    let store = MessageStore::default();
    store.reserve(1, 8 * 1024 * 1024).unwrap();
    assert!(store.reserve(1, 1).is_err());
    assert!(store.0.lock().unwrap().last_error.is_some());
}
fn count(path: &Path, lease: &Lease) -> i64 {
    open(path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM events WHERE relay=?1 AND account=?2",
            params![lease.relay, lease.account],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn shared_contract_roundtrips_originals_and_never_claims_coverage() {
    let f = fixture();
    for kind in f["storedKinds"].as_array().unwrap() {
        assert!(supported(kind.as_u64().unwrap()));
    }
    for kind in f["excludedKinds"].as_array().unwrap() {
        assert!(!supported(kind.as_u64().unwrap()));
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("messages.db");
    let store = MessageStore::default();
    let lease = store.lease("https://one", "a").unwrap();
    let events = f["events"].as_array().unwrap();
    store.put(&path, &lease, events).unwrap();
    store.put(&path, &lease, events).unwrap();
    assert_eq!(count(&path, &lease), events.len() as i64);
    // A new manager/connection simulates reopening after process exit.
    let reopened = MessageStore::default();
    let lease = reopened.lease("https://one", "a").unwrap();
    let conn = open(&path).unwrap();
    for event in events {
        let raw: String = conn
            .query_row(
                "SELECT raw_json FROM events WHERE relay=?1 AND account=?2 AND id=?3",
                params![lease.relay, lease.account, event["id"].as_str().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), *event);
    }
    assert_eq!(
        conn.query_row("SELECT needs_repair FROM scopes", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT value FROM event_tags WHERE id='delete' AND tag='e'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "root"
    );
}

#[test]
fn logout_fences_late_writes_and_preserves_other_accounts_and_relays() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("messages.db");
    let store = MessageStore::default();
    let a = store.lease("https://one", "a").unwrap();
    let b = store.lease("https://one", "b").unwrap();
    let c = store.lease("https://two", "a").unwrap();
    let f = fixture();
    let events = f["events"].as_array().unwrap();
    for lease in [&a, &b, &c] {
        store.put(&path, lease, events).unwrap();
    }
    store.clear(&path, "https://one", "a").unwrap();
    store.put(&path, &a, events).unwrap();
    assert_eq!(count(&path, &a), 0);
    assert_eq!(count(&path, &b), 7);
    assert_eq!(count(&path, &c), 7);
}

#[test]
fn no_head_cache_eviction_and_failed_batch_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("messages.db");
    let store = MessageStore::default();
    let lease = store.lease("https://one", "a").unwrap();
    let event = fixture()["events"][3].clone();
    for batch in 0..8 {
        let events: Vec<_> = (0..256)
            .map(|i| {
                let mut e = event.clone();
                e["id"] = Value::String(format!("{batch}-{i}"));
                e
            })
            .collect();
        store.put(&path, &lease, &events).unwrap();
    }
    assert_eq!(count(&path, &lease), 2048);
    let mut invalid = event.clone();
    invalid["created_at"] = Value::Null;
    assert!(store.put(&path, &lease, &[event, invalid]).is_err());
    assert_eq!(count(&path, &lease), 2048);
    open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert!(store.put(&path, &lease, &[]).is_err());
    let conn = Connection::open(&path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2048
    );
}
