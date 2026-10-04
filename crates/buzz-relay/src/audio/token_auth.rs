//! Huddle audio token authentication (centralized identity, plan §3.4).
//!
//! With `AUTH_TOKEN_ENABLED=true` the audio route accepts
//! `{"type":"auth","token":"bz…"}` beside the NIP-42 `event` form. The
//! admitted socket is bound to its token exactly like a root token
//! connection: it is registered in the same [`TokenSessionRegistry`] (so a
//! revocation published for the token closes it), watched for the token's
//! deadline, and may swap to a fresh token of the same principal with a
//! further `{"type":"auth","token":…}` frame (`auth_ok` / `auth_error`
//! replies) so a long huddle survives the 1 h access-token lifetime.
//!
//! [`TokenSessionRegistry`]: crate::identity::ws::TokenSessionRegistry

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::ws::Message as WsMessage;
use buzz_auth::{TokenBinding, TokenSecret};
use buzz_core::principal::PrincipalId;
use chrono::Utc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::handlers::auth::{community_ban_outcome, BanOutcome};
use crate::identity::verify_access_token;
use crate::identity::ws::{run_binding_watch, TokenSession};
use crate::state::AppState;

/// Close frame for a token-bound audio socket (the audio protocol's JSON
/// `error` shape, which clients already treat as terminal).
pub(crate) fn audio_close_frame(notice: &str) -> WsMessage {
    error_frame(notice)
}

/// `{"type":"error","message":…}`.
pub(crate) fn error_frame(message: &str) -> WsMessage {
    WsMessage::Text(
        serde_json::json!({ "type": "error", "message": message })
            .to_string()
            .into(),
    )
}

/// Verify a presented audio token. `Err` carries the client-visible message.
pub(crate) async fn verify(state: &AppState, token: &str) -> Result<TokenBinding, String> {
    verify_access_token(state, &TokenSecret::new(token.to_owned()))
        .await
        .map_err(|rejection| format!("auth-required: {}", rejection.code()))
}

/// The root token route's ban gate: the principal, then (bots) its owner,
/// whose ban cascades. `Some((class, message))` refuses admission.
pub(crate) async fn ban_refusal(
    state: &AppState,
    community: buzz_core::CommunityId,
    binding: &TokenBinding,
) -> Option<(buzz_auth::DenialClass, &'static str)> {
    let subjects = std::iter::once(binding.principal)
        .chain(binding.bot_owner)
        .map(|principal| principal.as_public_key());
    for subject in subjects {
        match community_ban_outcome(state, community, subject, None, None).await {
            BanOutcome::Clear => {}
            BanOutcome::Banned => {
                return Some((
                    buzz_auth::DenialClass::AuthorizationDenied,
                    "blocked: you are banned from this community",
                ))
            }
            BanOutcome::DbError => {
                return Some((
                    buzz_auth::DenialClass::AuthorizationUnavailable,
                    "error: internal error checking restriction state",
                ))
            }
        }
    }
    None
}

/// The live token binding of one admitted audio socket. Dropping it forgets
/// the socket in the revocation registry and stops its deadline watch.
pub(crate) struct AudioTokenBinding {
    conn_id: Uuid,
    principal: PrincipalId,
    session: Arc<TokenSession>,
    state: Arc<AppState>,
    watch: tokio::task::JoinHandle<()>,
    reauth_in_flight: Arc<AtomicBool>,
}

impl AudioTokenBinding {
    /// Register `binding` for the socket closed through `cancel` (after its
    /// close frame is queued on `ctrl_tx`), then re-read the token row: a
    /// revocation that committed between verification and registration was
    /// not delivered to this socket, so it is closed now (bind-then-check).
    /// The caller checks `cancel` afterwards.
    pub(crate) async fn bind(
        state: &Arc<AppState>,
        binding: &TokenBinding,
        cancel: CancellationToken,
        ctrl_tx: mpsc::Sender<WsMessage>,
    ) -> Self {
        let session = Arc::new(TokenSession::new(
            binding.token_hash,
            binding.expires_at,
            cancel,
            ctrl_tx,
            audio_close_frame,
        ));
        let conn_id = Uuid::new_v4();
        state
            .identity
            .sessions
            .insert(conn_id, Arc::clone(&session));
        let watch = tokio::spawn(run_binding_watch(Arc::clone(&session), state.db.clone()));
        let bound = Self {
            conn_id,
            principal: binding.principal,
            session,
            state: Arc::clone(state),
            watch,
            reauth_in_flight: Arc::new(AtomicBool::new(false)),
        };
        if !token_row_is_live(state, &bound.session.bound_hash()).await {
            bound.session.close("auth-revoked: token_revoked");
        }
        bound
    }

