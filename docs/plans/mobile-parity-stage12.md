# Mobile parity — stages 1 and 2

Mobile is a complete first-device client, as specified in VISION_MOBILE.md.
This implements the same progression as the current desktop, not a companion
or a new identity system. No group/history work or server deployment is included.

## Plan and focused risk review

1. Use the explicitly configured Google recovery origin for ordinary first-run
   sign-in. Reuse GoogleKeyBackupService, secure pending keys, community storage
   and signed relay authentication. Only authenticated absence permits creation;
   errors preserve the key, and competing first devices restore the winner.
   Complete signed AUTH before saving/entering the community. Keep retry,
   configured selection and advanced custom origins.
2. Show joined streams/private groups and eligible DMs in one selected-community
   Chats list. Personal pins precede actual message recency; empty/tied rows use
   stable IDs. Reuse encrypted stars and their persistence/sync, adding account
   and origin isolation. Preserve forums, read/mute/lifecycle/navigation/drafts.
3. Exercise production widgets/providers and mock HTTP/WebSocket boundaries,
   format/analyze affected code, review the diff and make signed local commits.
   Preserve CLAUDE.md byte-for-byte; no push, reset, secret reads or live-account
   mutations. Use the existing Flutter 3.41.7 cache; no packaging for ceremony.

Concrete risks: custody tokens mistaken for message identity; proceeding before
AUTH; generating after an uncertain read/upload; stale login/cancellation results;
cross-community pins; filtering DMs as stream membership; metadata bumping recency.
Tests must fail when those production guards are removed. Existing desktop
custody protocol/state-machine tests are the compatibility reference; change its
behavior only if a concrete interoperability defect requires it.

## Acceptance

- Mobile-first: Google login with no desktop/key creates a local signing key,
  uploads through the existing authorized custody binding, authenticates to the
  default community and enters the app. Later desktop Google login restores the
  exact key/pubkey, then follows normal provider/model setup and community entry.
- Desktop-first: later mobile Google login restores that exact key/pubkey and
  enters its default community. Neither direction needs key entry/QR/pairing.
- Error/cancel/retry and concurrent first sign-ins never silently replace an
  established key. Configured communities and selected community remain intact.
- Mixed Chats, pin/unpin, restart persistence, account/origin switches and message
  versus metadata updates follow the desktop stage-2 contract.
- Mock transport and focused tests are distinct from real two-device acceptance.
  Android/iOS device runs and APK builds are reported only if actually performed.

## Verification

- Implemented both stages. Ordinary first-run uses the explicit configured
  recovery origin; new phones generate/upload locally and returning phones
  restore through the existing custody module. The same signed NIP-42 socket
  implementation must receive AUTH OK before the community is persisted and
  the app opens. Custody tokens never become message identities. Errors retain
  the pending key; cancellation, session generations and changed selections
  fence late completion. Custom community entry remains available.
- Chats uses one selected-community list, existing personal stars, scoped local
  storage and encrypted relay sync. Legacy pins migrate once to one origin and
  keep their original data. Failed migration is visible and retryable. Delayed
  old reads/publications and menus cannot write into a new scope. Forums keep
  their independent surface and sort settings; old section preferences remain
  stored. No all-community aggregation is claimed.
- Review found that mobile's message timestamp had been gated by notification
  eligibility. Fixed the real provider: own messages, muted chats and unfollowed
  replies now advance Chats recency, while unread/notification rules and forum
  behavior remain unchanged. Metadata and edit events do not advance it.
- Focused agent review against VISION.md, VISION_MOBILE.md, VISION_SOVEREIGN.md
  and TESTING.md completed: minimalism/elegance/correctness each 9/10. Checked
  atomic enrollment ordering, bounded admission, key generation authorization,
  concurrent first-device conflict handling, state generations, account/origin
  isolation, DM eligibility, accessible actions and recovery affordances. No
  remaining blocking finding in this scope.
- Flutter 3.41.7 / Dart 3.11.5: 251 distinct focused tests passed across the
  final runs, covering custody/sign-in/default entry, actual local WebSocket
  AUTH acceptance/denial, community persistence, Chats/widgets, message
  providers, personal preferences, delayed sync, and four channel-details
  accessibility/action/layout cases. The combined run initially exposed one
  legacy widget fixture that had not completed auth boot; fixing that fixture
  and rerunning its three tests passed. The final custody run passed all 13,
  including rejection of a misleading backup_exists body on HTTP 503.
- Changed-area `flutter analyze --no-pub` passed with no issues. Dart formatting,
  diff whitespace and the mobile file-size gate passed. The four dev recipe
  default/custom-origin forwarding checks passed with the exact recipe shell
  bodies and isolated Flutter/pgrep stubs; shell syntax checks passed. Hermit
  activation still fails at `/pkg/hermit@stable/hermit`; native cached tools
  were used and no hooks were disabled. The checkout has sample Git hooks only.
- Desktop production functionality is unchanged. Five Rust custody tests passed
  (`cargo test --manifest-path desktop/src-tauri/Cargo.toml --lib
  auth::key_recovery::tests:: --offline`), including a new shared public test
  vector exercising mobile-first restore and desktop-first upload wire shapes
  through resolve_backup. Flutter uses that same fixture through its production
  upload/restore service. These are transport-contract tests, not actual
  two-device Google acceptance. Existing desktop single-flight login, refresh
  lock, origin/key fences and immutable-backup conflict path were reviewed;
  native browser cancellation and OS credential storage were not exercised.
- Device discovery found Windows/browser targets only, no attached Android or
  iOS device/emulator. No Android APK/iOS package was built or installed. No
  real Google login/logout/key reset, account mutation, secret inspection,
  desktop app restart, server deployment, DB/schema operation or real messages/
  pin edits occurred. CLAUDE.md is byte-unchanged from the initial user state
  (SHA-256 E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855).

## Remaining human/device acceptance

1. On an isolated test phone with the configured dev build and no desktop/key,
   sign in with Google. Expect default community/app entry without a server
   chooser, key entry or QR. On a later isolated desktop installation, use the
   same Google account: verify the same public identity, then complete normal
   provider/model setup and reach the default community.
2. Repeat desktop-first then mobile-later using an appropriate separate test
   account/device. Do not reset the user's current desktop session to stage it.
3. In the selected community, verify mixed DM/private/joined Chats. Pin an older
   chat, send in another chat, and see the pin remain first while unpinned chats
   follow message recency. Unpin and restart to confirm persistence; check another
   community/account does not inherit the pin. Real device/OAuth acceptance is
   pending, separate from all automated checks above.

Stop at these two stages. Group creation/history and schema/deployment work
remain outside this task.
