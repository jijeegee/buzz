//! `kuzz messages watch`: stream new messages for this identity as JSON
//! lines (device-bound keys plan §6, §9: the transport of the Hermes
//! plugin and of any external agent).
//!
//! One WebSocket, authenticated like every other command (a paired agent's
//! NIP-42 AUTH carries this computer's device tag), with three kinds of
//! subscription:
//!
//! - membership: `kind:39002` member lists and `kind:44100` "added"
//!   notifications that `#p`-tag this identity. They give the channels and
//!   DMs it is in, including ones joined while watching;
//! - channels: the messages of those channels (up to [`CHANNELS_PER_SUB`]
//!   `#h` values per subscription);
//! - mentions: messages anywhere that `#p`-tag this identity.
//!
//! A lost connection is reopened with exponential backoff and resumes from
//! when the last message arrived here, by this computer's clock (minus
//! [`SINCE_SKEW_SECS`], never later than now; deduplicated by id). Authors'
//! `created_at` is not trusted for this: a sender whose clock runs ahead
//! would otherwise move the cursor past messages not yet received. An idle
//! connection is probed every [`IDLE_PING`]. In token mode the token is
//! renewed before every reconnect. Status goes to stderr; stdout holds only
//! messages, plus access lines with `--access-owner`.
//!
//! With `--access-owner` the agent's access setting is read first on every
//! connection, so no message outruns it, then again every
//! [`MEMBERS_REFRESH`]. Each read prints a `{"type":"access",...}` line, as
//! does a live change. Whatever can't be read or verified prints owner-only.
//!
//! While it runs, the watch reports this identity online the way `kuzz-acp`
//! does: a `kind:20001` once a connection is subscribed, then every
//! [`PRESENCE_EVERY`] on the same socket. Ctrl+C, SIGTERM or a closed stdout
//! report offline, best effort. Presence never ends the watch: a failure is
//! a line on stderr and the next beat tries again.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::io::Write as _;
use std::pin::Pin;
use std::time::{Duration, Instant};

use kuzz_core::kind::{
    KIND_MANAGED_AGENT, KIND_MEMBER_ADDED_NOTIFICATION, KIND_PRESENCE_UPDATE, KIND_STREAM_MESSAGE,
    KIND_STREAM_MESSAGE_DIFF, KIND_STREAM_MESSAGE_V2,
};
use kuzz_ws_client::{NostrWsConnection, RelayMessage, WsClientError};
use nostr::{EventBuilder, Kind};
use serde_json::{json, Value};

use crate::client::KuzzClient;
use crate::error::CliError;

/// Message kinds printed: stream messages (v1, v2) and diffs.
pub const MESSAGE_KINDS: [u32; 3] = [
    KIND_STREAM_MESSAGE,
    KIND_STREAM_MESSAGE_V2,
    KIND_STREAM_MESSAGE_DIFF,
];
/// NIP-29 member list (`d` = channel id).
const KIND_MEMBER_LIST: u32 = 39002;
const MEMBERS_SUB: &str = "kuzz-watch-members";
const ACCESS_SUB: &str = "kuzz-watch-access";
/// How long one read of the access setting may take.
const ACCESS_READ_LIMIT: Duration = Duration::from_secs(10);
const MENTIONS_SUB: &str = "kuzz-watch-mentions";
const CHANNELS_SUB_PREFIX: &str = "kuzz-watch-channels-";
const PING_SUB: &str = "kuzz-watch-ping";
/// `#h` values per channel subscription (the relay allows 128 per REQ).
pub const CHANNELS_PER_SUB: usize = 100;
/// Overlap when resuming after a reconnect.
pub const SINCE_SKEW_SECS: i64 = 60;
/// Probe the relay after this long without a frame.
pub const IDLE_PING: Duration = Duration::from_secs(60);
/// Reconnect when a probe gets no answer within this.
const PING_TIMEOUT: Duration = Duration::from_secs(30);
/// Re-read the member lists this often (new channels are also pushed live).
const MEMBERS_REFRESH: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
const TICK: Duration = Duration::from_secs(5);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A connection that lasted this long resets the backoff.
const STABLE_AFTER: Duration = Duration::from_secs(30);
/// Consecutive refused AUTHs before giving up (exit 3).
const MAX_AUTH_REFUSALS: u32 = 3;
/// Remembered message ids (dedupe across reconnects).
const SEEN_CAP: usize = 4096;
const NAME_CACHE_CAP: usize = 4096;
/// How often a running watch repeats "online" (as `kuzz-acp` does; the relay
/// forgets presence after 180 s).
pub const PRESENCE_EVERY: Duration = Duration::from_secs(60);
/// Budget for "offline" on the way out: a fresh connection, within the 5 s
/// the Hermes plugin grants after SIGTERM.
const OFFLINE_TIMEOUT: Duration = Duration::from_secs(3);

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The values of every `name` tag of `event`, in order.
fn tag_values<'a>(event: &'a nostr::Event, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    event.tags.iter().filter_map(move |tag| {
        let parts = tag.as_slice();
        (parts.first().map(String::as_str) == Some(name))
            .then(|| parts.get(1).map(String::as_str))
            .flatten()
    })
}

/// The thread root of a reply: the `e` tag marked `root`, else the one marked
/// `reply` (a direct reply names only its root).
pub fn thread_root(event: &nostr::Event) -> Option<String> {
    let marked = |marker: &str| {
        event.tags.iter().find_map(|tag| {
            let parts = tag.as_slice();
            (parts.first().map(String::as_str) == Some("e")
                && parts.get(3).map(String::as_str) == Some(marker))
            .then(|| parts.get(1).cloned())
            .flatten()
        })
    };
    marked("root").or_else(|| marked("reply"))
}

/// The JSON line for a message event (`None` for other kinds).
pub fn message_line(event: &nostr::Event, author_name: Option<&str>) -> Option<Value> {
    if !MESSAGE_KINDS.contains(&u32::from(event.kind.as_u16())) {
        return None;
    }
    let mut mentions: Vec<&str> = Vec::new();
    for pubkey in tag_values(event, "p") {
        if !mentions.contains(&pubkey) {
            mentions.push(pubkey);
        }
    }
    Some(json!({
        "id": event.id.to_hex(),
        "channel": tag_values(event, "h").next(),
        "author": event.pubkey.to_hex(),
        "author_name": author_name,
        "content": event.content,
        "created_at": event.created_at.as_secs(),
        "thread_root": thread_root(event),
        "mentions": mentions,
    }))
}

