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
//! the newest message printed (minus [`SINCE_SKEW_SECS`], deduplicated by
//! id). An idle connection is probed every [`IDLE_PING`]. Status goes to
//! stderr; stdout holds only messages.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Write as _;
use std::time::{Duration, Instant};

use kuzz_core::kind::{
    KIND_MEMBER_ADDED_NOTIFICATION, KIND_STREAM_MESSAGE, KIND_STREAM_MESSAGE_DIFF,
    KIND_STREAM_MESSAGE_V2,
};
use kuzz_ws_client::{NostrWsConnection, RelayMessage, WsClientError};
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

/// The word a supervisor parks on (`signed_out`, `revoked`, `not_paired`)
/// for a refused AUTH that retrying cannot fix: the relay's
/// `restricted: signed_out` / `revoked`, and refusals of this key from this
/// computer (`device`, `unbound`, `owner`). `None`: worth retrying.
pub fn parked_reason(refusal: &str) -> Option<&'static str> {
    let reason = refusal
        .trim()
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

/// What survives reconnects.
struct Watch {
    me: String,
    /// Newest `created_at` printed; resubscriptions start here (minus skew).
    cursor: i64,
    channels: BTreeSet<String>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    names: HashMap<String, Option<String>>,
}

impl Watch {
    fn since(&self) -> i64 {
        self.cursor.saturating_sub(SINCE_SKEW_SECS).max(0)
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

async fn subscribe_channels(conn: &mut NostrWsConnection, watch: &Watch) -> Result<(), End> {
    for req in channel_reqs(&watch.channels, watch.since()) {
        send(conn, &req).await?;
    }
    Ok(())
}

/// Print one line and flush; a closed stdout ends the watch.
fn emit(line: &Value) -> Result<(), End> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")
        .and_then(|()| out.flush())
        .map_err(|e| End::Fatal(CliError::Other(format!("stdout closed: {e}"))))
}

/// One connection: subscribe, then print messages until it is lost.
async fn run_connection(client: &KuzzClient, watch: &mut Watch) -> End {
    let mut conn = match tokio::time::timeout(CONNECT_TIMEOUT, client.ws_connect()).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(error)) => return ws_lost(error),
        Err(_) => return End::Lost("connect timed out".into()),
    };
    let result = async {
        send(&mut conn, &members_req(&watch.me)).await?;
        send(&mut conn, &mentions_req(&watch.me, watch.since())).await?;
        let mut members_ready = false;
        let mut last_frame = Instant::now();
        let mut members_at = Instant::now();
        let mut ping_sent: Option<Instant> = None;
        loop {
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
                    if members_ready && members_at.elapsed() > MEMBERS_REFRESH {
                        send(&mut conn, &members_req(&watch.me)).await?;
                        members_at = Instant::now();
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
                    subscription_id, ..
                } if subscription_id == PING_SUB => {}
                RelayMessage::Event { event, .. } => {
                    if !MESSAGE_KINDS.contains(&u32::from(event.kind.as_u16()))
                        || !watch.first_sighting(event.id.to_hex())
                    {
                        continue;
                    }
                    let created_at = i64::try_from(event.created_at.as_secs()).unwrap_or(0);
                    watch.cursor = watch.cursor.max(created_at);
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
                    if subscription_id == MEMBERS_SUB && !members_ready =>
                {
                    members_ready = true;
                    members_at = Instant::now();
                    subscribe_channels(&mut conn, watch).await?;
                    eprintln!(
                        "kuzz watch: connected; watching {} channel(s) and mentions",
                        watch.channels.len()
                    );
                }
                RelayMessage::Eose { subscription_id } if subscription_id == PING_SUB => {
                    ping_sent = None;
                    send(&mut conn, &json!(["CLOSE", PING_SUB])).await?;
                }
                RelayMessage::Closed {
                    subscription_id,
                    message,
                } => {
                    eprintln!("kuzz watch: relay closed {subscription_id}: {message}");
                    if subscription_id == PING_SUB {
                        ping_sent = None;
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

/// `kuzz messages watch`.
pub async fn cmd_watch(client: &KuzzClient, since: Option<i64>) -> Result<(), CliError> {
    let mut watch = Watch {
        me: client.pubkey().to_hex(),
        cursor: since
            .unwrap_or_else(now_secs)
            .saturating_add(SINCE_SKEW_SECS),
        channels: BTreeSet::new(),
        seen: HashSet::new(),
        seen_order: VecDeque::new(),
        names: HashMap::new(),
    };
    let mut attempt: u32 = 0;
    let mut refusals: u32 = 0;
    loop {
        let started = Instant::now();
        let reason = match run_connection(client, &mut watch).await {
            End::Fatal(error) => return Err(error),
            End::Refused(message) => {
                if let Some(word) = parked_reason(&message) {
                    return Err(CliError::Auth(format!(
                        "{word}: relay rejected WebSocket auth: {message}"
                    )));
                }
                refusals += 1;
                if refusals >= MAX_AUTH_REFUSALS {
                    return Err(CliError::Auth(format!(
                        "relay rejected WebSocket auth: {message}"
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
    use nostr::{EventBuilder, Keys, Kind, Tag};

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

    #[test]
    fn seen_ids_are_bounded_and_deduplicated() {
        let mut watch = Watch {
            me: ALICE.into(),
            cursor: 100,
            channels: BTreeSet::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            names: HashMap::new(),
        };
        assert!(watch.first_sighting("a".into()));
        assert!(!watch.first_sighting("a".into()));
        for i in 0..SEEN_CAP {
            watch.first_sighting(format!("x{i}"));
        }
        assert_eq!(watch.seen.len(), SEEN_CAP);
        assert!(watch.first_sighting("a".into()), "evicted ids may repeat");
        assert_eq!(watch.since(), 100 - SINCE_SKEW_SECS);
    }
}
