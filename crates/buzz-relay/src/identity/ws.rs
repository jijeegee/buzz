//! WebSocket token authentication (`["AUTH", {"token": "…"}]`).
//!
//! A token-authenticated connection is bound to exactly one access-token hash
//! ([`TokenSession`]). A same-principal re-AUTH swaps the binding (hash and
//! deadline) without touching subscriptions; a different principal is refused
//! and the connection closed (plan §3.4, B3). One bounded task per connection
//! enforces the bound token's deadline and re-checks the token row every
//! [`RECHECK_INTERVAL`]; it exits when the connection is cancelled.
//!
//! Revocation fan-out ([`TokenSessionRegistry::apply`]) matches the
//! *currently bound* hash only, so a connection that already re-AUTHed onto a
//! new token is unaffected by the old token's revocation.

use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use axum::extract::ws::Message as WsMessage;
use buzz_auth::{AuthContext, AuthMethod, TokenSecret};
use buzz_core::draft::StampError;
use buzz_core::principal::PrincipalId;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use super::{verify_access_token, TokenRejection};
use crate::connection::{AuthState, ConnectionState};
use crate::handlers::auth::{
    community_ban_outcome, final_admission_denial, AdmissionDenial, BanOutcome,
};
use crate::metrics::{AuthOutcome, AuthPostTerminalState};
use crate::protocol::RelayMessage;
use crate::state::AppState;

/// How often a bound connection re-reads its token row (revocation backstop
/// when a Redis revocation message is missed).
pub(crate) const RECHECK_INTERVAL: Duration = Duration::from_secs(600);

/// `OK` label for token AUTH replies.
pub(crate) const AUTH_OK_ID: &str = "auth";

/// Rejection for a client-published kind 0 on a token connection.
pub(crate) const PROFILE_MANAGED_MESSAGE: &str = "blocked: profile is managed via /auth/profile";

/// Rejection for a draft whose `pubkey` names someone else (unchanged text).
pub(crate) const PUBKEY_MISMATCH_MESSAGE: &str =
    "invalid: event pubkey does not match authenticated identity";

#[derive(Debug, Clone, Copy)]
struct Bound {
    hash: [u8; 32],
    generation: u64,
    deadline: Option<DateTime<Utc>>,
}

/// Builds the frame a [`TokenSession`] queues before closing its socket.
pub(crate) type CloseFrame = fn(&str) -> WsMessage;

/// The root relay route's close frame: `["NOTICE", …]`.
fn notice_frame(notice: &str) -> WsMessage {
    WsMessage::Text(RelayMessage::notice(notice).into())
}

/// The token binding of one live connection.
pub(crate) struct TokenSession {
    bound: StdMutex<Bound>,
    wake: Notify,
    cancel: CancellationToken,
    ctrl_tx: mpsc::Sender<WsMessage>,
    close_frame: CloseFrame,
}

impl TokenSession {
    /// A binding to `hash` (expiring at `deadline`) for a socket that is
    /// closed by queueing `close_frame` on `ctrl_tx` and cancelling `cancel`.
    pub(crate) fn new(
        hash: [u8; 32],
        deadline: Option<DateTime<Utc>>,
        cancel: CancellationToken,
        ctrl_tx: mpsc::Sender<WsMessage>,
        close_frame: CloseFrame,
    ) -> Self {
        Self {
            bound: StdMutex::new(Bound {
                hash,
                deadline,
                generation: 0,
            }),
            wake: Notify::new(),
            cancel,
            ctrl_tx,
            close_frame,
        }
    }