/// The channel a membership event is about: `d` of a member list that
/// includes us, `h` of an "added" notification.
fn membership_channel(event: &nostr::Event) -> Option<String> {
    let kind = u32::from(event.kind.as_u16());
    let channel = if kind == KIND_MEMBER_LIST {
        tag_values(event, "d").next()
    } else if kind == KIND_MEMBER_ADDED_NOTIFICATION {
        tag_values(event, "h").next()
    } else {
        None
    };
    channel
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
        .map(str::to_owned)
}

/// What this agent's owner has set, as far as the watch can tell.
#[derive(Clone, Debug, Default, PartialEq)]
enum Access {
    #[default]
    OwnerOnly,
    Allowlist(Vec<String>),
    Anyone,
}

impl Access {
    fn line(&self) -> Value {
        let (mode, allowlist) = match self {
            Self::OwnerOnly => ("owner-only", &[][..]),
            Self::Allowlist(people) => ("allowlist", &people[..]),
            Self::Anyone => ("anyone", &[][..]),
        };
        json!({ "type": "access", "mode": mode, "allowlist": allowlist })
    }

    /// The setting in a `kind:30177`, or `None` unless the owner really signed
    /// it for this agent. `verify` turns down server-stamped events too.
    fn from_event(event: &nostr::Event, owner: &str, me: &str) -> Option<Self> {
        let genuine = u32::from(event.kind.as_u16()) == KIND_MANAGED_AGENT
            && event.pubkey.to_hex() == owner
            && tag_values(event, "d").next() == Some(me)
            && event.verify().is_ok();
        if !genuine {
            return None;
        }
        let content: Value = serde_json::from_str(&event.content).unwrap_or(Value::Null);
        Some(match content["respond_to"].as_str() {
            Some("anyone") => Self::Anyone,
            Some("allowlist") => Self::Allowlist(
                content["respond_to_allowlist"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|person| crate::validate::validate_hex64(person).is_ok())
                    .map(str::to_ascii_lowercase)
                    .collect(),
            ),
            _ => Self::OwnerOnly,
        })
    }
}

/// Reading the access setting of `owner`.
struct AccessWatch {
    owner: String,
    current: Access,
    /// Newest `created_at` taken: a lagging replica can't undo a change.
    newest: u64,
    /// While a read runs: when it started, and what it has seen.
    reading: Option<Instant>,
    seen: Option<Access>,
}

impl AccessWatch {
    fn new(owner: String) -> Self {
        Self {
            owner,
            current: Access::default(),
            newest: 0,
            reading: None,
            seen: None,
        }
    }

    /// Start a read: the REQ to send.
    fn begin_read(&mut self, me: &str) -> Value {
        self.reading = Some(Instant::now());
        self.seen = None;
        json!([
            "REQ",
            ACCESS_SUB,
            { "kinds": [KIND_MANAGED_AGENT], "authors": [self.owner], "#d": [me], "limit": 1 },
        ])
    }

    /// Take an event. During a read it waits for the read's end; outside one
    /// it applies now, and the line to print is returned.
    fn on_event(&mut self, event: &nostr::Event, me: &str) -> Option<Value> {
        let created_at = event.created_at.as_secs();
        let access = match Access::from_event(event, &self.owner, me) {
            Some(_) if created_at < self.newest => self.current.clone(),
            Some(access) => {
                self.newest = created_at;
                access
            }
            None => Access::OwnerOnly,
        };
        if self.reading.is_some() {
            self.seen = Some(access);
            return None;
        }
        self.current = access;
        Some(self.current.line())
    }

    /// End a read: what it saw, or owner-only if nothing. The line to print.
    fn finish_read(&mut self) -> Value {
        self.reading = None;
        self.current = self.seen.take().unwrap_or_default();
        self.current.line()
    }

    fn read_overdue(&self) -> bool {
        self.reading
            .is_some_and(|started| started.elapsed() >= ACCESS_READ_LIMIT)
    }
}

/// The membership REQ.
pub fn members_req(me: &str) -> Value {
    json!([
        "REQ",
        MEMBERS_SUB,
        { "kinds": [KIND_MEMBER_LIST], "#p": [me] },
        { "kinds": [KIND_MEMBER_ADDED_NOTIFICATION], "#p": [me], "since": now_secs() },
    ])
}

/// The mentions REQ from `since`.
pub fn mentions_req(me: &str, since: i64) -> Value {
    json!(["REQ", MENTIONS_SUB, { "kinds": MESSAGE_KINDS, "#p": [me], "since": since }])
}

/// The channel REQs (one per [`CHANNELS_PER_SUB`] channels) from `since`.
pub fn channel_reqs(channels: &BTreeSet<String>, since: i64) -> Vec<Value> {
    let channels: Vec<&String> = channels.iter().collect();
    channels
        .chunks(CHANNELS_PER_SUB)
        .enumerate()
        .map(|(index, chunk)| {
            json!([
                "REQ",
                format!("{CHANNELS_SUB_PREFIX}{index}"),
                { "kinds": MESSAGE_KINDS, "#h": chunk, "since": since },
            ])
        })
        .collect()
}

/// Delay before reconnect attempt `attempt` (0-based): exponential from
/// [`BACKOFF_MIN`] to [`BACKOFF_MAX`], jittered down to half.
pub fn backoff_delay(attempt: u32) -> Duration {
    let ceiling = BACKOFF_MIN
        .saturating_mul(1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX))
        .min(BACKOFF_MAX);
    let half = ceiling / 2;
    half + half.mul_f64(rand::random::<f64>())
}

/// Relay refusals that end this identity's access to the relay for good
/// (`handlers/auth.rs`): parked as `revoked`.
const REVOKED_REFUSALS: [&str; 3] = [
    "not a relay member",
    "removed from this relay",
    "account was deleted",
];

