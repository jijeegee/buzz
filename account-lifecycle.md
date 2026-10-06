# Account lifecycle

The final user correction is authoritative. Switching accounts selects a view;
it does not log out, disconnect, stop, restart, or reconfigure the previous
account's runtime. App exit keeps its existing behavior. `shutdown.rs` is not an
account-switch implementation seam.

## Boundaries and current dependencies

- Account identity is canonical service origin plus a stable server account ID
  (Google backup account) or Nostr pubkey, never an email or array position.
- Desktop currently has one mutable `AppState.keys` / relay override,
  origin-only `TokenAuthState`, and a frontend relay singleton.
  `apply_workspace` changes the key and relay; `useCommunityInit` disconnects
  the previous frontend relay and resets stores. These paths alone cannot
  implement the required concurrent account switch.
  `NativeRelayClient` also has a single archive-owned session slot that shuts
  down its predecessor; preserving frontend sockets alone is insufficient.
- Independent review additionally found that `apply_workspace` calls
  `agents/provider_access::reconcile_on_workspace_apply`, which can redeploy
  all existing provider agents using the newly active owner/relay/config.
  View selection must never call this reconciliation. Boot/config reconciliation
  needs captured account-owned inputs and an account-filtered target list.
- Agent records/list commands and global provider/model configuration currently
  share one storage namespace. Add explicit owner-account association while
  keeping the global runtime registry available to existing shutdown cleanup.
  Selected logout cannot reuse a relay-wide stop or community removal operation.
- Native workers must capture account, relay, signer, provider configuration,
  and destination before their first await/spawn. Later tool calls and result
  delivery must use that captured context. The shared process registry stays
  visible to existing app-exit cleanup. No hidden app instances or key changes.
- Mobile token controllers and Google recovery currently use origin-only
  lookups; recovery selects the first saved community at an origin. Adding B
  must not overwrite A's refresh token or link B to A's key. Riverpod relay
  lifetimes currently follow the active community and require an account-owned
  lifetime before presenting switching as concurrent operation.

## Transitions and storage

1. Boot: import the existing single-account record by reference to its existing
   key, device ID, token, communities and state. Never regenerate a missing key.
   Commit a versioned account index only after its referenced records exist;
   failures preserve the legacy record and remain retryable.
2. Add: one browser authorization at a time, explicit Google account chooser,
   PKCE/state and exact callback validation. Resolve the backup under the
   returned stable identity; reuse an existing account idempotently. Commit its
   isolated session/key and index together before allowing selection. Cancelled
   and superseded attempts cannot activate or overwrite an account.
3. Switch A -> B: keep A's runtime and credentials alive; select B's existing
   context and view. Namespace drafts, PIN, read/mute state, query caches,
   navigation and callbacks by account plus community. A's results and unread
   updates remain in A; alerts identify A. A -> B -> A restores A's state.
   Expired authorization requires honest reauthentication of that account.
4. Log out selected account: confirm if its local work must stop; fence only
   that account's new work, complete its cleanup, remove only its local session
   and account data, then select a remaining account or show login. A failed
   cleanup cannot report successful logout. Device-only logout must be explicit
   when server revocation cannot finish. Never revoke other devices or remote
   agents; never delete server history, account, or backup.
5. Default service community: hide Leave using canonical configured identity,
   exposing account actions instead. Additional communities retain Leave.

Mobile operating-system suspension remains authoritative; keeping account
ownership is not a promise of perpetual phone socket execution. Existing
onboarding stages, provider/model selection, login, and Each thread defaults
remain intact.

## Implementation and acceptance order

1. One independent read-only review of these seams and concrete leak/cancellation
   risks, before implementation. Resolve blockers in the design.
2. Fix proven OAuth callback/lifecycle defects with production-path tests.
   Production and side-by-side development callback schemes must not collide;
   any new server redirect allowlist entry is a deployment prerequisite, not a
   client-side workaround for Google MFA. No phone reproduction is claimed.
3. Introduce the smallest account-owned native/session contexts, then durable
   account storage and view selection; only expose controls when the full
   switch path preserves runtime ownership. Do not ship a UI-only switch or
   fall back to stopping A.
   Sequence: account identity/index and legacy adoption; keyed token/bot refresh
   and key custody; owned agent/config storage and pinned native commands;
   persistent account connections with view projections; scoped logout; UI.
