//! Head coverage against the shared cases in
//! test-fixtures/message-store-head-coverage.json (also read by mobile).

use std::collections::HashMap;

use serde_json::{json, Value};

use super::head::{thread_head_stream, HeadSpan, HEAD_STREAM};
use super::{Lease, MessageStore};

const RELAY: &str = "wss://relay.example";

fn fixture() -> Value {
    let text = include_str!("../../../../test-fixtures/message-store-head-coverage.json");
    serde_json::from_str(text).expect("head fixture parses")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

fn kinds(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|kind| kind.as_u64().unwrap())
        .collect()
}

/// Writes what is queued and stops every writer.
fn finish(store: &MessageStore) {
    let writers: Vec<_> = store
        .inner
        .lock()
        .unwrap()
        .writers
        .drain()
        .map(|(_, writer)| writer)
        .collect();
    for writer in writers {
        writer.finish();
    }
}

/// The head span of `stream` as `[lo_at, lo_id, hi_at]`, or null.
fn head_of(store: &MessageStore, lease: &Lease, room: &str, stream: &str) -> Value {
    let conn = rusqlite::Connection::open(store.path_of(lease)).unwrap();
    let mut statement = conn
        .prepare("SELECT lo_at, lo_id, hi_at FROM coverage WHERE room = ?1 AND stream = ?2")
        .unwrap();
    let spans: Vec<Value> = statement
        .query_map(rusqlite::params![room, stream], |row| {
            Ok(json!([
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?
            ]))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(spans.len() <= 1, "one head span per stream");
    spans.into_iter().next().unwrap_or(Value::Null)
}

fn stored_ids(store: &MessageStore, lease: &Lease) -> Vec<String> {
    let conn = rusqlite::Connection::open(store.path_of(lease)).unwrap();
    let mut statement = conn.prepare("SELECT id FROM events ORDER BY id").unwrap();
    let ids = statement.query_map([], |row| row.get(0)).unwrap();
    ids.map(Result::unwrap).collect()
}

fn head_span(head: &Value, room: &str, kinds: Vec<u64>) -> HeadSpan {
    HeadSpan {
        room: room.to_owned(),
        root: head["root"].as_str().map(str::to_owned),
        kinds,
        live_at: head["live_at"].as_i64().unwrap(),
        oldest: head["next_cursor"]
            .as_array()
            .filter(|_| head["has_more"].as_bool() == Some(true))
            .map(|cursor| {
                (
                    cursor[0].as_i64().unwrap(),
                    cursor[1].as_str().unwrap().to_owned(),
                )
            }),
        ids: strings(&head["ids"]).into_iter().collect(),
    }
}

#[test]
fn head_coverage_follows_shared_fixture() {
    let fixture = fixture();
    let account = fixture["account"].as_str().unwrap();
    let room = fixture["room"].as_str().unwrap();
    let root = fixture["root"].as_str().unwrap();
    let room_kinds = kinds(&fixture["kinds"]);
    let thread_kinds = kinds(&fixture["thread_kinds"]);
    let limit = fixture["limit"].as_u64().unwrap() as usize;
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = MessageStore::new(dir.path().to_path_buf());
        let lease = store.lease(RELAY, account).unwrap();
        store.note_relay_key(RELAY, fixture["relay_key"].as_str().unwrap());
        let mut marks: HashMap<String, String> = HashMap::new();
        for step in case["steps"].as_array().unwrap() {
            if let Some(events) = step["events"].as_array() {
                store.submit_events(&lease, events.clone());
            } else if let Some(span) = step["coverage"].as_array() {
                let scope = store.capture(&lease).unwrap();
                store
                    .extend_coverage(
                        RELAY,
                        account,
                        Some(&scope),
                        vec![room.to_owned()],
                        span[0].as_i64().unwrap(),
                        span[1].as_i64().unwrap(),
                    )
                    .unwrap();
            } else if let Some(mark) = step["sent"].as_str() {
                let capture = store.head_sent(&lease, strings(&step["rooms"])).unwrap();
                marks.insert(mark.to_owned(), capture);
            } else if let Some(head) = step.get("head") {
                let scope = &marks[head["sent"].as_str().unwrap()];
                let kinds = if head["root"].is_null() {
                    room_kinds.clone()
                } else {
                    thread_kinds.clone()
                };
                // A token that no longer holds only releases the mark.
                let heads = if step["token_changed"].as_bool() == Some(true) {
                    Vec::new()
                } else {
                    vec![head_span(head, room, kinds)]
                };
                store
                    .record_heads(RELAY, account, Some(scope), heads)
                    .unwrap();
            } else if let Some(keep) = step.get("forget_except") {
                let scope = store.capture(&lease).unwrap();
                store
                    .forget_rooms_except(RELAY, account, Some(&scope), strings(keep))
                    .unwrap();
            } else if step["lost"].as_bool() == Some(true) {
                let inner = store.inner.lock().unwrap();
                inner.writers[&lease.stem].mark_lossy_for_test();
            }
        }
        finish(&store);
        let live_at = case["live_at"].as_i64();
        let expect = &case["expect"];
        if let Some(certain) = expect["room_certain"].as_bool() {
            let head = store.room_head(&lease, room, &room_kinds, limit, live_at);
            assert_eq!(head.certain, certain, "{name}: room certain");
        }
        if let Some(certain) = expect["thread_certain"].as_bool() {
            let head = store.thread_head(&lease, room, root, &thread_kinds, limit, live_at);
            assert_eq!(head.certain, certain, "{name}: thread certain");
        }
        if !expect["events"].is_null() {
            assert_eq!(
                stored_ids(&store, &lease),
                strings(&expect["events"]),
                "{name}: events"
            );
        }
        let heads = &expect["heads"];
        assert_eq!(
            head_of(&store, &lease, room, HEAD_STREAM),
            heads["room"],
            "{name}: room head"
        );
        assert_eq!(
            head_of(&store, &lease, room, &thread_head_stream(root)),
            heads["thread"],
            "{name}: thread head"
        );
    }
}

fn small_room(store: &MessageStore, lease: &Lease) {
    let event = |id: &str, at: i64| {
        json!({"id": id, "kind": 9, "pubkey": "p", "created_at": at,
               "tags": [["h", "r"]], "content": ""})
    };
    store.submit_events(lease, vec![event("a1", 5000), event("a2", 6000)]);
    let scope = store.capture(lease).unwrap();
    let account = lease.parts().1.to_owned();
    store
        .extend_coverage(
            RELAY,
            &account,
            Some(&scope),
            vec!["r".into()],
            10_000,
            20_000,
        )
        .unwrap();
}

fn whole(ids: &[&str]) -> HeadSpan {
    HeadSpan {
        room: "r".into(),
        root: None,
        kinds: vec![9],
        live_at: 10_000,
        oldest: None,
        ids: ids.iter().map(|id| (*id).to_owned()).collect(),
    }
}

const ACCOUNT: &str = "aa00000000000000000000000000000000000000000000000000000000000001";

#[test]
fn a_head_needs_the_mark_of_its_own_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    small_room(&store, &lease);
    // A plain capture is no request mark: nothing is recorded or dropped.
    let plain = store.capture(&lease).unwrap();
    store
        .record_heads(RELAY, ACCOUNT, Some(&plain), vec![whole(&["a1"])])
        .unwrap();
    // A mark answers once.
    let mark = store.head_sent(&lease, vec!["r".into()]).unwrap();
    store
        .record_heads(RELAY, ACCOUNT, Some(&mark), vec![])
        .unwrap();
    store
        .record_heads(RELAY, ACCOUNT, Some(&mark), vec![whole(&["a1"])])
        .unwrap();
    finish(&store);
    assert_eq!(stored_ids(&store, &lease), ["a1", "a2"]);
    assert_eq!(head_of(&store, &lease, "r", HEAD_STREAM), Value::Null);
    let head = store.room_head(&lease, "r", &[9], 50, Some(19_000));
    assert!(!head.certain);
}

