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

/// The account active on this computer, whose agents and personas the
/// screens show.
pub(crate) fn active_owner_hex<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<String> {
    use tauri::Manager;
    app.state::<crate::app_state::AppState>()
        .current_identity_pubkey()
        .ok()
        .map(|key| key.to_hex())
}

/// The owner `record` is shown under: its own owner, else for a server bot
/// the principal of its server's token account (before its first spawn
/// records it). `None` when unknown.
pub(crate) fn visible_owner_hex<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
) -> Option<String> {
    use tauri::Manager;
    agent_owner_hex(app, record).or_else(|| {
        let origin = record.bot_origin.as_deref()?;
        token_account_principal(
            &app.state::<crate::app_state::AppState>().token_auth,
            origin,
        )
        .map(|principal| principal.to_hex())
    })
}

/// Whether an agent owned by `owner` is shown to the account `me`: only its
/// own owner sees it. An agent whose owner is unknown is shown to nobody.
pub(crate) fn owned_by_viewer(owner: Option<&str>, me: Option<&str>) -> bool {
    matches!((owner, me), (Some(owner), Some(me)) if owner.eq_ignore_ascii_case(me))
}

/// Owners-file key under which a persona's owner is recorded. Personas carry
/// no key of their own, so they are recorded by id.
fn definition_key(id: &str) -> String {
    format!("definition:{id}")
}

/// Owners-file entry marking that personas existing before owners were
/// recorded for them have been given one.
const DEFINITIONS_MIGRATED: &str = "definitions-migrated";

/// Record an owner for every persona that has none, and return each
/// persona's owner (by id). Built-in personas ship with the app and stay
/// everyone's. The first run gives existing personas to the account that
/// owns most local agents (the account this computer was used with before
/// several could sign in; the active account when there are no agents);
/// after that a new persona belongs to the account active when it is saved.
pub(crate) fn ensure_definition_owners<R: tauri::Runtime>(
    app: &AppHandle<R>,
    definitions: &[super::AgentDefinition],
) -> BTreeMap<String, String> {
    let Some(path) = owners_path(app) else {
        return BTreeMap::new();
    };
    let active = active_owner_hex(app);
    let first_run = !read_owners(&path).contains_key(DEFINITIONS_MIGRATED);
    let previous = if first_run {
        let agents = super::storage::load_managed_agents_without_keys(app).unwrap_or_default();
        main_owner(
            agents
                .iter()
                .filter_map(|record| agent_owner_hex(app, record)),
        )
    } else {
        None
    };
    match record_definition_owners(&path, definitions, previous.or(active).as_deref()) {
        Ok(owners) => owners,
        Err(error) => {
            eprintln!("buzz-desktop: agents: persona owners not recorded: {error}");
            definition_owners(&read_owners(&path), definitions)
        }
    }
}

/// The owner of most of `owners` (ties go to the one seen first). Pure.
fn main_owner(owners: impl Iterator<Item = String>) -> Option<String> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for owner in owners {
        let owner = owner.to_ascii_lowercase();
        match counts.iter_mut().find(|(seen, _)| *seen == owner) {
            Some((_, count)) => *count += 1,
            None => counts.push((owner, 1)),
        }
    }
    let mut best: Option<(String, usize)> = None;
    for (owner, count) in counts {
        if best.as_ref().is_none_or(|(_, top)| count > *top) {
            best = Some((owner, count));
        }
    }
    best.map(|(owner, _)| owner)
}

