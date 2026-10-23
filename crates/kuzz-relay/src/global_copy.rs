//! Global copy (global regions stage S5-1): global tables written in the
//! primary region, copied to every other region by Postgres logical
//! replication (`kuzz_db::global_copy`).
//!
//! - Primary with the link on: [`publish`] at startup, then [`run_heartbeat`]
//!   updates `global_heartbeat` every second.
//! - Copy (`KUZZ_GLOBAL_COPY`, on by default for a non-primary region with
//!   the link on): [`open_copy`] at startup waits until the first copy is
//!   done and the primary lists this region with this relay's link key, then
//!   claims the database and guards the copied tables. [`run_lag_monitor`]
//!   measures how far the copy is behind ([`CopyLag`],
//!   `kuzz_global_copy_lag_seconds`).
//!
//! A copy region never writes its own `regions` row: a region is added in
//! the primary (one row, design v2 P11), and the copy brings it here.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kuzz_db::Db;
use tracing::{info, warn};

use crate::config::Config;
use crate::state::AppState;

/// How often the primary beats and a copy measures.
const TICK: Duration = Duration::from_secs(1);
/// How often a waiting copy says what it waits for.
const WAIT_LOG_EVERY: Duration = Duration::from_secs(30);

/// How far this region's copy is behind the primary, as last measured.
#[derive(Debug)]
pub struct CopyLag {
    /// Milliseconds; `u64::MAX` before the first measurement or when the
    /// copy holds no heartbeat.
    millis: AtomicU64,
}

impl Default for CopyLag {
    fn default() -> Self {
        Self {
            millis: AtomicU64::new(u64::MAX),
        }
    }
}

impl CopyLag {
    /// The last measured lag, or `None` when unknown.
    pub fn get(&self) -> Option<Duration> {
        match self.millis.load(Ordering::Relaxed) {
            u64::MAX => None,
            millis => Some(Duration::from_millis(millis)),
        }
    }

    fn set(&self, lag: Option<Duration>) {
        let millis = lag.map_or(u64::MAX, |lag| {
            u64::try_from(lag.as_millis()).unwrap_or(u64::MAX - 1)
        });
        self.millis.store(millis, Ordering::Relaxed);
    }
}

/// Primary: publish the global tables (fatal on failure: other regions
/// would copy nothing or the wrong tables).
pub async fn publish(db: &Db) -> anyhow::Result<()> {
    db.publish_global_copy()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to publish the global tables: {e}"))?;
    info!(
        publication = kuzz_db::global_copy::PUBLICATION,
        "Global tables published"
    );
    Ok(())
}

/// Copy: wait for the first copy and this region's row, then claim the
/// database and guard the copied tables. Waits as long as it takes, saying
/// what it waits for; fails only on a configuration that cannot work.
pub async fn open_copy(db: &Db, config: &Config) -> anyhow::Result<()> {
    if db.local_region().await.ok().flatten().as_deref() == Some(config.region_id.as_str())
        && db.is_global_copy().await.unwrap_or(false)
    {
        // Restart of a running copy: the guards are in place; refresh them
        // in case this version copies more tables.
        wait_for_first_copy(db).await;
        db.guard_global_copy()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to guard the global copy: {e}"))?;
        info!(region = %config.region_id, "Global copy open");
        return Ok(());
    }
    wait_for_first_copy(db).await;
    wait_for_own_row(db, config).await?;
    db.claim_local_region(&config.region_id)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Failed to claim database for region {}: {e}",
                config.region_id
            )
        })?;
    db.guard_global_copy()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to guard the global copy: {e}"))?;
    info!(region = %config.region_id, "Global copy open: global tables are written in the primary");
    Ok(())
}

