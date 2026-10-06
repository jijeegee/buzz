# Google key recovery: implementation and validation

Date: 2026-10-05. Baseline: `10f4f76d61498516a00c4b2ef847da2b47132571`.
Contract: [Google key recovery](google-key-recovery.md), authored before the
implementation and explicitly superseding conflicting sections of
[centralized identity](centralized-identity.md).

## Implemented paths

| Area | Paths and behavior |
|---|---|
| Cryptography | `crates/buzz-auth/src/key_backup.rs`, `key_backup/tests.rs`, crate manifest/export: AES-256-GCM envelope, random nonces, version/account/pubkey/key-id AAD, redacted/zeroizing secrets, signed link-proof validation. |
| Persistence | `crates/buzz-db/src/store/identity/{principal,key_backup,mod}.rs`, `key_backup_postgres_tests.rs`: immutable account mode, provider/subject identity, transactional first-device initialization, unique pubkey binding, session recency. |
| Schema | `migrations/0059_custodial_key_backup.sql`, `schema/schema.sql`, `scripts/reconcile-schema-after-pgschema.sql`, DB migration/partition test expectations: both bootstrap paths and operator-global registry reconciliation. |
| Relay | `crates/buzz-relay/src/{identity,api/auth}/`, `router.rs`, `nip11.rs`, crate manifest: independent custody feature flag, OIDC mode propagation, authorized challenge/backup/restore, export freshness/rates, no-store including rejection responses, token messaging/bot exclusion. |
| Desktop | `desktop/src-tauri/src/auth/`, `app_state{,_tests}.rs`, `commands/identity.rs`, `relay/submit.rs`, managed-agent spawn tests: strict OS-keyring recovery, protected established identities, signed credential selection, stable refresh and agents. Onboarding/Settings and shared auth API under `desktop/src/` expose the real flow. |
| Mobile | `mobile/lib/shared/auth/`, `shared/community/`, sign-in/Settings and invite joining: secure pending key, status-first initialization/recovery, separate custody sessions, preserved signed communities, existing-key linking, same-key invite claims. Tests under corresponding `mobile/test/` paths. |
| Web/CLI | `web/src/shared/auth/`, `crates/buzz-cli/src/{commands/auth*,auth_loopback.rs}`: preserve supported signed and legacy token paths; request explicit token mode and reject unsupported custody adoption with an actionable explanation. |
| Operations/design | `.env.example`, both design documents and this report. `Cargo.lock` records the two added direct dependencies already present in the dependency graph. |

Google sessions authorize recovery storage. The original client public key
continues to sign messages, identify owners and agents, authorize NIP-OA
siblings, resolve mentions/allowlists and key conversation mappings. No agent
recreation, token-bot adoption, key replacement, account merging or automatic
conversion of existing token principals is part of this flow.

## Test-driven slices

The following failures were observed before implementing their production
seams, then rerun successfully:

- Crypto and database tests initially failed to compile against the required
  missing production APIs; implementations then passed round-trip, binding,
  corruption, proof, concurrency and refresh tests.
- The relay configuration harness executed the original configuration code
  and failed because it ignored custody enablement; custody validation and
  independent session routing then passed.
- Web/CLI tests failed when login omitted the identity mode or accepted a
  custody response as a token messaging principal; explicit mode and rejection
  guards made them pass.
- Desktop's Google completion test failed because completion always reloaded
  the app; it now reloads only when a permitted initial restore changes the
  signer. Same-key linking/renewal preserves the running app and agents.
- Mobile tests initially failed on missing production service interfaces.
  A later invite regression failed with `switchedExisting` instead of
  `confirming`; recovered communities now claim the invitation with their
  existing NIP-98 signer before switching.
- Final compatibility review exposed missing existing-token-account UI on
  dual-mode relays. Mobile's two actual widget regressions failed before the
  explicit route was added. Desktop's production login-mode, logout/restart
  and component regressions also failed before their fixes. Known backup
  bindings take precedence over discovery or an attempted token downgrade.
- Mobile's missing-local-key tests failed before preserving the public-key
  boundary at startup and restoring only a matching backup into the existing
  community record. Final tests verify persistence as well as the returned key.
- Desktop's lost-owner regression failed before the final persistence path
  checked surviving backup bindings and verified NIP-OA owner attestations.
- The explicit desktop compatibility-route regression also failed before its
  pre-login guard and final agent-store-locked recheck prevented adoption of
  existing key agents. Existing token bots remain usable and record JSON is
  unchanged.

## Executed checks

Commands below ran in the original worktree using installed native tools and
existing caches. `cargo` was invoked by its absolute Windows path
`C:/Users/jijee/.cargo/bin/cargo.exe`. No production OAuth was mocked outside
test code, and no real Google acceptance is implied.

