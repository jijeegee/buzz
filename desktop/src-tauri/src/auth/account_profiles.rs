//! Display details of the accounts on this computer: name, avatar, email and
//! when each was last used, for the account menu and the switch overlay.
//!
//! Kept in `account-profiles.json`, apart from the account list: saving the
//! list rewrites its secure-storage index every time, and these details change
//! far more often (every sign-in, restore and switch). Nothing here is secret
//! and nothing depends on it; a missing or unreadable file only loses names.
//! Name and avatar come from the relay's `/auth/me` (the Google or Apple
//! profile), so an account has them even when its own UI was never opened
//! here. The Kuzz profile name is recorded separately whenever the account is
//! active, and menus prefer it: the account is a Kuzz account, Google or Apple
//! is only how it signs in (recorded here at each sign-in; the relay does not
//! say).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use nostr::PublicKey;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use zeroize::Zeroizing;

use super::accounts::AccountState;

/// File name inside the app data directory.
pub(crate) const FILE_NAME: &str = "account-profiles.json";

const VERSION: u8 = 1;

/// One account's display details. Every field is optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AccountProfile {
    /// The Google or Apple name from `/auth/me`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The account's own Kuzz profile name, last seen while it was active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kuzz_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Unix seconds of the last sign-in or switch to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
    /// How it last signed in here: "google" or "apple". Absent: Google (every
    /// account from before Sign in with Apple).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

/// One account on this computer, for the UI. Never carries secrets.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub pubkey: String,
    pub origin: String,
    /// `signed_in | needs_login | signed_out | key_only`
    pub state: AccountState,
    pub active: bool,
    /// From [`AccountProfile`]; absent until first learned.
    /// The Google or Apple name.
    pub display_name: Option<String>,
    /// The Kuzz profile name; menus prefer it.
    pub kuzz_name: Option<String>,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
    /// Unix seconds of the last sign-in or switch to it.
    pub last_used_at: Option<i64>,
    /// "google" or "apple"; absent: Google.
    pub provider: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ProfileFile {
    version: u8,
    /// Keyed by hex pubkey.
    #[serde(default)]
    profiles: BTreeMap<String, AccountProfile>,
}

/// Serialises read-modify-write of the file in this process.
static LOCK: Mutex<()> = Mutex::new(());

/// Every account's details. Missing or unreadable: none.
pub(crate) fn load(data_dir: &Path) -> BTreeMap<String, AccountProfile> {
    let bytes = match std::fs::read(data_dir.join(FILE_NAME)) {
        Ok(bytes) => bytes,
        Err(_) => return BTreeMap::new(),
    };
    match serde_json::from_slice::<ProfileFile>(&bytes) {
        Ok(file) if file.version == VERSION => file.profiles,
        Ok(file) => {
            eprintln!(
                "kuzz-desktop: accounts: profile file version {} ignored",
                file.version
            );
            BTreeMap::new()
        }
        Err(error) => {
            eprintln!("kuzz-desktop: accounts: profile file unreadable: {error}");
            BTreeMap::new()
        }
    }
}

fn update(
    data_dir: &Path,
    account: &PublicKey,
    change: impl FnOnce(&mut AccountProfile),
) -> Result<(), String> {
    let _lock = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut profiles = load(data_dir);
    let profile = profiles.entry(account.to_hex()).or_default();
    let before = profile.clone();
    change(profile);
    if *profile == before {
        return Ok(());
    }
    let json = serde_json::to_vec_pretty(&ProfileFile {
        version: VERSION,
        profiles,
    })
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(data_dir).map_err(|e| format!("account profiles: {e}"))?;
    crate::managed_agents::storage::atomic_write_json(&data_dir.join(FILE_NAME), &json)
}

/// Record what the relay says about `account`. A value the relay did not send
/// keeps the one already known (an older relay sends no email).
pub(crate) fn learn(
    data_dir: &Path,
    account: &PublicKey,
    display_name: Option<String>,
    avatar_url: Option<String>,
    email: Option<String>,
) {
    let clean = |value: Option<String>| value.filter(|v| !v.trim().is_empty());
    let result = update(data_dir, account, |profile| {
        if let Some(name) = clean(display_name) {
            profile.display_name = Some(name);
        }
        if let Some(url) = clean(avatar_url) {
            profile.avatar_url = Some(url);
        }
        if let Some(email) = clean(email) {
            profile.email = Some(email);
        }
    });
    if let Err(error) = result {
        eprintln!("kuzz-desktop: accounts: profile not saved: {error}");
    }
}

