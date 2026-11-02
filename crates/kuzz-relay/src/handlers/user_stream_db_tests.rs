//! The stream REQ end to end on one connection, against Postgres.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::AtomicU8;
use std::sync::Mutex as StdMutex;

use axum::extract::ws::Message as WsMessage;
use kuzz_auth::{AuthContext, AuthMethod};
use kuzz_core::tenant::TenantContext;
use kuzz_core::{CommunityId, StoredEvent};
use nostr::Keys;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::*;
use super::{filter, fixture};
use crate::connection::AuthState;
use crate::user_stream_config::{Access, UserStreamConfig};

pub(super) struct Harness {
    pub(super) state: Arc<AppState>,
    pub(super) conn: Arc<ConnectionState>,
    pub(super) rx: mpsc::Receiver<WsMessage>,
    pub(super) ctrl_rx: mpsc::Receiver<WsMessage>,
    pub(super) terminal_rx: mpsc::Receiver<WsMessage>,
    pub(super) pool: sqlx::PgPool,
    pub(super) community: CommunityId,
    pub(super) keys: Keys,
}

fn database_url() -> String {
    std::env::var("DATABASE_URL").expect("DATABASE_URL")
}

pub(super) fn on() -> UserStreamConfig {
    UserStreamConfig {
        stream: Access::On,
        epoch: "e1".to_string(),
        ..UserStreamConfig::default()
    }
}

fn only(pubkey: &str) -> Access {
    Access::Allowlist(BTreeSet::from([pubkey.to_string()]))
}

fn auth_context(keys: &Keys) -> AuthContext {
    AuthContext {
        pubkey: keys.public_key(),
        scopes: Vec::new(),
        channel_ids: None,
        auth_method: AuthMethod::Nip42,
        agent_owner_pubkey: None,
        token: None,
    }
}

