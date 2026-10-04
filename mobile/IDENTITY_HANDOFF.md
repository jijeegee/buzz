# Mobile centralized identity — Phase 2b handoff

## Current result (supersedes intermediate notes below)

Phase 2b implementation is integrated: Google/server PKCE login, origin-scoped
secure refresh rotation, startup restoration, Bearer HTTP/media/invites, token
WebSocket/huddle authentication, unsigned drafts, global Account and Devices UI,
and pairing removal. Existing legacy nsec authentication remains for stored
legacy communities; Phase 3 has NOT started. The stage sections below are an
implementation log; later review fixes supersede their earlier gaps.

Final independent checks after updating the two pre-existing nullable-signature
test adapters (`dev_push_lease_test.dart` and
`push_presentation_export_test.dart`):

- `flutter analyze`: **No issues found**.
- `dart format --output=none --set-exit-if-changed .`: **642 files, 0 changed**.
- `flutter test --dart-define=BUZZ_PUSH_GATEWAY_URL=https://push.example`:
  **2693 passed, 5 skipped, 2 failed**. Both remaining failures are the baseline
  Windows ComposeBar voice-note cases (`records, previews, uploads, and sends a
  voice note`; `voice note rejects an existing attachment like desktop`).
  Full local output: `C:/tmp/mobile-parent-final-test.txt`.
- `git diff --check -- mobile`: passed.
- `flutter devices`: Windows/Chrome/Edge only; no iOS/Android device connected.

Native Google callback, restart/keychain, media upload, huddle and remote-revoke
end-to-end acceptance is still outstanding. Run `pod install` on macOS before
an iOS build (the new web-auth plugin is not resolved in the iOS lockfile).
Worktree Android installs share the callback scheme. No production login or
connected-account lookup was performed.

Known integration limitations: token-mode push/NSE lacks a token-compatible
server lease contract and is unavailable; read-state/mutes/stars/sections/sort
are local-only without NIP-44; reminders/theme cross-device sync remain key-based.
Large client clock skew can change a server-stamped draft ID and time out the
client's publish acknowledgement. These are not evidence of native readiness,
and must be resolved or explicitly accepted before declaring migration complete.

## Historical stage 1 notes

Stage 1 delivered the auth core, native callback registration and a minimal
"Sign in with Google" flow. At that stage legacy auth and pairing were untouched;
pairing has since been removed by Stage 2B below.

## Toolchain

- Flutter 3.41.7 / Dart 3.11.5, extracted to
  `C:/tmp/flutter-sdk/3.41.7/flutter` (from `flutter_3.41.7.zip`).
- Activate per shell: `. /c/tmp/flutter-sdk/env.sh`. It only exports `PATH`
  (prepended), `PUB_CACHE=C:/tmp/flutter-sdk/pub-cache`,
  `FLUTTER_SUPPRESS_ANALYTICS=true`, `CI=true`. Global PATH, user env and
  Flutter global config were not changed (`--suppress-analytics` was used
  instead of `flutter config --no-analytics`).
- New deps: `flutter_web_auth_2: ^5.1.0`, `crypto: ^3.0.7` (moved from
  dev_dependencies to dependencies).

## Baseline vs final

| | Baseline (before changes) | Final |
|---|---|---|
| `flutter analyze` | exit 1, 2 issues (both pre-existing errors: `test/shared/push/dev_push_lease_test.dart:212` argument_type_not_assignable, `test/shared/push/push_presentation_export_test.dart:591` return_of_invalid_type) | exit 1, the same 2 pre-existing issues (no new ones) |
| `flutter test --dart-define=BUZZ_PUSH_GATEWAY_URL=https://push.example` | exit 1, +2577 ~5 -4 (2 compose_bar voice-note tests + load failures of the 2 files above) | exit 1, +2630 ~5 -4: the same 4 baseline failures, no new ones (+53 passing tests) |
| `dart format --output=none --set-exit-if-changed .` | — | exit 0, 628 files, 0 changed |

## Files

New:
- `lib/shared/auth/token/` — `relay_origin.dart`, `token_auth_descriptor.dart`,
  `pkce.dart`, `auth_api.dart`, `web_auth_launcher.dart`, `oidc_login.dart`,
  `refresh_token_store.dart`, `token_session.dart`,
  `token_session_provider.dart`, `token.dart` (barrel).
- `lib/features/sign_in/` — `token_sign_in_page.dart`,
  `token_session_gate.dart`.
- Tests: `test/shared/auth/token/` (fakes, basics, api/login, session),
  `test/features/sign_in/token_sign_in_test.dart`.

Modified:
- `lib/shared/community/community.dart` — `tokenAuth` flag, persisted as
  `"authMode": "token"` (omitted for legacy records).
- `lib/shared/auth/auth_provider.dart` — token communities stay authenticated
  without an nsec; `authenticateWithTokenSession(relayUrl, principalId)`
  reuses the same-origin community or creates one.
