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
    local: &Keys,
    allow_restore: bool,
    status: BackupStatus,
) -> Result<Keys, String> {
    let expected = status
        .pubkey
        .as_deref()
        .and_then(|v| PublicKey::from_hex(v).ok())
        .ok_or("The stored backup has an invalid public key.")?;
    if !allow_restore && expected != local.public_key() {
        return Err("This Google account backs up a different Buzz identity. Your current key and agents were preserved; accounts cannot be merged.".into());
    }
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

/// Production network state machine. Only authenticated `absent` can upload;
/// every restore/status failure propagates without generating another key.
pub(crate) async fn resolve_backup(
    client: &reqwest::Client,
    origin: &str,
    access: &str,
    account: &str,
    local: &Keys,
    allow_restore: bool,
    persist_pending: impl FnOnce(&Keys) -> Result<(), String>,
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
        return restore(client, origin, access, local, allow_restore, current).await;
    }
    persist_pending(local)?;
    let challenge: Challenge = api::send_json(
        client
            .post(format!("{origin}/auth/key-backup/challenge"))
            .bearer_auth(access)
            .json(&serde_json::json!({"pubkey": local.public_key().to_hex()})),
    )
    .await
    .map_err(error)?;
    let proof = proof(local, account, origin, challenge)?;
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
            Ok(local.clone())
        }
        Err(failure)
            if failure.status == Some(409)
                && failure.code.as_deref() == Some("backup_exists")
                && allow_restore =>
        {
            let winner = status(client, origin, access, account).await?;
            restore(client, origin, access, local, true, winner).await
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

/// Resolve onboarding/link and atomically commit key + binding + refresh token.
pub(crate) async fn complete<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    origin: &str,
    tokens: &LoginTokens,
    allow_restore: bool,
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
    let recovering = state
        .identity_lost
        .load(std::sync::atomic::Ordering::Acquire);
    if state
        .keyring_locked
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err(
            "Unlock your OS keyring and relaunch Buzz before recovering with Google.".into(),
        );
    }
    let local = if recovering && allow_restore {
        state
            .keys
            .lock()
            .map_err(|_| "identity storage is busy")?
            .clone()
    } else {
        state.signing_keys()?
    };
    let local_pubkey = local.public_key();
    // Only an explicitly marked, unused bootstrap key can be replaced. Old
    // installs without the marker are established; no relay membership query
    // is needed, so recovery also works on closed communities.
    let can_restore = if recovering && allow_restore {
        true
    } else if allow_restore {
        if !crate::managed_agents::load_managed_agents(app)?.is_empty() {
            false
        } else {
            let store =
                crate::secret_store::SecretStore::shared(crate::app_state::keyring_service());
            store.load("identity.bootstrap")?.as_deref() == Some(local_pubkey.to_hex().as_str())
        }
    } else {
        false
    };
    let keys = resolve_backup(
        &state.media_fetch_client,
        origin,
        &tokens.access,
        &tokens.principal_id,
        &local,
        can_restore,
        |keys| {
            if recovering {
                return Err(
                    "This Google account has no backup. Your lost identity was not replaced."
                        .into(),
                );
            }
            persist_pending(&state, keys)
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
            != local_pubkey
    {
        return Err(
            "Your community or local identity changed during Google sign-in. Try again.".into(),
        );
    }
    let store = crate::secret_store::SecretStore::shared(crate::app_state::keyring_service());
    if keys.public_key() != local_pubkey && !recovering {
        if store.load("identity.bootstrap")?.as_deref() != Some(local_pubkey.to_hex().as_str())
            || !crate::managed_agents::load_managed_agents(app)?.is_empty()
        {
            return Err(
                "Your local identity was used during sign-in. It and its agents were preserved."
                    .into(),
            );
        }
    }
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "identity storage is unavailable")?;
    if recovering {
        let known = super::KeyringRefreshStore
            .signing_pubkey(origin)?
            .or_else(|| state.token_auth.signing_pubkey(origin));
        let agents = crate::managed_agents::load_managed_agents(app)?;
        verify_recovered_owner(
            keys.public_key(),
            known,
            agents
                .iter()
                .map(|agent| (agent.pubkey.as_str(), agent.auth_tag.as_deref())),
        )?;
    }
    crate::app_state::prepare_recovered_identity(&data_dir, &keys)?;
    let nsec = Zeroizing::new(
        keys.secret_key()
            .to_bech32()
            .map_err(|_| "could not encode signing key")?,
    );
    let mut entries = std::collections::HashMap::from([
        ("identity".to_owned(), nsec.to_string()),
        (binding_key(origin), keys.public_key().to_hex()),
        (super::refresh_key(origin), tokens.refresh.to_string()),
        ("identity.bootstrap".to_owned(), String::new()),
    ]);
    let result = store.store_all(&entries);
    for value in entries.values_mut() {
        zeroize::Zeroize::zeroize(value);
    }
    result?;
    if !store.verify_stored_raw("identity", &nsec)? {
        return Err("OS secure storage verification failed; retry recovery.".into());
    }
    let pubkey = keys.public_key();
    *state.keys.lock().map_err(|_| "identity storage is busy")? = keys;
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

fn verify_recovered_owner<'a>(
    recovered: PublicKey,
    known: Option<PublicKey>,
    agents: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Result<(), String> {
    const MISMATCH: &str = "This Google account backs up a different signing identity. Your existing agents were preserved. Sign in with the original Google account or import your original key.";
    if known.is_some_and(|key| key != recovered) {
        return Err(MISMATCH.into());
    }
    let mut has_agents = false;
    let mut verified_owner = known.is_some();
    for (agent, tag) in agents {
        has_agents = true;
        let Some(tag) = tag else { continue };
        let agent = PublicKey::from_hex(agent)
            .map_err(|_| "An existing agent identity is invalid. Import your original key to recover without changing its record.")?;
        let owner = buzz_sdk_pkg::nip_oa::verify_auth_tag(tag, &agent)
            .map_err(|_| "An existing agent owner attestation cannot be verified. Import your original key to recover; your agents were preserved.")?;
        if owner != recovered {
            return Err(MISMATCH.into());
        }
        verified_owner = true;
    }
    if has_agents && !verified_owner {
        return Err("The original owner cannot be verified: the backup binding and signed agent owner attestations are unavailable. Import your original key using the existing key recovery option; Google recovery has preserved your agents.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
