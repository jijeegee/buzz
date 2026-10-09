//! Observer receiving-device cap tests.
//!
//! Store scenarios run on the in-memory store and, when `BUZZ_TEST_REDIS_URL`
//! names an isolated Redis, the production Lua scripts. Handler scenarios
//! drive the production REQ, CLOSE and disconnect paths with the memory store.

use super::*;
use crate::handlers::close::{handle_close, release_connection_subscriptions};
use crate::handlers::req::handle_req;
use crate::observer_quota::ObserverTier;
use axum::extract::ws::Message as WsMessage;
use nostr::Keys;
use tokio::sync::mpsc;

/// A fixed instant well past the stale window.
const T: u64 = 1_700_000_000_000;

fn community() -> CommunityId {
    CommunityId::from_uuid(Uuid::new_v4())
}

fn lease(community: CommunityId, owner: &PublicKey, device: &str, sub: &str) -> DeviceLease {
    DeviceLease {
        community,
        owner: *owner,
        device: device.to_owned(),
        member: format!("conn-{device}:{sub}"),
    }
}

const ALLOWED: DeviceDecision = DeviceDecision::Allowed;

async fn cap_admits_known_devices_and_refuses_new_ones(store: &dyn ObserverDeviceStore) {
    let (c, owner) = (community(), Keys::generate().public_key());
    let l = |device, sub| lease(c, &owner, device, sub);
    assert_eq!(store.acquire_device(&l("a", "1"), T, 2, 2).await.unwrap(), ALLOWED);
    assert_eq!(store.acquire_device(&l("b", "1"), T, 2, 2).await.unwrap(), ALLOWED);
    assert_eq!(
        store.acquire_device(&l("c", "1"), T, 2, 2).await.unwrap(),
        DeviceDecision::DeviceCap { devices: 2 },
        "a third device exceeds the cap"
    );
    assert_eq!(
        store.acquire_device(&l("a", "1"), T, 2, 2).await.unwrap(),
        ALLOWED,
        "re-REQ of a counted subscription is admitted"
    );
    assert_eq!(
        store.acquire_device(&l("a", "2"), T, 2, 2).await.unwrap(),
        ALLOWED,
        "a known device may open a second subscription"
    );
    assert_eq!(
        store.acquire_device(&l("a", "3"), T, 2, 2).await.unwrap(),
        DeviceDecision::SubsPerDevice { subs: 2 },
        "a third subscription on one device is refused"
    );
    let other_owner = lease(c, &Keys::generate().public_key(), "c", "1");
    assert_eq!(
        store.acquire_device(&other_owner, T, 2, 2).await.unwrap(),
        ALLOWED,
        "owners are counted separately"
    );
    let other_community = lease(community(), &owner, "c", "1");
    assert_eq!(
        store.acquire_device(&other_community, T, 2, 2).await.unwrap(),
        ALLOWED,
        "communities are counted separately"
    );
}

async fn release_frees_the_slot_at_once(store: &dyn ObserverDeviceStore) {
    let (c, owner) = (community(), Keys::generate().public_key());
    let l = |device, sub| lease(c, &owner, device, sub);
    for (device, sub) in [("a", "1"), ("a", "2"), ("b", "1")] {
        assert_eq!(store.acquire_device(&l(device, sub), T, 2, 2).await.unwrap(), ALLOWED);
    }
    store.release_device(&l("a", "1")).await.unwrap();
    assert_eq!(
        store.acquire_device(&l("c", "1"), T, 2, 2).await.unwrap(),
        DeviceDecision::DeviceCap { devices: 2 },
        "device a still holds a subscription"
    );
    store.release_device(&l("a", "2")).await.unwrap();
    store.release_device(&l("a", "2")).await.unwrap(); // Idempotent.
    assert_eq!(
        store.acquire_device(&l("c", "1"), T, 2, 2).await.unwrap(),
        ALLOWED,
        "device a left with its last subscription"
    );
}