/// Record `account`'s Kuzz profile name. Blank keeps the one already known.
pub(crate) fn learn_kuzz_name(data_dir: &Path, account: &PublicKey, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    if let Err(error) = update(data_dir, account, |profile| {
        profile.kuzz_name = Some(name.to_owned())
    }) {
        eprintln!("kuzz-desktop: accounts: Kuzz name not saved: {error}");
    }
}

/// Record a sign-in of `account` for the account menu: used now, how it
/// signed in (`provider`), its email (from relays that send it), and in the
/// background its `/auth/me` name and avatar. Best effort.
pub(crate) fn remember_sign_in(
    app: &tauri::AppHandle,
    origin: &str,
    account: PublicKey,
    tokens: &super::api::LoginTokens,
    provider: &str,
) {
    let Ok(data_dir) = app.path().app_data_dir() else {
        return;
    };
    touch(&data_dir, &account);
    learn_provider(&data_dir, &account, provider);
    learn(&data_dir, &account, None, None, tokens.email.clone());
    let client = app
        .state::<crate::app_state::AppState>()
        .http_client
        .clone();
    let origin = origin.to_owned();
    let access = Zeroizing::new(tokens.access.0.to_string());
    tauri::async_runtime::spawn(async move {
        match super::api::me(&client, &origin, &access).await {
            Ok(me) => learn(
                &data_dir,
                &account,
                me.display_name,
                me.avatar_url,
                me.email,
            ),
            Err(error) => eprintln!("kuzz-desktop: accounts: profile not fetched: {error}"),
        }
    });
}

/// Record how `account` just signed in: "google" or "apple".
pub(crate) fn learn_provider(data_dir: &Path, account: &PublicKey, provider: &str) {
    if let Err(error) = update(data_dir, account, |profile| {
        profile.provider = Some(provider.to_owned())
    }) {
        eprintln!("kuzz-desktop: accounts: sign-in method not saved: {error}");
    }
}

/// Record that `account` was just signed into or switched to.
pub(crate) fn touch(data_dir: &Path, account: &PublicKey) {
    let now = chrono::Utc::now().timestamp();
    if let Err(error) = update(data_dir, account, |profile| {
        profile.last_used_at = Some(now)
    }) {
        eprintln!("kuzz-desktop: accounts: last use not saved: {error}");
    }
}

/// Most recently used first; never-used accounts last, in their given order.
/// Pure.
pub(crate) fn most_recent_first<T>(items: &mut [T], last_used_at: impl Fn(&T) -> Option<i64>) {
    items.sort_by_key(|item| std::cmp::Reverse(last_used_at(item).unwrap_or(i64::MIN)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_merge_and_survive_a_missing_or_damaged_file() {
        let dir = tempfile::tempdir().unwrap();
        let account = nostr::Keys::generate().public_key();
        assert!(load(dir.path()).is_empty());
        learn(
            dir.path(),
            &account,
            Some("Jiho".into()),
            Some("https://img.test/a.png".into()),
            None,
        );
        touch(dir.path(), &account);
        learn_kuzz_name(dir.path(), &account, " akak ");
        learn_kuzz_name(dir.path(), &account, "  ");
        // A relay that sends no email or name keeps what is known.
        learn(
            dir.path(),
            &account,
            Some(" ".into()),
            None,
            Some("j@test".into()),
        );
        let profile = load(dir.path()).remove(&account.to_hex()).unwrap();
        assert_eq!(profile.display_name.as_deref(), Some("Jiho"));
        assert_eq!(
            profile.avatar_url.as_deref(),
            Some("https://img.test/a.png")
        );
        assert_eq!(profile.email.as_deref(), Some("j@test"));
        assert_eq!(profile.kuzz_name.as_deref(), Some("akak"));
        assert!(profile.last_used_at.is_some());
        assert_eq!(profile.provider, None, "unknown until a sign-in records it");
        learn_provider(dir.path(), &account, "apple");
        let profile = load(dir.path()).remove(&account.to_hex()).unwrap();
        assert_eq!(profile.provider.as_deref(), Some("apple"));
        assert_eq!(profile.email.as_deref(), Some("j@test"), "others are kept");

        std::fs::write(dir.path().join(FILE_NAME), b"{not json").unwrap();
        assert!(load(dir.path()).is_empty());
        // The next write replaces the damaged file.
        touch(dir.path(), &account);
        assert!(load(dir.path()).contains_key(&account.to_hex()));
    }

    #[test]
    fn most_recently_used_accounts_come_first() {
        let mut items = vec![
            ("never", None),
            ("old", Some(10)),
            ("new", Some(20)),
            ("never2", None),
        ];
        most_recent_first(&mut items, |item| item.1);
        let names: Vec<&str> = items.iter().map(|item| item.0).collect();
        assert_eq!(names, ["new", "old", "never", "never2"]);
    }
}
