//! Order-number reads against a real database: `after_sequence` pages and the
//! `channel_sequence` channel window, across monthly `events` partitions.
use kuzz_core::thread_window::Direction;
use kuzz_core::CommunityId;
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use crate::channel::{ChannelType, ChannelVisibility};
use crate::error::DbError;
use crate::thread::{get_channel_window, get_channel_window_by_sequence, SequenceCursor};
use crate::{channel, dm, event, Db, EventQuery};

/// Noon UTC on the first of a month: past, 2026-01..05, future partitions.
const PAST: u64 = 1_748_779_200; // 2025-06-01
const JAN: u64 = 1_767_268_800;
const FEB: u64 = 1_769_947_200;
const MAR: u64 = 1_772_366_400;
const APR: u64 = 1_775_044_800;
const MAY: u64 = 1_777_636_800;
const FUTURE: u64 = 1_790_856_000; // 2026-10-01

struct Fixture {
    pool: PgPool,
    db: Db,
    scope: CommunityId,
    channel: Uuid,
    owner: Keys,
}

impl Fixture {
    async fn new() -> Self {
        let pool = PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("isolated test database");
        if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate");
        }
        let scope = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id, host) VALUES ($1, $2)")
            .bind(scope.as_uuid())
            .bind(format!("seq-page-{}.test", scope.as_uuid()))
            .execute(&pool)
            .await
            .expect("fixture community");
        let owner = Keys::generate();
        let channel = channel::create_channel(
            &pool,
            scope,
            "seq-page",
            ChannelType::Stream,
            ChannelVisibility::Open,
            None,
            &owner.public_key().to_bytes(),
            None,
        )
        .await
        .expect("real channel creation")
        .id;
        Self {
            db: Db::from_pool(pool.clone()),
            pool,
            scope,
            channel,
            owner,
        }
    }

    async fn send_to(&self, channel: Uuid, keys: &Keys, content: &str, ts: u64) -> (Event, i64) {
        let message = EventBuilder::new(Kind::Custom(9), content)
            .tags([Tag::parse(["h", &channel.to_string()]).expect("h tag")])
            .custom_created_at(Timestamp::from(ts))
            .sign_with_keys(keys)
            .expect("fixture signature");
        let (stored, inserted) = event::insert_event(&self.pool, self.scope, &message, Some(channel))
            .await
            .expect("real event writer");
        assert!(inserted);
        (message, stored.channel_sequence.expect("channel sequence"))
    }

    async fn send(&self, content: &str, ts: u64) -> (Event, i64) {
        self.send_to(self.channel, &self.owner, content, ts).await
    }

    fn page(&self, channel: Uuid, after: i64, limit: i64) -> EventQuery {
        let mut q = EventQuery::for_community(self.scope);
        q.channel_id = Some(channel);
        q.after_sequence = Some(after);
        q.order_by_sequence = true;
        q.limit = Some(limit);
        q
    }
}

