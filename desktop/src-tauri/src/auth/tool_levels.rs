//! Owner tools (AI tools S1): the relay's per-agent switch, `full` (on) or
//! `limited` (off), set only by the owner from a proven device under
//! `/auth/agents/…`. Nothing uses it to grant rights yet (S2).
//!
//! The app writes it:
//! - before every `respond_to` save, with the class the agent answers after
//!   the save: when the level is on, a failed call fails the save; switching
//!   to anyone sends off (the relay refuses on + anyone);
//! - once by default, only over a level never set: after a registration to
//!   this device and in one launch sweep (owner-only on, anything else off);
//! - never for server bots, which have no key registration.
//!
//! Deleting an agent revokes its key as the last server step, only from the
//! device hosting it, and never from a Dev data dir or a demo build.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use zeroize::Zeroizing;

use super::api::{self, ApiError};
use crate::app_state::AppState;
use crate::managed_agents::{ManagedAgentRecord, RespondTo};

/// Event the UI turns into the one-time notice (Q1a).
pub(crate) const DEFAULTED_EVENT: &str = "owner-tools-defaulted";

/// Owner tools on (`full`) or off (`limited`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolLevel {
    Full,
    Limited,
}

impl ToolLevel {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Limited => "limited",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "limited" => Some(Self::Limited),
            _ => None,
        }
    }
}

/// The default for a level never set: on only for an owner-only agent. Pure.
pub(crate) fn default_level(class: RespondTo) -> ToolLevel {
    match class {
        RespondTo::OwnerOnly => ToolLevel::Full,
        RespondTo::Allowlist | RespondTo::Anyone => ToolLevel::Limited,
    }
}

/// An agent anyone can talk to never has owner tools. Pure.
pub(crate) fn effective_level(level: ToolLevel, class: RespondTo) -> ToolLevel {
    if class == RespondTo::Anyone {
        ToolLevel::Limited
    } else {
        level
    }
}

/// The class a running agent of this build answers (the owner-only build
/// pins owner-only), as sent to the relay.
pub(crate) fn effective_class(record: &ManagedAgentRecord) -> RespondTo {
    crate::managed_agents::projected_access_with_policy(
        record,
        crate::managed_agents::owner_only_access_build(),
    )
    .0
}

/// What the relay said about one agent: `None` unknown (not answered, or
/// not registered), `Some(None)` never set, `Some(Some(level))` set.
pub(crate) type Reported = Option<Option<ToolLevel>>;

fn reported_of(row: Option<&api::AgentToolLevel>) -> Reported {
    row.map(|row| row.level.as_deref().and_then(ToolLevel::parse))
}

/// The route call a `respond_to` save makes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SaveWrite {
    pub level: ToolLevel,
    /// The save fails when this call fails: the level is on, or the owner
    /// changed it in this save.
    pub must_succeed: bool,
    /// The owner changed the level in this save. Only then does a stale
    /// class (`respond_class_stale`) stop the save; otherwise the relay's
    /// 30177 sync moves the class and the save goes on.
    pub owner_change: bool,
}

/// The call before a save to `class`: the owner's choice when they changed
/// it, else the reported level kept (off for anyone). Anyone always sends
/// off, even when the level couldn't be read, so full is never left behind.
/// Otherwise nothing when the level is unknown or never set (the default
/// write covers that). Pure.
pub(crate) fn save_write(
    reported: Reported,
    requested: Option<ToolLevel>,
    class: RespondTo,
) -> Option<SaveWrite> {
    if let Some(requested) = requested {
        return Some(SaveWrite {
            level: effective_level(requested, class),
            must_succeed: true,
            owner_change: true,
        });
    }
    if class == RespondTo::Anyone {
        return Some(SaveWrite {
            level: ToolLevel::Limited,
            must_succeed: false,
            owner_change: false,
        });
    }
    let level = effective_level(reported??, class);
    Some(SaveWrite {
        level,
        must_succeed: level == ToolLevel::Full,
        owner_change: false,
    })
}

