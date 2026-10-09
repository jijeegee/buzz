//! Observer telemetry tier policy and per-agent send limits.
//!
//! The relay publishes the owner's tier and the executor limits that go with
//! it at `GET /api/observer/policy`. The harness paces and summarises its
//! telemetry against those limits so a conforming executor never trips the
//! relay's own (1.5x) enforcement.
//!
//! Policy resolution at startup: a fresh fetch (waited for up to
//! [`INITIAL_POLICY_WAIT`]) → the last policy persisted in the state dir →
//! the free tier.

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use buzz_core::observer::OBSERVER_MAX_PLAINTEXT_LEN;
use serde::Deserialize;
use tokio::sync::{watch, Notify};
use tokio::time::Instant;

/// How long the publisher waits for the first policy fetch before falling
/// back to the persisted (or free) policy.
pub(crate) const INITIAL_POLICY_WAIT: Duration = Duration::from_secs(2);
/// Periodic policy refresh.
const POLICY_REFRESH_INTERVAL: Duration = Duration::from_secs(600);
/// Retry delay after a failed fetch; keeps a transient startup failure from
/// pinning the agent to its fallback tier for a full refresh interval.
const POLICY_RETRY_AFTER_FAILURE: Duration = Duration::from_secs(60);
/// Minimum spacing between rejection-triggered refreshes.
const POLICY_REFRESH_DEBOUNCE: Duration = Duration::from_secs(30);
/// How long a relay-advertised share stays adopted without a policy refresh.
const SHARE_TTL: Duration = Duration::from_secs(60);
/// Sliding window for the per-minute frame and byte caps.
const MINUTE: Duration = Duration::from_secs(60);
const SECOND: Duration = Duration::from_secs(1);

/// Observer telemetry tier of the agent's owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ObserverTier {
    Free,
    Standard,
    Premium,
}

impl ObserverTier {
    /// The `detail` marker summarised events carry; `None` on premium, whose
    /// telemetry is sent unsummarised.
    pub(crate) fn detail(self) -> Option<&'static str> {
        match self {
            Self::Free => Some("free"),
            Self::Standard => Some("standard"),
            Self::Premium => None,
        }
    }
}

/// Per-agent executor limits of one tier. Byte values are plaintext bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct ExecutorLimits {
    pub tick_ms: u64,
    pub agent_frames_per_min: u64,
    pub agent_burst_per_sec: u64,
    pub agent_frame_max_bytes: u64,
    pub agent_bytes_per_min: u64,
}

impl ExecutorLimits {
    /// Mirrors the relay's `TierLimits::FREE` executor values.
    pub(crate) const FREE: Self = Self {
        tick_ms: 1_500,
        agent_frames_per_min: 60,
        agent_burst_per_sec: 5,
        agent_frame_max_bytes: 4_096,
        agent_bytes_per_min: 100_000,
    };
}

/// The policy fields the executor acts on; the rest of the response is kept
/// only in the persisted copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct ObserverPolicy {
    pub tier: ObserverTier,
    pub executor: ExecutorLimits,
}

impl ObserverPolicy {
    /// Used when neither a fetched nor a persisted policy is available.
    pub(crate) const FREE: Self = Self {
        tier: ObserverTier::Free,
        executor: ExecutorLimits::FREE,
    };

    /// The relay's premium executor defaults.
    #[cfg(test)]
    pub(crate) const PREMIUM: Self = Self {
        tier: ObserverTier::Premium,
        executor: ExecutorLimits {
            tick_ms: 1_000,
            agent_frames_per_min: 60,
            agent_burst_per_sec: 5,
            agent_frame_max_bytes: 65_535,
            agent_bytes_per_min: 4_000_000,
        },
    };

    /// The relay's standard executor defaults.
    #[cfg(test)]
    pub(crate) const STANDARD: Self = Self {
        tier: ObserverTier::Standard,
        executor: ExecutorLimits {
            tick_ms: 1_000,
            agent_frames_per_min: 60,
            agent_burst_per_sec: 5,
            agent_frame_max_bytes: 16_384,
            agent_bytes_per_min: 400_000,
        },
    };