| Command / check | Observed result |
|---|---|
| `cargo test -p buzz-auth --lib --offline` | 267 passed, including all 4 new crypto/proof tests. |
| `cargo test -p buzz-db key_backup --offline -- --ignored --test-threads=1` | 3 passed against isolated local Postgres; repeated after current changes against the migration-created database. |
| DB test executable, `runtime::migration::postgres_tests:: --test-threads=1` | 14 passed, 9 explicitly ignored infrastructure tests. The earlier `...::tests` filter matched zero tests and is not counted as validation. |
| `cargo test -p buzz-relay --lib custody --offline` | 6 passed; 4 live tests ignored by this non-live invocation and exercised separately below. |
| Relay test executable, `api::auth::router_tests:: --test-threads=1` | 8 passed, including no-store on body-limit errors and unknown/disabled auth routes. |
| `cargo test -p buzz-relay --lib api::auth::live_tests::key_backup --offline -- --ignored --test-threads=1` | All 6 live HTTP/Postgres/Redis tests passed. Google provider alone is mocked. |
| Relay live tests: `oidc_login_ws_draft_and_reads_round_trip`, `refresh_rotation_replay_and_reuse_detection`, `every_revocation_path_revokes_its_tokens`, each `--exact --ignored --test-threads=1` | All 3 passed against the same isolated services, covering retained legacy token login/messages, renewal and revocation. |
| `cargo test -p buzz-cli commands::auth::tests --offline` | 29 passed, including rejecting custody before storing a token identity. |
| `cargo test -p buzz-sdk nip_oa --offline` | 24 passed. |
| `cargo test -p buzz-acp --lib test_allowlist --offline -- --test-threads=1` | 4 passed. |
| Fresh ACP test executable: `author_gate_tests`, `scope::tests`, `conversation_context_delta`, `two_threads_in_one_channel_get_distinct_sessions`, `idle_channel_switch_preserves_all_sibling_sessions_and_model` | Respectively 28, 13, 4, 1 and 1 passed: owner/sibling/allowlist/DM gates, mentions/thread scope, conversation and sibling session continuity. |
| `cargo clippy -p buzz-auth -p buzz-db -p buzz-relay -p buzz-cli --all-targets --offline -- -D warnings` | Passed, including final rerun. |
| `cargo fmt --all -- --check` | Passed. |
| `cargo test --manifest-path desktop/src-tauri/Cargo.toml --lib auth:: --offline` | Final 83/83 passed; includes login-mode selection, logout/restart, mismatched completion, lost-owner recovery and key-agent preservation regressions. Existing Windows warnings remain. |
| Desktop fresh test executable: `app_state::tests`, `respond_to` | 49 and 17 passed; keyring persistence and agent routing. |
| Desktop later `google_backup_session` regression run | 2 passed, including preserved legacy token-bot conflict handling. |
| Desktop Node tests with `--test-isolation=none` | Final 4 shared auth/reload tests and 8 actual Settings/onboarding component tests passed. |
| Web `node --test-isolation=none --experimental-strip-types --test 'src/shared/auth/*.test.mjs'` | 29 passed. |
| Desktop and web `node node_modules/typescript/bin/tsc --noEmit` from each package | Both passed. |
| Root Biome binary, `check --config-path=../biome.json` from each frontend directory, restricted to changed paths | Web 5 files and desktop 7 files passed. Invoking once from repository root encounters the pre-existing nested-root configuration error; the package-directory checks passed. |
| Mobile direct Flutter test engine, described below | Final 153/153 passed, including actual sign-in and Settings widgets, legacy login and lost-key recovery. |
| Mobile complete `lib/main.dart` Flutter kernel compilation | Passed. |
| Direct Dart analysis server: initial 20 changed paths, then all 9 paths touched by the final followups | Zero diagnostics. There are 21 changed Dart files in the final diff; standard Flutter/custom_lint invocation remains blocked as described below. |
| Dart formatter on initial and followup files | Clean; exit 0. |
| `git diff --check` | Passed. |

The six relay live tests exercise signup, encrypted persistence, a second
device's same-key recovery and actual signed `POST /events` / `POST /query`.
They verify unchanged p-tags and two existing agents' owner/sibling NIP-OA
proofs, plus existing-key linking, stale-session export denial after refresh,
unauthorized/cross-account requests, selectors, bad/replayed proofs, concurrent
initialization, ciphertext/AAD corruption, token bot denial, old token login,
and explicit unsupported web/CLI signup.