/// A registered, authenticated connection with a `buffer`-frame send queue in
/// a fresh community. `configure` gets the account's hex pubkey.
pub(super) async fn harness(
    buffer: usize,
    configure: impl FnOnce(&str) -> UserStreamConfig,
) -> Harness {
    let pool = sqlx::PgPool::connect(&database_url())
        .await
        .expect("connect to test DB");
    let community_id = Uuid::new_v4();
    let host = format!("us-{}.example", community_id.simple());
    sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
        .bind(community_id)
        .bind(&host)
        .execute(&pool)
        .await
        .expect("insert community");
    let community = CommunityId::from_uuid(community_id);
    let keys = Keys::generate();

    let mut state = crate::state::tests::test_state_with_database_pool(pool.clone()).await;
    {
        let state = Arc::get_mut(&mut state).expect("sole reference");
        let mut config = (*state.config).clone();
        config.user_stream = configure(&keys.public_key().to_hex());
        config.send_buffer_size = buffer;
        state.config = Arc::new(config);
    }

    let (send_tx, rx) = mpsc::channel(buffer);
    let (ctrl_tx, ctrl_rx) = mpsc::channel(8);
    let (terminal_ctrl_tx, terminal_rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let subscriptions = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let backpressure = Arc::new(AtomicU8::new(0));
    let control = crate::state::CommunityConnectionControl::new(cancel.clone());
    let conn = Arc::new(ConnectionState {
        conn_id: Uuid::new_v4(),
        tenant: TenantContext::resolved(community, host),
        remote_addr: "127.0.0.1:1234".parse().expect("socket addr"),
        auth_state: StdMutex::new(AuthState::Authenticated(auth_context(&keys))),
        subscriptions: Arc::clone(&subscriptions),
        send_tx: send_tx.clone(),
        ctrl_tx: ctrl_tx.clone(),
        terminal_ctrl_tx: terminal_ctrl_tx.clone(),
        cancel: cancel.clone(),
        backpressure_count: Arc::clone(&backpressure),
        grace_limit: 3,
        nip_fi_assertion: None,
        session_deadline: None,
        nip_fi_gate: crate::nip_fi_gate::SessionAdmissionGate::off_mode(cancel.clone()),
        community_control: control.clone(),
    });
    state.conn_manager.register(
        conn.conn_id,
        send_tx,
        ctrl_tx,
        terminal_ctrl_tx,
        None,
        cancel,
        community,
        backpressure,
        subscriptions,
        3,
        control,
    );
    state
        .conn_manager
        .set_authenticated_pubkey(conn.conn_id, keys.public_key().to_bytes().to_vec());
    Harness {
        state,
        conn,
        rx,
        ctrl_rx,
        terminal_rx,
        pool,
        community,
        keys,
    }
}

impl Harness {
    pub(super) fn pubkey(&self) -> Vec<u8> {
        self.keys.public_key().to_bytes().to_vec()
    }

    /// A room in this community; the account is a member when `member`.
    pub(super) async fn room(&self, member: bool, seq: i64) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO channels (id, community_id, name, channel_type, visibility, created_by)
             VALUES ($1, $2, $3, 'stream', 'open', $4)",
        )
        .bind(id)
        .bind(self.community.as_uuid())
        .bind(format!("r-{}", id.simple()))
        .bind(vec![0x41u8; 32])
        .execute(&self.pool)
        .await
        .expect("create channel");
        sqlx::query("UPDATE channels SET accepted_sequence = $2 WHERE id = $1")
            .bind(id)
            .bind(seq)
            .execute(&self.pool)
            .await
            .expect("set seq");
        if member {
            sqlx::query(
                "INSERT INTO channel_members (community_id, channel_id, pubkey, role)
                 VALUES ($1, $2, $3, 'member')",
            )
            .bind(self.community.as_uuid())
            .bind(id)
            .bind(self.pubkey())
            .execute(&self.pool)
            .await
            .expect("add member");
        }
        id
    }

    pub(super) async fn req(
        &self,
        sub: &str,
        filters: Vec<Filter>,
        token_rooms: Option<Vec<Uuid>>,
    ) {
        handle_stream_req(
            sub.to_string(),
            filters,
            self.pubkey(),
            token_rooms,
            Arc::clone(&self.conn),
            Arc::clone(&self.state),
        )
        .await;
    }

    /// Fan one kind-9 message in `room` out to local subscribers.
    pub(super) async fn message(&self, room: Uuid, text: &str) -> String {
        post(&self.state, self.community, room, text).await
    }

    /// Every data frame queued so far.
    pub(super) fn frames(&mut self) -> Vec<serde_json::Value> {
        drain(&mut self.rx)
    }
}

pub(super) async fn post(
    state: &AppState,
    community: CommunityId,
    room: Uuid,
    text: &str,
) -> String {
    let event = EventBuilder::new(Kind::Custom(9), text)
        .tags([Tag::parse(["h", &room.to_string()]).expect("h tag")])
        .sign_with_keys(&Keys::generate())
        .expect("sign");
    let id = event.id.to_hex();
    crate::handlers::event::fan_out_event_to_local_subscribers(
        state,
        community,
        &StoredEvent::new(event, Some(room)),
    )
    .await;
    id
}

pub(super) fn drain(rx: &mut mpsc::Receiver<WsMessage>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    while let Ok(message) = rx.try_recv() {
        if let WsMessage::Text(text) = message {
            out.push(serde_json::from_str(text.as_str()).expect("json frame"));
        }
    }
    out
}

pub(super) fn request() -> Filter {
    filter(fixture()["request"].clone())
}

pub(super) fn closed(frames: &[serde_json::Value], sub: &str) -> Vec<String> {
    frames
        .iter()
        .filter(|f| f[0] == "CLOSED" && f[1] == sub)
        .map(|f| f[2].as_str().unwrap_or_default().to_string())
        .collect()
}

