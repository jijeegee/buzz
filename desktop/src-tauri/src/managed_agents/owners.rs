//! Who owns each local agent.
//!
//! Several accounts can be signed in on this computer and every account's
//! agents keep running whichever account is active, so an agent's owner must
//! come from the agent itself, never from the active account. It is the owner
//! signed into the agent's NIP-OA `auth_tag`. Older agents created without one
//! get their owner written once into `agent-owners.json` (next to the agent
//! store) by [`migrate_owners`], from the account that was active when this
//! computer moved to multi-account.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use nostr::PublicKey;
use tauri::AppHandle;

use super::ManagedAgentRecord;

/// File (next to the agent store) holding recorded owners.
pub(crate) const FILE_NAME: &str = "agent-owners.json";

fn owners_path<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    super::managed_agents_base_dir(app)
        .ok()
        .map(|dir| dir.join(FILE_NAME))
}

fn read_owners(path: &Path) -> BTreeMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The owner signed into `record`'s attestation, when it verifies.
pub(crate) fn attested_owner_hex(record: &ManagedAgentRecord) -> Option<String> {
    let tag = record.auth_tag.as_deref()?;
    let agent = PublicKey::from_hex(&record.pubkey).ok()?;
    buzz_sdk_pkg::nip_oa::verify_auth_tag(tag, &agent)
        .ok()
        .map(|owner| owner.to_hex())
}

/// The owner of `record`: its attestation, else the owner recorded for it at
/// migration. `None` for an agent with neither (callers fall back to their
/// legacy behaviour).
pub(crate) fn agent_owner_hex<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
) -> Option<String> {
    attested_owner_hex(record).or_else(|| {
        let path = owners_path(app)?;
        read_owners(&path).remove(&record.pubkey.to_ascii_lowercase())
    })
}

/// Record `owner` for the agent `agent_pubkey` (at creation, or the first
/// time a server bot's owner is learned). An existing entry is kept.
pub(crate) fn record_agent_owner<R: tauri::Runtime>(
    app: &AppHandle<R>,
    agent_pubkey: &str,
    owner: &PublicKey,
) -> Result<(), String> {
    let path = owners_path(app).ok_or("agent storage is unavailable")?;
    record_owner_at(&path, agent_pubkey, owner)
}

fn record_owner_at(path: &Path, agent_pubkey: &str, owner: &PublicKey) -> Result<(), String> {
    let _lock = OWNERS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut recorded = read_owners(path);
    let key = agent_pubkey.to_ascii_lowercase();
    if recorded.contains_key(&key) {
        return Ok(());
    }
    recorded.insert(key, owner.to_hex());
    let json = serde_json::to_vec_pretty(&recorded).map_err(|e| e.to_string())?;
    super::storage::atomic_write_json(path, &json)
}

/// Serialises writes to the owners file in this process.
static OWNERS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The owner a spawn runs for: the agent's own owner, or for a server bot the
/// principal of the token account its bot token belongs to (recorded on first
/// use). Never the active account: an agent with neither is refused.
pub(crate) fn spawn_owner_hex<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
) -> Result<String, String> {
    use tauri::Manager;
    let bot_principal = record.bot_origin.as_deref().and_then(|origin| {
        token_account_principal(
            &app.state::<crate::app_state::AppState>().token_auth,
            origin,
        )
    });
    let (owner, learned) =
        resolve_spawn_owner(record, agent_owner_hex(app, record), bot_principal)?;
    if let Some(principal) = learned {
        record_agent_owner(app, &record.pubkey, &principal)?;
    }
    Ok(owner)
}

/// The principal of `origin`'s token account, which server bots there belong
/// to. `None` unless the origin is in token mode: a Google key-backup sign-in
/// also holds an active session, but its principal is the Google login, not
/// the account that owns anything.
pub(crate) fn token_account_principal(
    auth: &crate::auth::TokenAuthState,
    origin: &str,
) -> Option<PublicKey> {
    match auth.mode(origin) {
        crate::auth::CredentialMode::Token(session) => Some(session.principal),
        _ => None,
    }
}