/// The word a supervisor parks on (`signed_out`, `revoked`, `not_paired`)
/// for a refused AUTH that retrying cannot fix: the relay's
/// `restricted: signed_out` / `revoked`, a membership that ended (not a
/// member, removed, account deleted: `revoked`), and refusals of this key
/// from this computer (`device`, `unbound`, `owner`). `None`: worth retrying.
pub fn parked_reason(refusal: &str) -> Option<&'static str> {
    let refusal = refusal.trim();
    let lower = refusal.to_ascii_lowercase();
    if REVOKED_REFUSALS.iter().any(|phrase| lower.contains(phrase)) {
        return Some("revoked");
    }
    let reason = refusal
        .strip_prefix("restricted:")?
        .trim()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .next()?;
    match reason {
        "signed_out" => Some("signed_out"),
        "revoked" => Some("revoked"),
        "device" | "unbound" | "owner" => Some("not_paired"),
        _ => None,
    }
}

/// The start of the "no credential at all" error (`missing_credential`).
pub const NO_CREDENTIAL: &str = "no credential:";

/// The word a supervisor parks on for an auth error while setting up the
/// watch's identity: no credential at all is `not_paired`; an ended login or
/// a token the relay calls invalid, expired, revoked or disabled is
/// `signed_out`. `None` for anything else (an unreadable `/auth/me` answer,
/// a broker hiccup): not a reason to park, the supervisor retries.
pub fn credential_word(message: &str) -> Option<&'static str> {
    let message = message.trim();
    if message.starts_with(NO_CREDENTIAL) {
        return Some("not_paired");
    }
    let terminal_token = crate::client::TERMINAL_TOKEN_CODES
        .iter()
        .any(|code| message.starts_with(&format!("{code}:")));
    if terminal_token
        || message.contains("the stored login has ended")
        || message.starts_with("access token rejected")
    {
        return Some("signed_out");
    }
    None
}

/// A fresh token-mode client (a renewed token), being made.
pub type ReauthFuture = Pin<Box<dyn Future<Output = Result<KuzzClient, CliError>>>>;
/// Makes a [`ReauthFuture`] before each reconnect.
pub type Reauth = Box<dyn Fn() -> ReauthFuture>;

/// What survives reconnects.
struct Watch {
    me: String,
    /// Where resubscriptions start: the local time the newest message
    /// arrived, minus [`SINCE_SKEW_SECS`] (or `--since`).
    cursor: i64,
    channels: BTreeSet<String>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    names: HashMap<String, Option<String>>,
    access: Option<AccessWatch>,
}

impl Watch {
    fn since(&self) -> i64 {
        self.cursor.min(now_secs()).max(0)
    }

    /// A message arrived now: everything up to now (minus skew) was seen.
    fn received(&mut self) {
        self.cursor = self.cursor.max(now_secs().saturating_sub(SINCE_SKEW_SECS));
    }

    /// Record `id`; false when it was seen already.
    fn first_sighting(&mut self, id: String) -> bool {
        if !self.seen.insert(id.clone()) {
            return false;
        }
        self.seen_order.push_back(id);
        while self.seen_order.len() > SEEN_CAP {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }

    /// The author's display name from their profile (cached; `None` when
    /// they have none or it cannot be read).
    async fn author_name(&mut self, client: &KuzzClient, pubkey: &str) -> Option<String> {
        if let Some(name) = self.names.get(pubkey) {
            return name.clone();
        }
        let filter = json!({ "kinds": [0], "authors": [pubkey], "limit": 1 });
        let name = client
            .query(&filter)
            .await
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<Value>>(&raw).ok())
            .and_then(|events| events.into_iter().next())
            .and_then(|event| {
                let profile: Value = serde_json::from_str(event["content"].as_str()?).ok()?;
                ["display_name", "name"].iter().find_map(|field| {
                    profile[*field]
                        .as_str()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                })
            });
        if self.names.len() >= NAME_CACHE_CAP {
            self.names.clear();
        }
        self.names.insert(pubkey.to_owned(), name.clone());
        name
    }
}

/// Why a connection ended.
enum End {
    /// Reconnect after a backoff.
    Lost(String),
    /// The relay refused AUTH.
    Refused(String),
    /// Stop watching (stdout is gone).
    Fatal(CliError),
}

fn ws_lost(error: WsClientError) -> End {
    match error {
        WsClientError::AuthFailed(message) => End::Refused(message),
        other => End::Lost(other.to_string()),
    }
}

async fn send(conn: &mut NostrWsConnection, frame: &Value) -> Result<(), End> {
    conn.send_raw(frame).await.map_err(ws_lost)
}

/// End the access read and print it, then on a new connection open the rest.
async fn finish_access_read(
    conn: &mut NostrWsConnection,
    watch: &mut Watch,
    subscribed: &mut bool,
) -> Result<(), End> {
    if let Some(access) = watch.access.as_mut() {
        emit(&access.finish_read())?;
    }
    if !*subscribed {
        *subscribed = true;
        send(conn, &members_req(&watch.me)).await?;
        send(conn, &mentions_req(&watch.me, watch.since())).await?;
    }
    Ok(())
}

async fn subscribe_channels(conn: &mut NostrWsConnection, watch: &Watch) -> Result<(), End> {
    for req in channel_reqs(&watch.channels, watch.since()) {
        send(conn, &req).await?;
    }
    Ok(())
}

/// The `kind:20001` for `status`: the bare status as content and no tags, as
/// `kuzz-acp` publishes it.
fn presence_event(client: &KuzzClient, status: &str) -> Result<nostr::Event, CliError> {
    client.sign_event(EventBuilder::new(
        Kind::Custom(KIND_PRESENCE_UPDATE as u16),
        status,
    ))
}

/// Report online on `conn` without waiting: the relay's answer arrives in the
/// read loop. A failure is only logged; a dead socket shows up there too.
async fn announce_online(client: &KuzzClient, conn: &mut NostrWsConnection) {
    let sent = match presence_event(client, "online") {
        Ok(event) => conn
            .send_raw(&json!(["EVENT", event]))
            .await
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    if let Err(error) = sent {
        eprintln!("kuzz watch: could not report online: {error}");
    }
}

/// Report offline on a connection of its own (the watch's socket may be gone
/// by now), within [`OFFLINE_TIMEOUT`].
async fn announce_offline(client: &KuzzClient) {
    let published = async {
        client
            .publish_ephemeral_event(presence_event(client, "offline")?)
            .await
    };
    match tokio::time::timeout(OFFLINE_TIMEOUT, published).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => eprintln!("kuzz watch: could not report offline: {error}"),
        Err(_) => eprintln!("kuzz watch: reporting offline timed out"),
    }
}

