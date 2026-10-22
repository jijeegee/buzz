//! Open key-authenticated sockets (device-bound keys plan §2.2, PR A4b).
//!
//! A NIP-42 AUTH or huddle audio socket admitted by its message key is
//! registered here before its admission runs, and the admission records the
//! (account, device row) it was admitted as ([`admit_key_socket`]). A
//! sign-out, device revoke, account deletion or agent move then closes the
//! matching sockets on every replica ([`forget`] and [`close_revoked`], fed
//! by the auth-revocation channel), and a periodic registry recheck
//! ([`spawn_recheck`]) backstops a missed message and judges sockets refused
//! under `log` again, so a `log` → `enforce` flip closes them.
//!
//! What a refusal does follows the socket's surface mode, as at admission:
//! `enforce` closes the socket with the client-visible refusal
//! (`restricted: signed_out`, `restricted: revoked`, ...), `log` only
//! reports it, `off` does nothing and no registry work at all.
//!
//! [`admit_key_socket`]: super::admission::admit_key_socket

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::ws::Message as WsMessage;
use dashmap::DashMap;
use kuzz_auth::device_proof::{DevicePubkey, DEVICE_PUBKEY_LEN};
use kuzz_core::principal::PrincipalId;
use kuzz_db::identity::KeyRefusal;
use kuzz_pubsub::AuthRevocation;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::admission::{KeyRefused, Known};
use super::config::DeviceAdmissionMode;
use super::device::{self, Surface};
use super::ws::CloseFrame;
use crate::state::AppState;

/// How often open key sockets re-read the registry (plan S7).
pub(crate) const RECHECK_INTERVAL: Duration = Duration::from_secs(60);
/// Registry reads one recheck runs at a time.
const RECHECK_CONCURRENCY: usize = 16;

/// What a key was admitted as: the account it acts for and the device row
/// it is used from ([`KeyAdmission::Admitted`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Admitted {
    pub(crate) principal: PrincipalId,
    pub(crate) device_id: Uuid,
}

/// One open socket authenticated by a message key.
pub(crate) struct KeySocket {
    pubkey: [u8; 32],
    device_key: Option<[u8; DEVICE_PUBKEY_LEN]>,
    surface: Surface,
    admitted: Mutex<Option<Admitted>>,
    cancel: CancellationToken,
    ctrl_tx: mpsc::Sender<WsMessage>,
    close_frame: CloseFrame,
}

impl KeySocket {
    /// A socket for `pubkey` on the device holding `device_key` (its auth
    /// event's verified `device` tag), closed by queueing `close_frame` on
    /// `ctrl_tx` and cancelling `cancel`.
    pub(crate) fn new(
        pubkey: &nostr::PublicKey,
        device_key: Option<DevicePubkey>,
        surface: Surface,
        cancel: CancellationToken,
        ctrl_tx: mpsc::Sender<WsMessage>,
        close_frame: CloseFrame,
    ) -> Self {
        Self {
            pubkey: pubkey.to_bytes(),
            device_key: device_key.map(|key| *key.as_bytes()),
            surface,
            admitted: Mutex::new(None),
            cancel,
            ctrl_tx,
            close_frame,
        }
    }

    pub(crate) fn surface(&self) -> Surface {
        self.surface
    }

