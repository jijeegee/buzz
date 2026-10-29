use std::collections::HashMap;

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
        Input::events(vec![event("a1", 1, json!([["h", "r"]]))]),
    );
    store.submit(
        &theirs,
        Input::events(vec![event("b1", 1, json!([["h", "r"]]))]),
    );
    store.submit(
        &theirs,
        Input::events(vec![event("b2", 2, json!([["h", "r"]]))]),
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
        Input::events(vec![event("e1", 1, json!([["h", "r"]]))]),
    );
    store.submit(
        &mine,
        Input::events(vec![event("a1", 1, json!([["h", "r"]]))]),
    );
    store.submit(
        &theirs,
        Input::events(vec![event("b1", 1, json!([["h", "r"]]))]),
    );
    finish(&store);
    // The sign-out names the server as the app has it, not normalised.
    store.forget("https://relay.example/", ACCOUNT);
    assert!(!store.path_of(&mine).exists());
    assert!(store.path_of(&theirs).exists());
    // The same account on another server keeps its file and keeps recording.
    store.submit(
        &elsewhere,
        Input::events(vec![event("e2", 2, json!([["h", "r"]]))]),
    );
    finish(&store);
    assert_eq!(count(&store, &elsewhere), 2);
    // An answer to a request sent before the sign-out arrives late.
    store.submit(
        &mine,
        Input::events(vec![event("a2", 2, json!([["h", "r"]]))]),
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
        Input::events(vec![event("a3", 3, json!([["h", "r"]]))]),
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
        store.submit(&lease, Input::frame(frame));
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
        Input::events(vec![event("a1", 150, json!([["h", "r"]]))]),
    );
    store.submit(
        &lease,
        Input::Coverage {
            rooms: rooms.clone(),
            from: 100,
            to: 200,
            captured: u64::MAX,
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
    let writer = writer::Writer::start(store.path_of(&lease), ACCOUNT.to_owned());
    writer.mark_lossy_for_test();
    writer.submit(Input::Coverage {
        rooms: rooms.clone(),
        from: 200,
        to: 300,
        captured: u64::MAX,
    });
    writer.finish();
    assert!(store.resume_points(&lease, &rooms, 1000).is_empty());

    // The loss is already noted by an earlier batch: a span reaching back
    // over it is still dropped; one opened after it is kept.
    let writer = writer::Writer::start(store.path_of(&lease), ACCOUNT.to_owned());
    writer.mark_lossy_for_test();
    writer.submit(Input::events(vec![]));
    std::thread::sleep(std::time::Duration::from_millis(50));
    writer.submit(Input::Coverage {
        rooms: rooms.clone(),
        from: 300,
        to: 400,
        captured: u64::MAX,
    });
    let later = i64::MAX / 2;
    writer.submit(Input::Coverage {
        rooms: rooms.clone(),
        from: later,
        to: later,
        captured: u64::MAX,
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

#[test]
fn a_room_head_read_waits_for_queued_writes_and_distrusts_a_loss() {
    let fixture: Value = serde_json::from_str(
        &std::fs::read_to_string("../../test-fixtures/message-store-room-head.json").unwrap(),
    )
    .unwrap();
    let room = fixture["room"].as_str().unwrap();
    let kinds: Vec<u64> = fixture["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|kind| kind.as_u64().unwrap())
        .collect();
    let case = &fixture["cases"][0];
    assert!(case["expect"]["certain"].as_bool().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let fill = |store: &MessageStore| {
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
    };
    let limit = case["limit"].as_u64().unwrap() as usize;
    let live_at = case["live_at"].as_i64();

    // Read straight after queueing: the read sees what was queued.
    fill(&store);
    assert!(
        store
            .room_head(&lease, room, &kinds, limit, live_at)
            .certain
    );

    // A loss not yet written down: the read cannot vouch for the window.
    store
        .inner
        .lock()
        .unwrap()
        .writers
        .get(&lease.stem)
        .unwrap()
        .mark_lossy_for_test();
    assert!(
        !store
            .room_head(&lease, room, &kinds, limit, live_at)
            .certain
    );
    finish(&store);
}

#[test]
fn spans_an_older_build_wrote_are_dropped_and_not_trusted() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let rooms = vec!["room-1".to_owned()];
    store.submit(
        &lease,
        Input::Coverage {
            rooms: rooms.clone(),
            from: 100,
            to: 200,
            captured: u64::MAX,
        },
    );
    finish(&store);
    assert_eq!(store.resume_points(&lease, &rooms, 1000).len(), 1);

    // An older build's file: same spans, no coverage version.
    let conn = rusqlite::Connection::open(store.path_of(&lease)).unwrap();
    conn.execute("DELETE FROM sync_state WHERE key = 'coverage_version'", [])
        .unwrap();
    assert!(!super::coverage::is_current(&conn).unwrap());
    drop(conn);

    // The next writer drops them.
    store.submit(&lease, Input::events(vec![]));
    finish(&store);
    assert!(store.resume_points(&lease, &rooms, 1000).is_empty());
}

#[test]
fn coverage_is_refused_without_the_scope_of_its_eose() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let scope = store
        .capture(&store.lease(RELAY, ACCOUNT).unwrap())
        .unwrap();
    let extend = |scope: Option<&str>| {
        store.extend_coverage(RELAY, ACCOUNT, scope, vec!["room".into()], 10, 20)
    };
    assert!(extend(None).is_err(), "no scope");
    assert!(extend(Some("other:0")).is_err(), "another file");
    assert!(extend(Some(&scope)).is_ok());
    // A sign-out ends the scope: spans from before it are refused.
    store.forget(RELAY, ACCOUNT);
    assert!(extend(Some(&scope)).is_err(), "signed out since");
}

fn purge_fixture() -> Value {
    let text = include_str!("../../../../test-fixtures/message-store-purge.json");
    serde_json::from_str(text).expect("purge fixture parses")
}

/// The file as the purge fixture's `expect` describes it.
fn purge_state(store: &MessageStore, lease: &super::Lease) -> Value {
    let conn = rusqlite::Connection::open(store.path_of(lease)).unwrap();
    let column = |sql: &str| -> Vec<String> {
        let mut statement = conn.prepare(sql).unwrap();
        let rows = statement.query_map([], |r| r.get(0)).unwrap();
        rows.map(Result::unwrap).collect()
    };
    let mut coverage = serde_json::Map::new();
    let mut statement = conn
        .prepare("SELECT room, lo_at, hi_at FROM coverage ORDER BY room, lo_at")
        .unwrap();
    let spans = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .unwrap();
    for span in spans {
        let (room, lo, hi) = span.unwrap();
        coverage
            .entry(room)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(json!([lo, hi]));
    }
    json!({
        "events": column("SELECT id FROM events ORDER BY id"),
        "targets": column("SELECT DISTINCT id FROM targets ORDER BY id"),
        "summaries": column("SELECT root FROM summaries ORDER BY root"),
        "coverage": coverage,
    })
}

/// The capture a fixture step names, or one taken now.
fn step_capture(
    store: &MessageStore,
    lease: &super::Lease,
    captures: &HashMap<String, String>,
    name: &Value,
) -> String {
    name.as_str()
        .map(|name| captures[name].clone())
        .unwrap_or_else(|| store.capture(lease).unwrap())
}

fn extend_step(store: &MessageStore, account: &str, scope: &str, span: &Value) {
    let rooms = span["rooms"].as_array().unwrap();
    let rooms = rooms
        .iter()
        .map(|r| r.as_str().unwrap().to_owned())
        .collect();
    let (from, to) = (span["from"].as_i64().unwrap(), span["to"].as_i64().unwrap());
    store
        .extend_coverage(RELAY, account, Some(scope), rooms, from, to)
        .unwrap();
}

#[test]
fn purge_follows_shared_fixture() {
    let fixture = purge_fixture();
    let account = fixture["account"].as_str().unwrap();
    let relay_key = fixture["relay_key"].as_str().unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let open = || {
            let store = MessageStore::new(dir.path().to_path_buf());
            if case["relay_key_known"].as_bool() != Some(false) {
                store.note_relay_key(RELAY, relay_key);
            }
            store
        };
        let mut store = open();
        let mut lease = store.lease(RELAY, account).unwrap();
        let mut captures: HashMap<String, String> = HashMap::new();
        let seed = &fixture["seed"];
        store.submit_events(&lease, seed["events"].as_array().unwrap().clone());
        for span in seed["coverage"].as_array().unwrap() {
            extend_step(&store, account, &store.capture(&lease).unwrap(), span);
        }
        if case["seed_list"].as_bool() != Some(false) {
            let list = seed["list"].as_array().unwrap();
            let keep = list
                .iter()
                .map(|r| r.as_str().unwrap().to_owned())
                .collect();
            let scope = store.capture(&lease).unwrap();
            store
                .forget_rooms_except(RELAY, account, Some(&scope), keep)
                .unwrap();
        }
        for step in case["steps"].as_array().unwrap() {
            if step["restart"].as_bool() == Some(true) {
                finish(&store);
                store = open();
                lease = store.lease(RELAY, account).unwrap();
                captures.clear();
            } else if let Some(name) = step["capture"].as_str() {
                captures.insert(name.to_owned(), store.capture(&lease).unwrap());
            } else if let Some(events) = step["events"].as_array() {
                store.submit_events(&lease, events.clone());
            } else if let Some(span) = step.get("coverage") {
                let scope = step_capture(&store, &lease, &captures, &span["captured"]);
                extend_step(&store, account, &scope, span);
            } else if let Some(keep) = step["forget_except"].as_array() {
                let scope = match step["scope"].as_str() {
                    Some("stale") => "other:0:0".to_owned(),
                    _ => step_capture(&store, &lease, &captures, &step["captured"]),
                };
                let keep = keep.iter().map(|r| r.as_str().unwrap().to_owned());
                let result =
                    store.forget_rooms_except(RELAY, account, Some(&scope), keep.collect());
                assert_eq!(
                    result.is_err(),
                    step["refused"].as_bool().unwrap(),
                    "{name}: refused"
                );
            }
            // `forget_except: null`: the list failed and nothing is called.
        }
        finish(&store);
        assert_eq!(purge_state(&store, &lease), case["expect"], "{name}");
    }
}

#[test]
fn a_purge_is_refused_after_sign_out() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let scope = store
        .capture(&store.lease(RELAY, ACCOUNT).unwrap())
        .unwrap();
    store.forget(RELAY, ACCOUNT);
    assert!(store
        .forget_rooms_except(RELAY, ACCOUNT, Some(&scope), vec!["r".into()])
        .is_err());
    assert!(store
        .forget_rooms_except(RELAY, ACCOUNT, None, vec!["r".into()])
        .is_err());
    // A bare scope without its capture is refused too.
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    assert!(store
        .forget_rooms_except(RELAY, ACCOUNT, Some(&lease.token()), vec!["r".into()])
        .is_err());
}

fn stored_ids(store: &MessageStore, lease: &super::Lease) -> Vec<String> {
    let conn = rusqlite::Connection::open(store.path_of(lease)).unwrap();
    let mut statement = conn.prepare("SELECT id FROM events ORDER BY id").unwrap();
    let rows = statement.query_map([], |r| r.get(0)).unwrap();
    rows.map(Result::unwrap).collect()
}

const RELAY_KEY: &str = "cc00000000000000000000000000000000000000000000000000000000000003";

/// A relay system message about the account in room-a, as a socket frame.
fn notice_frame(id: &str, created_at: i64, kind: &str) -> String {
    let content = json!({"type": kind, "actor": ACCOUNT, "target": ACCOUNT});
    let event = json!({"id": id, "kind": 40099, "pubkey": RELAY_KEY, "created_at": created_at,
        "tags": [["h", "room-a"]], "content": content.to_string(), "sig": ""});
    json!(["EVENT", "history", event]).to_string()
}

/// The socket tap hands every received event to the store with the relay's
/// key once it is known: a leave and rejoin on any subscription forgets the
/// room's older rows.
#[test]
fn socket_notices_carry_the_relay_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    store.submit_events(&lease, vec![event("a1", 100, json!([["h", "room-a"]]))]);
    store.submit_frame(&lease, notice_frame("l1", 140, "member_left"));
    store.submit_frame(&lease, notice_frame("j1", 150, "member_joined"));
    finish(&store);
    assert_eq!(
        stored_ids(&store, &lease),
        ["a1", "j1", "l1"],
        "key unknown"
    );

    store.note_relay_key(RELAY, &RELAY_KEY.to_ascii_uppercase());
    store.submit_frame(&lease, notice_frame("l2", 155, "member_left"));
    store.submit_frame(&lease, notice_frame("j2", 160, "member_joined"));
    finish(&store);
    assert_eq!(stored_ids(&store, &lease), ["j2"]);
}

/// The capture a subscription's coverage is recorded with is taken as the
/// socket delivers its EOSE: a rejoin that arrives after it, before the app
/// asks, still fences the span.
#[test]
fn an_eose_capture_precedes_what_arrives_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    store.note_relay_key(RELAY, RELAY_KEY);
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    store.submit_events(&lease, vec![event("a1", 100, json!([["h", "room-a"]]))]);
    store.note_eose(&lease, "room-sub");
    store.submit_frame(&lease, notice_frame("l1", 140, "member_left"));
    store.submit_frame(&lease, notice_frame("j1", 150, "member_joined"));
    let captured = store.take_eose("room-sub").unwrap();
    assert!(store.take_eose("room-sub").is_none(), "taken once");
    store
        .extend_coverage(
            RELAY,
            ACCOUNT,
            Some(&captured),
            vec!["room-a".into()],
            50,
            200,
        )
        .unwrap();
    finish(&store);
    let conn = rusqlite::Connection::open(store.path_of(&lease)).unwrap();
    let spans: i64 = conn
        .query_row("SELECT count(*) FROM coverage", [], |r| r.get(0))
        .unwrap();
    assert_eq!(spans, 0, "the span from before the rejoin is refused");
}