fn ids_of(rows: &[kuzz_core::StoredEvent]) -> Vec<String> {
    rows.iter().map(|row| row.event.id.to_hex()).collect()
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn after_sequence_pages_every_partition_in_acceptance_order() {
    let f = Fixture::new().await;
    // Accepted out of time order, so sequence order differs from created_at.
    let mut accepted = Vec::new();
    for (n, ts) in [MAR, JAN, PAST, MAY, FEB, FUTURE, APR, JAN + 1]
        .into_iter()
        .enumerate()
    {
        accepted.push(f.send(&format!("m{n}"), ts).await);
    }
    // Rows outside the channel never join the page.
    let other = Keys::generate();
    let elsewhere = channel::create_channel(
        &f.pool,
        f.scope,
        "elsewhere",
        ChannelType::Stream,
        ChannelVisibility::Open,
        None,
        &other.public_key().to_bytes(),
        None,
    )
    .await
    .expect("second channel")
    .id;
    f.send_to(elsewhere, &other, "other room", FEB).await;

    let mut seen = Vec::new();
    let mut cursor = 0;
    loop {
        let rows = event::query_events(&f.pool, &f.page(f.channel, cursor, 3))
            .await
            .expect("sequence page");
        if rows.is_empty() {
            break;
        }
        let sequences: Vec<i64> = rows.iter().filter_map(|r| r.channel_sequence).collect();
        assert_eq!(sequences.len(), rows.len(), "every row carries its number");
        assert!(sequences.windows(2).all(|w| w[0] < w[1]), "{sequences:?}");
        assert!(sequences[0] > cursor);
        cursor = *sequences.last().unwrap();
        seen.extend(ids_of(&rows));
    }
    let expected: Vec<String> = accepted.iter().map(|(e, _)| e.id.to_hex()).collect();
    assert_eq!(seen, expected, "complete, in acceptance order, no duplicates");
    assert_eq!(cursor, accepted.last().unwrap().1);
    assert!(event::query_events(&f.pool, &f.page(f.channel, cursor, 3))
        .await
        .expect("past the end")
        .is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn after_sequence_hides_deleted_and_blocked_dm_rows_before_limit() {
    let f = Fixture::new().await;
    let (a, b) = (Keys::generate(), Keys::generate());
    let pk = |k: &Keys| k.public_key().to_bytes().to_vec();
    let direct = dm::create_dm(&f.pool, f.scope, &[&pk(&a), &pk(&b)], &pk(&a))
        .await
        .expect("1:1 DM")
        .id;
    let (before_block, _) = f.send_to(direct, &b, "before block", JAN).await;
    f.db.block_user(f.scope, &pk(&a), &pk(&b))
        .await
        .expect("block");
    for n in 0..3 {
        f.send_to(direct, &b, &format!("while blocked {n}"), JAN + 1 + n)
            .await;
    }
    let (deleted, _) = f.send_to(direct, &a, "deleted", FEB).await;
    sqlx::query("UPDATE events SET deleted_at = now() WHERE community_id = $1 AND id = $2")
        .bind(f.scope.as_uuid())
        .bind(deleted.id.to_bytes().to_vec())
        .execute(&f.pool)
        .await
        .expect("tombstone");
    let (visible, _) = f.send_to(direct, &a, "visible", MAR).await;

    let mut as_reader = f.page(direct, 0, 2);
    as_reader.blocked_dm_reader = Some(pk(&a));
    let rows = event::query_events(&f.pool, &as_reader)
        .await
        .expect("reader page");
    assert_eq!(
        ids_of(&rows),
        [before_block.id.to_hex(), visible.id.to_hex()],
        "hidden rows never take a slot of the page"
    );

    // Falsifiable: without the reader the blocked rows are there.
    let rows = event::query_events(&f.pool, &f.page(direct, 0, 10))
        .await
        .expect("unfiltered page");
    assert_eq!(rows.len(), 5, "4 from b + visible; the deleted row stays out");
    assert!(!ids_of(&rows).contains(&deleted.id.to_hex()));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn sequence_reads_require_one_channel() {
    let f = Fixture::new().await;
    let mut q = EventQuery::for_community(f.scope);
    q.after_sequence = Some(0);
    assert!(matches!(
        event::query_events(&f.pool, &q).await,
        Err(DbError::InvalidData(_))
    ));
    let mut q = EventQuery::for_community(f.scope);
    q.order_by_sequence = true;
    q.channel_ids = Some(vec![f.channel]);
    assert!(matches!(
        event::query_events(&f.pool, &q).await,
        Err(DbError::InvalidData(_))
    ));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn sequence_window_pages_both_ways_and_time_window_is_unchanged() {
    let f = Fixture::new().await;
    let mut accepted = Vec::new();
    for (n, ts) in [APR, JAN, MAY, PAST, FEB].into_iter().enumerate() {
        accepted.push(f.send(&format!("w{n}"), ts).await);
    }
    let id = |n: usize| accepted[n].0.id.to_hex();
    let seq = |n: usize| accepted[n].1;
    let window = |cursor, limit| {
        get_channel_window_by_sequence(&f.pool, f.scope, f.channel, limit, cursor, None)
    };
    let row_ids = |w: &crate::thread::ChannelWindow| -> Vec<String> {
        w.rows
            .iter()
            .map(|r| r.stored_event.event.id.to_hex())
            .collect()
    };

    let head = window(SequenceCursor::Head, 2).await.unwrap();
    assert_eq!(row_ids(&head), [id(4), id(3)]);
    assert!(head.has_more);
    assert_eq!(head.next_sequence, Some(seq(3)));
    assert_eq!(head.next_cursor, None);
    assert_eq!(head.rows[0].stored_event.channel_sequence, Some(seq(4)));

    let older = window(SequenceCursor::Before(seq(3)), 2).await.unwrap();
    assert_eq!(row_ids(&older), [id(2), id(1)]);
    assert!(older.has_more);
    let last = window(SequenceCursor::Before(seq(1)), 2).await.unwrap();
    assert_eq!(row_ids(&last), [id(0)]);
    assert!(!last.has_more);
    assert_eq!(last.next_sequence, None);

    // Exact multiple: a full final page still reports exhaustion.
    let all = window(SequenceCursor::Head, 5).await.unwrap();
    assert_eq!(all.rows.len(), 5);
    assert!(!all.has_more);

    let newer = window(SequenceCursor::After(seq(0)), 2).await.unwrap();
    assert_eq!(row_ids(&newer), [id(1), id(2)]);
    assert_eq!(newer.next_sequence, Some(seq(2)));
    let newer = window(SequenceCursor::After(seq(2)), 2).await.unwrap();
    assert_eq!(row_ids(&newer), [id(3), id(4)]);
    assert!(!newer.has_more);
    let past_end = window(SequenceCursor::After(seq(4)), 2).await.unwrap();
    assert!(past_end.rows.is_empty() && !past_end.has_more);

    // The (created_at, id) window keeps its order and cursor shape.
    let by_time = get_channel_window(&f.pool, f.scope, f.channel, 5, None, None, Direction::Older)
        .await
        .unwrap();
    assert_eq!(row_ids(&by_time), [id(2), id(0), id(4), id(1), id(3)]);
    assert_eq!(by_time.next_sequence, None);
    assert_eq!(by_time.rows[0].stored_event.channel_sequence, None);
}
