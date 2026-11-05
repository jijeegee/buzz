// Exercises goal edits and reads through the production HTTP router.
use super::postgres_tests::bridge_handler_test_state;
use super::*;
use axum::http::StatusCode;
use kuzz_core::goal_tree::{GoalOp, GoalStatus};
use kuzz_core::kind::{KIND_GOAL_OPS, KIND_GOAL_TREE};
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};
use uuid::Uuid;

struct Fixture {
    state: Arc<crate::state::AppState>,
    pool: sqlx::PgPool,
    host: String,
    community: kuzz_core::CommunityId,
    open: Uuid,
    private: Uuid,
    owner: Keys,
    stranger: Keys,
}

impl Fixture {
    async fn new(goal_store: bool) -> Self {
        let base = bridge_handler_test_state()
            .await
            .expect("Postgres and Redis");
        let mut state = (*base).clone();
        let mut config = (*state.config).clone();
        config.goal_store = goal_store;
        state.config = Arc::new(config);
        let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
            .await
            .unwrap();
        let host = format!("goals-{}.local", Uuid::new_v4());
        state.db.ensure_configured_community(&host).await.unwrap();
        state.db.ensure_future_partitions(1, true).await.unwrap();
        let tenant = crate::tenant::bind_community(&state.db, &host)
            .await
            .unwrap();
        let community = tenant.community();
        let (open, private) = (Uuid::new_v4(), Uuid::new_v4());
        let owner = Keys::generate();
        for (id, visibility) in [(open, "open"), (private, "private")] {
            sqlx::query("INSERT INTO channels(community_id,id,name,visibility,channel_type,created_by,history_policy,goals_enabled) VALUES($1,$2,$2::text,$3::channel_visibility,'stream',$4,'all',TRUE)")
                .bind(community.as_uuid()).bind(id).bind(visibility).bind(owner.public_key().as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO channel_members(community_id,channel_id,pubkey,role) VALUES($1,$2,$3,'owner')")
                .bind(community.as_uuid()).bind(id).bind(owner.public_key().as_bytes().as_slice()).execute(&pool).await.unwrap();
        }
        Self {
            state: Arc::new(state),
            pool,
            host,
            community,
            open,
            private,
            owner,
            stranger: Keys::generate(),
        }
    }

    async fn request(&self, key: &Keys, path: &str, body: Value) -> (StatusCode, Value) {
        use tower::ServiceExt;
        let response = crate::router::build_router(self.state.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("host", &self.host)
                    .header("content-type", "application/json")
                    .header("x-pubkey", key.public_key().to_hex())
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    fn event(&self, key: &Keys, channel: Uuid, kind: u32, content: String) -> Event {
        EventBuilder::new(Kind::Custom(kind as u16), content)
            .tags([Tag::parse(["h", &channel.to_string()]).unwrap()])
            .sign_with_keys(key)
            .unwrap()
    }

    fn ops_event(&self, key: &Keys, channel: Uuid, ops: &[GoalOp]) -> Event {
        let content = json!({ "ops": ops }).to_string();
        self.event(key, channel, KIND_GOAL_OPS, content)
    }

    async fn publish(&self, key: &Keys, event: &Event) -> (StatusCode, Value) {
        self.request(key, "/events", json!(event)).await
    }

    /// Write ops as the owner and return the outcome the writer sees.
    async fn write(&self, channel: Uuid, ops: &[GoalOp]) -> Value {
        let event = self.ops_event(&self.owner, channel, ops);
        let (status, body) = self.publish(&self.owner, &event).await;
        assert!(status.is_success(), "{status}: {body}");
        assert_eq!(body["accepted"], true, "{body}");
        serde_json::from_str(body["message"].as_str().unwrap()).unwrap()
    }

    async fn read(&self, key: &Keys, filter: Value) -> (StatusCode, Value) {
        self.request(key, "/query", json!([filter])).await
    }

    async fn stored_ops(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE community_id=$1 AND kind=$2")
            .bind(self.community.as_uuid())
            .bind(KIND_GOAL_OPS as i32)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn set_goals_enabled(&self, channel: Uuid, enabled: bool) {
        sqlx::query("UPDATE channels SET goals_enabled=$3 WHERE community_id=$1 AND id=$2")
            .bind(self.community.as_uuid())
            .bind(channel)
            .bind(enabled)
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

fn root() -> GoalOp {
    GoalOp::SetRoot {
        id: "root".into(),
        title: "room goal".into(),
        note: None,
    }
}

fn add(id: &str, parent: &str) -> GoalOp {
    GoalOp::Add {
        id: id.into(),
        parent: parent.into(),
        title: format!("goal {id}"),
        note: None,
        assignees: Vec::new(),
    }
}

fn done(id: &str) -> GoalOp {
    GoalOp::Update {
        id: id.into(),
        title: None,
        note: None,
        status: Some(GoalStatus::Done),
        add_assignees: Vec::new(),
        remove_assignees: Vec::new(),
    }
}

fn row_ids(body: &Value) -> Vec<&str> {
    let mut ids: Vec<&str> = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn an_edit_reads_back_and_reports_its_outcome() {
    let f = Fixture::new(true).await;
    f.write(f.open, &[root(), add("a", "root")]).await;
    let (status, body) = f
        .read(
            &f.owner,
            json!({"goals": 1, "channel": f.open, "view": "active"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(row_ids(&body), ["a", "root"]);
    assert_eq!(body["counts"]["active"], 2);
    let outcome = f.write(f.open, &[done("missing")]).await;
    assert_eq!(outcome["missing"], json!(["missing"]));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_broken_rule_is_refused_and_nothing_is_stored() {
    let f = Fixture::new(true).await;
    f.write(f.open, &[root()]).await;
    let before = f.stored_ops().await;
    let event = f.ops_event(&f.owner, f.open, &[add("a", "nope")]);
    let (status, body) = f.publish(&f.owner, &event).await;
    assert!(!status.is_success() || body["accepted"] == false, "{body}");
    assert!(body.to_string().contains("goal not found"), "{body}");
    assert_eq!(f.stored_ops().await, before);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_replayed_event_is_accepted_without_applying_again() {
    let f = Fixture::new(true).await;
    f.write(f.open, &[root()]).await;
    let event = f.ops_event(&f.owner, f.open, &[add("a", "root")]);
    for _ in 0..2 {
        let (status, body) = f.publish(&f.owner, &event).await;
        assert!(status.is_success(), "{status}: {body}");
        assert_eq!(body["accepted"], true, "{body}");
    }
    assert_eq!(f.stored_ops().await, 2);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_stranger_cannot_write_or_read_a_private_room() {
    let f = Fixture::new(true).await;
    f.write(f.private, &[root()]).await;
    let event = f.ops_event(&f.stranger, f.private, &[add("a", "root")]);
    let (status, body) = f.publish(&f.stranger, &event).await;
    assert!(!status.is_success() || body["accepted"] == false, "{body}");
    assert!(f.state.db.list_goals(f.community, f.private).await.unwrap()[0].id == "root");
    let (status, body) = f
        .read(
            &f.stranger,
            json!({"goals": 1, "channel": f.private, "view": "active"}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_stranger_can_read_an_open_room() {
    let f = Fixture::new(true).await;
    f.write(f.open, &[root()]).await;
    let (status, body) = f
        .read(
            &f.stranger,
            json!({"goals": 1, "channel": f.open, "view": "active"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(row_ids(&body), ["root"]);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn goals_off_refuses_edits_reads_empty_and_hides_the_log() {
    let f = Fixture::new(true).await;
    f.write(f.open, &[root()]).await;
    f.set_goals_enabled(f.open, false).await;
    let event = f.ops_event(&f.owner, f.open, &[add("a", "root")]);
    let (status, body) = f.publish(&f.owner, &event).await;
    assert!(!status.is_success() || body["accepted"] == false, "{body}");
    assert!(body.to_string().contains("turned off"), "{body}");
    let (status, body) = f
        .read(
            &f.owner,
            json!({"goals": 1, "channel": f.open, "view": "active"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([]));
    let (_, log) = f
        .read(
            &f.owner,
            json!({"kinds": [KIND_GOAL_OPS], "#h": [f.open.to_string()]}),
        )
        .await;
    assert_eq!(log.as_array().map(Vec::len), Some(0), "{log}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn goals_off_also_empties_a_thread_read() {
    let f = Fixture::new(true).await;
    let thread = [5u8; 32];
    f.write(
        f.open,
        &[
            root(),
            add("a", "root"),
            GoalOp::Link {
                id: "a".into(),
                thread: hex::encode(thread),
            },
        ],
    )
    .await;
    let filter = json!({"goals": 1, "channel": f.open, "threads": [hex::encode(thread)]});
    let (_, body) = f.read(&f.owner, filter.clone()).await;
    assert_eq!(body["threads"][0]["goal"], "a", "{body}");
    f.set_goals_enabled(f.open, false).await;
    let (status, body) = f.read(&f.owner, filter).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rows"], json!([]));
    assert_eq!(body["threads"], json!([]));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn an_archived_room_takes_no_edits() {
    let f = Fixture::new(true).await;
    f.write(f.open, &[root()]).await;
    sqlx::query("UPDATE channels SET archived_at=now() WHERE community_id=$1 AND id=$2")
        .bind(f.community.as_uuid())
        .bind(f.open)
        .execute(&f.pool)
        .await
        .unwrap();
    let event = f.ops_event(&f.owner, f.open, &[add("a", "root")]);
    let (status, body) = f.publish(&f.owner, &event).await;
    assert!(!status.is_success() || body["accepted"] == false, "{body}");
    assert!(body.to_string().contains("archived"), "{body}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_removed_member_loses_a_private_room() {
    let f = Fixture::new(true).await;
    f.write(f.private, &[root()]).await;
    let member = Keys::generate();
    sqlx::query("INSERT INTO channel_members(community_id,channel_id,pubkey,role,removed_at) VALUES($1,$2,$3,'member',now())")
        .bind(f.community.as_uuid()).bind(f.private).bind(member.public_key().as_bytes().as_slice()).execute(&f.pool).await.unwrap();
    let (status, body) = f
        .read(
            &member,
            json!({"goals": 1, "channel": f.private, "view": "active"}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_node_read_gives_the_goal_and_its_context() {
    let f = Fixture::new(true).await;
    f.write(
        f.open,
        &[root(), add("a", "root"), add("a1", "a"), add("b", "root")],
    )
    .await;
    let (status, body) = f
        .read(
            &f.owner,
            json!({"goals": 1, "channel": f.open, "nodes": ["a", "gone"]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut ids: Vec<&str> = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["a", "a1", "b", "root"], "{body}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_replay_is_marked_duplicate() {
    let f = Fixture::new(true).await;
    f.write(f.open, &[root()]).await;
    let event = f.ops_event(&f.owner, f.open, &[add("a", "root")]);
    f.publish(&f.owner, &event).await;
    let (_, body) = f.publish(&f.owner, &event).await;
    assert_eq!(body["message"], "duplicate:", "{body}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn with_the_switch_on_old_goal_documents_are_refused() {
    let f = Fixture::new(true).await;
    let tree = f.event(&f.owner, f.open, KIND_GOAL_TREE, "{}".into());
    let (status, body) = f.publish(&f.owner, &tree).await;
    assert!(!status.is_success() || body["accepted"] == false, "{body}");
    assert!(body.to_string().contains("goals have moved"), "{body}");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn with_the_switch_off_edits_and_reads_are_refused() {
    let f = Fixture::new(false).await;
    let event = f.ops_event(&f.owner, f.open, &[root()]);
    let (status, body) = f.publish(&f.owner, &event).await;
    assert!(!status.is_success() || body["accepted"] == false, "{body}");
    assert!(body.to_string().contains("not on yet"), "{body}");
    assert_eq!(f.stored_ops().await, 0);
    let (status, body) = f
        .read(
            &f.owner,
            json!({"goals": 1, "channel": f.open, "view": "active"}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "goal_store_disabled");
}
