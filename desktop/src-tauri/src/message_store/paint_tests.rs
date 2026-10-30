//! A paint read draws the stored window whether or not it is certain
//! (docs/message-sync.md#painting-from-the-store).

use serde_json::Value;

use super::{Input, MessageStore};

const ACCOUNT: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const RELAY: &str = "wss://relay.example";

fn kinds(fixture: &Value) -> Vec<u64> {
    fixture["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|kind| kind.as_u64().unwrap())
        .collect()
}

fn ids(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|event| event["id"].as_str().unwrap().to_owned())
        .collect()
}

/// A store holding `case`'s events and spans, all written.
fn filled(case: &Value, room: &str) -> (tempfile::TempDir, MessageStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    store.submit(
        &lease,
        Input::events(case["events"].as_array().unwrap().clone()),
    );
    for span in case["coverage"].as_array().unwrap() {
        store.submit(
            &lease,
            Input::Coverage {
                rooms: vec![room.to_owned()],
                from: span[0].as_i64().unwrap(),
                to: span[1].as_i64().unwrap(),
                captured: u64::MAX,
            },
        );
    }
    let writers: Vec<_> = store.inner.lock().unwrap().writers.drain().collect();
    for (_, writer) in writers {
        writer.finish();
    }
    (dir, store)
}

#[test]
fn a_room_paints_its_stored_window_certain_or_not() {
    let text = include_str!("../../../../test-fixtures/message-store-room-head.json");
    let fixture: Value = serde_json::from_str(text).unwrap();
    let room = fixture["room"].as_str().unwrap();
    let kinds = kinds(&fixture);
    let mut uncertain = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (_dir, store) = filled(case, room);
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        let limit = case["limit"].as_u64().unwrap() as usize;
        let certain = store.room_head(&lease, room, &kinds, limit, case["live_at"].as_i64());
        if !certain.certain {
            uncertain += 1;
        }
        let Some((painted, _)) = store.paint_room_head(&lease, room, &kinds, limit) else {
            assert!(certain.rows.is_empty(), "{name}: stored rows paint");
            continue;
        };
        let rows = ids(&certain.rows);
        assert_eq!(ids(&painted[..rows.len()]), rows, "{name}: rows");
        let bounds = painted.last().unwrap();
        assert_eq!(bounds["kind"], 39006, "{name}");
    }
    assert!(
        uncertain > 0,
        "the fixture has uncertain windows that still paint"
    );
}

#[test]
fn a_thread_paints_its_stored_replies_certain_or_not() {
    let text = include_str!("../../../../test-fixtures/message-store-thread-head.json");
    let fixture: Value = serde_json::from_str(text).unwrap();
    let room = fixture["room"].as_str().unwrap();
    let root = fixture["root"].as_str().unwrap();
    let kinds = kinds(&fixture);
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (_dir, store) = filled(case, room);
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        let limit = case["limit"].as_u64().unwrap() as usize;
        let certain =
            store.thread_head(&lease, room, root, &kinds, limit, case["live_at"].as_i64());
        match store.paint_thread_head(&lease, room, root, &kinds, limit) {
            Some((rows, cursor, _)) => {
                assert_eq!(ids(&rows), ids(&certain.rows), "{name}: rows");
                assert_eq!(cursor, certain.next_cursor, "{name}: cursor");
            }
            None => assert!(certain.rows.is_empty(), "{name}: stored replies paint"),
        }
    }
}

#[test]
fn nothing_stored_paints_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    assert!(store.paint_room_head(&lease, "room", &[9], 50).is_none());
    assert!(store
        .paint_thread_head(&lease, "room", "root", &[9], 50)
        .is_none());
}

fn row(id: u8, room: &str, at: i64) -> Value {
    serde_json::json!({
        "id": format!("{id:064x}"),
        "pubkey": "bb".repeat(32),
        "created_at": at,
        "kind": 9,
        "tags": [["h", room]],
        "content": "",
        "sig": "",
    })
}

/// Coverage chains back from the newest painted row; a gap stops it.
#[test]
fn a_paint_says_how_far_back_its_room_is_complete() {
    let margin = super::read::ROW_RECEIVE_MARGIN_SECS;
    let case = |spans: Vec<(i64, i64)>| {
        serde_json::json!({
            "events": [row(1, "room", 10_000), row(2, "room", 50_000), row(3, "room", 90_000)],
            "coverage": spans.iter().map(|(a, b)| vec![*a, *b]).collect::<Vec<_>>(),
        })
    };
    for (spans, expect) in [
        // Today's live span only: complete from its start, not back to 10_000.
        (vec![(80_000, 100_000)], Some(80_000 + margin)),
        // Two touching spans chain.
        (
            vec![(40_000, 85_000), (80_000, 100_000)],
            Some(40_000 + margin),
        ),
        // A gap between them stops the chain.
        (
            vec![(5_000, 20_000), (80_000, 100_000)],
            Some(80_000 + margin),
        ),
        // No span reaches the newest row.
        (vec![(5_000, 20_000)], None),
    ] {
        let (_dir, store) = filled(&case(spans.clone()), "room");
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        let (_, complete) = store.paint_room_head(&lease, "room", &[9], 50).unwrap();
        assert_eq!(complete, expect, "{spans:?}");
    }
}