    fn current(&self) -> Bound {
        *self.bound.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The currently bound token hash.
    pub(crate) fn bound_hash(&self) -> [u8; 32] {
        self.current().hash
    }

    /// Swap the binding to a new token (same-principal re-AUTH).
    pub(crate) fn rebind(&self, hash: [u8; 32], deadline: Option<DateTime<Utc>>) {
        let mut bound = self.bound.lock().unwrap_or_else(PoisonError::into_inner);
        *bound = Bound {
            hash,
            deadline,
            generation: bound.generation.wrapping_add(1),
        };
        drop(bound);
        self.wake.notify_one();
    }

    /// Lower the deadline when still bound to `hash`.
    fn lower_deadline(&self, hash: &[u8; 32], not_after: DateTime<Utc>) -> bool {
        let mut bound = self.bound.lock().unwrap_or_else(PoisonError::into_inner);
        if bound.hash != *hash {
            return false;
        }
        bound.deadline = Some(bound.deadline.map_or(not_after, |d| d.min(not_after)));
        drop(bound);
        self.wake.notify_one();
        true
    }

    // The match and cancellation are one critical section with rebind.
    fn close_if_current(&self, observed: Bound, notice: &str) -> bool {
        let bound = self.bound.lock().unwrap_or_else(PoisonError::into_inner);
        if bound.hash != observed.hash || bound.generation != observed.generation {
            return false;
        }
        self.close(notice);
        true
    }

    fn close_if_hash(&self, hash: &[u8; 32], notice: &str) -> bool {
        let bound = self.bound.lock().unwrap_or_else(PoisonError::into_inner);
        if &bound.hash != hash {
            return false;
        }
        self.close(notice);
        true
    }

    /// Queue the route's close frame for `notice` on the control channel
    /// (drained before the close), then cancel the connection.
    pub(crate) fn close(&self, notice: &str) {
        let _ = self.ctrl_tx.try_send((self.close_frame)(notice));
        self.cancel.cancel();
    }
}

/// Token-authenticated connections on this instance, keyed by connection id.
#[derive(Default)]
pub(crate) struct TokenSessionRegistry {
    sessions: DashMap<Uuid, Arc<TokenSession>>,
}

impl TokenSessionRegistry {
    /// Track `session` under `conn_id` so revocations reach it.
    pub(crate) fn insert(&self, conn_id: Uuid, session: Arc<TokenSession>) {
        self.sessions.insert(conn_id, session);
    }

    /// Forget a connection (called from connection teardown).
    pub(crate) fn remove(&self, conn_id: Uuid) {
        self.sessions.remove(&conn_id);
    }

    /// Close (or, with `not_after`, lower the deadline of) every connection
    /// currently bound to one of `hashes`.
    pub(crate) fn apply(&self, hashes: &[[u8; 32]], reason: &str, not_after: Option<i64>) -> usize {
        if hashes.is_empty() {
            return 0;
        }
        let not_after = not_after.and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0));
        let mut affected = 0;
        for entry in self.sessions.iter() {
            let bound = entry.current();
            if !hashes.contains(&bound.hash) {
                continue;
            }
            match not_after {
                Some(deadline) => {
                    if entry.lower_deadline(&bound.hash, deadline) {
                        affected += 1;
                    }
                }
                None => {
                    if entry.close_if_hash(&bound.hash, &format!("auth-revoked: {reason}")) {
                        affected += 1;
                    }
                }
            }
        }
        affected
    }
}

/// Deadline + periodic-recheck loop for one connection. Exits on cancel.
pub(crate) async fn run_binding_watch(session: Arc<TokenSession>, db: buzz_db::Db) {
    loop {
        let bound = session.current();
        let now = Utc::now();
        let until_deadline = bound
            .deadline
            .map(|deadline| (deadline - now).to_std().unwrap_or(Duration::ZERO));
        let wait = until_deadline.map_or(RECHECK_INTERVAL, |d| d.min(RECHECK_INTERVAL));
        tokio::select! {
            biased;
            _ = session.cancel.cancelled() => return,
            _ = session.wake.notified() => continue,
            _ = tokio::time::sleep(wait) => {}
        }
        // Re-read: a rebind may have landed while sleeping.
        let current = session.current();
        if current.generation != bound.generation {
            continue;
        }
        if current
            .deadline
            .is_some_and(|deadline| deadline <= Utc::now())
        {
            if session.close_if_current(current, "auth-expired: token_expired") {
                return;
            }
            continue;
        }
        if until_deadline.is_none_or(|d| d >= RECHECK_INTERVAL)
            && recheck_binding(&session, current, db.lookup_access_token(&current.hash)).await
        {
            return;
        }
    }
}

// Keep the asynchronous lookup separate from the synchronous binding decision.
async fn recheck_binding(
    session: &TokenSession,
    bound: Bound,
    lookup: impl std::future::Future<
        Output = Result<Option<buzz_db::identity::AccessTokenRecord>, buzz_db::DbError>,
    >,
) -> bool {
    let notice = match lookup.await {
        Ok(Some(record)) => match record.check(Utc::now()) {
            Ok(()) => return false,
            Err(buzz_db::identity::AccessTokenRejection::Expired) => "auth-revoked: token_expired",
            Err(buzz_db::identity::AccessTokenRejection::PrincipalDisabled) => {
                "auth-revoked: principal_disabled"
            }
            Err(_) => "auth-revoked: token_revoked",
        },
        Ok(None) => "auth-revoked: token_revoked",
        Err(error) => {
            warn!(%error, "token binding recheck failed");
            return false;
        }
    };
    session.close_if_current(bound, notice)
}

