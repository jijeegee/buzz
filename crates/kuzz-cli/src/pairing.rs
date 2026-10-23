//! External agent pairing (device-bound keys plan §6, §9, §14.7; no approval
//! step since 10-23): `kuzz pair <code>` connects an agent on this computer to
//! the owner whose app made the code. The first valid claim connects at once.
//! Afterwards the paired agent is this computer's default identity for that
//! relay, so `kuzz messages send` and `kuzz messages watch` work with no
//! Google sign-in here.
//!
//! Where the keys live:
//!
//! - **Device key:** this OS user's shared device key (TPM where Windows has
//!   one, else the software store), the same key `kuzz auth login` uses.
//!   `pair` creates it when there is none yet.
//! - **Agent key:** by default a new key per pairing, generated here and
//!   written only to `<local data dir>/kuzz/pairings/<agent pubkey>.json`
//!   ([`PAIRING_DIR_ENV`] overrides the directory). The file is owner-only:
//!   mode 0600 in a 0700 directory on Unix, and the per-user `%LOCALAPPDATA%`
//!   on Windows. It is never printed. An agent that already has a key passes
//!   it with `--private-key`/`KUZZ_PRIVATE_KEY`; the file then records only
//!   the public key and the secret stays where the agent keeps it.
//!
//! A copied agent key is useless on another computer: the relay admits a
//! paired key only together with this computer's device key.

use std::path::{Path, PathBuf};

use kuzz_auth::pairing::{is_pair_code, pair_claim_message, pair_code_hash};
use kuzz_token_broker::device::DeviceKey;
use nostr::Keys;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::auth_session::{relay_origin, SessionStore};
use crate::error::CliError;

/// Overrides the pairing directory (default `<local data dir>/kuzz/pairings`).
pub const PAIRING_DIR_ENV: &str = "KUZZ_PAIRING_DIR";
/// Picks one of several agents paired with the same relay (name or pubkey
/// hex). Without it the most recent pairing wins.
pub const PAIRED_AGENT_ENV: &str = "KUZZ_PAIRED_AGENT";
/// Pairing file format version.
const PAIRING_FILE_VERSION: u8 = 1;
/// Longest device or agent name the relay accepts.
const MAX_NAME_CHARS: usize = 64;

/// One pairing of an agent key with this computer, as stored.
#[derive(Clone, Serialize, Deserialize)]
pub struct Pairing {
    /// File format version.
    pub version: u8,
    /// Relay origin (`https://host[:port]`).
    pub relay: String,
    /// The agent's public key (hex).
    pub agent_pubkey: String,
    /// The agent's display name (`--name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// This computer's name as the owner sees it.
    pub device_name: String,
    /// `connected`, or `pending` while the claim's answer is unknown.
    pub state: String,
    /// Relay pairing id (connected only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_id: Option<String>,
    /// Relay device id of this computer (connected only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Unix seconds of the claim.
    pub paired_at: i64,
    /// The owner's account pubkey (hex), once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The agent's secret key (hex) when `pair` generated it; absent when
    /// the agent brought its own key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_secret: Option<String>,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing")
            .field("relay", &self.relay)
            .field("agent_pubkey", &self.agent_pubkey)
            .field("agent_name", &self.agent_name)
            .field("device_name", &self.device_name)
            .field("state", &self.state)
            .field("has_secret", &self.agent_secret.is_some())
            .finish_non_exhaustive()
    }
}

impl Pairing {
    /// The agent's keys, when this pairing holds its secret.
    pub fn keys(&self) -> Option<Keys> {
        let secret = Zeroizing::new(self.agent_secret.clone()?);
        Keys::parse(secret.as_str()).ok()
    }

    /// How the agent is named in messages: its name, else a short pubkey.
    pub fn label(&self) -> String {
        self.agent_name
            .clone()
            .unwrap_or_else(|| format!("agent {}", &self.agent_pubkey[..8]))
    }
}

