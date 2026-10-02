//! Channel dispatcher mode: the human-only author gate and the channel roster
//! section a dispatcher needs to route requests.
//!
//! A dispatcher listens to every non-DM channel it belongs to without being
//! mentioned, so it must not inherit the normal author policy: the
//! `owner-only` mode admits sibling agents, which would let one agent's
//! callback wake the dispatcher and loop. The gate here admits a stream
//! message only when a configured human (or, by default, a channel
//! owner/admin) wrote it, the author's kind:0 profile does not carry a valid
//! NIP-OA attestation, and the message does not already address another
//! agent. Explicitly listed AI pubkeys bypass the human check.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use buzz_core::kind::KIND_STREAM_MESSAGE;
use tokio::time::timeout;
use uuid::Uuid;

use crate::config::DispatcherConfig;
use crate::relay::RestClient;

/// Owner/admin snapshots change when members are promoted; keep them short.
const ADMINS_CACHE_TTL: Duration = Duration::from_secs(60);
/// NIP-OA attestations are immutable, but a human can later be re-keyed as an
/// agent profile, so profile verdicts expire too.
const PROFILE_CACHE_TTL: Duration = Duration::from_secs(300);
/// Bound for each relay lookup the gate performs on the listener path.
const LOOKUP_TIMEOUT: Duration = Duration::from_millis(2000);
/// Cap on cached entries per table to prevent unbounded growth.
const CACHE_CAP: usize = 256;
/// Longest `about` excerpt rendered per roster member.
const ROSTER_ABOUT_MAX_CHARS: usize = 200;
/// Most `p` mentions a request can carry before the gate stops looking them
/// up and drops the event instead of issuing a large profile query inline.
const MAX_MENTION_LOOKUPS: usize = 16;

/// What a pubkey's kind-0 profile says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProfileKind {
    /// A profile exists and carries no verifiable NIP-OA `auth` tag.
    Human,
    /// A profile exists and its NIP-OA `auth` tag verifies for the pubkey.
    Agent,
    /// The relay has no kind-0 event for the pubkey. Every Buzz human has a
    /// profile, so this is more likely a script key than a person.
    Absent,
}

/// Relay lookups the dispatcher gate depends on. `None` means the lookup
/// failed (timeout, transport, malformed response) and the gate fails closed;
/// an empty set or map is an authoritative negative answer.
pub(crate) trait TrustLookup {
    /// Owner and admin pubkeys from the channel's kind-39001 admins event.
    async fn channel_admins(&self, channel_id: Uuid) -> Option<HashSet<String>>;
    /// Pubkeys holding the `bot` role in the channel's kind-39002 members event.
    async fn channel_bots(&self, channel_id: Uuid) -> Option<HashSet<String>>;
    /// Profile verdicts for a batch of pubkeys in one query. Pubkeys without
    /// a kind-0 event are simply absent from the map.
    async fn profiles(&self, pubkeys: &[String]) -> Option<HashMap<String, ProfileKind>>;
}

/// Why the dispatcher gate dropped an event. Allowed events carry no verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DispatcherVerdict {
    Allow,
    /// The dispatcher's own message.
    SelfAuthored,
    /// Not a plainly human-authored kind-9 stream message (edits, approvals,
    /// reminders, relay-signed or delegated workflow traffic).
    NotHumanStreamMessage,
    /// Author is neither a configured human nor a channel owner/admin (nor
    /// the agent's own owner), or has no kind-0 profile at all.
    NotTrusted,
    /// Author holds the channel `bot` role or its profile proves it is an
    /// owned agent, and it is not allowlisted.
    AuthorIsAgent,
    /// The message already `p`-mentions another agent, so it has an assignee.
    AlreadyAssigned,
    /// A relay lookup the decision depends on failed; fail closed.
    LookupFailed,
}

struct Cached<T> {
    fetched_at: Instant,
    value: T,
}

/// Dispatcher author policy plus short-lived caches for the lookups it needs.
pub(crate) struct DispatcherGate {
    config: DispatcherConfig,
    /// The agent's own owner (lowercase hex). Implicitly trusted in channels
    /// without an explicit `humans` list: the person running the dispatcher
    /// must never be dropped as a stranger in their own channel.
    agent_owner: Option<String>,
    admins: Mutex<HashMap<Uuid, Cached<HashSet<String>>>>,
    bots: Mutex<HashMap<Uuid, Cached<HashSet<String>>>>,
    profiles: Mutex<HashMap<String, Cached<ProfileKind>>>,
}

impl DispatcherGate {
    pub(crate) fn new(config: DispatcherConfig, agent_owner: Option<String>) -> Self {
        Self {
            config,
            agent_owner: agent_owner.map(|owner| owner.to_ascii_lowercase()),
            admins: Mutex::new(HashMap::new()),
            bots: Mutex::new(HashMap::new()),
            profiles: Mutex::new(HashMap::new()),
        }
    }

    /// Apply the dispatcher gate to one non-DM channel event using the relay.
    pub(crate) async fn allows(
        &self,
        event: &nostr::Event,
        effective_author: &str,
        channel_id: Uuid,
        relay_self: Option<&str>,
        agent_pubkey_hex: &str,
        rest_client: &RestClient,
    ) -> bool {
        let lookup = RelayLookup { rest: rest_client };
        let verdict = self
            .evaluate(
                &lookup,
                event,
                effective_author,
                channel_id,
                relay_self,
                agent_pubkey_hex,
            )
            .await;
        if verdict != DispatcherVerdict::Allow {
            tracing::debug!(
                channel_id = %channel_id,
                author = %effective_author,
                ?verdict,
                "dispatcher gate — dropping event"
            );
        }
        verdict == DispatcherVerdict::Allow
    }

