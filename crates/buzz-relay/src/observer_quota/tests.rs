//! Observer tier quota tests.
//!
//! Store scenarios are written once against [`ObserverQuotaStore`] and run on
//! both the in-memory store and, when `BUZZ_TEST_REDIS_URL` names an isolated
//! Redis, the production Lua scripts. Every scenario uses a fresh community, so
//! keys never collide and nothing is flushed.

use super::*;
use nostr::nips::nip44;
use nostr::Keys;

/// A minute-aligned instant, so 1 s and 60 s windows start here.
const T: u64 = 28_333_334 * 60_000;
/// Large enough never to bind (and exact as a Lua double).
const UNLIMITED: u64 = 1_000_000_000_000;

fn unlimited() -> TierLimits {
    TierLimits {
        tick_ms: 1_000,
        agent_frames_per_min: UNLIMITED,
        agent_burst_per_sec: UNLIMITED,
        agent_frame_max_bytes: UNLIMITED,
        agent_bytes_per_min: UNLIMITED,
        account_agents: UNLIMITED,
        account_frames_per_min: UNLIMITED,
        account_burst_per_sec: UNLIMITED,
        account_bytes_per_min: UNLIMITED,
        account_bytes_per_day: UNLIMITED,
        account_typing_agents: UNLIMITED,
    }
}

fn community() -> CommunityId {
    CommunityId::from_uuid(Uuid::new_v4())
}

fn key() -> PublicKey {
    Keys::generate().public_key()
}

fn denied(limit: LimitKind, retry_after_secs: u64, active_agents: u64) -> Decision {
    Decision::Denied {
        limit,
        retry_after_secs,
        active_agents,
    }
}

/// One counter limit: which field binds, its window, and whether it counts
/// bytes (vs frames) and spans the account (vs one agent).
struct CounterCase {
    kind: LimitKind,
    set: fn(&mut TierLimits, u64),
    window_ms: u64,
    bytes: bool,
    account: bool,
}

const COUNTER_CASES: [CounterCase; 7] = [
    CounterCase {
        kind: LimitKind::AgentBurst,
        set: |l, v| l.agent_burst_per_sec = v,
        window_ms: 1_000,
        bytes: false,
        account: false,
    },
    CounterCase {
        kind: LimitKind::AgentFrames,
        set: |l, v| l.agent_frames_per_min = v,
        window_ms: 60_000,
        bytes: false,
        account: false,
    },
    CounterCase {
        kind: LimitKind::AgentBytes,
        set: |l, v| l.agent_bytes_per_min = v,
        window_ms: 60_000,
        bytes: true,
        account: false,
    },
    CounterCase {
        kind: LimitKind::AccountBurst,
        set: |l, v| l.account_burst_per_sec = v,
        window_ms: 1_000,
        bytes: false,
        account: true,
    },
    CounterCase {
        kind: LimitKind::AccountFrames,
        set: |l, v| l.account_frames_per_min = v,
        window_ms: 60_000,
        bytes: false,
        account: true,
    },
    CounterCase {
        kind: LimitKind::AccountBytes,
        set: |l, v| l.account_bytes_per_min = v,
        window_ms: 60_000,
        bytes: true,
        account: true,
    },
    CounterCase {
        kind: LimitKind::AccountDailyBytes,
        set: |l, v| l.account_bytes_per_day = v,
        window_ms: 86_400_000,
        bytes: true,
        account: true,
    },
];

