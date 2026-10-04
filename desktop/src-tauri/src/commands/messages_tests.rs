use super::*;

#[test]
fn search_messages_limit_allows_discussion_discovery_page() {
    assert_eq!(search_messages_limit(None), 20);
    assert_eq!(search_messages_limit(Some(500)), 500);
    assert_eq!(search_messages_limit(Some(1_000)), 500);
}

#[test]
fn marker_author_scope_validates_scope_and_required_pubkey() {
    assert_eq!(
        marker_author_for_scope(None, Some("agent")),
        Ok(Some("agent"))
    );
    assert_eq!(
        marker_author_for_scope(Some("agent"), Some("agent")),
        Ok(Some("agent"))
    );
    assert_eq!(marker_author_for_scope(Some("channel"), None), Ok(None));
    assert_eq!(
        marker_author_for_scope(Some("agent"), None),
        Err("agent pubkey is required for agent-scoped markers".to_string())
    );
    assert_eq!(
        marker_author_for_scope(None, None),
        Err("agent pubkey is required for agent-scoped markers".to_string())
    );
    assert_eq!(
        marker_author_for_scope(Some("unexpected"), Some("agent")),
        Err("unsupported marker scope: unexpected".to_string())
    );
}

#[test]
fn managed_agent_message_builder_adds_mentions_and_client_marker() {
    let pubkey = Keys::generate().public_key().to_hex();
    let event = build_managed_agent_channel_message(
        uuid::Uuid::new_v4(),
        "Welcome!",
        None,
        std::slice::from_ref(&pubkey),
        &[vec!["client".to_string(), "welcome-v1".to_string()]],
    )
    .expect("message should build")
    .sign_with_keys(&Keys::generate())
    .expect("message should sign");

    assert!(event.tags.iter().any(|tag| {
        let parts = tag.as_slice();
        parts.len() >= 2 && parts[0] == "p" && parts[1] == pubkey
    }));
    assert!(event_has_client_marker(&event, "welcome-v1"));
}

#[test]
fn managed_agent_message_builder_can_carry_multiple_client_markers() {
    let event = build_managed_agent_channel_message(
        uuid::Uuid::new_v4(),
        "Welcome!",
        None,
        &[],
        &[
            vec!["client".to_string(), "opener-v1".to_string()],
            vec!["client".to_string(), "closer-v1".to_string()],
        ],
    )
    .expect("message should build")
    .sign_with_keys(&Keys::generate())
    .expect("message should sign");

    assert!(event_has_client_marker(&event, "opener-v1"));
    assert!(event_has_client_marker(&event, "closer-v1"));
}

#[test]
fn managed_agent_message_builder_rejects_invalid_mentions() {
    let error = build_managed_agent_channel_message(
        uuid::Uuid::new_v4(),
        "Welcome!",
        None,
        &["not-a-pubkey".to_string()],
        &[],
    )
    .expect_err("invalid mentions should fail");
    assert!(error.contains("pubkey must be a 64-character hex string"));
}
#[test]
fn search_messages_filter_requests_prefix_mode_for_topbar_typeahead() {
    let filter = build_search_messages_filter("  pro  ", 12, Some("channel-1"), None, None, None);

    assert_eq!(filter["search"], serde_json::json!("pro"));
    assert_eq!(filter["search_mode"], serde_json::json!("prefix"));
    assert_eq!(filter["limit"], serde_json::json!(12));
    assert_eq!(filter["#h"], serde_json::json!(["channel-1"]));
    assert!(filter.get("authors").is_none());
    assert!(filter.get("since").is_none());
    assert!(filter.get("until").is_none());
}

#[test]
fn search_messages_filter_emits_operator_fields() {
    let authors =
        vec!["aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899".to_string()];
    let filter = build_search_messages_filter(
        "deploy",
        20,
        Some("channel-uuid"),
        Some(&authors),
        Some(1_700_000_000),
        Some(1_700_086_400),
    );

    assert_eq!(filter["search"], serde_json::json!("deploy"));
    assert_eq!(filter["#h"], serde_json::json!(["channel-uuid"]));
    assert_eq!(
        filter["authors"],
        serde_json::json!(["aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899"])
    );
    assert_eq!(filter["since"], serde_json::json!(1_700_000_000));
    assert_eq!(filter["until"], serde_json::json!(1_700_086_400));
}