- `lib/app.dart` — token communities render Home behind `TokenSessionGate`;
  onboarding shows pairing plus a "Sign in with Google" entry.
- `android/app/src/main/AndroidManifest.xml` — flutter_web_auth_2
  `CallbackActivity` for `xyz.block.buzz://auth`.
- `pubspec.yaml`, `pubspec.lock`; `test/shared/auth/auth_provider_test.dart`,
  `test/features/pairing/pairing_provider_test.dart` (fake gains the new method),
  `test/shared/community/community_test.dart`.

## Auth core public API (`shared/auth/token/token.dart`)

- `normalizeRelayOrigin(url)` — canonical `https://host[:port]` (ws→http
  folding, default ports dropped, lower-case host). Also the secure-storage
  scope.
- `fetchTokenAuthDescriptor(origin, client)` → `TokenAuthDescriptor?`
  (`null` = legacy relay; network/HTTP/JSON failures throw
  `TokenAuthDetectionException`, never "legacy").
- `PkcePair.generate()`, `pkceChallenge(verifier)`.
- `AuthApi(origin, client)` — `startUri`, `completeLogin`, `refresh`,
  `logout`; `AuthApiException.kind` is `terminal` (400/401/403/404) or
  `transient` (network, 408, 429, 5xx, malformed body).
- `runOidcLogin(api, launcher)` — `client=mobile`,
  `redirect_uri=xyz.block.buzz://auth/cb`; validates `state` before reading
  `error`/`code`.
- `RefreshTokenStore` / `SecureRefreshTokenStore` — key
  `buzz.auth.refresh.v1:<origin>`, `{refresh, principal_id}` JSON,
  `first_unlock_this_device`. Access tokens are memory-only.
- `TokenSessionController` — `state`, `addListener`, `restore()`,
  `signIn()`, `currentAccessToken`, `ensureFreshAccessToken()`,
  `refreshAfterTokenExpired(rejected)`, `retryNow()`, `signOut()`,
  `dispose()`. Guarantees:
  - rotation persisted before it is published; a persist failure is terminal
    (nothing published, new session revoked best-effort);
  - single-flight restore / refresh / sign-in; a generation fence stops a
    late rotation from resurrecting a signed-out or replaced session;
  - 400/401 → signed out (refresh deleted); network/5xx → backoff
    5s/15s/60s/120s/300s then `stalled` (session kept, UI offers Retry and
    Sign in again);
  - proactive refresh 5 min before expiry (min 30 s); timers cancelled on
    sign-out, dispose and generation change;
  - `signOut()` throws `TokenSessionException` and keeps the session on a
    transient failure; `token_expired` refreshes once and retries.
- Riverpod: `tokenSessionProvider(origin)` (state; building it starts the
  restore), `tokenSessionControllerProvider(origin)`,
  `activeTokenOriginProvider`; overridable seams `authHttpClientProvider`,
  `refreshTokenStoreProvider`, `webAuthLauncherProvider`,
  `sessionClockProvider`, `sessionTimerFactoryProvider`,
  `authDeviceNameProvider`.

## TDD evidence

- Auth core: stubs first. RED run `+7 -35: Some tests failed.` (all 35
  failures were `UnimplementedError`; the 7 passing tests cover
  relay_origin / descriptor / PKCE, which were implemented before their
  tests). GREEN run `+42: All tests passed!`.
- Mutation checks (each mutation applied, the named test failed, mutation
  reverted):
  - M1 drop the persist generation fence → "a rotation completing after
    sign-out cannot resurrect the session" fails.
  - M2 remove the stall bound → backoff test fails.
  - M3 ignore the state mismatch → 2 `runOidcLogin` tests fail.
  - M4 swallow logout transient failures → "network failure keeps the
    session" fails.
  - M5 skip `enterCommunity` after sign-in → the widget test "relay check,
    Sign in with Google, fake browser: signed in and recorded" fails.
  - M6 drop `active.tokenAuth ||` in `AuthNotifier.build` → "a stored token
    community without a key stays authenticated" fails.
- The UI, `Community.tokenAuth` and `AuthNotifier` tests were written
  alongside the code, not test-first; M5/M6 are their falsifiability check.

## Native callback

- Android: `com.linusu.flutter_web_auth_2.CallbackActivity`, exported,
  `taskAffinity=""`, `scheme=xyz.block.buzz host=auth`. `MainActivity`
  already has `taskAffinity=""`.
- iOS: no change. `ASWebAuthenticationSession` captures the callback scheme
  itself; no `CFBundleURLTypes` entry is needed (deployment target 16.0 ≥
  plugin minimum).
