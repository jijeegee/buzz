//! User-stream close and delivery rules (U1c, plan V4.2 section 2.4).

use std::sync::atomic::Ordering;

use axum::extract::ws::Message as WsMessage;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::*;
use super::db::*;
use super::filter;
use crate::user_stream_config::{Access, UserStreamConfig};

fn live() -> UserStreamConfig {
    UserStreamConfig {
        live: Access::On,
        ..on()
    }
}

/// Open a stream on `s` through the REQ and drain its parts and EOSE.
async fn open_stream(h: &mut Harness) {
    h.req("s", vec![request()], None).await;
    let frames = h.frames();
    assert_eq!(frames.last().expect("frames")[0], "EOSE", "{frames:?}");
}

/// Queue `n` filler frames on the data channel.
fn fill(h: &Harness, n: usize) {
    for _ in 0..n {
        assert!(h.conn.send(RelayMessage::notice("filler")));
    }
}

async fn until(what: &str, mut check: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !check().await {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

fn events(frames: &[serde_json::Value], sub: &str) -> usize {
    frames
        .iter()
        .filter(|f| f[0] == "EVENT" && f[1] == sub)
        .count()
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn shadow_over_half_full_closes_only_the_sub_after_its_frames() {
    let mut h = harness(8, |_| on()).await;
    let room = h.room(true, 0).await;
    open_stream(&mut h).await;
    for n in 0..6 {
        h.message(room, &format!("m{n}")).await;
    }
    until("the stream teardown", async || {
        !h.conn.subscriptions.lock().await.contains_key("s")
    })
    .await;
    let frames = h.frames();
    // 5 frames fit at or under 50% of 8; the 6th closes the sub, and the
    // CLOSED follows the frames already queued for it.
    assert_eq!(events(&frames, "s"), 5, "{frames:?}");
    assert_eq!(frames.len(), 6, "{frames:?}");
    assert_eq!(
        frames[5],
        serde_json::json!(["CLOSED", "s", reason::OVERFLOW])
    );
    assert!(!h.conn.cancel.is_cancelled(), "shadow keeps the socket");
    assert!(drain(&mut h.ctrl_rx).is_empty());
    assert_eq!(h.conn.backpressure_count.load(Ordering::Relaxed), 0);
    h.message(room, "after").await;
    assert!(h.frames().is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn live_on_a_full_buffer_gets_closed_then_close_and_the_terminal_slot_stays_free() {
    let mut h = harness(4, |_| live()).await;
    let room = h.room(true, 0).await;
    open_stream(&mut h).await;
    for n in 0..5 {
        h.message(room, &format!("m{n}")).await;
    }
    until("the socket cancel", async || h.conn.cancel.is_cancelled()).await;
    let frames = h.frames();
    assert_eq!(events(&frames, "s"), 4, "{frames:?}");
    assert!(closed(&frames, "s").is_empty());
    assert_eq!(closed(&drain(&mut h.ctrl_rx), "s"), [reason::OVERFLOW]);
    assert_eq!(
        h.conn.backpressure_count.load(Ordering::Relaxed),
        0,
        "no grace bump"
    );
    // FI-INV-05: a NIP-FI denial can still be enqueued afterwards.
    assert!(h
        .conn
        .terminal_ctrl_tx
        .try_send(WsMessage::Text("denied".into()))
        .is_ok());
    assert!(h.terminal_rx.try_recv().is_ok());
}

async fn stored_message(h: &Harness, room: Uuid) {
    let event = EventBuilder::new(Kind::Custom(9), "history")
        .tags([Tag::parse(["h", &room.to_string()]).unwrap()])
        .sign_with_keys(&h.keys)
        .unwrap();
    h.state
        .db
        .insert_event(h.community, &event, Some(room))
        .await
        .expect("store");
}

async fn history_req(h: &Harness, sub: &str, room: Uuid) {
    let req = filter(serde_json::json!({"kinds": [9], "#h": [room.to_string()], "limit": 5}));
    crate::handlers::req::handle_req(
        sub.into(),
        vec![req],
        vec![None],
        Arc::clone(&h.conn),
        Arc::clone(&h.state),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_burst_of_busy_refusals_does_not_cancel_the_socket() {
    let mut h = harness(100, |_| live()).await;
    let room = h.room(true, 0).await;
    stored_message(&h, room).await;
    open_stream(&mut h).await;
    fill(&h, 80);
    for n in 0..12 {
        history_req(&h, &format!("h{n}"), room).await;
    }
    let frames = h.frames();
    for n in 0..12 {
        let sub = format!("h{n}");
        assert_eq!(closed(&frames, &sub), ["error: busy"], "{sub}");
        assert_eq!(events(&frames, &sub), 0);
        assert!(!h.conn.subscriptions.lock().await.contains_key(&sub));
    }
    assert!(!h.conn.cancel.is_cancelled());
    assert!(
        drain(&mut h.ctrl_rx).is_empty(),
        "control channel untouched"
    );
    assert_eq!(h.conn.backpressure_count.load(Ordering::Relaxed), 0);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn history_headroom_applies_only_to_live_stream_connections() {
    let mut h = harness(100, |_| on()).await;
    let room = h.room(true, 0).await;
    stored_message(&h, room).await;
    open_stream(&mut h).await; // shadow
    fill(&h, 80);
    history_req(&h, "h", room).await;
    let frames = h.frames();
    assert_eq!(events(&frames, "h"), 1, "{frames:?}");
    assert!(closed(&frames, "h").is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn lagged_closes_every_stream_and_its_socket() {
    let mut h = harness(64, |_| on()).await;
    h.room(true, 0).await;
    open_stream(&mut h).await;
    assert_eq!(close_all_lagged(&h.state), 1);
    until("the socket cancel", async || h.conn.cancel.is_cancelled()).await;
    assert_eq!(closed(&drain(&mut h.ctrl_rx), "s"), [reason::LAGGED]);
    assert!(closed(&h.frames(), "s").is_empty());
    assert!(h.state.sub_registry.stream_of(h.conn.conn_id).is_none());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn fail_closed_closes_the_streams_among_the_recipients() {
    // Live: CLOSED on the control channel, then the socket closes.
    let mut h = harness(64, |_| live()).await;
    h.room(true, 0).await;
    open_stream(&mut h).await;
    let conn_id = h.conn.conn_id;
    fail_closed(
        &h.state,
        &[(conn_id, "s".to_string()), (conn_id, "x".to_string())],
    );
    until("the socket cancel", async || h.conn.cancel.is_cancelled()).await;
    assert_eq!(closed(&drain(&mut h.ctrl_rx), "s"), [reason::CLOSED]);

    // Shadow: the sub only, after its frames.
    let mut h = harness(64, |_| on()).await;
    h.room(true, 0).await;
    open_stream(&mut h).await;
    let conn_id = h.conn.conn_id;
    fail_closed(&h.state, &[(conn_id, "s".to_string())]);
    until("the stream teardown", async || {
        !h.conn.subscriptions.lock().await.contains_key("s")
    })
    .await;
    assert_eq!(closed(&h.frames(), "s"), [reason::CLOSED]);
    assert!(!h.conn.cancel.is_cancelled());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn the_eose_flush_follows_the_shadow_rule() {
    // Buffer 8 -> queue cap 2.
    let mut h = harness(8, |_| on()).await;
    let room = h.room(true, 0).await;
    let handle = snapshot_phase_stream(&h, StreamMode::Shadow, vec![room]).await;
    h.message(room, "a").await;
    h.message(room, "b").await;
    assert_eq!(handle.queued(), 2);
    fill(&h, 4);
    let mut sent = RawSend::Sent;
    assert!(!handle.go_live(|frame| {
        sent = send_frame(&h.state, h.conn.conn_id, StreamMode::Shadow, frame);
        sent == RawSend::Sent
    }));
    assert_eq!(sent, RawSend::Full);
    assert_eq!(events(&h.frames(), "s"), 1, "the first fit at 50%");
}

#[test]
fn threshold_frames_go_data_then_control_then_cancel() {
    let (tx, mut rx) = mpsc::channel::<WsMessage>(2);
    let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<WsMessage>(1);
    let cancel = CancellationToken::new();
    tx.try_send(WsMessage::Text("queued".into())).unwrap();
    assert!(crate::state::send_threshold_frame(
        &tx,
        &ctrl_tx,
        &cancel,
        "a".into()
    ));
    assert!(crate::state::send_threshold_frame(
        &tx,
        &ctrl_tx,
        &cancel,
        "b".into()
    ));
    assert!(!cancel.is_cancelled());
    assert!(!crate::state::send_threshold_frame(
        &tx,
        &ctrl_tx,
        &cancel,
        "c".into()
    ));
    assert!(cancel.is_cancelled());
    let text = |m: WsMessage| match m {
        WsMessage::Text(t) => t.to_string(),
        other => panic!("{other:?}"),
    };
    assert_eq!(text(rx.try_recv().unwrap()), "queued");
    assert_eq!(text(rx.try_recv().unwrap()), "a", "after the queued frame");
    assert_eq!(text(ctrl_rx.try_recv().unwrap()), "b");
}

#[test]
fn fill_thresholds() {
    let (tx, _rx) = mpsc::channel::<WsMessage>(10);
    for _ in 0..5 {
        tx.try_send(WsMessage::Text("x".into())).unwrap();
    }
    assert!(!crate::state::fill_exceeds(&tx, SHADOW_FILL_PERCENT, 0));
    tx.try_send(WsMessage::Text("x".into())).unwrap();
    assert!(crate::state::fill_exceeds(&tx, SHADOW_FILL_PERCENT, 0));
    tx.try_send(WsMessage::Text("x".into())).unwrap();
    assert!(
        !crate::state::fill_exceeds(&tx, HISTORY_FILL_PERCENT, 1),
        "7+1 = 80%"
    );
    tx.try_send(WsMessage::Text("x".into())).unwrap();
    assert!(crate::state::fill_exceeds(&tx, HISTORY_FILL_PERCENT, 1));
}