/// Each counter admits up to its limit, denies the frame that would exceed it
/// with the window's remaining seconds, charges nothing on denial, and resets
/// in the next window.
async fn every_counter_limit_allows_denies_without_charging_and_resets(
    store: &dyn ObserverQuotaStore,
) {
    for case in &COUNTER_CASES {
        let (community, owner) = (community(), key());
        let agents = [key(), key()];
        // Account limits are shared: alternate agents to prove it.
        let agent = |i: usize| &agents[if case.account { i % 2 } else { 0 }];
        let mut limits = unlimited();
        // Frames: limit 2, frames of 1 byte. Bytes: limit 100, frames of 40.
        let (limit, size) = if case.bytes { (100, 40) } else { (2, 1) };
        (case.set)(&mut limits, limit);
        let window_end = (T / case.window_ms + 1) * case.window_ms;
        let retry = (window_end - T).div_ceil(1_000);
        let admit = |i: usize, bytes: u64, now: u64| {
            store.admit_telemetry(community, &owner, agent(i), bytes, now, &limits)
        };

        for i in 0..2 {
            assert!(
                matches!(admit(i, size, T).await.unwrap(), Decision::Allowed { .. }),
                "{:?}: frame {i} is under the limit",
                case.kind
            );
        }
        let active = if case.account { 2 } else { 1 };
        assert_eq!(
            admit(2, size, T).await.unwrap(),
            denied(case.kind, retry, active),
            "{:?}: the frame reaching past the limit is denied",
            case.kind
        );
        if case.bytes {
            // The denied 40 bytes were not charged: 80 + 20 fits exactly.
            assert!(
                matches!(admit(3, 20, T).await.unwrap(), Decision::Allowed { .. }),
                "{:?}: a denied frame must not consume budget",
                case.kind
            );
        }
        assert!(
            matches!(admit(0, 1, T + 999).await.unwrap(), Decision::Denied { limit, .. } if limit == case.kind),
            "{:?}: still denied within the window",
            case.kind
        );
        assert!(
            matches!(
                admit(0, size, window_end).await.unwrap(),
                Decision::Allowed { .. }
            ),
            "{:?}: the next window starts fresh",
            case.kind
        );
    }
}

/// A frame denied by one counter must not charge the counters checked before
/// it: after an account-burst denial the agent's minute budget is untouched.
async fn denial_by_a_later_counter_charges_no_earlier_counter(store: &dyn ObserverQuotaStore) {
    let (community, owner, agent) = (community(), key(), key());
    let mut limits = unlimited();
    limits.agent_frames_per_min = 2;
    limits.account_burst_per_sec = 1;
    let admit = |now| store.admit_telemetry(community, &owner, &agent, 1, now, &limits);

    assert!(matches!(admit(T).await.unwrap(), Decision::Allowed { .. }));
    for _ in 0..5 {
        assert_eq!(
            admit(T + 10).await.unwrap(),
            denied(LimitKind::AccountBurst, 1, 1)
        );
    }
    assert!(
        matches!(admit(T + 1_000).await.unwrap(), Decision::Allowed { .. }),
        "account-burst denials must not have spent the agent's minute budget"
    );
    assert_eq!(
        admit(T + 2_000).await.unwrap(),
        denied(LimitKind::AgentFrames, 58, 1)
    );
}

/// The N+1th distinct agent is denied until the oldest active agent has been
/// idle for 60 s; agents already holding a slot keep sending.
async fn active_agent_slots_cap_new_agents_until_one_ages_out(store: &dyn ObserverQuotaStore) {
    let (community, owner) = (community(), key());
    let (a, b, c) = (key(), key(), key());
    let mut limits = unlimited();
    limits.account_agents = 2;
    let admit = |agent, now| store.admit_telemetry(community, &owner, agent, 1, now, &limits);

    assert_eq!(
        admit(&a, T).await.unwrap(),
        Decision::Allowed { active_agents: 1 }
    );
    assert_eq!(
        admit(&b, T + 1_000).await.unwrap(),
        Decision::Allowed { active_agents: 2 }
    );
    assert_eq!(
        admit(&c, T + 2_000).await.unwrap(),
        denied(LimitKind::ActiveAgents, 58, 2),
        "a third agent waits for the oldest (a, at T) to age out"
    );
    assert_eq!(
        admit(&a, T + 30_000).await.unwrap(),
        Decision::Allowed { active_agents: 2 },
        "an agent holding a slot keeps sending and refreshes it"
    );
    assert_eq!(
        admit(&c, T + 60_000).await.unwrap(),
        denied(LimitKind::ActiveAgents, 1, 2),
        "b (at T + 1 s) is now the oldest"
    );
    assert_eq!(
        admit(&c, T + 61_000).await.unwrap(),
        Decision::Allowed { active_agents: 2 },
        "b aged out, freeing its slot"
    );
}

