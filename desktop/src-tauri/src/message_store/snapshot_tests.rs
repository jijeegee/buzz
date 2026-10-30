//! Launch snapshots live in the account's file for one server
//! (docs/message-sync.md#launch-snapshots).

use super::MessageStore;

const ACCOUNT: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const OTHER: &str = "bb00000000000000000000000000000000000000000000000000000000000002";
const RELAY: &str = "wss://relay.example";

/// Writes everything queued so far.
fn settle(store: &MessageStore) {
    let writers: Vec<_> = store.inner.lock().unwrap().writers.drain().collect();
    for (_, writer) in writers {
        writer.finish();
    }
}

#[test]
fn a_snapshot_is_read_back_only_for_its_account_and_server() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    assert!(store.write_snapshot(&lease, "chat_rows", "[1]".into()));
    settle(&store);

    assert_eq!(
        store.read_snapshot(&lease, "chat_rows").as_deref(),
        Some("[1]")
    );
    // The https origin of the same server is the same file.
    let same = store.lease("https://Relay.example/", ACCOUNT).unwrap();
    assert_eq!(
        store.read_snapshot(&same, "chat_rows").as_deref(),
        Some("[1]")
    );
    // Another account, another server, another kind: nothing.
    let other_account = store.lease(RELAY, OTHER).unwrap();
    assert_eq!(store.read_snapshot(&other_account, "chat_rows"), None);
    let other_server = store.lease("wss://other.example", ACCOUNT).unwrap();
    assert_eq!(store.read_snapshot(&other_server, "chat_rows"), None);
    assert_eq!(store.read_snapshot(&lease, "profiles"), None);
}

#[test]
fn a_later_snapshot_replaces_the_earlier_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    store.write_snapshot(&lease, "profiles", "old".into());
    store.write_snapshot(&lease, "profiles", "new".into());
    settle(&store);
    assert_eq!(
        store.read_snapshot(&lease, "profiles").as_deref(),
        Some("new")
    );
}

#[test]
fn sign_out_deletes_snapshots_with_the_file_and_fences_late_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    store.write_snapshot(&lease, "chat_rows", "[1]".into());
    settle(&store);
    let path = store.path_of(&lease);
    assert!(path.exists());

    store.forget(RELAY, ACCOUNT);
    assert!(!path.exists());
    // A write under the lease from before the sign-out writes nothing.
    assert!(!store.write_snapshot(&lease, "chat_rows", "[2]".into()));
    settle(&store);
    assert!(!path.exists());
    let fresh = store.lease(RELAY, ACCOUNT).unwrap();
    assert_eq!(store.read_snapshot(&fresh, "chat_rows"), None);
}

#[test]
fn bad_kinds_and_oversized_blobs_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    assert!(!store.write_snapshot(&lease, "Bad Kind", "x".into()));
    assert!(!store.write_snapshot(&lease, "", "x".into()));
    let big = "x".repeat(super::snapshot::MAX_SNAPSHOT_BYTES + 1);
    assert!(!store.write_snapshot(&lease, "profiles", big));
    assert_eq!(store.read_snapshot(&lease, "Bad Kind"), None);
}

#[test]
fn a_snapshot_never_counts_as_a_received_room() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    store.write_snapshot(&lease, "chat_rows", r#"[{"channel_id":"room-a"}]"#.into());
    settle(&store);
    let conn = rusqlite::Connection::open(store.path_of(&lease)).unwrap();
    let events: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .unwrap();
    let marks: i64 = conn
        .query_row("SELECT COUNT(*) FROM room_purge", [], |row| row.get(0))
        .unwrap();
    assert_eq!((events, marks), (0, 0));
}
