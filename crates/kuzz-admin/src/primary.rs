//! `kuzz-admin primary`: check the primary region and hand it over to
//! another region (global regions S6-5).
//!
//! `hand-over` runs with the old primary's database (`DATABASE_URL`), the new
//! primary's (`KUZZ_HANDOVER_TO_DATABASE_URL`) and every other region's
//! (`KUZZ_HANDOVER_COPY_DATABASE_URLS`). Global writes stop from the first
//! step until the relays restart with the new `KUZZ_PRIMARY_REGION`. Each
//! step looks at where things stand first, so running the same command
//! again continues an interrupted handover. Steps: `docs/regions-runbook.md`.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use kuzz_db::primary_handover::publisher_of;
use kuzz_db::{Db, DbConfig};

/// Primary region commands.
#[derive(Subcommand)]
pub enum PrimaryCommand {
    /// Show this database's part in the global copy: its region, whether it
    /// publishes or copies the global tables, which region `regions` marks as
    /// the primary, the copy's lag and subscribers that are behind.
    Check,
    /// Hand the primary role to another region (a planned handover).
    ///
    /// Run with DATABASE_URL set to the current primary's database,
    /// KUZZ_HANDOVER_TO_DATABASE_URL to the new primary's and
    /// KUZZ_HANDOVER_COPY_DATABASE_URLS to every other region's
    /// (comma-separated). Database users must be superusers. Global writes
    /// (sign-ups, profiles, new rooms) fail until the relays restart with
    /// KUZZ_PRIMARY_REGION set to the new primary.
    HandOver {
        /// Region id of the new primary.
        #[arg(long)]
        to: String,
        /// Connection string the other regions' database servers use to
        /// reach the new primary's database (libpq form, as in
        /// CREATE SUBSCRIPTION).
        #[arg(long)]
        publisher_conninfo: String,
        /// Seconds to wait for every region to catch up.
        #[arg(long, default_value_t = 120)]
        wait_secs: u64,
        /// Undo a handover that has not promoted the new primary yet: the old
        /// primary takes global writes again.
        #[arg(long)]
        abort: bool,
    },
}

/// Run a primary command; the process exit code.
pub async fn run(command: PrimaryCommand) -> Result<i32> {
    match command {
        PrimaryCommand::Check => {
            check(&crate::connect_db().await?).await?;
            Ok(0)
        }
        PrimaryCommand::HandOver {
            to,
            publisher_conninfo,
            wait_secs,
            abort,
        } => {
            hand_over(
                &to,
                &publisher_conninfo,
                Duration::from_secs(wait_secs),
                abort,
            )
            .await?;
            Ok(0)
        }
    }
}

/// One progress line.
fn step(name: &str, detail: serde_json::Value) {
    println!("{}", serde_json::json!({ "step": name, "detail": detail }));
}

async fn connect_url(url: &str, what: &str) -> Result<Db> {
    let config = DbConfig {
        database_url: url.to_owned(),
        ..DbConfig::default()
    }
    .with_session_timeouts_from_env();
    Db::new(&config)
        .await
        .with_context(|| format!("connecting to {what}"))
}

async fn check(db: &Db) -> Result<()> {
    let region = db.local_region().await?;
    let copy = db.is_global_copy().await?;
    let subscription = db.global_subscription().await?;
    let progress = db.global_copy_progress().await?;
    let position = db.wal_position().await?;
    let behind: Vec<serde_json::Value> = db
        .slots_behind(&position)
        .await?
        .into_iter()
        .map(|slot| {
            serde_json::json!({
                "slot": slot.slot,
                "active": slot.active,
                "behind_bytes": slot.behind_bytes,
            })
        })
        .collect();
    let primaries: i64 = sqlx::query_scalar("SELECT count(*) FROM regions WHERE is_primary")
        .fetch_one(db.pool())
        .await?;
    let role = match (copy, db.publishes_global_copy().await?) {
        (true, _) => "copy",
        (false, true) => "primary",
        (false, false) => "single",
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "region": region,
            "role": role,
            "listed_primary": db.listed_primary_region().await?,
            "primaries_listed": primaries,
            "subscription": subscription.map(|s| serde_json::json!({
                "enabled": s.enabled,
                "slot": s.slot,
                "publisher": s.publisher,
                "tables_ready": progress.ready,
                "tables_missing": progress.missing,
            })),
            "copy_lag_seconds": if copy { db.global_copy_lag().await? } else { None },
            "subscribers_behind": behind,
        }))?
    );
    Ok(())
}