/// Typing admits members and up to `cap` agents per 10 s window.
async fn typing_caps_distinct_agents_per_owner(store: &dyn ObserverQuotaStore) {
    let (community, owner) = (community(), key());
    let (a, b, c) = (key(), key(), key());
    let typing = |agent, now| store.admit_typing(community, &owner, agent, now, 2);

    assert!(typing(&a, T).await.unwrap());
    assert!(typing(&b, T + 1_000).await.unwrap());
    assert!(
        !typing(&c, T + 2_000).await.unwrap(),
        "third agent is over the cap"
    );
    assert!(typing(&a, T + 5_000).await.unwrap(), "members keep typing");
    assert!(
        !typing(&c, T + 10_000).await.unwrap(),
        "b is still within 10 s"
    );
    assert!(typing(&c, T + 11_000).await.unwrap(), "b's slot expired");
    assert!(
        store
            .admit_typing(community, &key(), &c, T + 11_000, 2)
            .await
            .unwrap(),
        "caps are per owner"
    );
}

/// The same owner and agent keys in another community have their own budget.
async fn budgets_are_scoped_by_community(store: &dyn ObserverQuotaStore) {
    let (owner, agent) = (key(), key());
    let mut limits = unlimited();
    limits.agent_frames_per_min = 1;
    limits.account_agents = 1;
    let (a, b) = (community(), community());
    let admit = |community| store.admit_telemetry(community, &owner, &agent, 1, T, &limits);
    assert!(matches!(admit(a).await.unwrap(), Decision::Allowed { .. }));
    assert!(matches!(admit(a).await.unwrap(), Decision::Denied { .. }));
    assert!(
        matches!(admit(b).await.unwrap(), Decision::Allowed { .. }),
        "A's exhausted budget must not limit the same keys in B"
    );
    assert!(store.admit_typing(a, &owner, &agent, T, 1).await.unwrap());
    assert!(store.admit_typing(b, &owner, &key(), T, 1).await.unwrap());
}