/// Print one line and flush; a closed stdout ends the watch.
fn emit(line: &Value) -> Result<(), End> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")
        .and_then(|()| out.flush())
        .map_err(|e| End::Fatal(CliError::Other(format!("stdout closed: {e}"))))
}

/// One connection: subscribe, report online every `presence_every`, and print
/// messages until it is lost.
async fn run_connection(client: &KuzzClient, watch: &mut Watch, presence_every: Duration) -> End {
    let mut conn = match tokio::time::timeout(CONNECT_TIMEOUT, client.ws_connect()).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(error)) => return ws_lost(error),
        Err(_) => return End::Lost("connect timed out".into()),
    };
    let result = async {
        // Members and mentions wait for the access read, so nothing outruns it.
        let mut subscribed = false;
        if let Some(access) = watch.access.as_mut() {
            send(&mut conn, &access.begin_read(&watch.me)).await?;
        } else {
            finish_access_read(&mut conn, watch, &mut subscribed).await?;
        }
        let mut members_ready = false;
        let mut last_frame = Instant::now();
        let mut members_at = Instant::now();
        let mut ping_sent: Option<Instant> = None;
        // Set once subscribed: online means messages are being received.
        let mut presence_at: Option<Instant> = None;
        loop {
            // Here, not in the timeout arm below: a busy channel never idles.
            if presence_at.is_some_and(|at| at.elapsed() >= presence_every) {
                announce_online(client, &mut conn).await;
                presence_at = Some(Instant::now());
            }
            if watch.access.as_ref().is_some_and(AccessWatch::read_overdue) {
                finish_access_read(&mut conn, watch, &mut subscribed).await?;
            }
            if members_ready && members_at.elapsed() > MEMBERS_REFRESH {
                send(&mut conn, &members_req(&watch.me)).await?;
                if let Some(access) = watch.access.as_mut() {
                    send(&mut conn, &access.begin_read(&watch.me)).await?;
                }
                members_at = Instant::now();
            }
            let message = match conn.next_event(TICK).await {
                Ok(message) => message,
                Err(WsClientError::Timeout) => {
                    if let Some(sent) = ping_sent {
                        if sent.elapsed() > PING_TIMEOUT {
                            return Err(End::Lost("relay stopped answering".into()));
                        }
                    } else if last_frame.elapsed() > IDLE_PING {
                        let probe = json!([
                            "REQ",
                            PING_SUB,
                            { "kinds": [0], "authors": [watch.me], "limit": 1 },
                        ]);
                        send(&mut conn, &probe).await?;
                        ping_sent = Some(Instant::now());
                    }
                    continue;
                }
                // An unreadable frame is skipped; the socket is still fine.
                Err(WsClientError::Json(_) | WsClientError::UnexpectedMessage(_)) => continue,
                Err(error) => return Err(ws_lost(error)),
            };
            last_frame = Instant::now();
            match message {
                RelayMessage::Event {
                    subscription_id,
                    event,
                } if subscription_id == MEMBERS_SUB => {
                    if let Some(channel) = membership_channel(&event) {
                        if watch.channels.insert(channel) && members_ready {
                            subscribe_channels(&mut conn, watch).await?;
                        }
                    }
                }
                RelayMessage::Event {
                    subscription_id,
                    event,
                } if subscription_id == ACCESS_SUB => {
                    let me = &watch.me;
                    let line = watch.access.as_mut().and_then(|a| a.on_event(&event, me));
                    if let Some(line) = line {
                        emit(&line)?;
                    }
                }
                RelayMessage::Event {
                    subscription_id, ..
                } if subscription_id == PING_SUB => {}
                RelayMessage::Event { event, .. } => {
                    if !MESSAGE_KINDS.contains(&u32::from(event.kind.as_u16()))
                        || !watch.first_sighting(event.id.to_hex())
                    {
                        continue;
                    }
                    watch.received();
                    let author = event.pubkey.to_hex();
                    // Our own messages are not news to us.
                    if author == watch.me {
                        continue;
                    }
                    let name = watch.author_name(client, &author).await;
                    if let Some(line) = message_line(&event, name.as_deref()) {
                        emit(&line)?;
                    }
                }
                RelayMessage::Eose { subscription_id }
                    if subscription_id == ACCESS_SUB
                        && watch.access.as_ref().is_some_and(|a| a.reading.is_some()) =>
                {
                    finish_access_read(&mut conn, watch, &mut subscribed).await?;
                }
                RelayMessage::Eose { subscription_id }
                    if subscription_id == MEMBERS_SUB && !members_ready =>
                {
                    members_ready = true;
                    members_at = Instant::now();
                    subscribe_channels(&mut conn, watch).await?;
                    eprintln!(
                        "kuzz watch: connected; watching {} channel(s) and mentions",
                        watch.channels.len()
                    );
                    announce_online(client, &mut conn).await;
                    presence_at = Some(Instant::now());
                }
                RelayMessage::Eose { subscription_id } if subscription_id == PING_SUB => {
                    ping_sent = None;
                    send(&mut conn, &json!(["CLOSE", PING_SUB])).await?;
                }
                // Presence is the only thing a watch publishes.
                RelayMessage::Ok(ok) if !ok.accepted => {
                    eprintln!("kuzz watch: relay refused presence: {}", ok.message);
                }
                RelayMessage::Closed {
                    subscription_id,
                    message,
                } => {
                    eprintln!("kuzz watch: relay closed {subscription_id}: {message}");
                    if subscription_id == PING_SUB {
                        ping_sent = None;
                    }
                    if subscription_id == ACCESS_SUB {
                        // Nothing more will come: owner-only until the next read.
                        if let Some(access) = watch.access.as_mut() {
                            access.seen = None;
                        }
                        finish_access_read(&mut conn, watch, &mut subscribed).await?;
                    }
                }
                RelayMessage::Notice { message } => {
                    eprintln!("kuzz watch: relay notice: {message}");
                }
                _ => {}
            }
        }
    }
    .await;
    let end = match result {
        Ok(()) => End::Lost("closed".into()),
        Err(end) => end,
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), conn.disconnect()).await;
    end
}