/// The directory of pairing files.
#[derive(Debug, Clone)]
pub struct PairingStore {
    dir: PathBuf,
}

impl PairingStore {
    /// A store in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// [`PAIRING_DIR_ENV`], else `<local data dir>/kuzz/pairings`.
    pub fn open_default() -> Option<Self> {
        if let Some(dir) = std::env::var_os(PAIRING_DIR_ENV).filter(|dir| !dir.is_empty()) {
            return Some(Self::new(dir));
        }
        dirs::data_local_dir().map(|dir| Self::new(dir.join("kuzz").join("pairings")))
    }

    fn path(&self, agent_pubkey: &str) -> PathBuf {
        self.dir.join(format!("{agent_pubkey}.json"))
    }

    /// Write `pairing` (owner-only, atomically replacing an older copy).
    pub fn save(&self, pairing: &Pairing) -> Result<(), CliError> {
        let fail = |error: std::io::Error| {
            CliError::Other(format!(
                "cannot write the pairing in {}: {error}",
                self.dir.display()
            ))
        };
        create_private_dir(&self.dir).map_err(fail)?;
        let bytes = Zeroizing::new(
            serde_json::to_vec_pretty(pairing)
                .map_err(|e| CliError::Other(format!("pairing serialization failed: {e}")))?,
        );
        let path = self.path(&pairing.agent_pubkey);
        let tmp = path.with_extension("json.tmp");
        write_private_file(&tmp, &bytes).map_err(fail)?;
        std::fs::rename(&tmp, &path).map_err(|error| {
            let _ = std::fs::remove_file(&tmp);
            fail(error)
        })
    }

    /// Forget the pairing of `agent_pubkey` (missing is fine).
    pub fn remove(&self, agent_pubkey: &str) {
        let _ = std::fs::remove_file(self.path(agent_pubkey));
    }

    /// Every readable pairing; unreadable files are skipped.
    pub fn list(&self) -> Vec<Pairing> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| std::fs::read(path).ok())
            .filter_map(|bytes| {
                let bytes = Zeroizing::new(bytes);
                serde_json::from_slice::<Pairing>(&bytes).ok()
            })
            .collect()
    }

    /// The pairing to act as on `origin`: among pairings that hold their
    /// agent's secret, the one `selector` names (agent name or pubkey), else
    /// the most recent.
    pub fn for_relay(&self, origin: &str, selector: Option<&str>) -> Option<Pairing> {
        let mut candidates: Vec<Pairing> = self
            .list()
            .into_iter()
            .filter(|pairing| pairing.relay == origin && pairing.agent_secret.is_some())
            .collect();
        if let Some(selector) = selector.map(str::trim).filter(|s| !s.is_empty()) {
            candidates.retain(|pairing| {
                pairing.agent_pubkey.eq_ignore_ascii_case(selector)
                    || pairing.agent_name.as_deref() == Some(selector)
            });
        }
        candidates
            .into_iter()
            .max_by_key(|pairing| pairing.paired_at)
    }

    /// The only relay this computer's pairings use, if they all use one.
    pub fn sole_relay(&self) -> Option<String> {
        let mut relays: Vec<String> = self
            .list()
            .into_iter()
            .filter(|pairing| pairing.agent_secret.is_some())
            .map(|pairing| pairing.relay)
            .collect();
        relays.sort();
        relays.dedup();
        match relays.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        }
    }
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut out = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        out.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    out.write_all(bytes)?;
    out.sync_all()
}