async fn stale_entries_are_pruned_and_refresh_keeps_live_ones(store: &dyn ObserverDeviceStore) {
    let (c, owner) = (community(), Keys::generate().public_key());
    let l = |device, sub| lease(c, &owner, device, sub);
    assert_eq!(store.acquire_device(&l("a", "1"), T, 1, 2).await.unwrap(), ALLOWED);
    assert_eq!(store.acquire_device(&l("b", "1"), T, 2, 2).await.unwrap(), ALLOWED);
    store.refresh_device(&l("b", "1"), T + 120_000).await.unwrap();
    let just_before = T + DEVICE_STALE_MS - 1;
    assert_eq!(
        store.acquire_device(&l("c", "1"), just_before, 2, 2).await.unwrap(),
        DeviceDecision::DeviceCap { devices: 2 },
        "entries live until the stale window ends"
    );
    let stale = T + DEVICE_STALE_MS;
    assert_eq!(
        store.acquire_device(&l("c", "1"), stale, 2, 2).await.unwrap(),
        ALLOWED,
        "the unrefreshed device a was pruned"
    );
    assert_eq!(
        store.acquire_device(&l("d", "1"), stale, 2, 2).await.unwrap(),
        DeviceDecision::DeviceCap { devices: 2 },
        "the refreshed device b is still live"
    );

    store.release_device(&l("c", "1")).await.unwrap();
    store.refresh_device(&l("c", "1"), stale).await.unwrap();
    assert_eq!(
        store.acquire_device(&l("d", "1"), stale, 2, 2).await.unwrap(),
        ALLOWED,
        "refresh never resurrects a released lease"
    );
}

#[tokio::test]
async fn memory_store_cap() {
    cap_admits_known_devices_and_refuses_new_ones(&MemoryObserverDevices::default()).await;
}

#[tokio::test]
async fn memory_store_release() {
    release_frees_the_slot_at_once(&MemoryObserverDevices::default()).await;
}

#[tokio::test]
async fn memory_store_stale_and_refresh() {
    stale_entries_are_pruned_and_refresh_keeps_live_ones(&MemoryObserverDevices::default()).await;
}

fn redis_store() -> RedisObserverDevices {
    let url = std::env::var("BUZZ_TEST_REDIS_URL").expect("explicit isolated BUZZ_TEST_REDIS_URL");
    RedisObserverDevices::new(
        deadpool_redis::Config::from_url(url)
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .expect("redis pool"),
    )
}

#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_cap() {
    cap_admits_known_devices_and_refuses_new_ones(&redis_store()).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_release() {
    release_frees_the_slot_at_once(&redis_store()).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_stale_and_refresh() {
    stale_entries_are_pruned_and_refresh_keeps_live_ones(&redis_store()).await;
}

/// Keys carry the owner hash tag and expire after the stale window.
#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_keys_expire() {
    let store = redis_store();
    let l = lease(community(), &Keys::generate().public_key(), "a", "1");
    let now = crate::observer_quota::now_ms();
    store.acquire_device(&l, now, 2, 2).await.unwrap();
    let mut conn = store.pool.get().await.unwrap();
    for key in [l.devices_key(), l.subs_key()] {
        assert!(key.contains(&format!("{{{}}}", l.owner.to_hex())));
        let ttl: i64 = redis::cmd("PTTL")
            .arg(&key)
            .query_async(&mut *conn)
            .await
            .unwrap();
        assert!(ttl > 0 && ttl <= DEVICE_STALE_MS as i64, "{key}: {ttl}");
    }
    store.release_device(&l).await.unwrap();
    let exists: i64 = redis::cmd("EXISTS")
        .arg(l.subs_key())
        .query_async(&mut *conn)
        .await
        .unwrap();
    assert_eq!(exists, 0, "the last release deletes the device's key");
}

#[test]
fn device_tag_is_validated() {
    let auth = |tags: Vec<Vec<&str>>| {
        let tags = tags
            .into_iter()
            .map(|tag| nostr::Tag::parse(tag).unwrap())
            .collect::<Vec<_>>();
        let event = nostr::EventBuilder::new(nostr::Kind::Authentication, "")
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .unwrap();
        auth_device_tag(&event)
    };
    let max = "a".repeat(DEVICE_TAG_MAX_LEN);
    let long = "a".repeat(DEVICE_TAG_MAX_LEN + 1);
    assert_eq!(auth(vec![vec!["device", "Pixel_8-a"]]).as_deref(), Some("Pixel_8-a"));
    assert_eq!(auth(vec![vec!["device", &max]]).as_deref(), Some(max.as_str()));
    assert_eq!(auth(vec![]), None, "absent");
    assert_eq!(auth(vec![vec!["device", ""]]), None, "empty");
    assert_eq!(auth(vec![vec!["device"]]), None, "no value");
    assert_eq!(auth(vec![vec!["device", &long]]), None, "too long");
    for bad in ["a b", "a:b", "a/b", "기기", "a.b"] {
        assert_eq!(auth(vec![vec!["device", bad]]), None, "{bad}");
    }
    assert_eq!(
        auth(vec![vec!["device", "one"], vec!["device", "two"]]),
        None,
        "more than one device tag"
    );
}