Two new local databases were used, leaving all pre-existing databases intact:
`buzz_key_recovery_codex_20261005` (desired schema) and
`buzz_key_recovery_migration_codex_20261005` (all migrations 1–59).
Redis database 15 was checked empty before use; no flush was issued. Temporary
test helpers compiled against existing Rust artifacts applied the desired
schema/reconciliation and the full SQLx migrator. A live reconciliation test
removed only the newly created fixture registry row and verified restoration.
Catalog constraint definitions for `principals` and `account_key_backups`
matched between both databases. The actual `pgschema` binary was not run.
These isolated fixtures are intentionally left in place; no cleanup of shared
data, caches or worktrees was performed.

## Flutter execution workaround

The installed Flutter SDK is `C:/tmp/flutter-sdk/3.41.7/flutter`. Standard
`flutter analyze`/test wrappers could not start nested subprocesses
(`CreateFile failed 5` / SDK cache-lock access). The installed Dart frontend
compiler and real Flutter test engine were invoked directly instead:

```text
dartaotruntime.exe frontend_server_aot.dart.snapshot
  --sdk-root <flutter>/bin/cache/artifacts/engine/common/flutter_patched_sdk/
  --target=flutter --packages .dart_tool/package_config.json
  --output-dill build/key-backup-regressions.dill
  build/key-backup-regressions.dart

flutter_tester.exe --disable-vm-service --non-interactive
  --use-test-fonts --disable-asset-fonts
  --flutter-assets-dir=C:/Users/jijee/projects/buzz/mobile/build/unit_test_assets
  build/key-backup-regressions.dill
```

Environment: `FLUTTER_TEST=true`, `APP_NAME=buzz`, `UNIT_TEST_ASSETS` set to
that assets directory. Assertions were inspected in the output because this
direct engine can return exit 0 even when tests fail. The final log explicitly
reports `+153: All tests passed!`. Local evidence remains under the original
`mobile/build/key-backup-*`, including `key-backup-regressions-result.log` and
`key-backup-app-compile.log`; these are ignored build artifacts, not preview
artifacts. The aggregate imports the checked-in test suites for Google service
and UI, token APIs/session/auth, secure community storage/removal, signed relay
events/configuration, NIP-OA, mentions, Settings, and signed/token invitations.

## Remaining acceptance and deliberate limits

- Real Google OAuth requires operator-owned Google client configuration and
  HTTPS callback registration. No credentials/configuration were fabricated,
  fetched from personal stores or installed in production.
- No interactive native desktop/mobile device run, OS credential-store
  integration acceptance, Android/iOS package build, or real cross-device
  Google login has been claimed. Component tests and native/library compilation
  cover the implemented seams but do not replace those checks.
- `just ci` and a complete all-platform suite were not run. Hermit/Git Bash
  activation is blocked by the Windows sandbox's MSYS permission error;
  native pinned/installed executables and targeted cached builds were used.
  Flutter custom_lint could not run through the blocked subprocess path.
- Web/CLI do not implement new custody signup/link/restore. They retain signed
  access and existing token-account login and show explicit conflicts or
  unsupported-client errors instead of creating another principal.
- The explicit old-account compatibility action reports a conflict when it
  would adopt desktop key agents or replace a mobile signing community.
  Existing token records remain intact; this is not an account-merge feature.
- Google enrollment does not grant closed-community membership or operator
  access. Use the existing invitation/member workflow; mobile recovery now
  explicitly claims invitations using the restored key.
- Recovery restores the human signing key, not another machine's managed-agent
  private keys, local profiles/files or AI sessions. Existing local agents and
  conversation mappings are preserved; ACP restart-resume support is not
  asserted for adapters that do not implement it.
- Desktop automatic lost-key recovery refuses unverifiable old agent ownership
  when neither a backup binding nor a valid NIP-OA owner attestation survives;
  the existing manual original-key import remains available. With all keyring
  metadata lost and no agents remaining, the chosen Google backup can restore,
  but the prior local owner cannot be independently established. Invalid local
  mobile key data is reported as a conflict; missing-key recovery uses the
  retained pubkey as its boundary.
- Custody permits the operator or a Google-account takeover to recover keys.
  Logout only revokes future session access; it cannot revoke a copied Nostr
  key. Per-device cryptographic revocation, account merging, unlink/key
  replacement and online master-key rotation remain out of scope. See the
  design's setup, backup/loss and rotation instructions.

For native acceptance on an isolated configured relay: record an existing
user/agent pubkey and conversation; link Google in Settings; renew its session
and verify unchanged pubkeys and uninterrupted agents; sign in on a fresh
second device and compare the recovered user pubkey and signed-event sender.
Repeat with restore offline/corrupt, a different local identity, and an old
token account: each must preserve local data and show an explicit error.
No human acceptance is marked complete by this report.

`CLAUDE.md` remains exactly the initially supplied empty modified file,
SHA-256 `E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855`.
No commits, pushes, deployments, production DB/config changes, real secrets,
destructive cleanup, preview changes or other-worktree changes were made.
