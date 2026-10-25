use std::path::Path;

use serde_json::json;

use super::*;
use crate::message_store::MessageStore;

const ROOM: &str = "270f6caf-0feb-4055-93f3-cdbeb567ff28";

fn id(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

fn scope() -> StoreScope {
    StoreScope {
        pubkey: id(1),
        relay_url: "wss://relay.example".to_string(),
    }
}

fn event_frame(sub: &str, n: u8, kind: u32, tags: Value) -> String {
    json!(["EVENT", sub, {"id": id(n), "pubkey": id(2), "created_at": 100 + i64::from(n),
        "kind": kind, "tags": tags, "content": "x", "sig": ""}])
    .to_string()
}

fn live_req(sub: &str, since: i64, limit: u64) -> String {
    json!(["REQ", sub, {"kinds": SYNC_KINDS, "#h": [ROOM], "since": since, "limit": limit}])
        .to_string()
}

async fn stored_ids(store: &MessageStore, dir: &Path) -> Vec<String> {
    store
        .call(dir, &scope(), |conn| {
            let mut statement = conn
                .prepare(
                    "SELECT id FROM events UNION ALL SELECT root_id FROM thread_summary ORDER BY 1",
                )
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map([], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
        })
        .await
        .unwrap()
}

/// `(lo_at, hi_at)` of the room's `all` ranges, and its open segments.
async fn segments(store: &MessageStore, dir: &Path) -> (Vec<(i64, i64)>, i64) {
    store
        .call(dir, &scope(), |conn| {
            let mut statement = conn
                .prepare("SELECT lo_at, hi_at FROM coverage WHERE channel_id=?1 AND class='all' ORDER BY lo_at")
                .map_err(|e| e.to_string())?;
            let ranges = statement
                .query_map([ROOM], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            let open = conn
                .query_row(
                    "SELECT COUNT(*) FROM live_segment WHERE closed_at IS NULL",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            Ok((ranges, open))
        })
        .await
        .unwrap()
}

fn tap(store: &MessageStore, dir: &Path) -> StoreTap {
    StoreTap::new(7, scope(), store.writer(dir, &scope()).unwrap())
}

#[tokio::test]
async fn stores_room_events_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::default();
    let tap = tap(&store, dir.path());
    let room = json!([["h", ROOM]]);
    tap.incoming(Some(&event_frame("s", 1, 9, room.clone())));
    tap.incoming(Some(&event_frame("s", 2, 9, json!([["p", id(3)]]))));
    tap.incoming(Some(&event_frame("s", 3, 9, json!([["h", ""]]))));
    tap.incoming(Some(&event_frame("s", 4, 1, room.clone())));
    tap.incoming(Some(&event_frame("s", 5, 43003, room.clone())));
    tap.incoming(Some(&event_frame(
        "s",
        6,
        39005,
        json!([["h", ROOM], ["e", id(9)], ["d", id(9)]]),
    )));
    tap.incoming(Some(r#"["OK","x",true,""]"#));
    tap.incoming(Some("not json"));
    tap.flush();
    // Kinds 9 and job 43003 with a room tag, and the 39005 summary (by root).
    assert_eq!(
        stored_ids(&store, dir.path()).await,
        vec![id(1), id(5), id(9)]
    );
}

#[test]
fn only_a_scope_for_the_sockets_own_server_is_used() {
    let url = "wss://relay.example";
    assert_eq!(requested_scope(None, url), None);
    assert_eq!(requested_scope(Some(&json!({})), url), None);
    let config = json!({"storeScope": {"pubkey": id(1), "relayUrl": "https://relay.example/"}});
    assert_eq!(
        requested_scope(Some(&config), url),
        Some(StoreScope {
            pubkey: id(1),
            relay_url: "https://relay.example/".to_string(),
        })
    );
    assert_eq!(requested_scope(Some(&config), "wss://other.example"), None);
}

#[tokio::test]
async fn a_live_subscription_proves_its_rooms_until_the_socket_exits() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::default();
    let tap = tap(&store, dir.path());
    tap.outgoing(&live_req("live-1", 100, 1000));
    tap.incoming(Some(&event_frame("live-1", 1, 9, json!([["h", ROOM]]))));
    tap.incoming(Some(r#"["EOSE","live-1"]"#));
    tap.flush();
    assert_eq!(segments(&store, dir.path()).await, (vec![], 1));

    tap.finish();
    let (ranges, open) = segments(&store, dir.path()).await;
    assert_eq!(open, 0);
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].0, 100);
    assert!(
        (ranges[0].1 - unix_now()).abs() <= 5,
        "closes at the last frame"
    );
    // Nothing after exit is stored.
    tap.incoming(Some(&event_frame("live-1", 2, 9, json!([["h", ROOM]]))));
    tap.flush();
    assert_eq!(stored_ids(&store, dir.path()).await, vec![id(1)]);
}

#[tokio::test]
async fn close_closed_and_cut_replays_limit_what_a_segment_proves() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::default();
    let tap = tap(&store, dir.path());
    // A replay cut by its limit proves only from when the REQ was sent.
    tap.outgoing(&live_req("cut", 100, 1));
    tap.incoming(Some(&event_frame("cut", 1, 9, json!([["h", ROOM]]))));
    tap.incoming(Some(r#"["EOSE","cut"]"#));
    tap.outgoing(r#"["CLOSE","cut"]"#);
    let (ranges, open) = segments(&store, dir.path()).await;
    assert_eq!(open, 0);
    assert!(ranges[0].0 >= unix_now() - 5, "{ranges:?}");

    // CLOSED before EOSE: never live.
    tap.outgoing(&live_req("refused", 50, 1000));
    tap.incoming(Some(r#"["CLOSED","refused","rate-limited"]"#));
    tap.incoming(Some(r#"["EOSE","refused"]"#));
    // Narrower filters never become segments.
    tap.outgoing(&json!(["REQ", "authors", {"kinds": SYNC_KINDS, "#h": [ROOM], "since": 50, "limit": 9, "authors": [id(2)]}]).to_string());
    tap.outgoing(
        &json!(["REQ", "kinds", {"kinds": [9], "#h": [ROOM], "since": 50, "limit": 9}]).to_string(),
    );
    tap.incoming(Some(r#"["EOSE","authors"]"#));
    tap.incoming(Some(r#"["EOSE","kinds"]"#));
    tap.flush();
    let (after, open) = segments(&store, dir.path()).await;
    assert_eq!((after, open), (ranges, 0));
}

#[tokio::test]
async fn a_crash_closes_open_segments_at_the_last_frame() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::default();
    let tap = tap(&store, dir.path());
    tap.outgoing(&live_req("live-1", 100, 1000));
    tap.incoming(Some(r#"["EOSE","live-1"]"#));
    tap.flush();
    assert_eq!(segments(&store, dir.path()).await.1, 1);
    // No `finish`: the app died. The next open recovers the segment.
    drop(tap);
    store.shut_down_all();
    let reopened = MessageStore::default();
    let (ranges, open) = segments(&reopened, dir.path()).await;
    assert_eq!(open, 0);
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].0, 100);
    assert!((ranges[0].1 - unix_now()).abs() <= 5);
    reopened.shut_down_all();
}

async fn watermark(store: &MessageStore, dir: &Path) -> Option<i64> {
    store
        .call(dir, &scope(), |conn| {
            crate::message_store::sync::watermark(conn)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn a_socket_moves_the_watermark_only_once_caught_up() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::default();
    let tap = tap(&store, dir.path());
    tap.outgoing(&live_req("live-1", 100, 1000));
    tap.incoming(Some(r#"["EOSE","live-1"]"#));
    tap.incoming(Some(&event_frame("live-1", 1, 9, json!([["h", ROOM]]))));
    tap.flush();
    assert_eq!(watermark(&store, dir.path()).await, None);

    assert!(tap.mark_caught_up());
    // A live event vouches for its own time.
    tap.incoming(Some(&event_frame("live-1", 2, 9, json!([["h", ROOM]]))));
    tap.flush();
    assert_eq!(watermark(&store, dir.path()).await, Some(102));

    // A replayed (not yet live) event vouches for nothing; the EOSE then
    // vouches for when its REQ was sent.
    tap.outgoing(&json!(["REQ", "other", {"kinds": [9], "#h": [ROOM], "limit": 5}]).to_string());
    tap.incoming(Some(&event_frame("other", 50, 9, json!([["h", ROOM]]))));
    tap.flush();
    assert_eq!(watermark(&store, dir.path()).await, Some(102));
    tap.incoming(Some(r#"["EOSE","other"]"#));
    tap.flush();
    let at = watermark(&store, dir.path()).await.unwrap();
    assert!((at - unix_now()).abs() <= 5, "{at}");

    // An older live event never moves it back.
    tap.incoming(Some(&event_frame("live-1", 3, 9, json!([["h", ROOM]]))));
    tap.flush();
    assert_eq!(watermark(&store, dir.path()).await, Some(at));
}

#[tokio::test]
async fn a_closed_req_and_an_exited_socket_vouch_for_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = MessageStore::default();
    let tap = tap(&store, dir.path());
    assert!(tap.mark_caught_up());
    tap.outgoing(&live_req("closed", 100, 1000));
    tap.outgoing(r#"["CLOSE","closed"]"#);
    tap.incoming(Some(r#"["EOSE","closed"]"#));
    tap.flush();
    assert_eq!(watermark(&store, dir.path()).await, None);

    tap.finish();
    assert!(!tap.mark_caught_up());
    tap.incoming(Some(r#"["EOSE","closed"]"#));
    tap.flush();
    assert_eq!(watermark(&store, dir.path()).await, None);
}