    /// Decide whether a non-DM channel event should reach the dispatcher.
    pub(crate) async fn evaluate<L: TrustLookup>(
        &self,
        lookup: &L,
        event: &nostr::Event,
        effective_author: &str,
        channel_id: Uuid,
        relay_self: Option<&str>,
        agent_pubkey_hex: &str,
    ) -> DispatcherVerdict {
        let signer = event.pubkey.to_hex();
        let author = effective_author.to_ascii_lowercase();
        if author.eq_ignore_ascii_case(agent_pubkey_hex)
            || signer.eq_ignore_ascii_case(agent_pubkey_hex)
        {
            return DispatcherVerdict::SelfAuthored;
        }
        if !is_plain_stream_message(event, &author, relay_self) {
            return DispatcherVerdict::NotHumanStreamMessage;
        }

        // The channel's `bot` role is the relay's own statement that a member
        // is an agent; it needs no desktop-issued attestation, so it closes
        // the loop for manually run or imported agent identities.
        let Some(bots) = self
            .cached_channel_set(&self.bots, lookup, channel_id, true)
            .await
        else {
            return DispatcherVerdict::LookupFailed;
        };

        let policy = self.config.channel(channel_id);
        let allowlisted_ai = policy.is_some_and(|p| p.ais.contains(&author));
        if !allowlisted_ai {
            let trusted = match policy.filter(|p| !p.humans.is_empty()) {
                Some(p) => p.humans.contains(&author),
                None => {
                    self.agent_owner.as_deref() == Some(author.as_str())
                        || match self
                            .cached_channel_set(&self.admins, lookup, channel_id, false)
                            .await
                        {
                            Some(admins) => admins.contains(&author),
                            None => return DispatcherVerdict::LookupFailed,
                        }
                }
            };
            if !trusted {
                return DispatcherVerdict::NotTrusted;
            }
            if bots.contains(&author) {
                return DispatcherVerdict::AuthorIsAgent;
            }
            let Some(verdicts) = self
                .profiles_for(lookup, std::slice::from_ref(&author))
                .await
            else {
                return DispatcherVerdict::LookupFailed;
            };
            match verdicts
                .get(&author)
                .copied()
                .unwrap_or(ProfileKind::Absent)
            {
                ProfileKind::Agent => return DispatcherVerdict::AuthorIsAgent,
                ProfileKind::Absent => return DispatcherVerdict::NotTrusted,
                ProfileKind::Human => {}
            }
        }

        let mentions: Vec<String> = mentioned_pubkeys(event)
            .into_iter()
            .filter(|m| !m.eq_ignore_ascii_case(agent_pubkey_hex) && *m != author)
            .collect();
        if mentions.iter().any(|m| bots.contains(m)) {
            return DispatcherVerdict::AlreadyAssigned;
        }
        if mentions.len() > MAX_MENTION_LOOKUPS {
            tracing::warn!(
                channel_id = %channel_id,
                mentions = mentions.len(),
                cap = MAX_MENTION_LOOKUPS,
                "dispatcher gate — too many mentions to classify; dropping event"
            );
            return DispatcherVerdict::LookupFailed;
        }
        if !mentions.is_empty() {
            let Some(verdicts) = self.profiles_for(lookup, &mentions).await else {
                return DispatcherVerdict::LookupFailed;
            };
            if verdicts.values().any(|kind| *kind == ProfileKind::Agent) {
                return DispatcherVerdict::AlreadyAssigned;
            }
        }
        DispatcherVerdict::Allow
    }

    /// Per-channel pubkey set (admins from 39001 or bots from 39002) with a
    /// short TTL. `bots` selects which lookup feeds the given cache.
    async fn cached_channel_set<L: TrustLookup>(
        &self,
        cache: &Mutex<HashMap<Uuid, Cached<HashSet<String>>>>,
        lookup: &L,
        channel_id: Uuid,
        bots: bool,
    ) -> Option<HashSet<String>> {
        if let Some(cached) = cache.lock().ok().and_then(|cache| {
            cache
                .get(&channel_id)
                .filter(|c| c.fetched_at.elapsed() < ADMINS_CACHE_TTL)
                .map(|c| c.value.clone())
        }) {
            return Some(cached);
        }
        let fetched = if bots {
            lookup.channel_bots(channel_id).await?
        } else {
            lookup.channel_admins(channel_id).await?
        };
        if let Ok(mut cache) = cache.lock() {
            if cache.len() >= CACHE_CAP {
                cache.clear();
            }
            cache.insert(
                channel_id,
                Cached {
                    fetched_at: Instant::now(),
                    value: fetched.clone(),
                },
            );
        }
        Some(fetched)
    }

    /// Profile verdicts for `pubkeys`, serving fresh cache entries and fetching
    /// the rest in one batched query. Every requested pubkey is present in the
    /// result (`Absent` when the relay has no profile).
    async fn profiles_for<L: TrustLookup>(
        &self,
        lookup: &L,
        pubkeys: &[String],
    ) -> Option<HashMap<String, ProfileKind>> {
        let mut verdicts = HashMap::with_capacity(pubkeys.len());
        let mut uncached = Vec::new();
        {
            let cache = self.profiles.lock().ok()?;
            for pubkey in pubkeys {
                match cache
                    .get(pubkey)
                    .filter(|c| c.fetched_at.elapsed() < PROFILE_CACHE_TTL)
                {
                    Some(cached) => {
                        verdicts.insert(pubkey.clone(), cached.value);
                    }
                    None => uncached.push(pubkey.clone()),
                }
            }
        }
        if uncached.is_empty() {
            return Some(verdicts);
        }
        let fetched = lookup.profiles(&uncached).await?;
        let now = Instant::now();
        let mut cache = self.profiles.lock().ok()?;
        if cache.len() + uncached.len() > CACHE_CAP {
            cache.clear();
        }
        for pubkey in uncached {
            let kind = fetched.get(&pubkey).copied().unwrap_or(ProfileKind::Absent);
            cache.insert(
                pubkey.clone(),
                Cached {
                    fetched_at: now,
                    value: kind,
                },
            );
            verdicts.insert(pubkey, kind);
        }
        Some(verdicts)
    }
}

/// Whether a `p` tag value is a 64-character hex pubkey. Other encodings
/// (npub, truncated keys from third-party clients) are ignored rather than
/// treated as lookup failures.
fn is_hex_pubkey(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

/// A kind-9 message signed by its own author: not an edit, approval or
/// reminder, not relay-signed, and not attributed through workflow delegation.
fn is_plain_stream_message(event: &nostr::Event, author: &str, relay_self: Option<&str>) -> bool {
    if event.kind.as_u16() as u32 != KIND_STREAM_MESSAGE {
        return false;
    }
    let signer = event.pubkey.to_hex();
    if !signer.eq_ignore_ascii_case(author) {
        return false;
    }
    if relay_self.is_some_and(|relay| relay.eq_ignore_ascii_case(&signer)) {
        return false;
    }
    !event
        .tags
        .iter()
        .any(|tag| tag.as_slice().first().map(String::as_str) == Some("buzz:workflow"))
}

/// Lowercase 64-hex pubkeys from the event's `p` tags, in order, deduplicated.
/// Values that are not hex pubkeys are skipped.
fn mentioned_pubkeys(event: &nostr::Event) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tag in event.tags.iter() {
        let values = tag.as_slice();
        if values.first().map(String::as_str) != Some("p") {
            continue;
        }
        let Some(pubkey) = values.get(1).filter(|value| is_hex_pubkey(value)) else {
            continue;
        };
        let pubkey = pubkey.to_ascii_lowercase();
        if seen.insert(pubkey.clone()) {
            out.push(pubkey);
        }
    }
    out
}