fn token_ctx(pubkey: PublicKey, device: Option<Uuid>, bot: bool) -> AuthContext {
    let principal = buzz_core::principal::PrincipalId::from_slice(&pubkey.to_bytes()).unwrap();
    AuthContext {
        pubkey,
        scopes: Vec::new(),
        channel_ids: None,
        auth_method: buzz_auth::AuthMethod::Token,
        agent_owner_pubkey: None,
        token: Some(Box::new(buzz_auth::TokenBinding {
            principal: principal.clone(),
            kind: buzz_core::principal::AccessTokenKind::User,
            token_hash: [7; 32],
            expires_at: None,
            device_id: device,
            session_id: None,
            bot_id: bot.then_some(principal),
            bot_owner: None,
            is_operator: false,
        })),
    }
}

fn key_ctx(pubkey: PublicKey, agent_owner_pubkey: Option<PublicKey>) -> AuthContext {
    AuthContext {
        pubkey,
        scopes: Vec::new(),
        channel_ids: None,
        auth_method: buzz_auth::AuthMethod::Nip42,
        agent_owner_pubkey,
        token: None,
    }
}

#[test]
fn device_prefers_token_device_then_tag_then_connection() {
    let (pubkey, conn_id, device) = (Keys::generate().public_key(), Uuid::new_v4(), Uuid::new_v4());
    assert_eq!(
        device_id(&token_ctx(pubkey, Some(device), false), Some("tag1"), conn_id),
        format!("tok:{device}"),
        "a token session's device overrides any tag"
    );
    assert_eq!(
        device_id(&key_ctx(pubkey, None), Some("tag1"), conn_id),
        "tag:tag1"
    );
    assert_eq!(
        device_id(&key_ctx(pubkey, None), None, conn_id),
        format!("conn:{conn_id}"),
        "no tag counts the connection as its own device"
    );
    assert_eq!(
        device_id(&token_ctx(pubkey, None, false), None, conn_id),
        format!("conn:{conn_id}")
    );
}

#[test]
fn only_observer_filters_addressed_to_the_caller_count() {
    let me = Keys::generate().public_key();
    let other = Keys::generate().public_key();
    let observer = |p: &PublicKey| {
        Filter::new()
            .kind(nostr::Kind::Custom(24200))
            .pubkey(*p)
    };
    assert!(is_owner_observer_req(&[observer(&me)], &me));
    assert!(is_owner_observer_req(
        &[Filter::new().kind(nostr::Kind::TextNote), observer(&me)],
        &me
    ));
    assert!(!is_owner_observer_req(&[observer(&other)], &me));
    assert!(!is_owner_observer_req(
        &[Filter::new().kind(nostr::Kind::Custom(24200))],
        &me
    ));
    assert!(!is_owner_observer_req(
        &[Filter::new().kind(nostr::Kind::Custom(20002)).pubkey(me)],
        &me
    ));
    assert!(!is_owner_observer_req(&[Filter::new().pubkey(me)], &me));
}

// --- Handler scenarios --------------------------------------------------------

struct Harness {
    state: Arc<AppState>,
    owner: Keys,
}

/// Test state with memory device and quota stores, an unreachable database
/// (history reads fail fast and end in EOSE) and `owner` on the free tier.
async fn harness(store: Arc<dyn ObserverDeviceStore>) -> Harness {
    let base = crate::state::tests::test_state_with_database_url("postgres://127.0.0.1:1/devcap")
        .await;
    let mut state = (*base).clone();
    state.observer_devices = store;
    state.observer_device_leases = Arc::default();
    let owner = Keys::generate();
    Harness {
        state: Arc::new(state),
        owner,
    }
}

impl Harness {
    fn conn(&self, ctx: AuthContext) -> (Arc<ConnectionState>, mpsc::Receiver<WsMessage>) {
        let (conn, rx) = test_conn(AuthState::Authenticated(ctx.clone()));
        let key = (conn.tenant.community(), ctx.pubkey.to_bytes());
        self.state
            .observer_tier_cache
            .insert(key, Some(ObserverTier::Free));
        self.state
            .accessible_channels_cache
            .insert((key.0, key.1.to_vec()), Vec::new());
        (conn, rx)
    }

