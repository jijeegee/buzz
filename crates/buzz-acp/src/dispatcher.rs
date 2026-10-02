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

/// Relay lookups the dispatcher gate depends on. `None` means the lookup
/// failed (timeout, transport, malformed response) and the gate fails closed;
/// `Some(false)` / an empty set is an authoritative negative answer.
pub(crate) trait TrustLookup {
    /// Owner and admin pubkeys from the channel's kind-39001 admins event.
    async fn channel_admins(&self, channel_id: Uuid) -> Option<HashSet<String>>;
    /// Whether the pubkey's kind-0 profile carries a cryptographically valid
    /// NIP-OA `auth` tag. A missing profile is a human (`Some(false)`).
    async fn profile_is_agent(&self, pubkey_hex: &str) -> Option<bool>;
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
    /// Author is neither a configured human nor a channel owner/admin.
    NotTrusted,
    /// Author's profile proves it is an owned agent and it is not allowlisted.
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
    admins: Mutex<HashMap<Uuid, Cached<HashSet<String>>>>,
    profiles: Mutex<HashMap<String, Cached<bool>>>,
}

impl DispatcherGate {
    pub(crate) fn new(config: DispatcherConfig) -> Self {
        Self {
            config,
            admins: Mutex::new(HashMap::new()),
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

        let policy = self.config.channel(channel_id);
        let allowlisted_ai = policy.is_some_and(|p| p.ais.contains(&author));
        if !allowlisted_ai {
            let trusted = match policy.filter(|p| !p.humans.is_empty()) {
                Some(p) => p.humans.contains(&author),
                None => match self.admins_for(lookup, channel_id).await {
                    Some(admins) => admins.contains(&author),
                    None => return DispatcherVerdict::LookupFailed,
                },
            };
            if !trusted {
                return DispatcherVerdict::NotTrusted;
            }
            match self.is_agent(lookup, &author).await {
                Some(true) => return DispatcherVerdict::AuthorIsAgent,
                Some(false) => {}
                None => return DispatcherVerdict::LookupFailed,
            }
        }

        for mentioned in mentioned_pubkeys(event) {
            if mentioned.eq_ignore_ascii_case(agent_pubkey_hex) || mentioned == author {
                continue;
            }
            match self.is_agent(lookup, &mentioned).await {
                Some(true) => return DispatcherVerdict::AlreadyAssigned,
                Some(false) => {}
                None => return DispatcherVerdict::LookupFailed,
            }
        }
        DispatcherVerdict::Allow
    }

    async fn admins_for<L: TrustLookup>(
        &self,
        lookup: &L,
        channel_id: Uuid,
    ) -> Option<HashSet<String>> {
        if let Some(cached) = self.admins.lock().ok().and_then(|cache| {
            cache
                .get(&channel_id)
                .filter(|c| c.fetched_at.elapsed() < ADMINS_CACHE_TTL)
                .map(|c| c.value.clone())
        }) {
            return Some(cached);
        }
        let admins = lookup.channel_admins(channel_id).await?;
        if let Ok(mut cache) = self.admins.lock() {
            if cache.len() >= CACHE_CAP {
                cache.clear();
            }
            cache.insert(
                channel_id,
                Cached {
                    fetched_at: Instant::now(),
                    value: admins.clone(),
                },
            );
        }
        Some(admins)
    }

    async fn is_agent<L: TrustLookup>(&self, lookup: &L, pubkey_hex: &str) -> Option<bool> {
        if let Some(cached) = self.profiles.lock().ok().and_then(|cache| {
            cache
                .get(pubkey_hex)
                .filter(|c| c.fetched_at.elapsed() < PROFILE_CACHE_TTL)
                .map(|c| c.value)
        }) {
            return Some(cached);
        }
        let is_agent = lookup.profile_is_agent(pubkey_hex).await?;
        if let Ok(mut cache) = self.profiles.lock() {
            if cache.len() >= CACHE_CAP {
                cache.clear();
            }
            cache.insert(
                pubkey_hex.to_string(),
                Cached {
                    fetched_at: Instant::now(),
                    value: is_agent,
                },
            );
        }
        Some(is_agent)
    }
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

/// Lowercase hex pubkeys from the event's `p` tags, in order, deduplicated.
fn mentioned_pubkeys(event: &nostr::Event) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for tag in event.tags.iter() {
        let values = tag.as_slice();
        if values.first().map(String::as_str) != Some("p") {
            continue;
        }
        let Some(pubkey) = values.get(1) else {
            continue;
        };
        let pubkey = pubkey.to_ascii_lowercase();
        if seen.insert(pubkey.clone()) {
            out.push(pubkey);
        }
    }
    out
}

/// Whether any kind-0 event in a `/query` response carries a NIP-OA `auth`
/// tag that cryptographically verifies for `agent_pk`. Unlike the shape-only
/// heuristic used for reply anchoring, this is a trust decision, so the
/// signature is checked.
pub(crate) fn profile_events_prove_agent(
    json: &serde_json::Value,
    agent_pk: &nostr::PublicKey,
) -> bool {
    let Some(events) = json.as_array() else {
        return false;
    };
    events
        .iter()
        .filter_map(|ev| ev.get("tags").and_then(|t| t.as_array()))
        .flatten()
        .filter(|tag| {
            tag.as_array()
                .is_some_and(|parts| parts.len() >= 4 && parts[0].as_str() == Some("auth"))
        })
        .any(|tag| {
            serde_json::to_string(tag).ok().is_some_and(|tag_json| {
                buzz_sdk::nip_oa::verify_auth_tag(&tag_json, agent_pk).is_ok()
            })
        })
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

    async fn profile_is_agent(&self, pubkey_hex: &str) -> Option<bool> {
        let pk = nostr::PublicKey::from_hex(pubkey_hex).ok()?;
        let filter = nostr::Filter::new()
            .kind(nostr::Kind::Metadata)
            .author(pk)
            .limit(1);
        match timeout(
            LOOKUP_TIMEOUT,
            self.rest.query(std::slice::from_ref(&filter)),
        )
        .await
        {
            Ok(Ok(json)) => Some(profile_events_prove_agent(&json, &pk)),
            Ok(Err(e)) => {
                tracing::debug!(pubkey = pubkey_hex, "dispatcher profile lookup failed: {e}");
                None
            }
            Err(_) => {
                tracing::debug!(pubkey = pubkey_hex, "dispatcher profile lookup timed out");
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
    /// `owner`, `admin`, or `member` (from kind 39001; everyone else is a member).
    pub role: String,
    pub name: Option<String>,
    pub about: Option<String>,
    /// Shape-only NIP-OA hint from the kind-0 profile (same heuristic as the
    /// prompt profile lookup) — a routing aid, not a trust decision.
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
        .filter_map(|pk| nostr::PublicKey::from_hex(pk).ok())
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
        .map(|pubkey| {
            let pubkey = pubkey.to_ascii_lowercase();
            let profile = profiles.get(&pubkey);
            RosterMember {
                role: roles
                    .get(&pubkey)
                    .cloned()
                    .unwrap_or_else(|| "member".to_string()),
                name: profile.and_then(|p| p.name.clone()),
                about: profile.and_then(|p| p.about.clone()),
                is_agent: profile.is_some_and(|p| p.is_agent),
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

    /// Scripted lookup: admins per channel and agent-ness per pubkey.
    /// Pubkeys absent from `agents` are humans; `fail` makes every lookup fail.
    #[derive(Default)]
    struct StaticLookup {
        admins: HashMap<Uuid, HashSet<String>>,
        agents: HashSet<String>,
        fail: bool,
    }

    impl TrustLookup for StaticLookup {
        async fn channel_admins(&self, channel_id: Uuid) -> Option<HashSet<String>> {
            if self.fail {
                return None;
            }
            Some(self.admins.get(&channel_id).cloned().unwrap_or_default())
        }

        async fn profile_is_agent(&self, pubkey_hex: &str) -> Option<bool> {
            if self.fail {
                return None;
            }
            Some(self.agents.contains(pubkey_hex))
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
        let config = DispatcherConfig::from_channels(HashMap::from([(
            fx.channel,
            policy(&[&fx.human_hex], &[]),
        )]));
        let gate = DispatcherGate::new(config);
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
            DispatcherConfig::from_channels(HashMap::from([(fx.channel, policy(&[], &[]))])),
        ] {
            let gate = DispatcherGate::new(config);
            let lookup = StaticLookup {
                admins: HashMap::from([(fx.channel, HashSet::from([fx.human_hex.clone()]))]),
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

    #[tokio::test]
    async fn owner_with_agent_profile_is_rejected_unless_allowlisted_as_ai() {
        let fx = fixture();
        let bot = Keys::generate();
        let bot_hex = bot.public_key().to_hex();
        let lookup = StaticLookup {
            admins: HashMap::from([(fx.channel, HashSet::from([bot_hex.clone()]))]),
            agents: HashSet::from([bot_hex.clone()]),
            fail: false,
        };
        // Owner-role sibling bot: the owner+siblings rule is not inherited.
        let gate = DispatcherGate::new(DispatcherConfig::default());
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
        let gate = DispatcherGate::new(DispatcherConfig::from_channels(HashMap::from([(
            fx.channel,
            policy(&[&bot_hex], &[]),
        )])));
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
        let gate = DispatcherGate::new(DispatcherConfig::from_channels(HashMap::from([(
            fx.channel,
            policy(&[], &[&bot_hex]),
        )])));
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

    #[tokio::test]
    async fn dispatcher_skips_its_own_messages() {
        let agent_keys = Keys::generate();
        let fx = Fixture {
            agent: agent_keys.public_key().to_hex(),
            ..fixture()
        };
        let gate = DispatcherGate::new(DispatcherConfig::from_channels(HashMap::from([(
            fx.channel,
            policy(&[&fx.agent], &[&fx.agent]),
        )])));
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
        let gate = DispatcherGate::new(DispatcherConfig::from_channels(HashMap::from([(
            fx.channel,
            policy(&[&fx.human_hex], &[]),
        )])));
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

    #[tokio::test]
    async fn only_plain_human_stream_messages_are_dispatched() {
        let fx = fixture();
        let gate = DispatcherGate::new(DispatcherConfig::from_channels(HashMap::from([(
            fx.channel,
            policy(&[&fx.human_hex], &[]),
        )])));
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
        let gate = DispatcherGate::new(DispatcherConfig::default());
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
        let healthy = StaticLookup {
            admins: HashMap::from([(fx.channel, HashSet::from([fx.human_hex.clone()]))]),
            ..Default::default()
        };
        assert_eq!(
            verdict(&gate, &healthy, &fx, &event).await,
            DispatcherVerdict::Allow
        );
    }

    #[tokio::test]
    async fn cached_verdicts_survive_a_later_lookup_failure() {
        let fx = fixture();
        let gate = DispatcherGate::new(DispatcherConfig::default());
        let healthy = StaticLookup {
            admins: HashMap::from([(fx.channel, HashSet::from([fx.human_hex.clone()]))]),
            ..Default::default()
        };
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
            "both the admin set and the author's profile verdict are cached"
        );
    }

    #[test]
    fn profile_events_prove_agent_requires_a_valid_nip_oa_signature() {
        let owner = Keys::generate();
        let agent = Keys::generate();
        let tag =
            buzz_sdk::nip_oa::compute_auth_tag(&owner, &agent.public_key(), "").expect("auth tag");
        let tag: serde_json::Value = serde_json::from_str(&tag).expect("tag json");
        let valid = serde_json::json!([{ "pubkey": agent.public_key().to_hex(), "tags": [tag] }]);
        assert!(profile_events_prove_agent(&valid, &agent.public_key()));

        // The same tag does not prove a different pubkey is an agent.
        let other = Keys::generate();
        assert!(!profile_events_prove_agent(&valid, &other.public_key()));

        // Shape-only forgery: four parts, bogus signature.
        let forged =
            serde_json::json!([{ "tags": [["auth", owner.public_key().to_hex(), "", "00"]] }]);
        assert!(!profile_events_prove_agent(&forged, &agent.public_key()));

        // No profile at all is a human.
        assert!(!profile_events_prove_agent(
            &serde_json::json!([]),
            &agent.public_key()
        ));
        assert!(!profile_events_prove_agent(
            &serde_json::json!({}),
            &agent.public_key()
        ));
    }

    #[test]
    fn mentioned_pubkeys_dedup_and_lowercase_p_tags() {
        let keys = Keys::generate();
        let a = "AB".repeat(32);
        let b = "cd".repeat(32);
        let event = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "hi")
            .tags([
                Tag::parse(["p", &a]).unwrap(),
                Tag::parse(["e", &b]).unwrap(),
                Tag::parse(["p", &b]).unwrap(),
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
