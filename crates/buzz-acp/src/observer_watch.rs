//! The owner's "watching" signal for relay observer telemetry.
//!
//! Desktop and mobile subscribe to observer frames whenever they run, so a
//! relay-side subscriber count says nothing about whether anyone is looking.
//! Instead, an app showing a live activity view sends a `watching` control
//! frame every 30 s. For [`WATCHING_WINDOW`] after the last one the publisher
//! sends the owner's tier-level detail; outside it, every tier sends the free
//! summary. The window is agent-wide (any channel) and in memory only, so an
//! executor restart starts with it closed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use crate::observer_policy::ObserverTier;

/// How long one `watching` frame keeps tier-level detail on. Apps refresh
/// every 30 s, so one lost frame does not close the window.
pub(crate) const WATCHING_WINDOW: Duration = Duration::from_secs(60);

/// Shared deadline of the watching window, written by the control-frame
/// handler and read by the observer publisher.
#[derive(Debug, Clone, Default)]
pub(crate) struct ObserverWatch {
    until: Arc<Mutex<Option<Instant>>>,
}

impl ObserverWatch {
    /// A window that never closes, for publisher tests about tier behavior.
    #[cfg(test)]
    pub(crate) fn pinned_open() -> Self {
        let watch = Self::default();
        *watch.lock() = Some(Instant::now() + Duration::from_secs(100 * 365 * 24 * 3600));
        watch
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        // The guarded value is a plain deadline; a poisoned lock still holds
        // a valid one.
        self.until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// (Re)open the window for [`WATCHING_WINDOW`] from `now`.
    pub(crate) fn open(&self, now: Instant) {
        let until = now + WATCHING_WINDOW;
        let mut guard = self.lock();
        *guard = Some(guard.map_or(until, |current| current.max(until)));
    }

    /// Whether someone watched within the last [`WATCHING_WINDOW`].
    pub(crate) fn is_watching(&self, now: Instant) -> bool {
        self.lock().is_some_and(|until| now < until)
    }

    /// The detail level to summarise an event at: the owner's tier while
    /// watched, free otherwise.
    pub(crate) fn effective_tier(&self, tier: ObserverTier, now: Instant) -> ObserverTier {
        if self.is_watching(now) {
            tier
        } else {
            ObserverTier::Free
        }
    }
}

/// Apply a decrypted `watching` control payload. Returns whether the window
/// was opened. `channelId` names the viewed channel; it is informational
/// (the window is agent-wide) but must be a string when present.
pub(crate) fn apply_watching_control(
    payload: &serde_json::Value,
    watch: &ObserverWatch,
    now: Instant,
) -> bool {
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("watching") {
        return false;
    }
    if payload
        .get("channelId")
        .is_some_and(|channel| !channel.is_string())
    {
        tracing::warn!("watching control frame has a non-string channelId; ignoring");
        return false;
    }
    watch.open(now);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test(start_paused = true)]
    async fn window_opens_for_sixty_seconds_and_reopens() {
        let watch = ObserverWatch::default();
        let start = Instant::now();
        assert!(!watch.is_watching(start), "a fresh executor starts closed");
        assert_eq!(
            watch.effective_tier(ObserverTier::Premium, start),
            ObserverTier::Free
        );

        assert!(apply_watching_control(
            &json!({"type": "watching", "channelId": "c"}),
            &watch,
            start
        ));
        let open = start + Duration::from_secs(59);
        assert_eq!(
            watch.effective_tier(ObserverTier::Premium, open),
            ObserverTier::Premium
        );
        assert_eq!(
            watch.effective_tier(ObserverTier::Standard, open),
            ObserverTier::Standard
        );
        let expired = start + WATCHING_WINDOW;
        assert!(!watch.is_watching(expired));
        assert_eq!(
            watch.effective_tier(ObserverTier::Premium, expired),
            ObserverTier::Free
        );

        watch.open(expired);
        assert!(watch.is_watching(expired + Duration::from_secs(30)));
    }

    #[tokio::test(start_paused = true)]
    async fn an_older_frame_never_shortens_the_window() {
        let watch = ObserverWatch::default();
        let start = Instant::now();
        watch.open(start + Duration::from_secs(30));
        watch.open(start);
        assert!(watch.is_watching(start + Duration::from_secs(80)));
    }

    #[tokio::test(start_paused = true)]
    async fn malformed_or_other_payloads_do_not_open_the_window() {
        let watch = ObserverWatch::default();
        let now = Instant::now();
        for payload in [
            json!({"type": "watching", "channelId": 7}),
            json!({"type": "Watching", "channelId": "c"}),
            json!({"type": "cancel_turn"}),
            json!({"channelId": "c"}),
            json!("watching"),
        ] {
            assert!(!apply_watching_control(&payload, &watch, now), "{payload}");
        }
        assert!(!watch.is_watching(now));
        // channelId is informational: the window is agent-wide.
        assert!(apply_watching_control(
            &json!({"type": "watching"}),
            &watch,
            now
        ));
        assert!(watch.is_watching(now));
    }

    #[tokio::test(start_paused = true)]
    async fn clones_share_one_window() {
        let watch = ObserverWatch::default();
        let publisher_view = watch.clone();
        watch.open(Instant::now());
        assert!(publisher_view.is_watching(Instant::now()));
    }
}
