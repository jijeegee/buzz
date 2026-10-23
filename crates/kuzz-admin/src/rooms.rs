//! `kuzz-admin rooms`: move a room to another region (global regions S6-3,
//! S6-4).
//!
//! `move` runs with three databases: the primary's (`DATABASE_URL`, whose
//! directory says where rooms live), the room's old home
//! (`KUZZ_MOVE_FROM_DATABASE_URL`) and its new home
//! (`KUZZ_MOVE_TO_DATABASE_URL`); either home defaults to the primary's. It
//! freezes the room in the old home, waits for its side effects, copies it,
//! checks the copy, switches the directory, opens the copy and marks the old
//! room moved out. Each step looks at where things stand first, so running
//! the same command again continues an interrupted move. Steps:
//! `docs/regions-runbook.md`.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::Subcommand;
use kuzz_core::tenant::TenantContext;
use kuzz_db::room_move::{copy_room, RoomHere};
use kuzz_db::{Db, DbConfig};
use uuid::Uuid;

/// How long a step that waits for the directory copy is tried.
const DIRECTORY_WAIT: Duration = Duration::from_secs(60);

/// Pause between the side effects finishing and the copy.
const SETTLE: Duration = Duration::from_secs(5);

/// Room commands.
#[derive(Subcommand)]
pub enum RoomsCommand {
    /// Move a room to another region.
    ///
    /// Run with DATABASE_URL set to the primary region's database,
    /// KUZZ_MOVE_FROM_DATABASE_URL to the room's current home's and
    /// KUZZ_MOVE_TO_DATABASE_URL to the new home's (each defaults to
    /// DATABASE_URL). The new home's database user must be a superuser.
    /// KUZZ_MOVE_REDIS_URLS (comma-separated, every region's Redis) makes
    /// apps reconnect at the end. Writes to the room are refused, with a
    /// retryable error, while it moves.
    Move {
        /// Room (channel) id.
        #[arg(long)]
        channel: Uuid,
        /// Region id to move the room to.
        #[arg(long)]
        to: String,
        /// Undo a move that has not switched yet: remove the copy and let
        /// the old home take events again.
        #[arg(long)]
        abort: bool,
        /// Seconds to wait for the room's side effects before giving up.
        #[arg(long, default_value_t = 300)]
        wait_secs: u64,
    },
    /// Remove the old home's rows of a room that moved out of it. Run with
    /// DATABASE_URL set to the old home's database.
    ForgetMoved {
        /// Room (channel) id.
        #[arg(long)]
        channel: Uuid,
        /// Days a moved room is kept before it may be removed.
        #[arg(long, default_value_t = 7)]
        kept_days: u32,
    },
}

/// Run a room command; the process exit code.
pub async fn run(command: RoomsCommand) -> Result<i32> {
    match command {
        RoomsCommand::Move {
            channel,
            to,
            abort,
            wait_secs,
        } => {
            move_room(channel, &to, abort, Duration::from_secs(wait_secs)).await?;
            Ok(0)
        }
        RoomsCommand::ForgetMoved { channel, kept_days } => {
            let db = crate::connect_db().await?;
            let tenant = crate::resolve_admin_tenant(&db).await?;
            let removed = db
                .forget_moved_room(tenant.community(), channel, kept_days)
                .await?;
            step("forgotten", serde_json::json!({ "rows": removed }));
            Ok(0)
        }
    }
}

/// One progress line.
fn step(name: &str, detail: serde_json::Value) {
    println!("{}", serde_json::json!({ "step": name, "detail": detail }));
}

async fn connect(var: &str) -> Result<Db> {
    let url = match std::env::var(var) {
        Ok(url) if !url.is_empty() => url,
        _ => return crate::connect_db().await,
    };
    let config = DbConfig {
        database_url: url,
        ..DbConfig::default()
    }
    .with_session_timeouts_from_env();
    Db::new(&config)
        .await
        .with_context(|| format!("connecting to {var}"))
}

