//! Centralized-identity `Authorization: Bearer` door for Blossom media.
//!
//! Inert unless token auth is enabled and NIP-FI is `Off` (the same gate as
//! the bridge, [`crate::api::bridge::verify_bridge_bearer`]); otherwise every
//! media request takes the kind-24242 Blossom path exactly as before.
//!
//! A token-authenticated upload has no client-signed kind-24242 event. The
//! relay instead mints an internal *upload proof*: a server-stamped (sentinel
//! signature) kind-24242 event attributed to the verified principal whose
//! single `x` tag is the client's `X-SHA-256`. The media pipeline consumes it
//! exactly like a verified Blossom event — uploader = `pubkey`, and the
//! post-body hash check binds the stored bytes to the declared hash. The proof
//! never leaves the process and is never verified as a signature.

use axum::http::HeaderMap;
use axum::response::{IntoResponse as _, Response};
use buzz_core::principal::PrincipalId;
use buzz_core::tenant::TenantContext;
use buzz_media::auth::BlossomStrictness;
use buzz_media::MediaError;

use crate::state::AppState;

/// The verified token principal for a media request, after the relay
/// membership and ban gates; `Ok(None)` when the Bearer door does not apply
/// (flag off, NIP-FI active, or no Bearer header) and the caller must use the
/// Blossom path.
///
/// Token failures answer with the `/auth/*` 401 shape; a bot whose owner is
/// banned is refused like the owner; a non-member or banned principal gets
/// the same denial as the Blossom path.
pub(super) async fn media_token_principal(
    state: &AppState,
    tenant: &TenantContext,
    headers: &HeaderMap,
    strictness: BlossomStrictness,
) -> Result<Option<PrincipalId>, Response> {
    let Some(pubkey) = crate::api::bridge::verify_bridge_bearer(state, headers, tenant).await?
    else {
        return Ok(None);
    };
    let principal = PrincipalId::from_slice(pubkey.as_bytes()).map_err(|error| {
        tracing::error!(%error, "verified token principal is not a valid principal id");
        MediaError::Internal.into_response()
    })?;
    // No NIP-OA credential in the token model: ownership lives server-side,
    // and the owner-ban cascade already ran in `verify_bridge_bearer`.
    crate::api::relay_members::enforce_relay_membership(
        state,
        tenant.community(),
        principal.as_bytes(),
        None,
        None,
    )
    .await
    .map_err(|e| super::membership_denial(e, strictness).into_response())?;
    Ok(Some(principal))
}

/// Mint the internal upload proof for a token upload of `claimed_hash`
/// (already validated as 64 lowercase hex chars).
pub(super) fn token_upload_proof(
    principal: &PrincipalId,
    claimed_hash: &str,
) -> Result<nostr::Event, MediaError> {
    let draft = serde_json::json!({
        "kind": 24242,
        "content": "Upload buzz-media (token)",
        "tags": [["t", "upload"], ["x", claimed_hash]],
    });
    buzz_core::draft::stamp_draft(&draft, principal, chrono::Utc::now().timestamp()).map_err(
        |error| {
            tracing::error!(?error, "failed to mint token upload proof");
            MediaError::Internal
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_proof_binds_principal_and_declared_hash() {
        let principal = PrincipalId::generate();
        let hash = "ab".repeat(32);
        let proof = token_upload_proof(&principal, &hash).expect("proof");
        assert_eq!(proof.pubkey, principal.as_public_key());
        assert_eq!(proof.kind.as_u16(), 24242);
        assert!(buzz_core::draft::is_sentinel_sig(proof.sig.as_ref()));
        // The post-body check the media pipeline runs must accept exactly the
        // declared hash and nothing else.
        buzz_media::auth::verify_upload_hash_only(&proof, &hash).expect("declared hash binds");
        assert!(buzz_media::auth::verify_upload_hash_only(&proof, &"cd".repeat(32)).is_err());
    }
}
