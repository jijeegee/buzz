//! The stored window a check compares, and replacing it with a relay page
//! (docs/message-sync.md#window-check).

use std::collections::HashSet;

use rusqlite::Connection;
use serde_json::{json, Value};

use super::classify::classify;
use super::window::{forget_windows, replace, stored_window, WindowReplace};
use super::writer;

const ROOM: &str = "room";

fn row(id: &str, created_at: i64) -> Value {
    json!({"id": id, "kind": 9, "pubkey": "p1", "created_at": created_at,
           "tags": [["h", ROOM]], "content": "", "sig": ""})
}

fn reply(id: &str, created_at: i64, root: &str) -> Value {
    json!({"id": id, "kind": 9, "pubkey": "p1", "created_at": created_at,
           "tags": [["h", ROOM], ["e", root, "", "reply"]], "content": "", "sig": ""})
}

fn removal(id: &str, target: &str) -> Value {
    json!({"id": id, "kind": 9005, "pubkey": "admin", "created_at": 1,
           "tags": [["h", ROOM], ["e", target]], "content": "", "sig": ""})
}

fn open(dir: &tempfile::TempDir, events: &[Value]) -> Connection {
    let mut conn = writer::open(&dir.path().join("w.db")).unwrap();
    let rows: Vec<_> = events.iter().filter_map(classify).collect();
    writer::write(&mut conn, &rows).unwrap();
    conn
}

fn ids(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|event| event["kind"] != 39006)
        .map(|event| event["id"].as_str().unwrap().to_owned())
        .collect()
}

fn page(
    root: Option<&str>,
    before: Option<(i64, &str)>,
    oldest: Option<(i64, &str)>,
    ids: &[&str],
) -> WindowReplace {
    WindowReplace {
        room: ROOM.into(),
        root: root.map(str::to_owned),
        kinds: vec![9],
        before: before.map(|(at, id)| (at, id.to_owned())),
        oldest: oldest.map(|(at, id)| (at, id.to_owned())),
        ids: ids.iter().map(|id| (*id).to_owned()).collect(),
    }
}

#[test]
fn a_room_window_counts_top_level_rows_without_deleted_ones() {
    let dir = tempfile::tempdir().unwrap();
    let conn = open(
        &dir,
        &[
            row("a", 10),
            row("b", 20),
            row("c", 30),
            reply("r", 35, "a"),
            row("d", 40),
            removal("x", "c"),
        ],
    );
    let window = stored_window(&conn, ROOM, None, &[9], 2, None).unwrap();
    assert_eq!(ids(&window.events), vec!["d", "b"]);
    assert_eq!(window.count, 2);
    assert_eq!(window.oldest, Some((20, "b".into())));
    assert!(window.has_more);
    // Nothing a relay page answered: fetched, not checked.
    assert!(!window.fully_stored);
    let bounds = window.events.last().unwrap();
    assert_eq!(bounds["tags"][0], json!(["d", "room:head"]));

    let below = stored_window(&conn, ROOM, None, &[9], 2, Some((20, "b"))).unwrap();
    assert_eq!(ids(&below.events), vec!["a"]);
    assert!(!below.has_more);
    let bounds = below.events.last().unwrap();
    assert_eq!(bounds["tags"][0], json!(["d", "room:20:b"]));
    assert_eq!(
        serde_json::from_str::<Value>(bounds["content"].as_str().unwrap()).unwrap(),
        json!({"has_more": false, "next_cursor": null})
    );
}

