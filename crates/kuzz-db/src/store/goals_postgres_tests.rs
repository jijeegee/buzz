//! PostgreSQL tests for goal rows: all-or-nothing events, the deletion rule
//! against real links and thread metadata, and retries.
use super::goals::{
    parse_goal_ops, GoalCounts, GoalOutcome, GoalPage, GoalRow, GoalSelection, GoalWrite,
    MAX_GOAL_PAGE,
};
use crate::{Db, DbError};
use chrono::{Duration, Utc};
use kuzz_core::goal_store::GoalView;
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
            GoalWrite::Applied(_, outcome) | GoalWrite::NoEffect(outcome) => outcome,
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
        GoalWrite::Applied(..)
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
    assert!(matches!(write(&first).await.unwrap(), GoalWrite::Duplicate));
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
    f.ok(&[add("t", "root"), link("t", &thread), add("plan", "root")])
        .await;
    f.ok(&[done("plan")]).await;
    // A move never deletes, so this leaves a finished, childless task.
    f.ok(&[GoalOp::Move {
        id: "plan".into(),
        parent: "t".into(),
        order: None,
    }])
    .await;
    assert_eq!(f.goal("plan").await.status, GoalStatus::Done);
    let outcome = f.ok(&[set_status("plan", GoalStatus::Dropped)]).await;
    assert!(outcome.deleted.is_empty());
    assert_eq!(f.goal("plan").await.status, GoalStatus::Dropped);
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
    // The same channel id in another community must see nothing.
    sqlx::query("INSERT INTO channels(community_id,id,name,visibility,channel_type,created_by,history_policy) VALUES($1,$2,'same-id','open'::channel_visibility,'stream',$3,'all')")
        .bind(other.community.as_uuid()).bind(f.channel).bind(other.keys.public_key().to_bytes().as_slice()).execute(&other.db.pool).await.unwrap();
    let seen = other
        .db
        .list_goals(other.community, f.channel)
        .await
        .unwrap();
    assert!(seen.is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_move_with_no_order_sends_a_goal_last_among_its_siblings() {
    let f = Fixture::new().await;
    f.root().await;
    f.ok(&[add("a", "root"), add("b", "root")]).await;
    let first = f.goal("a").await.order;
    f.ok(&[GoalOp::Move {
        id: "a".into(),
        parent: "root".into(),
        order: None,
    }])
    .await;
    assert!(f.goal("a").await.order > f.goal("b").await.order);
    assert!(f.goal("a").await.order > first);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn relinking_a_reopened_goal_starts_it_again() {
    let f = Fixture::new().await;
    f.root().await;
    f.ok(&[add("a", "root"), link("a", &[4; 32])]).await;
    f.ok(&[set_status("a", GoalStatus::Open)]).await;
    f.ok(&[link("a", &[4; 32])]).await;
    assert_eq!(f.goal("a").await.status, GoalStatus::InProgress);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn the_reported_parent_survives_a_later_op_in_the_event() {
    let f = Fixture::new().await;
    f.root().await;
    f.ok(&[add("top", "root"), add("mid", "top"), add("leaf", "mid")])
        .await;
    let outcome = f
        .ok(&[
            done("leaf"),
            GoalOp::Remove {
                id: "mid".into(),
                recursive: true,
            },
        ])
        .await;
    assert_eq!(outcome.parent.as_deref(), Some("top"));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn one_event_can_add_link_and_finish() {
    let f = Fixture::new().await;
    f.root().await;
    let thread = f.thread(Utc::now()).await;
    let outcome = f
        .ok(&[
            add("t", "root"),
            link("t", &thread),
            add("task", "t"),
            done("t"),
        ])
        .await;
    assert_eq!(outcome.deleted, ["task"]);
    assert_eq!(outcome.parent.as_deref(), Some("root"));
    assert_eq!(f.ids().await, ["root", "t"]);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_chain_of_finished_tasks_goes_and_reports_the_goal_that_stays() {
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
    assert!(f.ok(&[done("a")]).await.deleted.is_empty());
    let outcome = f.ok(&[done("b")]).await;
    assert_eq!(outcome.deleted, ["b", "a"]);
    assert_eq!(outcome.parent.as_deref(), Some("t"));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn retries_with_a_new_event_id_store_nothing_and_keep_the_clock() {
    let f = Fixture::new().await;
    f.root().await;
    let thread = f.thread(Utc::now()).await;
    f.ok(&[
        add("plan", "root"),
        link("plan", &thread),
        add("other", "root"),
    ])
    .await;
    f.ok(&[done("plan")]).await;
    f.ok(&[GoalOp::Update {
        id: "other".into(),
        title: None,
        note: None,
        status: Some(GoalStatus::Dropped),
        add_assignees: Vec::new(),
        remove_assignees: Vec::new(),
    }])
    .await;
    f.age(10).await;
    let aged = f.goal("plan").await.updated_at;
    let before = f.stored_events().await;
    let retries = [
        vec![set_status("plan", GoalStatus::Done), link("plan", &thread)],
        vec![GoalOp::SetRoot {
            id: "ignored".into(),
            title: "room goal".into(),
            note: None,
        }],
        vec![GoalOp::Update {
            id: "plan".into(),
            title: Some("goal plan".into()),
            note: Some(String::new()),
            status: None,
            add_assignees: Vec::new(),
            remove_assignees: Vec::new(),
        }],
        vec![GoalOp::Move {
            id: "other".into(),
            parent: "root".into(),
            order: None,
        }],
        vec![GoalOp::Unlink {
            thread: hex::encode([3; 32]),
        }],
        vec![GoalOp::DropInactive],
    ];
    for ops in retries {
        let write = f.write(&ops).await.unwrap();
        assert!(
            matches!(write, GoalWrite::NoEffect(_)),
            "{ops:?}: {write:?}"
        );
    }
    assert_eq!(f.stored_events().await, before);
    assert_eq!(f.goal("plan").await.updated_at, aged);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn goal_rows_take_the_stored_events_receive_time() {
    let f = Fixture::new().await;
    f.root().await;
    let GoalWrite::Applied(stored, _) = f.write(&[add("a", "root")]).await.unwrap() else {
        panic!("expected a stored event");
    };
    let row_time: chrono::DateTime<Utc> = sqlx::query_scalar(
        "SELECT updated_at FROM goals WHERE community_id=$1 AND channel_id=$2 AND id='a'",
    )
    .bind(f.community.as_uuid())
    .bind(f.channel)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let event_time: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT received_at FROM events WHERE community_id=$1 AND id=$2")
            .bind(f.community.as_uuid())
            .bind(stored.event.id.as_bytes().as_slice())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(row_time, event_time);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn drop_inactive_settles_nested_thread_goals() {
    let f = Fixture::new().await;
    f.root().await;
    let outer = f.thread(Utc::now() - Duration::days(120)).await;
    let inner = f.thread(Utc::now() - Duration::days(120)).await;
    f.ok(&[
        add("outer", "root"),
        link("outer", &outer),
        add("mid", "outer"),
        add("inner", "mid"),
        link("inner", &inner),
        add("inner_task", "inner"),
        add("side", "outer"),
    ])
    .await;
    f.age(120).await;
    let outcome = f.ok(&[GoalOp::DropInactive]).await;
    let mut deleted = outcome.deleted;
    deleted.sort();
    assert_eq!(deleted, ["inner_task", "side"]);
    assert_eq!(f.ids().await, ["inner", "mid", "outer", "root"]);
    for id in ["inner", "mid", "outer"] {
        assert_eq!(f.goal(id).await.status, GoalStatus::Dropped, "{id}");
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn concurrent_writers_serialize_on_the_room() {
    let f = Fixture::new().await;
    f.root().await;
    let (ops_a, ops_b, ops_c) = ([add("a", "root")], [add("b", "root")], [add("c", "root")]);
    let (a, b, c) = tokio::join!(f.write(&ops_a), f.write(&ops_b), f.write(&ops_c),);
    for write in [a, b, c] {
        assert!(matches!(write.unwrap(), GoalWrite::Applied(..)));
    }
    let mut orders: Vec<i64> = f.goals().await.iter().map(|g| g.order).collect();
    orders.sort();
    orders.dedup();
    assert_eq!(orders.len(), 4, "each goal gets its own sort key");
}

impl Fixture {
    async fn view(&self, view: GoalView) -> GoalPage {
        let selection = GoalSelection::View { view, after: None };
        self.db
            .read_goals(self.community, self.channel, &selection)
            .await
            .unwrap()
    }
}

fn row_ids(page: &GoalPage, matched: bool) -> Vec<&str> {
    let mut ids: Vec<&str> = page
        .rows
        .iter()
        .filter(|r| r.matched == matched)
        .map(|r| r.goal.id.as_str())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_list_returns_its_rows_with_their_parents_and_counts() {
    let f = Fixture::new().await;
    f.root().await;
    f.ok(&[
        add("plan", "root"),
        add("sub", "plan"),
        add("old", "root"),
        add("old_sub", "old"),
    ])
    .await;
    f.age(120).await;
    f.ok(&[add("live", "plan")]).await;
    f.ok(&[done("sub")]).await;

    let done_page = f.view(GoalView::Done).await;
    assert_eq!(row_ids(&done_page, true), ["sub"]);
    assert_eq!(row_ids(&done_page, false), ["plan", "root"]);
    assert_eq!(
        done_page.counts,
        GoalCounts {
            active: 3,
            inactive: 2,
            done: 1
        }
    );

    let inactive = f.view(GoalView::Inactive).await;
    assert_eq!(row_ids(&inactive, true), ["old", "old_sub"]);
    assert_eq!(row_ids(&inactive, false), ["root"]);
    assert!(inactive
        .rows
        .iter()
        .any(|r| r.goal.id == "old" && r.goal.inactive));

    let active = f.view(GoalView::Active).await;
    let plan = active.rows.iter().find(|r| r.goal.id == "plan").unwrap();
    assert_eq!(plan.progress, (1, 2));
    assert_eq!(plan.open_children, 1);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn lists_page_through_every_row_once() {
    let f = Fixture::new().await;
    f.root().await;
    sqlx::query("INSERT INTO goals(community_id,channel_id,id,parent_id,title,order_key,updated_at,created_at) SELECT $1,$2,'g'||i,'root','t',i,now(),now() FROM generate_series(1,450) i")
        .bind(f.community.as_uuid()).bind(f.channel).execute(&f.db.pool).await.unwrap();
    let mut after = None;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let selection = GoalSelection::View {
            view: GoalView::Active,
            after: after.clone(),
        };
        let page =
            f.db.read_goals(f.community, f.channel, &selection)
                .await
                .unwrap();
        pages += 1;
        let matched: Vec<String> = page
            .rows
            .iter()
            .filter(|r| r.matched)
            .map(|r| r.goal.id.clone())
            .collect();
        assert!(matched.len() <= MAX_GOAL_PAGE);
        seen.extend(matched);
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(pages, 3);
    let total = seen.len();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), total);
    assert_eq!(total, 451);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_thread_read_finds_its_goal_even_when_quiet() {
    let f = Fixture::new().await;
    f.root().await;
    let thread = f.thread(Utc::now() - Duration::days(120)).await;
    f.ok(&[
        add("plan", "root"),
        add("t", "plan"),
        link("t", &thread),
        add("task", "t"),
        add("other", "root"),
    ])
    .await;
    f.age(120).await;
    f.ok(&[add("fresh", "root")]).await;
    let selection = GoalSelection::Threads(vec![thread, [8; 32]]);
    let page =
        f.db.read_goals(f.community, f.channel, &selection)
            .await
            .unwrap();
    assert_eq!(page.threads[0], (thread, Some("t".to_owned())));
    assert_eq!(page.threads[1], ([8; 32], None));
    assert_eq!(row_ids(&page, true), ["fresh", "t", "task"]);
    assert_eq!(row_ids(&page, false), ["plan", "root"]);
    let goal = page.rows.iter().find(|r| r.goal.id == "t").unwrap();
    assert!(goal.goal.is_thread && goal.goal.inactive);
}

#[test]
fn op_content_is_bounded() {
    let ops = |n| serde_json::json!({ "ops": vec![done("a"); n] }).to_string();
    assert!(parse_goal_ops(&ops(16)).is_ok());
    assert!(parse_goal_ops(&ops(17)).is_err());
    assert!(parse_goal_ops(&ops(0)).is_err());
    assert!(parse_goal_ops("{").is_err());
}
