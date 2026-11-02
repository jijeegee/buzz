//! `room_seq` and the stream's `sync_state` keys against
//! test-fixtures/user-stream.json (`ruleW`, `epoch`, `clearing`,
//! `skipCoverage`); docs/message-sync.md#watermark-room_seq.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::params;
use serde_json::{json, Value};

use super::{classify, coverage, room_seq, writer, Input, Lease, MessageStore, SeqWrite};

const ACCOUNT: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const RELAY: &str = "wss://relay.example";

fn fixture() -> Value {
    let text = include_str!("../../../../test-fixtures/user-stream.json");
    serde_json::from_str(text).expect("fixture parses")
}

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

fn mark_lossy(store: &MessageStore, lease: &Lease) {
    store
        .inner
        .lock()
        .unwrap()
        .writers
        .get(&lease.stem)
        .expect("writer running")
        .mark_lossy_for_test();
}

/// Seeds the file before any writer runs: one event per room (so a list
/// can forget it), rows and an epoch.
fn seed(
    store: &MessageStore,
    lease: &Lease,
    rooms: &[String],
    rows: &[(String, i64, i64)],
    epoch: Option<&str>,
) {
    let mut conn = writer::open(&store.path_of(lease)).unwrap();
    let events: Vec<_> = rooms
        .iter()
        .enumerate()
        .filter_map(|(index, room)| {
            classify(
                &json!({"id": format!("seed-{index}"), "kind": 9, "pubkey": "p1",
                "created_at": 10, "tags": [["h", room]], "content": "", "sig": ""}),
            )
        })
        .collect();
    writer::write(&mut conn, &events).unwrap();
    for (room, seq, at) in rows {
        conn.execute(
            "INSERT INTO room_seq(room, seq, at) VALUES (?1, ?2, ?3)",
            params![room, seq, at],
        )
        .unwrap();
    }
    if let Some(epoch) = epoch {
        room_seq::set_state(&conn, "stream_epoch", epoch).unwrap();
    }
}

fn stored_rows(store: &MessageStore, lease: &Lease) -> BTreeMap<String, (i64, i64)> {
    let conn = writer::open(&store.path_of(lease)).unwrap();
    let mut statement = conn.prepare("SELECT room, seq, at FROM room_seq").unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn stored_epoch(store: &MessageStore, lease: &Lease) -> Option<String> {
    let conn = writer::open(&store.path_of(lease)).unwrap();
    room_seq::epoch(&conn).unwrap()
}

fn expect_rows(value: &Value) -> BTreeMap<String, (i64, i64)> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(room, row)| {
            (
                room.clone(),
                (row[0].as_i64().unwrap(), row[1].as_i64().unwrap()),
            )
        })
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_owned())
        .collect()
}

fn seqs(value: &Value) -> HashMap<String, i64> {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(room, seq)| (room.clone(), seq.as_i64().unwrap()))
        .collect()
}

/// The app side of rule W (which snapshot vouches for a catch-up) is
/// modelled here; the store's own refusals are what is under test.
struct Snapshot {
    epoch: String,
    rooms: HashMap<String, i64>,
    eose: bool,
}

struct CatchUp {
    rooms: Vec<String>,
    live_at: i64,
    capture: Option<String>,
    vouch: Option<(String, HashMap<String, i64>)>,
}

