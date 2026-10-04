use super::*;

#[test]
fn limiter_caps_three_per_window_and_resets() {
    let mut limiter = RestartLimiter::default();
    let t0 = Instant::now();
    assert_eq!(limiter.record("a", t0), RestartDecision::Restart);
    assert_eq!(
        limiter.record("a", t0 + Duration::from_secs(10)),
        RestartDecision::Restart
    );
    assert_eq!(
        limiter.record("a", t0 + Duration::from_secs(20)),
        RestartDecision::Restart
    );
    assert_eq!(
        limiter.record("a", t0 + Duration::from_secs(30)),
        RestartDecision::Capped
    );
    // Other agents are independent.
    assert_eq!(limiter.record("b", t0), RestartDecision::Restart);
    // The window slides: 10 min after the first failure one slot frees.
    assert_eq!(
        limiter.record("a", t0 + RESTART_WINDOW + Duration::from_secs(1)),
        RestartDecision::Restart
    );
    limiter.reset("a");
    assert_eq!(
        limiter.record("a", t0 + Duration::from_secs(31)),
        RestartDecision::Restart
    );
}

#[test]
fn runtime_parks_at_cap_and_manual_restart_unparks() {
    let bots = BotRuntime::default();
    let now = Instant::now();
    for _ in 0..RESTART_CAP {
        assert_eq!(bots.record_auth_exit("a", now), RestartDecision::Restart);
        assert!(bots.auth_failed("a").is_none());
    }
    assert_eq!(bots.record_auth_exit("a", now), RestartDecision::Capped);
    assert!(bots.auth_failed("a").is_some());
    bots.reset("a");
    assert!(bots.auth_failed("a").is_none());
    assert_eq!(bots.record_auth_exit("a", now), RestartDecision::Restart);
}

#[test]
fn exit_queue_takes_only_code_78() {
    let mine = |entry: &(String, String)| entry.1 == "ws://exit-queue-test";
    note_exit("a", "ws://exit-queue-test", Some(1));
    note_exit("a", "ws://exit-queue-test", None);
    note_exit("b", "ws://exit-queue-test", Some(EXIT_AUTH_TERMINAL));
    note_exit("b", "ws://exit-queue-test", Some(EXIT_AUTH_TERMINAL));
    assert_eq!(
        take_exits_matching(mine),
        vec![("b".to_string(), "ws://exit-queue-test".to_string())]
    );
    assert!(take_exits_matching(mine).is_empty());
}

#[test]
fn issued_token_is_single_use_and_origin_bound() {
    let bots = BotRuntime::default();
    bots.put(
        "a",
        IssuedBotToken {
            origin: "http://one".into(),
            bot_id: "b".repeat(64),
            token: Zeroizing::new("bzb_x".into()),
            expires_at: Some(1),
        },
    );
    assert!(bots.take("a", "http://two").is_none());
    assert!(bots.take("a", "http://one").is_some());
    assert!(bots.take("a", "http://one").is_none());
}