/// The owners of `origin`'s token account, for stopping its agents when it
/// signs out: its live principal, plus the owners recorded for the server's
/// bots (learned in token mode). The recorded ones do not depend on the
/// session state, so a revoked, expired or still-restoring session still
/// finds its agents.
pub(crate) fn token_account_owners<R: tauri::Runtime>(
    app: &AppHandle<R>,
    origin: &str,
) -> Vec<PublicKey> {
    use tauri::Manager;
    let live = token_account_principal(
        &app.state::<crate::app_state::AppState>().token_auth,
        origin,
    );
    let records = super::storage::load_managed_agents_without_keys(app).unwrap_or_default();
    bot_owners(live, &records, origin, |record| {
        agent_owner_hex(app, record)
    })
}

/// [`token_account_owners`] without the app. Pure.
fn bot_owners(
    live: Option<PublicKey>,
    records: &[ManagedAgentRecord],
    origin: &str,
    owner_of: impl Fn(&ManagedAgentRecord) -> Option<String>,
) -> Vec<PublicKey> {
    let mut owners: Vec<PublicKey> = live.into_iter().collect();
    let recorded = records
        .iter()
        .filter(|record| record.bot_origin.as_deref() == Some(origin))
        .filter_map(&owner_of)
        .filter_map(|hex| PublicKey::from_hex(&hex).ok());
    for owner in recorded {
        if !owners.contains(&owner) {
            owners.push(owner);
        }
    }
    owners
}

/// The start gate: an agent runs only while its owner is signed in on this
/// computer for the agent's server. A server bot not on the account list is
/// refused only by an explicit sign-out marker (its token decides the rest).
pub(crate) fn ensure_owner_signed_in<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    relay_url: &str,
    owner_hex: &str,
) -> Result<(), String> {
    use crate::auth::accounts::OwnerHere;
    let owner = PublicKey::from_hex(owner_hex).map_err(|_| "invalid agent owner")?;
    let here = crate::auth::accounts::owner_here_for(app, relay_url, &owner);
    gate_verdict(record, here).inspect_err(|error| {
        if here == OwnerHere::Unlisted {
            eprintln!(
                "buzz-desktop: agents: owner {} of {} is not an account on this computer",
                &owner_hex[..owner_hex.len().min(8)],
                record.pubkey
            );
        } else {
            eprintln!("buzz-desktop: agents: {error}");
        }
    })
}

/// [`ensure_owner_signed_in`]'s decision. Pure.
fn gate_verdict(
    record: &ManagedAgentRecord,
    here: crate::auth::accounts::OwnerHere,
) -> Result<(), String> {
    use crate::auth::accounts::OwnerHere;
    match here {
        OwnerHere::SignedIn => Ok(()),
        OwnerHere::Unlisted if record.bot_origin.is_some() => Ok(()),
        OwnerHere::SignedOut => Err(format!(
            "agent {}'s account is signed out on this computer; it was not started.",
            record.pubkey
        )),
        OwnerHere::Unlisted => Err(format!(
            "agent {}'s account is not signed in on this computer; it was not started.",
            record.pubkey
        )),
    }
}

/// [`spawn_owner_hex`] without the app: `known` is the agent's own owner and
/// `bot_principal` the token principal for a server bot's origin. Returns the
/// owner, plus the principal to record when a bot's owner was just learned.
fn resolve_spawn_owner(
    record: &ManagedAgentRecord,
    known: Option<String>,
    bot_principal: Option<PublicKey>,
) -> Result<(String, Option<PublicKey>), String> {
    if let Some(owner) = known {
        return Ok((owner, None));
    }
    if record.bot_origin.is_some() {
        if let Some(principal) = bot_principal {
            return Ok((principal.to_hex(), Some(principal)));
        }
    }
    Err(format!(
        "agent {} has no known owner on this computer; it was not started.",
        record.pubkey
    ))
}

/// Agents (by pubkey) with no attested owner and no recorded owner.
fn unowned<'a>(
    records: &'a [ManagedAgentRecord],
    recorded: &BTreeMap<String, String>,
) -> impl Iterator<Item = String> + 'a {
    let recorded: Vec<String> = recorded.keys().cloned().collect();
    records
        .iter()
        .filter(|record| !record.pubkey.is_empty())
        // A server bot's owner is its token account's principal, recorded by
        // `spawn_owner_hex` on first use, never the local key passed here.
        .filter(|record| record.bot_origin.is_none())
        .filter(|record| attested_owner_hex(record).is_none())
        .map(|record| record.pubkey.to_ascii_lowercase())
        .filter(move |pubkey| !recorded.contains(pubkey))
}

