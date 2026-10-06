# Desktop chat list — stage 2

This stage follows `db3ae7ad6`. The user reported successful login into the app,
channel creation and message sending. Preserve that session, provider/model setup,
same-key recovery, conversations and drafts.

## Scope and behavior

- `useChannelsQuery` reads the selected community through the existing relay
  client. `CommunityQueryProvider` remounts its query client on community changes;
  persisted channel snapshots are keyed by relay and account. This is **one
  selected-community list**, not all-community aggregation. Community navigation
  remains available. No IDs from separate communities are combined.
- One Chats list contains joined stream channels (including private groups) and
  accessible, visible DMs. DMs do not require the stream `isMember` field.
  Unjoined public channels, archived channels, hidden DMs and hidden huddle backing
  channels keep their existing visibility rules. Forums keep their own surface.
- Personal pins come first; each partition uses actual `lastMessageAt`, newest
  first. Empty rooms fall below active rooms within their partition. Equal times
  use stable channel IDs, so metadata/name changes cannot move tied rows.
- Reuse `channelSortPreference`, `channelRecency`, the live-message hook and
  `channelStarsStorage`/`channelStarsSync`. No new pin backend or message-pin event.
  Pin/Unpin affects the owner's encrypted preference only, with durable local
  storage scoped by account and normalized relay. Legacy stars migrate to the
  first active scope once; their original data is retained.
- Existing icons, DM labels/presence, read/unread, mute, close DM, archive/leave,
  navigation and composer remain. Chats use fixed recent sorting; old section
  assignments/sort preferences remain stored but no longer split the chat list.
- No group creation/history changes, mobile work, server changes or deployment.

## Focused risk review

The concrete risks are filtering DMs as stream memberships, losing row actions
while combining render paths, bumping recency on metadata updates, and leaking
pins across accounts/communities or late async completions. Keep the existing
visibility/message seams and context menu; test real rendered rows and the pin
hook, including remounts, scope switches and pending remote reads. Keep native
app validation read-only; exercise mutations in an isolated mock harness.

## Acceptance

1. Mixed DM/private/public joined rows share one list; unjoined/hidden/archived
   rows remain excluded. Existing navigation and unread/mute actions still work.
2. A real message moves an unpinned chat above older unpinned chats; metadata
   updates do not. Pins stay above newer unpinned chats; Unpin restores recency.
3. Pin/unpin survive a remount; another account/community does not inherit them,
   even with identical channel IDs. Old async results cannot cross scopes.
4. Focused component/state tests, changed-file formatting and TypeScript pass.
   Verify the already-running DEV window after HMR without logout/restart or
   touching the user's messages/pins. Human sends and restart testing remain
   distinct from agent/mock validation.

## Validation

- Review completed against VISION.md, VISION_SOVEREIGN.md and TESTING.md:
  minimalism/elegance/correctness each 9/10. Fixed the DM membership gate,
  preserved channel-only owner/leave actions, kept the pin indicator clear of
  unread badges, and fenced deferred pin actions/remote results by scope. No
  remaining concrete blocker. The requested single list intentionally replaces
  per-section sorting while preserving tenant isolation and channel membership.
- Production component/hook tests passed: mixed rows and icons, private/unjoined/
  archived visibility, pinned partitions, empty/tied rooms, message versus
  metadata updates, persistence, account/community switches, legacy migration,
  stale reads and deferred actions. Existing recency and star-sync tests passed.
- TypeScript, E2E-mode frontend build, changed-file Biome and diff whitespace
  checks passed. Old split-list E2E selectors were updated, and manual-section
  tests now assert one list with retained preferences. The broad suites were
  not rerun; checks are limited to this milestone.
- Seven focused isolated Chromium scenarios passed: Pin/Unpin by pointer and
  Shift+F10, reload persistence, DM navigation/composer, live DM/channel recency,
  preserved forum sorting, row hover/selection and legacy-section consolidation.
  The mock native boundary needed its existing non-token AUTH
  responses (as in the stage-1 fixture); production auth was not changed.
- Native observation on 2026-10-06: the existing `Buzz Google Recovery DEV`
  process/window remained running. After HMR, its real WebView on 22446 showed
  one Chats list with four existing rows, no old split lists and no Vite error.
  The selected community and signed-in app remained available. Inspection used
  the window and read-only CDP on 22448; no agent sends, pin changes, logout,
  cache clearing, native build/restart or server deployment were performed.
- Windows Hermit activation still fails at `/pkg/hermit@stable/hermit` as in
  stage 1; checks used installed Node and repository dependencies. Git hooks
  contain only sample files. No hook was disabled; the local commit uses `-s`.

Human acceptance for stage 2 is still pending: choose two existing chats and
send a message in each to see the latest rise; Pin an older chat, send in another
chat and verify the pin stays above it; Unpin to restore message order. At a
convenient later restart, verify the personal pin remains. This work deliberately
leaves the current DEV app running. Group creation/history and all-community
aggregation require a separate decision; do not start them automatically.
