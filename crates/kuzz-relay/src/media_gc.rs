//! Unused-file cleanup: removes uploaded files that no live message uses.
//!
//! Once a day one relay (Postgres advisory lock) walks each community's
//! sidecars (`_meta/{community}/{sha}.json`). A blob counts as unused in a
//! community only when [`kuzz_db::Db::media_hashes_in_use`] finds nothing that
//! names it — no live event of any kind, no event deleted within the grace
//! period, no upload within the grace period (re-uploads refresh it), no
//! reported event, profile picture, icon, blob report, feedback attachment or
//! workflow — and its sidecar is older than the grace period. Each file is
//! checked again right before it is removed, and once more after its sidecar
//! is gone: a file uploaded or sent meanwhile gets its sidecar and bytes back.
//!
//! Blob bytes (`{sha}.{ext}`, `{sha}.thumb.jpg`) are shared by every
//! community. An unused community file loses its sidecar and its uploader and
//! reference rows. When no community has a sidecar left, the bytes move to
//! `_trash/{unix seconds}/{key}` instead of being deleted. Each run first
//! looks at the trash: a file some community has a sidecar for again (an
//! upload that raced the cleanup, or a re-upload) is restored, and the rest
//! is deleted for good once it has been in the trash for the grace period.
//! Restoring runs in `dry-run` too, so switching from `on` to `dry-run` (not
//! `off`) is the way to stop removals and still get raced files back.
//! Upload records (`_uploads/...`) stay for moderation.
//!
//! `KUZZ_MEDIA_GC` selects `off` (default), `dry-run` (log what would be
//! removed and restore raced files, remove nothing) or `on`. `KUZZ_MEDIA_GC_GRACE_DAYS` (default 7,
//! at least 1) and `KUZZ_MEDIA_GC_INTERVAL_SECS` (default one day) tune it.
//! The bucket must not be shared with another deployment: blob bytes carry no
//! deployment prefix, so another deployment's sidecars are invisible here.
//!
//! Regions share one bucket, but each region's database only knows the
//! messages of the rooms it serves. So the cleanup pauses (removes and
//! restores nothing) unless this database is the only registered region, or
//! no region is registered yet: with a second region it would remove files
//! that region's rooms still use. Cleanup across regions needs every region's
//! references (global regions S4).

use std::collections::BTreeMap;
use std::time::Duration;

use kuzz_core::tenant::TenantContext;
use kuzz_core::CommunityId;
use kuzz_db::media_access::MAX_CLEANUP_HASHES_PER_CALL;
use kuzz_db::Db;
use kuzz_media::MediaStorage;
use tracing::{error, info, warn};

use crate::state::AppState;

/// Advisory lock key that keeps the cleanup on one relay at a time.
const MEDIA_GC_LOCK_KEY: i64 = 0x4255_5A5A_4D47_4321;

const DEFAULT_GRACE_DAYS: i64 = 7;
const DEFAULT_INTERVAL_SECS: u64 = 24 * 60 * 60;
const LIST_PAGE_KEYS: usize = 1000;
const TRASH_PREFIX: &str = "_trash/";

/// What the cleanup does with unused files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcMode {
    /// No cleanup.
    Off,
    /// Find unused files and log them; change nothing.
    DryRun,
    /// Remove unused files.
    On,
}

impl GcMode {
    fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("on") | Some("true") | Some("1") => Self::On,
            Some("dry-run") | Some("dry_run") | Some("dryrun") => Self::DryRun,
            _ => Self::Off,
        }
    }
}

/// Cleanup settings read from the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcSettings {
    /// Off, dry-run or on.
    pub mode: GcMode,
    /// Files uploaded, or named by an event deleted, within this many days
    /// stay; trashed files are deleted after this many days.
    pub grace_days: i64,
    /// Seconds between runs.
    pub interval_secs: u64,
}

