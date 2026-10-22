//! Group chats: ownerless private streams created by one kind:9007 carrying a
//! `["group_chat"]` tag and up to [`GROUP_CHAT_MAX_INITIAL_PEOPLE`] initial
//! `p` tags.
//!
//! The creation request is checked here ([`parse_group_chat_create`], pure),
//! then every initial person is resolved against the same rules a kind:9000
//! add uses ([`plan_initial_people`]): a person who blocks the creator is
//! counted as invited but nothing is stored (as a kind:9000 add answers
//! "invitation sent"), agents follow their `channel_add_policy` and join as `bot`,
//! people join as `member` when their "who can add me" setting admits the
//! creator and otherwise get an invitation. One person needing an invitation
//! never fails the whole creation.

use std::sync::Arc;

use kuzz_core::tenant::TenantContext;
use kuzz_db::channel::MemberRole;
pub use kuzz_db::channel::GROUP_CHAT_MAX_INITIAL_PEOPLE;
use nostr::Event;
use uuid::Uuid;

use super::channel_authz;
use crate::state::AppState;

/// The bare marker tag of a group chat (kind:9007 and kind:39000).
pub const GROUP_CHAT_TAG: &str = "group_chat";

/// Whether `event` carries the bare `["group_chat"]` marker.
pub fn has_group_chat_tag(event: &Event) -> bool {
    event
        .tags
        .iter()
        .any(|t| t.as_slice().first().map(String::as_str) == Some(GROUP_CHAT_TAG))
}

/// Longest chat name, in characters (`channels.name` is `VARCHAR(255)`).
pub const GROUP_CHAT_MAX_NAME_CHARS: usize = 255;

/// A validated group chat creation request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupChatCreate {
    /// Shared name; empty for an unnamed chat.
    pub name: String,
    /// Distinct initial people (32-byte keys), never the creator.
    pub people: Vec<Vec<u8>>,
}

/// Tags a group chat creation must not carry: a chat has no description,
/// photo, address or expiry.
const FORBIDDEN_CREATE_TAGS: &[&str] = &["ttl", "about", "picture", "handle"];

/// Validate a kind:9007 group chat creation by `creator`. Returns the client
/// visible reason on rejection.
pub fn parse_group_chat_create(event: &Event, creator: &[u8]) -> Result<GroupChatCreate, String> {
    let mut name = None;
    let mut people: Vec<Vec<u8>> = Vec::new();
    for tag in event.tags.iter() {
        let parts = tag.as_slice();
        let Some(key) = parts.first().map(String::as_str) else {
            continue;
        };
        match key {
            "visibility" if parts.get(1).map(String::as_str) != Some("private") => {
                return Err("chats are always private".into());
            }
            "channel_type" if parts.get(1).map(String::as_str) != Some("stream") => {
                return Err("chats are always streams".into());
            }
            "name" if name.is_none() => {
                name = Some(parts.get(1).cloned().unwrap_or_default());
            }
            "p" => {
                let raw = parts.get(1).map(String::as_str).unwrap_or("");
                let pubkey = match hex::decode(raw) {
                    Ok(bytes) if bytes.len() == 32 => bytes,
                    _ => return Err(format!("invalid p tag: {raw}")),
                };
                if pubkey == creator {
                    return Err("do not list yourself as a chat member".into());
                }
                if !people.contains(&pubkey) {
                    people.push(pubkey);
                }
            }
            other if FORBIDDEN_CREATE_TAGS.contains(&other) => {
                return Err(format!("chats cannot have a {other} tag"));
            }
            _ => {}
        }
    }
    if people.len() > GROUP_CHAT_MAX_INITIAL_PEOPLE {
        return Err(format!(
            "a chat starts with at most {GROUP_CHAT_MAX_INITIAL_PEOPLE} people"
        ));
    }
    let name = kuzz_core::channel::canonical_channel_name(&name.unwrap_or_default())
        .trim()
        .to_owned();
    if name.chars().count() > GROUP_CHAT_MAX_NAME_CHARS {
        return Err(format!(
            "a chat name has at most {GROUP_CHAT_MAX_NAME_CHARS} characters"
        ));
    }
    Ok(GroupChatCreate { name, people })
}