- `scripts/mobile-worktree-overrides.sh` (read-only): worktree debug builds
  change the bundle id / applicationId (`xyz.block.buzz.dogfood.mobile.<slug>`,
  `xyz.block.buzz.mobile.<suffix>`) but **not** the callback scheme. On iOS
  this is fine. On Android, several side-by-side installs all register
  `xyz.block.buzz://auth`, so the OS may show a chooser or deliver the
  callback to another install; the relay only allows the configured
  `AUTH_MOBILE_REDIRECT_SCHEMES`. If side-by-side Android logins matter, make
  the scheme a manifest placeholder and add the per-worktree scheme to the
  relay allow-list.

## Design notes and known gaps

- `dispose()` does not bump the generation: an in-flight rotation is still
  persisted (so the refresh is not lost), but not published.
- A sign-in that replaces a stalled session does not revoke the old device
  session (it is unreachable); it expires on the relay.
- A persist failure revokes the new session best-effort; refresh-reuse
  detection on the relay is the backstop.
- `TokenSessionGate` rewrites the community's `pubkey` when a re-sign-in
  produces a different principal; if that write fails it is only logged and
  retried on the next signed-in transition (not durable).
- In token mode the app reaches Home, but the relay socket, media, invites,
  huddles and signing still require the nsec, so most features are inert
  until stage 2. Do not ship a token-only build before stage 2.
- Removing a token community does not yet call `signOut()` on its session
  (the refresh record stays in the keychain).
- The push NSE / lease path (`dev_push_lease`, revocation outbox,
  `push_bridge` key export) depends on the nsec; needs a token design.
- Pairing is still the only legacy onboarding; removing it conflicts with
  "keep legacy until Phase 3" because legacy users have no other way in.
  Decision needed before stage 2 deletes it.

## Audit — authenticated paths and planned replacement

Paths are relative to `mobile/`.

### NIP-98 HTTP → `Authorization: Bearer <ensureFreshAccessToken()>`, one retry via `refreshAfterTokenExpired` on 401 `token_expired`
- `lib/shared/relay/relay_session_auth.dart:3` `buildNip98AuthHeader` (signs at :20).
- `lib/shared/relay/relay_session.dart:157-185` `queryRelay` POST `/query` (header :175).
- `lib/features/invites/invite_create_provider.dart:211-234` `mintInvite` (:224).
- `lib/features/invites/invite_join_provider.dart:202-222` claim (generates keys :20, nsec :218, stores identity :239-246) → token-mode claim with the signed-in principal.

### WebSocket NIP-42 → `["AUTH", {"token": …}]`, re-AUTH on reconnect and on `token_expired`
- `lib/shared/relay/relay_socket.dart:195-231` `_handleAuthChallenge`, `_handleOk` :237-256, connect :68-127 (8 s timeout :109), reconnect :538 (no re-AUTH today).
- `lib/shared/relay/relay_session.dart:146-151` auto-connect gated on `config.nsec != null` (:148); `_connect` :486-510.
- `lib/shared/relay/signed_event_relay.dart:68-132` `submitSignedEventOnce` (callers `community_provider.dart:166`, `push_lease_revocation_outbox.dart:529`).
- `lib/features/pairing/pairing_provider.dart:891-915` `_validateCredentials` (deleted with pairing).

### Event signing → unsigned drafts stamped by the relay (plan §3.2)
- `lib/shared/relay/signed_event_relay.dart:49` submit, pubkey getter :21-27.
- `lib/shared/relay/relay_session.dart:322` publish; `_handleOk` :819-862 (placeholder with empty pubkey/sig :836-845).
- Direct signers: `features/profile/user_status_provider.dart:96-106` (kind 30315), `features/channels/compose_bar/helpers.dart:374-407` (typing).
- Submit callers: `send_message_provider` (:100/:231), `channel_management_actions` (:54-373/:390), `channel_huddle_actions` (:16, :40, :57, :79), `forum_provider` (:89, :107, :125, :160), `pulse_actions` (:22/:39, :66/:68), `profile_provider` presence (:393-399), `reminder_service` (:116/:143), `invite_create_provider` `relayAdminAddMember` (:268/:293).
- Self-encrypted (see NIP-44 below): `read_state_manager:406`, `channel_mutes_manager:357`, `channel_stars_manager:357`, `channel_sort_manager:302`, `channel_sections_manager:482`, `community_theme_sync:255`.

### Media (Blossom kind 24242) → Bearer upload/download
- `lib/shared/media/media_upload.dart:659-711` (fallback :591-608, nsec :679-687; `platform_bindings.dart:19`).
- `lib/shared/media/media_auth.dart:54-127` `MediaGetAuthService.headersFor`. Consumers: `media_image:100,211`, `message_content:532-541`, `message_actions:344`, `media_carousel:128`, `video_viewer:55-73`, `video_preview:32`, `voice_note_attachment:56`, `ios_native_picker:115-133`.

### Huddle → `{"type":"auth","token":…}`
- `huddle_auth.dart:59-87` `HuddleAuthV2.buildMessage` (params :7-43).
- `huddle_transport.dart:394-435` `_handleChallenge`.
- nsec gates: `mobile_huddle_controller.dart:488-503`, `huddle_sheet.dart:448-455`.