/// [`ensure_definition_owners`] on the owners file at `path`: records `owner`
/// for every non-built-in persona without one (none when `owner` is `None`)
/// and marks the first run done.
fn record_definition_owners(
    path: &Path,
    definitions: &[super::AgentDefinition],
    owner: Option<&str>,
) -> Result<BTreeMap<String, String>, String> {
    let _lock = OWNERS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut recorded = read_owners(path);
    let mut changed = false;
    if let Some(owner) = owner {
        for definition in definitions.iter().filter(|d| !d.is_builtin) {
            let key = definition_key(&definition.id);
            if !recorded.contains_key(&key) {
                recorded.insert(key, owner.to_ascii_lowercase());
                changed = true;
            }
        }
        if !recorded.contains_key(DEFINITIONS_MIGRATED) {
            recorded.insert(DEFINITIONS_MIGRATED.into(), "true".into());
            changed = true;
        }
    }
    if changed {
        let json = serde_json::to_vec_pretty(&recorded).map_err(|e| e.to_string())?;
        super::storage::atomic_write_json(path, &json)?;
    }
    Ok(definition_owners(&recorded, definitions))
}

/// Each persona's recorded owner, by id. Pure.
fn definition_owners(
    recorded: &BTreeMap<String, String>,
    definitions: &[super::AgentDefinition],
) -> BTreeMap<String, String> {
    definitions
        .iter()
        .filter_map(|definition| {
            recorded
                .get(&definition_key(&definition.id))
                .map(|owner| (definition.id.clone(), owner.clone()))
        })
        .collect()
}

/// Whether the account `me` sees `definition`: a built-in persona nobody has
/// claimed is everyone's; any other persona only its owner's.
pub(crate) fn definition_visible(
    definition: &super::AgentDefinition,
    owners: &BTreeMap<String, String>,
    me: Option<&str>,
) -> bool {
    match owners.get(&definition.id) {
        Some(owner) => owned_by_viewer(Some(owner), me),
        None => definition.is_builtin,
    }
}

/// Owners-file key marking the built-in persona `id` as removed from the
/// account `account`'s agents. Built-ins are one record shared by every
/// account, so each account's "in My Agents" choice is kept here instead of
/// in the shared `is_active`.
fn inactive_key(id: &str, account: &str) -> String {
    format!("inactive:{id}:{}", account.to_ascii_lowercase())
}

/// Mark the built-in persona `id` as removed from (`inactive`) or back in the
/// account `account`'s agents.
pub(crate) fn set_builtin_inactive<R: tauri::Runtime>(
    app: &AppHandle<R>,
    id: &str,
    account: &str,
    inactive: bool,
) -> Result<(), String> {
    let path = owners_path(app).ok_or("agent storage is unavailable")?;
    set_builtin_inactive_at(&path, id, account, inactive)
}

fn set_builtin_inactive_at(
    path: &Path,
    id: &str,
    account: &str,
    inactive: bool,
) -> Result<(), String> {
    let _lock = OWNERS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut recorded = read_owners(path);
    let key = inactive_key(id, account);
    let changed = if inactive {
        recorded.insert(key, "true".into()).is_none()
    } else {
        recorded.remove(&key).is_some()
    };
    if !changed {
        return Ok(());
    }
    let json = serde_json::to_vec_pretty(&recorded).map_err(|e| e.to_string())?;
    super::storage::atomic_write_json(path, &json)
}

/// Show each built-in persona as the active account sees it: one it removed
/// from its agents is inactive for it, whatever the other accounts chose.
pub(crate) fn apply_account_activation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    definitions: &mut [super::AgentDefinition],
) {
    let recorded = owners_path(app)
        .map(|path| read_owners(&path))
        .unwrap_or_default();
    account_activation(&recorded, definitions, active_owner_hex(app).as_deref());
}

/// [`apply_account_activation`] without the app. Pure.
fn account_activation(
    recorded: &BTreeMap<String, String>,
    definitions: &mut [super::AgentDefinition],
    me: Option<&str>,
) {
    let Some(me) = me else { return };
    for definition in definitions.iter_mut().filter(|d| d.is_builtin) {
        if recorded.contains_key(&inactive_key(&definition.id, me)) {
            definition.is_active = false;
        }
    }
}

