//! Behaviour tests and the JSON contract shared with `e2eMessageStore.ts`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{json, Value};

use super::{
    coverage::{Claim, Cursor, Upper},
    file_stem, ingest,
    ingest::Class,
    read, remove_orphans_in, schema, MessageStore, StoreScope,
};

fn open(dir: &Path) -> Connection {
    schema::open(&dir.join("s.sqlite3"), "scope").unwrap()
}

fn id(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

fn event(n: u8, created_at: i64, kind: u32, tags: Value) -> Value {
    let mut all = vec![json!(["h", "room"])];
    all.extend(tags.as_array().cloned().unwrap_or_default());
    json!({"id": id(n), "pubkey": "ab".repeat(32), "created_at": created_at, "kind": kind,
           "tags": all, "content": format!("text {n}"), "sig": ""})
}

fn cursor(value: &Value) -> Option<Cursor> {
    serde_json::from_value(value.clone()).ok()
}

fn claim(step: &Value) -> Claim {
    let hi = &step["hi"];
    Claim {
        lo: cursor(&step["lo"]),
        lo_is_start: step["loIsStart"].as_bool().unwrap(),
        hi: match hi.get("at") {
            Some(at) => Upper::At(at.as_i64().unwrap()),
            None => Upper::Before(cursor(&hi["before"]).unwrap()),
        },
    }
}

/// Replaces event lists by their ids, as the TypeScript runner does.
fn project(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| match (key.as_str(), &value) {
                    ("rows" | "aux" | "summaries", Value::Array(events)) => {
                        let ids = events.iter().map(|event| event["id"].clone()).collect();
                        let key = match key.as_str() {
                            "rows" => "rowIds",
                            "aux" => "auxIds",
                            _ => "summaryIds",
                        };
                        (key.to_string(), Value::Array(ids))
                    }
                    _ => (key, project(value)),
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(project).collect()),
        other => other,
    }
}

fn run_step(conn: &mut Connection, step: &Value) -> Option<Value> {
    let text = |key: &str| step[key].as_str().unwrap().to_string();
    let int = |key: &str| step[key].as_i64().unwrap();
    let result = match step["op"].as_str().unwrap() {
        "ingest" => {
            let events = step["events"].as_array().unwrap();
            ingest::insert_events(conn, step["channelId"].as_str(), events, 1).unwrap();
            return None;
        }
        "claim" => {
            let class = if text("class") == "all" {
                Class::All
            } else {
                Class::Top
            };
            ingest::claim_room(conn, &text("channelId"), class, &claim(step)).unwrap();
            return None;
        }
        "claimThread" => {
            ingest::claim_thread(conn, &text("rootId"), &claim(step)).unwrap();
            return None;
        }
        "beginLive" => {
            let channels: Vec<String> = serde_json::from_value(step["channelIds"].clone()).unwrap();
            ingest::begin_live(conn, int("wsId"), &channels, int("since")).unwrap();
            return None;
        }
        "closeLive" => {
            ingest::close_live(conn, Some(int("wsId")), int("closedAt")).unwrap();
            return None;
        }
        "readWindow" => {
            let page = read::window(
                conn,
                &text("channelId"),
                cursor(&step["before"]).as_ref(),
                int("limit") as usize,
            );
            serde_json::to_value(page.unwrap())
        }
        "readAround" => {
            let anchor = &step["anchor"];
            let anchor = read::anchor(
                anchor["createdAt"].as_i64().unwrap(),
                anchor["eventId"].as_str().map(str::to_string),
            );
            let page = read::around(
                conn,
                &text("channelId"),
                &anchor,
                int("before") as usize,
                int("after") as usize,
            );
            serde_json::to_value(page.unwrap())
        }
        "readThread" => {
            let page = read::thread(
                conn,
                &text("rootId"),
                cursor(&step["cursor"]).as_ref(),
                int("limit") as usize,
            );
            serde_json::to_value(page.unwrap())
        }
        "coverage" => {
            let rooms: Vec<String> = serde_json::from_value(step["channelIds"].clone()).unwrap();
            let coverage: Vec<_> = rooms
                .iter()
                .map(|room| read::coverage(conn, room).unwrap())
                .collect();
            serde_json::to_value(coverage)
        }
        "loadHeads" => serde_json::to_value(
            read::heads(conn, int("rooms") as usize, int("rows") as usize).unwrap(),
        ),
        op => panic!("unknown fixture op {op}"),
    };
    Some(project(result.unwrap()))
}