#[test]
fn channel_messages_before_filter_sends_before_id_the_relay_reads() {
    // The relay bridge's `extract_before_id` reads the composite tiebreak
    // from `before_id`. If this filter sent the id under any other key (an
    // earlier cut used `n`), the relay would silently drop the tiebreak and
    // the dense-second keyset would degrade to a bare inclusive `until` —
    // re-returning the same page forever. Pin the field name here so the
    // client/relay contract can't drift without a red test (the Playwright
    // mock reimplements the keyset in JS and cannot catch this).
    let filter = build_channel_messages_before_filter("channel-1", 1_700_000_000, Some("ab"), 200);

    assert_eq!(filter["until"], serde_json::json!(1_700_000_000));
    assert_eq!(filter["before_id"], serde_json::json!("ab"));
    assert_eq!(filter["limit"], serde_json::json!(200));
    assert_eq!(filter["#h"], serde_json::json!(["channel-1"]));
    assert!(
        !filter.contains_key("n"),
        "tiebreak must be `before_id`, not the `n` alias the relay ignores"
    );
}

#[test]
fn thread_replies_filter_carries_non_p_gated_kinds_to_clear_the_gate() {
    // The relay bridge p-gates EVERY filter before routing
    // (`p_gated_filters_authorized`): a kindless filter "could match" a
    // p-gated kind, so it demands a `#p` tag we don't send -> HTTP 403,
    // before the thread-subtree query runs. The headline Lane-1 fix
    // (`useThreadReplies` closing the descendant gap) then fails on every
    // call against a real relay. So the thread filter MUST carry `kinds`,
    // and every kind MUST be non-p-gated (else the gate still fires). The
    // Playwright mock does not model p-gating, so this unit test is the
    // only guard against the client/relay auth contract drifting.
    let filter = build_thread_replies_filter("root-hex", Some("channel-1"), 64, 200, None);

    let kinds = filter
        .get("kinds")
        .and_then(|v| v.as_array())
        .expect("thread filter must carry `kinds` so the p-gate passes");
    assert!(!kinds.is_empty(), "kinds must be non-empty");
    for kind in kinds {
        let k = kind.as_u64().expect("kind is a number") as u32;
        assert!(
            !buzz_core_pkg::kind::P_GATED_KINDS.contains(&k),
            "kind {k} is p-gated; a p-gated kind in the filter re-triggers the \
                 403 that this fix exists to prevent"
        );
    }
    assert_eq!(filter["#e"], serde_json::json!(["root-hex"]));
    assert_eq!(filter["depth_limit"], serde_json::json!(64));
    assert_eq!(filter["#h"], serde_json::json!(["channel-1"]));
    assert_eq!(filter["include_aux"], serde_json::json!(true));
}

#[test]
fn thread_replies_filter_pages_with_composite_cursor() {
    // When a cursor is supplied, both the timestamp and the event-id
    // tiebreak must be emitted (`thread_cursor` + `thread_cursor_id`), else
    // paging degrades to timestamp-only and drops same-second replies.
    let cursor = crate::models::ThreadCursor {
        created_at: 1_700_000_000,
        event_id: "abcd".to_string(),
    };
    let filter = build_thread_replies_filter("root-hex", None, 64, 200, Some(&cursor));
    assert_eq!(filter["thread_cursor"], serde_json::json!(1_700_000_000));
    assert_eq!(filter["thread_cursor_id"], serde_json::json!("abcd"));
    assert!(
        !filter.contains_key("#h"),
        "no channel_id -> no #h scope in the filter"
    );
}

