//! Sign-outs the user asked for: logout, sign-out on this device, deleting
//! the account. Only these delete the account's stored messages on the
//! server. A session the relay or another device ended (revoked or failed
//! refresh token, refused socket) goes through [`sign_out_account`] alone and
//! keeps them, so signing back in finds the history already here.

use nostr::PublicKey;
use tauri::{AppHandle, Manager};

use super::commands::{sign_out_account, SignedOut};
use crate::app_state::AppState;

/// [`sign_out_account`] the user asked for: on success the account's stored
/// messages on `origin` are deleted too.
pub(crate) async fn sign_out_by_user(
    app: &AppHandle,
    origin: &str,
    account: Option<PublicKey>,
    revoke: bool,
) -> Result<SignedOut, String> {
    // Read before signing out: afterwards `origin` may sign as another account.
    let target = account.or_else(|| app.state::<AppState>().token_auth.signing_pubkey(origin));
    let signed_out = sign_out_account(app, origin, account, revoke).await?;
    forget_messages(app, origin, target).await;
    Ok(signed_out)
}

/// Deletes `account`'s stored messages on `origin` (the user removed it).
pub(crate) async fn forget_messages(app: &AppHandle, origin: &str, account: Option<PublicKey>) {
    if let Some(account) = account {
        crate::message_store::purge_account(app, origin, account).await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::message_store::{MessageStore, StoreScope};

    fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
        source
            .split(signature)
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap_or_else(|| panic!("{signature} not found"))
    }

    fn stored_files(dir: &Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".sqlite3"))
                .count()
        })
    }

    #[test]
    fn remote_session_ends_keep_messages_and_user_sign_outs_delete_them() {
        let source = include_str!("commands.rs");
        // Every remote path (another device, the relay's ended-session hook,
        // the remote sign-out check) signs out through `sign_out_account`.
        let shared = function_body(source, "pub(crate) async fn sign_out_account(");
        assert!(!shared.contains("message_store") && !shared.contains("user_sign_out"));
        for user_command in [
            "pub async fn logout(",
            "pub async fn logout_on_this_device(",
            "pub async fn delete_account(",
        ] {
            assert!(
                function_body(source, user_command).contains("user_sign_out::"),
                "{user_command} must delete stored messages"
            );
        }
    }

    #[tokio::test]
    async fn user_sign_out_deletes_only_that_account_on_that_server() {
        let dir = tempfile::tempdir().unwrap();
        let store = MessageStore::default();
        let (alice, bob) = (
            nostr::Keys::generate().public_key(),
            nostr::Keys::generate().public_key(),
        );
        let scope = |account: &nostr::PublicKey, relay: &str| StoreScope {
            pubkey: account.to_hex(),
            relay_url: relay.to_string(),
        };
        for scope in [
            scope(&alice, "wss://relay.example"),
            scope(&alice, "wss://other.example"),
            scope(&bob, "wss://relay.example"),
        ] {
            store.call(dir.path(), &scope, |_| Ok(())).await.unwrap();
        }
        assert_eq!(stored_files(dir.path()), 3);
        store
            .purge_account_in(dir.path(), "https://relay.example", &alice)
            .unwrap();
        assert_eq!(stored_files(dir.path()), 2);
    }
}