#[test]
fn rule_w_follows_shared_fixture() {
    for case in fixture()["ruleW"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = MessageStore::new(dir.path().to_path_buf());
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        let steps = case["steps"].as_array().unwrap();
        let mut rooms: Vec<String> = vec!["room-keep".into()];
        let mut seeded_rows = Vec::new();
        let mut seeded_epoch = None;
        for step in steps {
            for key in ["rooms"] {
                match &step[key] {
                    Value::Object(map) => rooms.extend(map.keys().cloned()),
                    Value::Array(list) => rooms.extend(strings(&Value::Array(list.clone()))),
                    _ => {}
                }
            }
            match step["op"].as_str().unwrap() {
                "seedRow" => seeded_rows.push((
                    step["room"].as_str().unwrap().to_owned(),
                    step["row"][0].as_i64().unwrap(),
                    step["row"][1].as_i64().unwrap(),
                )),
                "seedEpoch" => seeded_epoch = step["epoch"].as_str().map(str::to_owned),
                _ => {}
            }
        }
        rooms.sort();
        rooms.dedup();
        seed(
            &store,
            &lease,
            &rooms,
            &seeded_rows,
            seeded_epoch.as_deref(),
        );
        // The first membership list is only recorded; a later one forgets.
        let first = store.capture(&lease);
        store
            .forget_rooms_except(RELAY, ACCOUNT, first.as_deref(), rooms.clone())
            .unwrap();

        let mut snapshots: HashMap<i64, Snapshot> = HashMap::new();
        let mut catch_ups: HashMap<String, CatchUp> = HashMap::new();
        let mut kept: HashSet<String> = rooms.iter().cloned().collect();
        let mut skipped_order = 0;
        for step in steps {
            match step["op"].as_str().unwrap() {
                "part1" => {
                    let epoch = step["epoch"].as_str().unwrap().to_owned();
                    assert!(store.stream_epoch(&lease, epoch.clone()), "{name}");
                    let snapshot = Snapshot {
                        epoch,
                        rooms: seqs(&step["rooms"]),
                        eose: false,
                    };
                    snapshots.insert(step["gen"].as_i64().unwrap(), snapshot);
                }
                "streamEose" => {
                    let gen = step["gen"].as_i64().unwrap();
                    snapshots.get_mut(&gen).unwrap().eose = true;
                }
                "closed" | "seedRow" | "seedEpoch" => {}
                "catchUpStart" => {
                    let gen = step["gen"].as_i64().unwrap();
                    let vouch = snapshots
                        .get(&gen)
                        .filter(|snapshot| snapshot.eose)
                        .map(|snapshot| (snapshot.epoch.clone(), snapshot.rooms.clone()));
                    catch_ups.insert(
                        step["id"].as_str().unwrap().to_owned(),
                        CatchUp {
                            rooms: strings(&step["rooms"]),
                            live_at: step["liveAt"].as_i64().unwrap(),
                            capture: store.capture(&lease),
                            vouch,
                        },
                    );
                }
                "purge" => {
                    kept.remove(step["room"].as_str().unwrap());
                    let capture = store.capture(&lease);
                    store
                        .forget_rooms_except(
                            RELAY,
                            ACCOUNT,
                            capture.as_deref(),
                            kept.iter().cloned().collect(),
                        )
                        .unwrap();
                }
                "loss" => mark_lossy(&store, &lease),
                "catchUpEnd" => {
                    let catch_up = &catch_ups[step["id"].as_str().unwrap()];
                    if !step["completed"].as_bool().unwrap() {
                        continue;
                    }
                    let Some((epoch, seqs)) = catch_up.vouch.clone() else {
                        skipped_order += catch_up.rooms.len();
                        continue;
                    };
                    store
                        .extend_stream_coverage(
                            RELAY,
                            ACCOUNT,
                            catch_up.capture.as_deref(),
                            catch_up.rooms.clone(),
                            catch_up.live_at - 100,
                            catch_up.live_at,
                            SeqWrite { epoch, seqs },
                        )
                        .unwrap();
                }
                other => panic!("{name}: unknown op {other}"),
            }
        }
        // Any input after a loss notes it, as the next receive would.
        store.submit(&lease, Input::events(vec![]));
        finish(&store);
        let expect = &case["expect"];
        assert_eq!(
            stored_rows(&store, &lease),
            expect_rows(&expect["rows"]),
            "{name}: rows"
        );
        assert_eq!(
            skipped_order as i64,
            expect["seqWriteSkippedOrder"].as_i64().unwrap(),
            "{name}: seqWriteSkippedOrder"
        );
    }
}

#[test]
fn epoch_follows_shared_fixture() {
    for case in fixture()["epoch"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = MessageStore::new(dir.path().to_path_buf());
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        let before: Vec<_> = expect_rows(&case["rowsBefore"])
            .into_iter()
            .map(|(room, (seq, at))| (room, seq, at))
            .collect();
        seed(&store, &lease, &[], &before, case["storedEpoch"].as_str());
        let part1 = &case["part1"];
        if part1["accept"].as_bool().unwrap() {
            let epoch = part1["epoch"].as_str().unwrap().to_owned();
            assert!(store.stream_epoch(&lease, epoch), "{name}");
        }
        finish(&store);
        let expect = &case["expect"];
        assert_eq!(
            stored_epoch(&store, &lease).as_deref(),
            expect["storedEpoch"].as_str(),
            "{name}: epoch"
        );
        assert_eq!(
            stored_rows(&store, &lease),
            expect_rows(&expect["rows"]),
            "{name}: rows"
        );
    }
}