#[test]
fn stored_managed_agent_auth_tag_trims_blank_values() {
    assert_eq!(
        stored_managed_agent_auth_tag(Some("  [\"auth\",\"owner\",\"\",\"sig\"]  ")),
        Some("[\"auth\",\"owner\",\"\",\"sig\"]".to_string())
    );
    assert_eq!(stored_managed_agent_auth_tag(Some("   ")), None);
    assert_eq!(stored_managed_agent_auth_tag(None), None);
}

#[test]
fn legacy_managed_agent_auth_tag_verifies_for_agent_pubkey() {
    let owner_keys = Keys::generate();
    let agent_keys = Keys::generate();

    let tag = legacy_managed_agent_auth_tag(&owner_keys, &agent_keys.public_key())
        .expect("legacy auth tag should compute")
        .expect("legacy auth tag should be present");

    let owner = buzz_sdk_pkg::nip_oa::verify_auth_tag(&tag, &agent_keys.public_key())
        .expect("legacy auth tag should verify");
    assert_eq!(owner, owner_keys.public_key());
}

#[test]
fn legacy_managed_agent_auth_tag_skips_self_attestation() {
    let owner_keys = Keys::generate();

    let tag = legacy_managed_agent_auth_tag(&owner_keys, &owner_keys.public_key())
        .expect("self-attestation should be skipped");

    assert_eq!(tag, None);
}

#[test]
fn provided_thread_ref_validates_and_preserves_root_and_parent() {
    let root = "11".repeat(32);
    let parent = "22".repeat(32);
    let thread_ref = thread_ref::provided_thread_ref(&root, &parent)
        .expect("valid 64-hex event ids should be accepted");
    assert_eq!(thread_ref.root_event_id.to_hex(), root);
    assert_eq!(thread_ref.parent_event_id.to_hex(), parent);
    assert!(thread_ref::provided_thread_ref("not-hex", &parent).is_err());
}

/// `FeedItem.category` is a wire contract with the desktop frontend
/// (`desktop/src/shared/api/types.ts`). The frontend routes notification
/// sounds, titles, mute-bypass, and inbox labels off these exact strings, so
/// the serialized form must stay singular `mention` — not the plural section
/// name `mentions` used by `FeedSections` and the `--types` filter.
#[test]
fn feed_item_category_serializes_to_frontend_contract() {
    let cases = [
        (FeedItemCategory::Mention, "mention"),
        (FeedItemCategory::NeedsAction, "needs_action"),
        (FeedItemCategory::Activity, "activity"),
        (FeedItemCategory::AgentActivity, "agent_activity"),
    ];
    for (category, expected) in cases {
        let value = serde_json::to_value(category).expect("category should serialize");
        assert_eq!(value, serde_json::Value::String(expected.to_string()));
    }
}

#[test]
fn feed_item_from_event_carries_singular_mention_category() {
    let pubkey = Keys::generate().public_key().to_hex();
    let event = build_managed_agent_channel_message(
        uuid::Uuid::new_v4(),
        "hey @you",
        None,
        std::slice::from_ref(&pubkey),
        &[],
    )
    .expect("message should build")
    .sign_with_keys(&Keys::generate())
    .expect("message should sign");

    let item = feed_item_from_event(&event, FeedItemCategory::Mention);
    let json = serde_json::to_value(&item).expect("feed item should serialize");

    assert_eq!(json["category"], "mention");
    assert_eq!(json["id"], event.id.to_hex());
}

fn poster_test_record(keys: &Keys) -> ManagedAgentRecord {
    let mut record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": keys.public_key().to_hex(),
        "name": "Fizz",
        "private_key_nsec": nostr::ToBech32::to_bech32(keys.secret_key()).expect("nsec"),
        "relay_url": "",
        "acp_command": "buzz-acp",
        "agent_command": "goose",
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 320,
        "system_prompt": null,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "last_started_at": null,
        "last_stopped_at": null,
        "last_exit_code": null,
        "last_error": null
    }))
    .expect("record");
    record.bot_origin = None;
    record
}

fn fallback(content: &str) -> UserFallbackMessage {
    UserFallbackMessage {
        content: content.into(),
        additional_markers: vec!["closer".into()],
    }
}