#[test]
fn contract_fixtures() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/message_store");
    let mut names: Vec<_> = std::fs::read_dir(&fixtures)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    names.sort();
    assert!(names.len() >= 3, "contract fixtures missing");
    for path in names {
        let fixture: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(dir.path());
        for (index, step) in fixture["steps"].as_array().unwrap().iter().enumerate() {
            if let Some(actual) = run_step(&mut conn, step) {
                assert_eq!(actual, step["expect"], "{} step {index}", path.display());
            }
        }
    }
}

#[test]
fn read_window_never_crosses_a_gap() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    let events: Vec<Value> = (1..=6)
        .map(|n| event(n, 100 * i64::from(n), 9, json!([])))
        .collect();
    ingest::insert_events(&mut conn, None, &events, 1).unwrap();
    // Two runs, [100, 200] and [400, 600]; 300 is stored but unproven.
    let top = |lo: u8, start, hi| Claim {
        lo: Some(Cursor {
            created_at: 100 * i64::from(lo),
            event_id: id(lo),
        }),
        lo_is_start: start,
        hi: Upper::At(hi),
    };
    ingest::claim_room(&mut conn, "room", Class::Top, &top(1, true, 200)).unwrap();
    ingest::claim_room(&mut conn, "room", Class::Top, &top(4, false, 600)).unwrap();
    let head = read::window(&conn, "room", None, 50).unwrap();
    let ids: Vec<&str> = head
        .rows
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![id(6), id(5), id(4)]);
    assert!(head.gap && head.has_more);
    let below = read::window(&conn, "room", head.next_cursor.as_ref(), 50).unwrap();
    assert!(below.rows.is_empty() && below.gap);
}

#[test]
fn delete_marks_target_and_blanks_raw_in_either_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    let message = event(1, 100, 9, json!([["imeta", "url https://x/y.png"]]));
    let late = event(2, 110, 9, json!([]));
    let delete = event(3, 120, 9005, json!([["e", id(1)], ["e", id(2)]]));
    ingest::insert_events(&mut conn, None, &[message, delete], 1).unwrap();
    ingest::insert_events(&mut conn, None, &[late], 1).unwrap();
    for target in [id(1), id(2)] {
        let (deleted, raw): (bool, String) = conn
            .query_row(
                "SELECT deleted, raw FROM events WHERE id=?1",
                [&target],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let raw: Value = serde_json::from_str(&raw).unwrap();
        assert!(deleted);
        assert_eq!(raw["content"], "");
        assert_eq!(raw["tags"], json!([["h", "room"]]));
    }
}

#[test]
fn newest_thread_summary_wins() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    let summary = |n, at| event(n, at, 39005, json!([["e", id(9)]]));
    ingest::insert_events(&mut conn, None, &[summary(2, 200), summary(1, 100)], 1).unwrap();
    ingest::insert_events(&mut conn, None, &[summary(3, 150)], 1).unwrap();
    let payload: String = conn
        .query_row(
            "SELECT payload FROM thread_summary WHERE root_id=?1",
            [id(9)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&payload).unwrap()["id"],
        id(2)
    );
}

#[test]
fn room_meta_tracks_bytes_once_per_event() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    let message = event(1, 100, 9, json!([]));
    assert_eq!(
        ingest::insert_events(&mut conn, None, std::slice::from_ref(&message), 1).unwrap(),
        1
    );
    assert_eq!(
        ingest::insert_events(&mut conn, None, &[message], 1).unwrap(),
        0
    );
    let bytes: i64 = conn
        .query_row(
            "SELECT bytes_est FROM room_meta WHERE channel_id='room'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(bytes > 200 && bytes < 2_000, "{bytes}");
}

