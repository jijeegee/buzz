//! Local data cleanup after sessions and owned agents have stopped.
use nostr::PublicKey;
use tauri::AppHandle;

pub(super) async fn finish(
    app: &AppHandle,
    origin: &str,
    owners: &[PublicKey],
    confirmed: &std::collections::HashSet<String>,
) -> Result<(), String> {
    // Key cleanup still runs when disk cleanup fails; the caller reports the
    // failure so sign-out can be retried, never silently treated as complete.
    let mut failure = None;
    for owner in owners {
        if let Err(error) = crate::message_store::message_store_clear(
            app.clone(),
            origin.to_string(),
            owner.to_hex(),
        )
        .await
        {
            failure = Some(error);
        }
    }
    forget_owner_agent_keys(app, origin, owners, confirmed);
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Remove the confirmed-copy agent keys of `owners` on `origin` (see
/// [`super::agent_hosting::forget_confirmed_keys`]). Best effort.
fn forget_owner_agent_keys(
    app: &AppHandle,
    origin: &str,
    owners: &[PublicKey],
    confirmed: &std::collections::HashSet<String>,
) {
    let owned: Vec<String> = crate::managed_agents::load_managed_agents_without_keys(app)
        .unwrap_or_default()
        .into_iter()
        .filter(|record| record.bot_origin.is_none())
        .filter(|record| {
            let owner = crate::managed_agents::agent_owner_hex(app, record);
            owners
                .iter()
                .any(|o| owner.as_deref() == Some(o.to_hex().as_str()))
        })
        .map(|record| record.pubkey)
        .collect();
    let removed = super::agent_hosting::forget_confirmed_keys(app, origin, &owned, confirmed);
    if removed > 0 {
        eprintln!("kuzz-desktop: auth: {removed} agent key(s) left this computer at sign-out");
    }
}