fn deny(
    conn: &ConnectionState,
    outcome: AuthOutcome,
    metric: &'static str,
    reason: &str,
    close: bool,
) {
    metrics::counter!("buzz_auth_failures_total", "reason" => metric).increment(1);
    if !conn.reject_auth(outcome) {
        return;
    }
    if close {
        let _ = conn.ctrl_tx.try_send(WsMessage::Text(
            RelayMessage::ok(AUTH_OK_ID, false, reason).into(),
        ));
        conn.cancel.cancel();
    } else {
        conn.send(RelayMessage::ok(AUTH_OK_ID, false, reason));
    }
}

fn deny_admission(conn: &ConnectionState, denial: AdmissionDenial) {
    deny(conn, denial.outcome, denial.metric, denial.reason, true);
}

/// Handle `["AUTH", {"token": …}]`. Only reachable when token auth is enabled.
pub(crate) async fn handle_token_auth(
    token: TokenSecret,
    conn: Arc<ConnectionState>,
    state: Arc<AppState>,
) {
    match conn.auth_state_snapshot() {
        AuthState::Pending { .. } => initial_auth(token, conn, state).await,
        AuthState::Authenticated(ctx) if ctx.token.is_some() => {
            reauth(token, ctx, conn, state).await
        }
        AuthState::Authenticated(_) => {
            crate::metrics::record_post_terminal_auth_frame(AuthPostTerminalState::Authenticated);
            conn.send(RelayMessage::ok(
                AUTH_OK_ID,
                false,
                "auth-required: already authenticated",
            ));
        }
        AuthState::Failed => {
            crate::metrics::record_post_terminal_auth_frame(AuthPostTerminalState::Failed);
            conn.send(RelayMessage::ok(
                AUTH_OK_ID,
                false,
                "auth-required: authentication already failed",
            ));
        }
    }
}