#[test]
fn a_head_from_another_scope_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    small_room(&store, &lease);
    let mark = store.head_sent(&lease, vec!["r".into()]).unwrap();
    assert!(store
        .record_heads(RELAY, ACCOUNT, None, vec![whole(&["a1", "a2"])])
        .is_err());
    store.forget(RELAY, ACCOUNT);
    assert!(store
        .record_heads(RELAY, ACCOUNT, Some(&mark), vec![whole(&["a1", "a2"])])
        .is_err());
}

#[test]
fn a_room_head_with_a_whole_history_is_certain_with_more_rows_since() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    small_room(&store, &lease);
    let mark = store.head_sent(&lease, vec!["r".into()]).unwrap();
    store
        .record_heads(RELAY, ACCOUNT, Some(&mark), vec![whole(&["a1", "a2"])])
        .unwrap();
    // Rows arriving live afterwards fill the window past its limit.
    let later: Vec<Value> = (0..4)
        .map(|n| {
            json!({"id": format!("b{n}"), "kind": 9, "pubkey": "p",
                   "created_at": 11_000 + n, "tags": [["h", "r"]], "content": ""})
        })
        .collect();
    store.submit_events(&lease, later);
    finish(&store);
    let head = store.room_head(&lease, "r", &[9], 3, Some(19_000));
    assert!(head.certain);
    assert_eq!(head.rows.len(), 3);
}

#[test]
fn an_expired_mark_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    small_room(&store, &lease);
    let mut marks = super::head::Marks::default();
    marks.open(1, vec!["r".into()]);
    marks.age_for_test(std::time::Duration::from_secs(121));
    assert!(marks.close(1).is_none());
    marks.open(2, vec!["r".into()]);
    marks.note("r", "x");
    marks.note("other", "y");
    let closed = marks.close(2).unwrap();
    assert!(closed.rooms.contains("r"));
    assert_eq!(closed.fresh.into_iter().collect::<Vec<_>>(), ["x"]);
    finish(&store);
}