### Profile → `PATCH /auth/profile`
- `features/profile/profile_provider.dart:136-145` → `_publishProfilePatchNow` :168-241 (kind 0 at :207). **Decision:** `about` has no PATCH field.

### Push → decision needed
- `shared/push/dev_push_lease.dart:306-398`, `:426-496` (kind 30350, NIP-44 with the member key).
- `push_bootstrap.dart:280-291`, `:359-399`; `community_provider.dart:118-176`.
- `push_lease_revocation_outbox.dart` (stores the nsec :39, :114, :141-153; republishes :522-536).
- Gateway enrollment itself is unaffected.

### Pairing (NIP-AB) → delete
- `features/pairing/pairing_socket.dart:209-219`.
- `features/pairing/pairing_provider.dart` (:361-383, :427/:642/:784/:799, :553/:813, :635-639, :723-749/:943, :826, :851).
- `features/pairing/pairing_crypto.dart:20`.

### NIP-44 self-encryption → decision needed (no user key in token mode)
- `read_state_manager:21-39` and the mutes / stars / sort / sections managers.
- `reminder_service:82-93`, `reminders_provider:159-198`, `community_theme_provider:219-224`.
- `observer_subscription:237-247` (end-to-end to the user key; blocked in token mode).
- `nip_oa.dart` only verifies, so it stays.

### nsec storage → none in token mode
- `community.dart:26/114/149`, `community_storage.dart:17-22, 46, 111-113`, `community_provider.dart:268-271`, `auth_provider.dart:39, 99-108`.

### Pubkey derivation → `Community.pubkey` (principal id) in token mode
- Choke point: `shared/relay/relay_provider.dart:111-126` `pubkeyFromNsec` / `myPubkeyProvider`.
- Also: `signed_event_relay:21-27`, `user_status_provider:37-41, 123, 163-171`, `observer_subscription:124-125, 302-316`, `connection_section:117-125`, channel mutes/stars/sort/sections providers, `read_state_provider:90-92`, `push_bridge:302`, `push_subscription_provider:48`, `community_provider:132`, `channel_management_provider:264`, `community_theme_provider:33`.

### iOS native → decision needed
- `push_bridge.dart:306-329` exports private keys.
- `PushSnapshotBridge.swift:145,204`, `PushNativeState.swift:46-69`, `NotificationService.swift:46-50,162`.
- `BuzzPushNotificationResolver.swift:122-150, 315-368` sign NIP-98 via `NostrHTTPAuth.swift:86-129`.
- Signature checks on fetched events (`NostrHTTPAuth.swift:37`, resolver :160/:382, `BuzzPushPresentationCache.swift:214,293,340`) must change if events are server-stamped.

## Stage 2 — remaining work

1. Bearer HTTP: `/query`, invites (mint/claim), media upload and download
   headers, with the single `token_expired` retry.
2. WebSocket: token AUTH, re-AUTH on reconnect, `token_expired` handling;
   lift the `config.nsec != null` auto-connect gate for token communities.
3. Unsigned drafts for every publish path and self-signers (status, typing).
4. Huddle token auth; remove the nsec gates.
5. Account profile / avatar via `PATCH /auth/profile` (decide on `about`).
6. Devices / security screen (list and revoke device sessions).
7. Bot-token revoke UI.
8. Delete account.
9. Remove pairing (NIP-AB, `features/pairing`); decide the legacy onboarding
   story first.
10. Sign the token session out when a token community is removed.
11. Decisions: NIP-44 self-encrypted state, push lease / NSE, iOS native
    signature checks, `myPubkeyProvider` in token mode.

## Stage 2A (transport)

- Slice 1 (WS token AUTH): green.
  - `RelaySocket.accessTokens` (set by the session before `connect()`) switches
    the socket to `["AUTH",{"token":…}]`. It sends at channel-ready with
    `fresh()`, and a relay challenge never produces NIP-42.
  - `OK auth false auth-required: token_expired|token_revoked|invalid_token`
    calls `afterExpired` once and re-AUTHs once on the same socket. A second
    rejection is terminal.
  - A null refresh produces `RelayTokenUnavailableException`.
    `auth-required: unavailable` and `error:` are transient (backoff).
  - `reauthenticate(token)` swaps the token on the live socket, queueing
    behind an in-flight AUTH.
  - `RelaySessionNotifier` auto-connects token communities without an nsec.
    It listens for tokens (re-AUTH when connected, reconnect when parked) and
    for state (signedOut disposes the socket and bumps the generation, so no
    stale token is sent).
  - Token unavailable or a token-mode rejection parks the session with no
    backoff timer. The next issued token reconnects, and the token session's
    gate is the user's recovery path.
  - Listeners are removed on dispose.
  - Tests: `test/shared/relay/relay_socket_token_auth_test.dart` (real
    loopback WS, 8) and `relay_session_token_ws_test.dart` (5). Red first:
    compile failure (no `accessTokens` / `reauthenticate`), then auto-connect
    `Expected length 1, Actual []`.
