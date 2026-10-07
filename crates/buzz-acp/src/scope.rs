//! Session scoping for ACP.
//!
//! A [`SessionScope`] is the single hashable key that identifies an ACP
//! provider session and its conversational-context boundary. It is derived
//! **once**, when an eligible event is admitted, from the operator
//! [`SessionPolicy`], whether the channel is a DM, and the event's NIP-10
//! thread tags. Later code must never re-infer scope from the last event in a
//! batch — it carries the resolved scope instead.
//!
//! Policy matrix under [`SessionPolicy::Thread`] ("Main and each thread"):
//!
//! | Surface                             | Scope                                   |
//! | ----------------------------------- | --------------------------------------- |
//! | Top-level channel message           | `Main(channel_id)`                      |
//! | Reply in a channel thread           | `Thread(channel_id, canonical_root)`    |
//! | Repeated mention in the same thread | reuse that thread scope                 |
//! | Direct message                      | `Conversation(channel_id)`              |
//!
//! Under [`SessionPolicy::Channel`] (the current default / rollback path) every
//! surface collapses to `Conversation(channel_id)`, preserving today's
//! channel-keyed behavior exactly.
//!
//! Scope only decides which provider session handles an event. Where the
//! agent replies is a separate, policy-independent rule: a top-level channel
//! message is answered on the main timeline, and a thread message in its
//! thread (see the `<context>` reply instructions in `queue.rs`).

use nostr::Event;
use uuid::Uuid;

use crate::queue::{routing_thread_tags, ResolvedEdit};

/// Operator policy controlling how ACP provider sessions are scoped.
///
/// Selected via `--session-policy` / `BUZZ_ACP_SESSION_POLICY`. Defaults to
/// [`Channel`](SessionPolicy::Channel) so the feature ships dark and can be
/// canaried, then flipped, then rolled back without code changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum SessionPolicy {
    /// One provider session per channel: the main timeline and every thread
    /// share a `Conversation(channel_id)` scope.
    #[default]
    Channel,
    /// Main and each thread: the channel main timeline shares one
    /// `Main(channel_id)` session and each canonical channel thread gets an
    /// isolated provider session. DMs remain conversation-scoped.
    ///
    /// Also accepts `main-and-threads` / `main_and_threads`, the spellings an
    /// earlier development build stored for this policy.
    #[value(alias = "main-and-threads", alias = "main_and_threads")]
    Thread,
}

impl SessionPolicy {
    /// Append only the configured session model to the shared base instructions.
    /// The resulting base is reused by modern and legacy ACP standing context.
    pub(crate) fn append_session_model(self, base_prompt: &str) -> String {
        let session_model = match self {
            Self::Channel => include_str!("session_model_channel.md"),
            Self::Thread => include_str!("session_model_thread.md"),
        };
        format!("{}\n\n{}", base_prompt.trim_end(), session_model.trim_end())
    }
}

impl std::fmt::Display for SessionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Channel => f.write_str("channel"),
            Self::Thread => f.write_str("thread"),
        }
    }
}

/// A hashable ACP execution and conversational-context scope.
///
/// This is the canonical key for provider sessions, queue partitions, in-flight
/// tracking, and context gathering. The channel remains the authorization and
/// collaboration boundary; the scope is the default *execution* boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SessionScope {
    /// The whole channel is one session. Used for DMs always, and for every
    /// channel event under [`SessionPolicy::Channel`].
    Conversation { channel_id: Uuid },
    /// A single canonical thread within a channel, keyed by its root event id
    /// (64-char lowercase hex).
    Thread {
        channel_id: Uuid,
        root_event_id: String,
    },
    /// The channel's main timeline (top-level messages) under
    /// [`SessionPolicy::Thread`]. Distinct from
    /// [`Conversation`](Self::Conversation): threads in the same channel keep
    /// their own [`Thread`](Self::Thread) scopes.
    Main { channel_id: Uuid },
}

impl SessionScope {
    /// The channel this scope belongs to. Always available — the channel is the
    /// authorization boundary regardless of scope variant.
    pub fn channel_id(&self) -> Uuid {
        match self {
            Self::Conversation { channel_id } => *channel_id,
            Self::Thread { channel_id, .. } => *channel_id,
            Self::Main { channel_id } => *channel_id,
        }
    }

    /// The canonical thread-root event id for a [`Thread`](Self::Thread) scope,
    /// or `None` for a conversation or main-timeline scope.
    pub fn root_event_id(&self) -> Option<&str> {
        match self {
            Self::Conversation { .. } | Self::Main { .. } => None,
            Self::Thread { root_event_id, .. } => Some(root_event_id),
        }
    }