4. Focused tests: delayed A work survives selection of B and publishes only as
   A to A's room; delayed callbacks and tool sends retain their signer/provider;
   duplicate add and OAuth cancellation; logout A leaves B running; unchanged
   app-exit coverage; default-community Leave predicate; migration preserves
   keys and A -> B -> A preserves account-local state.

Use fixture storage/processes and isolated UI harnesses. No real user switching,
logout, job termination, server/DB/proxy changes, deployment, APK installation,
native rebuild swap, secret reads, or cache cleaning. Native edits are not live
until a later coordinated rebuild. Commit tested increments with explicit paths
and DCO signoff, excluding the preexisting `CLAUDE.md` modification. Parent
review and manual desktop/device acceptance remain outstanding.

## Implemented checkpoint: mobile OAuth

- A process-wide launcher guard prevents concurrent token/key-backup/origin
  controllers from overwriting the Android plugin's pending callback. Another
  request gets a retryable explanation; it does not launch a second browser or
  consume the first request's result.
- Code exchange requires the expected scheme, authority and `/cb` path, no
  fragment, one state/code/error value, and the matching PKCE state. Every
  cancellation/error branch checks its original session generation.
- Android debug application suffixes now select a distinct callback scheme in
  both the merged manifest and Dart's installed-package lookup. For the existing
  `xyz.block.buzz.mobile.parity_dev` package that is
  `xyz.block.buzz.parity-dev://auth/cb`. Production remains
  `xyz.block.buzz://auth/cb`; iOS is unchanged.
- Before a later coordinated dev APK rollout, the relay must allow
  `xyz.block.buzz.parity-dev` in `AUTH_MOBILE_REDIRECT_SCHEMES` alongside the
  existing production scheme. This task does not change or deploy that setting.
- The duplicate-callback overwrite is confirmed in the installed
  `flutter_web_auth_2` 5.1.0 Android implementation
  (`callbacks[callbackUrlScheme] = resultCallback`). The reported physical-phone
  chooser/MFA/back/spinner sequence has not been reproduced. Google already
  receives `prompt=select_account`; MFA and ephemeral browser policy are intact.

Account addition/list/switch/logout controls and concurrent account runtimes
are not implemented by this checkpoint. The reviewed storage/runtime/provider
dependencies above remain required; no account switching is exposed through
the unsafe existing workspace-apply path. App-exit behavior is unchanged.

## Desktop HTTP ownership checkpoint

Two existing native HTTP paths re-read mutable view state after waiting for a
rate limit. `query_relay_at` now captures its user credential before waiting,
while still generating fresh NIP-98 authorization after admission. An explicit
agent-signed submission captures its destination before waiting. A delayed A
request can therefore finish using A's credential and destination while B is
selected, instead of leaking B's credential to A or A's event to B.

Loopback fixture tests exercise those exact production functions with a held
admission gate, key/token replacement and relay changes. This is a narrow HTTP
regression fix, not proof that all agent/controller lifetimes support multiple
accounts. No native process registry, provider/model flow or shutdown code was
changed. The installed desktop binary is not replaced by library tests.

## Validation and later manual acceptance

- Mobile: 104 focused tests passed, including browser ownership, callback URI,
  stale error/cancellation, sign-in widgets, custody interoperability, existing
  token settings and the age-gate fixture affected by the launcher interface.
  Changed Dart files passed analysis and formatting.
- Android: Gradle configuration and actual debug main-manifest merge passed;
  the merged `parity_dev` callback is `xyz.block.buzz.parity-dev://auth/cb`.
  Release main-manifest merge could not resolve uncached AndroidX dependencies
  in offline mode. No APK was built or installed; no dependency upgrade or cache
  cleaning was performed.
- Desktop: three loopback HTTP ownership tests and 13 existing admission tests
  passed. Changed Rust files passed rustfmt; offline library Clippy passed with
  warnings in unchanged code. Tests did not launch or replace the desktop app.
- Hermit activation failed on this Windows checkout (`/pkg/hermit@stable/hermit`
  missing). Checks use the installed Flutter 3.41.7 and Rust toolchains/caches.
- Later, after parent-coordinated allowlist change and APK build, check a phone
  with production and dev apps installed: each login returns to the initiating
  app, one tap sequence opens one auth flow, Back/cancel then retry can finish,
  and Google MFA is unchanged. No physical-device reproduction or acceptance
  is claimed here.
- Native rebuild/relaunch remains a separate coordinated step. Full A -> B -> A,
  selected logout, account-local draft/PIN/read/mute state, agent/provider
  ownership and default-community account actions still need implementation and
  isolated acceptance before a real account switch can be tested.