/// The paired agent to act as on `relay_url`, when this process has no other
/// credential: not a managed agent (`lookup` sees
/// `KUZZ_DISABLE_STORED_SESSION`), and no `kuzz auth login` session for that
/// relay (a signed-in person on this computer keeps acting as themselves).
pub(crate) fn paired_identity(
    relay_url: &str,
    session: Option<&SessionStore>,
    pairings: Option<&PairingStore>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<(Keys, Pairing)> {
    if kuzz_token_broker::stored_session_disabled(&lookup) {
        return None;
    }
    let origin = relay_origin(relay_url).ok()?;
    if let Some(store) = session {
        // An unreadable session file also keeps the session path, so its
        // error is reported rather than silently acting as the agent.
        if !matches!(store.get(&origin), Ok(None)) {
            return None;
        }
    }
    let pairing = pairings?.for_relay(&origin, lookup(PAIRED_AGENT_ENV).as_deref())?;
    let keys = pairing.keys()?;
    Some((keys, pairing))
}

/// The relay of this computer's pairings, used when no relay was configured.
pub(crate) fn paired_relay(
    pairings: Option<&PairingStore>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if kuzz_token_broker::stored_session_disabled(&lookup) {
        return None;
    }
    pairings?.sole_relay()
}

/// The host name, without the `(CLI)` suffix logins add.
fn default_pair_device_name() -> String {
    let name = crate::commands::auth::host_name();
    if name.is_empty() {
        "computer".to_owned()
    } else {
        name
    }
}

fn validate_name(what: &str, name: &str) -> Result<(), CliError> {
    let len = name.chars().count();
    if len == 0 || len > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(CliError::Usage(format!(
            "{what} must be 1-{MAX_NAME_CHARS} printable characters"
        )));
    }
    Ok(())
}

/// The claim body for `code`: both signatures over
/// [`pair_claim_message`]. Blocks on the TPM.
pub(crate) fn claim_body(
    code: &str,
    agent: &Keys,
    device: &DeviceKey,
    device_name: &str,
) -> Result<Value, CliError> {
    use nostr::hashes::{sha256::Hash as Sha256Hash, Hash as _};

    let agent_hex = agent.public_key().to_hex();
    let device_hex = device.pubkey().to_hex();
    let message = pair_claim_message(&pair_code_hash(code), &agent_hex, &device_hex);
    let digest = Sha256Hash::hash(message.as_bytes());
    let agent_signature = agent
        .sign_schnorr(&nostr::secp256k1::Message::from_digest(
            digest.to_byte_array(),
        ))
        .to_string();
    let device_signature = device
        .sign(message.as_bytes())
        .map_err(|e| CliError::Other(format!("device key could not sign: {e}")))?;
    Ok(json!({
        "code": code,
        "agent_pubkey": agent_hex,
        "agent_signature": agent_signature,
        "device_pubkey": device_hex,
        "device_signature": hex::encode(device_signature),
        "device_key_kind": device.kind().as_wire(),
        "device_name": device_name,
    }))
}

/// A refused claim as a clear CLI error.
fn claim_error(status: u16, body: &str) -> CliError {
    let code = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("code").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let message = match code.as_str() {
        "invalid_code" => {
            "invalid or expired: this pairing code is unknown or older than 10 minutes; \
             press \"Connect external agent\" in the app again and paste the new command"
        }
        "code_used" => {
            "code_used: this pairing code was used already (each code works once); \
             ask the owner for a new one"
        }
        "key_in_use" => {
            "key_in_use: this agent key is an account key or already registered; \
             pair without --private-key to use a new agent key"
        }
        "device_limit" => "device_limit: the owner has the most paired computers allowed",
        "invalid_signature" => "invalid_signature: the relay refused this computer's signatures",
        "rate_limited" => "rate_limited: too many pairing attempts; wait a minute and retry",
        _ => return crate::client::relay_error(status, body.to_owned()),
    };
    if status == 429 {
        CliError::Relay {
            status,
            body: message.to_owned(),
        }
    } else {
        CliError::Auth(message.to_owned())
    }
}

