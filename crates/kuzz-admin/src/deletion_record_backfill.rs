//! `kuzz-admin backfill-deletion-records`: store the relay-signed kind:9005
//! record for message removals made before the relay wrote one.
//!
//! Targets: soft-deleted content rows with a channel that were removed by a
//! staff delete (direct or by report) whose mutation committed, or by a kind:5
//! whose signer is not the row's raw author, and that have no relay record yet.
//! Each record is signed with the current time and written in its own
//! transaction under the same locks the relay uses, so the command can run next
//! to a live relay. Re-running is safe: recorded targets are skipped.
//!
//! Runbook:
//! 1. Run with `--dry-run --expect-pubkey <relay hex>` first. It prints the
//!    pubkey derived from the newest relay group-state event (39000/39002) and
//!    the candidate count, and writes nothing.
//! 2. Every record is a new channel row: it bumps `channels.accepted_sequence`
//!    and extends an ephemeral channel's TTL like any message would. Size the
//!    run (and `--batch-size`) from the dry-run count.
//! 3. Run without `--dry-run`. `KUZZ_RELAY_PRIVATE_KEY` must be the relay key;
//!    the run refuses before writing if it, `--expect-pubkey` and the derived
//!    pubkey disagree.
//! 4. Only after the backfill completed, set `KUZZ_DELETION_RECORDS_ADVERTISED`
//!    on the relay so clients start relying on the records.

use std::time::Instant;

use anyhow::Result;
use kuzz_core::CommunityId;
use kuzz_db::Db;
use nostr::{Keys, PublicKey};

/// Counts reported by one backfill run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DeletionRecordBackfillCounts {
    /// Targets without a relay record when visited.
    pub candidates: u64,
    /// Records this run stored.
    pub written: u64,
    /// Candidates that needed no record by write time.
    pub skipped: u64,
}

fn counts_json(counts: &DeletionRecordBackfillCounts) -> serde_json::Value {
    serde_json::json!({
        "candidates": counts.candidates,
        "written": counts.written,
        "skipped": counts.skipped,
    })
}

/// Confirm the relay identity before anything is written.
///
/// `expected` is the operator's `--expect-pubkey`; `signing` is the key that
/// would sign records (absent in a dry run). The signer of the newest relay
/// group-state event must match too when one exists. Returns that derived
/// pubkey for the run log.
pub async fn verify_relay_pubkey(
    db: &Db,
    community: CommunityId,
    expected: &PublicKey,
    signing: Option<&PublicKey>,
) -> Result<Option<PublicKey>> {
    if let Some(signing) = signing {
        if signing != expected {
            anyhow::bail!(
                "KUZZ_RELAY_PRIVATE_KEY is for {}, not the expected relay pubkey {}",
                signing.to_hex(),
                expected.to_hex()
            );
        }
    }
    let derived = db
        .newest_group_state_signer(community)
        .await?
        .map(|bytes| PublicKey::from_slice(&bytes))
        .transpose()
        .map_err(|e| anyhow::anyhow!("stored group-state pubkey is invalid: {e}"))?;
    if let Some(derived) = &derived {
        if derived != expected {
            anyhow::bail!(
                "the newest relay group-state event is signed by {}, not the expected relay \
                 pubkey {}; refusing to write records with a key clients would not trust",
                derived.to_hex(),
                expected.to_hex()
            );
        }
    }
    Ok(derived)
}

/// Visit every candidate page by page. Writes with `writer` (the relay key,
/// already verified to match `relay_pubkey`); `None` is a dry run.
///
/// `progress` receives the last target id of each page and the running counts.
pub async fn backfill(
    db: &Db,
    community: CommunityId,
    relay_pubkey: &PublicKey,
    writer: Option<&Keys>,
    batch_size: u32,
    mut progress: impl FnMut(&[u8], &DeletionRecordBackfillCounts),
) -> Result<DeletionRecordBackfillCounts> {
    if writer.is_some_and(|keys| keys.public_key() != *relay_pubkey) {
        anyhow::bail!("writer key does not match the relay pubkey");
    }
    let mut counts = DeletionRecordBackfillCounts::default();
    let mut after: Option<Vec<u8>> = None;
    loop {
        let page = db
            .deletion_record_backfill_candidates(
                community,
                relay_pubkey,
                after.as_deref(),
                batch_size,
            )
            .await?;
        let Some(last) = page.last().map(|c| c.target_id.clone()) else {
            break;
        };
        for candidate in &page {
            counts.candidates += 1;
            let Some(keys) = writer else {
                continue;
            };
            match db
                .write_deletion_record(community, &candidate.target_id, keys)
                .await?
            {
                Some(_) => counts.written += 1,
                None => counts.skipped += 1,
            }
        }
        progress(&last, &counts);
        after = Some(last);
    }
    Ok(counts)
}

/// Run the backfill for the configured relay's community; exit code 0 on success.
pub async fn run(dry_run: bool, expect_pubkey: &str, batch_size: u32) -> Result<i32> {
    let expected = PublicKey::from_hex(expect_pubkey)
        .map_err(|e| anyhow::anyhow!("invalid --expect-pubkey: {e}"))?;
    let relay_keys = match std::env::var("KUZZ_RELAY_PRIVATE_KEY") {
        Ok(hex) => Some(
            Keys::parse(&hex)
                .map_err(|e| anyhow::anyhow!("invalid KUZZ_RELAY_PRIVATE_KEY: {e}"))?,
        ),
        Err(_) if dry_run => None,
        Err(_) => anyhow::bail!(
            "KUZZ_RELAY_PRIVATE_KEY is required: records are signed by the relay key."
        ),
    };
    let db = crate::connect_db().await?;
    let tenant = crate::resolve_admin_tenant(&db).await?;
    let signing = relay_keys.as_ref().map(Keys::public_key);
    let derived = verify_relay_pubkey(&db, tenant.community(), &expected, signing.as_ref()).await?;
    println!(
        "{}",
        serde_json::json!({
            "event": "deletion_record_backfill_started",
            "community_host": tenant.host(),
            "relay_pubkey": expected.to_hex(),
            "derived_relay_pubkey": derived.map(|p| p.to_hex()),
            "dry_run": dry_run,
            "batch_size": batch_size,
        })
    );
    let started = Instant::now();
    let elapsed_ms = || i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let writer = if dry_run { None } else { relay_keys.as_ref() };
    let counts = backfill(
        &db,
        tenant.community(),
        &expected,
        writer,
        batch_size,
        |last, counts| {
            println!(
                "{}",
                serde_json::json!({
                    "event": "deletion_record_backfill_progress",
                    "last_target_id": hex::encode(last),
                    "duration_ms": elapsed_ms(),
                    "done": counts_json(counts),
                })
            );
        },
    )
    .await?;
    println!(
        "{}",
        serde_json::json!({
            "event": "deletion_record_backfill_completed",
            "dry_run": dry_run,
            "duration_ms": elapsed_ms(),
            "done": counts_json(&counts),
        })
    );
    Ok(0)
}

#[cfg(test)]
#[path = "deletion_record_backfill_tests.rs"]
mod tests;
