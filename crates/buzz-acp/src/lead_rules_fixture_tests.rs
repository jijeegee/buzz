//! Desktop "Lead" channel routing runs this harness in Config mode with rules
//! the desktop generates. `tests/fixtures/lead_rules.toml` is that file,
//! byte-for-byte, for a fixed owner (the desktop renders against the same
//! fixture), so these tests pin how the harness admits events under it.

use std::path::Path;

use nostr::{EventBuilder, Keys, Kind, Tag};
use uuid::Uuid;

use crate::config::{default_mention_kinds, load_rules};
use crate::filter::{match_event, SubscriptionRule};

/// Secret key 1; its pubkey is the owner the fixture's listener rule names.
const OWNER_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000001";

fn fixture_rules() -> Vec<SubscriptionRule> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lead_rules.toml");
    load_rules(&path).expect("the generated lead rules load")
}

fn event(author: &Keys, kind: u16, content: &str, mentions: &[&Keys]) -> nostr::Event {
    let tags = mentions
        .iter()
        .map(|keys| Tag::parse(["p", keys.public_key().to_hex().as_str()]).expect("p tag"));
    EventBuilder::new(Kind::Custom(kind), content)
        .tags(tags)
        .sign_with_keys(author)
        .expect("sign")
}

struct Cast {
    owner: Keys,
    lead: Keys,
    translator: Keys,
}

impl Cast {
    fn new() -> Self {
        Self {
            owner: Keys::parse(OWNER_SECRET).expect("owner key"),
            lead: Keys::generate(),
            translator: Keys::generate(),
        }
    }

    async fn tag_for(&self, event: &nostr::Event) -> Option<String> {
        match_event(
            event,
            Uuid::new_v4(),
            &fixture_rules(),
            &self.lead.public_key().to_hex(),
        )
        .await
        .map(|matched| matched.prompt_tag)
    }
}

#[test]
fn lead_fixture_mention_rule_uses_the_harness_default_kinds() {
    let rules = fixture_rules();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].name, "mentions");
    assert!(rules[0].require_mention);
    assert_eq!(rules[0].kinds, default_mention_kinds());
    assert_eq!(rules[1].name, "lead-listen");
    assert!(!rules[1].require_mention);
    assert_eq!(rules[1].kinds, vec![9]);
}

#[tokio::test]
async fn lead_fixture_mentioned_messages_match_the_mention_rule() {
    let cast = Cast::new();
    // The owner @mentions the lead.
    let from_owner = event(&cast.owner, 9, "@Coder fix the build", &[&cast.lead]);
    assert_eq!(cast.tag_for(&from_owner).await.as_deref(), Some("@mention"));
    // Addressed by `p` tag alone (no body `@`) — both rules could match, so
    // this pins the mention rule winning by order.
    let tagged_only = event(&cast.owner, 9, "fix the build", &[&cast.lead]);
    assert_eq!(
        cast.tag_for(&tagged_only).await.as_deref(),
        Some("@mention")
    );
    // Another agent @mentions the lead (a callback or a hand-off reply).
    let from_agent = event(&cast.translator, 9, "@Coder done", &[&cast.lead]);
    assert_eq!(cast.tag_for(&from_agent).await.as_deref(), Some("@mention"));
    // Edits, approval requests, and reminders that address the lead.
    for kind in [40003, 46010, 40007] {
        let addressed = event(&cast.owner, kind, "please look", &[&cast.lead]);
        assert_eq!(
            cast.tag_for(&addressed).await.as_deref(),
            Some("@mention"),
            "kind {kind}"
        );
    }
}

#[tokio::test]
async fn lead_fixture_unmentioned_owner_messages_without_at_match_lead_listen() {
    let cast = Cast::new();
    let plain = event(&cast.owner, 9, "can someone translate this doc", &[]);
    assert_eq!(cast.tag_for(&plain).await.as_deref(), Some("lead-listen"));
}

#[tokio::test]
async fn lead_fixture_everything_else_matches_nothing() {
    let cast = Cast::new();
    let cases = [
        // The owner addresses another agent: body `@` and its `p` tag.
        (
            "owner mentions translator",
            event(
                &cast.owner,
                9,
                "@Translator translate this",
                &[&cast.translator],
            ),
        ),
        // Body `@` without any `p` tag still keeps the lead out.
        (
            "owner body @ only",
            event(&cast.owner, 9, "mail me at a@b.c", &[]),
        ),
        // A sibling agent's unmentioned message.
        (
            "sibling unmentioned",
            event(&cast.translator, 9, "translation is done", &[]),
        ),
        // The lead's own unmentioned message.
        (
            "lead unmentioned",
            event(&cast.lead, 9, "handing this off", &[]),
        ),
        // Unmentioned non-message kinds from the owner (listener is kind 9).
        (
            "owner unmentioned edit",
            event(&cast.owner, 40003, "typo fix", &[]),
        ),
    ];
    for (label, event) in cases {
        assert_eq!(cast.tag_for(&event).await, None, "{label}");
    }
}