/// The account's own copy of the built-in persona `builtin`, made when the
/// account edits it, so the edit never reaches the other accounts: a new id,
/// not built-in, active, and not shared (the edit decides the rest).
pub(crate) fn builtin_copy(builtin: &super::AgentDefinition, now: &str) -> super::AgentDefinition {
    let mut copy = builtin.clone();
    copy.id = uuid::Uuid::new_v4().to_string();
    copy.is_builtin = false;
    copy.is_active = true;
    copy.shared = false;
    copy.created_at = now.to_string();
    copy.updated_at = now.to_string();
    copy
}

/// Whether an agent of the account `me` is made from the persona `id`. Only
/// these keep the account from removing a built-in from its agents; another
/// account's agents made from it keep using it.
pub(crate) fn account_uses_persona(
    records: &[ManagedAgentRecord],
    id: &str,
    me: Option<&str>,
    owner_of: impl Fn(&ManagedAgentRecord) -> Option<String>,
) -> bool {
    records.iter().any(|record| {
        record.persona_id.as_deref() == Some(id) && owned_by_viewer(owner_of(record).as_deref(), me)
    })
}

/// Move the account `me`'s agents made from the built-in persona `from` to
/// its copy `to`; other accounts' agents keep the built-in. Returns the moved
/// agents' pubkeys.
pub(crate) fn move_account_agents(
    records: &mut [ManagedAgentRecord],
    from: &str,
    to: &str,
    me: &str,
    owner_of: impl Fn(&ManagedAgentRecord) -> Option<String>,
) -> Vec<String> {
    let mut moved = Vec::new();
    for record in records
        .iter_mut()
        .filter(|record| record.persona_id.as_deref() == Some(from))
    {
        if owned_by_viewer(owner_of(record).as_deref(), Some(me)) {
            record.persona_id = Some(to.to_string());
            moved.push(record.pubkey.clone());
        }
    }
    moved
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

    fn persona(id: &str, builtin: bool) -> crate::managed_agents::AgentDefinition {
        let mut stored = record("", None);
        stored.slug = Some(id.into());
        stored.is_builtin = builtin;
        stored.to_definition_view().unwrap()
    }

    #[test]
    fn personas_belong_to_one_account_and_built_ins_to_everyone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let a = nostr::Keys::generate().public_key().to_hex();
        let b = nostr::Keys::generate().public_key().to_hex();
        let existing = vec![persona("mine", false), persona("builtin:fizz", true)];
        // First run: existing personas go to the given (previous) account.
        let owners = record_definition_owners(&path, &existing, Some(&a)).unwrap();
        assert_eq!(owners.get("mine"), Some(&a));
        assert!(!owners.contains_key("builtin:fizz"));
        // B makes a persona later: it is B's, and A's stays A's.
        let mut later = existing.clone();
        later.push(persona("theirs", false));
        let owners = record_definition_owners(&path, &later, Some(&b)).unwrap();
        assert_eq!(owners.get("mine"), Some(&a));
        assert_eq!(owners.get("theirs"), Some(&b));
        let seen = |me: &str| -> Vec<String> {
            later
                .iter()
                .filter(|d| definition_visible(d, &owners, Some(me)))
                .map(|d| d.id.clone())
                .collect()
        };
        assert_eq!(seen(&a), vec!["mine", "builtin:fizz"]);
        assert_eq!(seen(&b), vec!["builtin:fizz", "theirs"]);
        // Nobody signed in: built-ins only.
        assert!(later
            .iter()
            .filter(|d| definition_visible(d, &owners, None))
            .all(|d| d.is_builtin));
    }

    #[test]
    fn a_persona_without_an_owner_is_shown_to_nobody() {
        let owners = BTreeMap::new();
        let me = nostr::Keys::generate().public_key().to_hex();
        assert!(!definition_visible(
            &persona("x", false),
            &owners,
            Some(&me)
        ));
        // Nothing is recorded (or marked migrated) without an owner to give.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        assert!(
            record_definition_owners(&path, &[persona("x", false)], None)
                .unwrap()
                .is_empty()
        );
        assert!(!read_owners(&path).contains_key(DEFINITIONS_MIGRATED));
    }

    #[test]
    fn an_agent_is_listed_only_to_its_owner() {
        let a = "a".repeat(64);
        assert!(owned_by_viewer(Some(&a), Some(&a.to_uppercase())));
        assert!(!owned_by_viewer(Some(&a), Some(&"b".repeat(64))));
        assert!(!owned_by_viewer(None, Some(&a)));
        assert!(!owned_by_viewer(Some(&a), None));
    }

    #[test]
    fn the_previous_account_is_the_one_owning_most_agents() {
        let owners = ["b", "a", "A", "b", "a"].map(String::from);
        assert_eq!(main_owner(owners.into_iter()), Some("a".into()));
        assert_eq!(
            main_owner(["b", "a"].map(String::from).into_iter()),
            Some("b".into()),
            "a tie goes to the one seen first"
        );
        assert_eq!(main_owner(std::iter::empty()), None);
    }

    /// Dry run of the persona-owner migration on a copy of an agent store:
    /// `BUZZ_OWNERS_DRY_RUN_DIR` = a copied `agents/` directory (written to).
    #[test]
    #[ignore]
    fn dry_run_persona_owner_migration_on_a_store_copy() {
        let dir = std::path::PathBuf::from(std::env::var("BUZZ_OWNERS_DRY_RUN_DIR").unwrap());
        let store: Vec<ManagedAgentRecord> =
            serde_json::from_slice(&std::fs::read(dir.join("managed-agents.json")).unwrap())
                .unwrap();
        let path = dir.join(FILE_NAME);
        let recorded = read_owners(&path);
        let (definitions, agents): (Vec<_>, Vec<_>) =
            store.iter().partition(|record| record.pubkey.is_empty());
        let agent_owners: Vec<String> = agents
            .iter()
            .filter_map(|record| {
                attested_owner_hex(record)
                    .or_else(|| recorded.get(&record.pubkey.to_ascii_lowercase()).cloned())
            })
            .collect();
        for record in &agents {
            let owner = attested_owner_hex(record)
                .or_else(|| recorded.get(&record.pubkey.to_ascii_lowercase()).cloned());
            println!(
                "agent {} {:?} -> {:?}",
                &record.pubkey[..8],
                record.name,
                owner.as_deref().map(|o| &o[..8])
            );
        }
        let previous = main_owner(agent_owners.into_iter());
        println!("previous account: {:?}", previous);
        let definitions: Vec<_> = definitions
            .iter()
            .filter_map(|record| record.to_definition_view())
            .collect();
        let owners = record_definition_owners(&path, &definitions, previous.as_deref()).unwrap();
        for definition in &definitions {
            println!(
                "persona {} {:?} builtin={} -> {:?}",
                definition.id,
                definition.display_name,
                definition.is_builtin,
                owners.get(&definition.id).map(|o| &o[..8])
            );
        }
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

    /// An agent made from the persona `persona_id`.
    fn agent_of(persona_id: &str) -> ManagedAgentRecord {
        let mut agent = record(&nostr::Keys::generate().public_key().to_hex(), None);
        agent.persona_id = Some(persona_id.into());
        agent
    }

    #[test]
    fn an_edited_built_in_becomes_the_editing_accounts_own_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let a = nostr::Keys::generate().public_key().to_hex();
        let b = nostr::Keys::generate().public_key().to_hex();
        let fizz = persona("builtin:fizz", true);
        let a_agent = agent_of("builtin:fizz");
        let b_agent = agent_of("builtin:fizz");
        let owner_of = |record: &ManagedAgentRecord| {
            Some(if record.pubkey == a_agent.pubkey {
                a.clone()
            } else {
                b.clone()
            })
        };
        record_definition_owners(&path, std::slice::from_ref(&fizz), Some(&a)).unwrap();

        // A edits Fizz: the edit lands on A's copy, the built-in stays as is.
        let mut copy = builtin_copy(&fizz, "2026-10-12T00:00:00Z");
        copy.system_prompt = "A's Fizz".into();
        let definitions = vec![fizz.clone(), copy.clone()];
        let owners = record_definition_owners(&path, &definitions, Some(&a)).unwrap();
        set_builtin_inactive_at(&path, "builtin:fizz", &a, true).unwrap();
        let mut records = vec![a_agent.clone(), b_agent.clone()];
        let moved = move_account_agents(&mut records, "builtin:fizz", &copy.id, &a, owner_of);

        assert_ne!(copy.id, fizz.id);
        assert!(!copy.is_builtin && copy.is_active && !copy.shared);
        assert_eq!(owners.get(&copy.id), Some(&a));
        let view = |me: &str| -> Vec<(String, bool, String)> {
            let mut seen: Vec<_> = definitions
                .iter()
                .filter(|d| definition_visible(d, &owners, Some(me)))
                .cloned()
                .collect();
            account_activation(&read_owners(&path), &mut seen, Some(me));
            seen.into_iter()
                .map(|d| (d.id, d.is_active, d.system_prompt))
                .collect()
        };
        // A: its copy is active, the built-in is out of its agents.
        assert_eq!(
            view(&a),
            vec![
                (fizz.id.clone(), false, fizz.system_prompt.clone()),
                (copy.id.clone(), true, "A's Fizz".into()),
            ]
        );
        // B: the untouched built-in, still active; A's copy is not B's.
        assert_eq!(
            view(&b),
            vec![(fizz.id.clone(), true, fizz.system_prompt.clone())]
        );
        // Only A's agent moved to the copy.
        assert_eq!(moved, vec![a_agent.pubkey.clone()]);
        assert_eq!(records[0].persona_id.as_deref(), Some(copy.id.as_str()));
        assert_eq!(records[1].persona_id.as_deref(), Some("builtin:fizz"));
    }

    #[test]
    fn removing_a_built_in_is_one_accounts_choice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let a = nostr::Keys::generate().public_key().to_hex();
        let b = nostr::Keys::generate().public_key().to_hex();
        let view = |me: &str| {
            let mut seen = vec![persona("builtin:fizz", true), persona("mine", false)];
            seen[1].is_active = true;
            account_activation(&read_owners(&path), &mut seen, Some(me));
            seen.iter().map(|d| d.is_active).collect::<Vec<_>>()
        };
        set_builtin_inactive_at(&path, "builtin:fizz", &a, true).unwrap();
        assert_eq!(view(&a), vec![false, true]);
        assert_eq!(view(&b), vec![true, true]);
        // Marking it twice changes nothing; bringing it back clears it.
        set_builtin_inactive_at(&path, "builtin:fizz", &a, true).unwrap();
        set_builtin_inactive_at(&path, "builtin:fizz", &a.to_ascii_uppercase(), false).unwrap();
        assert_eq!(view(&a), vec![true, true]);
        assert!(!read_owners(&path)
            .keys()
            .any(|key| key.starts_with("inactive:")));
    }

    #[test]
    fn only_the_accounts_own_agents_keep_it_from_removing_a_built_in() {
        let a = nostr::Keys::generate().public_key().to_hex();
        let b = nostr::Keys::generate().public_key().to_hex();
        let b_agent = agent_of("builtin:fizz");
        let owner_of = |_: &ManagedAgentRecord| Some(b.clone());
        let records = vec![b_agent];
        // B's agent uses Fizz: A may still remove it, B may not.
        assert!(!account_uses_persona(
            &records,
            "builtin:fizz",
            Some(&a),
            owner_of
        ));
        assert!(account_uses_persona(
            &records,
            "builtin:fizz",
            Some(&b),
            owner_of
        ));
        assert!(!account_uses_persona(
            &records,
            "builtin:honey",
            Some(&b),
            owner_of
        ));
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
