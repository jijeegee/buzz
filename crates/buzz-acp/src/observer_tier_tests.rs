//! Tier pacing, coalescing and folding of the relay observer publisher.

use super::*;
use nostr::Keys;
use observer_policy::{ObserverPolicy, ObserverTierLinks};
use serde_json::{json, Value};
use tokio::time::Instant;

fn event(seq: u64, kind: &str, payload: Value) -> observer::ObserverEvent {
    observer::ObserverEvent {
        seq,
        timestamp: format!("2026-10-10T00:00:{:02}Z", seq % 60),
        kind: kind.to_string(),
        agent_index: Some(0),
        channel_id: Some("chan".to_string()),
        session_id: Some("session".to_string()),
        turn_id: Some("turn".to_string()),
        started_at: None,
        detail: None,
        payload,
    }
}

fn tool(
    seq: u64,
    id: &str,
    start: bool,
    status: &str,
    input_bytes: usize,
) -> observer::ObserverEvent {
    let kind = if start {
        "tool_call"
    } else {
        "tool_call_update"
    };
    event(
        seq,
        "acp_read",
        json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": "session", "update": {
                "sessionUpdate": kind, "toolCallId": id, "status": status,
                "title": format!("Bash {id}"),
                "rawInput": { "command": "x".repeat(input_bytes) },
            }},
        }),
    )
}

/// Drain every frame the pacer will release at `now`, then the inner events.
fn frame_events(frame: &observer::ObserverEvent) -> Vec<Value> {
    let value = serde_json::to_value(frame).unwrap();
    match value["payload"]["events"].as_array() {
        Some(inner) => inner.clone(),
        None => vec![value],
    }
}

fn gate(pacer: &mut ObserverPacer, message: &str) {
    assert!(pacer.on_refusal(message, Instant::now()));
}

#[tokio::test(start_paused = true)]
async fn gated_events_coalesce_into_the_next_allowed_frame() {
    let mut pacer = ObserverPacer::new(ObserverPolicy::FREE, Default::default());
    gate(
        &mut pacer,
        "rate-limited: observer agent frames per minute exceeded (tier free); retry in 3s",
    );
    pacer.ingest(event(1, "turn_started", json!({})));
    for seq in 2..8 {
        pacer.ingest(tool(seq, &format!("t{seq}"), true, "in_progress", 10));
    }
    let Err(retry_at) = pacer.next_frame(Instant::now()) else {
        panic!("the observer gate defers the frame");
    };
    assert_eq!(retry_at, Instant::now() + Duration::from_secs(3));

    tokio::time::advance(Duration::from_secs(3)).await;
    let frame = pacer.next_frame(Instant::now()).unwrap().unwrap();
    let events = frame_events(&frame);
    assert_eq!(events.len(), 7, "nothing dropped, everything in one frame");
    assert_eq!(events[0]["kind"], "turn_started");
    assert!(serialized_len(&frame) <= 4_096);
    assert!(pacer.queue.is_empty());
    assert!(!pacer.limited, "an empty queue ends the limited state");
}

#[tokio::test(start_paused = true)]
async fn limited_backlog_folds_oldest_completed_pairs_and_keeps_running_tools() {
    let mut pacer = ObserverPacer::new(ObserverPolicy::FREE, Default::default());
    gate(
        &mut pacer,
        "rate-limited: observer x exceeded (tier free); retry in 1s",
    );
    pacer.ingest(event(1, "turn_started", json!({})));
    let mut seq = 2;
    // 30 completed pairs (~300 B each summarised) cannot fit one 4 KB frame.
    for i in 0..30 {
        let id = format!("done{i}");
        pacer.ingest(tool(seq, &id, true, "pending", 500));
        pacer.ingest(tool(seq + 1, &id, false, "completed", 0));
        seq += 2;
    }
    pacer.ingest(tool(seq, "running", true, "in_progress", 500));
    pacer.ingest(event(seq + 1, "turn_completed", json!({})));

    tokio::time::advance(Duration::from_secs(1)).await;
    let frame = pacer.next_frame(Instant::now()).unwrap().unwrap();
    assert!(serialized_len(&frame) <= 4_096);
    let events = frame_events(&frame);
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds.first(), Some(&"turn_started"), "turn boundary kept");
    assert_eq!(kinds.last(), Some(&"turn_completed"), "turn boundary kept");
    let gap = events
        .iter()
        .find(|e| e["kind"] == "observer_gap")
        .expect("oldest pairs fold into a gap");
    assert_eq!(gap["detail"], "free");
    assert_eq!(gap["turnId"], "turn");
    assert_eq!(gap["channelId"], "chan");
    assert_eq!(
        gap["payload"]["folded_events"].as_u64().unwrap(),
        2 * gap["payload"]["folded_tools"].as_u64().unwrap()
    );
    assert_eq!(gap["payload"]["from_seq"], 2, "the OLDEST pairs fold");
    assert!(events
        .iter()
        .any(|e| { e["payload"]["params"]["update"]["toolCallId"] == "running" }));
    // Newest pairs that fit stay as real lines.
    let newest_pair = format!("done{}", 29);
    assert!(events
        .iter()
        .any(|e| { e["payload"]["params"]["update"]["toolCallId"] == newest_pair.as_str() }));
    assert!(pacer.queue.is_empty(), "the folded backlog fits one frame");
    assert_eq!(pacer.queue.dropped_events, 0);
}