/// The region a database belongs to; it must be `expected`.
async fn check_region(db: &Db, var: &str, expected: &str) -> Result<()> {
    match db.local_region().await? {
        Some(region) if region == expected => Ok(()),
        Some(region) => bail!("{var} is region {region}'s database, not region {expected}'s"),
        None => bail!("{var} belongs to no region yet; start its relay first"),
    }
}

/// Try `attempt` until it succeeds or [`DIRECTORY_WAIT`] passes: a region's
/// copy of the directory follows the primary within seconds.
async fn until_directory_follows<T, F, Fut>(what: &str, mut attempt: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = kuzz_db::Result<T>>,
{
    let deadline = tokio::time::Instant::now() + DIRECTORY_WAIT;
    loop {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(error) if tokio::time::Instant::now() < deadline => {
                tracing::debug!(%error, "{what}: waiting for the directory copy");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(anyhow!("{what}: {error}")),
        }
    }
}

async fn move_room(channel: Uuid, to: &str, abort: bool, wait: Duration) -> Result<()> {
    let primary = crate::connect_db().await?;
    let tenant = crate::resolve_admin_tenant(&primary).await?;
    let community = tenant.community();
    let from_db = connect("KUZZ_MOVE_FROM_DATABASE_URL").await?;
    let to_db = connect("KUZZ_MOVE_TO_DATABASE_URL").await?;
    check_region(&to_db, "KUZZ_MOVE_TO_DATABASE_URL", to).await?;

    let home = primary
        .room_home(community, channel)
        .await?
        .ok_or_else(|| anyhow!("the primary's directory has no room {channel}"))?;
    step(
        "home",
        serde_json::json!({ "region": home.region, "epoch": home.epoch }),
    );

    if home.region == to {
        if abort {
            bail!("room {channel} already lives in {to}; move it back instead of aborting");
        }
        // The switch is done: finish what a stopped run left.
        return finish(&from_db, &to_db, &tenant, channel).await;
    }
    check_region(&from_db, "KUZZ_MOVE_FROM_DATABASE_URL", &home.region).await?;

    if abort {
        let removed = to_db.delete_room_copy(community, channel).await?;
        let unfrozen = from_db.unfreeze_room(community, channel).await?;
        step(
            "aborted",
            serde_json::json!({ "copy_rows_removed": removed, "unfrozen": unfrozen }),
        );
        return Ok(());
    }

    let since = from_db.freeze_room(community, channel).await?;
    step("frozen", serde_json::json!({ "since": since }));

    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let pending = from_db
            .room_side_effects_pending(community, channel)
            .await?;
        if pending == 0 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            from_db.unfreeze_room(community, channel).await?;
            bail!(
                "{pending} side effects of room {channel} are still waiting; the room was unfrozen"
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    // Media an event names is recorded just after the event: let the last
    // events' records land before the copy.
    tokio::time::sleep(SETTLE).await;

    match to_db.room_here(community, channel).await? {
        RoomHere::Absent => {
            let copied = copy_room(&from_db, &to_db, community, channel).await?;
            let rows: serde_json::Map<String, serde_json::Value> = copied
                .iter()
                .filter(|table| table.rows > 0)
                .map(|table| (table.table.to_owned(), table.rows.into()))
                .collect();
            step("copied", serde_json::Value::Object(rows));
        }
        RoomHere::Frozen(_) => step("copied", serde_json::json!("earlier")),
        other => bail!("the new home already holds room {channel} ({other:?}); nothing was moved"),
    }

    let (old, new) = (
        from_db.room_fingerprint(community, channel).await?,
        to_db.room_fingerprint(community, channel).await?,
    );
    if old != new {
        to_db.delete_room_copy(community, channel).await?;
        from_db.unfreeze_room(community, channel).await?;
        bail!(
            "the copy does not match (old {old:?}, new {new:?}); it was removed and the room \
             was unfrozen"
        );
    }
    step(
        "checked",
        serde_json::json!({ "events": new.events, "last_sequence": new.last_sequence }),
    );

    let moved = primary
        .switch_room_home(community, channel, &home, to)
        .await?;
    step(
        "switched",
        serde_json::json!({ "region": moved.region, "epoch": moved.epoch }),
    );
    finish(&from_db, &to_db, &tenant, channel).await
}

/// After the switch: open the copy, mark the old room moved out and make
/// apps reconnect.
async fn finish(from_db: &Db, to_db: &Db, tenant: &TenantContext, channel: Uuid) -> Result<()> {
    let community = tenant.community();
    let opened = until_directory_follows("opening the new home", || {
        to_db.unfreeze_room(community, channel)
    })
    .await?;
    step("opened", serde_json::json!({ "was_frozen": opened }));
    if from_db.local_region().await? != to_db.local_region().await? {
        match from_db.room_here(community, channel).await? {
            RoomHere::Absent => {}
            _ => {
                until_directory_follows("marking the old home moved out", || {
                    from_db.mark_room_moved_out(community, channel)
                })
                .await?;
                step("moved_out", serde_json::json!({}));
            }
        }
    }
    reconnect(tenant).await;
    step("done", serde_json::json!({}));
    Ok(())
}

/// Ask every region's relays to drop the community's connections, so apps
/// watching the room at its old home reconnect (and fill any gap).
async fn reconnect(tenant: &TenantContext) {
    let urls = std::env::var("KUZZ_MOVE_REDIS_URLS").unwrap_or_default();
    let urls: Vec<&str> = urls
        .split(',')
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .collect();
    if urls.is_empty() {
        step(
            "reconnect_skipped",
            serde_json::json!(
                "KUZZ_MOVE_REDIS_URLS is not set; apps watching the room at its \
                 old home see new messages only after they reconnect"
            ),
        );
        return;
    }
    let channel = kuzz_pubsub::conn_control::conn_control_channel(tenant);
    let payload =
        serde_json::to_string(&kuzz_pubsub::conn_control::ConnControl::DisconnectCommunity)
            .expect("a fixed command serializes");
    for (index, url) in urls.iter().enumerate() {
        let sent = publish(url, &channel, &payload).await;
        // The address may carry a password: name the entry by position.
        match sent {
            Ok(pods) => step(
                "reconnect",
                serde_json::json!({ "redis": index, "pods": pods }),
            ),
            Err(error) => step(
                "reconnect_failed",
                serde_json::json!({ "redis": index, "error": error.to_string() }),
            ),
        }
    }
}

async fn publish(url: &str, channel: &str, payload: &str) -> Result<i64> {
    use deadpool_redis::redis;
    let client = redis::Client::open(url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    Ok(redis::cmd("PUBLISH")
        .arg(channel)
        .arg(payload)
        .query_async(&mut conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::RoomsCommand;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: RoomsCommand,
    }

    #[test]
    fn move_takes_a_room_and_a_region() {
        let id = "8b1b4d53-1f0a-4c55-9a8c-0d6f4b2f7f10";
        let cli = Cli::try_parse_from(["rooms", "move", "--channel", id, "--to", "jp"]).unwrap();
        match cli.command {
            RoomsCommand::Move {
                channel,
                to,
                abort,
                wait_secs,
            } => {
                assert_eq!(channel.to_string(), id);
                assert_eq!(to, "jp");
                assert!(!abort);
                assert_eq!(wait_secs, 300);
            }
            RoomsCommand::ForgetMoved { .. } => panic!("parsed as forget-moved"),
        }
        assert!(Cli::try_parse_from(["rooms", "move", "--channel", "nope", "--to", "jp"]).is_err());
        let forget = Cli::try_parse_from(["rooms", "forget-moved", "--channel", id]).unwrap();
        assert!(matches!(
            forget.command,
            RoomsCommand::ForgetMoved { kept_days: 7, .. }
        ));
    }
}
