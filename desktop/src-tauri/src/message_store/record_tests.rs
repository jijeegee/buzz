use rusqlite::Connection;
use serde_json::{json, Value};

use super::*;
use crate::message_store::{read, schema};

const ROOM_A: &str = "270f6caf-0feb-4055-93f3-cdbeb567ff28";
const ROOM_B: &str = "c1a7c0de-0feb-4055-93f3-cdbeb567ff28";

fn id(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

fn message(n: u8, created_at: i64, room: &str) -> Value {
    json!({"id": id(n), "pubkey": "ab".repeat(32), "created_at": created_at, "kind": 9,
           "tags": [["h", room]], "content": "hi", "sig": ""})
}

/// A bounds overlay the relay signed at 10_000, after every test fetch time.
fn bounds(room: &str, content: Value) -> Value {
    json!({"id": "ee".repeat(32), "pubkey": "cd".repeat(32), "created_at": 10_000, "kind": 39006,
           "tags": [["d", format!("{room}:head")], ["h", room]], "content": content.to_string(), "sig": ""})
}

fn open(dir: &std::path::Path) -> Connection {
    schema::open(&dir.join("s.sqlite3"), "scope").unwrap()
}

fn ranges(conn: &Connection, room: &str, class: &str) -> Vec<(i64, bool, i64)> {
    read::coverage(conn, room)
        .unwrap()
        .ranges
        .into_iter()
        .filter(|range| range.class == class)
        .map(|range| (range.lo_at, range.lo_is_start, range.hi_at))
        .collect()
}

#[test]
fn window_claims_come_from_the_bounds_overlay() {
    let more = bounds(
        ROOM_A,
        json!({"has_more": true, "next_cursor": {"created_at": 100, "id": id(1)}}),
    );
    let claim = window_claim(&[message(1, 100, ROOM_A), more.clone()], ROOM_A, None, 500).unwrap();
    assert_eq!(claim.lo.unwrap().created_at, 100);
    assert!(!claim.lo_is_start);
    assert_eq!(claim.hi, Upper::At(500));

    let below = window_claim(
        std::slice::from_ref(&more),
        ROOM_A,
        Some((200, &id(9))),
        500,
    );
    assert!(matches!(below.unwrap().hi, Upper::Before(cursor) if cursor.created_at == 200));

    let end = bounds(ROOM_A, json!({"has_more": false, "next_cursor": null}));
    assert!(
        window_claim(&[end.clone()], ROOM_A, None, 500)
            .unwrap()
            .lo_is_start
    );

    // A device clock ahead of the relay claims no further than the relay had
    // seen when it signed the bounds.
    let mut signed_earlier = end;
    signed_earlier["created_at"] = json!(450);
    assert_eq!(
        window_claim(&[signed_earlier], ROOM_A, None, 500)
            .unwrap()
            .hi,
        Upper::At(450)
    );

    // No overlay, another room's overlay, or a broken cursor prove nothing.
    assert!(window_claim(&[message(1, 100, ROOM_A)], ROOM_A, None, 500).is_none());
    assert!(window_claim(std::slice::from_ref(&more), ROOM_B, None, 500).is_none());
    let broken = bounds(
        ROOM_A,
        json!({"has_more": true, "next_cursor": {"created_at": 1, "id": "ab"}}),
    );
    assert!(window_claim(&[broken], ROOM_A, None, 500).is_none());
}

#[test]
fn batched_window_claims_are_recorded_per_room() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    let windows = [
        (
            ROOM_A,
            vec![
                message(1, 300, ROOM_A),
                bounds(ROOM_A, json!({"has_more": false, "next_cursor": null})),
            ],
        ),
        (
            ROOM_B,
            vec![
                message(2, 250, ROOM_B),
                bounds(
                    ROOM_B,
                    json!({"has_more": true, "next_cursor": {"created_at": 250, "id": id(2)}}),
                ),
            ],
        ),
    ];
    for (room, events) in &windows {
        let claim = window_claim(events, room, None, 400);
        record_room(&mut conn, room, events, Class::Top, claim.as_ref()).unwrap();
    }
    assert_eq!(ranges(&conn, ROOM_A, "top"), vec![(0, true, 400)]);
    assert_eq!(ranges(&conn, ROOM_B, "top"), vec![(250, false, 400)]);
    assert!(ranges(&conn, ROOM_A, "all").is_empty());
    let head = read::window(&conn, ROOM_B, None, 10).unwrap();
    assert_eq!(head.rows.len(), 1);
    // Bounds overlays are never stored.
    let stored: i64 = conn
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, 2);
}

fn repair(
    since: u64,
    limit: u32,
    until: Option<u64>,
    before_id: Option<&str>,
) -> RepairRequest<'_> {
    RepairRequest {
        since,
        limit,
        until,
        before_id,
    }
}

#[test]
fn repair_claims_reach_since_only_when_the_walk_completes() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    // Page 1 is full (limit 2): it proves only its own slice down to its
    // oldest event, never down to `since`.
    let page1 = vec![message(1, 300, ROOM_A), message(2, 250, ROOM_A)];
    let claim = repair_claim(&page1, &repair(100, 2, None, None), 400).unwrap();
    record_room(&mut conn, ROOM_A, &page1, Class::All, Some(&claim)).unwrap();
    assert_eq!(ranges(&conn, ROOM_A, "all"), vec![(250, false, 400)]);

    // Page 2, below page 1's oldest event, is short: the walk completed and
    // the room is proven from `since` up to the fetch time.
    let page2 = vec![message(3, 120, ROOM_A)];
    let id2 = id(2);
    let claim = repair_claim(&page2, &repair(100, 2, Some(250), Some(&id2)), 410).unwrap();
    record_room(&mut conn, ROOM_A, &page2, Class::All, Some(&claim)).unwrap();
    assert_eq!(ranges(&conn, ROOM_A, "all"), vec![(100, false, 400)]);
    // `all` proves top-level rows too.
    assert_eq!(ranges(&conn, ROOM_A, "top"), vec![(100, false, 400)]);
}

#[test]
fn repair_claim_bounds() {
    // Short first page: since .. fetch time; `since` 0 reaches the room start.
    let claim = repair_claim(&[], &repair(0, 500, None, None), 900).unwrap();
    assert!(claim.lo_is_start);
    assert_eq!(claim.hi, Upper::At(900));
    // `until` alone is inclusive and never beyond the fetch.
    let claim = repair_claim(&[], &repair(10, 500, Some(2_000), None), 900).unwrap();
    assert_eq!(claim.hi, Upper::At(900));
    // A malformed event in a full page proves nothing.
    let bad = vec![json!({"id": "x", "created_at": 5})];
    assert!(repair_claim(&bad, &repair(1, 1, None, None), 900).is_none());
}

#[test]
fn thread_pages_record_thread_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    let root = id(9);
    let claim = bounds_claim(
        &json!({"has_more": true, "next_cursor": {"created_at": 150, "id": id(4)}}),
        None,
        600,
    )
    .unwrap();
    ingest::insert_events(&mut conn, Some(ROOM_A), &[message(4, 150, ROOM_A)], 1).unwrap();
    ingest::claim_thread(&mut conn, &root, &claim).unwrap();
    let thread: Vec<(i64, i64)> = ingest::thread_ranges(&conn, &root)
        .unwrap()
        .into_iter()
        .map(|range| (range.lo_at, range.hi_at))
        .collect();
    assert_eq!(thread, vec![(150, 600)]);
    // A newer-direction page claims nothing.
    assert!(bounds_claim(&json!({"direction": "newer", "has_more": false}), None, 1).is_none());
}