    /// Parse a `GET /api/observer/policy` response body.
    pub(crate) fn parse(raw: &serde_json::Value) -> Option<Self> {
        serde_json::from_value(raw.clone()).ok()
    }

    /// Normal publish interval (one frame per tick when there is no backlog).
    pub(crate) fn tick(&self) -> Duration {
        Duration::from_millis(self.executor.tick_ms.max(100))
    }

    /// Largest frame this tier may send, never above the NIP-44 plaintext cap.
    pub(crate) fn frame_cap(&self) -> usize {
        usize::try_from(self.executor.agent_frame_max_bytes)
            .unwrap_or(usize::MAX)
            .clamp(1, OBSERVER_MAX_PLAINTEXT_LEN)
    }

    /// Spacing between frames while draining a backlog.
    pub(crate) fn burst_spacing(&self) -> Duration {
        SECOND / u32::try_from(self.executor.agent_burst_per_sec.max(1)).unwrap_or(u32::MAX)
    }
}

/// Where the active policy came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicySource {
    Fetched,
    Persisted,
    Default,
}

/// Startup fallback order: fetched → persisted → free.
pub(crate) fn initial_policy(
    fetched: Option<ObserverPolicy>,
    persisted: Option<ObserverPolicy>,
) -> (ObserverPolicy, PolicySource) {
    match (fetched, persisted) {
        (Some(policy), _) => (policy, PolicySource::Fetched),
        (None, Some(policy)) => (policy, PolicySource::Persisted),
        (None, None) => (ObserverPolicy::FREE, PolicySource::Default),
    }
}

/// Persisted policy file for one agent on one relay.
pub(crate) fn policy_path(state_dir: &Path, agent_pubkey_hex: &str, relay_url: &str) -> PathBuf {
    crate::session_ledger::agent_state_file(
        state_dir,
        agent_pubkey_hex,
        relay_url,
        "observer-policy",
    )
}

/// Load the last persisted policy. Missing or unreadable files yield `None`.
pub(crate) fn load_persisted(path: &Path) -> Option<ObserverPolicy> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(path = %path.display(), "cannot read observer policy: {error}");
            return None;
        }
    };
    let policy = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|raw| ObserverPolicy::parse(&raw));
    if policy.is_none() {
        tracing::warn!(path = %path.display(), "ignoring unparseable persisted observer policy");
    }
    policy
}

/// Persist a successful policy response verbatim (atomic replace).
pub(crate) fn persist(path: &Path, raw: &serde_json::Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(raw).map_err(std::io::Error::other)?;
    crate::session_ledger::write_atomically(path, &bytes)
}

/// The publisher's view of the policy task.
pub(crate) struct ObserverTierLinks {
    /// Latest successfully fetched policy; `None` until the first success.
    pub policy: watch::Receiver<Option<ObserverPolicy>>,
    /// Policy loaded from disk at startup, the first fallback.
    pub persisted: Option<ObserverPolicy>,
    /// Ask the policy task for an early (debounced) refresh.
    pub refresh: Arc<Notify>,
    /// Observer rejections reported by the relay socket.
    pub feedback: watch::Receiver<crate::relay::ObserverRelayFeedback>,
}

impl ObserverTierLinks {
    /// Links pinned to one policy, with no fetch task behind them.
    #[cfg(test)]
    pub(crate) fn fixed(
        policy: ObserverPolicy,
    ) -> (Self, watch::Sender<crate::relay::ObserverRelayFeedback>) {
        let (policy_tx, policy_rx) = watch::channel(Some(policy));
        // Keep the value readable after the sender is gone.
        drop(policy_tx);
        let (feedback_tx, feedback_rx) = watch::channel(Default::default());
        (
            Self {
                policy: policy_rx,
                persisted: None,
                refresh: Arc::new(Notify::new()),
                feedback: feedback_rx,
            },
            feedback_tx,
        )
    }
}