#[tokio::test(start_paused = true)]
async fn completion_of_an_already_sent_start_is_never_folded() {
    let mut pacer = ObserverPacer::new(ObserverPolicy::FREE, Default::default());
    pacer.ingest(tool(1, "early", true, "in_progress", 10));
    let frame = pacer.next_frame(Instant::now()).unwrap().unwrap();
    assert_eq!(frame_events(&frame).len(), 1, "start sent on its own");

    gate(
        &mut pacer,
        "rate-limited: observer x exceeded (tier free); retry in 1s",
    );
    let mut seq = 2;
    for i in 0..30 {
        let id = format!("done{i}");
        pacer.ingest(tool(seq, &id, true, "pending", 500));
        pacer.ingest(tool(seq + 1, &id, false, "completed", 0));
        seq += 2;
    }
    pacer.ingest(tool(seq, "early", false, "completed", 0));
    tokio::time::advance(Duration::from_secs(2)).await;
    let mut seen_early_completion = false;
    let mut now = Instant::now();
    while !pacer.queue.is_empty() {
        match pacer.next_frame(now) {
            Ok(Some(frame)) => {
                seen_early_completion |= frame_events(&frame).iter().any(|e| {
                    e["payload"]["params"]["update"]["toolCallId"] == "early"
                        && e["payload"]["params"]["update"]["status"] == "completed"
                });
            }
            Ok(None) => break,
            Err(at) => now = at,
        }
    }
    assert!(
        seen_early_completion,
        "its start was shown, so its end must be"
    );
}