impl GcSettings {
    /// Read `KUZZ_MEDIA_GC`, `KUZZ_MEDIA_GC_GRACE_DAYS`, `KUZZ_MEDIA_GC_INTERVAL_SECS`.
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var("KUZZ_MEDIA_GC").ok().as_deref(),
            std::env::var("KUZZ_MEDIA_GC_GRACE_DAYS").ok().as_deref(),
            std::env::var("KUZZ_MEDIA_GC_INTERVAL_SECS").ok().as_deref(),
        )
    }

    fn from_values(mode: Option<&str>, grace_days: Option<&str>, interval: Option<&str>) -> Self {
        Self {
            mode: GcMode::parse(mode),
            // Never below one day: a shorter grace could remove a file
            // between its upload and the message that sends it.
            grace_days: grace_days
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(DEFAULT_GRACE_DAYS)
                .max(1),
            interval_secs: interval
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(DEFAULT_INTERVAL_SECS)
                .max(60),
        }
    }

    fn grace(&self) -> chrono::Duration {
        chrono::Duration::days(self.grace_days)
    }
}

/// Counts from one cleanup run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GcReport {
    /// Sidecars looked at.
    pub scanned: u64,
    /// Unused community files found (removed when on, listed when dry-run).
    pub unused: u64,
    /// Bytes of those files' originals.
    pub bytes: u64,
    /// Community sidecars removed.
    pub sidecars_removed: u64,
    /// Shared blob objects (original and thumbnail) moved to the trash.
    pub trashed: u64,
    /// Trashed objects put back because a community uses them again.
    pub restored: u64,
    /// Trashed objects deleted for good.
    pub purged: u64,
}