/// Seeds rows for two rooms under epoch `e1`, with their coverage.
fn seeded(dir: &std::path::Path) -> (MessageStore, Lease, Vec<String>) {
    let store = MessageStore::new(dir.to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let rooms = vec![
        "room-a".to_owned(),
        "room-b".to_owned(),
        "room-c".to_owned(),
    ];
    let rows = vec![("room-a".into(), 7, 1000), ("room-b".into(), 3, 900)];
    seed(&store, &lease, &rooms, &rows, Some("e1"));
    let first = store.capture(&lease);
    store
        .forget_rooms_except(RELAY, ACCOUNT, first.as_deref(), rooms.clone())
        .unwrap();
    (store, lease, rooms)
}

fn write_e1(store: &MessageStore, capture: Option<&str>, from: i64) -> Result<(), String> {
    store.extend_stream_coverage(
        RELAY,
        ACCOUNT,
        capture,
        vec!["room-a".into(), "room-b".into()],
        from,
        2000,
        SeqWrite {
            epoch: "e1".into(),
            seqs: HashMap::from([("room-a".into(), 8), ("room-b".into(), 4)]),
        },
    )
}

#[test]
fn every_desktop_clearing_event_clears_rows_and_refuses_writes_in_flight() {
    for case in fixture()["clearing"].as_array().unwrap() {
        let event = case["event"].as_str().unwrap();
        let desktop = strings(&case["platforms"]).contains(&"desktop".to_owned());
        if !desktop {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let (store, lease, rooms) = seeded(dir.path());
        let capture = store.capture(&lease);
        let left = match event {
            // A failed write transaction notes the loss the same way
            // (`Loss::note` via `failed`/`flush`), so both are one path.
            "queueFull" | "writeFailed" => {
                store.submit(&lease, Input::events(vec![]));
                mark_lossy(&store, &lease);
                write_e1(&store, capture.as_deref(), 1500).unwrap();
                BTreeMap::new()
            }
            "purge" => {
                let keep = rooms.iter().filter(|room| *room != "room-a").cloned();
                let now = store.capture(&lease);
                store
                    .forget_rooms_except(RELAY, ACCOUNT, now.as_deref(), keep.collect())
                    .unwrap();
                write_e1(&store, capture.as_deref(), 1500).unwrap();
                // room-b's write is not refused; room-a's is.
                BTreeMap::from([("room-b".to_owned(), (4, 2000))])
            }
            "coverageVersion" => {
                finish(&store);
                let conn = writer::open(&store.path_of(&lease)).unwrap();
                conn.execute("DELETE FROM sync_state WHERE key = 'coverage_version'", [])
                    .unwrap();
                drop(conn);
                drop(writer::open(&store.path_of(&lease)).unwrap());
                BTreeMap::new()
            }
            "signOut" | "scopeEnd" => {
                store.forget(RELAY, ACCOUNT);
                assert!(
                    write_e1(&store, capture.as_deref(), 1500).is_err(),
                    "{event}"
                );
                BTreeMap::new()
            }
            "epochChange" => {
                assert!(store.stream_epoch(&lease, "e2".into()));
                write_e1(&store, capture.as_deref(), 1500).unwrap();
                BTreeMap::new()
            }
            other => panic!("unknown clearing event {other}"),
        };
        store.submit(&lease, Input::events(vec![]));
        finish(&store);
        assert_eq!(stored_rows(&store, &lease), left, "{event}");
        let scope = case["scope"].as_str().unwrap();
        assert_eq!(scope == "room", event == "purge", "{event}: scope");
    }
}

#[test]
fn skip_coverage_follows_shared_fixture() {
    for case in fixture()["skipCoverage"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = MessageStore::new(dir.path().to_path_buf());
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        let room = case["room"].as_str().unwrap().to_owned();
        let before = &case["before"];
        let row = (
            room.clone(),
            before["row"][0].as_i64().unwrap(),
            before["row"][1].as_i64().unwrap(),
        );
        seed(
            &store,
            &lease,
            std::slice::from_ref(&room),
            &[row],
            Some("e1"),
        );
        for segment in before["segments"].as_array().unwrap() {
            let capture = store.capture(&lease);
            let (lo, hi) = (segment[0].as_i64().unwrap(), segment[1].as_i64().unwrap());
            store
                .extend_coverage(
                    RELAY,
                    ACCOUNT,
                    capture.as_deref(),
                    vec![room.clone()],
                    lo,
                    hi,
                )
                .unwrap();
        }
        let l = case["L"].as_i64().unwrap();
        let capture = store.capture(&lease);
        match case["verdict"].as_str().unwrap() {
            // A skip: only the stream's own [L, L], no rule-W row.
            "skip" => store
                .extend_coverage(RELAY, ACCOUNT, capture.as_deref(), vec![room.clone()], l, l)
                .unwrap(),
            "walkCompleted" => store
                .extend_stream_coverage(
                    RELAY,
                    ACCOUNT,
                    capture.as_deref(),
                    vec![room.clone()],
                    case["since"].as_i64().unwrap(),
                    l,
                    SeqWrite {
                        epoch: "e1".into(),
                        seqs: HashMap::from([(room.clone(), case["S"].as_i64().unwrap())]),
                    },
                )
                .unwrap(),
            other => panic!("{name}: unknown verdict {other}"),
        }
        finish(&store);
        let expect = &case["expect"];
        let conn = writer::open(&store.path_of(&lease)).unwrap();
        let segments: Vec<Value> = coverage::segments(&conn, &room)
            .unwrap()
            .into_iter()
            .map(|(lo, hi)| json!([lo, hi]))
            .collect();
        assert_eq!(
            Value::Array(segments),
            expect["segments"],
            "{name}: segments"
        );
        let row = &expect["row"];
        assert_eq!(
            stored_rows(&store, &lease).get(&room).copied(),
            Some((row[0].as_i64().unwrap(), row[1].as_i64().unwrap())),
            "{name}: row"
        );
    }
}

#[test]
fn an_equal_seq_moves_at_forward_only_and_bad_values_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let rooms = vec![
        "room-a".to_owned(),
        "room-b".to_owned(),
        "room-c".to_owned(),
    ];
    seed(&store, &lease, &rooms, &[], Some("e1"));
    let write = |seqs: &[(&str, i64)], to: i64| {
        let capture = store.capture(&lease);
        store
            .extend_stream_coverage(
                RELAY,
                ACCOUNT,
                capture.as_deref(),
                rooms.clone(),
                to - 100,
                to,
                SeqWrite {
                    epoch: "e1".into(),
                    seqs: seqs
                        .iter()
                        .map(|(room, seq)| ((*room).to_owned(), *seq))
                        .collect(),
                },
            )
            .unwrap();
    };
    write(&[("room-a", 7)], 1300);
    write(&[("room-a", 7)], 1000);
    write(&[("room-a", 7)], 1500);
    write(&[("room-b", room_seq::MAX_SEQ + 1), ("room-c", -1)], 1500);
    write(&[("room-c", room_seq::MAX_SEQ)], 1500);
    let read = store.stream_seqs(&lease, &rooms, None);
    assert_eq!(read.rows.get("room-a"), Some(&(7, 1500)));
    assert_eq!(read.rows.get("room-b"), None);
    assert_eq!(read.rows.get("room-c"), Some(&(room_seq::MAX_SEQ, 1500)));
    // An epoch over 64 UTF-16 units is refused, at the queue and the span.
    assert!(!store.stream_epoch(&lease, "e".repeat(65)));
    assert!(store.stream_epoch(&lease, "\u{d55c}".repeat(64)));
    let long = SeqWrite {
        epoch: "\u{1f600}".repeat(33),
        seqs: HashMap::new(),
    };
    let capture = store.capture(&lease);
    assert!(store
        .extend_stream_coverage(
            RELAY,
            ACCOUNT,
            capture.as_deref(),
            rooms.clone(),
            1,
            2,
            long
        )
        .is_err());
    finish(&store);
    // The new epoch cleared every row.
    assert_eq!(stored_rows(&store, &lease), BTreeMap::new());
    assert_eq!(stored_epoch(&store, &lease), Some("\u{d55c}".repeat(64)));
}

#[test]
fn rows_written_before_an_epoch_change_read_with_their_epoch() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let rooms = vec!["room-a".to_owned()];
    seed(&store, &lease, &rooms, &[], Some("e1"));
    let capture = store.capture(&lease);
    store
        .extend_stream_coverage(
            RELAY,
            ACCOUNT,
            capture.as_deref(),
            rooms.clone(),
            900,
            1000,
            SeqWrite {
                epoch: "e1".into(),
                seqs: HashMap::from([("room-a".into(), 7)]),
            },
        )
        .unwrap();
    store.submit(&lease, Input::events(vec![]));
    let read = store.stream_seqs(&lease, &rooms, None);
    assert_eq!(read.epoch.as_deref(), Some("e1"));
    assert_eq!(read.rows.get("room-a"), Some(&(7, 1000)));
    let segments = store.walk_segments(&lease, &rooms).unwrap();
    assert_eq!(segments["room-a"], vec![(900, 1000)]);
    // Resume points ride the same drained read.
    let read = store.stream_seqs(&lease, &rooms, Some(1000));
    assert!(read.vouched);
    assert_eq!(read.resume.get("room-a"), Some(&1000));
    assert!(store
        .stream_seqs(&lease, &rooms, Some(900))
        .resume
        .is_empty());
    // A row ahead of the snapshot is deleted on request.
    assert!(store.clear_stream_rows(&lease, rooms.clone()));
    assert!(store.stream_seqs(&lease, &rooms, None).rows.is_empty());
    // A pending loss: nothing is vouched for, so nothing can be skipped.
    mark_lossy(&store, &lease);
    let read = store.stream_seqs(&lease, &rooms, Some(2000));
    assert!(!read.vouched && read.rows.is_empty() && read.resume.is_empty());
    assert!(store.walk_segments(&lease, &rooms).is_none());
    assert!(store.fresh_ids(&lease, &["seed-0".to_owned()], 0).is_none());
    finish(&store);
}