/// `kuzz messages watch`. `reauth` (token mode) renews the client before
/// every reconnect, so a watch outlives its access token.
pub async fn cmd_watch(
    client: &KuzzClient,
    since: Option<i64>,
    access_owner: Option<String>,
    reauth: Option<Reauth>,
) -> Result<(), CliError> {
    if let Some(owner) = &access_owner {
        crate::validate::validate_hex64(owner)?;
    }
    let access = access_owner.map(|owner| AccessWatch::new(owner.to_ascii_lowercase()));
    watch_until(client, since, access, reauth, stop_requested()).await
}

/// Ctrl+C or, on Unix, SIGTERM. Listening replaces the default exit, so every
/// path out of a watch comes through here. A handler that cannot be installed
/// just never fires.
async fn stop_requested() {
    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                terminate.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    let interrupt = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        () = terminate => {}
        () = interrupt => {}
    }
}

/// Watch until `stop` resolves, then report offline and return `Ok`.
async fn watch_until(
    client: &KuzzClient,
    since: Option<i64>,
    access: Option<AccessWatch>,
    reauth: Option<Reauth>,
    stop: impl Future<Output = ()>,
) -> Result<(), CliError> {
    // Out here so the newest login is still at hand once the watch is dropped.
    let mut renewed: Option<KuzzClient> = None;
    tokio::select! {
        result = watch_loop(client, since, access, reauth, &mut renewed) => return result,
        () = stop => {}
    }
    announce_offline(renewed.as_ref().unwrap_or(client)).await;
    Ok(())
}

