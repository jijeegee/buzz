//! Provider revoke tokens: Apple's refresh token is kept, encrypted, from
//! sign-in so that deleting the account also revokes the app's access to the
//! Apple ID (App Store review requirement). Both halves are best effort: a
//! missing master key, a storage error or a failed revoke is logged and never
//! blocks the sign-in or the deletion.

use std::sync::Arc;

use kuzz_auth::key_backup::SealedProviderToken;
use kuzz_auth::oidc::{OidcIdentity, RevokeToken};
use kuzz_core::principal::PrincipalId;
use kuzz_db::identity::ProviderRevokeTokenRecord;

use crate::state::AppState;

/// Store the revoke token of a successful provider sign-in, if it has one.
pub(super) async fn remember(state: &AppState, provider: &str, identity: &OidcIdentity) {
    let Some(token) = &identity.revoke_token else {
        return;
    };
    let Some(master) = &state.identity.config().key_backup_master else {
        tracing::warn!(%provider, "no key-backup master key; provider revoke token not kept");
        return;
    };
    let sealed = match master.seal_provider_token(provider, &identity.subject, token.expose()) {
        Ok(sealed) => sealed,
        Err(error) => {
            tracing::warn!(%error, %provider, "provider revoke token not sealed");
            return;
        }
    };
    let record = ProviderRevokeTokenRecord {
        provider: provider.to_owned(),
        subject: identity.subject.clone(),
        key_id: sealed.key_id,
        nonce: sealed.nonce,
        ciphertext: sealed.ciphertext,
    };
    if let Err(error) = state.db.store_provider_revoke_token(&record).await {
        tracing::warn!(%error, %provider, "provider revoke token not stored");
    }
}

/// Revoke, in the background, every provider sign-in of a deleted account
/// that has a stored token, then forget the tokens that were revoked.
pub(super) fn spawn_revoke_all(state: Arc<AppState>, principal: PrincipalId) {
    tokio::spawn(async move {
        let records = match state.db.principal_provider_revoke_tokens(&principal).await {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(%error, %principal, "provider revoke tokens unreadable");
                return;
            }
        };
        for record in records {
            revoke_one(&state, &principal, record).await;
        }
    });
}

async fn revoke_one(state: &AppState, principal: &PrincipalId, record: ProviderRevokeTokenRecord) {
    let provider_name = record.provider.clone();
    let Some(provider) = state.identity.provider(&provider_name) else {
        tracing::warn!(provider = %provider_name, %principal, "provider gone; sign-in not revoked");
        return;
    };
    let Some(master) = &state.identity.config().key_backup_master else {
        tracing::warn!(provider = %provider_name, %principal, "no master key; sign-in not revoked");
        return;
    };
    let sealed = SealedProviderToken {
        key_id: record.key_id,
        nonce: record.nonce,
        ciphertext: record.ciphertext,
    };
    let token = match master.open_provider_token(&provider_name, &record.subject, &sealed) {
        Ok(token) => RevokeToken::new(token.as_str().to_owned()),
        Err(error) => {
            tracing::warn!(%error, provider = %provider_name, %principal, "provider revoke token unreadable");
            return;
        }
    };
    if let Err(error) = provider.revoke(&token).await {
        tracing::warn!(%error, provider = %provider_name, %principal, "provider sign-in revoke failed");
        return;
    }
    tracing::info!(provider = %provider_name, %principal, "auth.provider_sign_in_revoked");
    if let Err(error) = state
        .db
        .delete_provider_revoke_token(&provider_name, &record.subject)
        .await
    {
        tracing::warn!(%error, provider = %provider_name, "revoked provider token not deleted");
    }
}