/// A refused or failed tool-level call, in words. Pure.
pub(crate) fn tool_level_error(error: &ApiError) -> String {
    match error.code.as_deref() {
        Some("full_anyone_refused") => {
            "Owner tools can't be on while anyone can talk to this agent.".into()
        }
        Some("respond_class_stale") => {
            "Change owner tools on the computer that runs this agent.".into()
        }
        Some("agent_not_found") => {
            "Owner tools aren't available for this agent yet. Start it once, then try again.".into()
        }
        _ => format!("Couldn't save owner tools: {error}"),
    }
}

/// Whether a save goes on after its route call. Pure.
pub(crate) fn settle_write(
    write: SaveWrite,
    result: Result<String, ApiError>,
) -> Result<(), String> {
    match result {
        Ok(_) => Ok(()),
        Err(error)
            if write.must_succeed
                && (write.owner_change || error.code.as_deref() != Some("respond_class_stale")) =>
        {
            Err(tool_level_error(&error))
        }
        Err(error) => {
            eprintln!(
                "kuzz-desktop: owner tools: level not sent before save: {}",
                error.code.as_deref().unwrap_or("no answer")
            );
            Ok(())
        }
    }
}

/// Run the route call, then the save; a failed call that must succeed stops
/// the save.
pub(crate) async fn route_then_save<T, Call, Save, SaveFut>(
    write: Option<SaveWrite>,
    call: Call,
    save: Save,
) -> Result<T, String>
where
    Call: std::future::Future<Output = Result<String, ApiError>>,
    Save: FnOnce() -> SaveFut,
    SaveFut: std::future::Future<Output = Result<T, String>>,
{
    if let Some(write) = write {
        settle_write(write, call.await)?;
    }
    save().await
}

const NOT_AVAILABLE: &str = "Owner tools aren't available for this agent.";
const SIGN_IN: &str = "Sign in to change owner tools.";

/// A current access token of `owner` on `origin`: `Ok(None)` when the owner
/// has no session there; `Err` when the active session can't be renewed.
async fn owner_session<R: tauri::Runtime>(
    app: &AppHandle<R>,
    origin: &str,
    owner: &str,
) -> Result<Option<Zeroizing<String>>, String> {
    let Ok(owner) = nostr::PublicKey::from_hex(owner) else {
        return Ok(None);
    };
    let state = app.state::<AppState>();
    if state.token_auth.signing_pubkey(origin) == Some(owner) {
        return super::commands::fresh_session(&state, origin)
            .await
            .map(|session| Some(session.access));
    }
    let now = chrono::Utc::now().timestamp();
    Ok(state
        .token_auth
        .inactive
        .on_origin(origin)
        .into_iter()
        .find(|(account, _)| *account == owner)
        .and_then(|(_, auth)| match auth.get(origin) {
            Some((_, super::OriginAuth::Active(session))) if session.access_expires_at > now => {
                Some(session.access)
            }
            _ => None,
        }))
}

/// Levels of `pubkeys` (lowercase keys), in batches the relay accepts.
async fn read_levels(
    state: &AppState,
    origin: &str,
    access: &str,
    pubkeys: &[String],
) -> Result<HashMap<String, api::AgentToolLevel>, ApiError> {
    let mut rows = HashMap::new();
    for batch in pubkeys.chunks(api::TOOL_LEVELS_BATCH) {
        for row in api::agent_tool_levels(&state.http_client, origin, access, batch).await? {
            rows.insert(row.pubkey.to_ascii_lowercase(), row);
        }
    }
    Ok(rows)
}

async fn reported_level(state: &AppState, origin: &str, access: &str, pubkey: &str) -> Reported {
    let key = pubkey.to_ascii_lowercase();
    let rows = read_levels(state, origin, access, std::slice::from_ref(&key))
        .await
        .ok()?;
    reported_of(rows.get(&key))
}

async fn send_write(
    state: &AppState,
    origin: &str,
    access: &str,
    pubkey: &str,
    write: SaveWrite,
    class: RespondTo,
) -> Result<String, ApiError> {
    api::set_agent_tool_level(
        &state.http_client,
        origin,
        access,
        pubkey,
        write.level.as_str(),
        class.as_str(),
        false,
    )
    .await
}