#[test]
fn segments_close_on_exit_and_at_boot() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    let rooms = vec!["room".to_string()];
    ingest::begin_live(&mut conn, 1, &rooms, 100).unwrap();
    ingest::close_live(&mut conn, Some(1), 180).unwrap();
    ingest::begin_live(&mut conn, 2, &rooms, 300).unwrap();
    ingest::note_frame(&conn, 350).unwrap();
    ingest::note_frame(&conn, 340).unwrap();
    assert!(read::coverage(&conn, "room")
        .unwrap()
        .live_covered
        .eq(&false));
    drop(conn);
    // A crash left segment 2 open: the next open closes it at the last frame.
    let mut conn = open(dir.path());
    ingest::recover_open_segments(&mut conn).unwrap();
    let coverage = read::coverage(&conn, "room").unwrap();
    let all: Vec<(i64, i64)> = coverage
        .ranges
        .iter()
        .filter(|range| range.class == "all")
        .map(|range| (range.lo_at, range.hi_at))
        .collect();
    assert_eq!(all, vec![(100, 180), (300, 350)]);
    let open: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM live_segment WHERE closed_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(open, 0);
}

fn scope(n: u8, relay: &str) -> StoreScope {
    StoreScope {
        pubkey: id(n),
        relay_url: relay.to_string(),
    }
}

#[test]
fn scope_file_names_follow_the_origin() {
    let a = file_stem(&id(1).to_uppercase(), "wss://Relay.Example/").unwrap();
    assert_eq!(a, file_stem(&id(1), "https://relay.example").unwrap());
    assert_ne!(a, file_stem(&id(1), "wss://other.example").unwrap());
    assert!(a.starts_with(&format!("{}@", id(1))));
    assert!(file_stem("not-a-key", "wss://relay.example").is_err());
}

#[tokio::test]
async fn scopes_stay_isolated_and_clear_deletes_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::default();
    let (alice, bob) = (
        scope(1, "wss://relay.example"),
        scope(2, "wss://relay.example"),
    );
    let message = event(7, 100, 9, json!([]));
    store
        .call(dir.path(), &alice, move |conn| {
            ingest::insert_events(conn, None, &[message], 1)?;
            ingest::claim_room(
                conn,
                "room",
                Class::Top,
                &Claim {
                    lo: None,
                    lo_is_start: true,
                    hi: Upper::At(200),
                },
            )
        })
        .await
        .unwrap();
    let root = dir.path();
    let read_rows = |scope: StoreScope| {
        let store = &store;
        async move {
            store
                .call(root, &scope, |conn| read::window(conn, "room", None, 10))
                .await
                .unwrap()
                .rows
                .len()
        }
    };
    assert_eq!(read_rows(alice.clone()).await, 1);
    assert_eq!(read_rows(bob.clone()).await, 0);

    let stem = alice.file_stem().unwrap();
    store.purge(dir.path(), &stem).unwrap();
    assert!(!dir.path().join(format!("{stem}.sqlite3")).exists());
    assert_eq!(read_rows(alice).await, 0);
    assert_eq!(read_rows(bob).await, 0);
    store.shut_down_all();
}

#[test]
fn orphan_cleanup_keeps_listed_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let names = [
        format!("{}@0123456789abcdef.sqlite3", id(1)),
        format!("{}@0123456789abcdef.sqlite3-wal", id(2)),
        format!("{}@0123456789abcdef.sqlite3.bad", id(2)),
        "notes.txt".to_string(),
    ];
    for name in &names {
        std::fs::write(dir.path().join(name), b"x").unwrap();
    }
    remove_orphans_in(dir.path(), &HashSet::from([id(1)]));
    let left: HashSet<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, HashSet::from([names[0].clone(), names[3].clone()]));
}
