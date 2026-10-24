//! The desktop app signer (`kuzz_token_broker::app_signer`): a loopback
//! signer over this install's device key for the runners the app spawns.
//!
//! On macOS only binaries signed with the app's keychain access group can
//! open the Secure Enclave device key; `kuzz-acp` cannot, so it signs device
//! tags and header proofs through this server instead. It runs on every
//! desktop platform: a runner that opens the key itself (Windows TPM,
//! software keys) never asks it, so nothing changes there.
//!
//! Each spawned runner gets its own lease (URL + secret) scoped to its agent
//! and relay; the lease ends with the process record, and [`stop`] revokes
//! every lease and releases the key handle (app exit). Secrets are never
//! logged.

use std::sync::{Arc, Mutex, PoisonError};

use kuzz_token_broker_pkg::app_signer::{AppSigner, AppSignerLease, APP_SIGNER_ENV_VARS};
use kuzz_token_broker_pkg::device::DEVICE_ENV_VARS;

use crate::device_key::DeviceKey;

static SIGNER: Mutex<Option<Arc<AppSigner>>> = Mutex::new(None);

fn signer() -> std::sync::MutexGuard<'static, Option<Arc<AppSigner>>> {
    SIGNER.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Start the app signer over `key` (once the device key is open). Idempotent;
/// a failed start only logs: runners then sign without the app.
pub(crate) fn start(key: &DeviceKey) {
    let mut slot = signer();
    if slot.as_ref().is_some_and(|running| !running.is_stopped()) {
        return;
    }
    match AppSigner::start(key.clone()) {
        Ok(started) => {
            eprintln!("kuzz-desktop: app signer ready");
            *slot = Some(started);
        }
        Err(error) => eprintln!("kuzz-desktop: warning: app signer not started: {error}"),
    }
}

/// Sign with `key` from now on (the device key was rotated); the runners
/// already spawned keep their leases. Starts the signer when none runs.
pub(crate) fn replace_key(key: &DeviceKey) {
    let running = signer().clone();
    match running.filter(|running| !running.is_stopped()) {
        Some(running) => running.replace_key(key.clone()),
        None => start(key),
    }
}

/// Stop the app signer: revoke every lease and release the key handle.
pub(crate) fn stop() {
    if let Some(running) = signer().take() {
        running.stop();
    }
}

/// Strip inherited device signing variables from a runner `command` and, when
/// the app signer runs, give it a fresh lease for `agent_pubkey` on
/// `relay_url`. Keep the lease exactly as long as the process. Call after the
/// user env so a saved value never wins.
pub(crate) fn apply_spawn_env(
    command: &mut std::process::Command,
    agent_pubkey: &str,
    relay_url: &str,
) -> Option<AppSignerLease> {
    let running = signer().clone();
    apply_spawn_env_with(running.as_ref(), command, agent_pubkey, relay_url)
}

/// [`apply_spawn_env`] with an explicit signer.
fn apply_spawn_env_with(
    running: Option<&Arc<AppSigner>>,
    command: &mut std::process::Command,
    agent_pubkey: &str,
    relay_url: &str,
) -> Option<AppSignerLease> {
    for name in APP_SIGNER_ENV_VARS.iter().chain(DEVICE_ENV_VARS.iter()) {
        command.env_remove(name);
    }
    let running = running?;
    let agent = match nostr::PublicKey::from_hex(agent_pubkey) {
        Ok(agent) => agent,
        Err(error) => {
            eprintln!("kuzz-desktop: warning: no app signer for this agent: {error}");
            return None;
        }
    };
    match running.register(agent, relay_url) {
        Ok(lease) => {
            for (name, value) in lease.env() {
                command.env(name, value);
            }
            Some(lease)
        }
        Err(error) => {
            eprintln!("kuzz-desktop: warning: no app signer for this agent: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuzz_token_broker_pkg::app_signer::{APP_SIGNER_SECRET_ENV, APP_SIGNER_URL_ENV};
    use kuzz_token_broker_pkg::device::{DEVICE_BROKER_SECRET_ENV, DEVICE_BROKER_URL_ENV};

    fn env_of(command: &std::process::Command, name: &str) -> Option<Option<String>> {
        command
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new(name))
            .map(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
    }

    fn software_key(dir: &std::path::Path) -> DeviceKey {
        DeviceKey::open_or_create_file(&dir.join("device.key")).expect("software key")
    }

    #[test]
    fn runners_get_their_own_lease_and_never_inherited_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        let running = AppSigner::start(software_key(dir.path())).expect("start");
        let agent = nostr::Keys::generate().public_key().to_hex();
        let mut command = std::process::Command::new("kuzz-acp");
        for name in [
            APP_SIGNER_URL_ENV,
            APP_SIGNER_SECRET_ENV,
            DEVICE_BROKER_URL_ENV,
            DEVICE_BROKER_SECRET_ENV,
        ] {
            command.env(name, "inherited");
        }
        let lease =
            apply_spawn_env_with(Some(&running), &mut command, &agent, "wss://relay.example")
                .expect("lease");
        assert_eq!(
            env_of(&command, APP_SIGNER_URL_ENV),
            Some(Some(running.url()))
        );
        let secret = env_of(&command, APP_SIGNER_SECRET_ENV)
            .flatten()
            .expect("secret set");
        assert_ne!(secret, "inherited");
        assert_eq!(env_of(&command, DEVICE_BROKER_URL_ENV), Some(None));
        assert_eq!(env_of(&command, DEVICE_BROKER_SECRET_ENV), Some(None));
        // The lease's Debug output never shows the secret.
        assert!(!format!("{lease:?}").contains(&secret));
        running.stop();
    }

    #[test]
    fn without_a_signer_or_agent_key_runners_get_nothing() {
        let mut command = std::process::Command::new("kuzz-acp");
        command.env(APP_SIGNER_SECRET_ENV, "inherited");
        assert!(apply_spawn_env_with(None, &mut command, "ab", "wss://r.example").is_none());
        assert_eq!(env_of(&command, APP_SIGNER_SECRET_ENV), Some(None));

        let dir = tempfile::tempdir().expect("tempdir");
        let running = AppSigner::start(software_key(dir.path())).expect("start");
        let mut command = std::process::Command::new("kuzz-acp");
        assert!(
            apply_spawn_env_with(Some(&running), &mut command, "not-hex", "wss://r.example")
                .is_none()
        );
        assert_eq!(env_of(&command, APP_SIGNER_URL_ENV), Some(None));
        // A stopped signer hands out no leases.
        running.stop();
        let agent = nostr::Keys::generate().public_key().to_hex();
        assert!(
            apply_spawn_env_with(Some(&running), &mut command, &agent, "wss://r.example").is_none()
        );
    }
}
