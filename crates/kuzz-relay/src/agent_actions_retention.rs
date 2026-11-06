//! Agent action log retention (AI tools plan §8, stage S1): rows of
//! `agent_actions` older than [`RETENTION_DAYS`] days are deleted.
//!
//! Always on, unlike the account purge: every hour one relay (Postgres
//! advisory lock) deletes the expired rows in batches. Rows of a community
//! being deleted are left to that deletion. Account purge removes an
//! account's rows at once ([`kuzz_db::Db::purge_account`]).

use std::time::Duration;

use chrono::Utc;
use kuzz_db::Db;
use tracing::{error, info};

use crate::state::AppState;

/// Advisory lock key that keeps retention on one relay at a time.
const AGENT_ACTIONS_RETENTION_LOCK_KEY: i64 = 0x4255_5A5A_4141_5254;

/// Days a row is kept.
pub const RETENTION_DAYS: i64 = 90;
const INTERVAL_SECS: u64 = 60 * 60;
/// Rows deleted per statement.
const BATCH: i64 = 5_000;

/// Spawn the hourly retention loop.
pub fn spawn(state: std::sync::Arc<AppState>) {
    tokio::spawn(async move {
        // Spread the first run so relays restarting together don't collide.
        let jitter = rand::random::<u64>() % 600;
        tokio::time::sleep(Duration::from_secs(60 + jitter)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(e) = run_once(&state.db).await {
                error!(error = %e, "agent action retention run failed");
            }
        }
    });
}

/// One pass, if this relay wins the lock: how many rows went.
pub async fn run_once(db: &Db) -> anyhow::Result<Option<u64>> {
    let Some(_leader) = db
        .try_lock_usage_metrics(AGENT_ACTIONS_RETENTION_LOCK_KEY)
        .await?
    else {
        return Ok(None);
    };
    let cutoff = Utc::now() - chrono::Duration::days(RETENTION_DAYS);
    let deleted = db.purge_agent_actions_before(cutoff, BATCH).await?;
    if deleted > 0 {
        info!(deleted, "agent action retention removed expired rows");
    }
    Ok(Some(deleted))
}