#[test]
fn a_page_drops_rows_it_lacks_in_its_stretch_and_vouches_for_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(
        &dir,
        &[
            row("a", 10),
            row("b", 20),
            row("gone", 25),
            row("c", 30),
            row("live", 50),
        ],
    );
    // The relay's newest 2: c and b; `gone` was deleted there. `live` was
    // written after the request went out.
    let fresh: HashSet<String> = ["live".to_owned()].into();
    replace(
        &mut conn,
        &page(None, None, Some((20, "b")), &["c", "b"]),
        &fresh,
    )
    .unwrap();
    let window = stored_window(&conn, ROOM, None, &[9], 3, None).unwrap();
    assert_eq!(ids(&window.events), vec!["live", "c", "b"]);
    assert!(window.fully_stored);
    // The row below the page's stretch is kept, but nothing vouches for it.
    let below = stored_window(&conn, ROOM, None, &[9], 3, Some((20, "b"))).unwrap();
    assert_eq!(ids(&below.events), vec!["a"]);
    assert!(!below.fully_stored);
}

#[test]
fn a_last_page_opens_the_lower_bound_and_joins_the_page_above() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(
        &dir,
        &[row("old", 5), row("a", 10), row("b", 20), row("c", 30)],
    );
    let none = HashSet::new();
    replace(
        &mut conn,
        &page(None, None, Some((20, "b")), &["c", "b"]),
        &none,
    )
    .unwrap();
    // Below b the relay has only a: `old` is outside the reader's history.
    replace(&mut conn, &page(None, Some((20, "b")), None, &["a"]), &none).unwrap();
    let below = stored_window(&conn, ROOM, None, &[9], 2, Some((20, "b"))).unwrap();
    assert_eq!(ids(&below.events), vec!["a"]);
    assert!(below.fully_stored, "a short window down to the start");
    // One merged span from the newest end to the start.
    let spans: i64 = conn
        .query_row(
            "SELECT count(*) FROM coverage WHERE room = ?1 AND stream = 'page'",
            [ROOM],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(spans, 1);
    let all = stored_window(&conn, ROOM, None, &[9], 50, None).unwrap();
    assert_eq!(all.count, 3);
    assert!(all.fully_stored);
}

#[test]
fn a_short_window_needs_the_start_of_the_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(&dir, &[row("a", 10), row("b", 20)]);
    replace(
        &mut conn,
        &page(None, None, Some((10, "a")), &["b", "a"]),
        &HashSet::new(),
    )
    .unwrap();
    // The relay said more remain below a; the store has none of them.
    let short = stored_window(&conn, ROOM, None, &[9], 5, None).unwrap();
    assert_eq!(short.count, 2);
    assert!(!short.fully_stored);
}

#[test]
fn a_thread_window_is_its_replies_without_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(
        &dir,
        &[
            row("root", 1),
            reply("r1", 10, "root"),
            reply("r2", 20, "root"),
            reply("gone", 15, "root"),
            row("top", 30),
        ],
    );
    let before = stored_window(&conn, ROOM, Some("root"), &[9], 50, None).unwrap();
    assert_eq!(ids(&before.events), vec!["r2", "gone", "r1"]);
    assert!(!before.fully_stored);
    replace(
        &mut conn,
        &page(Some("root"), None, None, &["r2", "r1"]),
        &HashSet::new(),
    )
    .unwrap();
    let after = stored_window(&conn, ROOM, Some("root"), &[9], 50, None).unwrap();
    assert_eq!(ids(&after.events), vec!["r2", "r1"]);
    assert_eq!(after.oldest, Some((10, "r1".into())));
    assert!(after.fully_stored);
    // The room's rows are untouched by a thread page.
    let room = stored_window(&conn, ROOM, None, &[9], 50, None).unwrap();
    assert_eq!(ids(&room.events), vec!["top", "root"]);
}

#[test]
fn forgetting_a_rooms_windows_makes_them_fetched_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(&dir, &[row("a", 10)]);
    replace(&mut conn, &page(None, None, None, &["a"]), &HashSet::new()).unwrap();
    assert!(
        stored_window(&conn, ROOM, None, &[9], 50, None)
            .unwrap()
            .fully_stored
    );
    forget_windows(&mut conn, &[ROOM.to_owned()]).unwrap();
    assert!(
        !stored_window(&conn, ROOM, None, &[9], 50, None)
            .unwrap()
            .fully_stored
    );
}