/// Spawn the policy task: fetch now, every [`POLICY_REFRESH_INTERVAL`], and on
/// request (at most every [`POLICY_REFRESH_DEBOUNCE`]). Successful responses
/// are persisted to `persist_path`. The task exits once every policy receiver
/// is gone.
pub(crate) fn spawn_policy_task<F, Fut>(
    fetch: F,
    persist_path: Option<PathBuf>,
) -> (
    watch::Receiver<Option<ObserverPolicy>>,
    Arc<Notify>,
    tokio::task::JoinHandle<()>,
)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = Result<serde_json::Value, String>> + Send,
{
    let (tx, rx) = watch::channel(None);
    let refresh = Arc::new(Notify::new());
    let task_refresh = Arc::clone(&refresh);
    let handle = tokio::spawn(async move {
        loop {
            let fetched_at = Instant::now();
            let next = match fetch().await {
                Ok(raw) => match ObserverPolicy::parse(&raw) {
                    Some(policy) => {
                        if let Some(path) = &persist_path {
                            if let Err(error) = persist(path, &raw) {
                                tracing::warn!(
                                    path = %path.display(),
                                    "cannot persist observer policy: {error}"
                                );
                            }
                        }
                        tracing::info!(tier = ?policy.tier, "observer policy fetched");
                        tx.send_replace(Some(policy));
                        POLICY_REFRESH_INTERVAL
                    }
                    None => {
                        tracing::warn!("observer policy response unparseable: {raw}");
                        POLICY_RETRY_AFTER_FAILURE
                    }
                },
                Err(error) => {
                    tracing::warn!("observer policy fetch failed: {error}");
                    POLICY_RETRY_AFTER_FAILURE
                }
            };
            tokio::select! {
                _ = tx.closed() => return,
                _ = tokio::time::sleep(next) => {}
                _ = task_refresh.notified() => {
                    tokio::select! {
                        _ = tx.closed() => return,
                        _ = tokio::time::sleep_until(fetched_at + POLICY_REFRESH_DEBOUNCE) => {}
                    }
                }
            }
        }
    });
    (rx, refresh, handle)
}

/// A per-agent share the relay advertised in a rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObserverShare {
    pub frames_per_min: Option<u64>,
    pub bytes_per_min: Option<u64>,
}

/// Parse `share frames_per_min=F bytes_per_min=B` from a relay rejection.
/// Either value may be missing; `None` when neither is present.
pub(crate) fn parse_share(message: &str) -> Option<ObserverShare> {
    let (_, tail) = message.split_once("share")?;
    let value = |key: &str| {
        let after = tail.split_once(key)?.1;
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        digits.parse::<u64>().ok()
    };
    let share = ObserverShare {
        frames_per_min: value("frames_per_min="),
        bytes_per_min: value("bytes_per_min="),
    };
    (share.frames_per_min.is_some() || share.bytes_per_min.is_some()).then_some(share)
}

/// Limits in force for the next frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectiveLimits {
    pub frames_per_min: usize,
    pub burst_per_sec: usize,
    pub bytes_per_min: usize,
    pub frame_cap: usize,
}

impl EffectiveLimits {
    /// The policy's limits narrowed by an adopted share.
    pub(crate) fn new(policy: &ObserverPolicy, share: Option<ObserverShare>) -> Self {
        let to_usize = |value: u64| usize::try_from(value).unwrap_or(usize::MAX);
        let limits = policy.executor;
        let frames = share
            .and_then(|share| share.frames_per_min)
            .map_or(limits.agent_frames_per_min, |f| {
                f.min(limits.agent_frames_per_min)
            });
        let bytes = share
            .and_then(|share| share.bytes_per_min)
            .map_or(limits.agent_bytes_per_min, |b| {
                b.min(limits.agent_bytes_per_min)
            });
        Self {
            frames_per_min: to_usize(frames.max(1)),
            burst_per_sec: to_usize(limits.agent_burst_per_sec.max(1)),
            bytes_per_min: to_usize(bytes.max(1)),
            frame_cap: policy.frame_cap(),
        }
    }
}