fn snapshot_content(frame: &serde_json::Value) -> serde_json::Value {
    assert_eq!(frame[0], "EVENT", "{frame}");
    assert_eq!(frame[2]["kind"], SNAPSHOT_KIND);
    serde_json::from_str(frame[2]["content"].as_str().expect("content")).expect("part")
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn parts_then_eose_then_live_frames() {
    let mut h = harness(64, |_| on()).await;
    let a = h.room(true, 7).await;
    let b = h.room(true, 0).await;
    let other = h.room(false, 3).await;

    h.req("s", vec![request()], None).await;
    let frames = h.frames();
    assert_eq!(frames.len(), 2, "{frames:?}");
    let part = snapshot_content(&frames[0]);
    assert_eq!(part["part"], 1);
    assert_eq!(part["parts"], 1);
    assert_eq!(part["mode"], "shadow");
    assert_eq!(part["epoch"], "e1");
    assert_eq!(
        part["rooms"],
        serde_json::json!({ a.to_string(): 7, b.to_string(): 0 })
    );
    let event = &frames[0][2];
    assert_eq!(event["pubkey"], h.state.relay_keypair.public_key().to_hex());
    assert_eq!(
        event["tags"],
        serde_json::json!([["p", h.keys.public_key().to_hex()]])
    );
    assert_eq!(frames[1], serde_json::json!(["EOSE", "s"]));

    let id = h.message(a, "hi").await;
    h.message(other, "not mine").await;
    let frames = h.frames();
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0][1], "s");
    assert_eq!(frames[0][2]["id"], id);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn refusal_table() {
    // Off.
    let mut h = harness(64, |_| UserStreamConfig::default()).await;
    h.req("s", vec![request()], None).await;
    assert_eq!(closed(&h.frames(), "s"), [reason::OFF]);

    // Not in the stream set.
    let mut h = harness(64, |_| UserStreamConfig {
        stream: only(&"0".repeat(64)),
        epoch: "e1".into(),
        ..UserStreamConfig::default()
    })
    .await;
    h.req("s", vec![request()], None).await;
    assert_eq!(closed(&h.frames(), "s"), [reason::NOT_ENABLED]);

    // Mode per account.
    let mut h = harness(64, |me| UserStreamConfig {
        live: only(me),
        ..on()
    })
    .await;
    h.req("s", vec![request()], None).await;
    assert_eq!(snapshot_content(&h.frames()[0])["mode"], "live");
    let mut h = harness(64, |me| UserStreamConfig {
        live: Access::On,
        gated: only(me),
        ..on()
    })
    .await;
    h.req("s", vec![request()], None).await;
    assert_eq!(snapshot_content(&h.frames()[0])["mode"], "gated");

    // Shape and kinds, each on its own id so the 5 s rule stays out.
    let mut h = harness(64, |_| on()).await;
    let mut wrong_kinds = fixture()["request"].clone();
    wrong_kinds["kinds"] = serde_json::json!([9]);
    let mut wrong_limit = fixture()["request"].clone();
    wrong_limit["limit"] = 5.into();
    h.req("k", vec![filter(wrong_kinds)], None).await;
    h.req("l", vec![filter(wrong_limit)], None).await;
    h.req("two", vec![request(), request()], None).await;
    let frames = h.frames();
    assert_eq!(closed(&frames, "k"), [reason::KINDS]);
    assert_eq!(closed(&frames, "l"), [reason::SHAPE]);
    assert_eq!(closed(&frames, "two"), [reason::SHAPE]);
    assert!(h.state.sub_registry.stream_of(h.conn.conn_id).is_none());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn second_stream_and_repeat_within_5s() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(true, 1).await;
    h.req("s", vec![request()], None).await;
    assert_eq!(h.frames().len(), 2);

    h.req("t", vec![request()], None).await;
    assert_eq!(closed(&h.frames(), "t"), [reason::SECOND]);
    assert!(
        h.state
            .sub_registry
            .stream_for(h.conn.conn_id, "s")
            .is_some(),
        "the first stream stays"
    );

    // The last stream REQ was `t`, so `s` again is not a repeat: it replaces
    // the stream with a fresh snapshot. Right after that, `s` is a repeat.
    h.req("s", vec![request()], None).await;
    let frames = h.frames();
    assert_eq!(snapshot_content(&frames[0])["part"], 1, "{frames:?}");
    h.req("s", vec![request()], None).await;
    assert_eq!(closed(&h.frames(), "s"), [reason::REPEAT]);
    assert!(
        h.state.sub_registry.stream_of(h.conn.conn_id).is_none(),
        "a repeat ends the active stream too"
    );
    assert!(!h.conn.subscriptions.lock().await.contains_key("s"));
    h.message(room, "after").await;
    assert!(h.frames().is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn auth_required_does_not_start_the_5s_window() {
    let mut h = harness(64, |_| on()).await;
    *h.conn.auth_state.lock().unwrap() = AuthState::Pending {
        challenge: "c".into(),
        started_at: Instant::now(),
    };
    crate::handlers::req::handle_req(
        "s".into(),
        vec![request()],
        vec![None],
        Arc::clone(&h.conn),
        Arc::clone(&h.state),
    )
    .await;
    assert_eq!(
        closed(&h.frames(), "s"),
        ["auth-required: not authenticated"]
    );

    *h.conn.auth_state.lock().unwrap() = AuthState::Authenticated(auth_context(&h.keys));
    crate::handlers::req::handle_req(
        "s".into(),
        vec![request()],
        vec![None],
        Arc::clone(&h.conn),
        Arc::clone(&h.state),
    )
    .await;
    let frames = h.frames();
    assert_eq!(snapshot_content(&frames[0])["part"], 1, "{frames:?}");
    assert_eq!(frames[1], serde_json::json!(["EOSE", "s"]));
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn token_scope_limits_the_rooms() {
    let mut h = harness(64, |_| on()).await;
    let a = h.room(true, 2).await;
    let b = h.room(true, 3).await;
    h.req("s", vec![request()], Some(vec![a])).await;
    let frames = h.frames();
    assert_eq!(
        snapshot_content(&frames[0])["rooms"],
        serde_json::json!({ a.to_string(): 2 })
    );
    h.message(b, "outside the token").await;
    assert!(h.frames().is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn more_than_5000_rooms_is_refused() {
    let mut h = harness(64, |_| on()).await;
    sqlx::query(
        "WITH c AS (
             INSERT INTO channels (id, community_id, name, channel_type, visibility, created_by)
             SELECT gen_random_uuid(), $1, 'bulk-' || g, 'stream', 'open', $2
             FROM generate_series(1, 5001) g
             RETURNING id)
         INSERT INTO channel_members (community_id, channel_id, pubkey, role)
         SELECT $1, id, $3, 'member' FROM c",
    )
    .bind(h.community.as_uuid())
    .bind(vec![0x41u8; 32])
    .bind(h.pubkey())
    .execute(&h.pool)
    .await
    .expect("bulk rooms");
    h.req("s", vec![request()], None).await;
    assert_eq!(closed(&h.frames(), "s"), [reason::TOO_MANY_ROOMS]);
    assert!(h.state.sub_registry.stream_of(h.conn.conn_id).is_none());
    assert!(!h.conn.subscriptions.lock().await.contains_key("s"));
}

/// Register a stream the way the REQ does, left in its snapshot phase.
pub(super) async fn snapshot_phase_stream(
    h: &Harness,
    mode: StreamMode,
    rooms: Vec<Uuid>,
) -> Arc<StreamHandle> {
    let owner = crate::handlers::close::next_owner();
    h.conn.subscriptions.lock().await.insert("s".into(), owner);
    let handle = Arc::new(StreamHandle::new(
        "s".into(),
        owner,
        mode,
        h.conn.tenant.clone(),
        queue_cap(h.state.config.send_buffer_size),
    ));
    h.state.sub_registry.register_stream(
        h.community,
        h.conn.conn_id,
        Arc::clone(&handle),
        vec![stream_filter()],
        rooms,
    );
    // The rooms are set at registration here: later changes apply directly.
    assert!(handle.take_room_ops().is_empty());
    spawn_teardown(Arc::clone(&h.state), h.conn.conn_id, Arc::clone(&handle));
    handle
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_burst_during_registration_waits_for_eose_in_order() {
    // Default send buffer (1000): the queue holds up to 250 frames.
    let mut h = harness(1000, |_| on()).await;
    let room = h.room(true, 0).await;
    let handle = snapshot_phase_stream(&h, StreamMode::Live, vec![room]).await;
    let mut ids = Vec::new();
    for n in 0..250 {
        ids.push(h.message(room, &format!("m{n}")).await);
    }
    assert!(h.frames().is_empty(), "nothing before the EOSE");
    assert_eq!(handle.queued(), 250);
    h.conn.send(RelayMessage::eose("s"));
    assert!(handle.go_live(|frame| {
        h.state
            .conn_manager
            .send_to_text_bytes(h.conn.conn_id, frame)
    }));
    let frames = h.frames();
    assert_eq!(frames[0], serde_json::json!(["EOSE", "s"]));
    let flushed: Vec<_> = frames[1..]
        .iter()
        .map(|f| f[2]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(flushed, ids, "queued frames keep their order");
    let late = h.message(room, "live").await;
    assert_eq!(h.frames()[0][2]["id"], late);
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_full_queue_closes_the_shadow_stream_only() {
    // Buffer 40 → queue cap 10.
    let mut h = harness(40, |_| on()).await;
    let room = h.room(true, 0).await;
    snapshot_phase_stream(&h, StreamMode::Shadow, vec![room]).await;
    for n in 0..11 {
        h.message(room, &format!("m{n}")).await;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.conn.subscriptions.lock().await.contains_key("s") {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("teardown ran");
    assert_eq!(closed(&h.frames(), "s"), [reason::OVERFLOW]);
    assert!(!h.conn.cancel.is_cancelled(), "shadow keeps the socket");
    assert!(h.state.sub_registry.stream_of(h.conn.conn_id).is_none());
    assert!(drain(&mut h.ctrl_rx).is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn a_full_queue_closes_a_live_stream_and_the_socket() {
    let mut h = harness(40, |_| on()).await;
    let room = h.room(true, 0).await;
    snapshot_phase_stream(&h, StreamMode::Live, vec![room]).await;
    for n in 0..11 {
        h.message(room, &format!("m{n}")).await;
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while !h.conn.cancel.is_cancelled() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("socket cancelled");
    // Live: the CLOSED goes on the control channel, ahead of the Close.
    assert!(closed(&h.frames(), "s").is_empty());
    assert_eq!(closed(&drain(&mut h.ctrl_rx), "s"), [reason::OVERFLOW]);
    assert!(h.state.sub_registry.stream_of(h.conn.conn_id).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Postgres"]
async fn live_frames_never_precede_the_eose() {
    let mut h = harness(1000, |_| on()).await;
    let room = h.room(true, 0).await;
    let poster = async {
        for n in 0..150 {
            post(&h.state, h.community, room, &format!("b{n}")).await;
            tokio::task::yield_now().await;
        }
    };
    tokio::join!(h.req("s", vec![request()], None), poster);
    let frames = h.frames();
    let eose = frames
        .iter()
        .position(|f| f[0] == "EOSE")
        .expect("EOSE sent");
    for frame in &frames[..eose] {
        assert_eq!(
            frame[2]["kind"], SNAPSHOT_KIND,
            "only parts before the EOSE"
        );
    }
    assert!(closed(&frames, "s").is_empty());
}

#[tokio::test]
#[ignore = "requires Postgres"]
async fn chunk_subscriptions_are_unchanged() {
    let mut h = harness(64, |_| on()).await;
    let room = h.room(true, 0).await;
    h.req("s", vec![request()], None).await;
    h.frames();
    h.state.sub_registry.register_channels_scoped(
        h.community,
        h.conn.conn_id,
        "chunk".into(),
        vec![Filter::new().kind(Kind::Custom(9))],
        vec![room],
    );
    h.message(room, "both").await;
    let subs: BTreeSet<String> = h
        .frames()
        .iter()
        .map(|f| f[1].as_str().unwrap().to_string())
        .collect();
    assert_eq!(subs, BTreeSet::from(["s".to_string(), "chunk".to_string()]));

    // The generic channel eviction (archive, open -> private) removes the
    // chunk and leaves the stream; U1b decides the stream's rooms.
    h.state
        .sub_registry
        .remove_channel_subscriptions_scoped(h.community, h.conn.conn_id, room);
    h.message(room, "stream only").await;
    let frames = h.frames();
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0][1], "s");
}
