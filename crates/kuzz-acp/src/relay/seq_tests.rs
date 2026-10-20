//! Channel order numbers from a `kuzz-seq` relay: parsing, cursor upkeep,
//! `after_seq` replay pages and the `KUZZ_RESYNC` signal. Synthetic sockets only.
use super::tests::{next_test_frame, seed_test_subscription, test_channel_filter, test_ws_pair};
use super::*;

fn channel_event(channel: Uuid, n: u64) -> Event {
    EventBuilder::new(Kind::Custom(9), format!("seq-{n}"))
        .tags([Tag::parse(["h", &channel.to_string()]).unwrap()])
        .custom_created_at(nostr::Timestamp::from(1_000 + n))
        .sign_with_keys(&Keys::generate())
        .unwrap()
}

fn bounds_event(channel: Uuid, after: i64, last: i64, has_more: bool) -> Event {
    EventBuilder::new(
        Kind::Custom(39006),
        json!({
            "order": "seq",
            "after_seq": after,
            "last_seq": last,
            "has_more": has_more,
            "sequences": [],
        })
        .to_string(),
    )
    .tags([
        Tag::parse(["d", &format!("{channel}:after_seq:{after}")]).unwrap(),
        Tag::parse(["h", &channel.to_string()]).unwrap(),
    ])
    .sign_with_keys(&Keys::generate())
    .unwrap()
}

async fn dispatch(
    client: &mut WsStream,
    state: &mut BgState,
    tx: &mpsc::Sender<Option<KuzzEvent>>,
    frame: Value,
) {
    let (control_tx, _control_rx) = mpsc::channel(1);
    assert!(
        handle_ws_message(
            Message::Text(frame.to_string().into()),
            client,
            tx,
            &control_tx,
            state,
            &crate::identity::AgentIdentity::from(Keys::generate()),
            "ws://127.0.0.1:1",
            "synthetic-agent",
            None,
        )
        .await
    );
}

/// A cursor whose stable number is `stable` (old enough for a resync).
fn aged_cursor(stable: i64, newest: i64) -> seq_cursor::SeqCursor {
    let long_ago = tokio::time::Instant::now() - Duration::from_secs(120);
    let mut cursor = seq_cursor::SeqCursor::default();
    cursor.observe(stable, long_ago);
    cursor.observe(newest, long_ago);
    cursor
}

async fn subscribe_req(
    client: &mut WsStream,
    server: &mut WebSocketStream<tokio::net::TcpStream>,
    state: &mut BgState,
    channel: Uuid,
    since: Option<u64>,
) -> Value {
    assert!(
        send_subscribe(
            client,
            state,
            channel,
            "synthetic-agent",
            since,
            &test_channel_filter()
        )
        .await
    );
    next_test_frame(server).await
}

#[test]
fn event_frame_carries_its_order_number_and_legacy_frames_have_none() {
    let event = serde_json::to_value(channel_event(Uuid::new_v4(), 1)).unwrap();
    let with = json!(["EVENT", "sub", event, {"channel_sequence": 42}]).to_string();
    match parse_relay_message(&with).unwrap() {
        RelayMessage::Event {
            channel_sequence, ..
        } => assert_eq!(channel_sequence, Some(42)),
        other => panic!("expected EVENT, got {other:?}"),
    }
    let legacy = json!(["EVENT", "sub", event]).to_string();
    match parse_relay_message(&legacy).unwrap() {
        RelayMessage::Event {
            channel_sequence, ..
        } => assert_eq!(channel_sequence, None),
        other => panic!("expected EVENT, got {other:?}"),
    }
    match parse_relay_message(r#"["KUZZ_RESYNC",{"reason":"redis_reconnect"}]"#).unwrap() {
        RelayMessage::Resync { reason } => assert_eq!(reason, "redis_reconnect"),
        other => panic!("expected KUZZ_RESYNC, got {other:?}"),
    }
}