- Slice 2 (unsigned drafts): green.
  - `shared/relay/signed_event_relay.dart` adds three helpers:
    - `outgoingAuthorPubkey(config)` returns the principal id in token mode,
      else the nsec pubkey.
    - `unsignedDraft(...)` builds a draft with the NIP-01 id and no `sig`.
    - `buildOutgoingEvent(config, ...)` returns a draft in token mode and
      signs as before in legacy mode.
  - `SignedEventRelay.forConfig` covers messages, edits, deletes, reactions,
    forum, pulse, invites, presence and profile. Each of these callers now
    uses it, and their validity checks compare `outgoingAuthorPubkey`.
  - Status (`user_status_provider`) and typing (`compose_bar/helpers`)
    publish drafts too.
  - Legacy `SignedEventRelay({session, nsec})` is unchanged.
  - Tests:
    - `test/shared/relay/signed_event_relay_draft_test.dart` (3).
    - A token draft case in `user_status_provider_test.dart` and in
      `compose_bar_test.dart`.
  - Red first: compile failure (`SignedEventRelay.forConfig` /
    `buildOutgoingEvent` missing). The status and typing tests were falsified
    by restoring the nsec gate, which fails with `Bad state: No element`.
  - Known gaps:
    - Kind 0 is refused by the relay in token mode. The profile editor must
      use `PATCH /auth/profile` (2B owner).
    - `submitSignedEventOnce` stays nsec-only. Its only caller is
      `community_provider.dart:167` (2B-owned).
    - With clock skew over ±5 min, the relay recomputes a different id, so
      the OK never matches and the publish times out. This is accepted.
- Slice 3 (huddle token auth): green.
  - `HuddleConnectionParameters.accessTokens` takes the place of `nsec` on a
    token community.
  - The challenge is answered with
    `{"type":"auth","token","parent_channel_id","protocol_version"}` from
    `fresh()`. A null token fails with `authenticationFailed`.
  - A rotation sends `{"type":"auth","token"}` on the same socket after
    admission, single-flight, and coalesces to the latest token.
  - `auth_ok` sends any pending token. `auth_error` emits an issue but no
    teardown and no automatic resend, so it cannot loop.
  - Listeners are scoped to the socket generation and removed on disconnect
    and failure. A `signedOut` state fails the transport (logout fence).
  - The nsec gates in `mobile_huddle_controller` and `huddle_sheet` are
    relaxed for `tokenAuth`.
  - Tests:
    - `test/shared/huddle/huddle_transport_token_test.dart` (9).
    - A token join case in `channel_detail_page_test.dart`.
  - Falsified by:
    - Disabling the token challenge branch: 6 tests fail.
    - Disabling the signedOut fence: the test fails with
      `Expected failed, Actual connected`.
  - After signout, the huddle session's bounded reconnect (7 attempts) fails
    fast on each attempt, then ends terminal. Nothing loops.
- Session sign-in generation fence (`shared/auth/token/token_session.dart`):
  green.
  - A sign-in that completes after a newer sign-in or a signout no longer
    overwrites state.
  - Tests: 3 new cases in `token_session_test.dart`.
- HTTP Bearer: green.
  - `relayAuthErrorCode` parses both `{"code":…}` and the invites shape
    `{"error":"token_expired"}`.
  - `sendWithBearer` / `sendRelayAuthorized` do one `afterExpired()` refresh
    and one retry.
  - The invite claim on an existing token community uses the Bearer
    (`inviteClaimAccessTokensProvider`, `_claimWithBearer`).
  - Tests: `invite_create_token_test` (+1), `invite_join_token_test` (3).
- Media Bearer: green.
  - `MediaAuth.get` requests a fresh token per relay media URL through
    `sendWithBearer`. `media_image` uses it. Upload was already Bearer.
  - Tests: 3 new cases in `media_image_test.dart`.
  - Synchronous `headersFor` consumers still read `current`, which is null
    only inside the 60 s expiry margin. A null token falls back to an
    unauthenticated request, never a stale one.
- NIP-44 settings and push without an nsec: green.
  - Mutes, stars, sections and sort take a nullable crypto. Without an nsec
    they are local-only (`_remoteEnabled` is false) and never publish. The
    pubkey falls back to the principal id.
  - Sections now emit state on local edits (`_mutate`). Before this, edits
    were invisible when offline.
  - Push stays disabled in token mode, because
    `currentRelayPushDescriptorProvider` gates on the nsec. This is pinned by
    `test/shared/push/push_token_mode_test.dart`.
  - Tests: `test/features/channels/channel_prefs_token_mode_test.dart` (4).
    Red first: `isReady` false.
  - Audited and left unchanged, because they are already inert without an
    nsec: read state, reminders, community theme sync, observer
    subscription, push bootstrap and lease revocation, and the NSE signing
    keys.
