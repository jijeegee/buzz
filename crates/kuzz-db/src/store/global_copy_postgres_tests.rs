//! Global copy (migration 0087) against a real database. The publication
//! and guards are made inside a transaction that is rolled back, so the
//! shared test database is left as it was.
use sqlx::{PgPool, Postgres, Transaction};

use super::global_copy::{
    guard_global_copy_on, publish_global_copy_on, COPIED_TABLES, LOCAL_TABLES, PUBLICATION,
};

async fn pool() -> PgPool {
    let pool = PgPool::connect(&crate::test_support::database_url())
        .await
        .expect("isolated test database");
    if std::env::var("KUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
        crate::migration::run_migrations(&pool)
            .await
            .expect("migrate");
    }
    pool
}

/// Run `sql` inside a savepoint; the error message if it failed.
async fn attempt(tx: &mut Transaction<'_, Postgres>, sql: &str) -> Option<String> {
    sqlx::query("SAVEPOINT attempt")
        .execute(&mut **tx)
        .await
        .expect("savepoint");
    match sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut **tx)
        .await
    {
        Ok(_) => {
            sqlx::query("RELEASE SAVEPOINT attempt")
                .execute(&mut **tx)
                .await
                .expect("release");
            None
        }
        Err(error) => {
            sqlx::query("ROLLBACK TO SAVEPOINT attempt")
                .execute(&mut **tx)
                .await
                .expect("rollback to savepoint");
            Some(error.to_string())
        }
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn every_table_is_copied_or_local_and_copied_tables_have_keys() {
    let pool = pool().await;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text FROM pg_class c \
           JOIN pg_namespace n ON n.oid = c.relnamespace \
          WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p') \
            AND NOT c.relispartition AND c.relname <> '_sqlx_migrations'",
    )
    .fetch_all(&pool)
    .await
    .expect("tables");
    for table in &tables {
        assert!(
            COPIED_TABLES.contains(&table.as_str()) || LOCAL_TABLES.contains(&table.as_str()),
            "table {table} is neither copied nor local: add it to kuzz_db::global_copy"
        );
    }
    for table in COPIED_TABLES.iter().chain(LOCAL_TABLES) {
        assert!(
            tables.iter().any(|t| t == table),
            "listed table {table} does not exist"
        );
    }
    // Replication finds a copied row by its primary key.
    for table in COPIED_TABLES {
        let keyed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_index i JOIN pg_class c ON c.oid = i.indrelid \
              WHERE c.relname = $1 AND i.indisprimary)",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("primary key");
        assert!(keyed, "copied table {table} has no primary key");
    }
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn the_primary_publishes_global_tables_and_room_less_events() {
    let pool = pool().await;
    let mut tx = pool.begin().await.expect("begin");
    sqlx::query("UPDATE local_region SET global_copy = false")
        .execute(&mut *tx)
        .await
        .expect("not a copy");
    publish_global_copy_on(&mut tx).await.expect("publish");
    // Twice is fine.
    publish_global_copy_on(&mut tx)
        .await
        .expect("publish again");

    let published: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT tablename::text, rowfilter FROM pg_publication_tables WHERE pubname = $1",
    )
    .bind(PUBLICATION)
    .fetch_all(&mut *tx)
    .await
    .expect("publication");
    assert_eq!(published.len(), COPIED_TABLES.len() + 1);
    let events_filter = published
        .iter()
        .find(|(name, _)| name == "events")
        .and_then(|(_, filter)| filter.clone())
        .expect("events published");
    assert!(
        events_filter.contains("channel_id IS NULL"),
        "{events_filter}"
    );

    // Every partition has the identity the filter needs, so updating a
    // room message still works in the primary.
    let lacking: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_partition_tree('events') t JOIN pg_class c ON c.oid = t.relid \
          WHERE c.relkind IN ('r', 'p') AND c.relreplident <> 'f'",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("identity");
    assert_eq!(lacking, 0);
    assert_eq!(
        attempt(
            &mut tx,
            "UPDATE events SET deleted_at = deleted_at WHERE false"
        )
        .await,
        None
    );
    tx.rollback().await.expect("rollback");
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_copy_refuses_local_writes_to_global_tables_only() {
    let pool = pool().await;
    let mut tx = pool.begin().await.expect("begin");
    for sql in [
        "DELETE FROM local_region",
        "INSERT INTO regions (id, connect_url, is_primary) VALUES ('t5', 'wss://t5.test', false) \
         ON CONFLICT (id) DO NOTHING",
        "INSERT INTO local_region (region_id) VALUES ('t5')",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut *tx)
            .await
            .expect(sql);
    }
    guard_global_copy_on(&mut tx).await.expect("guard");
    guard_global_copy_on(&mut tx).await.expect("guard again");
    let is_copy: bool = sqlx::query_scalar("SELECT kuzz_is_global_copy()")
        .fetch_one(&mut *tx)
        .await
        .expect("flag");
    assert!(is_copy);

    let refused = |error: Option<String>| {
        let error = error.expect("refused");
        assert!(error.contains("written in the primary region"), "{error}");
    };
    refused(
        attempt(
            &mut tx,
            "INSERT INTO regions (id, connect_url) VALUES ('t6', 'wss://t6.test')",
        )
        .await,
    );
    refused(
        attempt(
            &mut tx,
            "UPDATE regions SET connect_url = connect_url WHERE id = 't5'",
        )
        .await,
    );
    refused(attempt(&mut tx, "DELETE FROM regions WHERE id = 't5'").await);
    refused(attempt(&mut tx, "TRUNCATE users CASCADE").await);
    refused(
        attempt(
            &mut tx,
            "INSERT INTO communities (id, host) VALUES (gen_random_uuid(), 'copy-guard.test')",
        )
        .await,
    );

    // A room-less event is the primary's; a room message is this region's.
    let community: uuid::Uuid = uuid::Uuid::new_v4();
    sqlx::query("SAVEPOINT community")
        .execute(&mut *tx)
        .await
        .expect("savepoint");
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .expect("as replication");
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(community)
        .bind(format!("copy-{community}.test"))
        .execute(&mut *tx)
        .await
        .expect("replication writes the copy");
    sqlx::query("SET LOCAL session_replication_role = origin")
        .execute(&mut *tx)
        .await
        .expect("back to normal");
    let insert = |channel: &str| {
        format!(
            "INSERT INTO events (id, pubkey, created_at, kind, tags, content, sig, \
             community_id, channel_id) VALUES (decode(md5(random()::text) || md5(random()::text), 'hex'), \
             decode(repeat('ab', 32), 'hex'), now(), 1, '[]', '', decode(repeat('cd', 64), 'hex'), \
             '{community}', {channel})"
        )
    };
    refused(attempt(&mut tx, &insert("NULL")).await);
    // The room message may fail for other reasons (no such room); it must
    // not be the guard.
    if let Some(error) = attempt(&mut tx, &insert(&format!("'{}'", uuid::Uuid::new_v4()))).await {
        assert!(!error.contains("written in the primary region"), "{error}");
    }

    // Creating a room in a copy does not write the copied directory.
    let room_error = attempt(
        &mut tx,
        &format!(
            "INSERT INTO channels (community_id, id, name, channel_type, visibility, created_by) \
             VALUES ('{community}', gen_random_uuid(), 'copy-room', 'stream', 'open', \
             decode(repeat('ab', 32), 'hex'))"
        ),
    )
    .await;
    if let Some(error) = room_error {
        assert!(!error.contains("written in the primary region"), "{error}");
    }
    tx.rollback().await.expect("rollback");
}