async fn wait_for_first_copy(db: &Db) {
    let mut said = None::<Instant>;
    loop {
        match db.global_copy_progress().await {
            Ok(progress) if progress.is_ready() => return,
            Ok(progress) => {
                if said.is_none_or(|at| at.elapsed() >= WAIT_LOG_EVERY) {
                    said = Some(Instant::now());
                    if !progress.subscribed {
                        warn!(
                            subscription = kuzz_db::global_copy::SUBSCRIPTION,
                            "Waiting for the global copy: create the subscription to the primary's \
                             publication (see docs/regions)"
                        );
                    } else if !progress.enabled {
                        warn!("Waiting for the global copy: the subscription is disabled");
                    } else if !progress.missing.is_empty() {
                        warn!(
                            missing = ?progress.missing,
                            "Waiting for the global copy: refresh the subscription \
                             (ALTER SUBSCRIPTION kuzz_global REFRESH PUBLICATION)"
                        );
                    } else {
                        info!(
                            ready = progress.ready,
                            "Waiting for the global copy: first copy in progress"
                        );
                    }
                }
            }
            Err(e) => {
                if said.is_none_or(|at| at.elapsed() >= WAIT_LOG_EVERY) {
                    said = Some(Instant::now());
                    warn!("Waiting for the global copy: could not read its state: {e}");
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Wait until the copied `regions` lists this region, not as the primary,
/// with this relay's link key.
async fn wait_for_own_row(db: &Db, config: &Config) -> anyhow::Result<()> {
    let link_pubkey = config
        .region_key
        .as_ref()
        .and_then(|key| nostr::Keys::parse(key.expose()).ok())
        .map(|key| key.public_key().to_hex())
        .ok_or_else(|| anyhow::anyhow!("KUZZ_GLOBAL_COPY needs KUZZ_REGION_KEY"))?;
    let mut said = None::<Instant>;
    loop {
        let regions = db.list_regions().await;
        let own = match &regions {
            Ok(regions) => regions.iter().find(|r| r.id == config.region_id),
            Err(_) => None,
        };
        if let Some(own) = own {
            if own.is_primary {
                anyhow::bail!(
                    "The primary lists region {} as the primary, but KUZZ_PRIMARY_REGION is {}",
                    config.region_id,
                    config.primary_region
                );
            }
            match db.region_link(&config.region_id).await {
                Ok(Some((_, Some(key)))) if key == link_pubkey => {
                    if own.connect_url != config.region_connect_url {
                        warn!(
                            listed = %own.connect_url,
                            configured = %config.region_connect_url,
                            "This region's address in the primary differs from KUZZ_REGION_CONNECT_URL"
                        );
                    }
                    return Ok(());
                }
                _ => {}
            }
        }
        if said.is_none_or(|at| at.elapsed() >= WAIT_LOG_EVERY) {
            said = Some(Instant::now());
            warn!(
                region = %config.region_id,
                connect_url = %config.region_connect_url,
                link_pubkey = %link_pubkey,
                "Waiting for this region to be added in the primary: \
                 kuzz-admin regions add --id <region> --connect-url <connect_url> --link-pubkey <link_pubkey> \
                 against the primary's database"
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Primary: update `global_heartbeat` every second.
pub async fn run_heartbeat(state: Arc<AppState>) {
    let mut failing = false;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        match state.db.beat_global_copy().await {
            Ok(()) => {
                if std::mem::take(&mut failing) {
                    info!("Global copy heartbeat recovered");
                }
            }
            Err(e) => {
                if !std::mem::replace(&mut failing, true) {
                    warn!("Global copy heartbeat failed; other regions will see their copy fall behind: {e}");
                }
            }
        }
    }
}

/// Copy: measure the copy's lag every second.
pub async fn run_lag_monitor(state: Arc<AppState>) {
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let lag = match state.db.global_copy_lag().await {
            Ok(lag) => lag.map(|secs| Duration::from_secs_f64(secs.max(0.0))),
            Err(e) => {
                tracing::debug!("Could not read the global copy lag: {e}");
                None
            }
        };
        state.global_copy_lag.set(lag);
        // Unknown counts as very far behind for alerts.
        metrics::gauge!("kuzz_global_copy_lag_seconds")
            .set(lag.map_or(f64::INFINITY, |lag| lag.as_secs_f64()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lag_starts_unknown_and_reads_back() {
        let lag = CopyLag::default();
        assert_eq!(lag.get(), None);
        lag.set(Some(Duration::from_millis(1500)));
        assert_eq!(lag.get(), Some(Duration::from_millis(1500)));
        lag.set(None);
        assert_eq!(lag.get(), None);
    }
}