/// Before a save of `record` (already holding the class it will answer):
/// send its owner tools first. `requested` is the owner's change in this
/// save, if any.
pub(crate) async fn before_managed_save<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
    requested: Option<ToolLevel>,
) -> Result<(), String> {
    let refuse = |message: &str| match requested {
        Some(_) => Err(message.to_string()),
        None => Ok(()),
    };
    if record.bot_origin.is_some() {
        return refuse(NOT_AVAILABLE);
    }
    let Some(owner) = crate::managed_agents::attested_owner_hex(record) else {
        return refuse(NOT_AVAILABLE);
    };
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let access = match owner_session(app, &origin, &owner).await {
        Ok(Some(access)) => access,
        Ok(None) => return refuse(SIGN_IN),
        Err(error) => return refuse(&error),
    };
    let class = effective_class(record);
    let reported = match requested {
        Some(_) => None,
        None => reported_level(&state, &origin, &access, &record.pubkey).await,
    };
    let write = save_write(reported, requested, class);
    route_then_save(
        write,
        async {
            match write {
                Some(write) => {
                    send_write(&state, &origin, &access, &record.pubkey, write, class).await
                }
                None => Ok(String::new()),
            }
        },
        || async { Ok(()) },
    )
    .await
}

/// Before a paired agent's access save on the active account: send its
/// owner tools first.
pub(crate) async fn before_external_save(
    state: &AppState,
    agent: &str,
    class: RespondTo,
    requested: Option<ToolLevel>,
) -> Result<(), String> {
    let origin = state.current_auth_origin();
    let session = match super::commands::fresh_session(state, &origin).await {
        Ok(session) => session,
        Err(error) if requested.is_some() => return Err(error),
        Err(_) => return Ok(()),
    };
    let reported = match requested {
        Some(_) => None,
        None => reported_level(state, &origin, &session.access, agent).await,
    };
    let write = save_write(reported, requested, class);
    route_then_save(
        write,
        async {
            match write {
                Some(write) => {
                    send_write(state, &origin, &session.access, agent, write, class).await
                }
                None => Ok(String::new()),
            }
        },
        || async { Ok(()) },
    )
    .await
}

/// Before a disconnect narrows a paired agent to owner-only: owner tools
/// off. Logged, never in the way of the disconnect.
pub(crate) async fn lower_before_disconnect(
    state: &AppState,
    origin: &str,
    access: &str,
    agent: &str,
) {
    let result = api::set_agent_tool_level(
        &state.http_client,
        origin,
        access,
        agent,
        ToolLevel::Limited.as_str(),
        RespondTo::OwnerOnly.as_str(),
        false,
    )
    .await;
    match result {
        Ok(_)
        | Err(ApiError {
            status: Some(404), ..
        }) => {}
        Err(error) => eprintln!(
            "kuzz-desktop: owner tools: not turned off before disconnect: {}",
            error.code.as_deref().unwrap_or("no answer")
        ),
    }
}

/// After an inbound access change of `record` (persona sync): only keep or
/// lower. An agent now answering anyone gets owner tools off; nothing is
/// ever turned on from here.
pub(crate) async fn lower_after_inbound<R: tauri::Runtime>(
    app: &AppHandle<R>,
    record: &ManagedAgentRecord,
) {
    let class = effective_class(record);
    if class != RespondTo::Anyone || record.bot_origin.is_some() {
        return;
    }
    let Some(owner) = crate::managed_agents::attested_owner_hex(record) else {
        return;
    };
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let Ok(Some(access)) = owner_session(app, &origin, &owner).await else {
        return;
    };
    let write = SaveWrite {
        level: ToolLevel::Limited,
        must_succeed: false,
        owner_change: false,
    };
    let result = send_write(&state, &origin, &access, &record.pubkey, write, class).await;
    let _ = settle_write(write, result);
}

/// The default write of `record` (only over a level never set). `true` when
/// it turned owner tools on.
pub(crate) async fn write_default<R: tauri::Runtime>(
    app: &AppHandle<R>,
    origin: &str,
    access: &str,
    record: &ManagedAgentRecord,
) -> bool {
    if record.bot_origin.is_some() {
        return false;
    }
    let class = effective_class(record);
    let level = default_level(class);
    let state = app.state::<AppState>();
    let result = api::set_agent_tool_level(
        &state.http_client,
        origin,
        access,
        &record.pubkey,
        level.as_str(),
        class.as_str(),
        true,
    )
    .await;
    match result {
        Ok(result) => result == "set" && level == ToolLevel::Full,
        Err(error) => {
            eprintln!(
                "kuzz-desktop: owner tools: default not written for {}: {}",
                record.pubkey,
                error.code.as_deref().unwrap_or("no answer")
            );
            false
        }
    }
}

