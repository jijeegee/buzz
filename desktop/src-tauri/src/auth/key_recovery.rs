//! Google authorizes backup storage; the restored Nostr key remains the signer.

use nostr::{Event, EventBuilder, Keys, Kind, PublicKey, Tag, ToBech32};
use serde::Deserialize;
use tauri::Manager;
use zeroize::Zeroizing;

use super::api::{self, ApiError, LoginTokens, SecretField};
use crate::app_state::{AppState, IdentityStorage};

pub(crate) fn binding_key(origin: &str) -> String {
    format!("auth.key-backup.{origin}")
}

/// Durably retire a fresh-install marker before publishing meaningful content.
/// Auth proofs do not establish an identity. This shares the import/recovery
/// mutex so a Google restore cannot race a first user publication.
pub(crate) fn mark_identity_used(
    state: &AppState,
    pubkey: PublicKey,
    kind: Kind,
) -> Result<(), String> {
    if matches!(kind.as_u16(), 22242 | 27235)
        || state.identity_storage() != IdentityStorage::SystemKeyring
    {
        return Ok(());
    }
    let _guard = state
        .identity_mutation
        .lock()
        .map_err(|_| "identity storage is busy")?;
    let current = state
        .keys
        .lock()
        .map_err(|_| "identity storage is busy")?
        .public_key();
    if current != pubkey {
        return Ok(());
    } // A separately keyed managed agent.
    let store = crate::secret_store::SecretStore::shared(crate::app_state::keyring_service());
    if store.load("identity.bootstrap")?.as_deref() == Some(pubkey.to_hex().as_str()) {
        store.store("identity.bootstrap", "")?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct BackupStatus {
    state: String,
    account_id: String,
    pubkey: Option<String>,
    version: u8,
}

#[derive(Deserialize)]
struct Challenge {
    challenge: String,
    account_id: String,
    url: String,
}

#[derive(Deserialize)]
struct BackupIdentity {
    pubkey: String,
    version: u8,
}

#[derive(Deserialize)]
struct RestoredKey {
    secret_key: SecretField,
    pubkey: String,
    version: u8,
}

fn error(error: ApiError) -> String {
    // Never reflect response bodies from a key endpoint into UI/logs.
    match error.code.as_deref() {
        Some("reauth_required") => "Sign in with Google again to recover your key.".into(),
        Some("account_mode_conflict") => "This Google account has a separate token identity. Automatic merging is not supported.".into(),
        Some("key_already_linked") => "This signing key is already linked to another Google account.".into(),
        _ => "Key backup could not be completed. Your signing identity has not been replaced; retry Google sign-in.".into(),
    }
}

fn check_identity(expected: &PublicKey, value: &str, version: u8) -> Result<(), String> {
    if version != 1 || PublicKey::from_hex(value).ok().as_ref() != Some(expected) {
        return Err("The backup identity did not match; no replacement key was created.".into());
    }
    Ok(())
}

fn proof(keys: &Keys, account: &str, origin: &str, challenge: Challenge) -> Result<Event, String> {
    if challenge.account_id != account
        || challenge.url != format!("{origin}/auth/key-backup")
        || challenge.challenge.len() != 64
        || !challenge.challenge.bytes().all(|v| v.is_ascii_hexdigit())
    {
        return Err("The relay returned an invalid backup challenge.".into());
    }
    let tags = [
        vec!["u".to_owned(), challenge.url],
        vec!["method".to_owned(), "POST".to_owned()],
        vec!["challenge".to_owned(), challenge.challenge],
        vec!["account".to_owned(), account.to_owned()],
        vec!["action".to_owned(), "initialize".to_owned()],
    ]
    .into_iter()
    .map(Tag::parse)
    .collect::<Result<Vec<_>, _>>()
    .map_err(|_| "invalid backup challenge tags".to_owned())?;
    EventBuilder::new(Kind::HttpAuth, "")
        .tags(tags)
        .sign_with_keys(keys)
        .map_err(|_| "could not sign the backup proof".into())
}

async fn status(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    account: &str,
) -> Result<BackupStatus, String> {
    let status: BackupStatus = api::send_json(
        client
            .get(format!("{origin}/auth/key-backup"))
            .bearer_auth(access),
    )
    .await
    .map_err(error)?;
    if status.version != 1
        || status.account_id != account
        || !matches!(status.state.as_str(), "absent" | "ready")
        || (status.state == "absent" && status.pubkey.is_some())
        || (status.state == "ready" && status.pubkey.is_none())
    {
        return Err("The relay returned an invalid backup status.".into());
    }
    Ok(status)
}

async fn restore(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    status: BackupStatus,
) -> Result<Keys, String> {
    let expected = status
        .pubkey
        .as_deref()
        .and_then(|v| PublicKey::from_hex(v).ok())
        .ok_or("The stored backup has an invalid public key.")?;
    let restored: RestoredKey = api::send_json(
        client
            .post(format!("{origin}/auth/key-backup/restore"))
            .bearer_auth(access)
            .json(&serde_json::json!({})),
    )
    .await
    .map_err(error)?;
    check_identity(&expected, &restored.pubkey, restored.version)?;
    if restored.secret_key.len() != 64
        || !restored.secret_key.bytes().all(|v| v.is_ascii_hexdigit())
    {
        return Err("The recovered key is invalid; no replacement key was created.".into());
    }
    let keys = Keys::parse(&*restored.secret_key)
        .map_err(|_| "The recovered key is invalid; no replacement key was created.")?;
    check_identity(&expected, &keys.public_key().to_hex(), 1)?;
    Ok(keys)
}

/// The account behind the login method the user chose. A backed-up account is
/// restored, whatever was on this computer before. Only when the login method
/// has no account yet is `new_key` asked for the key to back up (it must be
/// durably stored before it returns); an authenticated `absent` is the only
/// state that uploads. Every restore/status failure propagates.
pub(crate) async fn resolve_backup(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    account: &str,
    new_key: impl FnOnce() -> Result<Keys, String>,
) -> Result<Keys, String> {
    let parsed = url::Url::parse(origin).map_err(|_| "invalid recovery origin")?;
    let local_dev = parsed
        .host_str()
        .is_some_and(|host| host == "localhost" || host == "127.0.0.1" || host == "[::1]");
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local_dev) {
        return Err("Google key recovery requires HTTPS outside local development.".into());
    }
    let current = status(client, origin, access, account).await?;
    if current.state == "ready" {
        return restore(client, origin, access, current).await;
    }
    let local = new_key()?;
    let challenge: Challenge = api::send_json(
        client
            .post(format!("{origin}/auth/key-backup/challenge"))
            .bearer_auth(access)
            .json(&serde_json::json!({"pubkey": local.public_key().to_hex()})),
    )
    .await
    .map_err(error)?;
    let proof = proof(&local, account, origin, challenge)?;
    let secret = Zeroizing::new(local.secret_key().to_secret_hex());
    let body = Zeroizing::new(
        serde_json::to_string(&serde_json::json!({"secret_key": secret.as_str(), "proof": proof}))
            .map_err(|_| "could not encode backup request")?,
    );
    let result: Result<BackupIdentity, ApiError> = api::send_json(
        client
            .post(format!("{origin}/auth/key-backup"))
            .bearer_auth(access)
            .header("Content-Type", "application/json")
            .body(body.as_bytes().to_vec()),
    )
    .await;
    match result {
        Ok(created) => {
            check_identity(&local.public_key(), &created.pubkey, created.version)?;
            Ok(local)
        }
        // Another device created this account's backup first: use it.
        Err(failure)
            if failure.status == Some(409) && failure.code.as_deref() == Some("backup_exists") =>
        {
            let winner = status(client, origin, access, account).await?;
            restore(client, origin, access, winner).await
        }
        Err(failure) => Err(error(failure)),
    }
}

/// Require the current key to be in verified OS secure storage before upload.
fn persist_pending(state: &AppState, keys: &Keys) -> Result<(), String> {
    let _guard = state
        .identity_mutation
        .lock()
        .map_err(|_| "identity storage is busy")?;
    if state
        .keys
        .lock()
        .map_err(|_| "identity storage is busy")?
        .public_key()
        != keys.public_key()
    {
        return Err("Your local identity changed during Google sign-in. Try again.".into());
    }
    let store = crate::secret_store::SecretStore::shared(crate::app_state::keyring_service());
    if state.identity_storage() == IdentityStorage::Environment {
        return Err(
            "Import your environment key into Buzz secure storage before linking Google.".into(),
        );
    }
    if let Some(stored) = store.load("identity")? {
        let stored = Zeroizing::new(stored);
        let previous = Keys::parse(stored.as_str())
            .map_err(|_| "The stored identity is invalid; recover it before linking Google.")?;
        if previous.public_key() != keys.public_key() {
            return Err(
                "A different identity exists in OS secure storage. It was preserved.".into(),
            );
        }
    }
    let nsec = Zeroizing::new(
        keys.secret_key()
            .to_bech32()
            .map_err(|_| "could not encode signing key")?,
    );
    store.store("identity", &nsec)?;
    if !store.verify_stored_raw("identity", &nsec)? {
        return Err("OS secure storage verification failed".into());
    }
    Ok(())
}

/// Complete a Google sign-in: enter the account behind the chosen Google
/// account and make it the active account on this computer. Accounts already
/// here stay signed in. The current key is linked to the Google account only
/// for an explicit `connect` (a signed-in key-only account protecting itself)
/// or when it is the unused first-install key; otherwise a Google account with
/// no backup gets a new key and a new Buzz account.
pub(crate) async fn complete<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    origin: &str,
    tokens: &LoginTokens,
    connect: bool,
) -> Result<PublicKey, String> {
    use super::RefreshStore;
    if tokens.identity_mode.as_deref() != Some("key_backup") {
        return Err("This Google account uses a separate token identity. No automatic conversion or account merge is supported.".into());
    }
    let state = app.state::<AppState>();
    if super::KeyringRefreshStore.legacy_token_account(origin)?
        || matches!(
            state.token_auth.mode(origin),
            super::CredentialMode::Token(_)
        )
    {
        return Err("Sign out of the separate token account before linking a Nostr key. Existing token agents will be preserved.".into());
    }
    if state
        .keyring_locked
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err(
            "Unlock your OS keyring and relaunch Buzz before recovering with Google.".into(),
        );
    }
    let recovering = state
        .identity_lost
        .load(std::sync::atomic::Ordering::Acquire);
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "identity storage is unavailable")?;
    let secrets = super::accounts::shared_secrets();
    let active = state
        .keys
        .lock()
        .map_err(|_| "identity storage is busy")?
        .clone();
    let active_pubkey = active.public_key();
    let registry = super::accounts::load_registry(&data_dir)?.unwrap_or_default();
    // "Connect" links a listed key-only account's own key; an account already
    // held by a Google account is never attached to another one. A refused
    // connect is an error, never a quiet new account.
    if connect
        && connect_refused(
            &registry,
            &active_pubkey,
            recovering,
            state.token_auth.signing_pubkey(origin).is_some()
                || super::KeyringRefreshStore.signing_pubkey(origin)?.is_some(),
        )
    {
        return Err("This account is already linked to a Google account. Sign in instead.".into());
    }
    let reuse_active = !recovering
        && (connect || {
            let marker = secrets.load("identity.bootstrap")?;
            super::accounts::is_unused_bootstrap_key(
                marker.as_deref(),
                &active_pubkey,
                registry.accounts.len(),
                crate::managed_agents::load_managed_agents(app)?.len(),
            )
        });
    let keys = resolve_backup(
        &state.media_fetch_client,
        origin,
        &tokens.access,
        &tokens.principal_id,
        || {
            if recovering {
                return Err(
                    "This Google account has no backup. Your lost identity was not replaced."
                        .into(),
                );
            }
            if reuse_active {
                persist_pending(&state, &active)?;
                return Ok(active.clone());
            }
            super::accounts::pending_new_key(secrets, &data_dir, origin, &tokens.principal_id)
        },
    )
    .await?;
    if tokens
        .signing_pubkey
        .as_deref()
        .is_some_and(|value| PublicKey::from_hex(value).ok() != Some(keys.public_key()))
    {
        return Err(
            "The Google session and recovered key disagree; your local identity was preserved."
                .into(),
        );
    }
    // An account already signed in here in the background: settle its loop
    // first so it cannot write an older token after this sign-in's.
    let was_running = state
        .token_auth
        .inactive
        .stop(origin, &keys.public_key())
        .await;
    let before = super::accounts::sign_in_before(secrets, &data_dir, origin, &keys.public_key())?;
    let replaced = before_token_replaced(&before, &tokens.refresh);
    let account = keys.public_key();
    let committed = (|| -> Result<PublicKey, String> {
        let _guard = state
            .identity_mutation
            .lock()
            .map_err(|_| "identity storage is busy")?;
        if state.current_auth_origin() != origin
            || state
                .keys
                .lock()
                .map_err(|_| "identity storage is busy")?
                .public_key()
                != active_pubkey
        {
            return Err(
                "Your community or local identity changed during Google sign-in. Try again.".into(),
            );
        }
        super::accounts::preserve_legacy_key_file(secrets, &data_dir, &keys)?;
        crate::app_state::prepare_recovered_identity(&data_dir, &keys)?;
        // While recovering a lost key, the in-memory key is a stand-in: never
        // keep it as an account or record it as anyone's owner.
        let outgoing = (!recovering).then_some(&active);
        super::accounts::commit_sign_in(
            secrets,
            &data_dir,
            origin,
            &keys,
            &tokens.refresh,
            outgoing,
        )?;
        let pubkey = keys.public_key();
        *state.keys.lock().map_err(|_| "identity storage is busy")? = keys;
        Ok(pubkey)
    })();
    let pubkey = match committed {
        Ok(pubkey) => pubkey,
        Err(error) => {
            // The commit may have got part-way (the new token in the
            // account's slot, the list making it active) and the caller
            // revokes the new session: put the account back as it was.
            let restored = match super::accounts::roll_back_sign_in(
                secrets,
                &data_dir,
                origin,
                &account,
                &tokens.refresh,
                &before,
            ) {
                Ok(restored) => restored,
                Err(rollback) => {
                    eprintln!("buzz-desktop: auth: failed sign-in not rolled back: {rollback}");
                    false
                }
            };
            // Keep its old session alive again in the background, unless it
            // became the active account meanwhile (its foreground session
            // refreshes it). A lock failure here never hides `error`.
            if was_running && restored {
                if let Ok(_guard) = state.identity_mutation.lock() {
                    let active_now = state.keys.lock().ok().map(|keys| keys.public_key());
                    if active_now.is_some_and(|active| active != account) {
                        state.token_auth.inactive.start(
                            &state.http_client,
                            Some(data_dir.clone()),
                            origin,
                            account,
                        );
                    }
                }
            }
            return Err(error);
        }
    };
    // Signing in again to an account that still had a session here replaced
    // that session: end the old one on the server instead of orphaning it.
    // Only now, once the new token is committed in its place.
    if let Some(old) = replaced {
        let client = state.http_client.clone();
        let origin = origin.to_owned();
        tauri::async_runtime::spawn(async move {
            match api::refresh(&client, &origin, &old).await {
                Ok(replaced) => {
                    if let Err(error) = api::logout(&client, &origin, &replaced.access).await {
                        eprintln!("buzz-desktop: auth: replaced session not revoked: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("buzz-desktop: auth: replaced session already ended: {error}")
                }
            }
        });
    }
    state.set_identity_storage(IdentityStorage::SystemKeyring);
    state
        .identity_lost
        .store(false, std::sync::atomic::Ordering::Release);
    state
        .keyring_locked
        .store(false, std::sync::atomic::Ordering::Release);
    state.token_auth.set_key_backup(origin, pubkey);
    Ok(pubkey)
}

/// The old session a sign-in replaces: the account's token before it, when
/// it differs from the new one.
fn before_token_replaced(
    before: &super::accounts::SignInBefore,
    new_token: &str,
) -> Option<Zeroizing<String>> {
    before
        .old_token()
        .filter(|old| *old != new_token)
        .map(|old| Zeroizing::new(old.to_owned()))
}

/// Whether "Connect" must be refused: only a listed key-only account's own
/// key may be linked, never while recovering a lost key, and never when this
/// login method is already bound to a key (`linked`).
pub(super) fn connect_refused(
    registry: &super::accounts::Registry,
    active: &PublicKey,
    recovering: bool,
    linked: bool,
) -> bool {
    let mut entries = registry
        .accounts
        .iter()
        .filter(|a| a.pubkey == active.to_hex())
        .peekable();
    let key_only = entries.peek().is_some()
        && entries.all(|a| a.state == super::accounts::AccountState::KeyOnly);
    recovering || !key_only || linked
}

#[cfg(test)]
mod tests;