    fn owner_conn(&self) -> (Arc<ConnectionState>, mpsc::Receiver<WsMessage>) {
        self.conn(key_ctx(self.owner.public_key(), None))
    }

    async fn req(&self, conn: &Arc<ConnectionState>, sub_id: &str, filters: Vec<Filter>) {
        handle_req(
            sub_id.to_owned(),
            filters,
            Vec::new(),
            Arc::clone(conn),
            Arc::clone(&self.state),
        )
        .await;
    }

    async fn observe(&self, conn: &Arc<ConnectionState>, sub_id: &str) {
        let pubkey = match conn.auth_state_snapshot() {
            AuthState::Authenticated(ctx) => ctx.pubkey,
            _ => unreachable!("test connections are authenticated"),
        };
        self.req(conn, sub_id, vec![observer_filter(&pubkey)]).await;
    }

    fn registered(&self, conn: &ConnectionState, sub_id: &str) -> bool {
        self.state
            .sub_registry
            .get_filters(conn.conn_id, sub_id)
            .is_some()
    }
}

/// An authenticated connection whose outbound queue holds every frame a
/// scenario produces.
fn test_conn(auth: AuthState) -> (Arc<ConnectionState>, mpsc::Receiver<WsMessage>) {
    let (send_tx, send_rx) = mpsc::channel(64);
    let cancel = tokio_util::sync::CancellationToken::new();
    let conn = ConnectionState {
        conn_id: Uuid::new_v4(),
        tenant: buzz_core::tenant::TenantContext::resolved(
            CommunityId::from_uuid(Uuid::nil()),
            "test.local".to_string(),
        ),
        remote_addr: "127.0.0.1:1234".parse().expect("addr"),
        auth_state: std::sync::Mutex::new(auth),
        subscriptions: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        send_tx,
        ctrl_tx: mpsc::channel(8).0,
        terminal_ctrl_tx: mpsc::channel(1).0,
        cancel: cancel.clone(),
        backpressure_count: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        grace_limit: 3,
        nip_fi_assertion: None,
        session_deadline: None,
        nip_fi_gate: crate::nip_fi_gate::SessionAdmissionGate::off_mode(cancel.clone()),
        community_control: crate::state::CommunityConnectionControl::new(cancel),
    };
    (Arc::new(conn), send_rx)
}

fn observer_filter(p: &PublicKey) -> Filter {
    Filter::new()
        .kind(nostr::Kind::Custom(24200))
        .pubkey(*p)
        .limit(0)
}

fn chat_filter() -> Filter {
    Filter::new().kind(nostr::Kind::TextNote).limit(0)
}

fn frames(rx: &mut mpsc::Receiver<WsMessage>) -> Vec<String> {
    std::iter::from_fn(|| rx.try_recv().ok())
        .map(|msg| match msg {
            WsMessage::Text(text) => text.to_string(),
            other => panic!("expected text frame, got {other:?}"),
        })
        .collect()
}

const CAP_CLOSED: &str = "restricted: observer device limit reached (tier free: 2 devices)";

fn cap_closed(sub_id: &str) -> String {
    format!(r#"["CLOSED","{sub_id}","{CAP_CLOSED}"]"#)
}

/// The free tier admits two devices; a third device's REQ is refused with the
/// exact CLOSED, registers nothing, and is admitted as soon as device A sends
/// CLOSE.
#[tokio::test]
async fn third_device_is_refused_until_a_device_closes() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    let ((a, mut a_rx), (b, _b_rx), (c, mut c_rx)) =
        (h.owner_conn(), h.owner_conn(), h.owner_conn());
    h.observe(&a, "obs").await;
    h.observe(&b, "obs").await;
    assert!(h.registered(&a, "obs") && h.registered(&b, "obs"));
    frames(&mut a_rx);

    h.observe(&c, "obs").await;
    assert_eq!(frames(&mut c_rx), vec![cap_closed("obs")]);
    assert!(!h.registered(&c, "obs"), "a refused REQ registers nothing");
    assert!(!c.subscriptions.lock().await.contains_key("obs"));

    handle_close("obs".into(), Arc::clone(&a), Arc::clone(&h.state)).await;
    h.observe(&c, "obs").await;
    assert!(
        h.registered(&c, "obs"),
        "device A's CLOSE frees its slot at once"
    );
    assert!(!frames(&mut c_rx).iter().any(|f| f.contains("CLOSED")));
}