    /// What the socket was admitted as (`None`: not yet, an unregistered
    /// key, or the surface was off).
    pub(crate) fn admitted(&self) -> Option<Admitted> {
        *self.admitted.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Record the admission (before it is returned to the caller).
    pub(crate) fn record(&self, admitted: Admitted) {
        *self.admitted.lock().unwrap_or_else(PoisonError::into_inner) = Some(admitted);
    }

    /// Drop the record (a revocation overtook the admission).
    pub(crate) fn forget(&self) {
        *self.admitted.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Queue the route's close frame for `notice` on the control channel
    /// (drained before the close), then cancel the connection.
    fn close(&self, notice: &str) {
        let _ = self.ctrl_tx.try_send((self.close_frame)(notice));
        self.cancel.cancel();
    }
}

/// Key-authenticated sockets on this instance, keyed by connection id.
#[derive(Default)]
pub(crate) struct KeySocketRegistry {
    sockets: DashMap<Uuid, Arc<KeySocket>>,
}

impl KeySocketRegistry {
    /// Track `socket` under `conn_id` so revocations reach it.
    pub(crate) fn insert(&self, conn_id: Uuid, socket: Arc<KeySocket>) {
        self.sockets.insert(conn_id, socket);
    }

    /// Forget a connection (connection teardown).
    pub(crate) fn remove(&self, conn_id: Uuid) {
        self.sockets.remove(&conn_id);
    }

    fn snapshot(&self) -> Vec<Arc<KeySocket>> {
        self.sockets
            .iter()
            .map(|entry| Arc::clone(entry.value()))
            .collect()
    }
}

/// Unregisters its socket when dropped (the audio handler's lifetime).
pub(crate) struct KeySocketGuard {
    state: Arc<AppState>,
    conn_id: Uuid,
}

impl KeySocketGuard {
    /// Register `socket` until the guard is dropped.
    pub(crate) fn register(state: &Arc<AppState>, socket: Arc<KeySocket>) -> Self {
        let conn_id = Uuid::new_v4();
        state.identity.key_sockets.insert(conn_id, socket);
        Self {
            state: Arc::clone(state),
            conn_id,
        }
    }
}

impl Drop for KeySocketGuard {
    fn drop(&mut self) {
        self.state.identity.key_sockets.remove(self.conn_id);
    }
}

/// The refusal a revocation of a whole device stands for.
fn device_refusal(reason: &str) -> KeyRefused {
    KeyRefused::Key(match reason {
        "account_disabled" => KeyRefusal::PrincipalDisabled,
        "device_revoked" | "revoke_all" => KeyRefusal::DeviceRevoked,
        _ => KeyRefusal::NotSignedIn,
    })
}

/// Apply `refused` to `socket` by its surface's mode. Returns whether it
/// was closed (`enforce` only).
fn refuse(state: &AppState, socket: &KeySocket, refused: KeyRefused) -> bool {
    let mode = device::mode(state, socket.surface);
    if mode == DeviceAdmissionMode::Off {
        return false;
    }
    let enforced = mode == DeviceAdmissionMode::Enforce;
    let subject = hex::encode(socket.pubkey);
    device::report(state, socket.surface, &subject, refused.as_str(), enforced);
    if enforced {
        tracing::info!(pubkey = %subject, reason = refused.as_str(), "closing key socket");
        socket.forget();
        socket.close(refused.reason());
    }
    enforced
}

/// Whether key sockets are checked at all: the root or audio surface is
/// not `off`. While both are off nothing is registered, published or read.
pub(crate) fn enabled(state: &AppState) -> bool {
    device::mode(state, Surface::Ws) != DeviceAdmissionMode::Off
        || device::mode(state, Surface::Audio) != DeviceAdmissionMode::Off
}

/// The synchronous part of a revocation: forget cached admissions for its
/// devices and keys, so no admission read before it is served or recorded.
pub(crate) fn forget(state: &AppState, message: &AuthRevocation) {
    if message.device_ids.is_empty() && message.pubkeys.is_empty() {
        return;
    }
    state
        .identity
        .admissions
        .revoke(&message.device_ids, &message.key_bytes());
}

/// Apply the device and key part of a revocation on this instance
/// ([`forget`], then [`close_revoked`]). Returns how many sockets closed.
#[cfg(test)]
pub(crate) async fn apply(state: &AppState, message: &AuthRevocation) -> usize {
    forget(state, message);
    close_revoked(state, message).await
}

/// Read the registry again for the sockets admitted on the revocation's
/// devices or using its keys and refuse the ones it no longer admits. The
/// revoking transaction committed before the message was sent, so the
/// registry already has the new state; reading it (instead of closing
/// blindly) keeps a socket admitted after a quick re-login on the same
/// device open when the message arrives late or twice. When the registry
/// cannot be read, sockets on a named device are refused anyway (fail
/// closed). Call after [`forget`]. Returns how many sockets were closed.
pub(crate) async fn close_revoked(state: &AppState, message: &AuthRevocation) -> usize {
    let pubkeys = message.key_bytes();
    let device_ids = &message.device_ids;
    if device_ids.is_empty() && pubkeys.is_empty() {
        return 0;
    }
    let on_device =
        |admitted: Option<Admitted>| admitted.is_some_and(|a| device_ids.contains(&a.device_id));
    let groups = groups(state, |socket| {
        on_device(socket.admitted()) || pubkeys.contains(&socket.pubkey)
    });
    let fallback = device_refusal(&message.reason);
    read_and_settle(state, groups, |socket| {
        on_device(socket.admitted()).then_some(fallback)
    })
    .await
}

type Group = ([u8; 32], Option<[u8; DEVICE_PUBKEY_LEN]>);

/// The open sockets `matches` selects, per (key, device key), whether or not
/// their admission was recorded: a socket refused (or unregistered) under
/// `log` is judged again, so flipping its surface to `enforce` closes it
/// within [`RECHECK_INTERVAL`]. Sockets on a surface that is off are skipped
/// (no work at all). Known limit: a socket that authenticated while its
/// surface was `off` read no device tag and was never registered, so an
/// `off` → `enforce` flip only reaches it when it reconnects.
fn groups(
    state: &AppState,
    matches: impl Fn(&KeySocket) -> bool,
) -> HashMap<Group, Vec<Arc<KeySocket>>> {
    let mut groups: HashMap<Group, Vec<Arc<KeySocket>>> = HashMap::new();
    for socket in state.identity.key_sockets.snapshot() {
        if socket.cancel.is_cancelled()
            || device::mode(state, socket.surface) == DeviceAdmissionMode::Off
            || !matches(&socket)
        {
            continue;
        }
        groups
            .entry((socket.pubkey, socket.device_key))
            .or_default()
            .push(socket);
    }
    groups
}

/// [`groups`] for a recheck of every open socket, or those of `only_keys`.
fn recheck_groups(
    state: &AppState,
    only_keys: Option<&[[u8; 32]]>,
) -> HashMap<Group, Vec<Arc<KeySocket>>> {
    groups(state, |socket| {
        only_keys.is_none_or(|keys| keys.contains(&socket.pubkey))
    })
}

/// Apply a registry answer (read at generation `epoch`) to the sockets of
/// one (key, device key): a refusal refuses them, an admission is recorded
/// (so a later revocation of that device finds them) unless a revocation
/// overtook the read, "unregistered" keeps them as they are.
fn settle(state: &AppState, sockets: &[Arc<KeySocket>], answer: Known, epoch: u64) -> usize {
    match answer {
        Known::Refused(reason) => sockets
            .iter()
            .filter(|socket| refuse(state, socket, KeyRefused::Key(reason)))
            .count(),
        Known::Admitted(admitted) => {
            for socket in sockets {
                socket.record(admitted);
                if state.identity.admissions.revoked_since(
                    epoch,
                    admitted.device_id,
                    &socket.pubkey,
                ) {
                    // The revocation may have missed this record; the next
                    // recheck reads the registry again.
                    socket.forget();
                }
            }
            0
        }
        Known::Unregistered => 0,
    }
}

/// The registry's answer for one group: from the admission cache while it
/// is valid, else read (and cached).
async fn answer(
    state: &AppState,
    (pubkey, device_key): Group,
) -> Result<(Known, u64), kuzz_db::DbError> {
    let cache = &state.identity.admissions;
    if let Some(known) = cache.known(&pubkey, device_key) {
        return Ok(known);
    }
    let epoch = cache.epoch();
    let answer = state.db.key_admission(&pubkey, device_key.as_ref()).await?;
    Ok((cache.remember(&pubkey, device_key, answer, epoch), epoch))
}

/// Answer every group, [`RECHECK_CONCURRENCY`] reads at a time, and settle
/// its sockets. On a read error, `on_error` names the refusal for a socket
/// (`None`: keep it until the next recheck). Returns how many sockets were
/// closed.
async fn read_and_settle(
    state: &AppState,
    groups: HashMap<Group, Vec<Arc<KeySocket>>>,
    on_error: impl Fn(&KeySocket) -> Option<KeyRefused>,
) -> usize {
    use futures_util::StreamExt as _;
    let mut answers = futures_util::stream::iter(groups)
        .map(|(group, sockets)| async move { (group, sockets, answer(state, group).await) })
        .buffer_unordered(RECHECK_CONCURRENCY);
    let mut closed = 0;
    while let Some(((pubkey, _), sockets, answer)) = answers.next().await {
        match answer {
            Ok((known, epoch)) => closed += settle(state, &sockets, known, epoch),
            Err(error) => {
                tracing::warn!(%error, pubkey = %hex::encode(pubkey), "key socket recheck failed");
                closed += sockets
                    .iter()
                    .filter(|socket| {
                        on_error(socket).is_some_and(|refused| refuse(state, socket, refused))
                    })
                    .count();
            }
        }
    }
    closed
}

/// Re-read the registry for every open key socket (or those of
/// `only_keys`) and refuse the ones it does not admit. A lookup error keeps
/// the socket (the next recheck tries again). Returns how many sockets were
/// closed.
pub(crate) async fn recheck(state: &AppState, only_keys: Option<&[[u8; 32]]>) -> usize {
    read_and_settle(state, recheck_groups(state, only_keys), |_| None).await
}

/// Recheck open key sockets every [`RECHECK_INTERVAL`] for the life of the
/// process.
pub fn spawn_recheck(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(RECHECK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick is immediate; nothing has been admitted yet.
        interval.tick().await;
        loop {
            interval.tick().await;
            recheck(&state, None).await;
        }
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A key socket for `pubkey` with no device key, and its control
    /// channel's receiver.
    pub(crate) fn socket_with_rx(
        pubkey: nostr::PublicKey,
        surface: Surface,
    ) -> (Arc<KeySocket>, mpsc::Receiver<WsMessage>) {
        let (tx, rx) = mpsc::channel(4);
        let socket = KeySocket::new(
            &pubkey,
            None,
            surface,
            CancellationToken::new(),
            tx,
            crate::audio::token_auth::audio_close_frame,
        );
        (Arc::new(socket), rx)
    }

    /// A key socket for `pubkey` whose close frames go nowhere.
    pub(crate) fn socket(pubkey: nostr::PublicKey, surface: Surface) -> KeySocket {
        let (socket, _) = socket_with_rx(pubkey, surface);
        Arc::into_inner(socket).expect("sole owner")
    }

    fn admitted(device_id: u128) -> Admitted {
        Admitted {
            principal: PrincipalId::generate(),
            device_id: Uuid::from_u128(device_id),
        }
    }

    async fn state(mode: DeviceAdmissionMode) -> Arc<AppState> {
        crate::state::tests::test_state_with_config(
            "postgres://kuzz:kuzz_dev@127.0.0.1:1/kuzz",
            |config| config.auth_token.device_admission = mode,
        )
        .await
    }

    fn text(frame: WsMessage) -> String {
        match frame {
            WsMessage::Text(text) => text.to_string(),
            other => panic!("unexpected frame {other:?}"),
        }
    }

    /// Old revocation messages (no device fields) still parse, and the
    /// reason picks the refusal sockets are closed with.
    #[test]
    fn revocation_reasons_map_to_client_refusals() {
        let old: AuthRevocation =
            serde_json::from_str(r#"{"token_hashes":[],"reason":"logout"}"#).expect("old message");
        assert!(old.device_ids.is_empty() && old.pubkeys.is_empty());
        for (reason, client) in [
            ("logout", "restricted: signed_out"),
            ("device_revoked", "restricted: signed_out"),
            ("revoke_all", "restricted: signed_out"),
            ("account_disabled", "restricted: revoked"),
        ] {
            assert_eq!(device_refusal(reason).reason(), client, "{reason}");
        }
    }

    /// A device revocation closes the sockets admitted on that device under
    /// `enforce` (with `restricted: signed_out`; the registry is unreachable
    /// here, so by the fail-closed fallback), only reports under `log`, and
    /// leaves others and unadmitted sockets alone.
    #[tokio::test]
    async fn a_device_revocation_closes_its_sockets_only_under_enforce() {
        for mode in [
            DeviceAdmissionMode::Enforce,
            DeviceAdmissionMode::Log,
            DeviceAdmissionMode::Off,
        ] {
            let state = state(mode).await;
            let key = nostr::Keys::generate().public_key();
            let (laptop, mut laptop_rx) = socket_with_rx(key, Surface::Ws);
            let (phone, mut phone_rx) = socket_with_rx(key, Surface::Audio);
            let (pending, _pending_rx) = socket_with_rx(key, Surface::Ws);
            laptop.record(admitted(1));
            phone.record(admitted(2));
            for socket in [&laptop, &phone, &pending] {
                state
                    .identity
                    .key_sockets
                    .insert(Uuid::new_v4(), Arc::clone(socket));
            }
            let message = AuthRevocation::key_batches(&[Uuid::from_u128(1)], &[], "logout")
                .pop()
                .expect("one batch");
            let closed = apply(&state, &message).await;
            let enforced = mode == DeviceAdmissionMode::Enforce;
            assert_eq!(closed, usize::from(enforced), "{mode:?}");
            assert_eq!(laptop.cancel.is_cancelled(), enforced, "{mode:?}");
            if enforced {
                assert!(text(laptop_rx.try_recv().expect("close frame"))
                    .contains("restricted: signed_out"));
                assert!(laptop.admitted().is_none());
            } else {
                assert!(laptop_rx.try_recv().is_err());
                assert!(laptop.admitted().is_some());
            }
            assert!(!phone.cancel.is_cancelled() && phone_rx.try_recv().is_err());
            assert!(!pending.cancel.is_cancelled());
        }
    }

    /// The recheck by mode: `off` selects nothing (so reads nothing), `log`
    /// and `enforce` read every open socket, refused-under-log ones too; a
    /// refusal closes only under `enforce`, an admission is recorded, an
    /// unknown answer keeps the socket; cached answers need no read.
    #[tokio::test]
    async fn recheck_closes_refused_sockets_only_under_enforce() {
        for mode in [
            DeviceAdmissionMode::Enforce,
            DeviceAdmissionMode::Log,
            DeviceAdmissionMode::Off,
        ] {
            let state = state(mode).await;
            let key = nostr::Keys::generate().public_key();
            let other = nostr::Keys::generate().public_key();
            let (socket, mut rx) = socket_with_rx(key, Surface::Ws);
            let (other_socket, _other_rx) = socket_with_rx(other, Surface::Ws);
            let (pending, _pending_rx) = socket_with_rx(key, Surface::Ws);
            socket.record(admitted(1));
            other_socket.record(admitted(2));
            for socket in [&socket, &other_socket, &pending] {
                state
                    .identity
                    .key_sockets
                    .insert(Uuid::new_v4(), Arc::clone(socket));
            }

            let groups = recheck_groups(&state, None);
            if mode == DeviceAdmissionMode::Off {
                assert!(groups.is_empty());
                // Nothing selected, so no registry read: no closes either.
                assert_eq!(recheck(&state, None).await, 0);
                continue;
            }
            assert_eq!(groups.len(), 2, "{mode:?}");
            // The unrecorded (refused under log) socket is judged too.
            assert_eq!(groups[&(key.to_bytes(), None)].len(), 2, "{mode:?}");
            let only = recheck_groups(&state, Some(&[other.to_bytes()]));
            assert_eq!(only.keys().collect::<Vec<_>>(), [&(other.to_bytes(), None)]);

            let sockets = &groups[&(key.to_bytes(), None)];
            let epoch = state.identity.admissions.epoch();
            assert_eq!(settle(&state, sockets, Known::Unregistered, epoch), 0);
            assert!(!socket.cancel.is_cancelled());
            // An unreachable registry keeps every socket.
            assert_eq!(recheck(&state, None).await, 0);
            assert!(!socket.cancel.is_cancelled());
            // An admission is recorded on the unrecorded socket too.
            let found = admitted(3);
            assert_eq!(settle(&state, sockets, Known::Admitted(found), epoch), 0);
            assert_eq!(pending.admitted(), Some(found));
            // ... unless a revocation of its device overtook the read.
            state.identity.admissions.revoke(&[found.device_id], &[]);
            settle(&state, sockets, Known::Admitted(found), epoch);
            assert_eq!(pending.admitted(), None);

            // A cached refusal needs no read (the registry is unreachable).
            let enforced = mode == DeviceAdmissionMode::Enforce;
            state.identity.admissions.remember(
                &key.to_bytes(),
                None,
                kuzz_db::identity::KeyAdmission::Refused {
                    role: kuzz_db::identity::KeyRole::Agent,
                    reason: KeyRefusal::WrongHost,
                },
                epoch,
            );
            assert_eq!(
                recheck(&state, Some(&[key.to_bytes()])).await,
                2 * usize::from(enforced),
                "{mode:?}"
            );
            assert_eq!(socket.cancel.is_cancelled(), enforced, "{mode:?}");
            assert_eq!(pending.cancel.is_cancelled(), enforced, "{mode:?}");
            if enforced {
                assert!(text(rx.try_recv().expect("close frame")).contains("restricted: device"));
            }
            assert!(!other_socket.cancel.is_cancelled());
        }
    }
}