/// Where a handover stands, read from the new primary's database.
#[derive(Debug, PartialEq, Eq)]
enum Stage {
    /// The new primary is still a copy.
    NotPromoted,
    /// The new primary publishes; regions still mark the old one.
    Promoted,
    /// `regions` marks the new primary.
    Done,
}

async fn stage(new: &Db, to: &str) -> Result<Stage> {
    if new.is_global_copy().await? {
        return Ok(Stage::NotPromoted);
    }
    if new.listed_primary_region().await?.as_deref() == Some(to) {
        return Ok(Stage::Done);
    }
    Ok(Stage::Promoted)
}

async fn hand_over(to: &str, conninfo: &str, wait: Duration, abort: bool) -> Result<()> {
    let old = crate::connect_db().await?;
    let new_url = std::env::var("KUZZ_HANDOVER_TO_DATABASE_URL")
        .map_err(|_| anyhow!("set KUZZ_HANDOVER_TO_DATABASE_URL to the new primary's database"))?;
    let new = connect_url(&new_url, "KUZZ_HANDOVER_TO_DATABASE_URL").await?;
    let mut copies = Vec::new();
    for (index, url) in std::env::var("KUZZ_HANDOVER_COPY_DATABASE_URLS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .enumerate()
    {
        // Named by position: the address may carry a password.
        copies.push(connect_url(url, &format!("copy database {index}")).await?);
    }

    let from = old
        .local_region()
        .await?
        .ok_or_else(|| anyhow!("DATABASE_URL belongs to no region"))?;
    match new.local_region().await?.as_deref() {
        Some(region) if region == to => {}
        Some(region) => {
            bail!("KUZZ_HANDOVER_TO_DATABASE_URL is region {region}'s database, not {to}'s")
        }
        None => bail!("KUZZ_HANDOVER_TO_DATABASE_URL belongs to no region"),
    }
    let wal_level: String = sqlx::query_scalar("SHOW wal_level")
        .fetch_one(new.pool())
        .await?;
    if wal_level != "logical" {
        bail!(
            "region {to}'s database has wal_level={wal_level}; a primary needs wal_level=logical"
        );
    }
    if from == to {
        bail!("region {to} is the database in DATABASE_URL; nothing to hand over");
    }
    let mut copy_regions = Vec::new();
    for copy in &copies {
        let region = copy
            .local_region()
            .await?
            .ok_or_else(|| anyhow!("a copy database belongs to no region"))?;
        if region == from || region == to {
            bail!("list region {region}'s database only once");
        }
        copy_regions.push(region);
    }
    step(
        "regions",
        serde_json::json!({ "from": from, "to": to, "copies": copy_regions }),
    );

    let stage = stage(&new, to).await?;
    let new_copies = new.global_subscription().await?.is_some();
    // The old primary, frozen by an earlier run: guarded, copying nothing.
    let old_frozen = old.is_global_copy().await? && old.global_subscription().await?.is_none();
    if stage == Stage::NotPromoted && old.is_global_copy().await? && !old_frozen {
        bail!(
            "DATABASE_URL is region {from}'s, a copy; point it at the current primary's database"
        );
    }
    if abort {
        if stage != Stage::NotPromoted || !new_copies {
            bail!(
                "region {to} has already left the copy; finish the handover, then hand the                  primary back instead of aborting"
            );
        }
        old.unguard_global_copy().await?;
        step("aborted", serde_json::json!({ "primary": from }));
        return Ok(());
    }

    if stage == Stage::NotPromoted && !new_copies && !old_frozen {
        bail!("region {to} has no subscription to the global copy; it cannot hold everything");
    }
    if stage == Stage::NotPromoted && new_copies {
        // 1. No more global writes in the old primary.
        if !old_frozen {
            if old.listed_primary_region().await?.as_deref() != Some(from.as_str()) {
                bail!(
                    "DATABASE_URL is region {from}'s, which regions does not mark as the primary"
                );
            }
            old.guard_global_copy().await?;
        }
        let position = old.wal_position().await?;
        step("frozen", serde_json::json!({ "wal": position }));

        // 2. Every subscriber has everything up to then.
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let behind = old.slots_behind(&position).await?;
            if behind.is_empty() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                let slots: Vec<String> = behind.iter().map(|slot| slot.slot.clone()).collect();
                bail!(
                    "subscribers {slots:?} did not catch up within {}s; global writes stay \
                     stopped. Run again to keep waiting, or with --abort to undo",
                    wait.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        step("caught_up", serde_json::json!({}));

        // 3. The new primary stops copying.
        if let Some(slot) = new.leave_global_copy().await? {
            old.drop_slot(&slot).await?;
        }
    }
    if stage == Stage::NotPromoted {
        // ... and publishes.
        new.unguard_global_copy().await?;
        new.publish_global_copy().await?;
        step("promoted", serde_json::json!({ "region": to }));
    }

    if stage != Stage::Done {
        // 4. Every other region copies from the new primary.
        for (db, region) in std::iter::once((&old, &from)).chain(copies.iter().zip(&copy_regions)) {
            follow(db, &old, conninfo, region).await?;
            step("subscribed", serde_json::json!({ "region": region }));
        }
        // 5. The new primary is the one `regions` marks.
        new.mark_primary_region(to).await?;
        new.beat_global_copy().await?;
        step("marked", serde_json::json!({ "primary": to }));
    }
    old.unpublish_global_copy().await?;

    step(
        "done",
        serde_json::json!(format!(
            "restart every relay with KUZZ_PRIMARY_REGION={to}, region {to}'s first"
        )),
    );
    Ok(())
}

/// Point `db`'s subscription at the new primary, unless it already is.
async fn follow(db: &Db, old: &Db, conninfo: &str, region: &str) -> Result<()> {
    let slot = kuzz_db::primary_handover::slot_name(region)?;
    if let Some(subscription) = db.global_subscription().await? {
        if subscription.slot.as_deref() == Some(slot.as_str())
            && subscription.publisher == publisher_of(conninfo)
        {
            return Ok(());
        }
        if let Some(old_slot) = db.leave_global_copy().await? {
            old.drop_slot(&old_slot).await?;
        }
    }
    db.subscribe_global_copy(conninfo, region).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::PrimaryCommand;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: PrimaryCommand,
    }

    #[test]
    fn hand_over_takes_a_region_and_a_connection() {
        let cli = Cli::try_parse_from([
            "primary",
            "hand-over",
            "--to",
            "jp",
            "--publisher-conninfo",
            "host=db-jp dbname=kuzz",
        ])
        .unwrap();
        match cli.command {
            PrimaryCommand::HandOver {
                to,
                wait_secs,
                abort,
                ..
            } => {
                assert_eq!(to, "jp");
                assert_eq!(wait_secs, 120);
                assert!(!abort);
            }
            PrimaryCommand::Check => panic!("parsed as check"),
        }
        assert!(Cli::try_parse_from(["primary", "hand-over", "--to", "jp"]).is_err());
        assert!(matches!(
            Cli::try_parse_from(["primary", "check"]).unwrap().command,
            PrimaryCommand::Check
        ));
    }
}
