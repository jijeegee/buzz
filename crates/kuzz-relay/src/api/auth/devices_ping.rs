//! Tell an account's own devices that its device list changed, so their
//! device and external agent lists reread `/auth/devices` at once instead of
//! waiting out their cache.

use std::sync::Arc;

use axum::http::{header, HeaderMap};
use kuzz_core::principal::PrincipalId;

use crate::state::AppState;

/// Whose devices hear the ping.
pub(super) enum PingTo {
    /// Signed-in calls: the account's message key is looked up.
    Principal(PrincipalId),
    /// A pairing claim already knows the owner's message key.
    Key([u8; 32]),
}

/// Send the "devices changed" ping (kind:24210, reason `devices`) to the
/// account in the community `headers` name. Runs on its own task, so it never
/// delays or fails the request; errors are only logged. An account without a
/// message key gets none.
pub(super) fn spawn_devices_ping(state: &Arc<AppState>, headers: &HeaderMap, to: PingTo) {
    let state = Arc::clone(state);
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    tokio::spawn(async move {
        let tenant = match crate::tenant::bind_community(&state.db, &host).await {
            Ok(tenant) => tenant,
            Err(crate::tenant::BindError::UnmappedHost) => return,
            Err(crate::tenant::BindError::Lookup(error)) => {
                tracing::warn!(%error, "devices ping: community lookup failed");
                return;
            }
        };
        let pubkey = match to {
            PingTo::Key(key) => key.to_vec(),
            PingTo::Principal(principal) => {
                match state.db.principal_messaging_pubkey(&principal).await {
                    Ok(Some(pubkey)) => pubkey,
                    Ok(None) => return,
                    Err(error) => {
                        tracing::warn!(%error, "devices ping: account key lookup failed");
                        return;
                    }
                }
            }
        };
        if let Err(error) =
            crate::handlers::side_effects::publish_devices_ping(&tenant, &state, &pubkey).await
        {
            tracing::warn!(%error, "devices ping failed");
        }
    });
}