/// Tell the UI which agents got owner tools on by default (one notice).
pub(crate) fn notify_defaulted<R: tauri::Runtime>(app: &AppHandle<R>, names: Vec<String>) {
    if names.is_empty() {
        return;
    }
    #[derive(Clone, Serialize)]
    struct Defaulted {
        names: Vec<String>,
    }
    let _ = app.emit(DEFAULTED_EVENT, Defaulted { names });
}

/// What the launch sweep does for one agent. Pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SweepStep {
    /// Unknown, not hosted here, or already in step.
    Skip,
    /// Never set: write the default.
    Default,
    /// Set, but the relay holds another class than this host runs: send
    /// this host's class, keeping the level (off for anyone).
    Reconcile(ToolLevel),
}

pub(crate) fn sweep_step(row: Option<&api::AgentToolLevel>, class: RespondTo) -> SweepStep {
    let Some(row) = row.filter(|row| row.here) else {
        return SweepStep::Skip;
    };
    match row.level.as_deref().and_then(ToolLevel::parse) {
        None if row.level.is_none() => SweepStep::Default,
        None => SweepStep::Skip,
        Some(level) if row.respond_class.as_deref() != Some(class.as_str()) => {
            SweepStep::Reconcile(effective_level(level, class))
        }
        Some(_) => SweepStep::Skip,
    }
}

static SWEPT: AtomicBool = AtomicBool::new(false);

/// Once per run: defaults for this device's agents never set, and their
/// class brought in step. One notice when any got owner tools on.
pub(crate) fn spawn_launch_sweep<R: tauri::Runtime>(app: &AppHandle<R>) {
    if SWEPT.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(records) = crate::managed_agents::load_managed_agents(&app) else {
            SWEPT.store(false, Ordering::SeqCst);
            return;
        };
        let state = app.state::<AppState>();
        let origin = state.current_auth_origin();
        let mut by_owner: HashMap<String, Vec<&ManagedAgentRecord>> = HashMap::new();
        for record in records.iter().filter(|record| record.bot_origin.is_none()) {
            if let Some(owner) = crate::managed_agents::attested_owner_hex(record) {
                by_owner.entry(owner).or_default().push(record);
            }
        }
        let mut turned_on = Vec::new();
        for (owner, records) in by_owner {
            let Ok(Some(access)) = owner_session(&app, &origin, &owner).await else {
                continue;
            };
            let keys: Vec<String> = records
                .iter()
                .map(|record| record.pubkey.to_ascii_lowercase())
                .collect();
            let Ok(rows) = read_levels(&state, &origin, &access, &keys).await else {
                continue;
            };
            for record in records {
                let class = effective_class(record);
                let row = rows.get(&record.pubkey.to_ascii_lowercase());
                match sweep_step(row, class) {
                    SweepStep::Skip => {}
                    SweepStep::Default => {
                        if write_default(&app, &origin, &access, record).await {
                            turned_on.push(record.name.clone());
                        }
                    }
                    SweepStep::Reconcile(level) => {
                        let write = SaveWrite {
                            level,
                            must_succeed: false,
                            owner_change: false,
                        };
                        let result =
                            send_write(&state, &origin, &access, &record.pubkey, write, class)
                                .await;
                        let _ = settle_write(write, result);
                    }
                }
            }
        }
        notify_defaulted(&app, turned_on);
    });
}

/// What deleting an agent does with its key on the relay. Pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RevokeStep {
    Revoke,
    /// Nothing to revoke from here (Dev data dir, demo build, server bot,
    /// no attested owner, or hosted on another device).
    Skip,
}

pub(crate) fn revoke_step(
    dev_data_dir: bool,
    demo_build: bool,
    server_bot: bool,
    attested: bool,
    hosting: Option<super::agent_hosting::Hosting>,
) -> RevokeStep {
    if dev_data_dir
        || demo_build
        || server_bot
        || !attested
        || hosting == Some(super::agent_hosting::Hosting::OtherDevice)
    {
        RevokeStep::Skip
    } else {
        RevokeStep::Revoke
    }
}