#[tokio::test]
async fn delivered_numbers_feed_the_cursor_and_replay_asks_after_seq() {
    let (mut client, mut server) = test_ws_pair().await;
    let (tx, mut rx) = mpsc::channel(8);
    let mut state = BgState::new();
    let channel = Uuid::new_v4();
    seed_test_subscription(&mut state, channel);

    let event = channel_event(channel, 1);
    dispatch(
        &mut client,
        &mut state,
        &tx,
        json!(["EVENT", channel_sub_id(channel), event, {"channel_sequence": 7}]),
    )
    .await;
    assert!(rx.try_recv().unwrap().is_some(), "event is forwarded");
    assert_eq!(state.seq[&channel].newest_for_test(), Some(7));
    // Numbers alone pin nothing: an explicit or reconnect replay keeps since.
    let req = subscribe_req(&mut client, &mut server, &mut state, channel, Some(1_000)).await;
    assert_eq!(req[2]["since"], json!(1_000 - SINCE_SKEW_SECS));
    assert!(req[2].get("after_seq").is_none());

    let mut cursor = aged_cursor(7, 9);
    assert!(cursor.resync(tokio::time::Instant::now()));
    state.seq.insert(channel, cursor);
    let req = subscribe_req(&mut client, &mut server, &mut state, channel, Some(1_000)).await;
    assert_eq!(req[2]["after_seq"], json!(7));
    assert_eq!(req[2]["limit"], json!(seq_cursor::AFTER_SEQ_PAGE_LIMIT));
    assert!(
        req[2].get("since").is_none(),
        "after_seq cannot mix with since"
    );
    assert_eq!(req[2]["#h"], json!([channel.to_string()]));
    // The pin is spent once the REQ is out.
    let req = subscribe_req(&mut client, &mut server, &mut state, channel, Some(1_000)).await;
    assert!(req[2].get("after_seq").is_none());
}

#[tokio::test]
async fn first_subscribe_still_starts_now_even_with_a_cursor() {
    let (mut client, mut server) = test_ws_pair().await;
    let mut state = BgState::new();
    let channel = Uuid::new_v4();
    seed_test_subscription(&mut state, channel);
    state.seq.insert(channel, aged_cursor(3, 4));
    let req = subscribe_req(&mut client, &mut server, &mut state, channel, None).await;
    assert!(req[2].get("since").is_some());
    assert!(req[2].get("after_seq").is_none());
}

#[tokio::test]
async fn bounds_event_is_consumed_and_pages_until_done() {
    let (mut client, mut server) = test_ws_pair().await;
    let (tx, mut rx) = mpsc::channel(8);
    let mut state = BgState::new();
    let channel = Uuid::new_v4();
    seed_test_subscription(&mut state, channel);
    state
        .active_subscriptions
        .insert(channel, channel_sub_id(channel));

    dispatch(
        &mut client,
        &mut state,
        &tx,
        json!([
            "EVENT",
            channel_sub_id(channel),
            bounds_event(channel, 7, 507, true)
        ]),
    )
    .await;
    assert!(rx.try_recv().is_err(), "bounds never reach the harness");
    // The next page waits for the paced, rate-gated retry drain.
    assert!(state.resubscribe_retry.contains(&channel));
    state.resubscribe_retry.clear();
    let next = subscribe_req(&mut client, &mut server, &mut state, channel, Some(1_000)).await;
    assert_eq!(next[0], "REQ");
    assert_eq!(next[1], channel_sub_id(channel));
    assert_eq!(next[2]["after_seq"], json!(507));

    dispatch(
        &mut client,
        &mut state,
        &tx,
        json!([
            "EVENT",
            channel_sub_id(channel),
            bounds_event(channel, 507, 600, false)
        ]),
    )
    .await;
    assert!(rx.try_recv().is_err());
    assert!(
        !state.resubscribe_retry.contains(&channel),
        "a finished page asks for nothing more"
    );
    assert_eq!(state.seq[&channel].pinned(), None);
}