- Needs from 2B:
  - A new-community invite join on a token relay still claims with a
    generated NIP-98 keypair. Onboarding should detect NIP-11
    `buzz_token_auth`, sign in first, then claim with the Bearer.
  - In token mode, read state, reminders, theme sync, push and cross-device
    settings sync are unavailable or local-only. The UI may want to say so.
  - `submitSignedEventOnce` caller in `community_provider.dart`: see slice 2.
- Checks (final):
  - `dart format --output=none --set-exit-if-changed .`: 0 changed.
  - `flutter analyze`: 2 issues, both baseline
    (`dev_push_lease_test.dart:212`, `push_presentation_export_test.dart:591`).
  - `flutter test`: +2634 ~5 -4. The 4 failures are the baseline:
    - those 2 files failing to compile;
    - 2 compose_bar voice-note tests.

## Stage 2B (UI/account)

Status: slices 1-5 done and green. 2B only edited `mobile/` files outside the
2A-owned areas: relay, token, media, invites, huddle, push and publish code.

### Files

- Deleted:
  - `lib/features/pairing/**` (crypto, page, provider and onboarding parts)
  - the pairing tests
  - the `mobile_scanner` dependency (`pubspec.yaml`, `pubspec.lock`, and
    `ios/Podfile.lock`, edited by hand)
- New:
  - `lib/shared/auth/account/account_api.dart`
  - `lib/features/settings/account_page.dart`
  - `lib/features/settings/devices_page.dart`
  - `lib/features/settings/settings_page/account_section.dart`
  - `test/shared/auth/account/{fake_account_server,account_api_test}.dart`
  - `test/features/settings/{token_settings_harness,account_page_test,devices_page_test}.dart`
  - `test/app_token_flow_test.dart`
  - `test/features/.../token_session_gate_identity_test.dart`
  - `community_token_removal_test.dart`