/// Disconnect releases every lease of the connection.
#[tokio::test]
async fn disconnect_frees_the_slot() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    let ((a, _), (b, _), (c, mut c_rx)) = (h.owner_conn(), h.owner_conn(), h.owner_conn());
    h.observe(&a, "obs").await;
    h.observe(&b, "obs").await;
    h.observe(&c, "obs").await;
    assert_eq!(frames(&mut c_rx), vec![cap_closed("obs")]);

    a.cancel.cancel();
    release_connection_subscriptions(&a, &h.state).await;
    assert!(h.state.observer_device_leases.leases.get(&a.conn_id).is_none());
    h.observe(&c, "obs").await;
    assert!(h.registered(&c, "obs"));
}

/// Reusing an observer sub_id for another REQ releases its slot; re-REQ of
/// the same observer sub_id keeps it without counting twice.
#[tokio::test]
async fn sub_id_reuse_releases_or_keeps_the_lease() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    let ((a, _), (b, _), (c, mut c_rx)) = (h.owner_conn(), h.owner_conn(), h.owner_conn());
    h.observe(&a, "obs").await;
    h.observe(&a, "obs").await; // Same sub again: still one subscription.
    h.observe(&a, "obs2").await; // Second subscription on device A.
    h.observe(&b, "obs").await;
    assert_eq!(h.state.observer_device_leases.len(), 3);

    h.req(&a, "obs", vec![chat_filter()]).await;
    h.observe(&c, "obs").await;
    assert_eq!(
        frames(&mut c_rx),
        vec![cap_closed("obs")],
        "device A still holds obs2"
    );
    h.req(&a, "obs2", vec![chat_filter()]).await;
    h.observe(&c, "obs").await;
    assert!(h.registered(&c, "obs"), "both of A's subscriptions were replaced");
    assert!(h.registered(&a, "obs"), "the replacing chat REQ stays registered");
}

/// A device may hold two observer subscriptions; the third is refused.
#[tokio::test]
async fn per_device_subscription_cap() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    let (a, mut a_rx) = h.owner_conn();
    h.observe(&a, "o1").await;
    h.observe(&a, "o2").await;
    frames(&mut a_rx);
    h.observe(&a, "o3").await;
    assert_eq!(
        frames(&mut a_rx),
        vec![r#"["CLOSED","o3","restricted: observer subscription limit reached (2 per device)"]"#]
    );
    assert!(!h.registered(&a, "o3"));
}

/// A refused observer REQ leaves the connection's other subscriptions, and
/// new non-observer REQs, working.
#[tokio::test]
async fn refusal_only_affects_that_observer_req() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    let ((a, _), (b, _), (c, mut c_rx)) = (h.owner_conn(), h.owner_conn(), h.owner_conn());
    h.req(&c, "chat", vec![chat_filter()]).await;
    h.observe(&a, "obs").await;
    h.observe(&b, "obs").await;
    frames(&mut c_rx);

    h.observe(&c, "obs").await;
    assert_eq!(frames(&mut c_rx), vec![cap_closed("obs")]);
    assert!(h.registered(&c, "chat"), "the existing subscription survives");
    assert!(!c.cancel.is_cancelled(), "the connection stays open");
    assert!(matches!(c.auth_state_snapshot(), AuthState::Authenticated(_)));

    h.req(&c, "typing", vec![Filter::new().kind(nostr::Kind::Custom(20002)).limit(0)])
        .await;
    assert!(h.registered(&c, "typing"), "new non-observer REQs still work");
    assert!(!frames(&mut c_rx).iter().any(|f| f.contains("CLOSED")));
}

/// Agent connections (NIP-OA owner or bot token) are never counted.
#[tokio::test]
async fn agent_connections_are_not_counted() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    let agent = Keys::generate().public_key();
    let mut conns = Vec::new();
    for _ in 0..3 {
        let (conn, _) = h.conn(key_ctx(agent, Some(h.owner.public_key())));
        h.observe(&conn, "obs").await;
        assert!(h.registered(&conn, "obs"));
        conns.push(conn);
    }
    let bot = Keys::generate().public_key();
    for _ in 0..3 {
        let (conn, _) = h.conn(token_ctx(bot, None, true));
        h.observe(&conn, "obs").await;
        assert!(h.registered(&conn, "obs"));
    }
    assert!(h.state.observer_device_leases.is_empty());
}