/// What happens to one initial person of a new group chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialAdd {
    /// Added as `member`.
    Member,
    /// Added as `bot` (an agent).
    Bot,
    /// Sent an invitation.
    Invite,
    /// A person who blocks the creator: reported as invited, nothing stored,
    /// so the creator cannot tell the block from a pending invitation.
    HiddenInvite,
    /// Not added and nothing stored (an agent blocking or refusing the add).
    Skip,
    /// Not added: the creator hit the daily invitation cap.
    RateLimited,
}

/// Decide one initial person from already-resolved facts, mirroring a
/// kind:9000 add by an active member:
/// - a person who blocks the creator → reported as invited, nothing stored;
/// - an agent that blocks the creator, or whose `channel_add_policy` refuses
///   the creator → skip;
/// - agents → `bot`;
/// - people the "who can add me" setting admits → `member`;
/// - otherwise an invitation, unless the daily cap is used up.
pub fn decide_initial_person(
    target_blocks_creator: bool,
    add_policy_allows: bool,
    is_agent: bool,
    admitted: bool,
    invitation_allowed: bool,
) -> InitialAdd {
    if target_blocks_creator && !is_agent {
        InitialAdd::HiddenInvite
    } else if target_blocks_creator || !add_policy_allows {
        InitialAdd::Skip
    } else if is_agent {
        InitialAdd::Bot
    } else if admitted {
        InitialAdd::Member
    } else if invitation_allowed {
        InitialAdd::Invite
    } else {
        InitialAdd::RateLimited
    }
}

/// The resolved initial people of a new group chat.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InitialPeople {
    /// Added now, with their role.
    pub members: Vec<(Vec<u8>, MemberRole)>,
    /// Sent an invitation.
    pub invitees: Vec<Vec<u8>>,
    /// People who block the creator: counted as invited, nothing stored.
    pub hidden_invites: usize,
    /// Skipped because the creator hit the daily invitation cap.
    pub rate_limited: usize,
}

impl InitialPeople {
    /// The `OK` message of the creation: empty, or `invite-sent:<n>` and/or
    /// `rate-limited:<n>` (space separated).
    pub fn ok_message(&self) -> String {
        let mut parts = Vec::new();
        let invited = self.invitees.len() + self.hidden_invites;
        if invited > 0 {
            parts.push(format!(
                "{}{invited}",
                super::side_effects::INVITE_SENT_PREFIX
            ));
        }
        if self.rate_limited > 0 {
            parts.push(format!("rate-limited:{}", self.rate_limited));
        }
        parts.join(" ")
    }
}

/// Whether `pubkey` is an agent: it has an owner, or is a bot principal.
pub(crate) async fn is_agent_pubkey(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    pubkey: &[u8],
) -> anyhow::Result<bool> {
    let owned = state
        .db
        .get_agent_channel_policy(tenant.community(), pubkey)
        .await?
        .is_some_and(|(_, owner)| owner.is_some());
    Ok(owned || state.db.is_bot_principal(pubkey).await?)
}