async fn initial_auth(token: TokenSecret, conn: Arc<ConnectionState>, state: Arc<AppState>) {
    if conn.nip_fi_assertion.is_some() {
        deny(
            &conn,
            AuthOutcome::Invalid,
            "token_auth_under_nip_fi",
            "auth-required: unsupported auth",
            false,
        );
        return;
    }
    let binding = match verify_access_token(&state, &token).await {
        Ok(binding) => binding,
        Err(rejection) => {
            let (outcome, metric) = match rejection {
                TokenRejection::Unavailable => {
                    (AuthOutcome::RelayMembershipCheckError, "token_lookup_error")
                }
                _ => (AuthOutcome::Invalid, "token_invalid"),
            };
            deny(
                &conn,
                outcome,
                metric,
                &format!("auth-required: {}", rejection.code()),
                false,
            );
            return;
        }
    };
    let community = conn.tenant.community();
    let pubkey = binding.principal.as_public_key();
    let binding_principal = binding.principal;
    let owner = binding.bot_owner.map(|owner| owner.as_public_key());

    // Ban gate: the principal, then (bots) its owner — owner ban cascades.
    for subject in std::iter::once(pubkey).chain(owner) {
        match community_ban_outcome(&state, community, subject, None, None).await {
            BanOutcome::Clear => {}
            BanOutcome::Banned => {
                deny(
                    &conn,
                    AuthOutcome::Banned,
                    "banned",
                    "blocked: you are banned from this community",
                    true,
                );
                return;
            }
            BanOutcome::DbError => {
                deny(
                    &conn,
                    AuthOutcome::BanCheckError,
                    "ban_check_error",
                    "error: internal error checking restriction state",
                    true,
                );
                return;
            }
        }
    }
    match crate::api::relay_members::check_relay_membership(
        &state,
        community,
        pubkey.as_bytes(),
        None,
        None,
    )
    .await
    {
        Ok(crate::api::relay_members::MembershipDecision::Denied) => {
            deny(
                &conn,
                AuthOutcome::NotRelayMember,
                "not_relay_member",
                "restricted: not a relay member",
                false,
            );
            return;
        }
        Ok(_) => {}
        Err(error) => {
            warn!(conn_id = %conn.conn_id, %error, "relay membership lookup failed, denying");
            deny(
                &conn,
                AuthOutcome::RelayMembershipCheckError,
                "relay_membership_check_error",
                "error: internal error checking relay membership",
                false,
            );
            return;
        }
    }

    let _permit = match conn.nip_fi_gate.acquire_effect().await {
        Ok(permit) => permit,
        Err(crate::nip_fi_gate::SessionExpired) => return,
    };
    if let Some(owner) = owner {
        // Same owner link NIP-OA records: lets owner revocation find the bot's
        // sockets and keeps `users.agent_owner_pubkey` populated.
        if !crate::api::relay_members::materialize_nip_oa_owner(
            &state,
            &conn.tenant,
            &pubkey,
            &owner,
        )
        .await
        {
            deny_admission(
                &conn,
                AdmissionDenial {
                    metric: "agent_owner_link_error",
                    reason: crate::handlers::auth::OWNER_LINK_ERROR,
                    outcome: AuthOutcome::RelayMembershipCheckError,
                    class: buzz_auth::DenialClass::AuthorizationUnavailable,
                },
            );
            return;
        }
        state
            .conn_manager
            .set_admitted_owner(conn.conn_id, owner.to_bytes());
    }
    state
        .conn_manager
        .set_authenticated_identity(conn.conn_id, pubkey.to_bytes().to_vec(), None);
    let denial = match final_admission_denial(&state, community, pubkey, None, None).await {
        None if conn.cancel.is_cancelled() => Some(AdmissionDenial {
            metric: "revoked_during_auth",
            reason: "blocked: access revoked",
            outcome: AuthOutcome::Banned,
            class: buzz_auth::DenialClass::AuthorizationDenied,
        }),
        denial => denial,
    };
    if let Some(denial) = denial {
        deny_admission(&conn, denial);
        return;
    }

    let session = Arc::new(TokenSession::new(
        binding.token_hash,
        binding.expires_at,
        conn.cancel.clone(),
        conn.ctrl_tx.clone(),
        notice_frame,
    ));
    let ctx = AuthContext {
        pubkey,
        scopes: buzz_auth::Scope::all_known(),
        channel_ids: None,
        auth_method: AuthMethod::Token,
        agent_owner_pubkey: owner,
        token: Some(Box::new(binding)),
    };
    if !conn.authenticate(ctx) {
        return;
    }
    let bound_hash = session.current().hash;
    state
        .identity
        .sessions
        .insert(conn.conn_id, Arc::clone(&session));
    tokio::spawn(run_binding_watch(Arc::clone(&session), state.db.clone()));
    // Bind-then-check: a revocation that committed between verification and
    // registration was not applied to this connection, so re-read the row now.
    // Any revocation after this point finds the registered binding.
    match state.db.lookup_access_token(&bound_hash).await {
        Ok(Some(record)) if record.check(Utc::now()).is_ok() => {}
        _ => {
            session.close_if_hash(&bound_hash, "auth-revoked: token_revoked");
            return;
        }
    }
    if conn.cancel.is_cancelled() {
        return;
    }
    state.conn_manager.mark_admitted(conn.conn_id);
    info!(conn_id = %conn.conn_id, principal = %pubkey.to_hex(), "token auth successful");
    super::profile::spawn_reconcile(
        &state,
        &conn.tenant,
        binding_principal,
        super::profile::Scope::Member,
    );
    conn.send(RelayMessage::ok(AUTH_OK_ID, true, ""));
}

async fn reauth(
    token: TokenSecret,
    current: AuthContext,
    conn: Arc<ConnectionState>,
    state: Arc<AppState>,
) {
    let binding = match verify_access_token(&state, &token).await {
        Ok(binding) => binding,
        Err(rejection) => {
            // The old binding stays in force; the client decides (acp treats
            // token_* codes as terminal).
            conn.send(RelayMessage::ok(
                AUTH_OK_ID,
                false,
                &format!("auth-required: {}", rejection.code()),
            ));
            return;
        }
    };
    let same_principal = current
        .token
        .as_ref()
        .is_some_and(|bound| bound.principal == binding.principal);
    if !same_principal {
        metrics::counter!("buzz_auth_failures_total", "reason" => "reauth_principal_mismatch")
            .increment(1);
        let _ = conn.ctrl_tx.try_send(WsMessage::Text(
            RelayMessage::ok(AUTH_OK_ID, false, "auth-required: principal mismatch").into(),
        ));
        conn.cancel.cancel();
        return;
    }
    let (hash, deadline, principal) = (binding.token_hash, binding.expires_at, binding.principal);
    if !conn.rebind_token(binding) {
        conn.send(RelayMessage::ok(
            AUTH_OK_ID,
            false,
            "auth-required: not authenticated",
        ));
        return;
    }
    let session = state
        .identity
        .sessions
        .sessions
        .get(&conn.conn_id)
        .map(|entry| Arc::clone(entry.value()));
    if let Some(session) = &session {
        session.rebind(hash, deadline);
    }
    // Bind-then-check, as in `initial_auth`: a revocation of the new token
    // that committed between verification and the rebind was published for a
    // hash this connection did not hold yet, so re-read the row now.
    match state.db.lookup_access_token(&hash).await {
        Ok(Some(record)) if record.check(Utc::now()).is_ok() => {}
        _ => {
            match &session {
                Some(session) => {
                    session.close_if_hash(&hash, "auth-revoked: token_revoked");
                }
                None => conn.cancel.cancel(),
            }
            return;
        }
    }
    conn.send(RelayMessage::ok(AUTH_OK_ID, true, ""));
    // A re-AUTH (≤ 1 h apart) also repairs a kind:0 publish that failed, so a
    // long-lived connection converges without reconnecting (Rule 1).
    super::profile::spawn_reconcile(
        &state,
        &conn.tenant,
        principal,
        super::profile::Scope::Member,
    );
}