/// Sliding-window record of frames sent in the last minute.
#[derive(Default)]
pub(crate) struct SendWindow {
    sent: VecDeque<(Instant, usize)>,
    bytes: usize,
}

impl SendWindow {
    fn prune(&mut self, now: Instant) {
        while let Some(&(at, bytes)) = self.sent.front() {
            if now.duration_since(at) < MINUTE {
                break;
            }
            self.sent.pop_front();
            self.bytes -= bytes;
        }
    }

    /// Record a sent frame of `bytes` plaintext bytes.
    pub(crate) fn record(&mut self, now: Instant, bytes: usize) {
        self.prune(now);
        self.sent.push_back((now, bytes));
        self.bytes += bytes;
    }

    /// Admit a frame needing at least `needed` bytes: `Ok(allowance)` with the
    /// largest frame that fits every limit now, or `Err(at)` with the earliest
    /// instant a frame of `needed` bytes could be admitted.
    pub(crate) fn admit(
        &mut self,
        now: Instant,
        needed: usize,
        limits: &EffectiveLimits,
    ) -> Result<usize, Instant> {
        self.prune(now);
        let in_last_second = self
            .sent
            .iter()
            .rev()
            .take_while(|(at, _)| now.duration_since(*at) < SECOND)
            .count();
        if in_last_second >= limits.burst_per_sec {
            let oldest = self.sent[self.sent.len() - in_last_second].0;
            return Err(oldest + SECOND);
        }
        if self.sent.len() >= limits.frames_per_min {
            let release = self.sent.len() + 1 - limits.frames_per_min;
            return Err(self.sent[release - 1].0 + MINUTE);
        }
        let needed = needed.min(limits.frame_cap).min(limits.bytes_per_min);
        let available = limits.bytes_per_min.saturating_sub(self.bytes);
        if available < needed {
            // Wait until enough of the oldest frames age out.
            let mut freed = available;
            for &(at, bytes) in &self.sent {
                freed += bytes;
                if freed >= needed {
                    return Err(at + MINUTE);
                }
            }
        }
        Ok(available.min(limits.frame_cap))
    }

    #[cfg(test)]
    pub(crate) fn frames(&self) -> usize {
        self.sent.len()
    }
}

/// A relay-advertised share, adopted until it expires or the policy changes.
#[derive(Default)]
pub(crate) struct AdoptedShare(Option<(ObserverShare, Instant)>);

impl AdoptedShare {
    pub(crate) fn adopt(&mut self, share: ObserverShare, now: Instant) {
        self.0 = Some((share, now + SHARE_TTL));
    }

    pub(crate) fn clear(&mut self) {
        self.0 = None;
    }

