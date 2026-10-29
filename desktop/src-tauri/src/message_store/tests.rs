use serde_json::{json, Value};

use super::{classify, coverage, http_origin, labelled, writer, Filed, Input, MessageStore};

const ACCOUNT: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const OTHER: &str = "bb00000000000000000000000000000000000000000000000000000000000002";
const RELAY: &str = "wss://relay.example";

fn fixture() -> Value {
    let text = include_str!("../../../../test-fixtures/message-store.json");
    serde_json::from_str(text).expect("fixture parses")
}

#[test]
fn classification_follows_shared_fixture() {
    for case in fixture()["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let expect = &case["expect"];
        let filed = classify(&case["event"]);
        if !expect["stored"].as_bool().unwrap() {
            assert!(filed.is_none(), "{name}: should not be kept");
            continue;
        }
        let str_or_none = |value: &Value| value.as_str().map(str::to_owned);
        match filed.unwrap_or_else(|| panic!("{name}: should be kept")) {
            Filed::Summary(row) => {
                assert!(
                    expect["summary"].as_bool().unwrap_or(false),
                    "{name}: not a summary"
                );
                assert_eq!(Some(row.root), str_or_none(&expect["root"]), "{name}: root");
                assert_eq!(row.room, str_or_none(&expect["room"]), "{name}: room");
            }
            Filed::Event(row) => {
                assert!(
                    !expect["summary"].as_bool().unwrap_or(false),
                    "{name}: is a summary"
                );
                assert_eq!(row.room, str_or_none(&expect["room"]), "{name}: room");
                assert_eq!(row.root, str_or_none(&expect["root"]), "{name}: root");
                assert_eq!(
                    row.broadcast,
                    expect["broadcast"].as_bool().unwrap(),
                    "{name}"
                );
                let targets: Vec<String> = expect["targets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t.as_str().unwrap().to_owned())
                    .collect();
                assert_eq!(row.targets, targets, "{name}: targets");
            }
        }
    }
}

fn event(id: &str, created_at: i64, tags: Value) -> Value {
    json!({"id": id, "kind": 9, "pubkey": "p1", "created_at": created_at, "tags": tags,
           "content": "", "sig": ""})
}

fn summary(created_at: i64, content: &str) -> Value {
    json!({"id": format!("s{created_at}"), "kind": 39005, "pubkey": "relay",
           "created_at": created_at, "tags": [["e", "r1"], ["h", "room1"]], "content": content})
}

fn rows(events: &[Value]) -> Vec<Filed> {
    events.iter().filter_map(classify).collect()
}

#[test]
fn first_copy_wins_and_newest_summary_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = writer::open(&dir.path().join("t.db")).unwrap();
    let first = event("a1", 10, json!([["h", "room1"]]));
    let mut changed = first.clone();
    changed["content"] = json!("changed");
    writer::write(&mut conn, &rows(&[first, summary(20, "new")])).unwrap();
    writer::write(&mut conn, &rows(&[changed, summary(15, "old")])).unwrap();
    let raw: String = conn
        .query_row("SELECT raw FROM events WHERE id='a1'", [], |r| r.get(0))
        .unwrap();
    assert!(!raw.contains("changed"));
    let kept: String = conn
        .query_row("SELECT raw FROM summaries WHERE root='r1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(kept.contains("\"new\""));
}

#[test]
fn reaction_targets_are_indexed_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = writer::open(&dir.path().join("t.db")).unwrap();
    let reaction = json!({"id": "x1", "kind": 7, "pubkey": "p2", "created_at": 5,
                          "tags": [["e", "a1"]], "content": "+"});
    writer::write(&mut conn, &rows(&[reaction.clone(), reaction])).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM targets WHERE target='a1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    drop(writer::open(&path).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert!(writer::open(&path).is_err());
}

fn finish(store: &MessageStore) {
    let writers: Vec<_> = store
        .inner
        .lock()
        .unwrap()
        .writers
        .drain()
        .map(|(_, w)| w)
        .collect();
    for writer in writers {
        writer.finish();
    }
}