/// `POST /pair/claim` for `body`: Ok(answer) on 2xx, the mapped error
/// otherwise. `Err(None)`: no answer (the claim may or may not have landed).
async fn post_claim(
    http: &reqwest::Client,
    relay_url: &str,
    body: &Value,
) -> Result<Value, Option<CliError>> {
    let response = http
        .post(format!("{relay_url}/pair/claim"))
        .json(body)
        .send()
        .await
        .map_err(|_| None)?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(|_| None)?;
    if !(200..300).contains(&status) {
        return Err(Some(claim_error(status, &text)));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// Inputs of one `kuzz pair`.
pub(crate) struct PairRequest<'a> {
    pub relay_url: &'a str,
    pub code: &'a str,
    pub agent_name: Option<&'a str>,
    pub device_name: &'a str,
    /// The agent's own key (`--private-key`/`KUZZ_PRIVATE_KEY`), if any.
    pub agent_keys: Option<Keys>,
}

/// Claim `request.code` with `device` and save the pairing in `store`.
/// The pairing is written before the claim (so a lost answer never loses a
/// connected key) and removed again when the relay refuses.
pub(crate) async fn pair(
    request: PairRequest<'_>,
    device: &DeviceKey,
    store: &PairingStore,
) -> Result<Pairing, CliError> {
    let origin = relay_origin(request.relay_url)?;
    let (keys, generated) = match request.agent_keys {
        Some(keys) => (keys, false),
        None => (Keys::generate(), true),
    };
    let mut pairing = Pairing {
        version: PAIRING_FILE_VERSION,
        relay: origin,
        agent_pubkey: keys.public_key().to_hex(),
        agent_name: request.agent_name.map(str::to_owned),
        device_name: request.device_name.to_owned(),
        state: "pending".to_owned(),
        pairing_id: None,
        device_id: None,
        paired_at: chrono::Utc::now().timestamp(),
        owner: None,
        agent_secret: generated.then(|| keys.secret_key().to_secret_hex()),
    };
    let body = {
        let (code, keys, device, name) = (
            request.code.to_owned(),
            keys.clone(),
            device.clone(),
            request.device_name.to_owned(),
        );
        tokio::task::spawn_blocking(move || claim_body(&code, &keys, &device, &name))
            .await
            .map_err(|e| CliError::Other(format!("signing task failed: {e}")))??
    };
    store.save(&pairing)?;
    let http = crate::client::build_http()?;
    match post_claim(&http, request.relay_url, &body).await {
        Ok(answer) => {
            pairing.state = "connected".to_owned();
            pairing.pairing_id = answer["pairing_id"].as_str().map(str::to_owned);
            pairing.device_id = answer["device_id"].as_str().map(str::to_owned);
            pairing.owner = ["owner_pubkey", "owner"]
                .iter()
                .find_map(|field| answer[*field].as_str())
                .filter(|owner| is_hex_pubkey(owner))
                .map(str::to_owned);
            store.save(&pairing)?;
            Ok(pairing)
        }
        Err(Some(error)) => {
            store.remove(&pairing.agent_pubkey);
            Err(error)
        }
        Err(None) => Err(CliError::DeliveryUnknown(format!(
            "the relay at {} did not answer the pairing claim; the agent key is saved in case \
             it connected (check the app). If the app does not show this computer, ask for a \
             new code and run `kuzz pair` again",
            request.relay_url
        ))),
    }
}

fn is_hex_pubkey(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The owner of the paired agent, asked as that agent from this computer
/// (`GET /agents/{pubkey}/status`). `None` when the relay does not say
/// (device admission `off`) or cannot be reached.
pub(crate) async fn fetch_owner(relay_url: &str, keys: &Keys) -> Option<String> {
    let client =
        crate::client::KuzzClient::new(relay_url.to_owned(), keys.clone(), None, None).ok()?;
    let pubkey = keys.public_key().to_hex();
    let raw = client
        .get_authed(&format!("/agents/{pubkey}/status"))
        .await
        .ok()?;
    let answer: Value = serde_json::from_str(&raw).ok()?;
    answer["keys"]
        .as_array()?
        .iter()
        .find(|key| key["pubkey"] == pubkey.as_str())
        .and_then(|key| key["owner"].as_str())
        .filter(|owner| is_hex_pubkey(owner))
        .map(str::to_owned)
}

/// `kuzz auth whoami` for a paired agent: who it is, on which computer, for
/// which owner (looked up once and remembered when the pairing lacks it).
pub(crate) async fn whoami(
    relay_url: &str,
    keys: &Keys,
    mut pairing: Pairing,
    store: Option<&PairingStore>,
) -> Value {
    if pairing.owner.is_none() {
        if let Some(owner) = fetch_owner(relay_url, keys).await {
            pairing.owner = Some(owner);
            if let Some(store) = store {
                let _ = store.save(&pairing);
            }
        }
    }
    json!({
        "mode": "paired",
        "pubkey": pairing.agent_pubkey,
        "agent_name": pairing.agent_name,
        "device_name": pairing.device_name,
        "relay": pairing.relay,
        "owner": pairing.owner,
    })
}

/// Where `kuzz pair --hermes` looks for the Hermes plugin installer, relative
/// to the `kuzz` binary's directory or any directory above it (a repo
/// checkout's `target/release`, or an install that ships `plugins/`).
pub const HERMES_INSTALLER: &str = "plugins/platforms/kuzz/install.py";

/// The Hermes home the installer uses: `$HERMES_HOME`, else
/// `%LOCALAPPDATA%\hermes` on Windows, else `~/.hermes`.
fn hermes_home() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(home) = var("HERMES_HOME") {
        return Some(home);
    }
    if cfg!(windows) {
        if let Some(local) = var("LOCALAPPDATA") {
            return Some(local.join("hermes"));
        }
    }
    dirs::home_dir().map(|home| home.join(".hermes"))
}

fn find_hermes_installer() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .skip(1)
        .map(|dir| dir.join(HERMES_INSTALLER))
        .find(|path| path.is_file())
}