    pub(crate) fn current(&mut self, now: Instant) -> Option<ObserverShare> {
        match self.0 {
            Some((share, until)) if now < until => Some(share),
            _ => {
                self.0 = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy_json(tier: &str, frame_max: u64) -> serde_json::Value {
        let limits = json!({
            "tick_ms": 1000, "agent_frames_per_min": 60, "agent_burst_per_sec": 5,
            "agent_frame_max_bytes": frame_max, "agent_bytes_per_min": 400000,
            "account_agents": 50, "account_frames_per_min": 1500,
            "account_burst_per_sec": 50, "account_bytes_per_min": 4000000,
            "account_bytes_per_day": 1000000000u64, "account_typing_agents": 50
        });
        json!({
            "tier": tier, "headroom_pct": 150, "executor": limits,
            "server": limits, "conn_frames_per_sec": 30
        })
    }

    fn limits(frames: usize, burst: usize, bytes: usize, cap: usize) -> EffectiveLimits {
        EffectiveLimits {
            frames_per_min: frames,
            burst_per_sec: burst,
            bytes_per_min: bytes,
            frame_cap: cap,
        }
    }

    #[test]
    fn parses_relay_policy_response() {
        let policy = ObserverPolicy::parse(&policy_json("standard", 16_384)).unwrap();
        assert_eq!(policy.tier, ObserverTier::Standard);
        assert_eq!(policy.frame_cap(), 16_384);
        assert_eq!(policy.tick(), Duration::from_secs(1));
        assert!(ObserverPolicy::parse(&policy_json("gold", 1)).is_none());
        assert!(ObserverPolicy::parse(&json!({"tier": "free"})).is_none());
    }

    #[test]
    fn frame_cap_never_exceeds_nip44_plaintext_cap() {
        let policy = ObserverPolicy::parse(&policy_json("premium", 10_000_000)).unwrap();
        assert_eq!(policy.frame_cap(), OBSERVER_MAX_PLAINTEXT_LEN);
    }

    #[test]
    fn initial_policy_prefers_fetched_then_persisted_then_free() {
        let fetched = ObserverPolicy::parse(&policy_json("premium", 65_535)).unwrap();
        let persisted = ObserverPolicy::parse(&policy_json("standard", 16_384)).unwrap();
        assert_eq!(
            initial_policy(Some(fetched), Some(persisted)),
            (fetched, PolicySource::Fetched)
        );
        assert_eq!(
            initial_policy(None, Some(persisted)),
            (persisted, PolicySource::Persisted)
        );
        assert_eq!(
            initial_policy(None, None),
            (ObserverPolicy::FREE, PolicySource::Default)
        );
        assert_eq!(ObserverPolicy::FREE.tick(), Duration::from_millis(1_500));
        assert_eq!(ObserverPolicy::FREE.frame_cap(), 4_096);
    }

    #[test]
    fn persisted_policy_round_trips_and_bad_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = policy_path(dir.path(), "AB12", "wss://relay.test/");
        assert_eq!(path.parent().unwrap(), dir.path().join("ab12"));
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("observer-policy-"));
        assert_eq!(load_persisted(&path), None, "missing file");

        let raw = policy_json("standard", 16_384);
        persist(&path, &raw).unwrap();
        assert_eq!(load_persisted(&path), ObserverPolicy::parse(&raw));
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored, raw, "the full response is persisted verbatim");

        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load_persisted(&path), None, "corrupt file");
    }

    #[tokio::test(start_paused = true)]
    async fn policy_task_persists_fetches_and_debounces_refresh_requests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent").join("observer-policy.json");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fetch_calls = Arc::clone(&calls);
        let (mut rx, refresh, task) = spawn_policy_task(
            move || {
                let n = fetch_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    if n == 0 {
                        Ok(policy_json("standard", 16_384))
                    } else {
                        Ok(policy_json("premium", 65_535))
                    }
                }
            },
            Some(path.clone()),
        );
        rx.wait_for(Option::is_some).await.unwrap();
        assert_eq!(rx.borrow().unwrap().tier, ObserverTier::Standard);
        assert_eq!(load_persisted(&path).unwrap().tier, ObserverTier::Standard);

        // A refresh request right after a fetch waits out the debounce.
        refresh.notify_one();
        tokio::time::sleep(Duration::from_secs(29)).await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        tokio::time::sleep(Duration::from_secs(2)).await;
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow().unwrap().tier, ObserverTier::Premium);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);

        drop(rx);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(task.is_finished(), "task exits once the publisher is gone");
    }

    #[tokio::test(start_paused = true)]
    async fn failed_fetch_publishes_nothing_and_retries_within_a_minute() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fetch_calls = Arc::clone(&calls);
        let (rx, _refresh, task) = spawn_policy_task(
            move || {
                fetch_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async { Err::<serde_json::Value, _>("HTTP 503".to_string()) }
            },
            None,
        );
        tokio::time::sleep(Duration::from_secs(61)).await;
        assert!(rx.borrow().is_none(), "failures never publish a policy");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        task.abort();
    }

    #[test]
    fn share_parsing_is_robust() {
        let message = "rate-limited: observer agent frames per minute exceeded (tier free); \
                       retry in 12s; share frames_per_min=20 bytes_per_min=33333";
        assert_eq!(
            parse_share(message),
            Some(ObserverShare {
                frames_per_min: Some(20),
                bytes_per_min: Some(33_333)
            })
        );
        assert_eq!(
            parse_share("rate-limited: x; share bytes_per_min=7"),
            Some(ObserverShare {
                frames_per_min: None,
                bytes_per_min: Some(7)
            })
        );
        assert_eq!(
            parse_share("rate-limited: observer quota unavailable; retry in 5s"),
            None
        );
        assert_eq!(parse_share("share frames_per_min=abc"), None);
        // Keys before `share` are not the share.
        assert_eq!(parse_share("frames_per_min=1 exceeded"), None);
    }

    #[test]
    fn share_narrows_but_never_widens_limits() {
        let policy = ObserverPolicy::FREE;
        let narrowed = EffectiveLimits::new(
            &policy,
            Some(ObserverShare {
                frames_per_min: Some(10),
                bytes_per_min: Some(1_000_000),
            }),
        );
        assert_eq!(narrowed.frames_per_min, 10);
        assert_eq!(
            narrowed.bytes_per_min, 100_000,
            "share above own limit is ignored"
        );
        assert_eq!(EffectiveLimits::new(&policy, None).frames_per_min, 60);
    }

    #[tokio::test(start_paused = true)]
    async fn adopted_share_expires_after_a_minute() {
        let mut share = AdoptedShare::default();
        let value = ObserverShare {
            frames_per_min: Some(5),
            bytes_per_min: None,
        };
        share.adopt(value, Instant::now());
        assert_eq!(share.current(Instant::now()), Some(value));
        tokio::time::advance(SHARE_TTL).await;
        assert_eq!(share.current(Instant::now()), None);
    }

    #[tokio::test(start_paused = true)]
    async fn window_caps_burst_per_second() {
        let mut window = SendWindow::default();
        let limits = limits(60, 5, 1_000_000, 4_096);
        let start = Instant::now();
        let mut sent_in_first_second = 0;
        let mut now = start;
        while now < start + SECOND {
            match window.admit(now, 100, &limits) {
                Ok(_) => {
                    window.record(now, 100);
                    sent_in_first_second += 1;
                    now += Duration::from_millis(10);
                }
                Err(at) => now = at,
            }
        }
        assert_eq!(sent_in_first_second, 5, "at most 5 frames in any second");
    }

    #[tokio::test(start_paused = true)]
    async fn window_caps_frames_per_minute() {
        let mut window = SendWindow::default();
        let limits = limits(60, 5, 10_000_000, 4_096);
        let start = Instant::now();
        let mut now = start;
        for _ in 0..60 {
            let admitted = window.admit(now, 100, &limits);
            assert!(admitted.is_ok());
            window.record(now, 100);
            now += Duration::from_millis(250);
        }
        let Err(retry_at) = window.admit(now, 100, &limits) else {
            panic!("61st frame within a minute must wait");
        };
        assert_eq!(retry_at, start + MINUTE, "waits for the oldest to age out");
        assert!(window.admit(retry_at, 100, &limits).is_ok());
        assert_eq!(window.frames(), 59);
    }

    #[tokio::test(start_paused = true)]
    async fn window_caps_bytes_per_minute_and_bounds_the_allowance() {
        let mut window = SendWindow::default();
        let limits = limits(60, 5, 10_000, 4_096);
        let start = Instant::now();
        assert_eq!(window.admit(start, 100, &limits), Ok(4_096), "frame cap");
        window.record(start, 4_000);
        window.record(start + SECOND, 4_000);
        // 2,000 bytes left: a small frame fits, the allowance shrinks to it.
        let now = start + 2 * SECOND;
        assert_eq!(window.admit(now, 500, &limits), Ok(2_000));
        // A frame needing 3,000 bytes waits until the first 4,000 age out.
        assert_eq!(window.admit(now, 3_000, &limits), Err(start + MINUTE));
    }
}