/// Connect and reconnect until a final error. `renewed` holds the newest
/// client in token mode.
async fn watch_loop(
    client: &KuzzClient,
    since: Option<i64>,
    access: Option<AccessWatch>,
    reauth: Option<Reauth>,
    renewed: &mut Option<KuzzClient>,
) -> Result<(), CliError> {
    let mut watch = Watch {
        me: client.pubkey().to_hex(),
        cursor: since.unwrap_or_else(now_secs),
        channels: BTreeSet::new(),
        seen: HashSet::new(),
        seen_order: VecDeque::new(),
        names: HashMap::new(),
        access,
    };
    let mut attempt: u32 = 0;
    let mut refusals: u32 = 0;
    let mut first = true;
    loop {
        if let (false, Some(reauth)) = (first, &reauth) {
            match reauth().await {
                Ok(fresh) => *renewed = Some(fresh),
                Err(CliError::Auth(message)) if credential_word(&message).is_some() => {
                    let word = credential_word(&message).unwrap_or("signed_out");
                    return Err(CliError::Auth(format!("{word}: {message}")));
                }
                // Transient (network, relay, an odd answer): keep the old
                // client and try again after the backoff.
                Err(error) => eprintln!("kuzz watch: could not renew the login: {error}"),
            }
        }
        first = false;
        let current = renewed.as_ref().unwrap_or(client);
        let started = Instant::now();
        let reason = match run_connection(current, &mut watch, PRESENCE_EVERY).await {
            End::Fatal(error) => {
                announce_offline(current).await;
                return Err(error);
            }
            End::Refused(message) => {
                if let Some(word) = parked_reason(&message) {
                    return Err(CliError::Auth(format!(
                        "{word}: relay rejected WebSocket auth: {message}"
                    )));
                }
                refusals += 1;
                if refusals >= MAX_AUTH_REFUSALS {
                    // Refused even with a renewed token: the login is over.
                    let prefix = if reauth.is_some() { "signed_out: " } else { "" };
                    return Err(CliError::Auth(format!(
                        "{prefix}relay rejected WebSocket auth: {message}"
                    )));
                }
                format!("auth refused: {message}")
            }
            End::Lost(reason) => {
                refusals = 0;
                reason
            }
        };
        if started.elapsed() >= STABLE_AFTER {
            attempt = 0;
        }
        let delay = backoff_delay(attempt);
        attempt = attempt.saturating_add(1);
        eprintln!(
            "kuzz watch: disconnected ({reason}); reconnecting in {:.1}s",
            delay.as_secs_f64()
        );
        tokio::time::sleep(delay).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use nostr::{Keys, Tag};
    use tokio::sync::mpsc;

    fn event(kind: u16, tags: &[&[&str]], keys: &Keys) -> nostr::Event {
        let tags: Vec<Tag> = tags
            .iter()
            .map(|t| Tag::parse(t.to_vec()).unwrap())
            .collect();
        EventBuilder::new(Kind::Custom(kind), "hello")
            .tags(tags)
            .sign_with_keys(keys)
            .unwrap()
    }

    const CHANNEL: &str = "6f0a3a5e-1f6b-4b0a-9a51-0b3f1d2c3e4f";
    const ROOT: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const PARENT: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const ALICE: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    #[test]
    fn message_lines_have_the_documented_schema() {
        let keys = Keys::generate();
        let ev = event(
            9,
            &[
                &["h", CHANNEL],
                &["e", ROOT, "", "root"],
                &["e", PARENT, "", "reply"],
                &["p", ALICE],
                &["p", ALICE],
            ],
            &keys,
        );
        let line = message_line(&ev, Some("Bob")).expect("a message");
        let mut fields: Vec<&str> = line
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "author",
                "author_name",
                "channel",
                "content",
                "created_at",
                "id",
                "mentions",
                "thread_root",
            ]
        );
        assert_eq!(line["id"], ev.id.to_hex());
        assert_eq!(line["channel"], CHANNEL);
        assert_eq!(line["author"], keys.public_key().to_hex());
        assert_eq!(line["author_name"], "Bob");
        assert_eq!(line["content"], "hello");
        assert_eq!(line["created_at"], ev.created_at.as_secs());
        assert_eq!(line["thread_root"], ROOT);
        assert_eq!(line["mentions"], json!([ALICE]));
        // One line of JSON.
        assert!(!line.to_string().contains('\n'));
    }

    #[test]
    fn top_level_and_direct_replies() {
        let keys = Keys::generate();
        let top = message_line(&event(9, &[&["h", CHANNEL]], &keys), None).unwrap();
        assert_eq!(top["thread_root"], Value::Null);
        assert_eq!(top["author_name"], Value::Null);
        assert_eq!(top["mentions"], json!([]));
        let direct = event(9, &[&["h", CHANNEL], &["e", ROOT, "", "reply"]], &keys);
        assert_eq!(thread_root(&direct).as_deref(), Some(ROOT));
        // Unmarked e tags (quotes, references) are not threads.
        let quoted = event(9, &[&["h", CHANNEL], &["e", ROOT]], &keys);
        assert_eq!(thread_root(&quoted), None);
    }

    #[test]
    fn only_message_kinds_are_printed() {
        let keys = Keys::generate();
        assert!(message_line(&event(40002, &[&["h", CHANNEL]], &keys), None).is_some());
        assert!(message_line(&event(40008, &[&["h", CHANNEL]], &keys), None).is_some());
        assert!(message_line(&event(7, &[&["h", CHANNEL]], &keys), None).is_none());
        assert!(message_line(&event(40003, &[&["h", CHANNEL]], &keys), None).is_none());
    }

    #[test]
    fn membership_events_name_their_channel() {
        let keys = Keys::generate();
        let list = event(39002, &[&["d", CHANNEL], &["p", ALICE]], &keys);
        assert_eq!(membership_channel(&list).as_deref(), Some(CHANNEL));
        let added = event(44100, &[&["h", CHANNEL], &["p", ALICE]], &keys);
        assert_eq!(membership_channel(&added).as_deref(), Some(CHANNEL));
        let bad = event(39002, &[&["d", "not-a-uuid"]], &keys);
        assert_eq!(membership_channel(&bad), None);
        let other = event(9, &[&["h", CHANNEL]], &keys);
        assert_eq!(membership_channel(&other), None);
    }

    #[test]
    fn subscriptions_cover_channels_and_mentions_within_relay_limits() {
        let channels: BTreeSet<String> = (0..250)
            .map(|i| format!("00000000-0000-4000-8000-{i:012}"))
            .collect();
        let reqs = channel_reqs(&channels, 1_700_000_000);
        assert_eq!(reqs.len(), 3);
        let mut covered = BTreeSet::new();
        for (index, req) in reqs.iter().enumerate() {
            assert_eq!(req[0], "REQ");
            assert_eq!(req[1], format!("{CHANNELS_SUB_PREFIX}{index}"));
            assert_eq!(req[2]["since"], 1_700_000_000);
            assert_eq!(req[2]["kinds"], json!(MESSAGE_KINDS));
            let h = req[2]["#h"].as_array().unwrap();
            assert!(h.len() <= CHANNELS_PER_SUB);
            covered.extend(h.iter().map(|v| v.as_str().unwrap().to_owned()));
        }
        assert_eq!(covered, channels);
        assert!(channel_reqs(&BTreeSet::new(), 0).is_empty());

        let mentions = mentions_req(ALICE, 5);
        assert_eq!(mentions[2]["#p"], json!([ALICE]));
        assert_eq!(mentions[2]["since"], 5);
        let members = members_req(ALICE);
        assert_eq!(members[2]["kinds"], json!([39002]));
        assert_eq!(members[2]["#p"], json!([ALICE]));
        assert_eq!(members[3]["kinds"], json!([44100]));
    }

    #[test]
    fn final_refusals_name_the_parking_word() {
        assert_eq!(parked_reason("restricted: signed_out"), Some("signed_out"));
        assert_eq!(parked_reason("restricted: revoked"), Some("revoked"));
        assert_eq!(parked_reason("restricted: device"), Some("not_paired"));
        assert_eq!(parked_reason("restricted: unbound"), Some("not_paired"));
        assert_eq!(parked_reason("restricted: owner"), Some("not_paired"));
        assert_eq!(parked_reason("error: internal error checking device"), None);
        assert_eq!(parked_reason("auth-required: stale challenge"), None);
        assert_eq!(parked_reason("restricted: rate limited"), None);
        assert_eq!(
            parked_reason("restricted: not a relay member"),
            Some("revoked")
        );
        assert_eq!(
            parked_reason("restricted: you were removed from this relay"),
            Some("revoked")
        );
        assert_eq!(
            parked_reason("restricted: this account was deleted"),
            Some("revoked")
        );
    }

    #[test]
    fn credential_errors_park_only_when_final() {
        assert_eq!(
            credential_word("no credential: run `kuzz auth login`"),
            Some("not_paired")
        );
        assert_eq!(
            credential_word("refresh_revoked: the stored login has ended; run `kuzz auth login`"),
            Some("signed_out")
        );
        assert_eq!(
            credential_word("access token rejected: {}"),
            Some("signed_out")
        );
        assert_eq!(
            credential_word("token_revoked: token was revoked"),
            Some("signed_out")
        );
        assert_eq!(
            credential_word("invalid /auth/me response: EOF while parsing"),
            None
        );
        assert_eq!(
            credential_word("/auth/me returned no valid principal_id"),
            None
        );
    }

    #[test]
    fn the_resume_point_is_local_time_never_the_future() {
        let mut watch = Watch {
            me: ALICE.into(),
            cursor: now_secs() + 3600,
            channels: BTreeSet::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            names: HashMap::new(),
            access: None,
        };
        // A `--since` (or any cursor) in the future is clamped to now.
        assert!(watch.since() <= now_secs());
        watch.cursor = 100;
        let before = now_secs();
        watch.received();
        assert!(watch.since() >= before - SINCE_SKEW_SECS);
        assert!(watch.since() <= now_secs());
    }

    #[test]
    fn backoff_grows_to_a_minute_with_jitter() {
        for attempt in 0..40 {
            let delay = backoff_delay(attempt);
            let ceiling = BACKOFF_MIN
                .saturating_mul(1u32 << attempt.min(16))
                .min(BACKOFF_MAX);
            assert!(
                delay >= ceiling / 2 && delay <= ceiling,
                "{attempt}: {delay:?}"
            );
        }
        assert!(backoff_delay(30) >= BACKOFF_MAX / 2);
    }

    /// What a [`fake_relay`] does beyond the handshake.
    #[derive(Clone, Copy)]
    struct RelayScript {
        /// Answer EVENTs with a refusal.
        refuse: bool,
        /// Send a frame every few ms, so the read loop never idles.
        busy: bool,
    }

    /// A relay that challenges, accepts AUTH, ends the membership
    /// subscription and hands every EVENT it receives to the channel.
    async fn fake_relay(script: RelayScript) -> (String, mpsc::UnboundedReceiver<nostr::Event>) {
        let (events, received) = mpsc::unbounded_channel();
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let events = events.clone();
                async move { ws.on_upgrade(move |socket| fake_session(socket, script, events)) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (url, received)
    }

    async fn fake_session(
        mut socket: WebSocket,
        script: RelayScript,
        events: mpsc::UnboundedSender<nostr::Event>,
    ) {
        let frame = |value: Value| Message::Text(value.to_string().into());
        let _ = socket.send(frame(json!(["AUTH", "challenge"]))).await;
        let mut noise = tokio::time::interval(Duration::from_millis(5));
        loop {
            let incoming = tokio::select! {
                incoming = socket.recv() => incoming,
                _ = noise.tick(), if script.busy => {
                    let _ = socket.send(frame(json!(["EOSE", "noise"]))).await;
                    continue;
                }
            };
            let Some(Ok(Message::Text(text))) = incoming else {
                return;
            };
            let value: Value = serde_json::from_str(text.as_str()).unwrap();
            let reply = match value[0].as_str() {
                Some("AUTH") => json!(["OK", value[1]["id"], true, ""]),
                Some("REQ") if value[1] == MEMBERS_SUB => json!(["EOSE", MEMBERS_SUB]),
                Some("EVENT") => {
                    let event: nostr::Event = serde_json::from_value(value[1].clone()).unwrap();
                    let id = event.id.to_hex();
                    let _ = events.send(event);
                    if script.refuse {
                        json!(["OK", id, false, "restricted: no presence today"])
                    } else {
                        json!(["OK", id, true, ""])
                    }
                }
                _ => continue,
            };
            let _ = socket.send(frame(reply)).await;
        }
    }

    fn assert_presence(event: &nostr::Event, status: &str, keys: &Keys) {
        assert_eq!(u32::from(event.kind.as_u16()), KIND_PRESENCE_UPDATE);
        assert_eq!(event.content, status);
        assert_eq!(event.pubkey, keys.public_key());
        assert!(event.tags.is_empty(), "the shape kuzz-acp publishes");
        event.verify().expect("a valid signature");
    }

    #[tokio::test]
    async fn online_repeats_on_a_busy_socket_and_survives_refusals() {
        let (url, mut events) = fake_relay(RelayScript {
            refuse: true,
            busy: true,
        })
        .await;
        let keys = Keys::generate();
        let client = KuzzClient::new(url, keys.clone(), None, None).unwrap();
        let mut watch = Watch {
            me: keys.public_key().to_hex(),
            cursor: now_secs(),
            channels: BTreeSet::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            names: HashMap::new(),
            access: None,
        };
        let run = run_connection(&client, &mut watch, Duration::from_millis(20));
        tokio::pin!(run);
        for _ in 0..3 {
            let event = tokio::select! {
                _ = &mut run => panic!("presence must not end the connection"),
                event = tokio::time::timeout(Duration::from_secs(4), events.recv()) => {
                    event.expect("the beat keeps going while frames arrive").unwrap()
                }
            };
            assert_presence(&event, "online", &keys);
        }
    }

    #[tokio::test]
    async fn stopping_reports_offline_after_online() {
        let (url, mut events) = fake_relay(RelayScript {
            refuse: false,
            busy: false,
        })
        .await;
        let keys = Keys::generate();
        let client = KuzzClient::new(url, keys.clone(), None, None).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let run = watch_until(&client, None, None, None, async {
            let _ = stopped.await;
        });
        tokio::pin!(run);
        let online = tokio::select! {
            result = &mut run => panic!("ended early: {:?}", result.err()),
            event = tokio::time::timeout(Duration::from_secs(4), events.recv()) => {
                event.expect("online once subscribed").unwrap()
            }
        };
        assert_presence(&online, "online", &keys);
        stop.send(()).unwrap();
        let (result, offline) = tokio::join!(
            run,
            tokio::time::timeout(Duration::from_secs(4), events.recv())
        );
        assert!(result.is_ok());
        assert_presence(
            &offline.expect("offline on stop").unwrap(),
            "offline",
            &keys,
        );
    }

    /// A `kind:30177` for agent `me`, signed by `keys`.
    fn setting(keys: &Keys, me: &str, content: &str, at: u64) -> nostr::Event {
        EventBuilder::new(Kind::Custom(KIND_MANAGED_AGENT as u16), content)
            .tags([Tag::parse(["d", me]).unwrap()])
            .custom_created_at(nostr::Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }

    fn access_watch(owner: &Keys) -> AccessWatch {
        AccessWatch::new(owner.public_key().to_hex())
    }

    #[test]
    fn a_setting_counts_only_from_the_owner_for_this_agent() {
        let owner = Keys::generate();
        let hex = owner.public_key().to_hex();
        let read = |event: &nostr::Event| Access::from_event(event, &hex, ALICE);
        let anyone = r#"{"respond_to":"anyone"}"#;
        assert_eq!(
            read(&setting(&owner, ALICE, anyone, 1)),
            Some(Access::Anyone)
        );
        assert_eq!(read(&setting(&Keys::generate(), ALICE, anyone, 1)), None);
        assert_eq!(read(&setting(&owner, ROOT, anyone, 1)), None);
        let other_kind = EventBuilder::new(Kind::Custom(30175), anyone)
            .tags([Tag::parse(["d", ALICE]).unwrap()])
            .sign_with_keys(&owner)
            .unwrap();
        assert_eq!(read(&other_kind), None);

        // The relay can't change it, and its stamp on a server draft is no signature.
        let mut changed = serde_json::to_value(setting(&owner, ALICE, "{}", 1)).unwrap();
        changed["content"] = json!(anyone);
        let changed: nostr::Event = serde_json::from_value(changed).unwrap();
        assert_eq!(read(&changed), None);
    }

    #[test]
    fn a_setting_reads_its_mode_narrowly() {
        let owner = Keys::generate();
        let hex = owner.public_key().to_hex();
        let read = |content: &str| {
            Access::from_event(&setting(&owner, ALICE, content, 1), &hex, ALICE).unwrap()
        };
        let list = format!(
            r#"{{"respond_to":"allowlist","respond_to_allowlist":["{}","79be667e",7]}}"#,
            ALICE.to_ascii_uppercase()
        );
        assert_eq!(read(&list), Access::Allowlist(vec![ALICE.to_owned()]));
        assert_eq!(
            read(r#"{"respond_to":"allowlist"}"#),
            Access::Allowlist(vec![])
        );
        assert_eq!(read(r#"{"respond_to":"owner-only"}"#), Access::OwnerOnly);
        assert_eq!(read(r#"{"respond_to":"everyone"}"#), Access::OwnerOnly);
        assert_eq!(read(r#"{"respond_to":7}"#), Access::OwnerOnly);
        assert_eq!(read("not json"), Access::OwnerOnly);
    }

    #[test]
    fn a_read_prints_what_it_saw_or_owner_only() {
        let owner = Keys::generate();
        let anyone = setting(&owner, ALICE, r#"{"respond_to":"anyone"}"#, 10);
        let mut access = access_watch(&owner);

        let req = access.begin_read(ALICE);
        assert_eq!(req[1], ACCESS_SUB);
        assert_eq!(req[2]["kinds"], json!([KIND_MANAGED_AGENT]));
        assert_eq!(req[2]["authors"], json!([owner.public_key().to_hex()]));
        assert_eq!(req[2]["#d"], json!([ALICE]));
        assert_eq!(
            access.on_event(&anyone, ALICE),
            None,
            "waits for the read's end"
        );
        assert_eq!(access.finish_read()["mode"], "anyone");

        // A deleted setting: the next read finds nothing.
        access.begin_read(ALICE);
        assert_eq!(
            access.finish_read(),
            json!({ "type": "access", "mode": "owner-only", "allowlist": [] })
        );
    }

    #[test]
    fn a_live_change_prints_at_once_and_a_stale_copy_changes_nothing() {
        let owner = Keys::generate();
        let at = |content: &str, at: u64| setting(&owner, ALICE, content, at);
        let mut access = access_watch(&owner);
        let newer = at(r#"{"respond_to":"anyone"}"#, 20);
        assert_eq!(access.on_event(&newer, ALICE).unwrap()["mode"], "anyone");
        let narrowed = at(r#"{"respond_to":"owner-only"}"#, 30);
        assert_eq!(
            access.on_event(&narrowed, ALICE).unwrap()["mode"],
            "owner-only"
        );

        // A lagging replica serves the wider one again, live or in a read.
        assert_eq!(
            access.on_event(&newer, ALICE).unwrap()["mode"],
            "owner-only"
        );
        access.begin_read(ALICE);
        assert_eq!(access.on_event(&newer, ALICE), None);
        assert_eq!(access.finish_read()["mode"], "owner-only");

        // A bad event never widens.
        access.current = Access::Anyone;
        let forged = setting(&Keys::generate(), ALICE, r#"{"respond_to":"anyone"}"#, 40);
        assert_eq!(
            access.on_event(&forged, ALICE).unwrap()["mode"],
            "owner-only"
        );
    }

    #[test]
    fn an_overdue_read_uses_what_it_saw() {
        let owner = Keys::generate();
        let mut access = access_watch(&owner);
        assert!(!access.read_overdue(), "no read, nothing overdue");
        access.begin_read(ALICE);
        assert!(!access.read_overdue());
        access.reading = Some(Instant::now() - ACCESS_READ_LIMIT);
        assert!(access.read_overdue());
        let anyone = setting(&owner, ALICE, r#"{"respond_to":"anyone"}"#, 10);
        access.on_event(&anyone, ALICE);
        assert_eq!(access.finish_read()["mode"], "anyone");
        assert!(!access.read_overdue());
    }

    /// A relay that answers the access REQ with `answer` and reports every
    /// REQ's subscription id.
    async fn access_relay(answer: &'static str) -> (String, mpsc::UnboundedReceiver<String>) {
        let (reqs, received) = mpsc::unbounded_channel();
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move |ws: WebSocketUpgrade| {
                let reqs = reqs.clone();
                async move { ws.on_upgrade(move |socket| access_session(socket, answer, reqs)) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        (url, received)
    }

    async fn access_session(
        mut socket: WebSocket,
        answer: &'static str,
        reqs: mpsc::UnboundedSender<String>,
    ) {
        let frame = |value: Value| Message::Text(value.to_string().into());
        let _ = socket.send(frame(json!(["AUTH", "challenge"]))).await;
        while let Some(Ok(Message::Text(text))) = socket.recv().await {
            let value: Value = serde_json::from_str(text.as_str()).unwrap();
            match value[0].as_str() {
                Some("AUTH") => {
                    let ok = json!(["OK", value[1]["id"], true, ""]);
                    let _ = socket.send(frame(ok)).await;
                }
                Some("REQ") => {
                    let _ = reqs.send(value[1].as_str().unwrap().to_owned());
                    if value[1] == ACCESS_SUB {
                        let reply = json!([answer, ACCESS_SUB, "no"]);
                        let _ = socket.send(frame(reply)).await;
                    }
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn the_access_read_comes_before_messages_on_every_connection() {
        for answer in ["EOSE", "CLOSED"] {
            let (url, mut reqs) = access_relay(answer).await;
            let keys = Keys::generate();
            let client = KuzzClient::new(url, keys.clone(), None, None).unwrap();
            let mut watch = Watch {
                me: keys.public_key().to_hex(),
                cursor: now_secs(),
                channels: BTreeSet::new(),
                seen: HashSet::new(),
                seen_order: VecDeque::new(),
                names: HashMap::new(),
                access: Some(access_watch(&Keys::generate())),
            };
            let run = run_connection(&client, &mut watch, PRESENCE_EVERY);
            tokio::pin!(run);
            let mut order = Vec::new();
            while order.len() < 3 {
                tokio::select! {
                    _ = &mut run => panic!("the connection ended"),
                    req = tokio::time::timeout(Duration::from_secs(4), reqs.recv()) => {
                        order.push(req.expect("the watch goes on after {answer}").unwrap());
                    }
                }
            }
            assert_eq!(order, [ACCESS_SUB, MEMBERS_SUB, MENTIONS_SUB], "{answer}");
        }
    }

    #[test]
    fn seen_ids_are_bounded_and_deduplicated() {
        let mut watch = Watch {
            me: ALICE.into(),
            cursor: 100,
            channels: BTreeSet::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            names: HashMap::new(),
            access: None,
        };
        assert!(watch.first_sighting("a".into()));
        assert!(!watch.first_sighting("a".into()));
        for i in 0..SEEN_CAP {
            watch.first_sighting(format!("x{i}"));
        }
        assert_eq!(watch.seen.len(), SEEN_CAP);
        assert!(watch.first_sighting("a".into()), "evicted ids may repeat");
        assert_eq!(watch.since(), 100);
    }
}