fn env_of(command: &std::process::Command) -> HashMap<String, Option<String>> {
    command
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

#[test]
fn token_spawn_env_has_bot_token_and_no_key() {
    let mut command = std::process::Command::new("buzz-acp");
    let auth = SpawnAuth::Token(IssuedBotToken {
        origin: "o".into(),
        bot_id: "b".into(),
        token: Zeroizing::new("bzb_live".into()),
        expires_at: Some(1_900_000_000),
    });
    apply_spawn_auth(&mut command, &auth, "nsec1secret", Some("[\"auth\"]"));
    let env = env_of(&command);
    assert_eq!(env["BUZZ_BOT_TOKEN"].as_deref(), Some("bzb_live"));
    assert_eq!(
        env["BUZZ_BOT_TOKEN_EXPIRES_AT"].as_deref(),
        Some("1900000000")
    );
    assert_eq!(env["BUZZ_PRIVATE_KEY"], None, "key must be removed");
    assert_eq!(env["BUZZ_AUTH_TAG"], None);
    for key in [
        "BUZZ_ACCESS_TOKEN",
        "BUZZ_TOKEN_BROKER_URL",
        "BUZZ_TOKEN_BROKER_SECRET",
    ] {
        assert_eq!(env[key], None, "{key} must be stripped");
    }
}

#[test]
fn key_spawn_env_is_unchanged_and_strips_token_vars() {
    let mut command = std::process::Command::new("buzz-acp");
    apply_spawn_auth(&mut command, &SpawnAuth::Keys, "nsec1secret", None);
    let env = env_of(&command);
    assert_eq!(env["BUZZ_PRIVATE_KEY"].as_deref(), Some("nsec1secret"));
    assert_eq!(env["BUZZ_AUTH_TAG"], None);
    assert_eq!(env["BUZZ_BOT_TOKEN"], None);
}

// ── adoption ───────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn keyed_record(keys: &nostr::Keys) -> ManagedAgentRecord {
    let mut record: ManagedAgentRecord = serde_json::from_str(
        r#"{
            "pubkey": "00",
            "name": "Scout",
            "private_key_nsec": "",
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
        }"#,
    )
    .unwrap();
    record.pubkey = keys.public_key().to_hex();
    record.private_key_nsec = nostr::ToBech32::to_bech32(keys.secret_key()).unwrap();
    record.auth_tag = Some("[\"auth\",\"x\",\"\",\"y\"]".into());
    record
}

fn bot_record(origin: &str) -> ManagedAgentRecord {
    let mut record = keyed_record(&nostr::Keys::generate());
    let bot = nostr::Keys::generate().public_key().to_hex();
    adopt_record_identity(&mut record, &bot, origin);
    record
}

/// In-memory store + relay: the real `adopt` runs against it.
struct FakeEffects {
    origin: String,
    records: Mutex<Vec<ManagedAgentRecord>>,
    running: bool,
    creates: AtomicUsize,
    fail_rewrites: AtomicUsize,
}

impl FakeEffects {
    fn new(origin: &str, records: Vec<ManagedAgentRecord>) -> Self {
        Self {
            origin: origin.into(),
            records: Mutex::new(records),
            running: false,
            creates: AtomicUsize::new(0),
            fail_rewrites: AtomicUsize::new(0),
        }
    }
}

impl AdoptionEffects for FakeEffects {
    fn is_bot(&self, pubkey: &str) -> Result<bool, String> {
        let records = self.records.lock().unwrap();
        let record = records
            .iter()
            .find(|r| r.pubkey == pubkey)
            .ok_or("missing")?;
        Ok(record.bot_origin.as_deref() == Some(self.origin.as_str()))
    }
    fn is_running(&self, _: &str) -> bool {
        self.running
    }
    fn create_bot(&self) -> Result<String, String> {
        // Widen the race window for the concurrency test.
        std::thread::sleep(Duration::from_millis(30));
        self.creates.fetch_add(1, Ordering::SeqCst);
        Ok(nostr::Keys::generate().public_key().to_hex())
    }
    fn delete_bot(&self, _: &str) {}
    fn rewrite(&self, from: &str, to: &str) -> Result<bool, String> {
        if self.fail_rewrites.load(Ordering::SeqCst) > 0 {
            self.fail_rewrites.fetch_sub(1, Ordering::SeqCst);
            return Err("disk full".into());
        }
        let mut records = self.records.lock().unwrap();
        let record = records
            .iter_mut()
            .find(|r| r.pubkey == from)
            .ok_or("missing")?;
        adopt_record_identity(record, to, &self.origin);
        Ok(true)
    }
}

fn unique_origin(tag: &str) -> String {
    format!(
        "http://{tag}-{}.test",
        nostr::Keys::generate().public_key().to_hex()
    )
}

