# Message sync: shared client rules

Desktop and mobile receive and place live room events by the same rules. Each
rule below has one implementation per client and a shared table of cases,
[`test-fixtures/room-live-routing.json`](../test-fixtures/room-live-routing.json),
that both clients' tests read.

| Rule | Desktop | Mobile |
|------|---------|--------|
| Room bundle chunking | `desktop/src/shared/api/relayRoomBundle.ts` (`chunkRoomIds`) | `chunkChannelIdsForLiveSubscriptions` in `mobile/lib/features/channels/channels_provider_lifecycle.dart` |
| Event placement | `RoomLiveBundle` (`relayRoomBundle.ts`) | `RoomLiveRouter` (`mobile/lib/features/channels/room_live_router.dart`) |
| Fixture tests | `relayRoomBundle.test.mjs` | `room_live_routing_fixture_test.dart` |
| Account bundle | `desktop/src/shared/api/relayMeBundle.ts` (`MeLiveBundle`); features call `meLive.subscribe` (`relayMeLive.ts`) | `mobile/lib/shared/relay/me_live_bundle.dart` (`MeLiveBundle`); `RelaySessionNotifier.subscribe` routes matching filters to it |
| Account fixture tests | `relayMeBundle.test.mjs` | `me_live_bundle_fixture_test.dart` |
| Message store | `desktop/src-tauri/src/message_store/` (fed by `native_websocket.rs` and `relay.rs`) | `mobile/lib/shared/message_store/` (fed by `relay_session.dart`) |
| Message store fixture tests | `message_store/tests.rs` | `message_store_test.dart` |
| Forgetting a room | `message_store/purge.rs`; list from `get_channels` (`commands/channels/fetch.rs`: `member_ids`, `store_scope`), applied by `desktop/src/features/channels/lib/storeRoomPurge.ts` (from `refreshChannelsQuery`) | `mobile/lib/shared/message_store/purge.dart`; list trigger `_forgetStoredRoomsMissingFrom` (`channel_directory.dart`, from the membership fetch in `channels_provider.dart`) |
| Forgetting fixture tests | `purge_follows_shared_fixture` (`message_store/tests.rs`); triggers: `socket_notices_carry_the_relay_key`, `an_eose_capture_precedes_what_arrives_after_it`, `a_large_purge_runs_in_chunks`, `a_list_older_than_the_last_applied_is_refused`, `store_rooms_follow_membership_not_the_shown_list` (`fetch_tests.rs`), `storeRoomPurge.test.mjs` | `message_store_purge_test.dart`; triggers: `relay_session_purge_test.dart`, `channels_store_purge_test.dart` |
| Head spans | `message_store/head.rs` (writer: `writer.rs`; commands `message_store_head_sent`, `message_store_record_heads`); requests: `desktop/src/shared/api/relayStoreHeads.ts`, used by `channelWindow.ts` and `threadWindow.ts` | `mobile/lib/shared/message_store/head_coverage.dart`; requests: `queryRelayWithStoreHeads` (`relay_session_store.dart`, `store_heads.dart`), used by `store_head.dart`, `thread_replies_provider.dart`, `channel_preload.dart` |
| Head span fixture tests | `head_coverage_follows_shared_fixture` and the other tests in `message_store/head_tests.rs`; `relayStoreHeads.test.mjs` | `message_store_head_test.dart`; `store_heads_test.dart` |
| Store-first preload | `desktop/src/features/messages/channelPreload.ts` (`roomsNeedingRelay`; plan: `lib/storePreloadPlan.ts`; wait: `waitStoreLive` in `relayStoreCoverage.ts`; command `message_store_certain_rooms`); waves: `desktop/src/features/home/lib/inboxPreloadRun.ts`, run by `useInboxPreload.ts` | `ChannelPreloader._roomsNeedingRelay` (`mobile/lib/features/channels/channel_preload.dart`; plan: `store_preload_plan.dart`; wait: `waitStoreLiveRooms` in `relay_session_store.dart`); waves: `runInboxPreload` (`mobile/lib/features/activity/inbox_preload.dart`) |
| Store-first preload tests | `storePreloadPlan.test.mjs`, `channelPreload.test.mjs`, `inboxPreloadRun.test.mjs`, `relayStoreCoverage.test.mjs` | `store_preload_plan_test.dart`, `channel_preload_test.dart`, `inbox_preload_test.dart`, `coverage_test.dart` |
| Agent observer filter | `desktop/src/shared/api/observerLiveFilter.ts` | `observer_subscription.dart` (`observerLive*`) |

The account bundle's cases live in
[`test-fixtures/me-live-bundle.json`](../test-fixtures/me-live-bundle.json),
the message store's in
[`test-fixtures/message-store.json`](../test-fixtures/message-store.json),
forgetting a room's in
[`test-fixtures/message-store-purge.json`](../test-fixtures/message-store-purge.json),
head spans' in
[`test-fixtures/message-store-head-coverage.json`](../test-fixtures/message-store-head-coverage.json),
the store-first preload plan's in
[`test-fixtures/message-store-preload-plan.json`](../test-fixtures/message-store-preload-plan.json).

## Live room bundle

A room is a channel, DM or group. A thread is part of its room: replies carry
the room's `h` tag, so the room's stream carries its threads, followed or not.