/// Record `owner` for every agent that has no attested or recorded owner yet.
/// Returns how many were recorded.
pub(crate) fn record_missing_owners(
    path: &Path,
    records: &[ManagedAgentRecord],
    owner: &PublicKey,
) -> Result<usize, String> {
    let _lock = OWNERS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut recorded = read_owners(path);
    let missing: Vec<String> = unowned(records, &recorded).collect();
    if missing.is_empty() {
        return Ok(0);
    }
    for pubkey in &missing {
        recorded.insert(pubkey.clone(), owner.to_hex());
    }
    let json = serde_json::to_vec_pretty(&recorded).map_err(|e| e.to_string())?;
    super::storage::atomic_write_json(path, &json)?;
    Ok(missing.len())
}

/// One-time launch migration: older agents without an attestation get the
/// account active now as their owner, unless owners were already recorded
/// when the account list was created.
pub(crate) fn migrate_owners<R: tauri::Runtime>(app: &AppHandle<R>) {
    use tauri::Manager;
    if let Ok(data_dir) = app.path().app_data_dir() {
        crate::auth::accounts::record_owners_on_launch(&data_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(pubkey: &str, auth_tag: Option<String>) -> ManagedAgentRecord {
        let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
            "pubkey": pubkey,
            "name": "a",
            "relay_url": "wss://relay.test",
            "acp_command": "x",
            "agent_command": "x",
            "agent_args": [],
            "mcp_command": "x",
            "turn_timeout_seconds": 1,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        record.auth_tag = auth_tag;
        record
    }

    #[test]
    fn owners_come_from_the_attestation_else_the_migration_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let owner_a = nostr::Keys::generate();
        let owner_b = nostr::Keys::generate().public_key();
        let tagged = nostr::Keys::generate().public_key();
        let tag = buzz_sdk_pkg::nip_oa::compute_auth_tag(&owner_a, &tagged, "kind=9").unwrap();
        let legacy = nostr::Keys::generate().public_key().to_hex();
        let records = vec![record(&tagged.to_hex(), Some(tag)), record(&legacy, None)];
        assert_eq!(
            attested_owner_hex(&records[0]),
            Some(owner_a.public_key().to_hex())
        );
        assert_eq!(attested_owner_hex(&records[1]), None);
        // Migration records only the unattested agent, under the given owner.
        assert_eq!(record_missing_owners(&path, &records, &owner_b).unwrap(), 1);
        let recorded = read_owners(&path);
        assert_eq!(recorded.get(&legacy), Some(&owner_b.to_hex()));
        assert!(!recorded.contains_key(&tagged.to_hex()));
        // Running again with another (now active) account changes nothing.
        let later = nostr::Keys::generate().public_key();
        assert_eq!(record_missing_owners(&path, &records, &later).unwrap(), 0);
        assert_eq!(read_owners(&path).get(&legacy), Some(&owner_b.to_hex()));
    }

    #[test]
    fn a_recorded_owner_is_never_overwritten_by_a_later_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let agent = nostr::Keys::generate().public_key().to_hex();
        let creator = nostr::Keys::generate().public_key();
        let later = nostr::Keys::generate().public_key();
        record_owner_at(&path, &agent, &creator).unwrap();
        record_owner_at(&path, &agent, &later).unwrap();
        assert_eq!(read_owners(&path).get(&agent), Some(&creator.to_hex()));
    }

    #[test]
    fn server_bots_are_never_recorded_under_the_local_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let bot = nostr::Keys::generate().public_key().to_hex();
        let mut bot_record = record(&bot, None);
        bot_record.bot_origin = Some("https://relay.test".into());
        let local = nostr::Keys::generate().public_key();
        assert_eq!(
            record_missing_owners(&path, std::slice::from_ref(&bot_record), &local).unwrap(),
            0
        );
        assert!(!read_owners(&path).contains_key(&bot));
        // Its first spawn learns the token principal instead.
        let principal = nostr::Keys::generate().public_key();
        let (owner, learned) = resolve_spawn_owner(&bot_record, None, Some(principal)).unwrap();
        assert_eq!(owner, principal.to_hex());
        assert_eq!(learned, Some(principal));
    }

    #[test]
    fn an_agent_without_an_owner_is_refused_not_given_the_active_account() {
        let agent = record(&nostr::Keys::generate().public_key().to_hex(), None);
        let principal = nostr::Keys::generate().public_key();
        // A token principal only counts for a server bot.
        assert!(resolve_spawn_owner(&agent, None, Some(principal)).is_err());
        assert!(resolve_spawn_owner(&agent, None, None).is_err());
        let mut bot = agent.clone();
        bot.bot_origin = Some("https://relay.test".into());
        assert!(resolve_spawn_owner(&bot, None, None).is_err());
        let known = nostr::Keys::generate().public_key().to_hex();
        assert_eq!(
            resolve_spawn_owner(&agent, Some(known.clone()), Some(principal)).unwrap(),
            (known, None)
        );
    }

    #[test]
    fn a_bot_never_learns_its_owner_from_a_google_key_backup_session() {
        let origin = "https://relay.test";
        let auth = crate::auth::TokenAuthState::default();
        let principal = nostr::Keys::generate().public_key();
        auth.set(
            origin,
            crate::auth::OriginAuth::Active(crate::auth::UserSession {
                principal,
                device_id: None,
                access: zeroize::Zeroizing::new("access".into()),
                refresh: zeroize::Zeroizing::new("refresh".into()),
                access_issued_at: 0,
                access_expires_at: i64::MAX,
            }),
        );
        // A token account: its principal owns the server bots.
        assert_eq!(token_account_principal(&auth, origin), Some(principal));
        // The same session under a Google key backup: its principal is the
        // Google login, never an owner.
        auth.set_key_backup(origin, nostr::Keys::generate().public_key());
        assert_eq!(token_account_principal(&auth, origin), None);
        assert_eq!(token_account_principal(&auth, "https://other.test"), None);
    }

    #[test]
    fn a_token_account_finds_its_bots_owners_without_a_live_session() {
        let origin = "https://relay.test";
        let (principal, other) = (
            nostr::Keys::generate().public_key(),
            nostr::Keys::generate().public_key(),
        );
        let mut bot = record(&nostr::Keys::generate().public_key().to_hex(), None);
        bot.bot_origin = Some(origin.into());
        let mut elsewhere = bot.clone();
        elsewhere.pubkey = nostr::Keys::generate().public_key().to_hex();
        elsewhere.bot_origin = Some("https://other.test".into());
        let local = record(&nostr::Keys::generate().public_key().to_hex(), None);
        let records = vec![bot.clone(), elsewhere.clone(), local];
        let owner_of = |record: &ManagedAgentRecord| {
            if record.pubkey == bot.pubkey {
                Some(principal.to_hex())
            } else {
                Some(other.to_hex())
            }
        };
        // Revoked, expired or restoring: no live principal, the recorded
        // owner of this server's bots still is one (not another server's
        // bot, not a local agent).
        assert_eq!(
            bot_owners(None, &records, origin, owner_of),
            vec![principal]
        );
        // Active: the live principal, without duplicates.
        assert_eq!(
            bot_owners(Some(principal), &records, origin, owner_of),
            vec![principal]
        );
        assert!(bot_owners(None, &[], origin, owner_of).is_empty());
    }

    #[test]
    fn the_gate_refuses_only_an_owner_signed_out_or_unknown_here() {
        use crate::auth::accounts::OwnerHere;
        let agent = record(&nostr::Keys::generate().public_key().to_hex(), None);
        assert!(gate_verdict(&agent, OwnerHere::SignedIn).is_ok());
        assert!(gate_verdict(&agent, OwnerHere::SignedOut).is_err());
        assert!(gate_verdict(&agent, OwnerHere::Unlisted).is_err());
        // A server bot's owner is its token account, not a listed account:
        // only an explicit sign-out marker refuses it.
        let mut bot = agent.clone();
        bot.bot_origin = Some("https://relay.test".into());
        assert!(gate_verdict(&bot, OwnerHere::Unlisted).is_ok());
        assert!(gate_verdict(&bot, OwnerHere::SignedOut).is_err());
    }
}