#[test]
fn adoption_rewrites_pubkey_and_drops_the_local_key() {
    let dir = tempfile::tempdir().unwrap();
    let origin = unique_origin("rewrite");
    let keys = nostr::Keys::generate();
    let local = keys.public_key().to_hex();
    let effects = FakeEffects::new(&origin, vec![keyed_record(&keys)]);
    let Adoption::Bot(bot_id) = adopt(dir.path(), &origin, &local, false, &effects).unwrap() else {
        panic!("expected adoption");
    };
    let records = effects.records.lock().unwrap();
    assert_eq!(records[0].pubkey, bot_id, "the record IS the bot now");
    assert_ne!(bot_id, local);
    assert!(records[0].private_key_nsec.is_empty(), "local key dropped");
    assert_eq!(records[0].auth_tag, None, "NIP-OA tag dropped");
    assert_eq!(records[0].bot_origin.as_deref(), Some(origin.as_str()));
    drop(records);
    // Already a bot: a second adoption is a no-op lookup.
    assert_eq!(
        adopt(dir.path(), &origin, &bot_id, false, &effects).unwrap(),
        Adoption::Bot(bot_id)
    );
    assert_eq!(effects.creates.load(Ordering::SeqCst), 1);
}

#[test]
fn concurrent_adoption_registers_one_bot() {
    let dir = Arc::new(tempfile::tempdir().unwrap());
    let origin = unique_origin("concurrent");
    let keys = nostr::Keys::generate();
    let local = keys.public_key().to_hex();
    let effects = Arc::new(FakeEffects::new(&origin, vec![keyed_record(&keys)]));
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let (dir, origin, local, effects) =
                (dir.clone(), origin.clone(), local.clone(), effects.clone());
            std::thread::spawn(move || {
                adopt(dir.path(), &origin, &local, false, effects.as_ref()).unwrap()
            })
        })
        .collect();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(effects.creates.load(Ordering::SeqCst), 1, "one server bot");
    assert!(outcomes.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn failed_rewrite_resumes_with_the_journaled_bot() {
    let dir = tempfile::tempdir().unwrap();
    let origin = unique_origin("resume");
    let keys = nostr::Keys::generate();
    let local = keys.public_key().to_hex();
    let effects = FakeEffects::new(&origin, vec![keyed_record(&keys)]);
    effects.fail_rewrites.store(1, Ordering::SeqCst);
    assert!(adopt(dir.path(), &origin, &local, false, &effects).is_err());
    let journaled = journal_get(dir.path(), &origin, &local).unwrap().unwrap();
    let outcome = adopt(dir.path(), &origin, &local, false, &effects).unwrap();
    assert_eq!(outcome, Adoption::Bot(journaled));
    assert_eq!(effects.creates.load(Ordering::SeqCst), 1, "no second bot");
    assert_eq!(journal_get(dir.path(), &origin, &local).unwrap(), None);
}

#[test]
fn a_running_agent_is_not_moved() {
    let dir = tempfile::tempdir().unwrap();
    let origin = unique_origin("running");
    let keys = nostr::Keys::generate();
    let mut effects = FakeEffects::new(&origin, vec![keyed_record(&keys)]);
    effects.running = true;
    let local = keys.public_key().to_hex();
    assert_eq!(
        adopt(dir.path(), &origin, &local, false, &effects).unwrap(),
        Adoption::Running
    );
    assert_eq!(effects.creates.load(Ordering::SeqCst), 0);
    assert_eq!(effects.records.lock().unwrap()[0].pubkey, local);
}

#[test]
fn replacing_a_rejected_bot_moves_the_record_again() {
    let dir = tempfile::tempdir().unwrap();
    let origin = unique_origin("replace");
    let record = bot_record(&origin);
    let old = record.pubkey.clone();
    let effects = FakeEffects::new(&origin, vec![record]);
    let Adoption::Bot(new) = adopt(dir.path(), &origin, &old, true, &effects).unwrap() else {
        panic!("expected replacement");
    };
    assert_ne!(new, old);
    assert_eq!(effects.records.lock().unwrap()[0].pubkey, new);
}

#[test]
fn key_mode_spawn_is_unchanged_and_a_bot_never_falls_back_to_keys() {
    let state = crate::app_state::build_app_state();
    let keyed = keyed_record(&nostr::Keys::generate());
    assert!(matches!(
        spawn_auth(&state, &keyed, "ws://keys.test").unwrap(),
        SpawnAuth::Keys
    ));
    let bot = bot_record("http://keys.test");
    let error = spawn_auth(&state, &bot, "ws://keys.test")
        .err()
        .expect("signed out: refused");
    assert!(error.contains("Sign in with Google"), "{error}");
    let error = spawn_auth(&state, &bot, "ws://other.test")
        .err()
        .expect("other community: refused");
    assert!(error.contains("http://keys.test"), "{error}");
}

#[test]
fn desktop_never_signs_agent_messages_locally_in_token_mode() {
    let keys = nostr::Keys::generate();
    let keyed = keyed_record(&keys);
    // Key mode: the agent's own key, as before.
    assert_eq!(
        managed_agent_message_keys(&keyed, false)
            .unwrap()
            .public_key(),
        keys.public_key()
    );
    // Token mode: refused even though a valid local key exists.
    assert!(managed_agent_message_keys(&keyed, true).is_err());
    // A bot record has no key to sign with in any mode.
    assert!(managed_agent_message_keys(&bot_record("http://o.test"), false).is_err());
}

#[tokio::test]
async fn server_delete_runs_before_and_gates_local_delete() {
    let ran = AtomicUsize::new(0);
    let failed: Result<(), String> =
        server_then_local(async { Err("relay down".to_string()) }, || async {
            ran.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(ran.load(Ordering::SeqCst), 0, "local delete must not run");
    let order = Mutex::new(Vec::new());
    server_then_local(
        async {
            order.lock().unwrap().push("server");
            Ok(())
        },
        || async {
            order.lock().unwrap().push("local");
            Ok::<_, String>(())
        },
    )
    .await
    .unwrap();
    assert_eq!(*order.lock().unwrap(), vec!["server", "local"]);
}

/// One-shot HTTP server answering `status` to the first request.
async fn serve_once(status: u16) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let body = r#"{"error":"x","message":"x"}"#;
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    format!("http://{addr}")
}

fn signed_in(state: &AppState, origin: &str) {
    state.token_auth.set(
        origin,
        super::super::OriginAuth::Active(UserSession {
            principal: nostr::Keys::generate().public_key(),
            device_id: None,
            access: Zeroizing::new("bzs_test".into()),
            refresh: Zeroizing::new("bzr_test".into()),
            access_issued_at: 0,
            access_expires_at: i64::MAX,
        }),
    );
}

#[tokio::test]
async fn server_bot_delete_failure_keeps_the_agent() {
    let state = crate::app_state::build_app_state();
    let origin = serve_once(500).await;
    signed_in(&state, &origin);
    assert!(delete_server_bot(&state, &bot_record(&origin))
        .await
        .is_err());

    let origin = serve_once(404).await;
    signed_in(&state, &origin);
    delete_server_bot(&state, &bot_record(&origin))
        .await
        .expect("already gone counts as deleted");

    // Key agents never call the relay.
    delete_server_bot(&state, &keyed_record(&nostr::Keys::generate()))
        .await
        .unwrap();
}

// ── move steps, resolution, sweep (N1, crash window) ──────────────────────

/// Records every [`MoveOps`] step; real `rewrite_record`/`sweep_journal` run
/// against it.
#[derive(Default)]
struct RecordingOps {
    log: Mutex<Vec<String>>,
    fail_save: bool,
    fail_key: bool,
}

impl RecordingOps {
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl MoveOps for RecordingOps {
    fn move_profile(&self, from: &str, to: &str) {
        self.log
            .lock()
            .unwrap()
            .push(format!("profile {from}->{to}"));
    }
    fn save_record(&self, from: &str, to: &str) -> Result<(), String> {
        if self.fail_save {
            return Err("disk full".into());
        }
        self.log.lock().unwrap().push(format!("save {from}->{to}"));
        Ok(())
    }
    fn forget_runtime(&self, from: &str) {
        self.log.lock().unwrap().push(format!("forget {from}"));
    }
    fn drop_local_key(&self, from: &str) -> Result<(), String> {
        if self.fail_key {
            return Err("keyring locked".into());
        }
        self.log.lock().unwrap().push(format!("key {from}"));
        Ok(())
    }
    fn announce(&self, to: &str) {
        self.log.lock().unwrap().push(format!("announce {to}"));
    }
}

#[test]
fn rewrite_moves_the_profile_before_the_record_and_announces_the_change() {
    let ops = RecordingOps::default();
    assert!(rewrite_record(&ops, "old", "bot").unwrap());
    assert_eq!(
        ops.log(),
        vec![
            "profile old->bot",
            "save old->bot",
            "forget old",
            "key old",
            // The UI refetches (agents-data-changed) only after the record
            // is saved under its new pubkey.
            "announce bot",
        ]
    );
}

#[test]
fn a_failed_record_save_moves_the_profile_back_and_keeps_the_key() {
    let ops = RecordingOps {
        fail_save: true,
        ..RecordingOps::default()
    };
    assert!(rewrite_record(&ops, "old", "bot").is_err());
    assert_eq!(ops.log(), vec!["profile old->bot", "profile bot->old"]);
}

/// Adoption whose rewrite reports the old key as still present.
struct KeyKept(FakeEffects);

impl AdoptionEffects for KeyKept {
    fn is_bot(&self, pubkey: &str) -> Result<bool, String> {
        self.0.is_bot(pubkey)
    }
    fn is_running(&self, pubkey: &str) -> bool {
        self.0.is_running(pubkey)
    }
    fn create_bot(&self) -> Result<String, String> {
        self.0.create_bot()
    }
    fn delete_bot(&self, bot_id: &str) {
        self.0.delete_bot(bot_id)
    }
    fn rewrite(&self, from: &str, to: &str) -> Result<bool, String> {
        self.0.rewrite(from, to).map(|_| false)
    }
}

#[test]
fn a_kept_local_key_leaves_the_journal_entry_for_the_sweep() {
    let ops = RecordingOps {
        fail_key: true,
        ..RecordingOps::default()
    };
    assert!(!rewrite_record(&ops, "old", "bot").unwrap());
    assert_eq!(ops.log().last().map(String::as_str), Some("announce bot"));

    let dir = tempfile::tempdir().unwrap();
    let origin = unique_origin("key-kept");
    let keys = nostr::Keys::generate();
    let local = keys.public_key().to_hex();
    let effects = KeyKept(FakeEffects::new(&origin, vec![keyed_record(&keys)]));
    let Adoption::Bot(bot) = adopt(dir.path(), &origin, &local, false, &effects).unwrap() else {
        panic!("expected adoption");
    };
    assert_eq!(journal_get(dir.path(), &origin, &local).unwrap(), Some(bot));
}

#[test]
fn ui_actions_holding_the_old_pubkey_reach_the_moved_record() {
    let dir = tempfile::tempdir().unwrap();
    let origin = unique_origin("resolve");
    let keys = nostr::Keys::generate();
    let local = keys.public_key().to_hex();
    let effects = FakeEffects::new(&origin, vec![keyed_record(&keys)]);
    let Adoption::Bot(bot) = adopt(dir.path(), &origin, &local, false, &effects).unwrap() else {
        panic!("expected adoption");
    };
    assert_eq!(current_agent_pubkey(&local), bot);
    assert_eq!(current_agent_pubkey(&local.to_uppercase()), bot);
    // The production record lookup Start/Stop/Edit/Delete go through.
    let mut records = effects.records.lock().unwrap().clone();
    let found = crate::managed_agents::find_managed_agent_mut(&mut records, &local).unwrap();
    assert_eq!(found.pubkey, bot);
    // A replaced bot is followed to the latest id.
    let Adoption::Bot(newer) = adopt(dir.path(), &origin, &bot, true, &effects).unwrap() else {
        panic!("expected replacement");
    };
    assert_eq!(current_agent_pubkey(&local), newer);
    // Unrelated pubkeys pass through.
    let other = nostr::Keys::generate().public_key().to_hex();
    assert_eq!(current_agent_pubkey(&other), other);
    assert!(crate::managed_agents::find_managed_agent_mut(&mut records, &other).is_err());
}

#[test]
fn sweep_settles_crash_leftovers_by_where_the_record_is() {
    let dir = tempfile::tempdir().unwrap();
    let origin = unique_origin("sweep");
    let unsaved = nostr::Keys::generate().public_key().to_hex();
    let saved = nostr::Keys::generate().public_key().to_hex();
    let deleted = nostr::Keys::generate().public_key().to_hex();
    journal_set(dir.path(), &origin, &unsaved, Some("bot-a")).unwrap();
    journal_set(dir.path(), &origin, &saved, Some("bot-b")).unwrap();
    journal_set(dir.path(), &origin, &deleted, Some("bot-c")).unwrap();
    let records: HashSet<String> = [unsaved.clone(), "bot-b".to_owned()].into();
    let ops = RecordingOps::default();
    sweep_journal(dir.path(), |pubkey| records.contains(pubkey), &ops).unwrap();
    let log = ops.log();
    // Crash before the record save: the profile goes back to the record and
    // the entry stays, so the next adoption resumes with bot-a.
    assert!(
        log.contains(&format!("profile bot-a->{unsaved}")),
        "{log:?}"
    );
    assert!(!log.contains(&format!("key {unsaved}")), "{log:?}");
    assert_eq!(
        journal_get(dir.path(), &origin, &unsaved)
            .unwrap()
            .as_deref(),
        Some("bot-a")
    );
    // Crash after the save: the profile follows the record and the old key
    // goes; the entry is settled and the old pubkey resolves.
    assert!(log.contains(&format!("profile {saved}->bot-b")), "{log:?}");
    assert!(log.contains(&format!("key {saved}")), "{log:?}");
    assert_eq!(journal_get(dir.path(), &origin, &saved).unwrap(), None);
    assert_eq!(current_agent_pubkey(&saved), "bot-b");
    // Deleted since: only the key is cleaned up.
    assert!(log.contains(&format!("key {deleted}")), "{log:?}");
    assert_eq!(journal_get(dir.path(), &origin, &deleted).unwrap(), None);
}

#[test]
fn sweep_keeps_the_entry_while_the_old_key_cannot_be_removed() {
    let dir = tempfile::tempdir().unwrap();
    let origin = unique_origin("sweep-locked");
    let saved = nostr::Keys::generate().public_key().to_hex();
    journal_set(dir.path(), &origin, &saved, Some("bot-d")).unwrap();
    let ops = RecordingOps {
        fail_key: true,
        ..RecordingOps::default()
    };
    sweep_journal(dir.path(), |pubkey| pubkey == "bot-d", &ops).unwrap();
    assert_eq!(
        journal_get(dir.path(), &origin, &saved).unwrap().as_deref(),
        Some("bot-d")
    );
}

#[test]
fn a_bot_of_another_community_is_refused_by_name() {
    let here = "http://here.test";
    let foreign = bot_record("http://there.test");
    let reason = foreign_bot_refusal(&foreign, here).expect("refused");
    assert!(reason.contains("http://there.test"), "{reason}");
    assert_eq!(foreign_bot_refusal(&bot_record(here), here), None);
    assert_eq!(
        foreign_bot_refusal(&keyed_record(&nostr::Keys::generate()), here),
        None
    );
}

#[test]
fn a_running_key_agent_after_sign_in_needs_a_restart() {
    let keyed = keyed_record(&nostr::Keys::generate());
    let entry = key_agent_move_pending(&keyed, true, true).expect("restart required");
    assert_eq!(entry.field, "sign_in");
    assert!(
        key_agent_move_pending(&keyed, false, true).is_none(),
        "stopped"
    );
    assert!(
        key_agent_move_pending(&keyed, true, false).is_none(),
        "key mode"
    );
    assert!(
        key_agent_move_pending(&bot_record("http://o.test"), true, true).is_none(),
        "already a bot"
    );
}

#[test]
fn delete_fence_blocks_starts_and_drops_queued_auth_exits() {
    let agent = nostr::Keys::generate().public_key().to_hex();
    note_exit(&agent, "ws://fence.test", Some(EXIT_AUTH_TERMINAL));
    {
        let _fence = DeleteFence::begin(&agent);
        assert!(is_being_deleted(&agent));
        assert!(take_exits_matching(|(queued, _)| queued == &agent).is_empty());
    }
    assert!(!is_being_deleted(&agent), "released when the delete ends");
}