#[test]
fn token_mode_posts_the_user_fallback_instead_of_dropping_the_message() {
    let keys = Keys::generate();
    let record = poster_test_record(&keys);
    match managed_agent_poster(&record, true, Some(fallback("@Fizz hi"))) {
        Ok(ManagedAgentPoster::User(message)) => {
            assert_eq!(message.content, "@Fizz hi");
            assert_eq!(message.additional_markers, vec!["closer".to_string()]);
        }
        other => panic!("token mode must post as the user, got {other:?}"),
    }
    // Without a fallback the refusal still surfaces as an error.
    let error = managed_agent_poster(&record, true, None).expect_err("refused");
    assert!(error.contains("Google"), "{error}");
    // A blank fallback is no fallback.
    assert!(managed_agent_poster(&record, true, Some(fallback("  "))).is_err());
}

#[test]
fn bot_record_posts_the_user_fallback_even_when_signed_out() {
    let keys = Keys::generate();
    let mut record = poster_test_record(&keys);
    crate::auth::bots::adopt_record_identity(
        &mut record,
        &Keys::generate().public_key().to_hex(),
        "http://o.test",
    );
    assert!(matches!(
        managed_agent_poster(&record, false, Some(fallback("@Fizz hi"))),
        Ok(ManagedAgentPoster::User(_))
    ));
}

#[test]
fn key_mode_signs_as_the_agent_and_ignores_the_fallback() {
    let keys = Keys::generate();
    let record = poster_test_record(&keys);
    match managed_agent_poster(&record, false, Some(fallback("@Fizz hi"))) {
        Ok(ManagedAgentPoster::Agent(agent_keys)) => {
            assert_eq!(agent_keys.public_key(), keys.public_key());
        }
        other => panic!("key mode must sign as the agent, got {other:?}"),
    }
    // A broken key in key mode is an error, never a silent user post.
    let mut broken = poster_test_record(&keys);
    broken.private_key_nsec = "not-a-key".into();
    assert!(managed_agent_poster(&broken, false, Some(fallback("@Fizz hi"))).is_err());
}

#[test]
fn user_fallback_mentions_put_the_agent_first_without_duplicates() {
    let agent = "a".repeat(64);
    let teammate = "b".repeat(64);
    assert_eq!(
        user_fallback_mentions(
            &agent,
            vec![agent.to_uppercase(), teammate.clone(), teammate.clone()]
        ),
        vec![agent.clone(), teammate]
    );
    assert_eq!(user_fallback_mentions(&agent, vec![]), vec![agent]);
}

#[test]
fn client_marker_tags_skip_blank_markers() {
    assert_eq!(
        client_marker_tags(Some("opener"), vec![" ".into(), "closer".into()]),
        vec![
            vec!["client".to_string(), "opener".to_string()],
            vec!["client".to_string(), "closer".to_string()],
        ]
    );
}

#[test]
fn key_mode_agent_post_does_not_need_the_user_credential() {
    // Recovery mode: the user's key is unavailable, the agent's is not.
    let keys = Keys::generate();
    let record = poster_test_record(&keys);
    let result = resolve_managed_agent_poster(
        &record,
        &crate::auth::CredentialMode::Keys,
        Some(fallback("@Fizz hi")),
        || Err("user key unavailable (recovery mode)".to_string()),
    );
    match result {
        Ok((ManagedAgentPoster::Agent(signer), None)) => {
            assert_eq!(signer.public_key(), keys.public_key());
        }
        Ok(_) => panic!("expected the agent to post without a user credential"),
        Err(error) => panic!("agent post failed: {error}"),
    }
}

#[test]
fn a_blocked_token_community_never_signs_as_the_agent() {
    let keys = Keys::generate();
    let record = poster_test_record(&keys);
    let error = resolve_managed_agent_poster(
        &record,
        &crate::auth::CredentialMode::Blocked("restoring".into()),
        Some(fallback("@Fizz hi")),
        || Err("restoring".to_string()),
    )
    .map(|_| ())
    .expect_err("blocked token community must not post");
    assert_eq!(error, "restoring");
}