/// Stamp a draft for `principal`, applying the token-connection rules: a
/// foreign `pubkey` keeps today's rejection text and client kind 0 is refused.
/// On rejection returns `(id for the OK frame, message)`.
pub(crate) fn stamp_for_principal(
    draft: &serde_json::Value,
    principal: &PrincipalId,
) -> Result<nostr::Event, (String, String)> {
    let fallback_id = || {
        draft
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
            .unwrap_or_default()
            .to_owned()
    };
    match buzz_core::draft::stamp_draft(draft, principal, Utc::now().timestamp()) {
        Ok(event) if event.kind.as_u16() == 0 => {
            Err((event.id.to_hex(), PROFILE_MANAGED_MESSAGE.to_owned()))
        }
        Ok(event) => Ok(event),
        Err(StampError::PubkeyMismatch { claimed_id }) => Err((
            claimed_id.unwrap_or_else(fallback_id),
            PUBKEY_MISMATCH_MESSAGE.to_owned(),
        )),
        Err(StampError::Invalid(message)) => Err((fallback_id(), format!("invalid: {message}"))),
    }
}

/// Stamp a WS draft for the connection's bound principal.
pub(crate) fn stamp_ws_draft(
    conn: &ConnectionState,
    draft: &serde_json::Value,
) -> Result<nostr::Event, (String, String)> {
    match conn.auth_state_snapshot() {
        AuthState::Authenticated(ctx) if ctx.token.is_some() => {
            stamp_for_principal(draft, &PrincipalId::from(ctx.pubkey))
        }
        _ => Err((String::new(), "auth-required: not authenticated".to_owned())),
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    #[test]
    fn stale_deadline_and_revocation_cannot_close_new_binding() {
        let (tx, mut rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let session = Arc::new(TokenSession::new(
            [1; 32],
            Some(Utc::now()),
            cancel.clone(),
            tx,
            notice_frame,
        ));
        let old = session.current();
        session.rebind([2; 32], None);
        assert!(!session.close_if_current(old, "auth-expired: token_expired"));
        assert!(!session.close_if_hash(&old.hash, "auth-revoked: stopped"));
        let registry = TokenSessionRegistry::default();
        registry.insert(Uuid::new_v4(), session);
        assert_eq!(registry.apply(&[[1; 32]], "stopped", None), 0);
        assert!(!cancel.is_cancelled());
        assert!(rx.try_recv().is_err());
        assert_eq!(registry.apply(&[[2; 32]], "stopped", None), 1);
        assert!(cancel.is_cancelled());
        assert!(rx.try_recv().is_ok());
    }

    #[tokio::test]
    async fn stale_lookup_cannot_close_rebound_session() {
        for new_hash in [[2; 32], [1; 32]] {
            let (tx, mut rx) = mpsc::channel(4);
            let cancel = CancellationToken::new();
            let session = Arc::new(TokenSession::new(
                [1; 32],
                None,
                cancel.clone(),
                tx,
                notice_frame,
            ));
            let observed = session.current();
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
            let task = tokio::spawn({
                let session = session.clone();
                async move {
                    recheck_binding(&session, observed, async {
                        started_tx.send(()).unwrap();
                        finish_rx.await.unwrap();
                        Ok(None)
                    })
                    .await
                }
            });
            started_rx.await.unwrap();
            session.rebind(new_hash, Some(Utc::now() + chrono::Duration::hours(1)));
            finish_tx.send(()).unwrap();
            assert!(
                !task.await.unwrap(),
                "old lookup must not terminate the new generation"
            );
            assert!(!cancel.is_cancelled());
            assert!(rx.try_recv().is_err());
        }
    }
}