/// Classify every kind-0 event in a `/query` response by its author: `Agent`
/// when a NIP-OA `auth` tag cryptographically verifies for that author,
/// otherwise `Human`. Unlike the shape-only heuristic used for reply
/// anchoring, this is a trust decision, so the signature is checked. Events
/// without a parseable author are ignored.
pub(crate) fn parse_profile_kinds(json: &serde_json::Value) -> HashMap<String, ProfileKind> {
    let mut kinds = HashMap::new();
    let Some(events) = json.as_array() else {
        return kinds;
    };
    for ev in events {
        let Some(pubkey_hex) = ev.get("pubkey").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(pk) = nostr::PublicKey::from_hex(pubkey_hex) else {
            continue;
        };
        let proves_agent = ev
            .get("tags")
            .and_then(|t| t.as_array())
            .into_iter()
            .flatten()
            .filter(|tag| {
                tag.as_array()
                    .is_some_and(|parts| parts.len() >= 4 && parts[0].as_str() == Some("auth"))
            })
            .any(|tag| {
                serde_json::to_string(tag).ok().is_some_and(|tag_json| {
                    buzz_sdk::nip_oa::verify_auth_tag(&tag_json, &pk).is_ok()
                })
            });
        let kind = if proves_agent {
            ProfileKind::Agent
        } else {
            ProfileKind::Human
        };
        let entry = kinds.entry(pubkey_hex.to_ascii_lowercase()).or_insert(kind);
        // Any verifying profile event for the author marks it as an agent.
        if kind == ProfileKind::Agent {
            *entry = ProfileKind::Agent;
        }
    }
    kinds
}

struct RelayLookup<'a> {
    rest: &'a RestClient,
}

impl TrustLookup for RelayLookup<'_> {
    async fn channel_admins(&self, channel_id: Uuid) -> Option<HashSet<String>> {
        match timeout(LOOKUP_TIMEOUT, self.rest.fetch_channel_admins(channel_id)).await {
            Ok(Ok(roles)) => Some(
                roles
                    .into_iter()
                    .filter(|(_, role)| role == "owner" || role == "admin")
                    .map(|(pubkey, _)| pubkey)
                    .collect(),
            ),
            Ok(Err(e)) => {
                tracing::debug!(channel_id = %channel_id, "dispatcher admins lookup failed: {e}");
                None
            }
            Err(_) => {
                tracing::debug!(channel_id = %channel_id, "dispatcher admins lookup timed out");
                None
            }
        }
    }

    async fn channel_bots(&self, channel_id: Uuid) -> Option<HashSet<String>> {
        match timeout(LOOKUP_TIMEOUT, self.rest.fetch_channel_members(channel_id)).await {
            Ok(Ok(members)) => Some(
                members
                    .into_iter()
                    .filter(|member| member.role == "bot")
                    .map(|member| member.pubkey)
                    .collect(),
            ),
            Ok(Err(e)) => {
                tracing::debug!(channel_id = %channel_id, "dispatcher members lookup failed: {e}");
                None
            }
            Err(_) => {
                tracing::debug!(channel_id = %channel_id, "dispatcher members lookup timed out");
                None
            }
        }
    }

    async fn profiles(&self, pubkeys: &[String]) -> Option<HashMap<String, ProfileKind>> {
        let authors: Vec<nostr::PublicKey> = pubkeys
            .iter()
            .filter_map(|pk| nostr::PublicKey::from_hex(pk).ok())
            .collect();
        if authors.is_empty() {
            return Some(HashMap::new());
        }
        let filter = nostr::Filter::new()
            .kind(nostr::Kind::Metadata)
            .authors(authors);
        match timeout(
            LOOKUP_TIMEOUT,
            self.rest.query(std::slice::from_ref(&filter)),
        )
        .await
        {
            Ok(Ok(json)) => Some(parse_profile_kinds(&json)),
            Ok(Err(e)) => {
                tracing::debug!(
                    count = pubkeys.len(),
                    "dispatcher profile lookup failed: {e}"
                );
                None
            }
            Err(_) => {
                tracing::debug!(count = pubkeys.len(), "dispatcher profile lookup timed out");
                None
            }
        }
    }
}

// ── channel roster ──────────────────────────────────────────────────────────

/// One channel member as rendered in the `<channel-roster>` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RosterMember {
    pub pubkey: String,
    /// `owner` / `admin` from kind 39001, otherwise the kind-39002 role
    /// (`bot`, `member`).
    pub role: String,
    pub name: Option<String>,
    pub about: Option<String>,
    /// Channel `bot` role, or the shape-only NIP-OA hint from the kind-0
    /// profile (same heuristic as the prompt profile lookup) — a routing aid,
    /// not a trust decision.
    pub is_agent: bool,
}

#[derive(Default)]
struct RosterProfile {
    name: Option<String>,
    about: Option<String>,
    is_agent: bool,
}

