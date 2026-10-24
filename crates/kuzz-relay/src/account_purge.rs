//! Account purge: 30 days after a user deletes their account, remove what
//! identifies them and keep the messages others saw
//! ([`kuzz_db::Db::purge_account`] says exactly what goes and what stays).
//!
//! Every interval one relay (Postgres advisory lock) purges the accounts
//! whose self-deletion window has passed, each in its own transaction, and
//! logs one line per account with what was removed. A login inside the
//! window restores the account and takes it off the list.
//!
//! `KUZZ_ACCOUNT_PURGE` selects `off` (default), `dry-run` (log what each
//! purge would remove, change nothing) or `on`.
//! `KUZZ_ACCOUNT_PURGE_INTERVAL_SECS` (default one hour, at least 60) sets
//! how often it looks.

use std::time::Duration;

use kuzz_db::identity::PurgeOutcome;
use kuzz_db::Db;
use tracing::{error, info, warn};

use crate::state::AppState;

/// Advisory lock key that keeps the purge on one relay at a time.
const ACCOUNT_PURGE_LOCK_KEY: i64 = 0x4255_5A5A_5055_5247;

const DEFAULT_INTERVAL_SECS: u64 = 60 * 60;
/// Accounts handled per run; the rest wait for the next run.
const BATCH: i64 = 50;

/// What the purge does with accounts whose window has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeMode {
    /// No purge.
    Off,
    /// Run each purge and roll it back; log what it would remove.
    DryRun,
    /// Purge.
    On,
}

impl PurgeMode {
    fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("on") | Some("true") | Some("1") => Self::On,
            Some("dry-run") | Some("dry_run") | Some("dryrun") => Self::DryRun,
            _ => Self::Off,
        }
    }
}

/// Purge settings read from the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PurgeSettings {
    /// Off, dry-run or on.
    pub mode: PurgeMode,
    /// Seconds between runs.
    pub interval_secs: u64,
}

impl PurgeSettings {
    /// Read `KUZZ_ACCOUNT_PURGE` and `KUZZ_ACCOUNT_PURGE_INTERVAL_SECS`.
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var("KUZZ_ACCOUNT_PURGE").ok().as_deref(),
            std::env::var("KUZZ_ACCOUNT_PURGE_INTERVAL_SECS")
                .ok()
                .as_deref(),
        )
    }

    fn from_values(mode: Option<&str>, interval: Option<&str>) -> Self {
        Self {
            mode: PurgeMode::parse(mode),
            interval_secs: interval
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(DEFAULT_INTERVAL_SECS)
                .max(60),
        }
    }
}

/// Counts from one run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PurgeReport {
    /// Accounts purged (or, in a dry run, that would be).
    pub purged: u64,
    /// Relay operators left for a person to remove.
    pub skipped_operators: u64,
    /// Accounts whose purge failed; retried next run.
    pub failed: u64,
}

/// Spawn the purge loop unless it is off.
pub fn spawn(state: std::sync::Arc<AppState>) {
    let settings = PurgeSettings::from_env();
    if settings.mode == PurgeMode::Off {
        return;
    }
    info!(?settings, "account purge enabled");
    tokio::spawn(async move {
        // Spread the first run so relays restarting together don't collide.
        let jitter = rand::random::<u64>() % settings.interval_secs.min(600);
        tokio::time::sleep(Duration::from_secs(60 + jitter)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(settings.interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(e) = run_once(&state.db, settings.mode).await {
                error!(error = %e, "account purge run failed");
            }
        }
    });
}

/// One pass over the due accounts, if this relay wins the lock.
pub async fn run_once(db: &Db, mode: PurgeMode) -> anyhow::Result<Option<PurgeReport>> {
    if mode == PurgeMode::Off {
        return Ok(None);
    }
    let Some(_leader) = db.try_lock_usage_metrics(ACCOUNT_PURGE_LOCK_KEY).await? else {
        return Ok(None);
    };
    let dry_run = mode == PurgeMode::DryRun;
    let mut report = PurgeReport::default();
    for account in db.due_account_purges(BATCH).await? {
        // A prefix is enough to find the account in the database; no email
        // or name is logged.
        let account_prefix = &account.to_hex()[..12];
        match db.purge_account(&account, dry_run).await {
            Ok(PurgeOutcome::Purged(counts)) => {
                report.purged += 1;
                info!(
                    account = account_prefix,
                    dry_run,
                    identities = counts.identities,
                    devices = counts.devices,
                    access_tokens = counts.access_tokens,
                    bots = counts.bots,
                    agent_keys = counts.agent_keys,
                    key_backups = counts.key_backups,
                    events = counts.events,
                    profiles = counts.profiles,
                    community_rows = counts.community_rows,
                    "account purge: {}",
                    if dry_run { "would purge" } else { "purged" }
                );
            }
            Ok(PurgeOutcome::SkippedOperator) => {
                report.skipped_operators += 1;
                warn!(
                    account = account_prefix,
                    "account purge: deleted account is a relay operator; remove it by hand"
                );
            }
            Ok(PurgeOutcome::NotDue) => {}
            Err(e) => {
                report.failed += 1;
                warn!(account = account_prefix, error = %e, "account purge failed; retried next run");
            }
        }
    }
    if report != PurgeReport::default() {
        info!(
            ?mode,
            purged = report.purged,
            skipped_operators = report.skipped_operators,
            failed = report.failed,
            "account purge run finished"
        );
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_off_and_hourly() {
        let settings = PurgeSettings::from_values(None, None);
        assert_eq!(settings.mode, PurgeMode::Off);
        assert_eq!(settings.interval_secs, 3600);
    }

    #[test]
    fn settings_parse_modes_and_floor_interval() {
        assert_eq!(PurgeMode::parse(Some(" ON ")), PurgeMode::On);
        assert_eq!(PurgeMode::parse(Some("dry-run")), PurgeMode::DryRun);
        assert_eq!(PurgeMode::parse(Some("dry_run")), PurgeMode::DryRun);
        assert_eq!(PurgeMode::parse(Some("yes please")), PurgeMode::Off);
        assert_eq!(
            PurgeSettings::from_values(Some("on"), Some("5")).interval_secs,
            60
        );
        assert_eq!(
            PurgeSettings::from_values(Some("on"), Some("7200")).interval_secs,
            7200
        );
    }
}