/// Spawn the daily cleanup loop unless it is off.
pub fn spawn(state: std::sync::Arc<AppState>) {
    let settings = GcSettings::from_env();
    if settings.mode == GcMode::Off {
        return;
    }
    info!(?settings, "media cleanup enabled");
    tokio::spawn(async move {
        // Spread the first run so relays restarting together don't collide.
        let jitter = rand::random::<u64>() % settings.interval_secs.min(3600);
        tokio::time::sleep(Duration::from_secs(60 + jitter)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(settings.interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match region_pause(&state.db).await {
                Ok(None) => {}
                Ok(Some(reason)) => {
                    warn!(reason, "media cleanup paused");
                    metrics::counter!("kuzz_media_gc_paused_total").increment(1);
                    continue;
                }
                Err(e) => {
                    error!(error = %e, "media cleanup paused: regions unreadable");
                    metrics::counter!("kuzz_media_gc_paused_total").increment(1);
                    continue;
                }
            }
            if let Err(e) = run_once(&state.db, &state.media_storage, settings).await {
                error!(error = %e, "media cleanup run failed");
            }
        }
    });
}

/// Why the cleanup must not run now, read fresh each run (see the module
/// docs), or `None` when it may.
async fn region_pause(db: &Db) -> anyhow::Result<Option<String>> {
    let local = db.local_region().await?;
    let regions = db.list_regions().await?;
    Ok(region_pause_reason(local.as_deref(), &regions))
}

fn region_pause_reason(
    local: Option<&str>,
    regions: &[kuzz_db::region::RegionRecord],
) -> Option<String> {
    match regions {
        [] => None,
        [only] if local.is_none_or(|local| local == only.id) => None,
        [only] => Some(format!(
            "this database belongs to region {}, but only region {} is registered",
            local.unwrap_or_default(),
            only.id
        )),
        _ => Some(format!(
            "{} regions are registered; cleanup across regions is not built yet",
            regions.len()
        )),
    }
}

/// One cleanup pass, if this relay wins the lock: the trash first, then every
/// community's files.
pub async fn run_once(
    db: &Db,
    storage: &MediaStorage,
    settings: GcSettings,
) -> anyhow::Result<Option<GcReport>> {
    let Some(_leader) = db.try_lock_usage_metrics(MEDIA_GC_LOCK_KEY).await? else {
        return Ok(None);
    };
    let mut report = GcReport::default();
    if let Err(e) = empty_trash(db, storage, settings, &mut report).await {
        warn!(error = %e, "media cleanup: trash step stopped early");
    }
    for community in db.all_community_ids().await? {
        let community = CommunityId::from_uuid(community);
        if let Err(e) = clean_community(db, storage, settings, community, &mut report).await {
            warn!(%community, error = %e, "media cleanup skipped a community");
        }
    }
    info!(
        mode = ?settings.mode,
        scanned = report.scanned,
        unused = report.unused,
        bytes = report.bytes,
        sidecars_removed = report.sidecars_removed,
        trashed = report.trashed,
        restored = report.restored,
        purged = report.purged,
        "media cleanup run finished"
    );
    Ok(Some(report))
}

async fn clean_community(
    db: &Db,
    storage: &MediaStorage,
    settings: GcSettings,
    community: CommunityId,
    report: &mut GcReport,
) -> anyhow::Result<()> {
    let shas = community_sidecar_hashes(storage, community).await?;
    report.scanned += shas.len() as u64;
    let grace_cutoff = chrono::Utc::now() - settings.grace();
    let tenant = TenantContext::resolved(community, String::new());

    for batch in shas.chunks(MAX_CLEANUP_HASHES_PER_CALL) {
        let in_use = db
            .media_hashes_in_use(community, batch, grace_cutoff)
            .await?;
        let mut old_unused = Vec::new();
        for sha in batch.iter().filter(|sha| !in_use.contains(sha)) {
            // The sidecar's first-upload time covers files uploaded before
            // uploads were recorded in the database.
            match storage.get_sidecar(&tenant, sha).await {
                Ok(meta) if meta.uploaded_at <= grace_cutoff.timestamp() => {
                    old_unused.push((sha.clone(), meta));
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(%community, sha, error = %e, "media cleanup: sidecar unreadable; kept")
                }
            }
        }
        for (sha, meta) in &old_unused {
            if settings.mode != GcMode::On {
                report.unused += 1;
                report.bytes += meta.size;
                info!(%community, sha, size = meta.size, "media cleanup (dry-run): would remove");
                continue;
            }
            remove_unused(db, storage, &tenant, sha, meta, grace_cutoff, report).await?;
        }
    }
    Ok(())
}

/// Whether one file is in use, checked right before acting on it.
async fn still_in_use(
    db: &Db,
    community: CommunityId,
    sha: &str,
    grace_cutoff: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<bool> {
    let hashes = [sha.to_owned()];
    Ok(!db
        .media_hashes_in_use(community, &hashes, grace_cutoff)
        .await?
        .is_empty())
}

/// Every blob hash with a sidecar in `community`.
async fn community_sidecar_hashes(
    storage: &MediaStorage,
    community: CommunityId,
) -> anyhow::Result<Vec<String>> {
    let prefix = format!("_meta/{community}/");
    Ok(list_keys(storage, &prefix)
        .await?
        .iter()
        .filter_map(|key| sidecar_hash(key, &prefix))
        .collect())
}

async fn list_keys(storage: &MediaStorage, prefix: &str) -> anyhow::Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut token = None;
    loop {
        let page = storage
            .list_prefix_page(prefix, token.take(), LIST_PAGE_KEYS)
            .await?;
        keys.extend(page.objects.into_iter().map(|(key, _)| key));
        match page.next_continuation_token {
            Some(next) if page.is_truncated => token = Some(next),
            _ => return Ok(keys),
        }
    }
}

fn sidecar_hash(key: &str, prefix: &str) -> Option<String> {
    let sha = key.strip_prefix(prefix)?.strip_suffix(".json")?;
    is_sha256_hex(sha).then(|| sha.to_owned())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// Remove `community`'s copy of an unused file, and trash the shared bytes
/// when no community has the file any more.
async fn remove_unused(
    db: &Db,
    storage: &MediaStorage,
    tenant: &TenantContext,
    sha: &str,
    meta: &kuzz_media::BlobMeta,
    grace_cutoff: chrono::DateTime<chrono::Utc>,
    report: &mut GcReport,
) -> anyhow::Result<()> {
    let community = tenant.community();
    // Checked per file right before acting: a message or upload may have
    // arrived since the batch check.
    if still_in_use(db, community, sha, grace_cutoff).await? {
        return Ok(());
    }
    // Bytes first: if the run stops part way, the sidecar is still there, so
    // the next run sees the file again and the trash step restores the bytes.
    let mut trashed = Vec::new();
    if !any_other_sidecar(db, storage, sha, community).await? {
        let now = chrono::Utc::now().timestamp();
        for key in blob_object_keys(storage, sha).await? {
            let trash = format!("{TRASH_PREFIX}{now}/{key}");
            storage.copy(&key, &trash).await?;
            storage.delete(&key).await?;
            report.trashed += 1;
            trashed.push((key, trash));
        }
    }
    // Community purge drains this lease, so a cleanup never races a purge.
    let serving_write = kuzz_deletion::acquire_serving_write(db, community, "media_gc").await?;
    let sidecar = MediaStorage::sidecar_key(community, sha);
    serving_write
        .protect(async { storage.delete(&sidecar).await.map_err(anyhow::Error::from) })
        .await??;

    // An upload or message that landed while the bytes moved keeps the file:
    // put the sidecar and bytes back.
    if still_in_use(db, community, sha, grace_cutoff).await? {
        storage.put_sidecar(tenant, sha, meta).await?;
        for (key, trash) in &trashed {
            if !exists(storage, key).await? {
                storage.copy(trash, key).await?;
                report.restored += 1;
            }
            storage.delete(trash).await?;
        }
        info!(%community, sha, "media cleanup: file was used meanwhile; kept");
        return Ok(());
    }
    report.unused += 1;
    report.bytes += meta.size;
    report.sidecars_removed += 1;
    // Only rows from before the grace cutoff and for events no longer live,
    // so a row written in the last moment survives.
    db.forget_media(community, sha, grace_cutoff).await?;
    info!(%community, sha, "media cleanup: removed unused file");
    Ok(())
}

/// Whether a community other than `except` (all of them when `None`) still
/// has a sidecar for `sha`. Reads the community list fresh each time.
async fn any_other_sidecar(
    db: &Db,
    storage: &MediaStorage,
    sha: &str,
    except: impl Into<Option<CommunityId>>,
) -> anyhow::Result<bool> {
    let except = except.into();
    for other in db.all_community_ids().await? {
        let other = CommunityId::from_uuid(other);
        if Some(other) == except {
            continue;
        }
        if exists(storage, &MediaStorage::sidecar_key(other, sha)).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `key` exists. A one-key listing, not HEAD: the S3 client sleeps
/// and retries every failed request, and a missing object is a failed HEAD.
async fn exists(storage: &MediaStorage, key: &str) -> anyhow::Result<bool> {
    let page = storage.list_prefix_page(key, None, 1).await?;
    Ok(page.objects.iter().any(|(found, _)| found == key))
}

/// The shared objects for `sha`: the original (`{sha}.{ext}`) and thumbnail.
async fn blob_object_keys(storage: &MediaStorage, sha: &str) -> anyhow::Result<Vec<String>> {
    Ok(list_keys(storage, sha)
        .await?
        .into_iter()
        .filter(|key| is_blob_object_key(key, sha))
        .collect())
}

fn is_blob_object_key(key: &str, sha: &str) -> bool {
    key.strip_prefix(sha)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|ext| !ext.is_empty() && !ext.contains('/'))
}

/// A trashed object: when it was trashed, its original key and its hash.
#[derive(Debug, PartialEq, Eq)]
struct TrashEntry {
    trashed_at: i64,
    original: String,
    sha: String,
}

fn parse_trash_key(key: &str) -> Option<TrashEntry> {
    let (stamp, original) = key.strip_prefix(TRASH_PREFIX)?.split_once('/')?;
    let sha = original.get(..64).filter(|sha| is_sha256_hex(sha))?;
    let trashed_at = stamp.parse().ok()?;
    is_blob_object_key(original, sha).then(|| TrashEntry {
        trashed_at,
        original: original.to_owned(),
        sha: sha.to_owned(),
    })
}

/// Restore trashed files a community has a sidecar for again; when on, delete
/// the rest once they have been in the trash for the grace period.
async fn empty_trash(
    db: &Db,
    storage: &MediaStorage,
    settings: GcSettings,
    report: &mut GcReport,
) -> anyhow::Result<()> {
    let mut by_sha: BTreeMap<String, Vec<(String, TrashEntry)>> = BTreeMap::new();
    for key in list_keys(storage, TRASH_PREFIX).await? {
        if let Some(entry) = parse_trash_key(&key) {
            by_sha
                .entry(entry.sha.clone())
                .or_default()
                .push((key, entry));
        }
    }
    let purge_before = (chrono::Utc::now() - settings.grace()).timestamp();
    for (sha, entries) in by_sha {
        if any_other_sidecar(db, storage, &sha, None).await? {
            for (key, entry) in &entries {
                // An upload that raced the cleanup may have written fresh bytes.
                if !exists(storage, &entry.original).await? {
                    storage.copy(key, &entry.original).await?;
                    report.restored += 1;
                }
                storage.delete(key).await?;
            }
            info!(sha, "media cleanup: restored a file a community uses again");
            continue;
        }
        if settings.mode != GcMode::On {
            continue;
        }
        for (key, entry) in &entries {
            if entry.trashed_at <= purge_before {
                storage.delete(key).await?;
                report.purged += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    const SHA: &str = "0168c8815ba00ec20d05c8659ea37939f8fca8afabfe648708fb0b2ef3f0174c";

    #[test]
    fn mode_defaults_off_and_parses_dry_run_and_on() {
        assert_eq!(GcMode::parse(None), GcMode::Off);
        assert_eq!(GcMode::parse(Some("")), GcMode::Off);
        assert_eq!(GcMode::parse(Some("nonsense")), GcMode::Off);
        assert_eq!(GcMode::parse(Some("dry-run")), GcMode::DryRun);
        assert_eq!(GcMode::parse(Some(" DRY_RUN ")), GcMode::DryRun);
        assert_eq!(GcMode::parse(Some("on")), GcMode::On);
    }

    #[test]
    fn cleanup_runs_only_with_a_single_region() {
        let region = |id: &str, is_primary: bool| kuzz_db::region::RegionRecord {
            id: id.to_owned(),
            connect_url: format!("wss://{id}.test"),
            is_primary,
        };
        assert_eq!(region_pause_reason(None, &[]), None);
        assert_eq!(region_pause_reason(Some("kr"), &[region("kr", true)]), None);
        assert_eq!(region_pause_reason(None, &[region("kr", true)]), None);
        assert!(region_pause_reason(Some("jp"), &[region("kr", true)]).is_some());
        let two = [region("kr", true), region("jp", false)];
        assert!(region_pause_reason(Some("kr"), &two).is_some());
        assert!(region_pause_reason(Some("jp"), &two).is_some());
    }

    #[test]
    fn grace_is_never_below_one_day() {
        let s = GcSettings::from_values(Some("on"), Some("0"), Some("10"));
        assert_eq!(s.grace_days, 1);
        assert_eq!(s.interval_secs, 60);
        let d = GcSettings::from_values(None, None, None);
        assert_eq!(d.grace_days, 7);
        assert_eq!(d.interval_secs, 86_400);
    }

    #[test]
    fn sidecar_keys_yield_hashes_only() {
        let prefix = "_meta/0eaa0496-74c9-4d86-bccc-c938743932ab/";
        assert_eq!(
            sidecar_hash(&format!("{prefix}{SHA}.json"), prefix).as_deref(),
            Some(SHA)
        );
        assert_eq!(sidecar_hash(&format!("{prefix}{SHA}.png"), prefix), None);
        assert_eq!(sidecar_hash(&format!("{prefix}abc.json"), prefix), None);
    }

    #[test]
    fn trash_keys_carry_time_original_and_hash() {
        assert_eq!(
            parse_trash_key(&format!("_trash/1760000000/{SHA}.thumb.jpg")),
            Some(TrashEntry {
                trashed_at: 1_760_000_000,
                original: format!("{SHA}.thumb.jpg"),
                sha: SHA.to_owned(),
            })
        );
        assert_eq!(parse_trash_key(&format!("_trash/x/{SHA}.png")), None);
        assert_eq!(parse_trash_key(&format!("_trash/1/{SHA}")), None);
        assert_eq!(parse_trash_key("_trash/1/abc.png"), None);
        assert_eq!(parse_trash_key(&format!("_meta/1/{SHA}.png")), None);
    }

    #[test]
    fn blob_object_keys_are_original_and_thumbnail_only() {
        assert!(is_blob_object_key(&format!("{SHA}.png"), SHA));
        assert!(is_blob_object_key(&format!("{SHA}.thumb.jpg"), SHA));
        assert!(!is_blob_object_key(SHA, SHA));
        assert!(!is_blob_object_key(&format!("{SHA}0.png"), SHA));
        assert!(!is_blob_object_key(&format!("{SHA}./x"), SHA));
    }

    /// Real Postgres and MinIO: the cleanup removes only what nothing uses.
    /// Needs `KUZZ_TEST_DATABASE_URL` (or the default test DB) and an existing
    /// bucket named by `KUZZ_TEST_S3_BUCKET` at `KUZZ_S3_ENDPOINT`.
    #[tokio::test]
    #[ignore = "requires Postgres and MinIO"]
    async fn cleanup_removes_only_unused_files_against_real_storage() {
        use kuzz_media::BlobMeta;

        let mut config = crate::config::Config::for_test();
        config.media.s3_bucket = std::env::var("KUZZ_TEST_S3_BUCKET").expect("test bucket");
        config.media.s3_addressing_style = kuzz_media::config::S3AddressingStyle::Path;
        let storage = MediaStorage::new(&config.media).expect("storage");
        let pool = sqlx::PgPool::connect(&crate::test_support::database_url())
            .await
            .expect("test database");
        let db = Db::from_pool(pool.clone());
        let community_a = db
            .ensure_configured_community(&format!("gc-a-{}.test", Uuid::new_v4().simple()))
            .await
            .expect("community a")
            .id;
        let community_b = db
            .ensure_configured_community(&format!("gc-b-{}.test", Uuid::new_v4().simple()))
            .await
            .expect("community b")
            .id;
        let sha = |_: u8| -> String {
            use std::fmt::Write;
            rand::random::<[u8; 32]>()
                .iter()
                .fold(String::new(), |mut out, b| {
                    let _ = write!(out, "{b:02x}");
                    out
                })
        };
        let (unused, shared, in_use, fresh) = (sha(1), sha(2), sha(3), sha(4));
        let old = chrono::Utc::now().timestamp() - 30 * 86_400;
        let meta = |uploaded_at: i64| BlobMeta {
            dim: String::new(),
            blurhash: String::new(),
            thumb_url: String::new(),
            ext: "png".into(),
            mime_type: "image/png".into(),
            size: 3,
            uploaded_at,
            duration_secs: None,
        };
        let put = |community: CommunityId, sha: String, uploaded_at: i64| {
            let storage = &storage;
            async move {
                storage
                    .put(&format!("{sha}.png"), b"png", "image/png")
                    .await
                    .expect("blob");
                storage
                    .put(&format!("{sha}.thumb.jpg"), b"jpg", "image/jpeg")
                    .await
                    .expect("thumb");
                let tenant = TenantContext::resolved(community, String::new());
                storage
                    .put_sidecar(&tenant, &sha, &meta(uploaded_at))
                    .await
                    .expect("sidecar");
            }
        };
        put(community_a, unused.clone(), old).await;
        put(community_a, shared.clone(), old).await;
        put(community_b, shared.clone(), old).await;
        put(community_a, in_use.clone(), old).await;
        put(community_a, fresh.clone(), chrono::Utc::now().timestamp()).await;

        // Community B still uses `shared`; community A uses `in_use`.
        let keys = nostr::Keys::generate();
        for (community, sha) in [(community_b, &shared), (community_a, &in_use)] {
            let event = nostr::EventBuilder::new(
                nostr::Kind::Custom(30175),
                format!("https://x.test/media/{sha}.png"),
            )
            .sign_with_keys(&keys)
            .expect("event");
            db.insert_event(community, &event, None)
                .await
                .expect("event write");
        }

        let exists = |key: String| {
            let storage = &storage;
            async move { storage.head(&key).await.expect("head") }
        };
        let sidecar = |c: CommunityId, sha: &str| MediaStorage::sidecar_key(c, sha);

        let dry = GcSettings::from_values(Some("dry-run"), None, None);
        let report = run_once(&db, &storage, dry)
            .await
            .expect("dry run")
            .expect("leader");
        assert!(
            report.unused >= 2,
            "unused in A and shared in A: {report:?}"
        );
        assert_eq!(report.sidecars_removed, 0);
        assert!(
            exists(sidecar(community_a, &unused)).await,
            "dry-run deletes nothing"
        );

        let on = GcSettings::from_values(Some("on"), None, None);
        run_once(&db, &storage, on)
            .await
            .expect("run")
            .expect("leader");

        // Unused everywhere: sidecar, original and thumbnail are gone.
        assert!(!exists(sidecar(community_a, &unused)).await);
        assert!(!exists(format!("{unused}.png")).await);
        assert!(!exists(format!("{unused}.thumb.jpg")).await);
        // Unused in A but used in B: A's sidecar goes, the bytes stay.
        assert!(!exists(sidecar(community_a, &shared)).await);
        assert!(exists(sidecar(community_b, &shared)).await);
        assert!(exists(format!("{shared}.png")).await);
        // Named by a live event, or uploaded recently: untouched.
        assert!(exists(sidecar(community_a, &in_use)).await);
        assert!(exists(format!("{in_use}.png")).await);
        assert!(exists(sidecar(community_a, &fresh)).await);
        assert!(exists(format!("{fresh}.png")).await);

        // The unused file's bytes sit in the trash, not deleted yet.
        let trashed: Vec<String> = list_keys(&storage, TRASH_PREFIX)
            .await
            .expect("trash")
            .into_iter()
            .filter(|key| key.contains(&unused))
            .collect();
        assert_eq!(trashed.len(), 2, "original and thumbnail: {trashed:?}");

        // A community uploads the file again before the trash is emptied:
        // the next run restores the bytes and keeps them.
        put_sidecar_only(
            &storage,
            community_b,
            &unused,
            chrono::Utc::now().timestamp(),
        )
        .await;
        let report = run_once(&db, &storage, on)
            .await
            .expect("run")
            .expect("leader");
        assert_eq!(report.restored, 2, "{report:?}");
        assert!(exists(format!("{unused}.png")).await);
        assert!(exists(format!("{unused}.thumb.jpg")).await);
        assert!(list_keys(&storage, TRASH_PREFIX)
            .await
            .expect("trash")
            .iter()
            .all(|key| !key.contains(&unused)));

        // Trash older than the grace period is deleted for good.
        let stale = format!("{TRASH_PREFIX}{}/{in_use}.thumb.jpg", old);
        storage
            .put(&stale, b"jpg", "image/jpeg")
            .await
            .expect("stale trash");
        let recent = format!(
            "{TRASH_PREFIX}{}/{fresh}.thumb.jpg",
            chrono::Utc::now().timestamp()
        );
        storage
            .put(&recent, b"jpg", "image/jpeg")
            .await
            .expect("recent trash");
        // Trash is restored while any community has a sidecar, so drop them.
        for sha in [&in_use, &fresh] {
            storage
                .delete(&MediaStorage::sidecar_key(community_a, sha))
                .await
                .expect("drop sidecar");
        }
        let report = run_once(&db, &storage, on)
            .await
            .expect("run")
            .expect("leader");
        assert!(report.purged >= 1, "{report:?}");
        assert!(!exists(stale).await, "old trash deleted");
        assert!(exists(recent).await, "recent trash kept");
    }

    async fn put_sidecar_only(
        storage: &MediaStorage,
        community: CommunityId,
        sha: &str,
        uploaded_at: i64,
    ) {
        let meta = kuzz_media::BlobMeta {
            dim: String::new(),
            blurhash: String::new(),
            thumb_url: String::new(),
            ext: "png".into(),
            mime_type: "image/png".into(),
            size: 3,
            uploaded_at,
            duration_secs: None,
        };
        storage
            .put_sidecar(
                &TenantContext::resolved(community, String::new()),
                sha,
                &meta,
            )
            .await
            .expect("sidecar");
    }
}
