//! `kuzz-admin backfill-chat-state`: fill server-side chat state (Chats S3)
//! from message history for the configured relay's community.
//!
//! One channel per transaction, channels listed in pages of `--batch-size`
//! with one JSON progress line per page.
//!
//! Order: turn on `KUZZ_CHAT_STATE_WRITES` first (this command never changes
//! it), so the relay records all new activity, then run this to fill in
//! everything older. State the relay already wrote is kept. Re-running is
//! safe and changes nothing new, so an interrupted run is simply started
//! again.

use std::time::Instant;

use anyhow::Result;
use kuzz_db::chat_backfill::{ChatBackfillCounts, ChatBackfillOptions};

fn counts_json(counts: &ChatBackfillCounts) -> serde_json::Value {
    serde_json::json!({
        "channels": counts.channels,
        "main_last_updated": counts.main_last_updated,
        "thread_state_inserted": counts.thread_state_inserted,
        "thread_state_updated": counts.thread_state_updated,
        "threads_closed": counts.threads_closed,
        "member_reads_filled": counts.member_reads_filled,
        "follows_inserted": counts.follows_inserted,
        "follow_reads_filled": counts.follow_reads_filled,
    })
}

fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}

/// Run the backfill; exit code 0 on success.
pub async fn run(dry_run: bool, batch_size: u32) -> Result<i32> {
    let db = crate::connect_db().await?;
    let tenant = crate::resolve_admin_tenant(&db).await?;
    let community = tenant.community();
    let options = ChatBackfillOptions::from_now(dry_run);
    println!(
        "{}",
        serde_json::json!({
            "event": "chat_backfill_started",
            "community_host": tenant.host(),
            "dry_run": dry_run,
            "batch_size": batch_size,
            "follow_window_days": options.follow_window.num_days(),
        })
    );

    let started = Instant::now();
    let mut total = ChatBackfillCounts::default();
    let mut after = None;
    loop {
        let page = db
            .chat_backfill_channels(community, after, batch_size)
            .await?;
        let Some(last) = page.last().copied() else {
            break;
        };
        for channel in page {
            match db
                .backfill_chat_state_channel(community, channel, &options)
                .await
            {
                Ok(counts) => total += counts,
                Err(error) => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "event": "chat_backfill_failed",
                            "channel_id": channel.to_string(),
                            "duration_ms": elapsed_ms(started),
                            "dry_run": dry_run,
                            "done": counts_json(&total),
                            "error": error.to_string(),
                        })
                    );
                    return Err(error.into());
                }
            }
        }
        after = Some(last);
        println!(
            "{}",
            serde_json::json!({
                "event": "chat_backfill_progress",
                "last_channel_id": last.to_string(),
                "duration_ms": elapsed_ms(started),
                "done": counts_json(&total),
            })
        );
    }

    println!(
        "{}",
        serde_json::json!({
            "event": "chat_backfill_completed",
            "dry_run": dry_run,
            "duration_ms": elapsed_ms(started),
            "unchanged": total.is_unchanged(),
            "done": counts_json(&total),
        })
    );
    Ok(0)
}