- **One stream for every room.** The rooms the account belongs to share one
  live REQ per 128 rooms (the relay's explicit-`#h` cap per REQ): ids sorted
  and deduplicated, then split in order. A room does not get a REQ of its own
  when it is opened.
- **Filter.** Every channel event kind plus the thread-summary recount
  (39005), `limit: 0` and no `since`. The relay applies `since` to live
  matching, so a `since` would drop events stamped in the past within the
  relay's skew allowance.
- **Rooms outside the bundle.** A room opened before its chunk is live (cold
  start) or without membership (a browsed open channel) gets a one-room live
  REQ, closed once a live chunk covers it.
- **Replacing a chunk.** When the room set changes, the new chunk is opened
  first and the old one is closed only after the new one is live. Overlapping
  deliveries are expected.

## Placing an event

1. An event with an `h` tag belongs to that room.
2. Reactions (7) and their removals (5) carry no `h` tag:
   - on a one-room subscription, the event belongs to that room;
   - otherwise to the room of the event it references, the last `e` tag first:
     an event routed earlier (so a removal follows its reaction), or an event
     loaded in an open room. The room must be one the chunk carries.
3. An event whose room cannot be told is dropped. It references nothing on
   screen, and a page that later loads its target carries it in the page's aux
   closure.

## Who receives it

- **Open room and its threads** (the timeline, the thread panel, reply
  badges): every event of the room once, thread summaries included.
  Duplicates from overlapping chunks or a room's own REQ are dropped.
- **All-rooms consumers** (unread, notifications, chat list, inbox): member
  rooms only, never thread summaries. Each keeps its existing duplicate
  handling.

## Open room and thread requests

- **Room refresh after subscribing (desktop).** Opening a room fetches its
  newest window; once the room's listener is set up, the client fetches it
  again to close the gap between that window and the listener going live.
  That second request is skipped when the open's own window request went out
  while this open's room listener was attached and a live REQ carrying the
  room had reached EOSE, and both are unchanged when the refresh runs: the
  same listener, and each such REQ live since that EOSE (no CLOSED, socket
  loss or retirement in between). A request still running is waited for; if
  it fails or is replaced, the refresh runs. A window fetched without this
  open's listener (the background preload, an earlier visit) never counts,
  and the refresh after a reconnect always runs. This assumes the newest
  window is read from the relay's primary database: the relay may serve it
  from a read replica when `KUZZ_REPLICA_READ_MAX_AGE_MS` is set, and the
  window request has no way to pin the primary. Mobile subscribes before it
  fetches the room, so it asks once.
- **Live replies in an open thread.** A reply from the relay joins the
  thread's held replies without a request: desktop merges every one; mobile
  folds it into the thread's window when that window shows the newest page.
  Mobile still marks the thread stale (fetched again at once while open, or
  when reopened) when its window was opened at a reply and has newer pages
  to load, when the reply is not newer than the oldest loaded reply while
  older pages remain, while the thread's own request is running or its last
  one failed (a failed refresh after a reconnect may hide a gap), and while
  another send in the thread awaits proof (the reply's own echo does not
  count); and, as before, on reconnect and for a thread held under the
  reply's parent rather than its root.

## Reconnect

- Desktop re-sends each chunk unchanged and repairs the missed window over
  HTTP: one keyset walk per chunk covering all its rooms, from the chunk's
  last-seen event time (or its start) minus the 1865 s lookback. Every row
  goes to the device store, but rows older than the chunk's start minus 5 s
  are not handed to listeners: they were never live on it and must not show
  up as new notifications or activity.
- Mobile re-sends each chunk; the open room refetches its newest page and the
  open thread its replies, as before.
- Both apps also catch the device store up after every chunk EOSE (see
  [Coverage](#coverage)). Desktop skips this when its reconnect repair runs
  for the chunk on that connection; mobile has no repair, so it always runs.

## Account bundle

Live-only subscriptions about the account itself (settings documents,
contact list, agent personas, teams and devices, chat-list pings, sign-in
notices)
share two REQs instead of one per feature:

- **Authored:** `{kinds: authoredKinds, authors: [me], limit: 0}`.
- **Addressed:** `{kinds: addressedKinds, "#p": [me], limit: 0}`.

Features keep their own filters and listen through the bundle:

- **Which filters ride it.** Live only (`limit: 0`, no `since`), exactly one
  pubkey in `authors` or in `#p` (not both), every kind in that bundle's
  list, and no keys besides `kinds`, `authors`, `#p`, `#d`, `#t` and `limit`.
  Anything else, including a filter that asks for history, opens its own REQ
  as before.
- **Delivery.** Each listener gets the events its filter matches (kinds,
  authors, and each `#x` tag; `#p` and authors compare case-insensitively),
  each event once per listener.
- **Reconnect.** Each live bundle fetches what it missed from the moment the
  connection dropped minus the 1865 s lookback, up to 500 events, but never
  from before its earliest listener started (minus 5 s). Each listener gets
  only events from when it started (minus 5 s), so nothing from before a
  feature was listening shows up as new. A failed catch-up owes its window to
  the next reconnect.
- **Closed by the relay.** When the relay ends a bundle's REQ for good, the
  bundle is dropped and the next listener opens a fresh one. Mobile also tells
  current listeners; desktop features never reacted to their own REQ closing
  and still don't.
- **Lifetime.** A bundle opens with its first listener and closes once its
  last one leaves, after the current task, so a rebuild that subscribes again
  at once keeps it.

Kept out on purpose:

- **Filters that ask for history** keep their own REQ: read state, mobile's
  sidebar lanes that take their head on the REQ (sections, mutes, stars,
  names, sort), and the windows of mentions, approvals and membership
  notices. Moving one in means giving it `limit: 0` once it reads its own
  history first.
- **Mobile's agent devices** (30180/30181) take their last 200 on the REQ
  and so keep it; desktop's devices are live-only and ride the authored
  bundle.
- **Community-wide streams** (user status, presence, catalogs, custom emoji)
  are not about the account.

## Agent activity

Work frames from the account's agents (24200, `#p: [me]`) arrive on one REQ
per client:

- **Filter.** `limit: 1000`, `since` five minutes before the subscription
  starts ([`test-fixtures/agent-observer.json`](../test-fixtures/agent-observer.json)).
  A turn already running sent its first frames earlier; on reconnect the
  session replays from the last frame seen, up to the limit.
- **Duplicates.** Frames are deduplicated by sequence number and timestamp,
  so the overlap with what a reader already holds is harmless.
- **Agent online status** comes with everyone else's presence on the
  presence subscription; it needs no REQ of its own.
- **Start.** The REQ opened for the account's agents at launch waits until
  the channel list and the first room window have loaded
  ([Launch order](#launch-order)). Opening an agent's session or sending it
  a control starts it at once.

## Launch order

An account with data on the device draws from that data first; the server's
answers follow in the background, in this order, and never block the screen.

1. **Draw.** The active account is drawn from device storage: channel list
   snapshot, stored rooms ([Painting from the store](#painting-from-the-store)),
   and unread badges from the device's read markers (the relay's markers
   merge in later; markers only advance). No loading text while stored data
   exists; the splash only fades (no minimum time).
   Desktop also draws the Chats rows and other people's names and pictures
   from their [launch snapshots](#launch-snapshots), read when the
   community's query cache is made.
   Mobile draws the channel list, other people's profiles and the
   account's own profile from its [launch snapshots](#launch-snapshots)
   before the token and the socket; the channel list's server requests
   after the memberships run in parallel.
2. **Sign-in restore.** When the device holds a saved session whose bound
   account key is the key held here (desktop: `saved_session_ready`, local
   reads only), the app draws before the restore (relay info, token
   refresh, account check) and runs it in the background. Otherwise
   (legacy token accounts, no saved session, signed out here, or a session
   already ended this run) it waits for the restore as before. If the
   background restore ends the session, signs this device out, or yields
   another identity, the app reloads into the regular launch, which shows
   the signed-out screen or the new account. Mobile: right after the saved
   session is read, when its account (`principal_id`) is exactly the active
   community's key on the same server, the token session reports
   `resuming` (never `signedIn`: there is no access token yet) and the gate
   shows the app with no banner while the refresh runs; requests that need
   a token wait for it. A refused refresh signs out: chats pushed above the
   app are closed and the sign-in page shows (a sign-out, switch or removal
   in progress moves its own screens). Mobile reads its migration marker
   from preferences, not the keychain, before the first frame.
3. **Connect.** The relay socket connects once the restore has settled
   (signed in, needs login, or no session). A restore that times out while
   still retrying keeps the socket held until a token status event reports
   a settled state. After 3 s held, the existing connection card shows
   "connecting" so the drawn list does not look live.
4. **The room on screen and the channel list** ask first; profiles follow
   as rows need them.
5. **First preload wave** right after the socket connects, once the room on
   screen has its window (at most 3 s), with 20 rows per room and no store
   check ([Store-first preload](#store-first-preload)).
6. **Agent indicator replay** ([Agent activity](#agent-activity)) after the
   channel list and the first room window (with no room open, 2 s after the
   channel list; at most 10 s after launch). Mobile starts it once the
   channel list has resolved, and publishes the replayed frames as one
   update per 16 ms.
7. **Background preload wave** and cleanup, when the user is idle. Mobile
   queues forgetting left rooms ([Forgetting a room](#forgetting-a-room))
   until after the first frame and the first window of the room on screen
   (at most 5 s), then runs each list as its own task between frames.

## Message store

Every room event the device receives is kept on the device, with the spans
known complete ([Coverage](#coverage)). Opening a room or thread draws its
stored newest window at once while asking the relay
([Painting from the store](#painting-from-the-store)); a certain window
(below) lets the preload skip a room
([Store-first preload](#store-first-preload)). Older pages still load from
the relay.

- **One file per server and account:** `messages/<first 8 bytes of
  sha256(origin) in hex>-<account pubkey>.db`, where the origin is the relay's
  `https://host[:port]` (a `wss://` socket and an `https://` query to the same
  server share it). Desktop keeps it under the app data directory, mobile
  under the application support directory.
- **What is captured:** events from the relay socket after its AUTH (desktop:
  the account is the author of the socket's AUTH event, or for a token AUTH
  the account the token was issued to; mobile: the session's account) and event arrays returned by `/query`. The scope is taken when the
  socket authenticates or the query is sent.
- **What is kept:** room content kinds (9, 40001, 40002, 40008, 40099,
  43001–43006, 45001, 45003, 48100–48103) with an `h` tag; overlays (5, 7,
  9005, 40003), indexed by every `e` tag they reference, with an `h` tag except
  for reactions (7) and removals (5); and thread summaries (39005), the newest
  per thread root (first `e` tag). Anything else is ignored.
- **Thread placement:** the same rule as live placement: the last
  `reply`-marked `e` tag is the parent, the `root`-marked one (else the
  parent) the root. An event without a `reply` marker is top-level.
- **First copy wins.** An event id is written once; later copies are ignored.
- **Never blocks receiving.** Writes go through a queue per file and are
  batched (up to 512 rows per transaction). The queue counts receives, not
  events: one relay answer is one entry however large (up to 4096 entries;
  mobile also caps waiting events at 50000 as a memory guard). A full queue
  or a failed write drops that receive and marks the file `lossy`, so a later
  catch-up does not take it for complete.
- **Sign-out** of a server (or removing that community) deletes that
  account's file for that server only and ends its scope: answers to requests
  sent before the sign-out write nothing. The same account's files from other
  servers, and other accounts' files, are untouched and keep recording.
- **Only the signed-in account's traffic is kept.** Desktop `/query` answers
  signed with other keys (agents, robots, team adoption) are not stored, and
  an empty answer creates no file.
- **Schema** is versioned (`user_version` 1). A file from a newer app is not
  opened.

### Coverage

A room's coverage is the set of spans `[lo, hi]` (unix seconds, receive time)
over which every event of the room in the repair kinds (9, 5, 7, 9005, 40001,
40002, 40003, 40008, 40099, 45001, 45003, 48100–48103) is in the store.
Workflow kinds 43001–43006 and thread summaries are stored but not covered.
Merging rules and cases: `test-fixtures/message-store-coverage.json`.

- **Who records:** a live REQ that names explicit rooms (at most 128) and
  carries every channel event kind (a room-bundle chunk or a room's own REQ).
- **EOSE at L:** records `[L, L]` for its rooms; from then on the REQ receives
  every new event.
- **While live:** an event arriving on the same connection extends the span to
  `[L, now]`, at most once every 60 seconds (no timers). After a reconnect
  nothing extends until the next EOSE.
- **Reconnect repair (desktop):** a completed walk from `since` records
  `[since, now]`.
- **Catch-up:** at EOSE (desktop: unless its repair runs), for the chunk's
  rooms that have a span starting before L, take the oldest of their latest
  `hi`, minus 1865 s (relay future tolerance 900 + storage fence 960 + 5). One
  keyset walk (`#h`, repair kinds, `since`, `before_id`, 500 per page; the
  first page has no `until`, so rows the relay stamped after the local L are
  not cut off by a slow clock) into the store only; nothing is shown. When it ends within 4 pages it
  records `[since, L]`. A longer or more than 7-day gap records nothing and
  is left for opening the room. Rooms without coverage are never fetched.
- **Order and loss:** spans go through the same write queue as events, so a
  span is recorded only after the events received before it are written. A
  dropped event or failed write clears every span of the file, and any later
  span starting at or before the time of the loss is not recorded.
- **Version:** the file records which coverage version wrote its spans
  (`sync_state.coverage_version`, now 2). On open, spans of another version
  are deleted; desktop reads also trust no span until the version matches.
- **Scope (desktop):** each span carries the store scope (server file,
  account, sign-out epoch) current when its live REQ reached EOSE; a span
  arriving after that scope changed, or with no scope (signed out), is refused, and a catch-up whose REQ was
  retired stops before recording.
- **Sign-out** deletes coverage with the file.

### Head spans

A newest window the relay answers (a room's channel window, or a thread's
reply window, with no cursor) holds every row of that window from its oldest
row on. When a live REQ carrying the room was receiving all along, the store
records this as a head span, so a quiet room whose rows are older than any
coverage can still open from the store. Cases:
`test-fixtures/message-store-head-coverage.json`.

- **Streams:** `head` for the room's window, `head:<root>` for a thread's,
  next to the room's own `room` spans. One span per stream. The next answer
  replaces it, except that a span reaching further back (older `lo`, or the
  whole history) is kept while a room span joins its `hi` to the new L: it
  keeps its `lo` and takes `hi` = the later L. One no longer joined is
  replaced. The coverage version stays 2.
- **Span:** `lo` = the answer's oldest row `(created_at, id)`, or `0` (whole
  history) when it had no more; `hi` = L, the latest EOSE of the room's live
  REQs at the request (see the token). In relay order (`created_at DESC, id
  ASC`) the span holds every row at or before `lo`.
- **Token:** as the request goes out, the client takes every live REQ
  carrying the room that has reached EOSE on this connection (desktop: each
  REQ's coverage object; mobile: each REQ's EOSE record and the connection
  generation). The span is recorded only when each of them is still the same,
  unchanged, when the answer arrives (no CLOSED, socket loss, retirement or
  new EOSE in between). No such REQ means no span. Unlike the room refresh
  rule above, the room's listener is not part of it: the store only needs
  the live REQ.
- **Mark:** before sending, the client queues a mark for the request's rooms
  in the store's write queue (its capture) and waits for it. The answer's
  spans carry that capture; a span with any other capture (a plain one, a
  mark already answered or evicted) is refused, as is one whose scope ended.
  A failed request or a token that no longer holds releases the mark with
  no spans. The mark keeps its rooms; a span for any other room is refused.
  At most 64 marks wait per file (the oldest is dropped), a mark not
  answered within 2 minutes expires, and each remembers at most 4096 rows
  (beyond either, its answer records nothing).
- **Deleted rows:** in the same transaction as the span, stored rows of the
  window from `lo` on (every one, for a whole history) up to L
  (`created_at <= L`) that the answer lacks were deleted on the relay and are
  dropped with their target index entries and thread summaries. Rows newer
  than L are kept, and so are rows written to the store after the mark:
  they may have reached the relay after its answer was read. Room windows
  look at top-level rows and direct broadcast replies, thread windows at the
  thread's replies (9, 40002); each leaves the other's rows alone.
- **Refused** like a room span: after a loss not yet written down or one at
  or after L, and for a room forgotten after the mark.
- **History changes:** any relay-signed `history_changed` for a room
  (`all` or `since_join`, newer or not) deletes the room's `head` and
  `head:<root>` spans in the transaction that handles it, since rows a head
  said were absent may now be visible. A span whose mark went out before
  such a notice is refused.
- **Consistency:** newest room-window filters carry `"consistency":
  "strong"`, but the relay's window path does not honour it yet (see
  [Limits](#limits)). Thread-window filters cannot carry it: the relay's
  thread-window parser rejects unknown fields.
- **Which requests:** on both platforms, only the newest window a room or
  thread opens on from the relay, and the background preload, opt in; and
  only a request for at least the open window's 50 rows records. So the
  room preload (50 rows) records, the thread preload (20 replies) and
  desktop's workflow message picker (25) never do, and older pages, a
  link's walk to a reply and catch-up never record. Requests are not
  skipped yet; stage 5-6 does that.
- **Matching thread pages:** a thread page is matched to its request by
  channel and root (its bounds' `h` and `e` tags). Its `d` binding is not
  recomputed: it hashes the relay's resolved host and normalised kinds. So
  a request naming the same root twice, an answer with two pages for one
  root, or bounds whose `direction` is not `older` record nothing for that
  root.

### Painting from the store

Paintable is not the same as certain. A room or thread opened for the first
time in this run (as defined below) reads its stored newest window and asks
the relay at the same time; neither waits for the other.

- **Stored rows draw at once** when the store answers before the relay, or
  when the relay fails first. They draw before the connection is up and
  before the room's live subscription has loaded: no EOSE, coverage or
  write wait is asked for. The rows and bounds are the ones a certain read
  would give (below).
- **Marked unconfirmed:** a painted window never counts as a window fetched
  during this visit, so it neither decides a position for good
  ([Opening at a position](#opening-at-a-position)) nor vouches for the
  live token.
- **The relay's answer replaces it silently** as the refresh that follows.
  That refresh takes the request already out: one relay request per open.
  On desktop the request is sent once the effects of the render that opened
  the room have run, so the room listener attached in that render goes with
  it and the refresh after subscribing is not needed.
- **Nothing stored, or a failed store read:** the relay's answer is drawn
  when it arrives (the only case that shows loading). A relay answer before
  the store read is drawn instead, and the late stored rows are ignored.
- **Certain** (below) keeps its rule; it now only decides what the
  store-first preload may skip.
- **How far back it is whole:** each paint also returns `complete_from`,
  the earliest time from which the store holds the room's events without a
  gap, or none when unknown. It chains the room's spans (a room span vouches
  from `lo + 1865`, a head span from its `lo`, or from 0 for a whole
  history) back from the newest painted row, and is none when the room is
  not current or has ever stored a job row. A thread paint uses the room
  spans and the thread's head span. See [Opening at a
  position](#opening-at-a-position) for how a gap is used.
- **Same account and server:** a paint names the account and the relay it
  is for (desktop: from the community's query cache); the store answers
  nothing when the open store belongs to another account or server.
- **Thread confirmed by its relay page:** a painted thread counts as
  confirmed only once the relay's page has been folded into it; a refresh
  that was dropped (the thread closed) leaves it unconfirmed.

Cases: `test-fixtures/message-store-paint-first.json` (`paint`). Desktop:
`storeRoomHead.ts`, `threadWindowCache.ts`; mobile: to follow.

### Opening a room from the store

On a room's first fetch, the client reads the room's newest window from the
store and draws it while the relay is asked
([Painting from the store](#painting-from-the-store); before that rule it
used the window only when certain). A first fetch is one for a room this
run has not yet fetched from the store or the relay, the background
preload's requests included (a room the preload left to the store is not
fetched; see [Store-first preload](#store-first-preload)). Desktop tracks this per query cache (a community switch starts a
new one); mobile per room notifier, which lives for the run, and for the
preload per account and server. A stored window only draws the first frame: once that fetch has
settled, the relay's answer to the request sent with it (one request; on
desktop, the refresh after the room's live subscription joins it) replaces the stored rows (stored rows inside its span that the relay no longer has are dropped; older rows that join it stay, see [Opening at a position](#opening-at-a-position); live arrivals stay).
A painted window is not certain, so this request is always sent (stage 5
may later skip it for certain rooms once the store-vs-relay log shows no
differences). Later refreshes always
ask the relay. Cases: `test-fixtures/message-store-room-head.json`.

- **Rows:** timeline kinds of the channel window, top-level (no `reply`
  marker) or depth-1 broadcast replies (parent is the root), newest 50 by
  `created_at DESC, id ASC`. A row targeted by a removal (9005) or by its
  author's deletion (5) is left out.
- **With the rows:** overlays targeting them (5, 7, 9005, 40003), removals of
  those overlays (5, 9005), the stored thread summary of each row that counts
  at least one reply (the relay sends none for `reply_count` 0), and one
  `39006` bounds event (`d = <room>:head`, unsigned, `next_cursor` = the
  oldest row) so older pages load from the relay as usual.
- **Certain** when a live REQ carrying the room reached EOSE at L on the
  current connection and has not been closed since (the client passes L;
  none means not certain), the room has never stored a job row
  (43001–43006), and either:
  - the window has 50 rows and one room span has `lo ≤ created_at of the
    50th row − 1865` and `hi ≥ L`; or
  - a [head span](#head-spans) reaches the window's oldest row (whole
    history, or the window has 50 rows and its 50th row is at or before the
    head's `lo` in relay order), and one room span has `lo ≤` the head's
    `hi` and `hi ≥ L`. A room with fewer than 50 rows is certain this way
    once a whole-history head is recorded. Job rows show in the window, but
  live REQs do not carry them (that set is frozen), so coverage cannot vouch
  for them. A room's first job row is still missed until something stores
  one; the relay request after a stored open (above) corrects that frame.
- **Written first:** the certain read waits (at most 0.5 s) until every
  event received before it is written. A writer that is gone, still busy
  after the wait, or holding a loss not yet written down means not certain.
  The paint read does not wait: rows still in the write queue are missing
  from the first frame and arrive with the relay's window.

### Store-first preload

The background preload (the rooms behind the chat list's top rows) asks the
store before the relay, so rooms it already holds certainly cost no request
and open from the store. Desktop and mobile behave the same. Cases:
`test-fixtures/message-store-preload-plan.json`.

- **Start:** never ahead of the room on screen ([Launch order](#launch-order)).
  Desktop's first wave goes right after the relay socket connects, once no
  room's open window is out (at most 3 s); mobile's first wave goes after
  the first frame as soon as the session is usable (checked every 100 ms
  for the first 3 s, then at the usual intervals), once the room on screen
  has its first window (at most 3 s). The first wave leaves out the room on
  screen; a room the user opens meanwhile loads itself and is left out.
- **Waves:** the rooms behind the top 12 rows in one request (20 rows per
  room, asked at once with no wait or store check below; 20 rows record no
  head span), then their threads four per request; then the rooms of rows
  13 to 48 not already loaded, 12 per request (50 rows), and their threads,
  each request waiting until the user is idle (no input for 1.5 s, nothing
  loading in the foreground, app visible; mobile also keyboard down and
  relay usable).
- **Wait (background wave only):** each room request first waits, at most 3 s, until a live REQ
  carrying each of its rooms has reached EOSE and the catch-up (or
  reconnect repair) begun there has ended, completed or not. With store
  opens off, no store or an untrusted clock (desktop reads it through
  `message_store_clock_trusted`), nothing can be certain: it does not wait,
  and every room counts as without a token. Signing out, switching account
  or leaving Home ends the wait; nothing is then requested, and the request
  is also dropped if the account or server changed meanwhile.
- **Check (background wave only):** rooms with a token (a live REQ that reached EOSE) are asked of
  the store (desktop: one call, one drain), with the rule of [opening a room
  from the store](#opening-a-room-from-the-store); the call returns each
  certain room's window. A certain room is not requested and not marked
  fetched; its window is kept as a paintable copy (desktop: a stored, stale
  query that is never saved as head cache; mobile: written as the head
  cache), so the first open paints it at once, then reads the store and
  asks the relay once. It is looked at again after 60 s (failed rooms after
  5 minutes). The rest, rooms without a token included, are requested in
  one batch as before (recording [head spans](#head-spans)) and marked
  fetched. A background-wave request checks again that the user is idle
  after the store check, right before it goes out.
- **Log:** one line per room request:
  `[preload] certain N / fetched M / skipped (no token) K`, where M counts
  every room requested and K those of them that had no token, so were never
  checked.
- **Not yet:** opening a certain room still asks the relay right after the
  stored window is drawn (stage 5-6 drops that request). Threads are
  preloaded as before; the 20-reply thread preload never records a head.

### Opening a thread from the store

On a thread's first load (no reply page held for it: desktop's thread
window state, mobile's thread window; both are released some time after the
thread closes, and the next open counts as first again), the client reads the
thread's newest replies from the store and draws them while the relay is
asked ([Painting from the store](#painting-from-the-store)). As with rooms,
the relay's page then folds in, from the same single request. On desktop, a load that must include a linked reply skips
the store (its walk to the reply pages the relay anyway); mobile opens linked
replies with a separate relay window (`openAt`). Later loads and older pages always ask
the relay. Cases: `test-fixtures/message-store-thread-head.json`.

- **Rows:** replies of the thread-window kinds (9, 40002) whose root is the
  thread's root, at any depth, newest 50 by `created_at DESC, id ASC`. The
  root itself is not a row. A reply targeted by a removal (9005) or by its
  author's deletion (5) is left out. No overlays or summaries, as the relay
  sends none (`include_aux` false).
- **Bounds:** a full window has more (`next_cursor` = the oldest reply). The
  answer ends with one unsigned `39007` bounds event (`h` = room, `e` = root,
  content `{version: 1, direction: "older", has_more, next_cursor}`).
- **Certain** when a live REQ carrying the room reached EOSE at L on the
  current connection and has not been closed since, and either one room span
  has `hi ≥ L` and `lo ≤` (full window: the oldest reply's `created_at`;
  fewer replies: the root's `created_at`, which must be stored in the room)
  `− 1865`; or the thread's head span (`head:<root>`) reaches the window's
  oldest reply as for rooms and one room span has `lo ≤` its `hi` and
  `hi ≥ L`. A quiet thread is certain this way without its root stored.
- **Written first:** as for rooms (the paint read does not wait).

### Launch snapshots

Desktop keeps small snapshots of what the first screen shows in the same
store file (table `snapshot`, one blob per kind), so they are per account
and server, and a sign-out deletes them with the file. They only draw; no
snapshot counts as a stored room or takes part in [forgetting a
room](#forgetting-a-room).

- **Read and written like a paint:** each call names the account and the
  relay; the app answers nothing (and writes nothing) when they are not the
  active ones, or after that account signed out of that server. Writes go
  through the file's write queue (a full queue drops the snapshot, never
  marking the file lossy). One read is allowed for any other account
  signed in on this computer and not signed out of that server: background
  notifications for an account that is not active read its `profiles`
  (read only; writes stay with the active account).
- **Chats rows (`chat_rows`):** the first 200 rows of the relay's chat list
  as last shown, saved 2 s after they change. At launch they draw until the
  relay's first page of this run arrives, which replaces them in its order;
  a relay that turns chat state off drops them. While they draw, this
  device's read positions apply over their kept counts and marks (opening
  a room clears its badge), and thread rows have no menu (pin, unfollow,
  mark unread) until the relay's chat state is on.
- **Profiles (`profiles`):** other people's names, NIP-05 and picture URLs,
  the 500 most recently answered, read into memory at launch before any
  profile request; relay answers overwrite entries (a newer entry is never
  replaced by an older stored one) and are saved 1.5 s later. A save waits
  until that account's stored profiles have been read, and is built when it
  is written, so it never replaces them with less; switching the active
  account writes waiting saves at once (refused if no longer active).
  Pictures draw from the stored URL through the image cache. The labels
  earlier builds kept in localStorage per server (no account split) move to
  the account opening that server only when it is the only account signed
  in there; the merged set is written and the key deleted when the store
  has it. With more than one account signed in on that server, the key is
  deleted without moving anything.
- **Opening from a row:** a room the channel list does not hold yet (list
  still loading, a new DM, a notification, a deep link, a search result)
  opens at once from its Chats row (name, kind, member) instead of a
  loading screen; the stored window then paints. That channel only stands
  in for display: it names no members (nothing routes to, mentions or marks
  agents from it), the composer is replaced by a loading line until the real
  channel arrives, and a DM is titled by its people's stored profile names,
  else a short id. A loaded list lacking the room is fetched again once. The
  relay's "not found" for the room, or an answer that it cannot be opened,
  keeps the existing handling.
- Own profile stays in its own per-account cache (`selfProfileStorage`).

Desktop: `deviceSnapshot.ts`, `chatRowsSnapshot.ts`, `chatRowChannel.ts`,
`userLabelStorage.ts`, `message_store/snapshot.rs`.

Mobile keeps the same `snapshot` table in its store file (key, value,
time), wiped with the file at sign-out. The channel list (`channels`) is
saved 1 s after the relay's list is shown and drawn at launch until the
relay's list replaces it; it never counts as the loaded list nor keeps a
room for forgetting, and an account or server switch clears the shown list
at once. Other people's profiles (`profile:<pubkey>`, newest 500, saved
after each batch of changes) are drawn at launch and each is asked for again once;
the relay's answer overwrites it. The account's own profile
(`self_profile`) draws before its fetch. Chats rows, search results,
the thread's group button, notifications and deep links open a room the
list lacks from its Chats row (or its id) at once; a deep link to a room no
row names waits for the relay's list, and a room missing from it keeps the
not-found handling. Mobile: `snapshot.dart`, `channel_list_snapshot.dart`,
`profile_snapshot.dart`, `chat_row_channel.dart`.

### Forgetting a room

The store drops its copy of a room the account can no longer read, or whose
older rows it should no longer see. Cases:
`test-fixtures/message-store-purge.json`.

- **What is forgotten:** the room's rows (all, or those created before a
  time), the room-less overlays (reactions, deletions) aimed only at them
  (a deletion also naming a row of another room stays, or that row would
  come back), their thread summaries (every summary of the room when all
  rows go) and all of the room's coverage. Other rooms are untouched.
- **Captures:** every input queued for a file (a receive, a span, a list)
  takes the next number of that file's sequence. A capture is the scope
  (server file, account; desktop also the sign-out epoch) plus the number
  reached when it was taken: when a live REQ's EOSE arrives (desktop takes
  it in Rust as the socket delivers the EOSE frame and hands it to the app
  with the span; mobile when the session handles it) and when a membership
  fetch starts. The writer remembers, per room, the input that first wrote
  it and the last one that forgot it. The numbers live in memory only: a
  capture never outlives its writer (the queue dies with the process, and a
  new writer only starts for a new scope), so nothing older has to be kept.
  No clock is compared.
- **Fence:** a coverage span captured before a room was last forgotten is
  refused for that room (other rooms in the span are kept), so no span from
  before the purge can vouch for rows that are gone. A span captured after
  it is kept, however early it starts.
- **Left mark:** each room has a persisted mark of when the account left it
  (`room_purge.left_at`). It is set by a relay-signed `member_left` by the
  account (`target`, or `actor` when there is no target) or `member_removed`
  naming it, when newer than the room's last handled join; and when a list
  drops the room, as of the room's newest stored row (no mark when nothing
  was stored). A rejoin acts only on a room with a mark.
- **Access lost (list):** the list is membership, never what is shown:
  every room whose kind:39002 names the account (`#p`), whether or not its
  kind:39000 parses (huddle backing rooms), hidden DMs included, plus
  channels the pending-owner overlay marks member. Desktop gets it from
  `get_channels` (`member_ids`, also on a not-modified answer); mobile from
  the paginated membership fetch, never the push-cached list. Both page the
  membership query until an empty page, so a relay serving short pages
  cannot end it early; a mobile page with nothing new (a relay repeating
  itself) ends paging and that list forgets nothing. Never from the live
  bundle set. The file keeps the last list applied
  (`sync_state.kept_rooms`). Only a stored room of that list which the new
  list lacks is forgotten and marked left. Rooms no list had (open channels
  only browsed) are never forgotten by a list. The first list (none kept
  yet, e.g. after the upgrade) is only recorded. A failed fetch forgets
  nothing, nor does a membership answer naming no room, whatever is shown
  joined (checked before the shown rooms are added).
- **Bad list guard:** a list that would drop more than 5 kept rooms and
  more than half of them is not applied (logged; the kept list stays).
- **Joined during the fetch:** kept are the list plus the rooms shown as
  joined at settlement (desktop: `isJoinedChat` in the authoritative list
  and the displayed cache; mobile: members in the channels state). A kept
  room first written after the fetch's capture is left alone too and stays
  kept for the next list. Mobile applies the list later
  ([Launch order](#launch-order)) and then also keeps the rooms shown as
  joined at that moment; the capture taken before the fetch still decides
  scope and what may be forgotten.
- **Same scope only:** the capture is taken before the fetch for the same
  identity (desktop: by `get_channels` from the same pubkey it fetches for;
  mobile: the capture's account must be the fetching pubkey). The store
  refuses the list when that scope changed since (community or account
  switch, sign-out). Desktop also drops a list whose fetch started before
  the last one acted on, in the app and again in the writer (a list
  captured before the last one applied is refused).
- **Rejoined (list):** a stored room marked left that a list has again is
  forgotten whole (its rows came from an earlier membership or arrived
  while the account was out) and loses the mark. Unmarked rooms new to a
  list are kept.
- **Rejoined (notice):** the relay sends `member_joined` (`target` = the
  account) on every join, the first one too. Each newer one is recorded
  (`joined_at`); only when the room's left mark is older than it does it
  forget the room's rows created before it (the notice itself is kept) and
  all its coverage, and clear the mark. A first join keeps the room's rows.
- **History hidden:** every newer `history_changed` is recorded
  (`history_at`); one with `history` = `since_join` forgets the room's rows
  created before it and all its coverage (the notice and newer rows, sent
  to this account under the new rule, are kept). `all` forgets nothing, and
  a `since_join` older than a handled `all` is ignored.
- **Any delivery:** a notice counts however it arrives: a live REQ, a
  replay before EOSE, a `/query` answer, an older page or catch-up.
- **Signed by the relay:** the notice's `pubkey` must be the relay's own key,
  NIP-11 `self` (desktop learns it from the identity archive's relay info
  fetch, also run after each list purge; mobile from `relayInfoProvider`
  after a list purge, noted only while that list's scope is current). Until
  the key is known, no notice forgets anything.
- **Newer only:** the marks are kept across restarts; a notice not newer
  than the room's mark of its kind is stored but ignored.
- **Order and cost:** forgetting goes through the file's write queue, after
  the events and spans received before it; a notice is acted on in one
  transaction with its own write. The writer keeps the file's stored rooms,
  left marks and kept list in memory (loaded once when it opens the file,
  updated as it writes and forgets), so a list never scans the file, and a
  list equal to the kept one is skipped unless a room was first stored
  since or one of its rooms is marked left. Only a 40099 naming the account
  (or a `history_changed`) is written on its own; other relay messages are
  batched. After a failed purge the writer reloads its rooms from the file.
  Desktop keeps the EOSE capture of at most 512 subscriptions, dropping the
  oldest. A list's purge runs 16 rooms per transaction, taking in newly
  queued input between chunks (desktop moves it to an in-order backlog so
  the bounded queue cannot fill; mobile yields). A purge deletes through
  the `targets_id` and `summaries_room` indexes.

### Limits

- **since_join rooms:** the relay hides rows from before the reader joined.
  The store keeps only what the relay sent this account. Rows kept from an
  earlier membership (left, then re-added) are forgotten by the rejoin
  notice once the leave is known (its notice, or a list that lacked the
  room). A leave and rejoin between two lists whose leave notice never
  reaches this device, or arrives only after the rejoin (an older page read
  newest first), leaves those rows until the relay's answer replaces a
  stored window.
- **The leaver's own notice:** the relay usually ends the leaving
  account's subscriptions before it sends `member_left`, so this device
  rarely receives it. A rejoin then purges only because a list dropped the
  room in between; a leave and rejoin within the same second (equal
  `created_at`) is not seen as a rejoin.
- **Left mark from a list:** it is the room's newest stored row, a lower
  bound of the real leave: a rejoin notice between that row and the actual
  leave would count as a rejoin and forget rows once more (they are fetched
  again).
- **Rooms first stored during a list:** mobile counts a room written in the
  same batch as a list's capture as new and keeps it; desktop forgets it.
  Both are corrected by the next list.
- **Opening the file:** loading the stored-room set and building the
  `targets_id` index once run on the writer before it drains the queue; on
  a very large file a startup burst could overflow the queue (the file is
  then marked lossy and coverage cleared).
- **After a live forget:** live coverage for that room is refused until its
  subscription's next EOSE (the capture is older than the forget). Only a
  speed cost: the room opens from the relay meanwhile.
- **Relay key:** until the relay key is known in this run (the first list
  purge or identity archive read), notices forget nothing.
- **Deletions without an event:** apart from [head spans](#head-spans),
  the store drops a row only for a removal (9005) or author deletion (5) it
  has received. A row the relay deletes
  without publishing such an event (an admin deletion today) cannot be seen
  from the store, so a stored window may show it until the relay's answer
  replaces it. The relay request after every stored open covers this until
  the relay keeps a deletion record clients can fetch (a later stage).
- **Job rows:** see the room's certain rule above.
- **Head consistency:** the relay serves channel and thread windows through
  its routed read path, which may use a read replica when
  `KUZZ_REPLICA_READ_MAX_AGE_MS` is set, and ignores `consistency` there
  (only the catch-all query path honours it). A lagging replica could
  answer without a row the store already has, and the head would drop it
  (the live REQ brings it back only if it is sent again). Production has no
  read replica today; the relay must honour `consistency: "strong"` on the
  window paths (and accept it on thread windows) before replicas are used.
- **Head drop range:** rows the answer lacks are dropped only from `lo` up
  to L; newer ones wait for a later head. A row at or before L that the
  answer lacks only because its read lagged (see head consistency above)
  is still dropped.
- **Thread preload heads:** the thread preload asks for 20 replies, below
  the open page's 50, so it records no head; a thread opens from the store
  only after it was opened from the relay once (or a whole history was
  recorded then). Desktop and mobile behave the same.
- **Build switch:** `VITE_KUZZ_STORE_OPEN=off` (desktop) or
  `--dart-define=KUZZ_STORE_OPEN=off` (mobile) opens every room and thread
  from the relay. Events are still stored and covered.
- **Head cache:** only a relay window is saved as a room's head cache, never
  a window opened from the store. One exception on mobile: the preload
  writes a certain room's stored window as its head cache when the saved
  one is missing or older than the room's newest message (see
  [store-first preload](#store-first-preload)).

### Clock check

Coverage times and L come from this device's clock. Each successful `/query`
answer's `Date` header is compared with that clock; while the last
difference is over 60 s, or no answer has been compared yet, rooms and
threads open from the relay. The difference is not corrected.

### Store-vs-relay log

When the relay's answer replaces a window opened from the store, the client
compares it with the relay's page itself (never the merged result, and
nothing when no relay page arrived), by row ids inside the range both cover: from the newer of the two
oldest rows up to the stored newest row (rows that arrived live afterwards,
or fell off the end of the relay's page, are not differences). It logs one
line: `[message-store] <room|thread> <id>: store matches relay (<n>)`, or
`store differs from relay` with the ids missing from the store and the ids
only the store had. Rooms compare timeline rows of the newest page; threads
compare replies. For rooms it also compares, for rows both hold, what is
aimed at them (edits, reactions, removals: ids of events whose `e` tag names
a shared row, leaving out any its own answer removes, as the relay does) and
their thread reply counts (`reply_count` of the newest 39005 per root; the
rest of a summary depends on the reader's blocks). Nothing newer than the
store's newest event is compared there. Thread pages carry neither. Each run
keeps counts (matches and differences per room and thread stream, and
differences by type) and logs them at most once a minute as
`[message-store] comparisons this run`: the evidence for turning off the
follow-up request.

## Opening at a position

A room opens at, in order: a linked message, the first unread message, where
the reader left it, the newest message. The list is drawn once, already at
that position.

- **Decided on the rows held, on the first frame:** stored rows (a paint, a
  head cache) and the device's read markers decide at once when they reach
  back to the read marker: the first unread row among them, else the
  remembered row if held, else the newest. A store paint decides only when
  it has no gap back to the read marker: its `complete_from` is at or
  before the marker, or no painted row is unread, or the room was never
  read. For a thread the reply before the first unread one stands in for
  the marker; when the first unread reply is the oldest loaded, the paint
  decides only if older replies can be paged or `complete_from` is 0 (whole
  history). Otherwise an earlier unread row may be missing, and the open
  waits for the relay's window. Only when every held row is
  unread and older history exists, or the remembered row is older than
  them, is the relay asked for older pages first (below); that is the only
  case that shows loading.
- **Provisional until current (desktop):** a decision on rows that are not a
  current window is provisional. A current window is one fetched during
  this visit from the relay, or one fetched earlier whose newest row is at
  least as new as the channel's last message; it counts once its newest row
  has reached the rows drawn. It also stays provisional until the relay's
  read markers have been taken in (or their fetch gave up): a marker taken
  from the device before then is taken again as the later of the two, and
  the rule runs again. When it is shown the same rule runs again; if it
  gives another row and the reader has not moved, the open moves there
  once, silently; if the new row is only in older pages, those are sought
  first (as on a first open) and the open moves when found. The reader has
  moved after a wheel or touch scroll, a scroll key or a pointer press
  outside a text field (scrollbar drag, jump buttons, autoscroll), find in
  page, or a scroll of the message list more than 0.75 s after the open
  (earlier scrolls are the list placing itself). This resets on each new
  room or thread. If the reader moved, it stays. Threads follow the same
  rule against the relay's replies. Cases:
  `test-fixtures/message-store-paint-first.json` (`anchor`, `reanchor`).
- **Threads:** the same order: a linked reply, the first unread reply (near
  the top), where the reader left the thread, the newest reply. The reading
  position is kept per thread under `<channel id>/<root id>`. Desktop pages
  back for a reply in an older page (the same limits as a room) before the
  replies are drawn.
- **Where positions are kept:** per server and account. Desktop keys them by
  the relay's `https://host[:port]` origin (the store files' server identity)
  and account; a record from before this moves once to the first server the
  account reads on.
- **Forgetting a position:** a visit that opens at another row (a first
  unread, a link) or jumps to the newest forgets the remembered one, as does
  one whose row is no longer within reach (paged back to the limit, or gone).
  A visit that only ran out of time (a slow relay, no read state in time)
  keeps it for the next visit; settling at the newest during that visit does
  not forget it until the reader has scrolled away once.
- **The device's read markers decide (desktop):** first unread and the
  remembered row use the read markers stored on the device, loaded as the
  app starts; the relay's markers are not waited for and merge in later. A
  room or thread opened before the device's markers have loaded waits for
  them up to 1 s (once per session), and otherwise opens at the newest
  without trusting a remembered row or (for a thread) its unread replies.
  With no read state at all (no account signed in) nothing waits. Opening
  does not mark the room read before that decision. A link does not wait.
- **Opening at a message does not wait for the newest page (mobile):** a
  room opened from a certain stored window fetches the two pages around the
  message as soon as the stored window is shown, without waiting for the
  relay's newest page. If that page lands after the window opened at the
  message, it is dropped; if the window could not open there, it replaces the
  stored rows as usual.
- **Seeking a far first unread or remembered row (desktop):** up to three
  older pages of the relay's largest window (200 rows), within 2 s; a failed
  page is asked again after 250 ms rather than counted. Not found in time:
  the newest message. The hidden list re-finds its row every frame, so rows
  loaded above it before it is shown do not move it.
- **Replacing the newest page keeps joined older rows:** when the relay's
  newest page arrives (after a store open, on subscribe, on reconnect), rows
  inside its span are its rows; those it no longer has were deleted. Older
  retained rows stay when the two join: some retained row is not older than
  the new page's end cursor (relay order: `created_at DESC, id ASC`). They
  are re-cut into one page that starts at that cursor and keeps the old
  tail's end cursor, so paging further back continues the chain; overlays
  (5, 7, 9005, 40003) targeting the kept rows, and deletions of those
  overlays, go with them. A new page with
  no more history proves every older row gone. Pages that do not join leave
  only the new page. Live thread summaries of kept rows stay; for roots the
  new page delivers, both keep a live one pushed after the page's summary
  (`created_at` of its 39005; desktop judges a root without one by the page's
  bounds overlay, mobile keeps its live summary, and mobile also keeps those
  received while the page was being fetched); desktop drops the rest unless
  the root is a live overlay row. A mobile window opened at a
  message starts over at the newest page. Cases:
  `test-fixtures/channel-window-replace.json`.
- **Limit:** a kept older row keeps what it had: if it was deleted, edited,
  reacted to or replied to while the client was disconnected (desktop's
  reconnect repair stores such events but does not hand older ones to the
  room), that shows only once the room is fetched again from the newest page
  with no older rows kept, or the row is paged in again. Mobile's screen list
  also keeps rows its window drops (as before), so after a replace that does
  not join, desktop and mobile can show different older rows until reopened.