/// A list dropping many rooms forgets them over several transactions, taking
/// in what is queued meanwhile, in order.
#[test]
fn a_large_purge_runs_in_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let rooms: Vec<String> = (0..40).map(|i| format!("room-{i:02}")).collect();
    let events = rooms
        .iter()
        .enumerate()
        .map(|(i, room)| event(&format!("e{i:02}"), 100, json!([["h", room]])))
        .collect();
    store.submit_events(&lease, events);
    let apply = |keep: &[String]| {
        let scope = store.capture(&lease).unwrap();
        store
            .forget_rooms_except(RELAY, ACCOUNT, Some(&scope), keep.to_vec())
            .unwrap();
    };
    apply(&rooms);
    apply(&rooms[..21]);
    store.submit_events(&lease, vec![event("late", 200, json!([["h", "room-39"]]))]);
    finish(&store);
    let ids = stored_ids(&store, &lease);
    assert_eq!(ids.len(), 22);
    assert!(
        ids.contains(&"late".to_owned()),
        "queued after the purge, kept"
    );
}

/// Lists are applied in the order their fetches began: one captured before
/// the last list applied is stale and changes nothing.
#[test]
fn a_list_older_than_the_last_applied_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    store.submit_events(
        &lease,
        vec![
            event("a1", 100, json!([["h", "room-a"]])),
            event("b1", 100, json!([["h", "room-b"]])),
        ],
    );
    let both = vec!["room-a".to_owned(), "room-b".to_owned()];
    let first = store.capture(&lease).unwrap();
    store
        .forget_rooms_except(RELAY, ACCOUNT, Some(&first), both.clone())
        .unwrap();
    let older = store.capture(&lease).unwrap();
    store.submit_events(&lease, vec![event("b2", 110, json!([["h", "room-b"]]))]);
    let newer = store.capture(&lease).unwrap();
    store
        .forget_rooms_except(RELAY, ACCOUNT, Some(&newer), both)
        .unwrap();
    store
        .forget_rooms_except(RELAY, ACCOUNT, Some(&older), vec!["room-b".into()])
        .unwrap();
    finish(&store);
    assert_eq!(stored_ids(&store, &lease), ["a1", "b1", "b2"]);
}