    /// True when this scope is thread-scoped (not conversation-scoped).
    pub fn is_thread(&self) -> bool {
        matches!(self, Self::Thread { .. })
    }

    /// True when this scope is the channel main timeline.
    pub fn is_main(&self) -> bool {
        matches!(self, Self::Main { .. })
    }

    /// True when a batch for this scope waits for the worker that owns its
    /// provider session instead of opening a duplicate session on another idle
    /// worker. `Conversation` scopes keep the legacy fork-onto-idle behavior.
    pub fn holds_for_busy_owner(&self) -> bool {
        matches!(self, Self::Thread { .. } | Self::Main { .. })
    }

    /// Derive the scope for an admitted event.
    ///
    /// Resolution order:
    /// 1. DMs are always [`Conversation`](Self::Conversation) — direct
    ///    messages stay conversation-scoped regardless of policy.
    /// 2. Under [`SessionPolicy::Channel`], every channel event is
    ///    conversation-scoped (legacy / rollback behavior).
    /// 3. Under [`SessionPolicy::Thread`], a channel event with a NIP-10 root
    ///    scopes to that canonical thread, and every other channel event
    ///    (top-level messages, edits of top-level originals, unresolved edits)
    ///    scopes to the channel's [`Main`](Self::Main) session.
    ///
    /// Thread roots are resolved with [`crate::queue::parse_thread_tags`], i.e. Buzz's shared
    /// [`buzz_core::nip10`] canonical-root rules — a malformed marker id is
    /// ignored (treated as top-level), and a lone `root` marker with no `reply`
    /// is top-level, matching relay ingest.
    ///
    /// The root id is normalized to lowercase before it becomes the scope key.
    /// The shared NIP-10 parser accepts and preserves uppercase ASCII hex
    /// (`is_ascii_hexdigit`), but the relay decodes event ids to bytes on
    /// ingest, so `AB…` and `ab…` name the *same* thread. Without normalization
    /// those equivalent spellings would hash to different `Thread` keys and
    /// split one relay thread across two ACP sessions (queue state, provider
    /// sessions, affinity, delivery ledgers).
    pub fn derive(policy: SessionPolicy, channel_id: Uuid, is_dm: bool, event: &Event) -> Self {
        Self::derive_routed(policy, channel_id, is_dm, event, None)
    }

    /// [`derive`](Self::derive) for an event whose reply routing may come from
    /// an edited original message.
    ///
    /// A kind:40003 edit belongs to its original message's place, never to a
    /// new thread rooted at the auxiliary edit event: a threaded original
    /// scopes to its root, and a top-level original (or one that could not be
    /// fetched) scopes to the main timeline.
    pub fn derive_routed(
        policy: SessionPolicy,
        channel_id: Uuid,
        is_dm: bool,
        event: &Event,
        edit: Option<&ResolvedEdit>,
    ) -> Self {
        if is_dm || policy == SessionPolicy::Channel {
            return Self::Conversation { channel_id };
        }
        match routing_thread_tags(event, edit).root_event_id {
            Some(root) => Self::Thread {
                channel_id,
                root_event_id: root.to_ascii_lowercase(),
            },
            None => Self::Main { channel_id },
        }
    }

