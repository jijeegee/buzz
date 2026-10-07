//! Cancellation stops browser acquisition only, before account storage starts.

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;

pub(crate) const CANCELLED: &str = "Google sign-in cancelled.";

struct Pending {
    id: String,
    cancel: watch::Sender<bool>,
    completing: bool,
}

#[derive(Default)]
pub(crate) struct LoginControl(Arc<Mutex<Option<Pending>>>);

pub(crate) struct LoginAttempt {
    control: Arc<Mutex<Option<Pending>>>,
    id: String,
    cancelled: watch::Receiver<bool>,
}

impl LoginControl {
    pub(crate) fn begin(&self, id: String) -> Result<LoginAttempt, String> {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return Err("a Google sign-in is already in progress".into());
        }
        let (cancel, cancelled) = watch::channel(false);
        *slot = Some(Pending {
            id: id.clone(),
            cancel,
            completing: false,
        });
        Ok(LoginAttempt {
            control: self.0.clone(),
            id,
            cancelled,
        })
    }

    pub(crate) fn cancel(&self, id: &str) -> bool {
        let slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(pending) = slot.as_ref().filter(|p| p.id == id && !p.completing) else {
            return false;
        };
        pending.cancel.send_replace(true);
        true
    }
}

impl LoginAttempt {
    /// Only a matching, uncancelled attempt can leave the browser phase.
    pub(crate) async fn wait<T>(
        &mut self,
        browser: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        let result = tokio::select! {
            biased;
            _ = self.cancelled.wait_for(|cancelled| *cancelled) => return Err(CANCELLED.into()),
            result = browser => result?,
        };
        let mut slot = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(pending) = slot.as_mut().filter(|p| p.id == self.id) else {
            return Err(CANCELLED.into());
        };
        if *pending.cancel.borrow() {
            return Err(CANCELLED.into());
        }
        // Cancellation and the transition to account processing share this lock.
        pending.completing = true;
        Ok(result)
    }
}

impl Drop for LoginAttempt {
    fn drop(&mut self) {
        let mut slot = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|p| p.id == self.id) {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::loopback::{LoopbackListener, LOGIN_TIMEOUT};

    #[tokio::test]
    async fn cancel_closes_listener_and_releases_slot_without_a_callback() {
        let control = LoginControl::default();
        let mut attempt = control.begin("first".into()).unwrap();
        assert!(control.begin("duplicate".into()).is_err());
        let listener = LoopbackListener::bind().await.unwrap();
        let uri = url::Url::parse(&listener.redirect_uri()).unwrap();
        assert!(!control.cancel("other"));
        assert!(control.cancel("first"));
        let result = attempt
            .wait(async move {
                listener
                    .wait_for_code("state", LOGIN_TIMEOUT)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", uri.port().unwrap()))
                .await
                .is_err()
        );
        drop(attempt);
        let mut retry = control.begin("second".into()).unwrap();
        assert!(!control.cancel("first"));
        assert_eq!(retry.wait(async { Ok(42) }).await, Ok(42));
        assert!(
            !control.cancel("second"),
            "cannot interrupt account persistence"
        );
    }

    #[tokio::test]
    async fn cancellation_wins_over_a_ready_callback_before_completion() {
        let control = LoginControl::default();
        let mut attempt = control.begin("attempt".into()).unwrap();
        control.cancel("attempt");
        assert_eq!(attempt.wait(async { Ok(42) }).await, Err(CANCELLED.into()));
    }

    #[tokio::test]
    async fn failure_and_dropped_tasks_release_the_slot() {
        let control = LoginControl::default();
        let mut attempt = control.begin("attempt".into()).unwrap();
        assert_eq!(
            attempt.wait(async { Err::<(), _>("failed".into()) }).await,
            Err("failed".into())
        );
        drop(attempt);
        drop(control.begin("retry".into()).unwrap());
        assert!(control.begin("again".into()).is_ok());
    }
}