- Changed:
  - `lib/app.dart` (onboarding is now `TokenSignInPage`)
  - `lib/shared/auth/auth_provider.dart` (`signOut({deviceOnly})`)
  - `lib/shared/community/**` (removal ends the token session)
  - `lib/features/sign_in/**` (the gate's identity-failure view)
  - `settings_page.dart`, `connection_section.dart`
  - `ios/Runner/Info.plist` (the camera string no longer mentions pairing)
  - `test/widget_test.dart` (pairing-removed assertion)
  - `token_settings_test.dart`, `token_sign_in_test.dart` (refresh 401)
  - `age_gate_app_test.dart` (disposes the container so the session's
    refresh timer is not left pending)
  - fake overrides in `channels_page_test.dart` and `relay_session_test.dart`

### Slices

1. **Pairing removed: green.**
   - Onboarding is `TokenSignInPage` (`_OnboardingHome`). The community
     switcher's "add" pushes `TokenSignInPage`.
   - `widget_test.dart` asserts that the directory is gone and that no `lib/`
     file references it.
   - The remaining grep hits are not NIP-AB code:
     - the 2A `dev_push_lease.dart` NIP-11 key `pairing_relay_url`
     - comments in relay files
     - `theme_pairs.dart`
2. **Token community removal signs out: green.**
   - `communityTokenSessionEnderProvider` revokes the session on the relay
     before any local teardown.
   - A `TokenSessionException` leaves the community intact.
     `removeCommunityWithRelaySignOut` then offers Try again, Remove from this
     device only, or Keep. This works from both the settings row and the
     switcher.
   - A failed account-switch identity write is surfaced by `TokenSessionGate`
     with a retry (`token-identity-retry`).
3. **Settings › Account: green.**
   - `AccountPage` loads `GET /auth/me`.
   - Save makes one `PATCH /auth/profile` carrying `display_name`,
     `avatar_url` and `username` together, so the write is atomic.
   - Input is validated on the client first: name 1-64 characters, username
     3-32 characters of `[a-z0-9_]`.
   - The avatar is uploaded with `mediaUploadServiceProvider` before the PATCH.
     "Remove photo" sends `null`.
   - Errors show in a live region and keep the user's edits. `username_taken`
     has its own message.
   - A load failure shows a Try again action, with no silent Riverpod retry.
4. **Devices & security: green.**
   - Lists `GET /auth/devices`. The current device is marked and has no
     remote sign-out button.
   - Per-device sign-out (`DELETE /auth/devices/{id}`), sign out all other
     devices, and revoke all bot tokens each need confirmation, then refresh
     the list.
   - Delete account: confirm, `DELETE /auth/account`, then a local
     `signOut(deviceOnly: true)` and pop to the root. A failure keeps all
     local state and shows the error.
5. **Token mode: green.**
   - `activeTokenOriginProvider` swaps the nsec Connection section for the
     Account section: profile, Devices & security, and the Account ID shown as
     an npub, which is what gets copied.
   - The nsec UI is hidden even when a legacy nsec is stored, and there is no
     crash when no nsec exists.
   - "Edit profile" opens `AccountPage`, because the relay rejects client
     kind 0 in token mode.

### Decisions

- `/auth/me` exposes no email, so settings shows the name, `@username` and
  the principal npub.
- Invite links remain the legacy, non-pairing nsec entry point until Phase 3.
  `invite_join_provider` is owned by 2A.
- Deleting the account signs out locally on this device only, because the
  relay has already ended every session.

### Red to green evidence

- The new test files first failed to compile:
  - `Couldn't find constructor 'AccountPage'/'DevicesPage'`
  - `Type 'Override' not found`
- They then failed at runtime:
  - The load-failure test showed no error until auto-retry was disabled.
  - Account ID copy returned null for a non-hex principal.
  - The page-back animation had not settled.
- All of these are now green.
- TDD caveats:
  - `AccountApi` was written before its tests.
  - The App-flow tests and the refresh-401 tests passed on their first run.
    They characterize behaviour that already existed.
- App-level tests in `app_token_flow_test.dart`, all against real providers
  with fake HTTP:
  - Google sign-in reaches HomePage
  - a stored session restores and reaches HomePage
  - a revoked session lands on sign-in with the community kept
  - a 401 refresh mid-session returns to sign-in

### Check results

- `dart format --output=none --set-exit-if-changed .`: 636 files, 0 changed.
- `flutter analyze`: the 2 baseline errors only, both in 2A push tests:
  - `dev_push_lease_test.dart:212`
  - `push_presentation_export_test.dart:591`
- `flutter test --dart-define=BUZZ_PUSH_GATEWAY_URL=https://push.example`:
  `+2628 ~5 -4`. The 4 failures are the same as the baseline:
  - 2 compose_bar voice-note tests
  - 2 load failures in the push test files above

### Runtime blockers / follow-ups

- Run `pod install` on macOS to regenerate `Podfile.lock` properly.
- Nothing has been run on a real device or simulator yet: Google sign-in,
  avatar upload and device revoke all need a live relay.
- `profile_provider` still publishes kind 0 on the legacy nsec path.
- The push lease / NSE and the NIP-44 self-encrypted state decisions are
  still open with 2A.

## Review fixes (UI/account/push)

Each fix had a failing test on the production seam first, then went green.

- **B1 (rule 6), `TokenSessionGate`.**
  - Retrying or stalled with a saved principal now keeps HomePage under a
    non-blocking banner (Retry now / Sign in again, live region). HomePage
    stays mounted at a stable position, so it is not remounted.
  - Stalled with no principal (keychain read failure), or a principal that
    differs from the community's, still shows the full recovery view.
  - The recovery view and the locked `TokenSignInPage` both embed the new
    `features/sign_in/community_recovery_actions.dart`:
    - "Switch community" (other stored communities)
    - "Remove this community" (confirm, then
      `removeCommunityWithRelaySignOut` with the device-only fallback)
  - Tests: `test/features/sign_in/token_session_gate_recovery_test.dart`; the
    unreachable-relay test in `token_sign_in_test.dart` was updated.
- **B6, legacy to token conversion** (`AuthNotifier.authenticateWithTokenSession`).
  - First the push-lease tombstone is journaled through the outbox, signed
    with the legacy nsec. A failure propagates and changes nothing.
  - Then one community write sets `tokenAuth`, the principal, `nsec: null`,
    push off and a fresh subscription state. After that the outbox trigger
    fires.
  - Outbox and `_deactivateCommunityPushLease` now derive `memberPubkey`
    from the nsec.
  - `push_bridge` exports no signing key for token communities or for
    nsec/pubkey mismatches.
- **B7 (rule 7).** The Account ID row is one container semantics node that
  owns the label and the tap action. The test performs `SemanticsAction.tap`
  and checks the copy.
- **Cross-feature import.** `channels_page` no longer imports `sign_in`.
  - The switcher calls `pushNamed(addCommunityRouteName)`, a new constant in
    `shared/community/add_community_route.dart`.
  - `app.dart` registers that route.
  - Guard test: `test/features/channels/channels_page_imports_test.dart`.
- **Devices.** Once the relay has deleted the account, the delete row is gone.
  A failed local sign-out is shown separately, with a local-only "Finish
  signing out" retry that does not send a second DELETE.
- **Account page.**
  - The name limit is now `characters.length`.
  - A save writes the returned profile into the shared `userCacheProvider`.
    It uses the captured container, so this also works if the page closed
    during the save.
  - `profileProvider` (features/profile) is not invalidated, because that
    would be a cross-feature import. It refreshes on its next relay fetch.
- **AccountApi.** A `token_revoked` or `principal_disabled` response presents
  the token that was actually sent to `refreshAfterTokenExpired`. The terminal
  refresh signs the session out, so the gate shows sign-in. This uses the
  existing public API only.

## Review fixes (transport)

Each fix had a failing test on the production seam first (red recorded),
then went green.

- **B2, first-AUTH refresh** (`relay_socket.dart`, `relay_session.dart`).
  - A rejected first AUTH fails the connection (ws.rs), so the socket
    refreshes the token and then fails with `RelayTokenRefreshedException`.
  - The session reconnects at once on a NEW socket. The budget is one
    refresh: a second rejection parks until the next issued token.
  - A rotation during the first AUTH is queued until AUTH OK.
  - The refreshable set is shared as `isRefreshableTokenRejection`.
  - Tests: `relay_socket_token_auth_test.dart`, `relay_session_token_ws_test.dart`.
- **B3, claim on the active token community** (`invite_join_provider.dart`).
  - Switching to the already-active community is a no-op, so after the
    Bearer claim the parked session (member-restricted AUTH deny) is
    reconnected explicitly.
  - Test: `invite_join_token_test.dart`.
- **B4, new-community invite on a token relay.**
  - `prepare` detects NIP-11 `buzz_token_auth` (`inviteTokenAuthDetectorProvider`).
  - `confirmJoin` re-detects if a detection outage left it unchecked; it
    never falls back to keygen.
  - The flow is: restore or sign in (system browser), THEN the Bearer claim,
    then save a `tokenAuth` community for the principal (`nsec: null`), then
    starter-setup recovery.
  - A cancelled sign-in or a relay outage is an error with no claim and no
    key.
  - The sheet shows "Sign in with Google to join" and the token-account
    copy.
  - Tests: `invite_join_token_test.dart`, `invite_join_sheet_test.dart`.
  - Known gap: an invite for an existing token community still shows the
    legacy "only copy of this identity" text, because `tokenRelayOrigin` is
    not set on that branch.
- **B5, read state on token communities.**
  - `ReadStateManager` runs local-only: there is no NIP-44 crypto or signed
    publish, and it is keyed by the principal.
  - The unread badge stays 0 until `isReady`.
  - Channels unread catch-up waits, bounded at 30 s, for loaded markers
    before querying `since`.
  - Tests: `read_state_provider_test.dart`, `unread_badge_provider_test.dart`,
    `channels_provider_test.dart`.
  - Also: the initialize log truncates the pubkey safely (a short principal
    crashed `substring`, caught by `channel_detail_page_test`).
- **Optional fixes.**
  - `TokenSessionController.dispose` bumps the generation, so an in-flight
    rotation is never persisted after a device-only removal.
  - `relayAccessTokensProvider` returns `null`, never a `FormatException`,
    for a token community with no canonical origin. Callers already fail
    closed on `null`.
  - Huddle: a first-AUTH `{"type":"error","message":"auth-required:
    token_*"}` calls `afterExpired` once and admits on a new socket.
    - The transport stays `connecting` meanwhile, so `HuddleSessionNotifier`
      does not tear down on a transient `failed`.
    - A second rejection is final. A failed refresh fails with "Sign in…".
    - Non-token errors are not refreshed.
  - `/auth/refresh` 403/404 is transient. The handler only answers
    400/401/429/5xx, and a 403/404 is the unrouted fallback with
    `AUTH_TOKEN_ENABLED` off (mod.rs says 403, router_tests says 404). The
    session stalls and is kept rather than wiped. Logout still treats 404 as
    "already revoked".

## Final integration check (2026-10-06, session buzz-b6)

Run from `mobile/` with `. /c/tmp/flutter-sdk/env.sh` (Flutter 3.41.7), after both review-fix rounds and an LF line-ending normalization:

- `dart format --output=none --set-exit-if-changed .`: 642 files, 0 changed.
- `flutter analyze`: 2 issues. These are the baseline errors only (`test/shared/push/dev_push_lease_test.dart:212`, `test/shared/push/push_presentation_export_test.dart:591`).
- `flutter test --dart-define=BUZZ_PUSH_GATEWAY_URL=https://push.example`: `+2666 ~5 -4`. The baseline was `+2577 ~5 -4`. The 4 failures are the same as the baseline:
  - two ComposeBar voice-note tests (Windows path/mime)
  - two load failures in the push test files above
- Sign-in generation fence mutation check: disabling `_isCurrent(startGeneration)` in `_runSignIn` makes 3 tests fail. They cover sign-out during the browser, sign-out during the exchange, and dispose during the browser. The guard was restored afterwards.
- No new `StatefulWidget` or `print(`. The two pre-existing `StatefulWidget` files are unchanged.
- No files outside `mobile/` were touched by this work, and nothing was committed.

Not yet exercised on a device or simulator. Each of these needs a native runtime and a token-enabled relay:
- Google sign-in through `ASWebAuthenticationSession` and Custom Tabs
- the `xyz.block.buzz://auth/cb` callback
- keychain/keystore persistence across restarts
- live WS, huddle, media and avatar round trips
- device revoke

Before any iOS build, run `pod install` on macOS to add `flutter_web_auth_2` to `Podfile.lock`. Android side-by-side worktree installs share the callback scheme (see Native callback).