/// Resolve every initial person of a new group chat (`channel_id`, not yet
/// created) for `creator`. Reads only; the caller writes the result in one
/// transaction.
pub async fn plan_initial_people(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    channel_id: Uuid,
    creator: &[u8],
    people: &[Vec<u8>],
) -> anyhow::Result<InitialPeople> {
    let community = tenant.community();
    let creator_agent_owner = state
        .db
        .get_agent_channel_policy(community, creator)
        .await?
        .and_then(|(_, owner)| owner);
    let mut sent_last_day: Option<i64> = None;
    let mut plan = InitialPeople::default();
    for target in people {
        let blocked = state.db.is_blocked_by(community, target, creator).await?;
        let agent_policy = state.db.get_agent_channel_policy(community, target).await?;
        let add_policy_allows = match &agent_policy {
            Some((policy, owner)) => {
                channel_authz::decide_channel_add_policy(policy, owner.as_deref(), creator).is_ok()
            }
            None => true,
        };
        let is_agent = agent_policy
            .as_ref()
            .is_some_and(|(_, owner)| owner.is_some())
            || state.db.is_bot_principal(target).await?;
        let admitted = if blocked || !add_policy_allows || is_agent {
            false
        } else if state.db.group_add_policy(community, target).await? == "everyone" {
            true
        } else {
            // The creator runs the chat it is creating, so an agent may bring
            // in its own owner (see `admits_as_friend`).
            channel_authz::admits_as_friend(false, creator_agent_owner.as_deref(), target, true)
                || super::side_effects::is_friend_of(tenant, state, target, creator).await?
        };
        let invitation_allowed = if blocked || !add_policy_allows || is_agent || admitted {
            true
        } else {
            let sent = match sent_last_day {
                Some(n) => n,
                None => {
                    state
                        .db
                        .group_invitation_write_state(community, channel_id, creator, target)
                        .await?
                        .sent_last_day
                }
            };
            let allowed = matches!(
                channel_authz::decide_invitation_write(
                    false,
                    false,
                    false,
                    sent,
                    kuzz_db::groups::INVITATIONS_PER_DAY,
                ),
                channel_authz::InvitationWrite::Upsert
            );
            sent_last_day = Some(if allowed { sent + 1 } else { sent });
            allowed
        };
        match decide_initial_person(
            blocked,
            add_policy_allows,
            is_agent,
            admitted,
            invitation_allowed,
        ) {
            InitialAdd::Member => plan.members.push((target.clone(), MemberRole::Member)),
            InitialAdd::Bot => plan.members.push((target.clone(), MemberRole::Bot)),
            InitialAdd::Invite => plan.invitees.push(target.clone()),
            InitialAdd::HiddenInvite => plan.hidden_invites += 1,
            InitialAdd::RateLimited => plan.rate_limited += 1,
            InitialAdd::Skip => {}
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind, Tag};

    fn create_event(keys: &Keys, tags: Vec<Vec<String>>) -> Event {
        let mut all = vec![
            Tag::parse(["h", &Uuid::new_v4().to_string()]).unwrap(),
            Tag::parse([GROUP_CHAT_TAG]).unwrap(),
        ];
        all.extend(tags.into_iter().map(|t| Tag::parse(t).unwrap()));
        EventBuilder::new(Kind::Custom(9007), "")
            .tags(all)
            .allow_self_tagging()
            .sign_with_keys(keys)
            .unwrap()
    }

    fn s(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn marker_tag_is_detected() {
        let keys = Keys::generate();
        assert!(has_group_chat_tag(&create_event(&keys, vec![])));
        let plain = EventBuilder::new(Kind::Custom(9007), "")
            .tags([Tag::parse(["name", "x"]).unwrap()])
            .sign_with_keys(&keys)
            .unwrap();
        assert!(!has_group_chat_tag(&plain));
    }

    #[test]
    fn name_is_optional_and_may_be_empty() {
        let keys = Keys::generate();
        let me = keys.public_key().to_bytes();
        let unnamed = parse_group_chat_create(&create_event(&keys, vec![]), &me).unwrap();
        assert_eq!(unnamed.name, "");
        let empty =
            parse_group_chat_create(&create_event(&keys, vec![s(&["name", ""])]), &me).unwrap();
        assert_eq!(empty.name, "");
        let named = parse_group_chat_create(
            &create_event(
                &keys,
                vec![
                    s(&["name", " Trip "]),
                    s(&["visibility", "private"]),
                    s(&["channel_type", "stream"]),
                ],
            ),
            &me,
        )
        .unwrap();
        assert_eq!(named.name, "Trip");
    }

    #[test]
    fn rejects_open_non_stream_and_managed_group_tags() {
        let keys = Keys::generate();
        let me = keys.public_key().to_bytes();
        for tags in [
            vec![s(&["visibility", "open"])],
            vec![s(&["channel_type", "forum"])],
            vec![s(&["ttl", "3600"])],
            vec![s(&["about", "hello"])],
            vec![s(&["picture", "https://x/media/a"])],
            vec![s(&["handle", "trip"])],
        ] {
            assert!(
                parse_group_chat_create(&create_event(&keys, tags.clone()), &me).is_err(),
                "{tags:?} must be rejected"
            );
        }
    }

    #[test]
    fn people_are_deduplicated_and_bounded() {
        let keys = Keys::generate();
        let me = keys.public_key().to_bytes();
        let other = Keys::generate().public_key().to_hex();
        let parsed = parse_group_chat_create(
            &create_event(&keys, vec![s(&["p", &other]), s(&["p", &other])]),
            &me,
        )
        .unwrap();
        assert_eq!(parsed.people.len(), 1);

        let self_listed = create_event(&keys, vec![s(&["p", &keys.public_key().to_hex()])]);
        assert!(parse_group_chat_create(&self_listed, &me).is_err());

        let bad = create_event(&keys, vec![s(&["p", "zz"])]);
        assert!(parse_group_chat_create(&bad, &me).is_err());

        let exactly_max: Vec<Vec<String>> = (0..GROUP_CHAT_MAX_INITIAL_PEOPLE)
            .map(|_| s(&["p", &Keys::generate().public_key().to_hex()]))
            .collect();
        assert!(parse_group_chat_create(&create_event(&keys, exactly_max.clone()), &me).is_ok());
        let mut too_many = exactly_max;
        too_many.push(s(&["p", &Keys::generate().public_key().to_hex()]));
        assert!(parse_group_chat_create(&create_event(&keys, too_many), &me).is_err());
    }

    /// Full combination table of the initial-person decision.
    #[test]
    fn initial_person_table() {
        for blocked in [false, true] {
            for policy_ok in [false, true] {
                for agent in [false, true] {
                    for admitted in [false, true] {
                        for invite_ok in [false, true] {
                            let expected = if blocked && !agent {
                                InitialAdd::HiddenInvite
                            } else if blocked || !policy_ok {
                                InitialAdd::Skip
                            } else if agent {
                                InitialAdd::Bot
                            } else if admitted {
                                InitialAdd::Member
                            } else if invite_ok {
                                InitialAdd::Invite
                            } else {
                                InitialAdd::RateLimited
                            };
                            assert_eq!(
                                decide_initial_person(
                                    blocked, policy_ok, agent, admitted, invite_ok
                                ),
                                expected,
                                "blocked={blocked} policy_ok={policy_ok} agent={agent} \
                                 admitted={admitted} invite_ok={invite_ok}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn ok_message_reports_invitations_and_rate_limit() {
        let mut plan = InitialPeople::default();
        assert_eq!(plan.ok_message(), "");
        plan.invitees.push(vec![1; 32]);
        plan.invitees.push(vec![2; 32]);
        assert_eq!(plan.ok_message(), "invite-sent:2");
        plan.rate_limited = 1;
        assert_eq!(plan.ok_message(), "invite-sent:2 rate-limited:1");
        plan.hidden_invites = 1;
        assert_eq!(
            plan.ok_message(),
            "invite-sent:3 rate-limited:1",
            "people who block the creator count as invited"
        );
        let only_hidden = InitialPeople {
            hidden_invites: 1,
            ..InitialPeople::default()
        };
        assert_eq!(only_hidden.ok_message(), "invite-sent:1");
    }

    #[test]
    fn name_longer_than_the_column_is_rejected() {
        let keys = Keys::generate();
        let me = keys.public_key().to_bytes();
        let max = "가".repeat(GROUP_CHAT_MAX_NAME_CHARS);
        let ok = parse_group_chat_create(&create_event(&keys, vec![s(&["name", &max])]), &me)
            .expect("a name at the limit is accepted");
        assert_eq!(ok.name.chars().count(), GROUP_CHAT_MAX_NAME_CHARS);
        let long = format!("{max}a");
        let err = parse_group_chat_create(&create_event(&keys, vec![s(&["name", &long])]), &me)
            .unwrap_err();
        assert!(err.contains("at most"), "{err}");
    }
}

#[cfg(test)]
#[path = "group_chat_postgres_tests.rs"]
mod postgres_tests;