fn parse_roster_profiles(json: &serde_json::Value) -> HashMap<String, RosterProfile> {
    let mut profiles = HashMap::new();
    let Some(events) = json.as_array() else {
        return profiles;
    };
    for ev in events {
        let (Some(pubkey), Some(content)) = (
            ev.get("pubkey").and_then(|v| v.as_str()),
            ev.get("content").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let Ok(profile) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        let text = |key: &str| {
            profile
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        profiles.insert(
            pubkey.to_ascii_lowercase(),
            RosterProfile {
                name: text("display_name").or_else(|| text("name")),
                about: text("about"),
                is_agent: crate::pool::profile_event_is_agent(ev),
            },
        );
    }
    profiles
}

/// Fetch the channel's members (39002), roles (39001) and profiles (kind 0)
/// and render them as the `<channel-roster>` body. `None` when the member
/// list is unavailable; a missing admins snapshot or profile set degrades to
/// `member` roles and bare pubkeys rather than failing the turn.
pub(crate) async fn fetch_channel_roster_section(
    channel_id: Uuid,
    agent_pubkey_hex: &str,
    rest: &RestClient,
) -> Option<String> {
    let members = match timeout(LOOKUP_TIMEOUT, rest.fetch_channel_members(channel_id)).await {
        Ok(Ok(members)) => members,
        Ok(Err(e)) => {
            tracing::debug!(channel_id = %channel_id, "roster members lookup failed: {e}");
            return None;
        }
        Err(_) => {
            tracing::debug!(channel_id = %channel_id, "roster members lookup timed out");
            return None;
        }
    };
    if members.is_empty() {
        return None;
    }
    let roles = match timeout(LOOKUP_TIMEOUT, rest.fetch_channel_admins(channel_id)).await {
        Ok(Ok(roles)) => roles,
        _ => {
            tracing::debug!(channel_id = %channel_id, "roster admins lookup unavailable — rendering without roles");
            HashMap::new()
        }
    };
    let authors: Vec<nostr::PublicKey> = members
        .iter()
        .filter_map(|member| nostr::PublicKey::from_hex(&member.pubkey).ok())
        .collect();
    let profiles = if authors.is_empty() {
        HashMap::new()
    } else {
        let filter = nostr::Filter::new()
            .kind(nostr::Kind::Metadata)
            .authors(authors);
        match timeout(LOOKUP_TIMEOUT, rest.query(std::slice::from_ref(&filter))).await {
            Ok(Ok(json)) => parse_roster_profiles(&json),
            _ => {
                tracing::debug!(channel_id = %channel_id, "roster profile lookup unavailable — rendering without names");
                HashMap::new()
            }
        }
    };
    let roster: Vec<RosterMember> = members
        .iter()
        .map(|member| {
            let pubkey = member.pubkey.to_ascii_lowercase();
            let profile = profiles.get(&pubkey);
            RosterMember {
                role: roles
                    .get(&pubkey)
                    .cloned()
                    .unwrap_or_else(|| member.role.clone()),
                name: profile.and_then(|p| p.name.clone()),
                about: profile.and_then(|p| p.about.clone()),
                is_agent: member.role == "bot" || profile.is_some_and(|p| p.is_agent),
                pubkey,
            }
        })
        .collect();
    Some(render_channel_roster(&roster, agent_pubkey_hex))
}

fn role_rank(role: &str) -> u8 {
    match role {
        "owner" => 0,
        "admin" => 1,
        _ => 2,
    }
}

/// Render the roster body: owners and admins first, then by name. Names and
/// descriptions are untrusted member-supplied text, so they are escaped.
pub(crate) fn render_channel_roster(members: &[RosterMember], agent_pubkey_hex: &str) -> String {
    let mut sorted: Vec<&RosterMember> = members.iter().collect();
    sorted.sort_by_key(|m| {
        (
            role_rank(&m.role),
            m.name.as_deref().unwrap_or("").to_ascii_lowercase(),
            m.pubkey.clone(),
        )
    });
    let mut body = format!(
        "Members of this channel ({}). Address an agent with its exact name and pass its pubkey with `--mention`.",
        members.len()
    );
    for member in sorted {
        let name = member
            .name
            .as_deref()
            .map(collapse_whitespace)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "(unnamed)".to_string());
        let kind = if member.is_agent { "agent" } else { "human" };
        let you = if member.pubkey.eq_ignore_ascii_case(agent_pubkey_hex) {
            ", you"
        } else {
            ""
        };
        body.push_str(&format!(
            "\n- {} ({}, {kind}{you}) pubkey={}",
            crate::prompt_framing::escape_semantic_text(&name),
            crate::prompt_framing::escape_semantic_text(&member.role),
            member.pubkey
        ));
        if let Some(about) = member.about.as_deref().map(collapse_whitespace) {
            if !about.is_empty() {
                let about: String = about.chars().take(ROSTER_ABOUT_MAX_CHARS).collect();
                body.push_str(" — ");
                body.push_str(&crate::prompt_framing::escape_semantic_text(&about));
            }
        }
    }
    body
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DispatcherChannelPolicy, DispatcherConfig};
    use nostr::{EventBuilder, Keys, Kind, Tag};

    /// Scripted lookup: admins and bots per channel, profile kinds per pubkey.
    /// Pubkeys in `agents` have agent profiles, pubkeys in `absent` have no
    /// profile, everyone else is a human. `fail` makes every lookup fail;
    /// `fail_profiles_containing` fails only profile batches that include
    /// that pubkey. `profile_batches` records each profile query.
    #[derive(Default)]
    struct StaticLookup {
        admins: HashMap<Uuid, HashSet<String>>,
        bots: HashMap<Uuid, HashSet<String>>,
        agents: HashSet<String>,
        absent: HashSet<String>,
        fail: bool,
        fail_profiles_containing: Option<String>,
        profile_batches: Mutex<Vec<Vec<String>>>,
    }

    impl StaticLookup {
        fn profile_batches(&self) -> Vec<Vec<String>> {
            self.profile_batches.lock().expect("batches").clone()
        }
    }

    impl TrustLookup for StaticLookup {
        async fn channel_admins(&self, channel_id: Uuid) -> Option<HashSet<String>> {
            if self.fail {
                return None;
            }
            Some(self.admins.get(&channel_id).cloned().unwrap_or_default())
        }

        async fn channel_bots(&self, channel_id: Uuid) -> Option<HashSet<String>> {
            if self.fail {
                return None;
            }
            Some(self.bots.get(&channel_id).cloned().unwrap_or_default())
        }

        async fn profiles(&self, pubkeys: &[String]) -> Option<HashMap<String, ProfileKind>> {
            if let Ok(mut batches) = self.profile_batches.lock() {
                batches.push(pubkeys.to_vec());
            }
            if self.fail
                || self
                    .fail_profiles_containing
                    .as_ref()
                    .is_some_and(|pk| pubkeys.contains(pk))
            {
                return None;
            }
            Some(
                pubkeys
                    .iter()
                    .filter(|pk| !self.absent.contains(*pk))
                    .map(|pk| {
                        let kind = if self.agents.contains(pk) {
                            ProfileKind::Agent
                        } else {
                            ProfileKind::Human
                        };
                        (pk.clone(), kind)
                    })
                    .collect(),
            )
        }
    }

    fn message(signer: &Keys, kind: u32, p_tags: &[&str]) -> nostr::Event {
        let tags: Vec<Tag> = p_tags
            .iter()
            .map(|pk| Tag::parse(["p", pk]).expect("p tag"))
            .collect();
        EventBuilder::new(Kind::Custom(kind as u16), "please build the thing")
            .tags(tags)
            .sign_with_keys(signer)
            .expect("signed event")
    }

    fn policy(humans: &[&str], ais: &[&str]) -> DispatcherChannelPolicy {
        DispatcherChannelPolicy {
            humans: humans.iter().map(|s| s.to_string()).collect(),
            ais: ais.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_gate(config: DispatcherConfig) -> DispatcherGate {
        DispatcherGate::new(config, None)
    }

    fn channel_config(channel: Uuid, policy: DispatcherChannelPolicy) -> DispatcherConfig {
        DispatcherConfig::from_channels(HashMap::from([(channel, policy)]))
    }

    struct Fixture {
        channel: Uuid,
        agent: String,
        human: Keys,
        human_hex: String,
        other_agent_hex: String,
    }

    fn fixture() -> Fixture {
        let human = Keys::generate();
        Fixture {
            channel: Uuid::new_v4(),
            agent: Keys::generate().public_key().to_hex(),
            human_hex: human.public_key().to_hex(),
            human,
            other_agent_hex: Keys::generate().public_key().to_hex(),
        }
    }

    impl Fixture {
        /// A lookup where the fixture human owns the channel.
        fn owner_lookup(&self) -> StaticLookup {
            StaticLookup {
                admins: HashMap::from([(self.channel, HashSet::from([self.human_hex.clone()]))]),
                ..Default::default()
            }
        }
    }

    async fn verdict(
        gate: &DispatcherGate,
        lookup: &StaticLookup,
        fx: &Fixture,
        event: &nostr::Event,
    ) -> DispatcherVerdict {
        let author = event.pubkey.to_hex();
        gate.evaluate(lookup, event, &author, fx.channel, None, &fx.agent)
            .await
    }

    #[tokio::test]
    async fn humans_allowlist_admits_listed_human_and_rejects_others() {
        let fx = fixture();
        let stranger = Keys::generate();
        let gate = make_gate(channel_config(fx.channel, policy(&[&fx.human_hex], &[])));
        // Stranger is even a channel owner: an explicit humans list replaces
        // the owner/admin default rather than extending it.
        let lookup = StaticLookup {
            admins: HashMap::from([(fx.channel, HashSet::from([stranger.public_key().to_hex()]))]),
            ..Default::default()
        };
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &[])
            )
            .await,
            DispatcherVerdict::Allow
        );
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&stranger, KIND_STREAM_MESSAGE, &[])
            )
            .await,
            DispatcherVerdict::NotTrusted
        );
    }

    #[tokio::test]
    async fn empty_humans_falls_back_to_channel_owner_or_admin() {
        let fx = fixture();
        let stranger = Keys::generate();
        for config in [
            DispatcherConfig::default(),
            channel_config(fx.channel, policy(&[], &[])),
        ] {
            let gate = make_gate(config);
            let lookup = fx.owner_lookup();
            assert_eq!(
                verdict(
                    &gate,
                    &lookup,
                    &fx,
                    &message(&fx.human, KIND_STREAM_MESSAGE, &[])
                )
                .await,
                DispatcherVerdict::Allow,
                "channel owner must be admitted by default"
            );
            assert_eq!(
                verdict(
                    &gate,
                    &lookup,
                    &fx,
                    &message(&stranger, KIND_STREAM_MESSAGE, &[])
                )
                .await,
                DispatcherVerdict::NotTrusted,
                "a plain member must not be admitted by default"
            );
        }
    }

    /// The person running the dispatcher is never a stranger in a channel
    /// without an explicit `humans` list, even when they hold no channel role.
    #[tokio::test]
    async fn agent_owner_is_trusted_by_default_but_not_against_an_explicit_humans_list() {
        let fx = fixture();
        let owner = Keys::generate();
        let owner_hex = owner.public_key().to_hex();
        let event = message(&owner, KIND_STREAM_MESSAGE, &[]);
        // No admins event at all for the channel; the owner is a plain member.
        let lookup = StaticLookup::default();

        let gate = DispatcherGate::new(DispatcherConfig::default(), Some(owner_hex.to_uppercase()));
        assert_eq!(
            verdict(&gate, &lookup, &fx, &event).await,
            DispatcherVerdict::Allow,
            "the agent owner is admitted without any channel role (case-insensitively)"
        );

        let gate = DispatcherGate::new(
            channel_config(fx.channel, policy(&[&fx.human_hex], &[])),
            Some(owner_hex.clone()),
        );
        assert_eq!(
            verdict(&gate, &lookup, &fx, &event).await,
            DispatcherVerdict::NotTrusted,
            "an explicit humans list is authoritative, even against the agent owner"
        );

        // Owner trust is still subject to the agent check: an owner key that
        // is a bot in the channel must not wake the dispatcher.
        let gate = DispatcherGate::new(DispatcherConfig::default(), Some(owner_hex.clone()));
        let lookup = StaticLookup {
            bots: HashMap::from([(fx.channel, HashSet::from([owner_hex.clone()]))]),
            ..Default::default()
        };
        assert_eq!(
            verdict(&gate, &lookup, &fx, &event).await,
            DispatcherVerdict::AuthorIsAgent
        );
    }

    #[tokio::test]
    async fn owner_with_agent_profile_is_rejected_unless_allowlisted_as_ai() {
        let fx = fixture();
        let bot = Keys::generate();
        let bot_hex = bot.public_key().to_hex();
        let lookup = StaticLookup {
            admins: HashMap::from([(fx.channel, HashSet::from([bot_hex.clone()]))]),
            agents: HashSet::from([bot_hex.clone()]),
            ..Default::default()
        };
        // Owner-role sibling bot: the owner+siblings rule is not inherited.
        let gate = make_gate(DispatcherConfig::default());
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&bot, KIND_STREAM_MESSAGE, &[])
            )
            .await,
            DispatcherVerdict::AuthorIsAgent
        );
        // Listed in the humans list but still an agent profile: rejected.
        let gate = make_gate(channel_config(fx.channel, policy(&[&bot_hex], &[])));
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&bot, KIND_STREAM_MESSAGE, &[])
            )
            .await,
            DispatcherVerdict::AuthorIsAgent
        );
        // Explicit ais allowlist passes the agent through without the human check.
        let gate = make_gate(channel_config(fx.channel, policy(&[], &[&bot_hex])));
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&bot, KIND_STREAM_MESSAGE, &[])
            )
            .await,
            DispatcherVerdict::Allow
        );
    }

    /// Loop safety for agents without a desktop-issued attestation (manual
    /// `buzz-acp`, imported keys): the channel's `bot` role alone marks the
    /// author as an agent, even when it owns the channel or is listed as a
    /// human, and no profile lookup is needed to decide.
    #[tokio::test]
    async fn channel_bot_role_marks_author_as_agent_without_attestation() {
        let fx = fixture();
        let bot = Keys::generate();
        let bot_hex = bot.public_key().to_hex();
        let event = message(&bot, KIND_STREAM_MESSAGE, &[]);
        let lookup = StaticLookup {
            admins: HashMap::from([(fx.channel, HashSet::from([bot_hex.clone()]))]),
            bots: HashMap::from([(fx.channel, HashSet::from([bot_hex.clone()]))]),
            // Its kind-0 profile looks perfectly human.
            ..Default::default()
        };
        let gate = make_gate(DispatcherConfig::default());
        assert_eq!(
            verdict(&gate, &lookup, &fx, &event).await,
            DispatcherVerdict::AuthorIsAgent,
            "a bot-role channel owner must not wake the dispatcher"
        );
        assert!(
            lookup.profile_batches().is_empty(),
            "the bot role decides without a profile lookup"
        );

        let gate = make_gate(channel_config(fx.channel, policy(&[&bot_hex], &[])));
        assert_eq!(
            verdict(&gate, &lookup, &fx, &event).await,
            DispatcherVerdict::AuthorIsAgent,
            "listing a bot as a human does not make it one"
        );

        let gate = make_gate(channel_config(fx.channel, policy(&[], &[&bot_hex])));
        assert_eq!(
            verdict(&gate, &lookup, &fx, &event).await,
            DispatcherVerdict::Allow,
            "the ais allowlist is the only way to admit a bot"
        );
    }

    /// Every Buzz human has a kind-0 profile; a trusted pubkey without one is
    /// more likely a script key, so it is not admitted.
    #[tokio::test]
    async fn trusted_author_without_a_profile_is_not_admitted() {
        let fx = fixture();
        let lookup = StaticLookup {
            admins: HashMap::from([(fx.channel, HashSet::from([fx.human_hex.clone()]))]),
            absent: HashSet::from([fx.human_hex.clone()]),
            ..Default::default()
        };
        let event = message(&fx.human, KIND_STREAM_MESSAGE, &[]);
        assert_eq!(
            verdict(
                &make_gate(DispatcherConfig::default()),
                &lookup,
                &fx,
                &event
            )
            .await,
            DispatcherVerdict::NotTrusted,
            "an owner with no profile is untrusted"
        );
        assert_eq!(
            verdict(
                &make_gate(channel_config(fx.channel, policy(&[&fx.human_hex], &[]))),
                &lookup,
                &fx,
                &event
            )
            .await,
            DispatcherVerdict::NotTrusted,
            "a listed human with no profile is untrusted"
        );
        assert_eq!(
            verdict(
                &make_gate(channel_config(fx.channel, policy(&[], &[&fx.human_hex]))),
                &lookup,
                &fx,
                &event
            )
            .await,
            DispatcherVerdict::Allow,
            "the ais allowlist does not require a profile"
        );
    }

    #[tokio::test]
    async fn dispatcher_skips_its_own_messages() {
        let agent_keys = Keys::generate();
        let fx = Fixture {
            agent: agent_keys.public_key().to_hex(),
            ..fixture()
        };
        let gate = make_gate(channel_config(
            fx.channel,
            policy(&[&fx.agent], &[&fx.agent]),
        ));
        let lookup = StaticLookup::default();
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&agent_keys, KIND_STREAM_MESSAGE, &[])
            )
            .await,
            DispatcherVerdict::SelfAuthored
        );
    }

    #[tokio::test]
    async fn messages_already_mentioning_another_agent_are_skipped() {
        let fx = fixture();
        let colleague = Keys::generate().public_key().to_hex();
        let gate = make_gate(channel_config(fx.channel, policy(&[&fx.human_hex], &[])));
        let lookup = StaticLookup {
            agents: HashSet::from([fx.other_agent_hex.clone()]),
            ..Default::default()
        };
        let other = fx.other_agent_hex.clone();
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &[&other])
            )
            .await,
            DispatcherVerdict::AlreadyAssigned,
            "a message that already addresses another agent has an assignee"
        );
        let me = fx.agent.clone();
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &[&me])
            )
            .await,
            DispatcherVerdict::Allow,
            "mentioning the dispatcher itself must still dispatch"
        );
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &[&colleague, &me])
            )
            .await,
            DispatcherVerdict::Allow,
            "mentioning a human colleague is not an assignment"
        );
    }

    /// A mentioned member holding the channel `bot` role is an assignee even
    /// without an attested profile, and is recognised without a profile query.
    #[tokio::test]
    async fn mentioning_a_bot_role_member_is_an_assignment() {
        let fx = fixture();
        let bot_hex = Keys::generate().public_key().to_hex();
        let gate = make_gate(channel_config(fx.channel, policy(&[&fx.human_hex], &[])));
        let lookup = StaticLookup {
            bots: HashMap::from([(fx.channel, HashSet::from([bot_hex.clone()]))]),
            ..Default::default()
        };
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &[&bot_hex])
            )
            .await,
            DispatcherVerdict::AlreadyAssigned
        );
        assert_eq!(
            lookup.profile_batches(),
            vec![vec![fx.human_hex.clone()]],
            "only the author's profile was looked up"
        );
    }

    /// Mentioned pubkeys are classified with one batched profile query, and a
    /// message carrying more than `MAX_MENTION_LOOKUPS` mentions is dropped
    /// instead of being looked up.
    #[tokio::test]
    async fn mentions_are_classified_in_one_batch_and_capped() {
        let fx = fixture();
        let gate = make_gate(channel_config(fx.channel, policy(&[&fx.human_hex], &[])));
        let lookup = StaticLookup::default();
        let humans: Vec<String> = (0..MAX_MENTION_LOOKUPS)
            .map(|_| Keys::generate().public_key().to_hex())
            .collect();
        let refs: Vec<&str> = humans.iter().map(String::as_str).collect();
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &refs)
            )
            .await,
            DispatcherVerdict::Allow
        );
        let batches = lookup.profile_batches();
        assert_eq!(
            batches.len(),
            2,
            "one query for the author, one for all mentions"
        );
        assert_eq!(batches[0], vec![fx.human_hex.clone()]);
        let mut expected = humans.clone();
        expected.sort();
        let mut batched = batches[1].clone();
        batched.sort();
        assert_eq!(batched, expected, "every mention travels in the same batch");

        // Cached verdicts are not re-fetched; a second message mentioning the
        // same people (plus one newcomer) only queries the newcomer.
        let newcomer = Keys::generate().public_key().to_hex();
        let mut refs = refs.clone();
        refs.pop();
        refs.push(&newcomer);
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &refs)
            )
            .await,
            DispatcherVerdict::Allow
        );
        assert_eq!(lookup.profile_batches().last(), Some(&vec![newcomer]));

        // One over the cap: dropped, and no profile query is issued for it.
        let before = lookup.profile_batches().len();
        let mut too_many: Vec<String> = humans.clone();
        too_many.push(Keys::generate().public_key().to_hex());
        let refs: Vec<&str> = too_many.iter().map(String::as_str).collect();
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &refs)
            )
            .await,
            DispatcherVerdict::LookupFailed
        );
        assert_eq!(lookup.profile_batches().len(), before);

        // A failed mention batch fails closed.
        let lookup = StaticLookup {
            fail_profiles_containing: Some(fx.other_agent_hex.clone()),
            ..Default::default()
        };
        assert_eq!(
            verdict(
                &gate,
                &lookup,
                &fx,
                &message(&fx.human, KIND_STREAM_MESSAGE, &[&fx.other_agent_hex])
            )
            .await,
            DispatcherVerdict::LookupFailed
        );
    }

    /// A `p` tag that is not a 64-hex pubkey (npub, truncated, empty) is
    /// ignored; it neither blocks the request nor counts as a lookup failure.
    #[tokio::test]
    async fn malformed_p_tags_are_ignored() {
        let fx = fixture();
        let gate = make_gate(channel_config(fx.channel, policy(&[&fx.human_hex], &[])));
        let lookup = StaticLookup {
            agents: HashSet::from([fx.other_agent_hex.clone()]),
            ..Default::default()
        };
        let malformed = [
            "npub1sn0wdenkukak0d9dfczzeacvhkrgz92ak56egt7vdgzn8pv2wfqqhrjdv9".to_string(),
            "abc123".to_string(),
            String::new(),
            "zz".repeat(32),
        ];
        let event = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "help")
            .tags(
                malformed
                    .iter()
                    .map(|v| Tag::parse(["p", v.as_str()]).expect("p tag")),
            )
            .sign_with_keys(&fx.human)
            .expect("signed event");
        assert_eq!(
            verdict(&gate, &lookup, &fx, &event).await,
            DispatcherVerdict::Allow,
            "malformed mentions do not block an otherwise valid request"
        );
        assert_eq!(
            lookup.profile_batches(),
            vec![vec![fx.human_hex.clone()]],
            "malformed values are never looked up"
        );

        // A malformed tag next to a real agent mention still yields the
        // assignment verdict from the real one.
        let mixed = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "help")
            .tags([
                Tag::parse(["p", "npub1broken"]).expect("p tag"),
                Tag::parse(["p", &fx.other_agent_hex]).expect("p tag"),
            ])
            .sign_with_keys(&fx.human)
            .expect("signed event");
        assert_eq!(
            verdict(&gate, &lookup, &fx, &mixed).await,
            DispatcherVerdict::AlreadyAssigned
        );
    }

    #[tokio::test]
    async fn only_plain_human_stream_messages_are_dispatched() {
        let fx = fixture();
        let gate = make_gate(channel_config(fx.channel, policy(&[&fx.human_hex], &[])));
        let lookup = StaticLookup::default();
        for kind in [
            buzz_core::kind::KIND_STREAM_MESSAGE_EDIT,
            buzz_core::kind::KIND_WORKFLOW_APPROVAL_REQUESTED,
            buzz_core::kind::KIND_STREAM_REMINDER,
        ] {
            assert_eq!(
                verdict(&gate, &lookup, &fx, &message(&fx.human, kind, &[])).await,
                DispatcherVerdict::NotHumanStreamMessage,
                "kind {kind} is not a human request"
            );
        }
        // Relay-signed workflow traffic: the signer is the relay.
        let relay = Keys::generate();
        let relay_hex = relay.public_key().to_hex();
        let workflow = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "scheduled")
            .tags([Tag::parse(["buzz:workflow", "true"]).unwrap()])
            .sign_with_keys(&relay)
            .unwrap();
        assert_eq!(
            gate.evaluate(
                &lookup,
                &workflow,
                &relay_hex,
                fx.channel,
                Some(&relay_hex),
                &fx.agent
            )
            .await,
            DispatcherVerdict::NotHumanStreamMessage
        );
        // Delegated attribution (effective author differs from the signer).
        let plain = message(&fx.human, KIND_STREAM_MESSAGE, &[]);
        assert_eq!(
            gate.evaluate(
                &lookup,
                &plain,
                &fx.other_agent_hex,
                fx.channel,
                None,
                &fx.agent
            )
            .await,
            DispatcherVerdict::NotHumanStreamMessage
        );
    }

    #[tokio::test]
    async fn lookup_failures_fail_closed_and_are_not_cached() {
        let fx = fixture();
        let gate = make_gate(DispatcherConfig::default());
        let failing = StaticLookup {
            fail: true,
            ..Default::default()
        };
        let event = message(&fx.human, KIND_STREAM_MESSAGE, &[]);
        assert_eq!(
            verdict(&gate, &failing, &fx, &event).await,
            DispatcherVerdict::LookupFailed
        );
        // Once the relay answers, the same gate admits the owner: nothing
        // negative was cached by the failure.
        let healthy = fx.owner_lookup();
        assert_eq!(
            verdict(&gate, &healthy, &fx, &event).await,
            DispatcherVerdict::Allow
        );
    }

    #[tokio::test]
    async fn cached_verdicts_survive_a_later_lookup_failure() {
        let fx = fixture();
        let gate = make_gate(DispatcherConfig::default());
        let healthy = fx.owner_lookup();
        let event = message(&fx.human, KIND_STREAM_MESSAGE, &[]);
        assert_eq!(
            verdict(&gate, &healthy, &fx, &event).await,
            DispatcherVerdict::Allow
        );
        let failing = StaticLookup {
            fail: true,
            ..Default::default()
        };
        assert_eq!(
            verdict(&gate, &failing, &fx, &event).await,
            DispatcherVerdict::Allow,
            "the bot set, the admin set and the author's profile verdict are cached"
        );
    }

    #[test]
    fn profile_kinds_require_a_valid_nip_oa_signature_to_prove_an_agent() {
        let owner = Keys::generate();
        let agent = Keys::generate();
        let agent_hex = agent.public_key().to_hex();
        let tag =
            buzz_sdk::nip_oa::compute_auth_tag(&owner, &agent.public_key(), "").expect("auth tag");
        let tag: serde_json::Value = serde_json::from_str(&tag).expect("tag json");
        let other = Keys::generate();
        let other_hex = other.public_key().to_hex();
        let human_hex = Keys::generate().public_key().to_hex();

        let json = serde_json::json!([
            // Valid attestation for the agent.
            { "pubkey": agent_hex.to_uppercase(), "tags": [tag] },
            // The same tag does not prove a different pubkey is an agent.
            { "pubkey": other_hex, "tags": [tag] },
            // Shape-only forgery: four parts, bogus signature.
            { "pubkey": human_hex, "tags": [["auth", owner.public_key().to_hex(), "", "00"]] },
            // Unparseable author: ignored.
            { "pubkey": "not-a-key", "tags": [] },
            { "tags": [] }
        ]);
        let kinds = parse_profile_kinds(&json);
        assert_eq!(
            kinds.len(),
            3,
            "keys are lowercased; bad authors are skipped"
        );
        assert_eq!(kinds[&agent_hex], ProfileKind::Agent);
        assert_eq!(kinds[&other_hex], ProfileKind::Human);
        assert_eq!(kinds[&human_hex], ProfileKind::Human);

        // A human-looking duplicate never downgrades a proven agent.
        let json = serde_json::json!([
            { "pubkey": agent_hex, "tags": [] },
            { "pubkey": agent_hex, "tags": [tag] },
            { "pubkey": agent_hex, "tags": [] }
        ]);
        assert_eq!(parse_profile_kinds(&json)[&agent_hex], ProfileKind::Agent);

        // No profile at all is simply absent (the gate treats it as untrusted).
        assert!(parse_profile_kinds(&serde_json::json!([])).is_empty());
        assert!(parse_profile_kinds(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn mentioned_pubkeys_dedup_lowercase_and_skip_non_hex_p_tags() {
        let keys = Keys::generate();
        let a = "AB".repeat(32);
        let b = "cd".repeat(32);
        let event = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "hi")
            .tags([
                Tag::parse(["p", &a]).unwrap(),
                Tag::parse(["e", &b]).unwrap(),
                Tag::parse(["p", "npub1broken"]).unwrap(),
                Tag::parse(["p", &b]).unwrap(),
                Tag::parse(["p", &"gg".repeat(32)]).unwrap(),
                Tag::parse(["p", &a.to_ascii_lowercase()]).unwrap(),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(mentioned_pubkeys(&event), vec![a.to_ascii_lowercase(), b]);
    }

    #[test]
    fn roster_renders_roles_names_about_and_agent_flags() {
        let me = "aa".repeat(32);
        let members = vec![
            RosterMember {
                pubkey: "bb".repeat(32),
                role: "member".into(),
                name: Some("Coder".into()),
                about: Some("  Writes   and\nreviews code ".into()),
                is_agent: true,
            },
            RosterMember {
                pubkey: "cc".repeat(32),
                role: "owner".into(),
                name: Some("Alice <Smith>".into()),
                about: None,
                is_agent: false,
            },
            RosterMember {
                pubkey: me.clone(),
                role: "member".into(),
                name: Some("Dispatcher".into()),
                about: None,
                is_agent: true,
            },
            RosterMember {
                pubkey: "dd".repeat(32),
                role: "member".into(),
                name: None,
                about: None,
                is_agent: false,
            },
        ];
        let body = render_channel_roster(&members, &me);
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines[0],
            "Members of this channel (4). Address an agent with its exact name and pass its pubkey with `--mention`."
        );
        assert_eq!(
            lines[1],
            format!(
                "- Alice &lt;Smith&gt; (owner, human) pubkey={}",
                "cc".repeat(32)
            ),
            "owner first, untrusted name escaped"
        );
        assert_eq!(
            lines[2],
            format!("- (unnamed) (member, human) pubkey={}", "dd".repeat(32)),
            "unnamed members sort before named ones by empty name"
        );
        assert_eq!(
            lines[3],
            format!(
                "- Coder (member, agent) pubkey={} — Writes and reviews code",
                "bb".repeat(32)
            ),
            "about is whitespace-collapsed"
        );
        assert_eq!(
            lines[4],
            format!("- Dispatcher (member, agent, you) pubkey={me}"),
            "the dispatcher is marked as itself"
        );
    }

    #[test]
    fn roster_profiles_parse_display_name_name_about_and_agent_hint() {
        let json = serde_json::json!([
            {
                "pubkey": "AA",
                "content": "{\"name\":\"coder\",\"display_name\":\"Coder\",\"about\":\"Builds things\"}",
                "tags": [["auth", "owner", "", "sig"]]
            },
            {
                "pubkey": "bb",
                "content": "{\"name\":\"alice\",\"about\":\"   \"}",
                "tags": []
            },
            { "pubkey": "cc", "content": "not json", "tags": [] }
        ]);
        let profiles = parse_roster_profiles(&json);
        let coder = &profiles["aa"];
        assert_eq!(coder.name.as_deref(), Some("Coder"));
        assert_eq!(coder.about.as_deref(), Some("Builds things"));
        assert!(coder.is_agent);
        let alice = &profiles["bb"];
        assert_eq!(alice.name.as_deref(), Some("alice"));
        assert_eq!(alice.about, None);
        assert!(!alice.is_agent);
        assert!(!profiles.contains_key("cc"));
    }
}