/// Whether the delete goes on after the revoke call: done, already gone
/// (404), hosted elsewhere (`409 not_host`: delete here only), or the relay
/// unreachable (no answer, or a gateway error: deleted here as before S1,
/// with a warning). A refusal from the relay keeps the agent so the owner
/// can retry. Pure.
pub(crate) fn revoke_outcome(result: Result<(), ApiError>) -> Result<(), String> {
    match result {
        Ok(())
        | Err(ApiError {
            status: Some(404), ..
        }) => Ok(()),
        Err(error) if error.code.as_deref() == Some("not_host") => Ok(()),
        Err(error) if matches!(error.status, None | Some(502..=504)) => {
            eprintln!("kuzz-desktop: owner tools: revoke skipped, relay unreachable: {error}");
            Ok(())
        }
        Err(error) => Err(format!(
            "Couldn't remove the agent's key on the server, so it was kept. Try again. ({error})"
        )),
    }
}

fn dev_data_dir<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    app.path()
        .app_data_dir()
        .ok()
        .and_then(|dir| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .map(crate::migration::is_dev_data_dir_name)
        })
        .unwrap_or(false)
}

/// The last server step of a delete: revoke the agent's key when this device
/// hosts it. No session for the owner here: nothing to revoke from here.
pub(crate) async fn revoke_for_delete<R: tauri::Runtime>(
    app: &AppHandle<R>,
    pubkey: &str,
) -> Result<(), String> {
    let record = {
        let state = app.state::<AppState>();
        let _store = state
            .managed_agents_store_lock
            .lock()
            .map_err(|e| e.to_string())?;
        crate::managed_agents::load_managed_agents(app)?
            .into_iter()
            .find(|record| record.pubkey.eq_ignore_ascii_case(pubkey))
    };
    let Some(record) = record else {
        return Ok(());
    };
    let owner = crate::managed_agents::attested_owner_hex(&record);
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let hosting = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|dir| super::agent_hosting::hosting_in(&dir, &origin, &record.pubkey));
    let step = revoke_step(
        dev_data_dir(app),
        crate::build_identity::is_demo_build(),
        record.bot_origin.is_some(),
        owner.is_some(),
        hosting,
    );
    let (RevokeStep::Revoke, Some(owner)) = (step, owner) else {
        return Ok(());
    };
    // No session, or one that can't be renewed: delete here as before S1.
    let access = match owner_session(app, &origin, &owner).await {
        Ok(Some(access)) => access,
        Ok(None) => {
            eprintln!(
                "kuzz-desktop: owner tools: {} not revoked: its owner is signed out here",
                record.pubkey
            );
            return Ok(());
        }
        Err(error) => {
            eprintln!(
                "kuzz-desktop: owner tools: {} not revoked: session not renewed: {error}",
                record.pubkey
            );
            return Ok(());
        }
    };
    revoke_outcome(api::revoke_agent(&state.http_client, &origin, &access, &record.pubkey).await)
}

/// One agent's owner tools, as the UI reads them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLevelView {
    pub pubkey: String,
    /// `full`, `limited`, or `None` when never set (acts as off).
    pub level: Option<String>,
    pub respond_class: Option<String>,
    pub here: bool,
}

/// Owner tools of `pubkeys` on the active account. A key missing from the
/// answer is unknown (shown off).
#[tauri::command]
pub async fn get_agent_tool_levels(
    app: AppHandle,
    pubkeys: Vec<String>,
) -> Result<Vec<ToolLevelView>, String> {
    let state = app.state::<AppState>();
    let origin = state.current_auth_origin();
    let session = super::commands::fresh_session(&state, &origin).await?;
    let keys: Vec<String> = pubkeys
        .iter()
        .map(|key| key.trim().to_ascii_lowercase())
        .collect();
    let rows = read_levels(&state, &origin, &session.access, &keys)
        .await
        .map_err(|error| error.to_string())?;
    Ok(rows
        .into_values()
        .map(|row| ToolLevelView {
            pubkey: row.pubkey.to_ascii_lowercase(),
            level: row.level,
            respond_class: row.respond_class,
            here: row.here,
        })
        .collect())
}

#[cfg(test)]
#[path = "tool_levels_tests.rs"]
mod tests;