fn find_python() -> Option<&'static str> {
    ["python3", "python", "py"].into_iter().find(|python| {
        std::process::Command::new(python)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

/// After a pairing: install the Hermes plugin for `owner`, or say how.
/// Everything goes to stderr (stdout holds the pairing result).
fn install_hermes(owner: Option<&str>) {
    let owner_arg = owner.unwrap_or("<owner pubkey>");
    let installer = find_hermes_installer();
    let manual = format!(
        "To connect Hermes, run: python {} --owner {owner_arg}",
        installer
            .as_deref()
            .map_or_else(|| HERMES_INSTALLER.to_owned(), |p| p.display().to_string())
    );
    let home = hermes_home().filter(|home| home.is_dir());
    let (Some(installer), Some(home), Some(python), Some(owner)) =
        (installer, home, find_python(), owner)
    else {
        eprintln!("{manual}");
        return;
    };
    let output = std::process::Command::new(python)
        .arg(&installer)
        .arg("--owner")
        .arg(owner)
        .arg("--hermes-home")
        .arg(&home)
        .output();
    match output {
        Ok(output) => {
            eprint!("{}", String::from_utf8_lossy(&output.stdout));
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
            if output.status.success() {
                eprintln!(
                    "Hermes plugin installed in {}; restart Hermes",
                    home.display()
                );
            } else {
                eprintln!("warning: the Hermes plugin installer failed. {manual}");
            }
        }
        Err(error) => eprintln!("warning: could not run the Hermes installer ({error}). {manual}"),
    }
}

/// This OS user's device key, created when there is none (like a login).
async fn pair_device_key() -> Result<DeviceKey, CliError> {
    #[cfg(test)]
    {
        crate::device::process_signer()
            .await
            .and_then(|signer| signer.key().cloned())
            .ok_or_else(|| CliError::Other("no test device key".into()))
    }
    #[cfg(not(test))]
    {
        use kuzz_token_broker::device::{open_shared, OpenMode};
        match tokio::task::spawn_blocking(|| open_shared(OpenMode::CreateIfMissing)).await {
            Ok(Ok(Some(key))) => Ok(key),
            Ok(Ok(None)) => Err(CliError::Other("device key could not be created".into())),
            Ok(Err(error)) => Err(CliError::Other(format!(
                "this computer's device key is unavailable: {error}"
            ))),
            Err(error) => Err(CliError::Other(format!("device key task failed: {error}"))),
        }
    }
}

/// Publish the agent's display name (best effort: a warning on failure).
async fn publish_name(relay_url: &str, keys: &Keys, name: &str) {
    let result = async {
        let client =
            crate::client::KuzzClient::new(relay_url.to_owned(), keys.clone(), None, None)?;
        let builder = kuzz_sdk::build_profile(Some(name), None, None, None, None)
            .map_err(|e| CliError::Other(format!("build_profile failed: {e}")))?;
        let event = client.sign_event(builder)?;
        client.submit_event(event).await.map(|_| ())
    }
    .await;
    if let Err(error) = result {
        eprintln!("warning: connected, but the agent name was not published: {error}");
    }
}

/// `kuzz pair <code>`.
pub(crate) async fn cmd_pair(
    relay_url: &str,
    code: &str,
    agent_name: Option<String>,
    device_name: Option<String>,
    private_key: Option<String>,
    hermes: bool,
) -> Result<(), CliError> {
    if kuzz_token_broker::stored_session_disabled(|name| std::env::var(name).ok()) {
        return Err(CliError::Usage(format!(
            "pairing is disabled in this process ({})",
            kuzz_token_broker::DISABLE_STORED_SESSION_ENV
        )));
    }
    let code = code.trim();
    if !is_pair_code(code) {
        return Err(CliError::Usage(
            "invalid: this is not a pairing code; copy the whole `kuzz pair ...` command from the app again".into(),
        ));
    }
    let agent_name = agent_name.map(|name| name.trim().to_owned());
    if let Some(name) = &agent_name {
        validate_name("--name", name)?;
    }
    let device_name = device_name
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(default_pair_device_name);
    validate_name("--device-name", &device_name)?;
    let agent_keys = match private_key {
        Some(secret) => {
            let secret = Zeroizing::new(secret);
            Some(
                Keys::parse(secret.trim())
                    .map_err(|e| CliError::Key(format!("invalid KUZZ_PRIVATE_KEY: {e}")))?,
            )
        }
        None => None,
    };
    let store = PairingStore::open_default()
        .ok_or_else(|| CliError::Other("cannot locate a data directory for pairings".into()))?;
    let device = pair_device_key().await?;
    let keys_for_name = agent_keys.clone();
    let pairing = pair(
        PairRequest {
            relay_url,
            code,
            agent_name: agent_name.as_deref(),
            device_name: &device_name,
            agent_keys,
        },
        &device,
        &store,
    )
    .await?;
    let mut pairing = pairing;
    if let Some(keys) = keys_for_name.or_else(|| pairing.keys()) {
        if let Some(name) = &agent_name {
            publish_name(relay_url, &keys, name).await;
        }
        if pairing.owner.is_none() {
            pairing.owner = fetch_owner(relay_url, &keys).await;
            if pairing.owner.is_some() {
                let _ = store.save(&pairing);
            }
        }
    }
    let message = format!(
        "Connected as {} on {}",
        pairing.label(),
        pairing.device_name
    );
    eprintln!("{message}");
    println!(
        "{}",
        json!({
            "connected": true,
            "message": message,
            "agent_pubkey": pairing.agent_pubkey,
            "agent_name": pairing.agent_name,
            "device_name": pairing.device_name,
            "relay": pairing.relay,
            "pairing_id": pairing.pairing_id,
            "device_id": pairing.device_id,
            "owner": pairing.owner,
        })
    );
    if hermes {
        install_hermes(pairing.owner.as_deref());
    }
    Ok(())
}

#[cfg(test)]
#[path = "pairing_tests.rs"]
mod tests;
