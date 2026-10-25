use std::sync::Mutex;

use super::*;
use crate::message_store::sync::{CATCH_UP_MAX_PAGES, CATCH_UP_PAGE_LIMIT};

const ROOM_A: &str = "270f6caf-0feb-4055-93f3-cdbeb567ff28";
const ROOM_B: &str = "5c3c1c1e-8d0f-4a51-9d55-0b7a9e1f6a10";

fn event(room: &str, created_at: i64, n: usize) -> Value {
    serde_json::json!({
        "id": format!("{n:064x}"),
        "created_at": created_at,
        "kind": 9,
        "tags": [["h", room]],
    })
}

fn full_page(room: &str, created_at: i64) -> Vec<Value> {
    (0..CATCH_UP_PAGE_LIMIT)
        .map(|n| event(room, created_at, n))
        .collect()
}

#[tokio::test]
async fn a_short_first_page_completes_the_chunk() {
    let rooms = vec![ROOM_A.to_string()];
    let asked = Mutex::new(Vec::new());
    let result = catch_up_chunk(&rooms, 100, |filter| {
        asked.lock().unwrap().push(filter);
        async { Ok(vec![event(ROOM_A, 150, 1)]) }
    })
    .await;
    assert!(result.complete);
    assert_eq!(result.events.len(), 1);
    let asked = asked.into_inner().unwrap();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["#h"], serde_json::json!([ROOM_A]));
    assert_eq!(asked[0]["since"], 100);
    assert_eq!(asked[0]["kinds"], serde_json::json!(SYNC_KINDS));
    assert_eq!(asked[0]["limit"], CATCH_UP_PAGE_LIMIT);
    assert!(asked[0].get("until").is_none());
}

#[tokio::test]
async fn full_pages_page_down_then_overflow_is_a_gap() {
    let rooms = vec![ROOM_A.to_string()];
    let asked = Mutex::new(Vec::new());
    let result = catch_up_chunk(&rooms, 100, |filter| {
        asked.lock().unwrap().push(filter);
        async { Ok(full_page(ROOM_A, 500)) }
    })
    .await;
    assert!(!result.complete);
    let asked = asked.into_inner().unwrap();
    assert_eq!(asked.len(), CATCH_UP_MAX_PAGES);
    // Later pages continue below the previous page's oldest event.
    assert_eq!(asked[1]["until"], 500);
    assert_eq!(
        asked[1]["before_id"],
        format!("{:064x}", CATCH_UP_PAGE_LIMIT - 1)
    );
}

#[tokio::test]
async fn a_failed_page_is_a_gap() {
    let rooms = vec![ROOM_A.to_string()];
    let calls = Mutex::new(0);
    let result = catch_up_chunk(&rooms, 100, |_| {
        let first = {
            let mut calls = calls.lock().unwrap();
            *calls += 1;
            *calls == 1
        };
        async move {
            if first {
                Ok(full_page(ROOM_A, 500))
            } else {
                Err("boom".to_string())
            }
        }
    })
    .await;
    assert!(!result.complete);
    assert_eq!(result.events.len(), CATCH_UP_PAGE_LIMIT);
}

#[test]
fn complete_chunks_claim_every_room_and_others_are_gaps() {
    let complete = ChunkResult {
        events: vec![event(ROOM_A, 150, 1), event(ROOM_B, 160, 2)],
        complete: true,
        fetched_at: 900,
    };
    let overflow = ChunkResult {
        events: vec![event(ROOM_B, 170, 3)],
        complete: false,
        fetched_at: 900,
    };
    let dispatch: HashSet<String> = [ROOM_B.to_string()].into_iter().collect();
    let (commit, dispatched, gaps) = assemble(
        vec![
            (vec![ROOM_A.to_string()], complete),
            (vec![ROOM_B.to_string()], overflow),
        ],
        100,
        &dispatch,
    );
    assert_eq!(commit.events.len(), 3);
    assert_eq!(
        commit.claims,
        vec![(ROOM_A.to_string(), sync::chunk_claim(100, 900))]
    );
    assert_eq!(gaps, vec![ROOM_B.to_string()]);
    assert_eq!(dispatched.len(), 2);
    assert!(dispatched
        .iter()
        .all(|event| room_of(event) == Some(ROOM_B)));
}

#[test]
fn rooms_are_validated_and_deduplicated() {
    assert!(valid_rooms(vec!["not-a-room".to_string()]).is_err());
    let rooms = valid_rooms(vec![
        ROOM_A.to_string(),
        ROOM_A.to_ascii_uppercase(),
        ROOM_B.to_string(),
    ])
    .unwrap();
    assert_eq!(rooms, vec![ROOM_A.to_string(), ROOM_B.to_string()]);
    let too_many = (0..=MAX_CATCH_UP_ROOMS)
        .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
        .collect();
    assert!(valid_rooms(too_many).is_err());
}