fn count(store: &MessageStore, lease: &super::Lease) -> i64 {
    let conn = rusqlite::Connection::open(store.path_of(lease)).unwrap();
    conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn each_account_and_server_has_its_own_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let mine = store.lease(RELAY, ACCOUNT).unwrap();
    let theirs = store.lease(RELAY, OTHER).unwrap();
    let elsewhere = store.lease("wss://other.example", ACCOUNT).unwrap();
    store.submit(
        &mine,
        Input::Events(vec![event("a1", 1, json!([["h", "r"]]))]),
    );
    store.submit(
        &theirs,
        Input::Events(vec![event("b1", 1, json!([["h", "r"]]))]),
    );
    store.submit(
        &theirs,
        Input::Events(vec![event("b2", 2, json!([["h", "r"]]))]),
    );
    finish(&store);
    assert_eq!(count(&store, &mine), 1);
    assert_eq!(count(&store, &theirs), 2);
    assert_ne!(store.path_of(&mine), store.path_of(&elsewhere));
    // A socket and a query to the same server share the file.
    assert_eq!(store.lease("https://relay.example/", ACCOUNT), Some(mine));
}

#[test]
fn sign_out_deletes_only_that_account_on_that_server_and_fences_late_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let mine = store.lease(RELAY, ACCOUNT).unwrap();
    let theirs = store.lease(RELAY, OTHER).unwrap();
    let elsewhere = store.lease("wss://other.example", ACCOUNT).unwrap();
    store.submit(
        &elsewhere,
        Input::Events(vec![event("e1", 1, json!([["h", "r"]]))]),
    );
    store.submit(
        &mine,
        Input::Events(vec![event("a1", 1, json!([["h", "r"]]))]),
    );
    store.submit(
        &theirs,
        Input::Events(vec![event("b1", 1, json!([["h", "r"]]))]),
    );
    finish(&store);
    // The sign-out names the server as the app has it, not normalised.
    store.forget("https://relay.example/", ACCOUNT);
    assert!(!store.path_of(&mine).exists());
    assert!(store.path_of(&theirs).exists());
    // The same account on another server keeps its file and keeps recording.
    store.submit(
        &elsewhere,
        Input::Events(vec![event("e2", 2, json!([["h", "r"]]))]),
    );
    finish(&store);
    assert_eq!(count(&store, &elsewhere), 2);
    // An answer to a request sent before the sign-out arrives late.
    store.submit(
        &mine,
        Input::Events(vec![event("a2", 2, json!([["h", "r"]]))]),
    );
    finish(&store);
    assert!(
        !store.path_of(&mine).exists(),
        "late write recreated the file"
    );
    // Signing in again starts a fresh scope.
    let again = store.lease(RELAY, ACCOUNT).unwrap();
    store.submit(
        &again,
        Input::Events(vec![event("a3", 3, json!([["h", "r"]]))]),
    );
    finish(&store);
    assert_eq!(count(&store, &again), 1);
}

#[test]
fn socket_frames_keep_only_events() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let ev = event("a1", 1, json!([["h", "r"]]));
    for frame in [
        json!(["EVENT", "sub", ev]).to_string(),
        json!(["EOSE", "sub"]).to_string(),
        json!(["EVENT", "sub", {"kind": 20002}]).to_string(),
        "not json".to_owned(),
    ] {
        store.submit(&lease, Input::Frame(frame));
    }
    finish(&store);
    assert_eq!(count(&store, &lease), 1);
}