#[test]
fn stream_state_persists_across_restarts_and_only_known_keys() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = MessageStore::new(dir.path().to_path_buf());
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        for key in room_seq::STATE_KEYS {
            assert!(store.set_stream_state(&lease, key, format!("{{\"k\":\"{key}\"}}")));
        }
        assert!(!store.set_stream_state(&lease, "lossy", "0".into()));
        assert!(!store.set_stream_state(&lease, "stream_epoch", "e9".into()));
        finish(&store);
    }
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    for key in room_seq::STATE_KEYS {
        assert_eq!(
            store.stream_state(&lease, key),
            Some(format!("{{\"k\":\"{key}\"}}"))
        );
    }
    assert_eq!(store.stream_state(&lease, "coverage_version"), None);
    // Signing out of the server deletes them with the file.
    store.forget(RELAY, ACCOUNT);
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    assert_eq!(store.stream_state(&lease, "stream_backoff"), None);
}

#[test]
fn rows_stored_at_or_after_the_connection_start_are_fresh_whatever_stored_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let rooms = vec!["room-a".to_owned()];
    // Stored before the column existed: no stored_at, so stored before.
    seed(&store, &lease, &rooms, &[], None);
    {
        let conn = writer::open(&store.path_of(&lease)).unwrap();
        conn.execute("UPDATE events SET stored_at = NULL", [])
            .unwrap();
        conn.execute_batch(
            "INSERT INTO events(id, kind, pubkey, created_at, room, raw, stored_at)
             VALUES ('old', 9, 'p1', 20, 'room-a', '{}', 1000),
                    ('edge', 9, 'p1', 21, 'room-a', '{}', 5000)",
        )
        .unwrap();
    }
    let start = 5000;
    let ids: Vec<String> = ["seed-0", "old", "edge", "absent"]
        .iter()
        .map(|id| (*id).to_owned())
        .collect();
    let fresh = store.fresh_ids(&lease, &ids, start).unwrap();
    assert_eq!(fresh, vec!["edge".to_owned(), "absent".to_owned()]);
    // A row the writer stores now (any path: repair, catch-up, live frame).
    let event = json!({"id": "now", "kind": 9, "pubkey": "p1",
        "created_at": 30, "tags": [["h", "room-a"]], "content": "", "sig": ""});
    let before = writer::now_ms();
    store.submit(&lease, Input::events(vec![event]));
    let fresh = store
        .fresh_ids(&lease, &["now".to_owned()], before)
        .unwrap();
    assert_eq!(fresh, vec!["now".to_owned()]);
    assert!(store
        .fresh_ids(&lease, &["now".to_owned()], writer::now_ms() + 60_000)
        .unwrap()
        .is_empty());
    finish(&store);
}

#[test]
fn a_file_from_before_stored_at_gains_the_column() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::new(dir.path().to_path_buf());
    let lease = store.lease(RELAY, ACCOUNT).unwrap();
    let path = store.path_of(&lease);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE events(
               id TEXT PRIMARY KEY, kind INTEGER NOT NULL, pubkey TEXT NOT NULL,
               created_at INTEGER NOT NULL, room TEXT, root TEXT,
               broadcast INTEGER NOT NULL DEFAULT 0, raw TEXT NOT NULL);
             INSERT INTO events(id, kind, pubkey, created_at, room, raw)
               VALUES ('kept', 9, 'p1', 10, 'room-a', '{}');
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let conn = writer::open(&path).unwrap();
    let at: Option<i64> = conn
        .query_row(
            "SELECT stored_at FROM events WHERE id = 'kept'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(at, None);
    drop(conn);
    // Opening again leaves it as it is.
    writer::open(&path).unwrap();
    assert!(store
        .fresh_ids(&lease, &["kept".to_owned()], 0)
        .unwrap()
        .is_empty());
}