#[tokio::test]
async fn late_frames_after_unsubscribe_leave_no_cursor() {
    let (mut client, _server) = test_ws_pair().await;
    let (tx, _rx) = mpsc::channel(8);
    let mut state = BgState::new();
    let channel = Uuid::new_v4();
    seed_test_subscription(&mut state, channel);
    // The channel was left; a late frame still carries its sub id.
    state.active_subscriptions.remove(&channel);
    dispatch(
        &mut client,
        &mut state,
        &tx,
        json!([
            "EVENT",
            channel_sub_id(channel),
            bounds_event(channel, 7, 507, true)
        ]),
    )
    .await;
    assert!(!state.seq.contains_key(&channel));
    assert!(!state.resubscribe_retry.contains(&channel));
}

#[tokio::test]
async fn a_dropped_event_pulls_the_next_replay_back() {
    let (mut client, mut server) = test_ws_pair().await;
    // Capacity 1 and already full: the next event is dropped.
    let (tx, _rx) = mpsc::channel(1);
    tx.try_send(None).unwrap();
    let mut state = BgState::new();
    let channel = Uuid::new_v4();
    seed_test_subscription(&mut state, channel);
    state
        .active_subscriptions
        .insert(channel, channel_sub_id(channel));
    state.seq.insert(channel, aged_cursor(20, 30));

    dispatch(
        &mut client,
        &mut state,
        &tx,
        json!(["EVENT", channel_sub_id(channel), channel_event(channel, 2), {"channel_sequence": 12}]),
    )
    .await;
    let req = subscribe_req(&mut client, &mut server, &mut state, channel, Some(1_000)).await;
    assert_eq!(req[2]["after_seq"], json!(11));
    // The replay went out; the dropped range is covered.
    let req = subscribe_req(&mut client, &mut server, &mut state, channel, Some(1_000)).await;
    assert!(req[2].get("after_seq").is_none());
}

#[tokio::test]
async fn resync_queues_every_channel_and_membership_replay() {
    let (mut client, _server) = test_ws_pair().await;
    let (tx, _rx) = mpsc::channel(8);
    let mut state = BgState::new();
    let channels = [Uuid::new_v4(), Uuid::new_v4()];
    for channel in channels {
        seed_test_subscription(&mut state, channel);
        state
            .active_subscriptions
            .insert(channel, channel_sub_id(channel));
    }
    state.membership_sub_active = true;
    state.seq.insert(channels[0], aged_cursor(40, 50));

    dispatch(
        &mut client,
        &mut state,
        &tx,
        json!(["KUZZ_RESYNC", {"reason": "fanout_lagged"}]),
    )
    .await;
    for channel in channels {
        assert!(state.resubscribe_retry.contains(&channel));
    }
    assert!(state.membership_resub_needed);
    // Pinned when the signal arrives, not when the REQ goes out.
    assert_eq!(state.seq[&channels[0]].pinned(), Some(40));
    assert!(
        !state.seq.contains_key(&channels[1]),
        "no numbers: since path"
    );
}

#[tokio::test]
async fn handshake_buffer_keeps_the_order_number() {
    let (mut client, _server) = test_ws_pair().await;
    let (tx, mut rx) = mpsc::channel(8);
    let (control_tx, _control_rx) = mpsc::channel(1);
    let mut state = BgState::new();
    let channel = Uuid::new_v4();
    seed_test_subscription(&mut state, channel);
    let buffer = VecDeque::from([RelayMessage::Event {
        subscription_id: channel_sub_id(channel),
        event: Box::new(channel_event(channel, 3)),
        channel_sequence: Some(55),
    }]);
    assert!(
        process_handshake_buffer(
            &mut client,
            buffer,
            &tx,
            &control_tx,
            &mut state,
            &crate::identity::AgentIdentity::from(Keys::generate()),
            "ws://127.0.0.1:1",
            "synthetic-agent",
            None,
        )
        .await
    );
    assert!(rx.try_recv().unwrap().is_some());
    assert_eq!(state.seq[&channel].newest_for_test(), Some(55));
}