#[test]
fn frame_labels_and_origins() {
    assert!(labelled(r#"[ "EVENT","s",{}]"#, "EVENT"));
    assert!(labelled(r#"["AUTH",{}]"#, "AUTH"));
    assert!(!labelled(r#"["EVENTS",{}]"#, "EVENT"));
    assert!(!labelled(r#"["EOSE","s"]"#, "EVENT"));
    assert_eq!(
        http_origin("wss://Relay.Example/path").as_deref(),
        Some("https://relay.example")
    );
    assert_eq!(
        http_origin("ws://localhost:3000").as_deref(),
        Some("http://localhost:3000")
    );
    assert_eq!(http_origin("ftp://x"), None);
}

#[test]
fn a_token_auth_names_its_account_without_keeping_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    store.note_token("bzs_first", ACCOUNT);
    store.note_token("bzs_second", OTHER);
    assert_eq!(store.token_account("bzs_first").as_deref(), Some(ACCOUNT));
    assert_eq!(store.token_account("bzs_second").as_deref(), Some(OTHER));
    assert_eq!(store.token_account("bzs_unknown"), None);
    let kept = store.inner.lock().unwrap();
    assert!(kept
        .tokens
        .iter()
        .all(|(hash, _)| !hash.starts_with(b"bzs_")));
}

#[test]
fn coverage_follows_shared_fixture() {
    let text = include_str!("../../../../test-fixtures/message-store-coverage.json");
    let fixture: Value = serde_json::from_str(text).unwrap();
    let pairs = |value: &Value| -> Vec<(i64, i64)> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_i64().unwrap(), p[1].as_i64().unwrap()))
            .collect()
    };
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let span = pairs(&json!([case["extend"]]))[0];
        let merged = coverage::merge(&pairs(&case["segments"]), span.0, span.1);
        assert_eq!(merged, pairs(&case["expect"]), "{name}");
        assert_eq!(
            merged.iter().map(|s| s.1).max(),
            case["resume"].as_i64(),
            "{name}: resume"
        );
    }
}

#[test]
fn coverage_is_recorded_after_its_events_and_forgotten_when_lossy() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let rooms = vec!["r".to_owned()];
    store.submit(
        &lease,
        Input::Events(vec![event("a1", 150, json!([["h", "r"]]))]),
    );
    store.submit(
        &lease,
        Input::Coverage {
            rooms: rooms.clone(),
            from: 100,
            to: 200,
        },
    );
    finish(&store);
    assert_eq!(count(&store, &lease), 1);
    assert_eq!(
        store.resume_points(&lease, &rooms, 1000).get("r"),
        Some(&200)
    );
    assert!(store
        .resume_points(&lease, &["other".to_owned()], 1000)
        .is_empty());
    // A span opened at the catch-up boundary is not a resume point.
    assert!(store.resume_points(&lease, &rooms, 100).is_empty());

    // An event dropped before a span: the span is not recorded and every
    // earlier one is forgotten.
    let writer = writer::Writer::start(store.path_of(&lease));
    writer.mark_lossy_for_test();
    writer.submit(Input::Coverage {
        rooms: rooms.clone(),
        from: 200,
        to: 300,
    });
    writer.finish();
    assert!(store.resume_points(&lease, &rooms, 1000).is_empty());

    // The loss is already noted by an earlier batch: a span reaching back
    // over it is still dropped; one opened after it is kept.
    let writer = writer::Writer::start(store.path_of(&lease));
    writer.mark_lossy_for_test();
    writer.submit(Input::Events(vec![]));
    std::thread::sleep(std::time::Duration::from_millis(50));
    writer.submit(Input::Coverage {
        rooms: rooms.clone(),
        from: 300,
        to: 400,
    });
    let later = i64::MAX / 2;
    writer.submit(Input::Coverage {
        rooms: rooms.clone(),
        from: later,
        to: later,
    });
    writer.finish();
    assert_eq!(
        store.resume_points(&lease, &rooms, i64::MAX).get("r"),
        Some(&later)
    );
    assert!(store.resume_points(&lease, &rooms, later).is_empty());
}

#[test]
fn room_head_follows_shared_fixture() {
    let text = include_str!("../../../../test-fixtures/message-store-room-head.json");
    let fixture: Value = serde_json::from_str(text).unwrap();
    let room = fixture["room"].as_str().unwrap();
    let kinds: Vec<u64> = fixture["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|kind| kind.as_u64().unwrap())
        .collect();
    let ids = |events: &[Value]| -> Vec<String> {
        events
            .iter()
            .map(|event| event["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let sorted = |mut list: Vec<String>| {
        list.sort();
        list
    };
    let strings = |value: &Value| -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    };
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = MessageStore::new(dir.path().to_path_buf());
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        store.submit(
            &lease,
            Input::Events(case["events"].as_array().unwrap().clone()),
        );
        for span in case["coverage"].as_array().unwrap() {
            store.submit(
                &lease,
                Input::Coverage {
                    rooms: vec![room.to_owned()],
                    from: span[0].as_i64().unwrap(),
                    to: span[1].as_i64().unwrap(),
                },
            );
        }
        finish(&store);
        let head = store.room_head(
            &lease,
            room,
            &kinds,
            case["limit"].as_u64().unwrap() as usize,
            case["live_at"].as_i64(),
        );
        let expect = &case["expect"];
        assert_eq!(head.certain, expect["certain"].as_bool().unwrap(), "{name}");
        assert_eq!(ids(&head.rows), strings(&expect["rows"]), "{name}: rows");
        assert_eq!(
            sorted(ids(&head.aux)),
            sorted(strings(&expect["aux"])),
            "{name}: aux"
        );
        let roots: Vec<String> = head
            .summaries
            .iter()
            .map(|summary| summary["tags"][0][1].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            sorted(roots),
            sorted(strings(&expect["summaries"])),
            "{name}: summaries"
        );
        let cursor = &expect["next_cursor"];
        assert_eq!(
            head.next_cursor.clone(),
            Some((
                cursor[0].as_i64().unwrap(),
                cursor[1].as_str().unwrap().to_owned()
            )),
            "{name}: cursor"
        );
        let response = head.into_response(room);
        let bounds = response.last().unwrap();
        assert_eq!(bounds["kind"], 39006, "{name}");
        assert_eq!(bounds["tags"][0][1], format!("{room}:head"), "{name}");
    }
}

#[test]
fn thread_head_follows_shared_fixture() {
    let text = include_str!("../../../../test-fixtures/message-store-thread-head.json");
    let fixture: Value = serde_json::from_str(text).unwrap();
    let room = fixture["room"].as_str().unwrap();
    let root = fixture["root"].as_str().unwrap();
    let kinds: Vec<u64> = fixture["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|kind| kind.as_u64().unwrap())
        .collect();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = MessageStore::new(dir.path().to_path_buf());
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        store.submit(
            &lease,
            Input::Events(case["events"].as_array().unwrap().clone()),
        );
        for span in case["coverage"].as_array().unwrap() {
            store.submit(
                &lease,
                Input::Coverage {
                    rooms: vec![room.to_owned()],
                    from: span[0].as_i64().unwrap(),
                    to: span[1].as_i64().unwrap(),
                },
            );
        }
        finish(&store);
        let head = store.thread_head(
            &lease,
            room,
            root,
            &kinds,
            case["limit"].as_u64().unwrap() as usize,
            case["live_at"].as_i64(),
        );
        let expect = &case["expect"];
        assert_eq!(head.certain, expect["certain"].as_bool().unwrap(), "{name}");
        let rows: Vec<&str> = head
            .rows
            .iter()
            .map(|event| event["id"].as_str().unwrap())
            .collect();
        let expected: Vec<&str> = expect["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_str().unwrap())
            .collect();
        assert_eq!(rows, expected, "{name}: rows");
        assert_eq!(
            head.has_more,
            expect["has_more"].as_bool().unwrap(),
            "{name}"
        );
        let cursor = &expect["next_cursor"];
        assert_eq!(
            head.next_cursor,
            cursor.as_array().map(|pair| (
                pair[0].as_i64().unwrap(),
                pair[1].as_str().unwrap().to_owned()
            )),
            "{name}: cursor"
        );
    }
}
