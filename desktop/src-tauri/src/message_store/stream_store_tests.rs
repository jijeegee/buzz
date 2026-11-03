//! The store's reads behind the user stream's catch-up against
//! test-fixtures/user-stream.json (`connectionSpans`);
//! docs/message-sync.md#catch-up-at-the-stream-eose.

use serde_json::Value;

use super::{coverage, writer, MessageStore};

const ACCOUNT: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const RELAY: &str = "wss://relay.example";

fn fixture() -> Value {
    let text = include_str!("../../../../test-fixtures/user-stream.json");
    serde_json::from_str(text).expect("fixture parses")
}

fn pair(value: &Value) -> (i64, i64) {
    (value[0].as_i64().unwrap(), value[1].as_i64().unwrap())
}

#[test]
fn resume_points_take_only_coverage_from_before_the_connection() {
    for case in fixture()["connectionSpans"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = MessageStore::new(dir.path().to_path_buf());
        let lease = store.lease(RELAY, ACCOUNT).unwrap();
        let mut conn = writer::open(&store.path_of(&lease)).unwrap();
        let room = vec!["room-a".to_owned()];
        for segment in case["segments"].as_array().unwrap() {
            let (lo, hi) = pair(segment);
            coverage::extend_with(&mut conn, &room, lo, hi, |_| Ok(())).unwrap();
        }
        let connected_at = case["connectedAt"].as_i64().unwrap();
        let points = store.resume_points(&lease, &room, connected_at);
        assert_eq!(
            points.get("room-a").copied(),
            case["expect"]["resume"].as_i64(),
            "{name}: resume"
        );
        let head = coverage::segments(&conn, "room-a")
            .unwrap()
            .into_iter()
            .filter(|&(lo, _)| lo < connected_at)
            .max_by_key(|&(lo, _)| lo);
        let expected = case["expect"]["head"]
            .as_array()
            .map(|_| pair(&case["expect"]["head"]));
        assert_eq!(head, expected, "{name}: head");
    }
}
