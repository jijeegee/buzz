//! PostgreSQL tests for goal rows: all-or-nothing events, the deletion rule
//! against real links and thread metadata, and retries.
use super::goals::{parse_goal_ops, GoalOutcome, GoalRow, GoalWrite};
use crate::{Db, DbError};
use chrono::{Duration, Utc};
use kuzz_core::goal_tree::{GoalOp, GoalStatus};
use kuzz_core::kind::KIND_GOAL_OPS;
use kuzz_core::CommunityId;
use nostr::{EventBuilder, Keys, Kind, Tag};
use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    db: Db,
    community: CommunityId,
    channel: Uuid,
    keys: Keys,
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

fn set_status(id: &str, status: GoalStatus) -> GoalOp {
    GoalOp::Update {
        id: id.into(),
        title: None,
        note: None,
        status: Some(status),
        add_assignees: Vec::new(),
        remove_assignees: Vec::new(),
    }
}

fn done(id: &str) -> GoalOp {
    set_status(id, GoalStatus::Done)
}

fn link(id: &str, thread: &[u8; 32]) -> GoalOp {
    GoalOp::Link {
        id: id.into(),
        thread: hex::encode(thread),
    }
}

impl Fixture {
    async fn new() -> Self {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .unwrap();
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool).await.unwrap();
        }
        let db = Db::from_pool(pool);
        db.ensure_future_partitions(1, true).await.unwrap();
        let community = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id,host) VALUES($1,$2)")
            .bind(community.as_uuid())
            .bind(format!("goals-{}.test", community.as_uuid()))
            .execute(&db.pool)
            .await
            .unwrap();
        let keys = Keys::generate();
        let channel = Uuid::new_v4();
        sqlx::query("INSERT INTO channels(community_id,id,name,visibility,channel_type,created_by,history_policy) VALUES($1,$2,$3,'open'::channel_visibility,'stream',$4,'all')")
            .bind(community.as_uuid()).bind(channel).bind(channel.to_string()).bind(keys.public_key().to_bytes().as_slice()).execute(&db.pool).await.unwrap();
        Self {
            db,
            community,
            channel,
            keys,
        }
    }

    fn event(&self, ops: &[GoalOp]) -> nostr::Event {
        let content = serde_json::json!({ "ops": ops }).to_string();
        EventBuilder::new(Kind::Custom(KIND_GOAL_OPS as u16), content)
            .tags([Tag::parse(["h", &self.channel.to_string()]).unwrap()])
            .sign_with_keys(&self.keys)
            .unwrap()
    }

    async fn write(&self, ops: &[GoalOp]) -> crate::Result<GoalWrite> {
        let event = self.event(ops);
        let ops = parse_goal_ops(&event.content)?;
        self.db
            .apply_goal_ops(self.community, self.channel, &event, &ops)
            .await
    }

    async fn ok(&self, ops: &[GoalOp]) -> GoalOutcome {
        match self.write(ops).await.unwrap() {
            GoalWrite::Applied(outcome) | GoalWrite::NoEffect(outcome) => outcome,
            GoalWrite::Duplicate => panic!("unexpected duplicate"),
        }
    }

    async fn goals(&self) -> Vec<GoalRow> {
        self.db
            .list_goals(self.community, self.channel)
            .await
            .unwrap()
    }

    async fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.goals().await.into_iter().map(|g| g.id).collect();
        ids.sort();
        ids
    }

    async fn goal(&self, id: &str) -> GoalRow {
        self.goals()
            .await
            .into_iter()
            .find(|g| g.id == id)
            .unwrap_or_else(|| panic!("goal {id} missing"))
    }

    async fn root(&self) {
        self.ok(&[GoalOp::SetRoot {
            id: "root".into(),
            title: "room goal".into(),
            note: None,
        }])
        .await;
    }

    /// Make `id` a thread with replies, the last reply at `at`.
    async fn thread(&self, at: chrono::DateTime<Utc>) -> [u8; 32] {
        let id: [u8; 32] = *Uuid::new_v4()
            .as_bytes()
            .iter()
            .chain(Uuid::new_v4().as_bytes())
            .copied()
            .collect::<Vec<u8>>()
            .first_chunk()
            .unwrap();
        sqlx::query("INSERT INTO thread_metadata (community_id,event_created_at,event_id,channel_id,depth,reply_count,last_reply_at) VALUES($1,$2,$3,$4,0,1,$2)")
            .bind(self.community.as_uuid()).bind(at).bind(id.as_slice()).bind(self.channel).execute(&self.db.pool).await.unwrap();
        id
    }

    async fn age(&self, days: i64) {
        sqlx::query("UPDATE goals SET updated_at = $3 WHERE community_id=$1 AND channel_id=$2")
            .bind(self.community.as_uuid())
            .bind(self.channel)
            .bind(Utc::now() - Duration::days(days))
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    async fn stored_events(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE community_id=$1 AND kind=$2")
            .bind(self.community.as_uuid())
            .bind(KIND_GOAL_OPS as i32)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_rejected_op_rolls_back_the_whole_event() {
    let f = Fixture::new().await;
    f.root().await;
    let before = f.stored_events().await;
    let err = f.write(&[add("a", "root"), add("b", "nope")]).await;
    assert!(matches!(err, Err(DbError::GoalRejected(_))));
    assert_eq!(f.ids().await, ["root"]);
    assert_eq!(f.stored_events().await, before);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn replaying_an_event_changes_nothing() {
    let f = Fixture::new().await;
    f.root().await;
    let first = f.event(&[add("a", "root")]);
    let ops = parse_goal_ops(&first.content).unwrap();
    let write = |e| f.db.apply_goal_ops(f.community, f.channel, e, &ops);
    assert!(matches!(
        write(&first).await.unwrap(),
        GoalWrite::Applied(_)
    ));
    f.ok(&[GoalOp::Update {
        id: "a".into(),
        title: Some("newer".into()),
        note: None,
        status: None,
        add_assignees: Vec::new(),
        remove_assignees: Vec::new(),
    }])
    .await;
    assert_eq!(write(&first).await.unwrap(), GoalWrite::Duplicate);
    assert_eq!(f.goal("a").await.title, "newer");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn add_retry_with_a_new_event_succeeds_and_a_real_clash_fails() {
    let f = Fixture::new().await;
    f.root().await;
    f.ok(&[add("a", "root")]).await;
    // A different note gives the retry its own event id.
    let retry = f
        .write(&[GoalOp::Add {
            id: "a".into(),
            parent: "root".into(),
            title: "goal a".into(),
            note: Some("retried".into()),
            assignees: Vec::new(),
        }])
        .await
        .unwrap();
    assert!(matches!(retry, GoalWrite::NoEffect(_)), "{retry:?}");
    let clash = f
        .write(&[GoalOp::Add {
            id: "a".into(),
            parent: "root".into(),
            title: "other".into(),
            note: None,
            assignees: Vec::new(),
        }])
        .await;
    assert!(matches!(clash, Err(DbError::GoalRejected(_))));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn finishing_a_task_deletes_it_and_reports_the_parent() {
    let f = Fixture::new().await;
    f.root().await;
    let thread = f.thread(Utc::now()).await;
    f.ok(&[add("t", "root"), link("t", &thread), add("task", "t")])
        .await;
    let outcome = f.ok(&[done("task")]).await;
    assert_eq!(outcome.deleted, ["task"]);
    assert_eq!(outcome.parent.as_deref(), Some("t"));
    assert_eq!(f.ids().await, ["root", "t"]);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_message_link_without_replies_is_not_a_thread_goal() {
    let f = Fixture::new().await;
    f.root().await;
    f.ok(&[
        add("plan", "root"),
        link("plan", &[7; 32]),
        add("sub", "plan"),
    ])
    .await;
    assert!(!f.goal("plan").await.is_thread);
    let outcome = f.ok(&[done("sub")]).await;
    assert!(
        outcome.deleted.is_empty(),
        "sub is a plan goal: {outcome:?}"
    );
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn finishing_a_thread_goal_keeps_it_and_clears_threadless_branches() {
    let f = Fixture::new().await;
    f.root().await;
    let outer = f.thread(Utc::now()).await;
    let inner = f.thread(Utc::now()).await;
    f.ok(&[
        add("t", "root"),
        link("t", &outer),
        add("open_task", "t"),
        add("mid", "t"),
        add("inner", "mid"),
        link("inner", &inner),
    ])
    .await;
    let outcome = f.ok(&[done("t")]).await;
    assert_eq!(outcome.deleted, ["open_task"]);
    assert_eq!(f.ids().await, ["inner", "mid", "root", "t"]);
    assert_eq!(f.goal("t").await.status, GoalStatus::Done);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn done_to_dropped_is_not_a_finish() {
    let f = Fixture::new().await;
    f.root().await;
    let thread = f.thread(Utc::now()).await;
    f.ok(&[
        add("t", "root"),
        link("t", &thread),
        add("a", "t"),
        add("b", "a"),
    ])
    .await;
    f.ok(&[done("a")]).await;
    let outcome = f.ok(&[set_status("a", GoalStatus::Dropped)]).await;
    assert!(outcome.deleted.is_empty());
    assert_eq!(f.goal("a").await.status, GoalStatus::Dropped);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn missing_targets_are_reported_and_store_no_event() {
    let f = Fixture::new().await;
    f.root().await;
    let before = f.stored_events().await;
    let write = f
        .write(&[
            done("gone"),
            GoalOp::Remove {
                id: "gone2".into(),
                recursive: false,
            },
        ])
        .await
        .unwrap();
    let GoalWrite::NoEffect(outcome) = write else {
        panic!("expected no effect, got {write:?}");
    };
    assert_eq!(outcome.missing, ["gone", "gone2"]);
    assert_eq!(f.stored_events().await, before);
    let plain_edit = f
        .write(&[GoalOp::Update {
            id: "gone".into(),
            title: Some("x".into()),
            note: None,
            status: None,
            add_assignees: Vec::new(),
            remove_assignees: Vec::new(),
        }])
        .await;
    assert!(matches!(plain_edit, Err(DbError::GoalRejected(_))));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn layer_1_cannot_be_linked_and_a_second_root_rewrites_the_first() {
    let f = Fixture::new().await;
    f.root().await;
    let linked = f.write(&[link("root", &[1; 32])]).await;
    assert!(matches!(linked, Err(DbError::GoalRejected(_))));
    f.ok(&[GoalOp::SetRoot {
        id: "other".into(),
        title: "new sentence".into(),
        note: Some("key info".into()),
    }])
    .await;
    assert_eq!(f.ids().await, ["root"]);
    let root = f.goal("root").await;
    assert_eq!(root.title, "new sentence");
    assert_eq!(root.note, "key info");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn remove_needs_recursive_and_takes_links_with_it() {
    let f = Fixture::new().await;
    f.root().await;
    f.ok(&[add("a", "root"), add("b", "a"), link("b", &[2; 32])])
        .await;
    let refused = f
        .write(&[GoalOp::Remove {
            id: "a".into(),
            recursive: false,
        }])
        .await;
    assert!(matches!(refused, Err(DbError::GoalRejected(_))));
    let outcome = f
        .ok(&[GoalOp::Remove {
            id: "a".into(),
            recursive: true,
        }])
        .await;
    assert_eq!(outcome.deleted.len(), 2);
    let links: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM goal_links WHERE community_id=$1")
        .bind(f.community.as_uuid())
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(links, 0);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn one_link_per_thread_and_relinking_moves_it() {
    let f = Fixture::new().await;
    f.root().await;
    let thread = f.thread(Utc::now()).await;
    f.ok(&[add("a", "root"), add("b", "root"), link("a", &thread)])
        .await;
    f.ok(&[link("b", &thread)]).await;
    assert!(f.goal("a").await.links.is_empty());
    assert_eq!(f.goal("b").await.links, [hex::encode(thread)]);
    assert_eq!(f.goal("b").await.status, GoalStatus::InProgress);
    f.ok(&[GoalOp::Unlink {
        thread: hex::encode(thread),
    }])
    .await;
    assert!(f.goal("b").await.links.is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn move_rejects_a_cycle_and_a_goal_can_change_parent() {
    let f = Fixture::new().await;
    f.root().await;
    f.ok(&[add("a", "root"), add("b", "a"), add("c", "root")])
        .await;
    let cycle = f
        .write(&[GoalOp::Move {
            id: "a".into(),
            parent: "b".into(),
            order: None,
        }])
        .await;
    assert!(matches!(cycle, Err(DbError::GoalRejected(_))));
    f.ok(&[GoalOp::Move {
        id: "b".into(),
        parent: "c".into(),
        order: Some(9),
    }])
    .await;
    let b = f.goal("b").await;
    assert_eq!((b.parent.as_deref(), b.order), (Some("c"), 9));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn assignees_merge_without_repeats() {
    let f = Fixture::new().await;
    f.root().await;
    let one = "a".repeat(64);
    let two = "b".repeat(64);
    f.ok(&[add("a", "root")]).await;
    let edit = |add: Vec<String>, remove: Vec<String>| GoalOp::Update {
        id: "a".into(),
        title: None,
        note: None,
        status: None,
        add_assignees: add,
        remove_assignees: remove,
    };
    f.ok(&[edit(vec![one.clone(), two.clone()], vec![])]).await;
    f.ok(&[edit(vec![one.to_uppercase()], vec![two.clone()])])
        .await;
    assert_eq!(f.goal("a").await.assignees, [one]);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn inactivity_follows_replies_and_comes_back_on_edit() {
    let f = Fixture::new().await;
    f.root().await;
    let quiet = f.thread(Utc::now() - Duration::days(120)).await;
    let live = f.thread(Utc::now() - Duration::days(120)).await;
    f.ok(&[
        add("quiet", "root"),
        link("quiet", &quiet),
        add("live", "root"),
        link("live", &live),
    ])
    .await;
    f.age(120).await;
    // A nested reply moves only its direct parent's row, under the thread root.
    sqlx::query("INSERT INTO thread_metadata (community_id,event_created_at,event_id,channel_id,parent_event_id,root_event_id,depth,reply_count,last_reply_at) VALUES($1,$2,$3,$4,$3,$5,1,1,$2)")
        .bind(f.community.as_uuid()).bind(Utc::now()).bind([9u8; 32].as_slice()).bind(f.channel).bind(live.as_slice()).execute(&f.db.pool).await.unwrap();
    assert!(f.goal("quiet").await.inactive);
    assert!(!f.goal("live").await.inactive);
    assert!(!f.goal("root").await.inactive);
    f.ok(&[GoalOp::Update {
        id: "quiet".into(),
        title: None,
        note: Some("back".into()),
        status: None,
        add_assignees: Vec::new(),
        remove_assignees: Vec::new(),
    }])
    .await;
    assert!(!f.goal("quiet").await.inactive);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_deleted_task_keeps_its_parent_active() {
    let f = Fixture::new().await;
    f.root().await;
    let thread = f.thread(Utc::now() - Duration::days(120)).await;
    f.ok(&[add("t", "root"), link("t", &thread), add("task", "t")])
        .await;
    f.age(120).await;
    assert!(f.goal("t").await.inactive);
    f.ok(&[done("task")]).await;
    assert!(!f.goal("t").await.inactive);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn drop_inactive_gives_up_old_goals_and_leaves_live_ones() {
    let f = Fixture::new().await;
    f.root().await;
    let old = f.thread(Utc::now() - Duration::days(120)).await;
    f.ok(&[
        add("old", "root"),
        link("old", &old),
        add("old_task", "old"),
        add("plan", "root"),
    ])
    .await;
    f.age(120).await;
    f.ok(&[add("fresh", "root")]).await;
    let outcome = f.ok(&[GoalOp::DropInactive]).await;
    assert_eq!(outcome.deleted, ["old_task"]);
    assert_eq!(f.goal("old").await.status, GoalStatus::Dropped);
    assert_eq!(f.goal("plan").await.status, GoalStatus::Dropped);
    assert_eq!(f.goal("fresh").await.status, GoalStatus::Open);
    assert_eq!(f.goal("root").await.status, GoalStatus::Open);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_written_goal_records_its_editor_and_community_isolation_holds() {
    let f = Fixture::new().await;
    let other = Fixture::new().await;
    f.root().await;
    assert_eq!(
        f.goal("root").await.updated_by,
        Some(f.keys.public_key().to_hex())
    );
    assert!(other.goals().await.is_empty());
}

#[test]
fn op_content_is_bounded() {
    let ops = |n| serde_json::json!({ "ops": vec![done("a"); n] }).to_string();
    assert!(parse_goal_ops(&ops(16)).is_ok());
    assert!(parse_goal_ops(&ops(17)).is_err());
    assert!(parse_goal_ops(&ops(0)).is_err());
    assert!(parse_goal_ops("{").is_err());
}