#[tokio::test]
async fn memory_store_community_scope() {
    budgets_are_scoped_by_community(&MemoryObserverQuota::default()).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_community_scope() {
    budgets_are_scoped_by_community(&redis_store()).await;
}

#[tokio::test]
async fn memory_store_counter_limits() {
    every_counter_limit_allows_denies_without_charging_and_resets(&MemoryObserverQuota::default())
        .await;
}

#[tokio::test]
async fn memory_store_denial_charges_nothing() {
    denial_by_a_later_counter_charges_no_earlier_counter(&MemoryObserverQuota::default()).await;
}

#[tokio::test]
async fn memory_store_active_agent_slots() {
    active_agent_slots_cap_new_agents_until_one_ages_out(&MemoryObserverQuota::default()).await;
}

#[tokio::test]
async fn memory_store_typing_cap() {
    typing_caps_distinct_agents_per_owner(&MemoryObserverQuota::default()).await;
}

fn redis_store() -> RedisObserverQuota {
    let url = std::env::var("BUZZ_TEST_REDIS_URL").expect("explicit isolated BUZZ_TEST_REDIS_URL");
    RedisObserverQuota::new(
        deadpool_redis::Config::from_url(url)
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .expect("redis pool"),
    )
}

#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_counter_limits() {
    every_counter_limit_allows_denies_without_charging_and_resets(&redis_store()).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_denial_charges_nothing() {
    denial_by_a_later_counter_charges_no_earlier_counter(&redis_store()).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_active_agent_slots() {
    active_agent_slots_cap_new_agents_until_one_ages_out(&redis_store()).await;
}

#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_typing_cap() {
    typing_caps_distinct_agents_per_owner(&redis_store()).await;
}

/// Keys carry an expiry, so nothing outlives its window in shared Redis.
#[tokio::test]
#[ignore = "requires explicit isolated BUZZ_TEST_REDIS_URL"]
async fn redis_store_keys_expire() {
    let store = redis_store();
    let (community, owner, agent) = (community(), key(), key());
    let now = now_ms();
    store
        .admit_telemetry(community, &owner, &agent, 1, now, &unlimited())
        .await
        .unwrap();
    let plan = TelemetryPlan::new(community, &owner, &agent, 1, now, &unlimited());
    let mut conn = store.pool.get().await.unwrap();
    for key in std::iter::once(&plan.agents_key).chain(plan.counters.iter().map(|c| &c.key)) {
        let ttl: i64 = redis::cmd("PTTL")
            .arg(key)
            .query_async(&mut *conn)
            .await
            .unwrap();
        assert!(ttl > 0, "{key} must expire (pttl {ttl})");
    }
}

#[test]
fn tier_names_round_trip() {
    for tier in ObserverTier::ALL {
        assert_eq!(tier.as_str().parse::<ObserverTier>(), Ok(tier));
        assert_eq!(tier.to_string(), tier.as_str());
    }
    assert_eq!(
        ObserverTier::ALL.map(ObserverTier::as_str),
        buzz_db::observer_tier::OBSERVER_TIER_NAMES,
        "relay tiers must match the DB CHECK constraint"
    );
    assert!("Premium".parse::<ObserverTier>().is_err());
}

/// Server limits are the plan's table: executor × 1.5, rounded up.
#[test]
fn server_limits_are_executor_limits_with_headroom() {
    let config = ObserverQuotaConfig::default();
    assert_eq!(config.default_tier, ObserverTier::Premium);
    let expected = [
        (
            ObserverTier::Free,
            [
                1_500,
                90,
                8,
                6_144,
                150_000,
                30,
                450,
                30,
                750_000,
                150_000_000,
                30,
            ],
        ),
        (
            ObserverTier::Standard,
            [
                1_000,
                90,
                8,
                24_576,
                600_000,
                75,
                2_250,
                75,
                6_000_000,
                1_500_000_000,
                75,
            ],
        ),
        (
            ObserverTier::Premium,
            // The 98 304-byte frame cap is clamped to NIP-44's 65 535.
            [
                1_000,
                90,
                8,
                65_535,
                6_000_000,
                300,
                9_000,
                300,
                60_000_000,
                22_500_000_000,
                300,
            ],
        ),
    ];
    for (tier, values) in expected {
        let mut server = config.server_limits(tier);
        let actual: Vec<u64> = server.fields_mut().into_iter().map(|(_, v)| *v).collect();
        assert_eq!(actual, values, "{tier}");
    }

    let mut odd = config.clone();
    odd.headroom_pct = 101;
    odd.free.agent_frames_per_min = 3;
    assert_eq!(
        odd.server_limits(ObserverTier::Free).agent_frames_per_min,
        4
    );
}

#[test]
fn share_splits_account_limits_capped_per_agent() {
    assert_eq!(TierLimits::FREE.share(0), (60, 100_000));
    assert_eq!(TierLimits::FREE.share(1), (60, 100_000));
    assert_eq!(TierLimits::FREE.share(10), (30, 50_000));
    assert_eq!(TierLimits::PREMIUM.share(200), (30, 200_000));
}

#[test]
fn policy_carries_tier_executor_and_server_limits() {
    let config = ObserverQuotaConfig::default();
    let policy = serde_json::to_value(config.policy(ObserverTier::Standard)).unwrap();
    assert_eq!(policy["tier"], "standard");
    assert_eq!(policy["headroom_pct"], 150);
    assert_eq!(policy["conn_frames_per_sec"], 30);
    assert_eq!(policy["executor"]["agent_frame_max_bytes"], 16_384);
    assert_eq!(policy["server"]["agent_frame_max_bytes"], 24_576);
    assert_eq!(policy["server"]["account_typing_agents"], 75);
    assert_eq!(policy["executor"]["tick_ms"], 1_000);
}

/// Largest plaintext the pinned `nostr` crate encrypts (its codec stops at
/// 65 536 − 128 bytes; NIP-44 itself allows 65 535).
const CRATE_MAX_PLAINTEXT: usize = 65_408;

/// Length of the NIP-44 v2 ciphertext of a `len`-byte plaintext: real
/// encryption where the crate supports it, else the spec's encoded length
/// (base64 of version + nonce + length prefix + padded plaintext + MAC).
fn ciphertext_len(len: usize) -> usize {
    if len > CRATE_MAX_PLAINTEXT {
        return (nip44_padded_len(len) + NIP44_V2_OVERHEAD).div_ceil(3) * 4;
    }
    let (sender, recipient) = (Keys::generate(), Keys::generate());
    nip44::encrypt(
        sender.secret_key(),
        &recipient.public_key(),
        "x".repeat(len),
        nip44::Version::V2,
    )
    .unwrap()
    .len()
}

/// The ciphertext-derived bound is the exact padded length — never below the
/// real plaintext and at most NIP-44 padding above it.
#[test]
fn plaintext_bound_covers_real_nip44_ciphertexts() {
    for len in [
        1,
        31,
        32,
        33,
        257,
        300,
        1_025,
        4_096,
        16_384,
        CRATE_MAX_PLAINTEXT,
    ] {
        let bound = nip44_plaintext_upper_bound(ciphertext_len(len));
        assert!(bound >= len, "len {len}: bound {bound}");
        assert_eq!(bound, nip44_padded_len(len), "len {len}");
        assert!(bound <= len + len / 4 + 32, "len {len}: bound {bound}");
    }
    // The spec formula used above the crate's limit matches real encryption.
    assert_eq!(
        (nip44_padded_len(CRATE_MAX_PLAINTEXT) + NIP44_V2_OVERHEAD).div_ceil(3) * 4,
        ciphertext_len(CRATE_MAX_PLAINTEXT)
    );
    // A 65 535-byte plaintext produces NIP-44's largest ciphertext.
    assert_eq!(
        ciphertext_len(OBSERVER_MAX_PLAINTEXT_LEN),
        buzz_core::observer::NIP44_MAX_CONTENT_LEN
    );
    assert_eq!(
        nip44_plaintext_upper_bound(buzz_core::observer::NIP44_MAX_CONTENT_LEN),
        65_536
    );
}

/// The frame cap admits every plaintext up to the server cap — including the
/// largest NIP-44 frame (65 535 bytes) on premium — and rejects anything larger.
#[test]
fn frame_cap_admits_the_cap_and_rejects_more() {
    let config = ObserverQuotaConfig::default();
    for tier in ObserverTier::ALL {
        let server = config.server_limits(tier);
        let cap = server.agent_frame_max_bytes as usize;
        let executor_max = (config.executor_limits(tier).agent_frame_max_bytes as usize)
            .min(OBSERVER_MAX_PLAINTEXT_LEN);
        assert!(
            !frame_exceeds_cap(ciphertext_len(executor_max), &server),
            "{tier}"
        );
        assert!(!frame_exceeds_cap(ciphertext_len(cap), &server), "{tier}");
        if cap < OBSERVER_MAX_PLAINTEXT_LEN {
            assert!(
                frame_exceeds_cap(ciphertext_len(nip44_padded_len(cap) + 1), &server),
                "{tier}"
            );
        }
    }
    let premium = config.server_limits(ObserverTier::Premium);
    assert!(!frame_exceeds_cap(ciphertext_len(65_535), &premium));
    assert!(!frame_exceeds_cap(
        buzz_core::observer::NIP44_MAX_CONTENT_LEN,
        &premium
    ));
    let standard = config.server_limits(ObserverTier::Standard);
    assert!(frame_exceeds_cap(ciphertext_len(65_535), &standard));
}

/// A conforming executor — never more than its tier's executor limits in any
/// 60 s span, sending at its tick — never trips the server. Simulated for three
/// minutes from a window-misaligned start with `K` agents at full rate (`K` =
/// account ÷ agent capacity), for the largest frame (65 535 bytes on premium)
/// and for sizes NIP-44 pads the most. Frames are charged from NIP-44
/// ciphertext lengths.
#[tokio::test]
async fn conforming_executors_never_trip_server_limits() {
    let config = ObserverQuotaConfig::default();
    for tier in ObserverTier::ALL {
        let executor = *config.executor_limits(tier);
        let server = config.server_limits(tier);
        let max_frame = (executor.agent_frame_max_bytes as usize).min(OBSERVER_MAX_PLAINTEXT_LEN);
        let k = (executor.account_frames_per_min / executor.agent_frames_per_min)
            .min(executor.account_bytes_per_min / executor.agent_bytes_per_min);
        for plaintext in [max_frame, 33, 257, 1_025] {
            let content_len = ciphertext_len(plaintext);
            assert!(
                !frame_exceeds_cap(content_len, &server),
                "{tier} {plaintext}"
            );
            let charged = nip44_plaintext_upper_bound(content_len) as u64;

            let store = MemoryObserverQuota::default();
            let (community, owner) = (community(), key());
            let agents: Vec<PublicKey> = (0..k).map(|_| key()).collect();
            // Per agent: send times in the last 60 s.
            let mut sent: Vec<Vec<u64>> = vec![Vec::new(); agents.len()];
            let start = T + 12_345;
            let mut admitted = 0;
            let mut now = start;
            while now < start + 180_000 {
                for (agent, history) in agents.iter().zip(&mut sent) {
                    history.retain(|at| now - at < 60_000);
                    let frames = history.len() as u64;
                    if frames + 1 > executor.agent_frames_per_min
                        || (frames + 1) * plaintext as u64 > executor.agent_bytes_per_min
                    {
                        continue; // The executor coalesces instead of sending.
                    }
                    let decision = store
                        .admit_telemetry(community, &owner, agent, charged, now, &server)
                        .await
                        .unwrap();
                    assert!(
                        matches!(decision, Decision::Allowed { .. }),
                        "{tier} {plaintext}-byte frames at {} ms: {decision:?}",
                        now - start
                    );
                    history.push(now);
                    admitted += 1;
                }
                now += executor.tick_ms;
            }
            assert!(
                admitted >= k * 3,
                "{tier} {plaintext}: only {admitted} frames"
            );
        }
    }
}

/// Per-connection pre-verification cap: `conn_frames_per_sec` per second per
/// connection, reset each second, independent across connections.
#[tokio::test]
async fn connection_cap_is_per_connection_per_second() {
    let state = crate::state::tests::test_state().await;
    let cap = state.config.observer_quota.conn_frames_per_sec;
    let (one, two) = (Uuid::new_v4(), Uuid::new_v4());
    for _ in 0..cap {
        assert!(conn_frame_admitted_at(&state, one, 100));
    }
    assert!(!conn_frame_admitted_at(&state, one, 100));
    assert!(conn_frame_admitted_at(&state, two, 100));
    assert!(conn_frame_admitted_at(&state, one, 101));
}

#[test]
fn telemetry_frames_are_recognised_by_kind_and_frame_tag() {
    let event = |kind: u16, frame: &str| {
        nostr::EventBuilder::new(nostr::Kind::Custom(kind), "")
            .tags([nostr::Tag::parse([OBSERVER_FRAME_TAG, frame]).unwrap()])
            .sign_with_keys(&Keys::generate())
            .unwrap()
    };
    assert!(is_telemetry_frame(&event(24200, OBSERVER_FRAME_TELEMETRY)));
    assert!(!is_telemetry_frame(&event(
        24200,
        buzz_core::observer::OBSERVER_FRAME_CONTROL
    )));
    assert!(!is_telemetry_frame(&event(20002, OBSERVER_FRAME_TELEMETRY)));
}

/// Tier resolution and the policy endpoint's caller rule, through the
/// production lookups with their caches primed (no database).
#[tokio::test]
async fn caller_policy_uses_owner_override_else_default() {
    let state = crate::state::tests::test_state().await;
    let community = community();
    let (owner, agent, human) = (key(), key(), key());
    state
        .agent_owner_cache
        .insert((community, agent.to_bytes()), Some(owner.to_bytes()));
    state
        .agent_owner_cache
        .insert((community, human.to_bytes()), None);
    state
        .agent_owner_cache
        .insert((community, owner.to_bytes()), None);
    state
        .observer_tier_cache
        .insert((community, owner.to_bytes()), Some(ObserverTier::Free));
    state
        .observer_tier_cache
        .insert((community, human.to_bytes()), None);

    let policy = |caller| crate::api::observer::caller_policy(&state, community, caller);
    let agent_policy = policy(&agent).await.unwrap();
    assert_eq!(
        agent_policy.tier,
        ObserverTier::Free,
        "agents inherit the owner's tier"
    );
    assert_eq!(agent_policy.server.account_agents, 30);
    assert_eq!(policy(&owner).await.unwrap().tier, ObserverTier::Free);
    assert_eq!(
        policy(&human).await.unwrap().tier,
        state.config.observer_quota.default_tier,
        "no override uses the configured default"
    );
}

// Discovered by the isolated PostgreSQL lane.
mod postgres_tests {
    use super::*;

    /// The same rule against PostgreSQL: the override row, then its removal.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn caller_policy_reads_observer_tier_rows() {
        let state =
            crate::state::tests::test_state_with_database_url(&crate::test_support::database_url())
                .await;
        if std::env::var("BUZZ_TEST_SCHEMA_MODE").as_deref() != Ok("desired") {
            state.db.migrate().await.expect("migrate test DB");
        }
        let community = Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community)
            .bind(format!("observer-policy-{}.example", community.simple()))
            .execute(state.db.pool())
            .await
            .expect("insert community");
        let community = CommunityId::from_uuid(community);
        let (owner, agent) = (key(), key());
        for user in [&owner, &agent] {
            state
                .db
                .ensure_user(community, &user.to_bytes())
                .await
                .unwrap();
        }
        state
            .db
            .set_agent_owner(community, &agent.to_bytes(), &owner.to_bytes())
            .await
            .unwrap();
        state
            .db
            .set_observer_tier(community, &owner.to_bytes(), "standard")
            .await
            .unwrap();

        let policy = crate::api::observer::caller_policy(&state, community, &agent)
            .await
            .unwrap();
        assert_eq!(policy.tier, ObserverTier::Standard);

        assert!(state
            .db
            .clear_observer_tier(community, &owner.to_bytes())
            .await
            .unwrap());
        state.observer_tier_cache.invalidate_all();
        assert_eq!(
            crate::api::observer::caller_policy(&state, community, &agent)
                .await
                .unwrap()
                .tier,
            state.config.observer_quota.default_tier
        );
    }
}