/// Non-observer REQs are never counted.
#[tokio::test]
async fn non_observer_reqs_are_not_counted() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    for _ in 0..3 {
        let (conn, _) = h.owner_conn();
        h.req(&conn, "chat", vec![chat_filter()]).await;
        assert!(h.registered(&conn, "chat"));
    }
    assert!(h.state.observer_device_leases.is_empty());
}

/// Connections sharing a token device or an AUTH device tag count as one
/// device; a token device wins over a tag.
#[tokio::test]
async fn shared_device_ids_count_once() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    let owner = h.owner.public_key();
    let phone = Uuid::new_v4();
    let (t1, _) = h.conn(token_ctx(owner, Some(phone), false));
    let (t2, _) = h.conn(token_ctx(owner, Some(phone), false));
    h.state
        .observer_device_leases
        .set_auth_device(t2.conn_id, "laptop".into());
    let (k1, _) = h.owner_conn();
    let (k2, _) = h.owner_conn();
    for conn in [&k1, &k2] {
        h.state
            .observer_device_leases
            .set_auth_device(conn.conn_id, "laptop".into());
    }
    let (other, mut other_rx) = h.owner_conn();
    for conn in [&t1, &t2, &k1, &k2] {
        h.observe(conn, "obs").await;
        assert!(h.registered(conn, "obs"));
    }
    let devices: std::collections::HashSet<String> = h
        .state
        .observer_device_leases
        .snapshot()
        .into_iter()
        .map(|lease| lease.device)
        .collect();
    assert_eq!(
        devices,
        [format!("tok:{phone}"), "tag:laptop".to_owned()].into(),
        "the token device overrides t2's tag"
    );
    h.observe(&other, "obs").await;
    assert_eq!(frames(&mut other_rx), vec![cap_closed("obs")]);
}

/// Forgetting the connection drops its AUTH device tag.
#[tokio::test]
async fn disconnect_forgets_the_auth_device() {
    let h = harness(Arc::new(MemoryObserverDevices::default())).await;
    let (a, _) = h.owner_conn();
    h.state
        .observer_device_leases
        .set_auth_device(a.conn_id, "laptop".into());
    a.cancel.cancel();
    release_connection_subscriptions(&a, &h.state).await;
    assert_eq!(h.state.observer_device_leases.auth_device(a.conn_id), None);
}

/// An unreachable device store admits the REQ uncounted.
#[tokio::test]
async fn unavailable_store_fails_open() {
    let base = crate::state::tests::test_state_with_database_url("postgres://127.0.0.1:1/devcap")
        .await; // Unreachable Redis too.
    let h = Harness {
        state: base,
        owner: Keys::generate(),
    };
    let mut conns = Vec::new();
    for _ in 0..3 {
        let (conn, _) = h.owner_conn();
        h.observe(&conn, "obs").await;
        assert!(h.registered(&conn, "obs"));
        conns.push(conn);
    }
    assert!(h.state.observer_device_leases.is_empty());
}

/// The refresh pass keeps this node's live leases current.
#[tokio::test]
async fn refresh_pass_touches_live_leases() {
    let store = Arc::new(MemoryObserverDevices::default());
    let h = harness(store.clone()).await;
    let (a, _) = h.owner_conn();
    h.observe(&a, "obs").await;
    let lease = h.state.observer_device_leases.snapshot().remove(0);
    let seen = |store: &MemoryObserverDevices| {
        store.sets.lock().unwrap()[&lease.subs_key()][&lease.member]
    };
    store
        .sets
        .lock()
        .unwrap()
        .get_mut(&lease.subs_key())
        .unwrap()
        .insert(lease.member.clone(), 1);
    refresh_live_leases(&h.state).await;
    assert!(seen(&store) > 1, "the refresh pass moved last-seen forward");
}

#[test]
fn policy_reports_receiving_devices_without_headroom() {
    let config = crate::observer_quota::ObserverQuotaConfig::default();
    for (tier, devices) in [
        (ObserverTier::Free, 2),
        (ObserverTier::Standard, 3),
        (ObserverTier::Premium, 5),
    ] {
        let policy = serde_json::to_value(config.policy(tier)).unwrap();
        assert_eq!(policy["receiving_devices"], devices, "{tier}");
    }
}
