//! Unit tests for the user-stream REQ (shape, CLOSED table, snapshot parts).

use super::*;

fn fixture() -> serde_json::Value {
    // The client contract's request and frozen kinds (test-fixtures/user-stream.json).
    serde_json::json!({
        "frozenKinds": [5, 7, 9, 9005, 39005, 40001, 40002, 40003, 40008, 40009, 40099, 45001, 45003, 48100, 48101, 48102, 48103],
        "request": {
            "#h": ["$joined"],
            "kinds": [5, 7, 9, 9005, 39005, 40001, 40002, 40003, 40008, 40009, 40099, 45001, 45003, 48100, 48101, 48102, 48103],
            "limit": 0
        }
    })
}

fn filter(value: serde_json::Value) -> Filter {
    serde_json::from_value(value).expect("filter")
}

#[test]
fn frozen_kinds_match_the_client_contract() {
    let kinds: Vec<u16> = serde_json::from_value(fixture()["frozenKinds"].clone()).unwrap();
    assert_eq!(kinds, FROZEN_KINDS);
}

#[test]
fn the_contract_request_is_a_valid_stream_req() {
    let req = filter(fixture()["request"].clone());
    assert!(is_stream_req(std::slice::from_ref(&req)));
    assert_eq!(check_shape(&[req]), Ok(()));
}

#[test]
fn kinds_compare_as_a_set() {
    let mut req = fixture()["request"].clone();
    let mut kinds: Vec<u16> = FROZEN_KINDS.to_vec();
    kinds.reverse();
    kinds.push(9);
    req["kinds"] = kinds.into();
    assert_eq!(check_shape(&[filter(req)]), Ok(()));
}

#[test]
fn refusals_name_their_detail() {
    let base = fixture()["request"].clone();
    let with = |key: &str, value: serde_json::Value| {
        let mut req = base.clone();
        req[key] = value;
        filter(req)
    };
    let without = |key: &str| {
        let mut req = base.clone();
        req.as_object_mut().unwrap().remove(key);
        filter(req)
    };
    let mut fewer = FROZEN_KINDS.to_vec();
    fewer.pop();
    for (filters, expected) in [
        (vec![with("kinds", fewer.into())], reason::KINDS),
        (vec![with("kinds", serde_json::json!([1]))], reason::KINDS),
        (vec![without("kinds")], reason::KINDS),
        (vec![with("limit", 10.into())], reason::SHAPE),
        (vec![without("limit")], reason::SHAPE),
        (vec![with("since", 1.into())], reason::SHAPE),
        (
            vec![with("#h", serde_json::json!(["$joined", "x"]))],
            reason::SHAPE,
        ),
        (vec![with("#p", serde_json::json!(["a"]))], reason::SHAPE),
        (
            vec![filter(base.clone()), filter(base.clone())],
            reason::SHAPE,
        ),
    ] {
        assert!(is_stream_req(&filters));
        assert_eq!(check_shape(&filters), Err(expected), "{filters:?}");
        assert!(expected.starts_with("invalid: user-stream "));
    }
    // Not a stream REQ at all: ordinary chunk REQs are untouched.
    let chunk = filter(serde_json::json!({"#h": [Uuid::new_v4().to_string()], "kinds": [9]}));
    assert!(!is_stream_req(&[chunk]));
}

#[test]
fn closed_reasons_match_the_contract_strings() {
    assert_eq!(reason::OFF, "unsupported: user-stream off");
    assert_eq!(reason::NOT_ENABLED, "unsupported: user-stream not enabled");
    assert_eq!(
        reason::TOO_MANY_ROOMS,
        "unsupported: user-stream too many rooms"
    );
    assert_eq!(reason::OVERFLOW, "error: user-stream overflow");
    assert_eq!(reason::LAGGED, "error: user-stream lagged");
    assert_eq!(reason::CLOSED, "error: user-stream closed");
    for detail in [reason::KINDS, reason::SHAPE, reason::REPEAT, reason::SECOND] {
        assert!(detail.starts_with("invalid: user-stream "));
    }
}

#[test]
fn queue_cap_is_a_quarter_of_the_send_buffer_up_to_250() {
    assert_eq!(queue_cap(1000), 250);
    assert_eq!(queue_cap(4000), 250);
    assert_eq!(queue_cap(400), 100);
    assert_eq!(queue_cap(2), 1);
}

#[test]
fn one_part_with_no_rooms() {
    let parts = snapshot_parts(StreamMode::Live, "e1", &BTreeMap::new());
    assert_eq!(parts.len(), 1);
    let part: serde_json::Value = serde_json::from_str(&parts[0]).unwrap();
    assert_eq!(
        part,
        serde_json::json!({
            "v": 1, "op": "snapshot", "part": 1, "parts": 1, "mode": "live",
            "epoch": "e1", "kinds": FROZEN_KINDS.to_vec(), "rooms": {}
        })
    );
}

#[test]
fn parts_hold_at_most_500_rooms_and_later_parts_only_rooms() {
    let rooms: BTreeMap<Uuid, i64> = (0..1001u128)
        .map(|i| (Uuid::from_u128(i + 1), i as i64))
        .collect();
    let parts = snapshot_parts(StreamMode::Gated, "e2", &rooms);
    assert_eq!(parts.len(), 3);
    let parsed: Vec<serde_json::Value> = parts
        .iter()
        .map(|p| serde_json::from_str(p).unwrap())
        .collect();
    assert_eq!(parsed[0]["parts"], 3);
    assert_eq!(parsed[0]["mode"], "gated");
    assert_eq!(parsed[0]["rooms"].as_object().unwrap().len(), 500);
    assert_eq!(
        parsed[1].as_object().unwrap().keys().collect::<Vec<_>>(),
        ["rooms"]
    );
    assert_eq!(parsed[1]["rooms"].as_object().unwrap().len(), 500);
    assert_eq!(parsed[2]["rooms"].as_object().unwrap().len(), 1);
    let total: usize = parsed
        .iter()
        .map(|p| p["rooms"].as_object().unwrap().len())
        .sum();
    assert_eq!(total, 1001, "no room in two parts");
}

#[test]
fn the_largest_seq_is_kept_exactly() {
    let rooms = BTreeMap::from([(Uuid::from_u128(1), MAX_SEQ)]);
    let part: serde_json::Value =
        serde_json::from_str(&snapshot_parts(StreamMode::Shadow, "e", &rooms)[0]).unwrap();
    assert_eq!(
        part["rooms"][Uuid::from_u128(1).to_string()],
        9_007_199_254_740_991i64
    );
}

#[path = "user_stream_db_tests.rs"]
mod db;
