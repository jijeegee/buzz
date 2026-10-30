//! Launch snapshots in the device message store (the Chats rows, other
//! people's profiles), read and written in the account and server the caller
//! drew under, like a paint (docs/message-sync.md#launch-snapshots).

use nostr::PublicKey;
use tauri::{Manager, State};

use super::message_store_coverage::{off_main, paint_scope};
use crate::app_state::AppState;
use crate::auth::accounts::{AccountState, Registry};

/// Snapshot `kind` kept for `account` on `relay_url`, or `None` when none is
/// kept or either is no longer the active one.
#[tauri::command]
pub async fn message_store_snapshot_read(
    kind: String,
    account: String,
    relay_url: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let Some((relay, account)) = paint_scope(&state, &account, &relay_url)? else {
        return Ok(None);
    };
    off_main(move || crate::message_store::read_snapshot(&relay, &account, &kind)).await
}

/// Keeps `blob` as snapshot `kind` for `account` on `relay_url`. Nothing is
/// written (`false`) once either is no longer the active one, or after that
/// account signed out of that server (its file is gone and stays gone).
#[tauri::command]
pub async fn message_store_snapshot_write(
    kind: String,
    blob: String,
    account: String,
    relay_url: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let Some((relay, account)) = paint_scope(&state, &account, &relay_url)? else {
        return Ok(false);
    };
    let key = PublicKey::from_hex(&account).ok();
    let origin = crate::auth::origin_for(&relay);
    if state.token_auth.signed_out.is_signed_out(&origin, key) {
        return Ok(false);
    }
    off_main(move || crate::message_store::write_snapshot(&relay, &account, &kind, blob)).await
}

/// The accounts signed in on `origin` and not signed out there.
fn signed_in_on(
    registry: &Registry,
    origin: &str,
    is_signed_out: impl Fn(PublicKey) -> bool,
) -> Vec<String> {
    registry
        .accounts
        .iter()
        .filter(|account| account.state == AccountState::SignedIn && account.origin == origin)
        .filter_map(|account| {
            let key = PublicKey::from_hex(&account.pubkey).ok()?;
            (!is_signed_out(key)).then(|| account.pubkey.to_ascii_lowercase())
        })
        .collect()
}

/// The device's account list and the accounts signed in on `relay_url`'s
/// server; `None` when there is no list (a single-account device).
fn signed_in_accounts(
    app: &tauri::AppHandle,
    relay_url: &str,
) -> Result<Option<Vec<String>>, String> {
    let state = app.state::<AppState>();
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| "account storage is unavailable")?;
    let origin = crate::auth::origin_for(relay_url);
    let signed_out = &state.token_auth.signed_out;
    Ok(
        crate::auth::accounts::load_registry(&data_dir)?.map(|registry| {
            signed_in_on(&registry, &origin, |key| {
                signed_out.is_signed_out(&origin, Some(key))
            })
        }),
    )
}

/// Snapshot `kind` of an account watched in the background (signed in on
/// this computer and not signed out of `relay_url`'s server), whether or not
/// it is the active one. Read only: writes stay with the active account.
#[tauri::command]
pub async fn message_store_snapshot_read_signed_in(
    kind: String,
    account: String,
    relay_url: String,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    let account = account.to_ascii_lowercase();
    let listed = signed_in_accounts(&app, &relay_url)?;
    if !listed.is_some_and(|accounts| accounts.contains(&account)) {
        return Ok(None);
    }
    off_main(move || crate::message_store::read_snapshot(&relay_url, &account, &kind)).await
}

/// Whether the active account is the only one signed in on `relay_url`'s
/// server: profile labels earlier builds kept per server (no account split)
/// may move to it. A device without an account list has one account.
#[tauri::command]
pub fn message_store_sole_account_on_server(
    account: String,
    relay_url: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<bool, String> {
    let Some((relay, account)) = paint_scope(&state, &account, &relay_url)? else {
        return Ok(false);
    };
    let account = account.to_ascii_lowercase();
    Ok(match signed_in_accounts(&app, &relay)? {
        None => true,
        Some(accounts) => accounts == [account],
    })
}

#[cfg(test)]
mod tests {
    use super::signed_in_on;
    use crate::auth::accounts::{Account, AccountState, Registry};

    fn account(pubkey: &str, origin: &str, state: AccountState) -> Account {
        Account {
            pubkey: pubkey.to_owned(),
            origin: origin.to_owned(),
            state,
        }
    }

    #[test]
    fn only_signed_in_accounts_of_that_server_count() {
        let hex = || nostr::Keys::generate().public_key().to_hex();
        let (a, b, c) = (hex(), hex(), hex());
        let registry = Registry {
            accounts: vec![
                account(&a, "https://one", AccountState::SignedIn),
                account(&b, "https://one", AccountState::SignedIn),
                account(&c, "https://two", AccountState::SignedIn),
            ],
            ..Registry::default()
        };
        assert_eq!(
            signed_in_on(&registry, "https://one", |_| false),
            vec![a.clone(), b.clone()]
        );
        let b_key = nostr::PublicKey::from_hex(&b).unwrap();
        assert_eq!(
            signed_in_on(&registry, "https://one", |key| key == b_key),
            vec![a]
        );
        assert_eq!(signed_in_on(&registry, "https://two", |_| false), vec![c]);
        assert!(signed_in_on(&registry, "https://three", |_| false).is_empty());
    }
}