    /// A compact, log-friendly label for telemetry (e.g. `conversation` or
    /// `thread:<root8>`), never leaking full ids into high-cardinality fields.
    pub fn telemetry_label(&self) -> String {
        match self {
            Self::Conversation { .. } => "conversation".to_string(),
            Self::Main { .. } => "main".to_string(),
            Self::Thread { root_event_id, .. } => {
                let short: String = root_event_id.chars().take(8).collect();
                format!("thread:{short}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};

    /// Build a signed event with the given NIP-10 `e`/`p` tags.
    fn event_with_tags(tags: Vec<Vec<String>>) -> Event {
        let keys = Keys::generate();
        let tags: Vec<nostr::Tag> = tags
            .into_iter()
            .map(|t| nostr::Tag::parse(t).expect("valid tag"))
            .collect();
        EventBuilder::new(Kind::Custom(9), "hello")
            .tags(tags)
            .sign_with_keys(&keys)
            .unwrap()
    }

    fn plain_event() -> Event {
        event_with_tags(vec![])
    }

    fn reply_in(root: &str) -> Event {
        event_with_tags(vec![
            vec!["e".into(), root.to_string(), String::new(), "root".into()],
            vec!["e".into(), "f".repeat(64), String::new(), "reply".into()],
        ])
    }

    fn thread_root(scope: &SessionScope) -> String {
        match scope {
            SessionScope::Thread { root_event_id, .. } => root_event_id.clone(),
            other => panic!("expected thread scope, got {other:?}"),
        }
    }

    #[test]
    fn session_model_is_appended_once_and_matches_policy() {
        let base = include_str!("base_prompt.md");
        assert!(!base.contains("## Session Model"));
        for policy in [SessionPolicy::Channel, SessionPolicy::Thread] {
            let prompt = policy.append_session_model(base);
            assert!(prompt.starts_with(base.trim_end()));
            assert_eq!(prompt.matches("## Session Model").count(), 1);
            assert!(prompt.ends_with("assume the owning session has it handled."));
            assert!(prompt.contains("DMs stay one conversation"));
            assert!(prompt.contains(
                "core memory, your workspace on disk, relay access, and channel authorization"
            ));
            assert!(prompt.contains("leave execution with the owning session"));
            match policy {
                SessionPolicy::Channel => {
                    assert!(prompt.contains("one per-channel session"));
                    assert!(!prompt.contains("each thread gets its own"));
                }
                SessionPolicy::Thread => {
                    assert!(prompt.contains("main timeline (top-level messages) is one"));
                    assert!(prompt.contains("each thread gets its own"));
                    assert!(!prompt.contains("one per-channel session"));
                    assert!(!prompt.contains("new thread rooted at a top-level mention"));
                }
            }
        }
    }

    #[test]
    fn dm_is_always_conversation_scoped() {
        let ch = Uuid::new_v4();
        for policy in [SessionPolicy::Channel, SessionPolicy::Thread] {
            for ev in [plain_event(), reply_in(&"a".repeat(64))] {
                assert_eq!(
                    SessionScope::derive(policy, ch, true, &ev),
                    SessionScope::Conversation { channel_id: ch }
                );
            }
        }
    }

    #[test]
    fn channel_policy_collapses_everything_to_conversation() {
        let ch = Uuid::new_v4();
        let conv = SessionScope::Conversation { channel_id: ch };
        let reply = reply_in(&"a".repeat(64));
        assert_eq!(
            SessionScope::derive(SessionPolicy::Channel, ch, false, &reply),
            conv
        );
        assert_eq!(
            SessionScope::derive(SessionPolicy::Channel, ch, false, &plain_event()),
            conv
        );
    }

    #[test]
    fn top_level_messages_share_the_main_session() {
        let ch = Uuid::new_v4();
        let main = SessionScope::Main { channel_id: ch };
        // Two independent top-level mentions share the one main session.
        let a = SessionScope::derive(SessionPolicy::Thread, ch, false, &plain_event());
        let b = SessionScope::derive(SessionPolicy::Thread, ch, false, &plain_event());
        assert_eq!(a, main);
        assert_eq!(b, main);
    }

    #[test]
    fn lone_root_marker_and_malformed_tags_are_top_level() {
        let ch = Uuid::new_v4();
        let main = SessionScope::Main { channel_id: ch };
        // NIP-10: a lone `root` marker with no `reply` is top-level per ingest.
        let lone_root = event_with_tags(vec![vec![
            "e".into(),
            "c".repeat(64),
            String::new(),
            "root".into(),
        ]]);
        assert_eq!(
            SessionScope::derive(SessionPolicy::Thread, ch, false, &lone_root),
            main
        );
        // A non-64-hex marker id is ignored by the shared NIP-10 resolver.
        let malformed = event_with_tags(vec![vec![
            "e".into(),
            "not-a-valid-hex-id".into(),
            String::new(),
            "reply".into(),
        ]]);
        assert_eq!(
            SessionScope::derive(SessionPolicy::Thread, ch, false, &malformed),
            main
        );
    }

    #[test]
    fn nested_reply_scopes_to_canonical_root_not_parent() {
        let ch = Uuid::new_v4();
        let root = "c".repeat(64);
        let ev = event_with_tags(vec![
            vec!["e".into(), root.clone(), String::new(), "root".into()],
            vec!["e".into(), "d".repeat(64), String::new(), "reply".into()],
        ]);
        let scope = SessionScope::derive(SessionPolicy::Thread, ch, false, &ev);
        assert_eq!(thread_root(&scope), root);
    }

    #[test]
    fn repeated_replies_in_same_thread_share_scope() {
        let ch = Uuid::new_v4();
        let root = "e".repeat(64);
        let a = SessionScope::derive(SessionPolicy::Thread, ch, false, &reply_in(&root));
        let b = SessionScope::derive(SessionPolicy::Thread, ch, false, &reply_in(&root));
        assert_eq!(a, b, "same-root replies must reuse the same thread scope");
    }

    #[test]
    fn edits_follow_their_original() {
        use crate::edit_routing::test_support::{edit_event, message};
        let ch = Uuid::new_v4();
        let root = "ab".repeat(32);

        // Threaded original: the edit shares the original's thread session.
        let threaded = message(Some(&root));
        let edit = edit_event(&threaded.id.to_hex(), &[]);
        let resolved = ResolvedEdit {
            target_event_id: threaded.id.to_hex(),
            target_thread_tags: crate::queue::parse_thread_tags(&threaded),
        };
        let scope =
            SessionScope::derive_routed(SessionPolicy::Thread, ch, false, &edit, Some(&resolved));
        assert_eq!(thread_root(&scope), root);

        // Top-level or unresolved original: the main timeline, never a thread
        // rooted at the edit event.
        let top = message(None);
        let edit = edit_event(&top.id.to_hex(), &[]);
        let resolved = ResolvedEdit {
            target_event_id: top.id.to_hex(),
            target_thread_tags: crate::queue::parse_thread_tags(&top),
        };
        let main = SessionScope::Main { channel_id: ch };
        assert_eq!(
            SessionScope::derive_routed(SessionPolicy::Thread, ch, false, &edit, Some(&resolved)),
            main
        );
        let edit = edit_event(&"cd".repeat(32), &[]);
        assert_eq!(
            SessionScope::derive_routed(SessionPolicy::Thread, ch, false, &edit, None),
            main
        );
    }

    #[test]
    fn mixed_case_root_spellings_share_one_thread_scope() {
        let ch = Uuid::new_v4();
        let root_lower = "a1b2c3d4e5f6".repeat(4) + &"0".repeat(16);
        assert_eq!(root_lower.len(), 64);
        let root_upper = root_lower.to_ascii_uppercase();
        let lower = SessionScope::derive(SessionPolicy::Thread, ch, false, &reply_in(&root_lower));
        let upper = SessionScope::derive(SessionPolicy::Thread, ch, false, &reply_in(&root_upper));
        assert_eq!(lower, upper);
        assert_eq!(upper.root_event_id(), Some(root_lower.as_str()));
    }

    #[test]
    fn session_policy_parses_cli_and_legacy_dev_spellings() {
        use clap::ValueEnum;
        for spelling in ["thread", "main-and-threads", "main_and_threads"] {
            assert_eq!(
                SessionPolicy::from_str(spelling, false),
                Ok(SessionPolicy::Thread),
                "{spelling}"
            );
        }
        assert_eq!(
            SessionPolicy::from_str("channel", false),
            Ok(SessionPolicy::Channel)
        );
        assert_eq!(SessionPolicy::Thread.to_string(), "thread");
    }

    #[test]
    fn accessors_and_labels() {
        let ch = Uuid::new_v4();
        let conv = SessionScope::Conversation { channel_id: ch };
        assert_eq!(conv.channel_id(), ch);
        assert_eq!(conv.root_event_id(), None);
        assert!(!conv.is_thread());
        assert!(!conv.holds_for_busy_owner());
        assert_eq!(conv.telemetry_label(), "conversation");

        let root = "abcdef0123456789".repeat(4);
        let thread = SessionScope::Thread {
            channel_id: ch,
            root_event_id: root.clone(),
        };
        assert_eq!(thread.channel_id(), ch);
        assert_eq!(thread.root_event_id(), Some(root.as_str()));
        assert!(thread.is_thread());
        assert!(!thread.is_main());
        assert!(thread.holds_for_busy_owner());
        assert_eq!(thread.telemetry_label(), "thread:abcdef01");

        let main = SessionScope::Main { channel_id: ch };
        assert_eq!(main.channel_id(), ch);
        assert_eq!(main.root_event_id(), None);
        assert!(!main.is_thread());
        assert!(main.is_main());
        assert!(main.holds_for_busy_owner());
        assert_eq!(main.telemetry_label(), "main");
    }

    #[test]
    fn scope_is_hashable_and_usable_as_map_key() {
        use std::collections::HashMap;
        let ch = Uuid::new_v4();
        let mut map: HashMap<SessionScope, u32> = HashMap::new();
        let s1 = SessionScope::Thread {
            channel_id: ch,
            root_event_id: "a".repeat(64),
        };
        *map.entry(s1.clone()).or_insert(0) += 1;
        *map.entry(s1.clone()).or_insert(0) += 1;
        *map.entry(SessionScope::Main { channel_id: ch })
            .or_insert(0) += 1;
        *map.entry(SessionScope::Conversation { channel_id: ch })
            .or_insert(0) += 1;
        assert_eq!(map.get(&s1), Some(&2));
        assert_eq!(map.len(), 3);
    }
}