#[tokio::test(start_paused = true)]
async fn relay_share_narrows_the_minute_budget_until_policy_changes() {
    let mut pacer = ObserverPacer::new(ObserverPolicy::PREMIUM, Default::default());
    gate(
        &mut pacer,
        "rate-limited: observer agent frames per minute exceeded (tier premium); retry in 1s; \
         share frames_per_min=2 bytes_per_min=1000000",
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    let mut now = Instant::now();
    for seq in 0..2 {
        pacer.ingest(event(seq, "turn_liveness", json!({})));
        assert!(pacer.next_frame(now).unwrap().is_some());
        now += Duration::from_secs(1);
    }
    pacer.ingest(event(9, "turn_liveness", json!({})));
    let Err(retry_at) = pacer.next_frame(now) else {
        panic!("the adopted share allows 2 frames per minute");
    };
    assert!(retry_at > now + Duration::from_secs(50));

    pacer.set_policy(ObserverPolicy::STANDARD);
    assert!(
        pacer.next_frame(now).unwrap().is_some(),
        "a new policy drops the old share"
    );
}

#[tokio::test(start_paused = true)]
async fn own_budget_caps_frames_and_bytes_per_minute() {
    let mut pacer = ObserverPacer::new(ObserverPolicy::FREE, Default::default());
    let start = Instant::now();
    let mut now = start;
    let mut sent = 0usize;
    let mut bytes = 0usize;
    // Far more than a minute's budget of ~3.5 KB frames.
    for seq in 0..200 {
        pacer.ingest(tool(seq, &format!("t{seq}"), true, "in_progress", 10));
        pacer.ingest(event(
            seq,
            "context_usage",
            json!({ "pad": "p".repeat(3_000) }),
        ));
    }
    while now < start + Duration::from_secs(60) {
        match pacer.next_frame(now) {
            Ok(Some(frame)) => {
                sent += 1;
                bytes += serialized_len(&frame);
                now += Duration::from_millis(1);
            }
            Ok(None) => break,
            Err(at) => now = at,
        }
    }
    assert!(sent <= 60, "frames per minute: {sent}");
    assert!(bytes <= 100_000, "bytes per minute: {bytes}");
    assert!(
        bytes > 90_000,
        "the byte budget is used, not wasted: {bytes}"
    );
}

#[test]
fn premium_backlog_over_budget_folds_before_dropping() {
    let mut queue = ObserverPublishQueue::default();
    let mut seq = 0;
    for i in 0..90 {
        let id = format!("big{i}");
        queue.ingest(tool(seq, &id, true, "pending", 50_000));
        queue.ingest(tool(seq + 1, &id, false, "completed", 0));
        seq += 2;
    }
    assert!(queue.total_pending_bytes() <= OBSERVER_PENDING_QUEUE_MAX_BYTES);
    assert_eq!(
        queue.dropped_events, 0,
        "folding made room, nothing dropped"
    );
    let gap = queue
        .events
        .iter()
        .find(|(_, _, event)| event.kind == observer_summary::OBSERVER_GAP_KIND)
        .expect("oldest pairs folded");
    assert!(gap.2.detail.is_none(), "premium gaps carry no detail");
    assert_eq!(gap.1, gap.2.payload["folded_events"].as_u64().unwrap());
}

/// Publisher links whose policy fetch never answers.
fn silent_links(
    persisted: Option<ObserverPolicy>,
) -> (
    ObserverTierLinks,
    tokio::sync::watch::Sender<Option<ObserverPolicy>>,
) {
    let (policy_tx, policy_rx) = tokio::sync::watch::channel(None);
    let (links, _feedback) = ObserverTierLinks::fixed(ObserverPolicy::PREMIUM);
    (
        ObserverTierLinks {
            policy: policy_rx,
            persisted,
            ..links
        },
        policy_tx,
    )
}

/// Publish one oversized tool call through the real publisher loop and
/// return the decrypted frame.
async fn publish_one(links: ObserverTierLinks, wait: Duration) -> Value {
    let observer = observer::ObserverHandle::in_process();
    let agent_keys = Keys::generate();
    let owner_keys = Keys::generate();
    let (publisher, mut published_rx) = RelayEventPublisher::test_pair();
    let tool_call = tool(0, "call", true, "pending", 20_000);
    observer.emit(
        "acp_read",
        Some(0),
        &observer::context_for(Some(uuid::Uuid::new_v4()), None, Some("turn".into())),
        tool_call.payload,
    );
    let rx = observer.subscribe();
    let snapshot = observer.snapshot();
    let task = tokio::spawn(run_relay_observer_publisher(
        snapshot,
        rx,
        publisher,
        agent_keys.clone(),
        agent_keys.public_key().to_hex(),
        owner_keys.public_key().to_hex(),
        owner_keys.public_key(),
        links,
    ));
    tokio::time::sleep(wait).await;
    let frame = published_rx.try_recv().expect("one frame published");
    task.abort();
    decrypt_observer_payload(&owner_keys, &frame).expect("decrypt frame")
}

#[tokio::test(start_paused = true)]
async fn publisher_uses_the_fetched_policy() {
    let (links, _feedback) = ObserverTierLinks::fixed(ObserverPolicy::STANDARD);
    let frame = publish_one(links, Duration::from_millis(1_100)).await;
    assert_eq!(frame["detail"], "standard");
}

#[tokio::test(start_paused = true)]
async fn publisher_falls_back_to_the_persisted_policy_after_two_seconds() {
    let (links, _policy_tx) = silent_links(Some(ObserverPolicy::PREMIUM));
    let frame = publish_one(links, Duration::from_millis(3_100)).await;
    assert!(
        frame.get("detail").is_none(),
        "persisted premium: full detail"
    );
    let command = &frame["payload"]["params"]["update"]["rawInput"]["command"];
    assert_eq!(command.as_str().unwrap().len(), 20_000);
}

#[tokio::test(start_paused = true)]
async fn publisher_falls_back_to_free_without_any_policy() {
    let (links, _policy_tx) = silent_links(None);
    let frame = publish_one(links, Duration::from_millis(3_600)).await;
    assert_eq!(frame["detail"], "free");
    let preview = &frame["payload"]["params"]["update"]["rawInput"]["preview"];
    assert_eq!(preview.as_str().unwrap().chars().count(), 41);
    assert!(serde_json::to_string(&frame).unwrap().len() < 4_096);
}

#[tokio::test(start_paused = true)]
async fn publisher_backlog_drains_at_most_five_frames_per_second() {
    let observer = observer::ObserverHandle::in_process();
    let agent_keys = Keys::generate();
    let owner_keys = Keys::generate();
    let (publisher, mut published_rx) = RelayEventPublisher::test_pair();
    // One channel per event: a frame never mixes channels, so 20 frames.
    for _ in 0..20 {
        observer.emit(
            "turn_liveness",
            None,
            &observer::context_for(Some(uuid::Uuid::new_v4()), None, None),
            json!({}),
        );
    }
    let rx = observer.subscribe();
    let snapshot = observer.snapshot();
    let task = tokio::spawn(run_relay_observer_publisher(
        snapshot,
        rx,
        publisher,
        agent_keys.clone(),
        agent_keys.public_key().to_hex(),
        owner_keys.public_key().to_hex(),
        owner_keys.public_key(),
        premium_tiers(),
    ));
    let mut per_second = Vec::new();
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let mut count = 0;
        while published_rx.try_recv().is_ok() {
            count += 1;
        }
        per_second.push(count);
    }
    task.abort();
    assert!(per_second.iter().all(|count| *count <= 5), "{per_second:?}");
    assert!(
        per_second[1..].iter().all(|count| *count >= 4),
        "backlog drains at the burst rate: {per_second:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn relay_refusal_pauses_the_publisher_and_requests_a_policy_refresh() {
    let observer = observer::ObserverHandle::in_process();
    let agent_keys = Keys::generate();
    let owner_keys = Keys::generate();
    let (publisher, mut published_rx) = RelayEventPublisher::test_pair();
    let (links, feedback_tx) = ObserverTierLinks::fixed(ObserverPolicy::PREMIUM);
    let refresh = Arc::clone(&links.refresh);
    let rx = observer.subscribe();
    let task = tokio::spawn(run_relay_observer_publisher(
        Vec::new(),
        rx,
        publisher,
        agent_keys.clone(),
        agent_keys.public_key().to_hex(),
        owner_keys.public_key().to_hex(),
        owner_keys.public_key(),
        links,
    ));
    tokio::time::sleep(Duration::from_millis(10)).await;
    feedback_tx.send_modify(|feedback| {
        feedback.seq += 1;
        feedback.message = "rate-limited: observer x exceeded (tier premium); retry in 4s".into();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    tokio::time::timeout(Duration::from_millis(1), refresh.notified())
        .await
        .expect("a refusal asks for a policy refresh");

    observer.emit(
        "turn_liveness",
        None,
        &observer::context_for(None, None, None),
        json!({}),
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(published_rx.try_recv().is_err(), "held while gated");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    assert!(published_rx.try_recv().is_ok(), "sent once the gate lifts");
    task.abort();
}

// ── Watching window ─────────────────────────────────────────────────────────

/// Every inner event the pacer releases for everything queued.
fn drain(pacer: &mut ObserverPacer) -> Vec<Value> {
    let mut events = Vec::new();
    let mut now = Instant::now();
    loop {
        match pacer.next_frame(now) {
            Ok(Some(frame)) => events.extend(frame_events(&frame)),
            Ok(None) => return events,
            Err(at) => now = at,
        }
    }
}

fn command_len(event: &Value) -> Option<usize> {
    event["payload"]["params"]["update"]["rawInput"]["command"]
        .as_str()
        .map(str::len)
}

fn on_channel(mut event: observer::ObserverEvent, channel: &str) -> observer::ObserverEvent {
    event.channel_id = Some(channel.to_string());
    event
}

#[tokio::test(start_paused = true)]
async fn unwatched_premium_sends_free_summaries_marked_free() {
    let mut pacer = ObserverPacer::new(ObserverPolicy::PREMIUM, Default::default());
    pacer.ingest(event(
        1,
        "turn_started",
        json!({ "prompt": "p".repeat(500) }),
    ));
    pacer.ingest(tool(2, "a", true, "pending", 20_000));
    pacer.ingest(event(
        3,
        "turn_completed",
        json!({ "stopReason": "end_turn" }),
    ));
    let events = drain(&mut pacer);
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[1]["detail"], "free",
        "premium owner, nobody watching"
    );
    assert_eq!(
        command_len(&events[1]),
        None,
        "arguments reduced to a preview"
    );
    let preview = &events[1]["payload"]["params"]["update"]["rawInput"]["preview"];
    assert_eq!(preview.as_str().unwrap().chars().count(), 41);
    for lifecycle in [&events[0], &events[2]] {
        assert!(lifecycle.get("detail").is_none(), "lifecycle unchanged");
    }
    assert_eq!(events[0]["payload"]["prompt"].as_str().unwrap().len(), 500);
}

#[tokio::test(start_paused = true)]
async fn watching_window_switches_detail_and_expires_after_a_minute() {
    let watch = observer_watch::ObserverWatch::default();
    let mut pacer = ObserverPacer::new(ObserverPolicy::PREMIUM, watch.clone());
    let control = json!({ "type": "watching", "channelId": "chan" });
    assert!(observer_watch::apply_watching_control(
        &control,
        &watch,
        Instant::now()
    ));

    pacer.ingest(event(1, "turn_started", json!({})));
    pacer.ingest(tool(2, "a", true, "pending", 20_000));
    let events = drain(&mut pacer);
    assert!(events[1].get("detail").is_none(), "watched premium: full");
    assert_eq!(command_len(&events[1]), Some(20_000));

    tokio::time::advance(Duration::from_secs(59)).await;
    pacer.ingest(tool(3, "b", true, "pending", 20_000));
    assert_eq!(command_len(&drain(&mut pacer)[0]), Some(20_000));

    tokio::time::advance(Duration::from_secs(1)).await;
    pacer.ingest(tool(4, "c", true, "pending", 20_000));
    pacer.ingest(event(5, "turn_completed", json!({})));
    let events = drain(&mut pacer);
    assert_eq!(events[0]["detail"], "free", "window expired after 60 s");
    assert!(events[1].get("detail").is_none(), "lifecycle unchanged");

    observer_watch::apply_watching_control(&control, &watch, Instant::now());
    pacer.ingest(tool(6, "d", true, "pending", 20_000));
    let events = drain(&mut pacer);
    assert!(events[0].get("detail").is_none(), "re-opened: full again");
    assert_eq!(command_len(&events[0]), Some(20_000));
}

#[tokio::test(start_paused = true)]
async fn watched_standard_keeps_standard_and_unwatched_drops_to_free() {
    let watch = observer_watch::ObserverWatch::default();
    let mut pacer = ObserverPacer::new(ObserverPolicy::STANDARD, watch.clone());
    pacer.ingest(tool(1, "a", true, "pending", 1_000));
    assert_eq!(drain(&mut pacer)[0]["detail"], "free");
    watch.open(Instant::now());
    pacer.ingest(tool(2, "b", true, "pending", 1_000));
    assert_eq!(drain(&mut pacer)[0]["detail"], "standard");
}

#[tokio::test(start_paused = true)]
async fn events_queued_before_the_window_opens_stay_summarised() {
    let watch = observer_watch::ObserverWatch::default();
    let mut pacer = ObserverPacer::new(ObserverPolicy::PREMIUM, watch.clone());
    pacer.ingest(tool(1, "a", true, "pending", 20_000));
    watch.open(Instant::now());
    pacer.ingest(tool(2, "b", true, "pending", 20_000));
    let events = drain(&mut pacer);
    assert_eq!(events[0]["detail"], "free");
    assert!(events[1].get("detail").is_none());
}

#[tokio::test(start_paused = true)]
async fn the_window_is_agent_wide_across_channels() {
    let watch = observer_watch::ObserverWatch::default();
    let mut pacer = ObserverPacer::new(ObserverPolicy::PREMIUM, watch.clone());
    // The app watches channel "viewed"; work in another channel is full too.
    observer_watch::apply_watching_control(
        &json!({ "type": "watching", "channelId": "viewed" }),
        &watch,
        Instant::now(),
    );
    pacer.ingest(on_channel(tool(1, "a", true, "pending", 20_000), "viewed"));
    pacer.ingest(on_channel(tool(2, "b", true, "pending", 20_000), "other"));
    let events = drain(&mut pacer);
    assert_eq!(events.len(), 2);
    assert!(events
        .iter()
        .all(|event| command_len(event) == Some(20_000)));
}

#[tokio::test(start_paused = true)]
async fn a_gap_folding_unwatched_summaries_is_marked_free_on_premium() {
    let mut pacer = ObserverPacer::new(ObserverPolicy::PREMIUM, Default::default());
    // Fill the queue with unwatched (free) completed pairs, then force a fold.
    for i in 0..40u64 {
        let id = format!("t{i}");
        pacer.ingest(tool(i * 2, &id, true, "pending", 10));
        pacer.ingest(tool(i * 2 + 1, &id, false, "completed", 10));
    }
    let entries: Vec<usize> = (0..pacer.queue.events.len()).collect();
    assert!(pacer.queue.fold_oldest(&entries, usize::MAX));
    let gap = &pacer.queue.events[0].2;
    assert_eq!(gap.kind, observer_summary::OBSERVER_GAP_KIND);
    assert_eq!(gap.detail.as_deref(), Some("free"));

    let full = tool(3, "b", true, "pending", 10);
    assert_eq!(observer_summary::gap_detail(&[(1, &full)], None), None);
    assert_eq!(
        observer_summary::gap_detail(&[(1, &full)], Some("standard")),
        Some("standard")
    );
}

/// The real publisher loop running with `watch`, and its decrypted frames.
struct WatchedPublisher {
    observer: observer::ObserverHandle,
    owner_keys: Keys,
    published_rx: tokio::sync::mpsc::Receiver<nostr::Event>,
    task: tokio::task::JoinHandle<()>,
}

impl WatchedPublisher {
    fn start(policy: ObserverPolicy, watch: observer_watch::ObserverWatch) -> Self {
        let observer = observer::ObserverHandle::in_process();
        let agent_keys = Keys::generate();
        let owner_keys = Keys::generate();
        let (publisher, published_rx) = RelayEventPublisher::test_pair();
        let (links, _feedback) = ObserverTierLinks::fixed(policy);
        let links = ObserverTierLinks { watch, ..links };
        let rx = observer.subscribe();
        let task = tokio::spawn(run_relay_observer_publisher(
            observer.snapshot(),
            rx,
            publisher,
            agent_keys.clone(),
            agent_keys.public_key().to_hex(),
            owner_keys.public_key().to_hex(),
            owner_keys.public_key(),
            links,
        ));
        Self {
            observer,
            owner_keys,
            published_rx,
            task,
        }
    }

    fn emit_tool(&self) {
        let payload = tool(0, "call", true, "pending", 20_000).payload;
        let context = observer::context_for(None, None, Some("turn".into()));
        self.observer.emit("acp_read", Some(0), &context, payload);
    }

    async fn next(&mut self) -> Value {
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let frame = self.published_rx.try_recv().expect("one frame published");
        decrypt_observer_payload(&self.owner_keys, &frame).expect("decrypt frame")
    }
}

#[tokio::test(start_paused = true)]
async fn publisher_follows_the_shared_watch_and_a_restart_starts_closed() {
    let watch = observer_watch::ObserverWatch::default();
    let mut publisher = WatchedPublisher::start(ObserverPolicy::PREMIUM, watch.clone());
    tokio::time::sleep(Duration::from_millis(10)).await;
    publisher.emit_tool();
    assert_eq!(publisher.next().await["detail"], "free");

    // The control handler and the publisher share one window.
    observer_watch::apply_watching_control(
        &json!({ "type": "watching", "channelId": "c" }),
        &watch,
        Instant::now(),
    );
    publisher.emit_tool();
    let frame = publisher.next().await;
    assert!(frame.get("detail").is_none());
    assert_eq!(command_len(&frame), Some(20_000));
    publisher.task.abort();

    // A restarted executor has a fresh window: closed.
    let mut restarted = WatchedPublisher::start(ObserverPolicy::PREMIUM, Default::default());
    tokio::time::sleep(Duration::from_millis(10)).await;
    restarted.emit_tool();
    assert_eq!(restarted.next().await["detail"], "free");
    restarted.task.abort();
}