    /// Handle a post-admission `{"type":"auth","token":…}` frame: swap the
    /// binding when the token is valid and names the same principal (reply
    /// `auth_ok`), keep the old binding on an invalid token (reply
    /// `auth_error`), close on a different principal. One swap at a time;
    /// a frame arriving while one is in flight gets `auth_error`.
    pub(crate) fn reauth(&self, token: String, ctrl_tx: mpsc::Sender<WsMessage>) {
        if self.reauth_in_flight.swap(true, Ordering::AcqRel) {
            let _ = ctrl_tx.try_send(auth_error_frame("auth-required: reauth in progress"));
            return;
        }
        let in_flight = Arc::clone(&self.reauth_in_flight);
        let session = Arc::clone(&self.session);
        let state = Arc::clone(&self.state);
        let principal = self.principal;
        tokio::spawn(async move {
            let reply = reauth_once(&state, &session, principal, &token).await;
            if let Some(reply) = reply {
                let _ = ctrl_tx.try_send(reply);
            }
            in_flight.store(false, Ordering::Release);
        });
    }
}

impl Drop for AudioTokenBinding {
    fn drop(&mut self) {
        self.state.identity.sessions.remove(self.conn_id);
        self.watch.abort();
    }
}

fn auth_error_frame(message: &str) -> WsMessage {
    WsMessage::Text(
        serde_json::json!({ "type": "auth_error", "message": message })
            .to_string()
            .into(),
    )
}

async fn token_row_is_live(state: &AppState, hash: &[u8; 32]) -> bool {
    matches!(
        state.db.lookup_access_token(hash).await,
        Ok(Some(record)) if record.check(Utc::now()).is_ok()
    )
}

/// One re-AUTH; returns the reply frame (`None` when the socket was closed).
async fn reauth_once(
    state: &AppState,
    session: &TokenSession,
    principal: PrincipalId,
    token: &str,
) -> Option<WsMessage> {
    let binding = match verify(state, token).await {
        Ok(binding) => binding,
        Err(message) => return Some(auth_error_frame(&message)),
    };
    if binding.principal != principal {
        metrics::counter!("buzz_auth_failures_total", "reason" => "audio_reauth_principal_mismatch")
            .increment(1);
        session.close("auth-required: principal mismatch");
        return None;
    }
    session.rebind(binding.token_hash, binding.expires_at);
    // Bind-then-check, as on the root route.
    if !token_row_is_live(state, &binding.token_hash).await {
        session.close("auth-revoked: token_revoked");
        return None;
    }
    Some(WsMessage::Text(
        serde_json::json!({ "type": "auth_ok" }).to_string().into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One re-AUTH at a time: while a swap is verifying (here: stuck on an
    /// unreachable token store), a second frame is answered `auth_error`
    /// immediately instead of starting a parallel swap.
    #[tokio::test]
    async fn reauth_is_single_flight() {
        let state = crate::state::tests::test_state_with_database_url(
            "postgres://buzz:buzz_dev@127.0.0.1:1/buzz",
        ) // sadscan:disable np.postgres.1 -- closed port, never connects
        .await;
        let (ctrl_tx, mut ctrl_rx) = mpsc::channel(8);
        let session = Arc::new(TokenSession::new(
            [7u8; 32],
            None,
            CancellationToken::new(),
            ctrl_tx.clone(),
            audio_close_frame,
        ));
        let binding = AudioTokenBinding {
            conn_id: Uuid::new_v4(),
            principal: PrincipalId::generate(),
            session,
            state: Arc::clone(&state),
            watch: tokio::spawn(async {}),
            reauth_in_flight: Arc::new(AtomicBool::new(false)),
        };
        let (token, _) = buzz_auth::generate_token(buzz_auth::TokenKind::UserAccess);
        binding.reauth(token.expose().to_owned(), ctrl_tx.clone());
        binding.reauth(token.expose().to_owned(), ctrl_tx.clone());
        let reply = tokio::time::timeout(std::time::Duration::from_millis(500), ctrl_rx.recv())
            .await
            .expect("second re-AUTH answered at once")
            .expect("frame");
        let WsMessage::Text(text) = reply else {
            panic!("text frame expected");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["type"], "auth_error");
        assert_eq!(value["message"], "auth-required: reauth in progress");
    }
}