/// The typing gate the ephemeral handler calls: agents (owner from the
/// session) are capped per owner, non-agents pass, and a store failure admits
/// the indicator rather than blocking typing.
#[tokio::test]
async fn typing_gate_caps_agents_per_owner_and_fails_open() {
    let redis_backed = crate::state::tests::test_state().await; // Unreachable Redis.
    let mut state = (*redis_backed).clone();
    let mut config = (*state.config).clone();
    config.observer_quota.default_tier = ObserverTier::Free;
    config.observer_quota.free.account_typing_agents = 1; // Server cap: 2.
    state.config = Arc::new(config);
    state.observer_quota = Arc::new(MemoryObserverQuota::default());

    let owner = key();
    let conn = |pubkey: PublicKey, agent_owner_pubkey: Option<PublicKey>| {
        crate::connection::tests::test_conn_with_auth(AuthState::Authenticated(
            buzz_auth::AuthContext {
                pubkey,
                scopes: Vec::new(),
                channel_ids: None,
                auth_method: buzz_auth::AuthMethod::Nip42,
                agent_owner_pubkey,
                token: None,
            },
        ))
        .0
    };
    let (a, b, c, human) = (key(), key(), key(), key());
    let community = conn(a, Some(owner)).tenant.community();
    for state in [&state, &*redis_backed] {
        state
            .observer_tier_cache
            .insert((community, owner.to_bytes()), None);
    }
    state
        .agent_owner_cache
        .insert((community, human.to_bytes()), None);

    assert!(typing_admitted(&state, &conn(a, Some(owner)), &a).await);
    assert!(typing_admitted(&state, &conn(b, Some(owner)), &b).await);
    assert!(
        !typing_admitted(&state, &conn(c, Some(owner)), &c).await,
        "a third agent of the owner exceeds the free cap of 2"
    );
    assert!(typing_admitted(&state, &conn(a, Some(owner)), &a).await);
    assert!(
        typing_admitted(&state, &conn(human, None), &human).await,
        "non-agents are never capped"
    );
    assert!(
        typing_admitted(&redis_backed, &conn(c, Some(owner)), &c).await,
        "an unavailable store admits typing"
    );
}
