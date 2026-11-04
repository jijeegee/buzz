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
| Community bundle | `desktop/src/shared/api/relayCommunityBundle.ts` (`CommunityLiveBundle`); features call `communityLive.subscribe` (`relayCommunityLive.ts`) | `mobile/lib/shared/relay/community_live_bundle.dart` (`CommunityLiveBundle`); `RelaySessionNotifier.subscribe`/`subscribeWithStatus` route matching filters to it, presence uses `communityPresence` |
| Community fixture tests | `relayCommunityBundle.test.mjs` | `community_live_bundle_test.dart` |
| Multi-filter live REQ | `desktop/src/shared/api/relayLiveFilters.ts` (`subscribeLiveFilters`) | `mobile/lib/shared/relay/relay_session_live_filters.dart` (`_subscribeFilters`) |
| Message store | `desktop/src-tauri/src/message_store/` (fed by `native_websocket.rs` and `relay.rs`) | `mobile/lib/shared/message_store/` (fed by `relay_session.dart`) |
| Message store fixture tests | `message_store/tests.rs` | `message_store_test.dart` |
| Forgetting a room | `message_store/purge.rs`; list from `get_channels` (`commands/channels/fetch.rs`: `member_ids`, `store_scope`), applied by `desktop/src/features/channels/lib/storeRoomPurge.ts` (from `refreshChannelsQuery`) | `mobile/lib/shared/message_store/purge.dart`; list trigger `_forgetStoredRoomsMissingFrom` (`channel_directory.dart`, from the membership fetch in `channels_provider.dart`) |
| Forgetting fixture tests | `purge_follows_shared_fixture` (`message_store/tests.rs`); triggers: `socket_notices_carry_the_relay_key`, `an_eose_capture_precedes_what_arrives_after_it`, `a_large_purge_runs_in_chunks`, `a_list_older_than_the_last_applied_is_refused`, `store_rooms_follow_membership_not_the_shown_list` (`fetch_tests.rs`), `storeRoomPurge.test.mjs` | `message_store_purge_test.dart`; triggers: `relay_session_purge_test.dart`, `channels_store_purge_test.dart` |
| Head spans | `message_store/head.rs` (writer: `writer.rs`; commands `message_store_head_sent`, `message_store_record_heads`); requests: `desktop/src/shared/api/relayStoreHeads.ts`, used by `channelWindow.ts` and `threadWindow.ts` | `mobile/lib/shared/message_store/head_coverage.dart`; requests: `queryRelayWithStoreHeads` (`relay_session_store.dart`, `store_heads.dart`), used by `store_head.dart`, `thread_replies_provider.dart`, `channel_preload.dart` |
| Head span fixture tests | `head_coverage_follows_shared_fixture` and the other tests in `message_store/head_tests.rs`; `relayStoreHeads.test.mjs` | `message_store_head_test.dart`; `store_heads_test.dart` |
| Store-first preload | `desktop/src/features/messages/channelPreload.ts` (`roomsNeedingRelay`; plan: `lib/storePreloadPlan.ts`; wait: `waitStoreLive` in `relayStoreCoverage.ts`; command `message_store_certain_rooms`); waves: `desktop/src/features/home/lib/inboxPreloadRun.ts`, run by `useInboxPreload.ts` | `ChannelPreloader._roomsNeedingRelay` (`mobile/lib/features/channels/channel_preload.dart`; plan: `store_preload_plan.dart`; wait: `waitStoreLiveRooms` in `relay_session_store.dart`); waves: `runInboxPreload` (`mobile/lib/features/activity/inbox_preload.dart`) |
| Store-first preload tests | `storePreloadPlan.test.mjs`, `channelPreload.test.mjs`, `inboxPreloadRun.test.mjs`, `relayStoreCoverage.test.mjs` | `store_preload_plan_test.dart`, `channel_preload_test.dart`, `inbox_preload_test.dart`, `coverage_test.dart` |
| Agent observer filter (rides the addressed account bundle) | `desktop/src/shared/api/observerLiveFilter.ts`, used by `observerRelay.ts` through `meLive.subscribe` | `observerLiveFilter` in `observer_subscription.dart`, routed to the bundle by `RelaySessionNotifier.subscribe` |

The account bundle's cases live in
[`test-fixtures/me-live-bundle.json`](../test-fixtures/me-live-bundle.json),
the community bundle's in
[`test-fixtures/community-live-bundle.json`](../test-fixtures/community-live-bundle.json),
the message store's in
[`test-fixtures/message-store.json`](../test-fixtures/message-store.json),
forgetting a room's in
[`test-fixtures/message-store-purge.json`](../test-fixtures/message-store-purge.json),
head spans' in
[`test-fixtures/message-store-head-coverage.json`](../test-fixtures/message-store-head-coverage.json),
the store-first preload plan's in
[`test-fixtures/message-store-preload-plan.json`](../test-fixtures/message-store-preload-plan.json),
the user stream's in
[`test-fixtures/user-stream.json`](../test-fixtures/user-stream.json).

## Live room bundle

A room is a channel, DM or group. A thread is part of its room: replies carry
the room's `h` tag, so the room's stream carries its threads, followed or not.

- **One stream for every room.** The rooms the account belongs to share one
  live REQ per 128 rooms (the relay's explicit-`#h` cap per REQ): ids sorted
  and deduplicated, then split in order. A room does not get a REQ of its own
  when it is opened. When the relay runs the account's
  [user stream](#user-stream) in `live` or `gated` mode, that one REQ
  carries the member rooms instead, and only rooms it lacks stay on chunks.
- **Filter.** Every channel event kind plus the thread-summary recount
  (39005) and room metadata (thread names, 40009), `limit: 0` and no
  `since`. The relay applies `since` to live matching, so a `since` would
  drop events stamped in the past within the relay's skew allowance. Huddle
  lifecycle (48100–48103) is a channel kind, so huddle views ride the bundle
  too (desktop `roomHuddleLifecycle.ts`: one listener and one history read
  per room for the indicator, every card and the roster). The desktop
  huddle presence runtime (who is in which huddle) listens on all rooms in
  the main window (`huddlePresenceSources.ts`); the huddle window carries no
  rooms and keeps its own `limit: 0` lifecycle REQ per 128 rooms.
- **Rooms outside the bundle.** A room opened before its chunk is live (cold
  start) or without membership (a browsed open channel) gets a one-room live
  REQ, closed once a live chunk covers it.
- **Quiet rooms.** Hidden DMs ride the same chunks without being listed
  (desktop `setQuietRooms`, mobile `setQuietRooms`): chunk ids are
  member ∪ quiet, so hiding or resurfacing a DM moves it between the two
  sets on the same chunk and sends no REQ. Desktop keeps a room that leaves
  both sets in its chunk for 1 s (refetch lag), except when another room
  joins in the same change or the chunk is not live yet. A chunk made only of
  stale hidden ids is closed by the relay and not reopened; nothing is lost
  (its rooms are gone).
- **Replacing a chunk.** When the room set changes, the new chunk is opened
  first and the old one is closed only after the new one is live. Overlapping
  deliveries are expected.
- **Waiting for coverage.** `whenRoomCovered(roomId)` (desktop
  `RoomLiveBundle`, mobile `RoomLiveRouter`) resolves once the room's live
  events arrive, so a history read started afterwards misses nothing. It
  resolves at the first of: a chunk (or the room's own REQ) carrying the room
  is live; the member list is known and nothing will carry the room
  (archived, the huddle window, not a member); 1.5 s. A quiet room (hidden
  DM) waits for its chunk like a member room. Before the member list is
  first known (cold start) only the bound ends it. Desktop
  `whenRoomsCovered(ids)` waits for several rooms with one timer and returns
  false when the bound ended it; mobile `whenRoomsCovered` waits for each.
  Features that read history after the wait read again once coverage
  completes when the bound ended it (huddle presence, remote start, the
  mobile visible-DM feed).

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
  rooms only, never thread summaries or room metadata. Each keeps its
  existing duplicate handling.
- **Quiet listeners** (hidden-DM resurface; desktop remote start in a hidden
  agent DM): every event of a quiet room once, never thread summaries or
  room metadata. Quiet rooms never reach the all-rooms consumers: no unread,
  badge, notification or chat row until the DM is resurfaced (then it is a
  member room again, on the same chunk). Desktop `listenQuietRooms`, mobile
  `listenQuiet`. The same holds for [push inbox](#push-inbox) events: a
  quiet room's pushed rows are stored but never folded into unread or a
  list.
- **Passive room listeners** (mobile `listenRoomEvents`: roster and bot-role
  refresh on a 40099 membership notice, at 0, 1 and 4 s because the relay
  rebuilds the 39002 list after the notice): a room's events from its chunk
  or its own REQ. They open nothing and do not make the room wanted.
- **Room metadata listeners** (thread names): every 40009 of every room the
  bundle carries, member or browsed, once. Nothing else gets them: not the
  timeline, not unread, not the chat list, not the store. Desktop
  `listenRoomMetadata`, mobile `listenMetadata`; shared cases in
  `test-fixtures/room-live-routing.json` (`delivery`).

## Open room REQ

Typing indicators (20002) and the goal tree (40110) of an open room ride one
live REQ per room, shared by every screen of that room (desktop
`relayRoomLive.ts`, mobile `room_live_request.dart`):

- Always the same two filters, `{kinds:[20002], #h:[room], limit:0}` and
  `{kinds:[40110], #h:[room], limit:0}` (`test-fixtures/room-live-request.json`),
  so a listener coming or going never re-sends the REQ. Every filter carries
  the room's `#h`, which keeps the REQ room-scoped on the relay.
- The first listener opens it; it closes one microtask after the last one
  leaves, so a remount in the same task keeps it. A terminal CLOSED tells
  every listener and forgets the room; a later listener opens a fresh one.
- Reconnect re-sends it unchanged (`limit: 0`); goal listeners then re-read
  the tree, since a missed revision is not replayed. Typing has nothing to
  lose.
- Mobile typing does not rebuild on reconnect. Mobile goal chips stay live
  through `goalTreeLiveProvider`, watched by the room and thread goal buttons.
- Not in it: thread names (room metadata on the bundle, so sidebar and inbox
  rows of unopened rooms stay live too).

## Thread names

- **No REQ per row.** Rows read names through one batcher per app (desktop
  `threadNameBatch.ts`, mobile `thread_name_batch.dart`): names asked for
  within 50 ms share reads of `{kinds:[40009], #h:[≤128 rooms], #e:[≤100
  heads], limit:200}`, sent once the rooms are covered (`whenRoomCovered`).
  Pages go back with an inclusive `until`, drop named heads from `#e`, and
  stop at a short page or after 10 pages (the rest keep what they hold).
- **Live.** Renames arrive as room metadata on the bundle. Desktop has one
  cache-wide listener (`threadNameLive.ts`) that updates rows the app holds;
  mobile thread-name notifiers listen to the router's metadata.
- **Newest wins, always.** A read's answer is merged with the cached name and
  any rename that arrived during the read (`selectThreadName`, mobile
  `latestThreadName`); a read never replaces a newer live name.
- **Reconnect.** Desktop re-reads the names on screen (active queries);
  mobile notifiers rebuild on `connected` and re-read through the batcher.

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

- **Usable at once.** The socket counts as connected (sends, new REQs, room
  and thread window requests) once it is authenticated and its live REQs are
  re-sent; the repair and catch-up of missed messages run behind them and
  hold nothing. They still run to the end under their own rules: no room is
  certain before its repair or catch-up has ended ([Coverage](#coverage),
  [Store-first preload](#store-first-preload)). Desktop's connect resolves
  once the REQs are back on the socket; mobile marks the session connected
  at AUTH OK and re-sends the REQs behind it. The first connect after launch
  has no live REQs to re-send or repair yet.
- **User stream first.** The [user stream](#user-stream) REQ is the first
  REQ after AUTH OK on every connection; the chunks are re-sent as usual
  right behind it, and in `live`/`gated` they shrink only after the stream
  EOSE. The stream is not one of the chunks below: it has its own repair
  (the stream EOSE catch-up, and desktop's listener repair from the
  stream's last-seen event time minus 1865 s) and is never replayed by the
  generic paths.
- Desktop re-sends each chunk unchanged and repairs the missed window over
  HTTP: one keyset walk per chunk covering all its rooms, from the chunk's
  last-seen event time (or its start) minus the 1865 s lookback. Every row
  goes to the device store, but rows older than the chunk's start minus 5 s
  are not handed to listeners: they were never live on it and must not show
  up as new notifications or activity.
- Mobile re-sends each chunk unchanged; the open room refetches its newest
  page and the open thread its replies, as before.
- **Quiet rooms after a drop.** Desktop: the chunk repair covers them, under
  the same chunk-start floor, and hands their rows to the quiet listeners.
  Mobile: the activity notifier records when a connection drops (not before
  the first connect). On the next connection it waits until every chunk
  carrying a quiet room is ready again (its EOSE after the re-send; at most
  1.5 s), then reads the hidden DMs' gap once over HTTP (`since` = drop time
  minus 1865 s, 128 rooms per filter) and routes it as quiet backfill. If
  the wait hit its bound it reads anyway and once more when ready (at most
  30 s later); the router's duplicate check keeps it to one resurface.
  Backfill rows older than the room's start on the bundle minus 5 s are
  dropped, so a DM hidden a moment ago is not reopened by an old message.
- **Live-only REQs go back unchanged.** Every REQ whose filter has
  `limit: 0` (room chunks, both account bundles, the community bundle with
  its current filter set, one-room REQs) is re-sent byte-identical, with no
  `since`. The
  relay returns no stored rows for `limit: 0` (it runs the history query as
  `LIMIT 0`, one empty statement per filter, as before), so a `since` could
  never recover anything; but the relay also applies `since` to live
  matching, so one event stamped ahead of real time (observer frames up to
  +300 s, stored kinds up to +900 s, presence and typing with no bound) used
  to drop every later live event on that REQ until the next reconnect.
  What a dropped connection missed comes from elsewhere: desktop room chunks
  from the HTTP repair above (from the last-seen time minus 1865 s, which
  absorbs a +900 s stamp), mobile room chunks from the open-room refetch and
  the store catch-up, the account bundles from their clock-based catch-up
  ([Account bundle](#account-bundle)). Desktop (`buildReconnectReplayFilter`
  in `relayReconnectReplay.ts`) and mobile (`_replayFilter` in
  `relay_session_replay.dart`, also used by CLOSED retries) share the rule.
- **Read state reads again on every reconnect.** Its listener is live-only,
  so nothing replays the marks made elsewhere while the socket was down:
  each real reconnect runs one read-state fetch (desktop through the relay
  client's reconnect hook, mobile from `reinitializeRemote`). Status steps
  that are not a new socket fetch nothing.
- Both apps also catch the device store up after every chunk EOSE (see
  [Coverage](#coverage)). Desktop skips this when its reconnect repair runs
  for the chunk on that connection; mobile has no repair, so it always runs.
  In `live`/`gated` the stream EOSE does the same for the rooms it carries
  ([Catch-up at the stream EOSE](#catch-up-at-the-stream-eose)).

## Account bundle

Live-only subscriptions about the account itself (settings documents,
read state, sidebar lanes, contact list, agent personas, teams and devices,
chat-list pings, mentions and notices, sign-in notices, agent work frames)
share two REQs instead of one per feature:

- **Authored:** `{kinds: authoredKinds, authors: [me], limit: 0}`.
- **Addressed:** `{kinds: addressedKinds, "#p": [me], limit: 0}`. It also
  carries agent work frames (24200, [Agent activity](#agent-activity)); the
  relay never stores them, so a catch-up returns none.

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
  feature was listening shows up as new. The 500 cap is per bundle, shared
  by every listener in that scope, so one busy feature can use it up for
  the others. When the catch-up may have missed something (it returned 500
  events, or its last attempt failed) the bundle calls `onGap` on every
  listener in the scope, once per catch-up and only for the current
  connection; features that hold history refetch it
  ([below](#history-features-listen-first-then-fetch-once)).
- **Closed by the relay.** When the relay ends a bundle's REQ for good, the
  bundle is dropped and the next listener opens a fresh one. Both apps tell
  current listeners (mobile `onClosed`, desktop `onRemoved`); the read state
  re-attaches with backoff (1 s doubling to 60 s), other features as before.
- **Lifetime.** A bundle opens with its first listener and closes once its
  last one leaves, after the current task, so a rebuild that subscribes again
  at once keeps it.

### History features listen first, then fetch once

A feature that needs past events rides the bundle with a live-only filter
and reads its history with a separate one-shot fetch. It attaches the
listener first and fetches once it is live, so nothing lands between the
two. Features that apply events one by one use `historyThenLive`
(`historyThenLive.ts` on desktop, `history_then_live.dart` on mobile):
an event seen live and fetched is applied once. Features backed by a query
(home feed, reminders, activity) treat a live event as a cue to refresh the
query. On `onGap` each fetches again; its own single-flight folds repeat
calls into one.

| Feature | App | Scope | Fetch |
|---|---|---|---|
| Home feed mentions and approvals | desktop | addressed | the feed query |
| Event reminders | desktop | addressed | the feed query |
| Membership notices | desktop | addressed | last 30 s, 50 |
| Activity (mentions, notices) | mobile | addressed | the activity query |
| Sidebar lanes (sections, mutes, stars, names, sort) | mobile | authored | the lane head |
| Own agent devices (30180/30181) | mobile | authored | last 200 |
| Read state | both | authored | last 7 days, 500 |

- **Read state takes the whole batch.** Its fetch goes to the batch merge
  (own-slot union, slot-squatter check), not through the per-event seen
  gate. A parse cache by event id (256) saves the second decrypt; every
  event still reaches the merge. A failed fetch retries with backoff (1 s
  doubling to 60 s) until one succeeds: with the listener live-only, the
  retry is the only cover. Desktop's fetch keeps its early interactive slot,
  so unread badges wait on one query, not on the bundle; the listener
  drains with the bundle, and once it is live a short fetch from the first
  fetch's start minus 5 s covers marks made in between.
- **Lanes keep their head.** Each mobile lane keeps a `limit: 1` head fetch
  next to its `limit: 0` listener; the head still runs when attaching the
  listener fails, and sections read their head again after the relay
  closes their listener.
- **Devices load once read.** Mobile marks its own devices loaded only after
  a fetch succeeds (it gates a publish); a failed fetch retries with
  backoff.

Kept out on purpose:

- **Community-wide streams** (user status, presence, catalogs, custom emoji)
  are not about the account: they ride the
  [community bundle](#community-bundle).
- **Room-scoped live REQs** (typing, goals, huddles, thread names, DM
  batches) are not about the account either.

## Community bundle

Every community-wide live stream rides one multi-filter REQ per app. The
rows ([`test-fixtures/community-live-bundle.json`](../test-fixtures/community-live-bundle.json)),
all `limit: 0`:

| Row | Filter | Desktop | Mobile |
|-----|--------|---------|--------|
| emoji | `{30030, #d:[kuzz:custom-emoji]}` | yes | no |
| status | `{30315, #d:[general]}` | yes | yes |
| catalogs | `{30175, 30178}` | yes | no |
| presence | `{20001, authors:[set]}`, left out while the set is empty | yes | yes |

- **Routing.** A listener filter rides a row when it is live only, its
  kinds are a subset of the row's and its `#d` is the row's. A filter with
  `authors` never rides (it would silently get only the tracked set), nor
  one with `since` or any other key; those get their own REQ. Each listener
  gets the events its own filter matches, once.
- **Presence author set.** Lowercased, sorted, grow-only for the session,
  capped at 1000 (the relay's per-list limit). Keys past the cap are
  refused with one warning and stay on the 60 s poll. Desktop adds the keys
  that active presence queries ask about; mobile adds the keys the presence
  cache tracks.
- **Updates.** New keys go out with `setFilters`: the first 250 ms after it
  is added (or 250 ms after the opening EOSE, if it came before), then at
  most one re-REQ per 30 s, always with the newest set. A person shown for
  the first time still gets their snapshot at once from the presence fetch;
  their live updates follow within 30 s.
- **One REQ in flight.** A same-subId REQ replaces the relay's filters, and
  two in flight can be applied out of order, so the next set goes out only
  after the previous REQ's EOSE or CLOSED. The opening REQ counts. While the
  socket is down or a CLOSED retry is waiting, the update rides the replay
  or the retry instead.
- **No second "ready".** The EOSE that answers a `setFilters` REQ is not
  passed on: listeners hear "ready" on open, after a reconnect and after a
  reopen only. Mobile presence drops its snapshot on every "ready", so a
  ready per update would blank it.
- **Closed by the relay.** A terminal CLOSED reopens the bundle with
  backoff (1 s doubling to 60 s); listeners stay and hear "retrying", then
  "ready". It never calls a listener's `onClosed`.
- **Reset.** Session teardown (desktop: the connection going idle on
  sign-out or community switch; mobile: session dispose) clears the author
  set and calls each listener's `onReset`; presence adds its keys again.
- **Lifetime.** Opens 250 ms after its first listener and closes one task
  after its last.

## Agent activity

Work frames from the account's agents (24200, `#p: [me]`) have no REQ of
their own: they ride the addressed [account bundle](#account-bundle).

- **Filter.** `{kinds: [24200], "#p": [me], limit: 0}`
  ([`test-fixtures/agent-observer.json`](../test-fixtures/agent-observer.json)),
  a listener on the addressed REQ. Only the observer's listener takes 24200
  frames; the other listeners check the kind first.
- **No history.** The relay never stores frames (it fans them out live
  within ±300 s of its clock), so there is nothing to fetch: not at start,
  not on reconnect, not in the bundle's catch-up. Frames sent while the
  listener was not attached are gone; desktop's local archive keeps what it
  saw on its own socket.
- **Duplicates.** Frames are deduplicated by sequence number and timestamp,
  so a frame the archive already holds is harmless.
- **Agent online status** comes with everyone else's presence on the
  [community bundle](#community-bundle)'s presence row; it needs no REQ of
  its own.
- **Start.** The listener for the account's agents attaches at launch only
  after the channel list and the first room window have loaded
  ([Launch order](#launch-order)), so it never opens the addressed REQ
  ahead of the first screen. Opening an agent's session or sending it a
  control attaches it at once. When the addressed REQ is already live,
  attaching sends nothing.
- **Closed by the relay.** A terminal CLOSED on the addressed REQ also stops
  the observer, as it stopped its own REQ before: desktop re-attaches after
  a community reset, mobile on its next rebuild while connected.

## Launch order

An account with data on the device draws from that data first; the server's
answers follow in the background, in this order, and never block the screen.

1. **Draw.** The active account is drawn from device storage: channel list
   snapshot, stored rooms ([Painting from the store](#painting-from-the-store)),
   and unread badges from the device's read markers (the relay's markers
   merge in later: the read state listens on the authored bundle, then
   fetches once; markers only advance). No loading text while stored data
   exists; the splash only fades (no minimum time).
   Desktop also draws the Chats rows and other people's names and pictures
   from their [launch snapshots](#launch-snapshots), read when the
   community's query cache is made.
   Mobile draws the channel list, other people's profiles and the
   account's own profile from its [launch snapshots](#launch-snapshots)
   before the token and the socket; the channel list's server requests
   after the memberships run in parallel. On iPhone it then takes the
   [push inbox](#push-inbox) into the store and folds it into the drawn
   lists, also without waiting for the socket.
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
6. **Agent indicator listener** ([Agent activity](#agent-activity)) attaches
   after the channel list and the first room window (with no room open, 2 s
   after the channel list; at most 10 s after launch). There is no replay:
   frames are never stored. Mobile attaches it once the channel list has
   resolved, and publishes live frames as one update per 16 ms.
7. **Background preload wave** and cleanup, when the user is idle. Mobile
   queues forgetting left rooms ([Forgetting a room](#forgetting-a-room))
   until after the first frame and the first window of the room on screen
   (at most 5 s), then runs each list as its own task between frames.

Mobile reads the heads of the account's own 30078 lanes (mutes, stars,
theme, ...) together when they start within 50 ms of each other: one
one-shot `{30078, authors:[me], #d:[lanes], limit:N}`, and each lane gets
its newest event (`lane_head_batch.dart`).

## Live REQ budget

The open live REQs per checkpoint are pinned in
`test-fixtures/live-req-budget.json` (desktop: Playwright
`tests/e2e/live-req-budget.spec.ts`; mobile:
`mobile/test/live_req_budget_test.dart`). A change in the count or in a
signature fails the test; update with `KUZZ_UPDATE_REQ_BUDGET=1` and say why
in the commit.

| Checkpoint (fixture key) | Desktop | Mobile |
|---|---|---|
| Idle (`idle`) | 5 (a landing room is always open) | 4 |
| Agents view, no room (`agentsView`) | 4 | — |
| Room open (`roomA`/`roomB` · `roomOpen`) | 5 | 5 |
| Thread open (`threadOpen`) | 5 | 5 |
| DM open (`dmOpen`) | 5 | 6 (DM identity) |
| Room with 10 / 50 thread rows (`rows10`, `rows50`) | 5 | 5 |
| Idle with an agent DM (`idleWithAgent`) | 5 | — |
| Idle as the relay's own key (`idleRelaySelf`) | 5 | — |
| Idle with a hidden DM (`idleHiddenDm`) | 5 | 4 |
| Idle, user stream in `shadow` (`idleStreamShadow`) | 6 (idle + the stream) | 5 (idle + the stream) |
| Idle, user stream in `live` (`idleStreamLive`) | 5 (idle − the member chunk + the stream) | 4 (the stream replaces the room chunk) |
| Activity inbox (`activityInbox`) | — | 4 |

The REQs behind them: the community bundle, the room bundle (one per 128
rooms), the account bundles (authored and addressed), and per open room its
typing + goal-tree REQ. The pinned counts are for a relay without the
[user stream](#user-stream) (the e2e mock). With it, `shadow` adds one REQ
(the stream) next to the chunks (`idleStreamShadow`), and `live`/`gated`
replace the member chunks with it (`idleStreamLive`).

Rules:
- No REQ per row (thread rows, inbox rows, search hits): rows ride a bundle
  and read history in batches.
- Fixed filters: a REQ's filter does not change with what is on screen.
- `limit: 0` REQs are re-sent byte-identical on reconnect
  ([Reconnect](#reconnect)).
- Each feature names its recovery (repair, catch-up, refetch or one-shot
  read) where it is described above.

Allowed exceptions:
- a one-room REQ for a non-member room, or a room not yet covered;
- mobile DM identity (`{0, 10100}` by participants, `limit: 0`; filters with
  `authors` cannot ride the room bundle);
- admin join alerts (2 REQs, admins only);
- the mobile huddle sheet's room REQ and in-call reactions (desktop
  `HuddleBar` in-call reactions too);
- the huddle window's socket (its own lifecycle REQ, see
  [Live room bundle](#live-room-bundle)).

Budget probes: putting an old per-feature REQ back fails its checkpoint
(hidden-DM batch, remote-start REQ, huddle lifecycle REQ, visible-DM batch,
per-room 39002).

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
  socket authenticates or the query is sent. Mobile also takes the
  [push inbox](#push-inbox): events the iPhone notification extension
  fetched, under the scope current when the app drains it.
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
  carries every channel event kind (a room-bundle chunk or a room's own REQ),
  and the [user stream](#user-stream) in `live`/`gated` for the rooms it
  carries: its snapshot rooms from its EOSE, and a room joined by 44100 from
  that notice's receive time (`[T, T]`). A `shadow` stream records nothing.
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
  The stream's catch-up walks its rooms in batches of 128 sorted by resume
  point, may skip rooms in `gated`, and writes the `room_seq` watermark
  under rule W ([User stream](#watermark-room_seq)).
- **Order and loss:** spans go through the same write queue as events, so a
  span is recorded only after the events received before it are written. A
  dropped event or failed write clears every span of the file, and any later
  span starting at or before the time of the loss is not recorded. Each path
  that clears spans also clears `room_seq` in the same transaction.
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
the store (its walk to the reply pages the relay anyway); mobile lands on a
linked reply among the replies it holds first, and otherwise asks a separate
relay window (`openAt`). Later loads and older pages always ask
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

### Opening a thread from its root id

A thread row in Chats opens the thread page with only the root id (plus the
row's thread name, root preview and archived flag). The page draws at once,
also offline; nothing on it waits for the root, the channel timeline or the
relay. Cases: `test-fixtures/thread-open-root.json`.

- **Root, in order:** passed in, the room's loaded rows (timeline or a
  deep-link copy), the store by id (pushed rows included; a removal (9005) or
  the author's deletion (5) in the store marks it deleted), the relay. An
  author's deletion counts only when signed by the root's author, so it needs
  the root in the same answer or already known; an answer holding only such
  a deletion is kept and judged again once the author is known. The
  relay is asked over `/query` with two filters in one request: the root by
  id, and a deletion tagging it (`#h` room, `#e` root). A relay answer
  replaces a stored copy. The root fills only the top bubble, agent pins and
  the thread menu; until it is found a thin row shows the thread name there.
  That row replaces the loading text whenever replies are drawn; loading
  shows only once the store has answered with no replies.
- **Asked again:** a failed, timed-out or empty answer is unresolved. A
  failed request while connected is retried after 2, 5 and 15 s; after that,
  and for an empty answer, the relay is asked again each time the session
  reconnects until it answers with the root or a deletion.
- **Room history in the background:** the room's message provider (below)
  starts its top-level history load as on any room open; nothing on the
  thread page waits for it.
- **Deleted only on proof:** the root counts as deleted only on an explicit
  deletion (stored, live, or in the relay's answer). Then the deleted row
  replaces the root and the replies stay. A failure or an empty answer never
  means deleted, and a thread never falls back to the channel timeline.
- **Replies:** the stored newest replies draw at once
  ([Opening a thread from the store](#opening-a-thread-from-the-store)),
  pushed rows included until the relay's copy replaces them. The room's
  message provider is created before the thread query, so own replies still
  awaiting relay proof settle (that query takes the complete scan).
- **Composer:** replies are tagged with the root id only, so a found root is
  not needed to send. A room built from the Chats row (channel list not yet
  loaded) stands in for display only: the composer shows a loading line until
  the listed room or its details arrive ([Launch snapshots](#launch-snapshots)).
- **Other entry points** (notifications, Inbox, goals, search, drafts) open
  the same thread page directly (mobile `threadOrRoomPage`). Forum rooms keep
  the channel route.
- **Linked reply:** a link names the reply's parent, which is not always the
  root, while replies are read by their root. The linked reply (the room's
  rows, the store, the relay) gives the root. A nested parent stays the head
  and the replies read from the root. A parent the reader cannot get moves
  the page to the root in place, on the same route. A reply not found at
  first is asked for again after a push inbox drain and on reconnect, and the
  list opens at it when it arrives.
- **Room read:** opening a thread this way does not mark its room read, as
  with a Chats thread row.
- **History REQs wait for the socket:** a WebSocket history REQ made while
  the socket is down or before AUTH is held and sent on connect (its timeout
  still runs from when it was asked), not dropped.
- **Desktop:** the channel screen's thread panel opens on the root id alone
  (`useThreadRootOpen.ts`, `useThreadRoot.ts`, `threadRootResolve.ts`): a thin
  top row shows the thread name (its stored name, else the Chats row's title)
  until the root is found, and the replies draw at once. A root among the
  loaded room events is used as is, with no store or relay ask (a live
  deletion reaches it through the room's subscription). Otherwise the
  remembered route copy (a Chats row hands over its root; same room only)
  seeds it, and the store by id (`message_store_thread_root`) and the relay
  (`get_thread_root`) are asked together. The relay request asks both filters with
  `"consistency":"strong"` so a just-written root or deletion is not missed
  (mobile could adopt this). The panel never closes for a missing root; only
  forum rooms and huddle transcripts keep the old close. Links that carry
  `threadRootId` (notifications, search, quotes, goals) open the thread on the
  root at once and expand the branch to the reply once it loads.
  A reply also sent to the group opens the same way, cached or not; the main
  timeline scrolls to it too. A root from
  before the reader joined, which the relay does not return, stays a
  placeholder row.

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
after each batch of changes) are drawn at launch and each is checked again once
([Rechecking stored profiles](#rechecking-stored-profiles)). The account's own profile
(`self_profile`) draws before its fetch. Chats rows, search results,
the thread's group button, notifications and deep links open a room the
list lacks from its Chats row (or its id) at once; a deep link to a room no
row names waits for the relay's list, and a room missing from it keeps the
not-found handling. Mobile: `snapshot.dart`, `channel_list_snapshot.dart`,
`profile_snapshot.dart`, `chat_row_channel.dart`.

### Rechecking stored profiles

A stored profile of another person is asked for again only as "newer than
what I hold", so an unchanged one costs no download. Desktop and mobile
behave the same.

- **Held with it:** the kind:0's `created_at` and event id, and
  `verifiedAt`: this device's clock when a relay read covering that person
  was sent and succeeded. An entry without all three (saved by an earlier
  build) is unknown and read in full once.
- **Request:** unknown people share one full filter. Known people are sorted
  by `verifiedAt` and cut at the largest gaps into groups, at most 10
  filters in all; each group asks `since = max(0, oldest verifiedAt − 86400)`.
  The day of margin covers clock skew between devices. Several filters go
  in one `/query` request.
- **Answer:** an event replaces the held profile only if the relay would
  keep it over the held one (newer `created_at`, or the same second and a
  lower id). Profiles edited within the margin come back and are dropped.
  A known person the answer leaves out is unchanged: kept, its
  `verifiedAt` moved to the request's time, never treated as missing. Only
  an unknown person left out is missing. A failed read stamps nothing.
- **Desktop:** the stored owner (`ownerPubkey`, `isAgent`) is reused only
  when it was taken from the same kind:0 (`ownerCreatedAt == createdAt`);
  otherwise that person is read in full.
- **Mobile:** the signed kind:0 (up to 16 KB) is stored too, so an
  unchanged profile is re-exported to the iOS push cache as before.
- **Limit:** a kind:0 removed on the relay is not noticed; devices that hold
  it keep showing it.

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
  No clock is compared, with one exception: mobile stamps each forgotten
  room's `room_forgot.at` with this device's clock to fence
  [pushed rows](#push-inbox), which the notification extension timed with
  the same device's clock.
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

### Push inbox

Stage B of push content on the device. The notification extension (NSE)
on iPhone, or the background message handler on Android
(`push_fcm_background.dart`), saves the verified events it fetched for a
push; the app takes them into this store and its lists without waiting for
the socket.
Cases: `test-fixtures/push-inbox.json` (Swift and Dart tests read it).

- **Token:** each push-enabled, signed-in community has a random inbox
  token (32 lowercase hex), sent to the NSE in its community snapshot. A
  new one is made when a signed-out community is resumed and when its
  account changes; sign-out drops it.
- **Files:** `<App Group>/push-inbox/<token>/<unixms>-<uuid>.json`,
  `{v: 1, relayUrl, account, fetchedAt (unix seconds), events}`, written
  to a `.tmp` file and renamed. The folder is excluded from backup and
  readable after the first unlock. Per token the NSE keeps at most 7 days,
  500 events and 2 MB, pruning the oldest files first.
- **NSE fetch:** for each community with a token, one bounded `/query`
  with a filter per push policy: `since` = max(cursor − 1865 s,
  now − 72 h), `limit` 50, 1 MB of response for the whole run, cancelled
  10 s into it. It keeps the events the policies match, plus the refreshed
  kind:0 of the notified sender. If that fetch is cancelled or too large,
  or finds nothing, today's query (no `since`, `limit` 10) runs for the
  notification, by 17 s. The file is written before the notification is
  shown, only for a token still in the snapshot, and never while the
  device is age-restricted. The notification text never depends on the
  inbox.
- **Cursor:** `<token>/cursor`, unix seconds. It moves to the newest
  returned row after a bounded fetch, even a truncated one (pushed rows
  claim no completeness; catch-up fills gaps), and not after a fallback
  or an empty answer. When the app goes to the background while connected,
  it moves the active community's cursor to now.
- **Reading:** the app reads the active community's folder off the UI
  isolate, oldest file first, ignoring `.tmp` files and the cursor. A file
  of a newer `v` stays for a newer app. A file for another server (origins
  compared after [normalizing](#message-store); a path or user info is
  refused), for another account, malformed or over 2 MB is deleted unread.
  Events need a 64-hex id and pubkey, a 128-hex sig, integer fields,
  string tags and a push kind (9, 40002, 45001, 45003) or kind 0. The NSE
  verified the signatures; the app relies on that and does not check them
  again.
- **Drain:** on launch, on a community or account change and on resume,
  one at a time, never while age-restricted. Each file is submitted with
  its own `fetchedAt`. A file is deleted only when the store reports its
  write committed; a full or cleared queue, a failed write or an ended
  lease reports it dropped and the file stays for the next drain (files
  the NSE writes during a drain are not touched). Every drain first deletes
  token folders no community holds. Sign-out syncs the snapshot without
  the token, then deletes the folder; removing a community deletes it
  beside the store file.
- **Pushed rows:** a room row is written only for a room of the last
  membership list (none before the first list), not marked left, and not
  forgotten since the fetch (`room_forgot.at` < `fetchedAt` − 5 s; a list
  drop, a rejoin by list or notice and `since_join` all stamp it). Rows
  without a room are dropped. Pushed rows set no coverage and no head
  mark, and do not count as written for a list fetched meanwhile (it can
  still drop their room). A full queue drops them without marking the file
  lossy.
- **Lists, and thread paints on mobile:** a certain read (room or thread)
  leaves pushed rows out (`pushed_rows` marks them; the relay copy or a
  purge clears the mark). Store paints draw them until the relay's answer
  replaces them, and a first open always asks the relay. A room paint puts
  them in its window; no span vouches for them, so the paint's
  `complete_from` is none past the spans, and its summaries count the
  pushed replies (9, 40002, not deleted) newer than a root's
  `last_reply_at` (direct replies in `reply_count`, all in
  `descendant_count`; a root without a summary gets one). That count is
  paint only: the `summaries` table, the saved head cache and coverage
  stay the relay's. A thread paint draws pushed replies the same way.
  After a drain that wrote at least one file, an open or opening room
  that someone watches and whose relay window has not landed, and a
  thread's saved replies (the inbox preload's plus the stored replies
  newer than its newest; older stored ones may be deleted on the relay),
  read the store again; the thread's first-load flag stays off for that
  re-read. A store read that answers after a later one drew is ignored,
  and the cached head never draws over a store paint.
- **Folds:** only the events the store accepted, and only for rooms the
  channel list shows as listed (member, not archived, not a hidden DM; the
  list already leaves hidden DMs out), taken once the saved channel list
  read at launch has settled. The channel list moves those rooms'
  `lastMessageAt` at once and records unread once read state is loaded,
  by the catch-up's rule. Chats keeps them in memory for the lease (at most
  500, numbered by drain) and folds them, as replays through the live path
  (blocked authors, deletions; a thread marker only asks for the page),
  into the rows shown, saved or live, and into the saved rows it draws at
  a later start, before the socket connects. A replay counts an event a
  row already has once, and counts nothing for a row this device has read
  past it. A first page drops the kept events drained before it was asked
  for (the page has them) and folds the later ones. Pushes never reload
  Chats, and nothing pushed is written to prefs. The Inbox folds them, as
  replays, into rows already loaded (a first page in flight gets them once
  it lands); it adds no row and skips blocked authors. A fold whose lease
  is no longer current changes nothing. Kind:0 profiles are cached for the
  active community, newer wins.
- **Build switch:** `--dart-define=KUZZ_PUSH_INBOX=off` stops tokens,
  reading and draining. It is set at build time, so rolling back means a
  new build.

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
- **Pushed rows after a deletion (N5):** a row deleted on the relay after
  the NSE fetched it shows in the lists until catch-up replaces them or the
  relay keeps a deletion record. Rooms not in the kept list get no pushed
  rows.
- **Pushed rows in a room:** a room draws them only once the relay's copy
  arrives (there is no gap marker to draw them above the covered head). A
  mobile thread draws them in its store paint until the relay's page
  replaces them ([Opening a thread from its root id](#opening-a-thread-from-its-root-id)).
- **Push inbox trust (N7):** the app checks a pushed event's shape, not its
  signature: it relies on the NSE's verification and on the App Group's
  file protection.
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

## User stream

One live REQ for all of the account's rooms, with the room set kept by the
relay: no 128-room chunks, no re-chunking on join, no client re-subscribe for
a room joined offline. The relay decides per account, on every connection,
how much the clients rely on it. Cases:
[`test-fixtures/user-stream.json`](../test-fixtures/user-stream.json); each
rule below names its section there.

- **Build switch:** `VITE_KUZZ_USER_STREAM=off|follow` (desktop) or
  `--dart-define=KUZZ_USER_STREAM=off|follow` (mobile). `follow` (default)
  sends the stream REQ and obeys the relay's mode; `off` never sends it.
- **REQ:** `["REQ", <sub>, {"#h": ["$joined"], "kinds": <frozen kinds>,
  "limit": 0}]` (`request`), sent **first after AUTH OK** on every
  connection, as its own subscription type. It is never in the generic
  live-subscription map: no generic path (reconnect replay, coverage,
  access revocations, the CLOSED retry) ever sees `$joined` as a room id.
- **Frozen v1 kinds** (`frozenKinds`): 5, 7, 9, 9005, 39005, 40001, 40002,
  40003, 40008, 40009, 40099, 45001, 45003, 48100–48103, the same set as
  `ROOM_LIVE_KINDS`. The relay accepts exactly this set; any change bumps `v`.
- **Not stored, not routed:** kind 24220 (the snapshot) is never stored.

### Modes

Part 1 of the snapshot names the mode for this connection (`modes`). It is
never cached across connections.

| Mode | Chunks | Stream frames | Store catch-up | `room_seq` |
|---|---|---|---|---|
| `shadow` | as without the stream | not stored, not routed; compared with the chunks | at chunk EOSE, as today | written under rule W |
| `live` | member rooms leave the chunks after the stream EOSE; extras stay | stored and routed like a chunk's | at the stream EOSE, every covered snapshot room | written under rule W |
| `gated` | as `live` | as `live` | as `live`, minus rooms whose stored seq equals the snapshot's, plus the skip audit | written under rule W |

No stream (the build switch, the overflow backoff below, or any CLOSED) means
chunks exactly as without it, for that connection. In `live`/`gated` the
stream counts as a live REQ carrying every room it carries
([Coverage](#coverage)); in `shadow` it records no coverage.

### Snapshot

The relay answers the REQ with every snapshot part, then EOSE, and only
then live frames (it queues frames that arrive meanwhile). Each part is a
relay-signed kind 24220 event addressed to the account (`p`), on the stream
sub.

- **Part 1:** content `{"v":1, "op":"snapshot", "part":1, "parts":n,
  "mode":"shadow|live|gated", "epoch":"…", "kinds":[…], "rooms":{"<room>":
  <seq>, …}}`, at most 500 rooms per part. `seq` is the room's accepted
  sequence: an integer from 0 to 2^53 − 1. `epoch` is a non-empty string of
  at most 64 characters.
- **Later parts** need only `rooms`. A later part that carries `part` must
  carry its arrival position (2, 3, …); `v`, `op` and other fields there are
  ignored.
- **Refused (`part1`):** content that is not JSON; `v` ≠ 1; `op` ≠
  `snapshot`; part 1 not numbered 1; `parts` < 1; an unknown `mode`; an empty
  or missing `epoch`; `kinds` ≠ the frozen kinds (compared as sets); a seq
  that is not a non-negative integer; a room in two parts; more parts than
  announced, or a part out of order; fewer parts than announced at EOSE. A
  refused snapshot CLOSEs the stream: chunks for this connection. The store
  layer runs the same checks ([Store skip](#store-skip)).
- **Integer fields:** every integer field (`v`, `part`, `parts`, `kinds`,
  room seq) must be written as plain digits (no sign, fraction or
  exponent), as the store's serde check (`as_u64`) reads it: `1.0`, `1e0`
  and `-0` are refused alike (serde reads `-0` as a float). This holds for
  a later part's `part` and room seqs too. The fixture lists the cases.
  Both apps check the literal number tokens of the content text
  (`parsePlainIntegers`) and read any other token as `null`. Desktop: in
  `userStreamRules.ts`. Mobile: in `user_stream.dart`, because `jsonDecode`
  reads `-0` as the int 0.
- Any other kind arriving before part 1 is ignored (not stored, routed or
  compared). 24220 events after EOSE are ignored.
- **Epoch (`epoch`):** the file keeps the last epoch in `sync_state`. An
  accepted part 1 whose epoch differs (or the first one) clears every
  `room_seq` row and stores the new epoch, in one writer transaction queued
  before any rule-W write of that connection. Every rule-W write carries the
  epoch its catch-up started under, and the writer refuses it when the
  stored epoch differs at apply time, so a walk still running from before
  the change writes nothing ([rule W](#watermark-room_seq)). A refused part
  1 changes nothing. The relay bumps the epoch on a DB restore and when an
  account leaves `gated`.

### Store skip

The store layer decides for itself whether a stream frame is stored
(`storeSkip`), so the decision is made before any feature sees it.

- Desktop `SocketTap` learns the stream sub id from the outgoing REQ (its
  filter carries `$joined`) and sets that sub's verdict to `pending`. Mobile
  keeps the same state in `user_stream.dart`, checked in `_handleEvent`
  before `submit`.
- The verdict stays `pending` until the stream EOSE has validated the whole
  snapshot: the store layer runs the `part1` checks itself, including the
  part count at EOSE. Then it becomes `keep` (`live`, `gated`) or `skip`
  (`shadow`, or any refusal).
- **While `pending`, frames on the stream sub are dropped:** not stored, not
  held, not routed. The relay sends no live frame before EOSE (it queues
  them and flushes them after EOSE, when the verdict is already `keep`), so
  a frame seen while pending breaks that rule. Its event comes on the chunks
  anyway (they stay until the stream EOSE), and rule W's walk covers what
  was committed before EOSE. Kind 24220 is never stored in any state.
- A client CLOSE or a relay CLOSED for that id sets `skip` until the socket
  ends, so frames still queued behind a CLOSED are never stored. A new REQ
  on the same id overrides that and resets the verdict to `pending`; only
  the `auth-required` retry reuses the id. A new socket starts with no
  entries.

### Relay answers

`closed` and `closedReasons`. `auth-required:` and `invalid: user-stream`
match as prefixes, the rest exactly.

| Reason | Client |
|---|---|
| (snapshot parts, then EOSE) | mode from part 1 |
| `unsupported: user-stream off` | chunks for this connection |
| `unsupported: user-stream not enabled` | chunks for this connection |
| `invalid: user-stream <detail>` | chunks for this connection, and a log line (a client bug) |
| `unsupported: user-stream too many rooms` | chunks for this connection |
| `restricted: not a channel member` (old relay) | chunks for this connection |
| `auth-required: …` | before the stream EOSE (or in `shadow`): send the REQ again after the next AUTH OK, once per connection; a second one → chunks. After the EOSE in `live`/`gated`: the [mid-connection fallback](#mid-connection-fallback) at once, no retry (the relay already dropped frames; a retry would leave that gap unrepaired) |
| `error: user-stream overflow` | `shadow`: stream off, chunks untouched (`closed{shadow_full}`). `live`/`gated`: wait for the socket to close (`closed{full}`, counts toward the backoff) |
| `error: user-stream lagged` | any mode: wait for the socket to close; the reconnect first waits a random 0–10 s |
| `error: user-stream closed` | as any CLOSED for the mode (counts toward the backoff after the EOSE in `live`/`gated`) |
| anything else | as any CLOSED for the mode (counts as `closed` does) |

"As any CLOSED for the mode": before the stream EOSE, or in `shadow`,
chunks for this connection (they were never closed); after the EOSE in
`live`/`gated`, the [mid-connection fallback](#mid-connection-fallback), at
once. The stream is not requested again on that connection.

"Wait for the socket to close": the relay cancels the connection right
after these CLOSEDs, so the client starts no fallback, chunk or walk on the
dying socket. If the socket is still open 2 s later the client closes it
itself. Then it reconnects normally (with the lagged jitter when the reason
was `lagged`), and the next connection takes the stream or chunks by the
usual rules. This keeps a relay-wide lag from turning into one synchronized
catch-up burst. From the CLOSED on, carried rooms no longer count as
live or settled for coverage or store opens (the relay dropped frames), but
no chunk opens for them on the dying socket. Every
connect and every socket end, including a backed-off connect, puts the
carried rooms back on chunks: a connection with no stream for any reason
(CLOSED, refused, the build switch, the backoff) starts with nothing
carried, so every member room is on chunks.

- **Overflow backoff (`backoff`):** two counting closes within 10 min (per
  account and server) stop the stream REQ for 1 h from the second close.
  Counting closes are `full` and, after the stream EOSE in `live`/`gated`,
  `closed` and unknown reasons, so a fail-closed that fires on every
  connection cannot loop reconnect and catch-up. `lagged` (relay-wide,
  jittered) and `shadow_full` never count. A network change or a resume
  after at least 1 h in the background ends it early and forgets earlier
  closes. A network change is a change in the set of network types the OS
  reports: the first report after the app starts and a repeat of the same
  set (Android capability updates) are not changes. The backoff is kept in
  `sync_state`, so an app restart does not end it; the `auth-required` retry
  count is per connection.
- **`error: busy` on a history REQ (`busyRetry`).** The relay refuses a
  history frame that would fill a live/gated stream connection's send
  buffer past 80% and ends that REQ with `CLOSED "error: busy"`. History
  query paths on both apps retry it instead of failing: 1 s doubling to 30 s
  (`busyRetryDelaysSecs`), each retry on a fresh sub id so no late frame of
  the refused REQ is taken for the retry's, until the caller's own timeout
  or the socket ends. Desktop: `handleRelayClosed` in
  `relayClosedRecovery.ts` for one-shot subscriptions, and
  `readOnlyRelayClient.ts`. Mobile: the history branch of `_handleClosed`
  (`relay_session.dart`, logic in `relay_session_busy_retry.dart`). On the
  stream sub itself `error: busy` is just an unknown reason.

### Handoff

`handoff`, `handoffDedupe`.

- **At connect:** chunks open at once, as without the stream, so an account
  without it waits no extra round trip. The stream REQ goes first after AUTH
  OK. Mobile reads the stream state when the socket starts connecting; on
  the first connect of an account, if that read is still running at AUTH
  OK, the chunk replay waits for the stream REQ at most 300 ms. With the
  stream switched off in the build nothing waits, nor when the relay
  refused the stream (a CLOSED like `unsupported: user-stream off`, or
  another refusal no retry changes) on the last connection of that account
  and relay; mobile keeps that result in its preferences, so it is known
  before the store opens. An accepted EOSE, or an answer that is neither,
  brings the wait back. Desktop: on the first
  connect of an account or community the stored backoff and counters are
  read first (off the main thread); the chunks do not wait for that read,
  and the REQ follows when it lands (when that connect is still current).
- **`live`/`gated`:** after the stream EOSE (never at part 1) the chunks are
  replaced ([replacing a chunk](#live-room-bundle)) by chunks of the rooms
  the stream does not take (member ∪ quiet − handed); usually none remain.
  Deliveries in the overlap are routed once by id. Per-room listener starts
  carry over from the chunk that carried the room.
- **Rooms without a usable resume point:** a snapshot room has a usable
  resume point when it was covered before this connection
  ([connectionSpans](#catch-up-at-the-stream-eose)), is not stale, and was
  read from a store that vouches for what it holds (nothing queued
  unwritten, no write loss pending). The stream catch-up and listener
  repair cannot reach a room without one (uncovered, stale more than 7
  days, or after a write loss), yet such a room must still get its offline
  gap delivered to listeners on this connection. The stream still carries
  and stores its frames (routed once by id).
  - Desktop: such rooms are not handed to the stream (`handed`) for this
    connection: they keep their chunks, so the existing chunk catch-up and
    chunk reconnect repair still reach them, and the next connection (which
    then has their chunk spans) hands them over; they get no stream span.
    Desktop keeps every chunk carrying a room the stream does not repair
    (such a room, or one outside the snapshot) as it is, not
    re-partitioned, so its repair in flight is not cut; that chunk may
    carry handed rooms too until the socket ends. A room a 44100 adds
    after the EOSE is handed at once.
    `handoffKept` counts the rooms kept.
  - Mobile: every snapshot room is handed. Mobile room chunks are `limit: 0`
    and never re-delivered the gap, so keeping them would repair nothing.
    The listeners reload from the relay whatever the coverage: the open
    room refetches its newest page on reconnect, and the unread catch-up
    runs from read markers for every member room
    ([Catch-up at the stream EOSE](#catch-up-at-the-stream-eose)).
- **`shadow`, or a CLOSED before the EOSE:** chunks stay.
- **Carried rooms:** the snapshot rooms, plus a room whose 44100 arrives
  after the stream EOSE (below), minus a room whose 44101 arrives. The
  stream's coverage spans and the mid-connection fallback cover the handed
  ones only.
- **Chunk catch-up and repair (`chunkCatchUp`):** while the mode is unknown
  (no accepted EOSE yet), and with no stream or in `shadow`, the chunks'
  own catch-up and desktop's chunk repair run as today; walks already
  running when the stream EOSE is accepted finish as today. After an
  accepted EOSE in `live`/`gated`, chunk catch-ups and repairs skip the
  handed rooms, whose catch-up is the stream's; rooms outside the snapshot
  and desktop's kept rooms keep chunk catch-up. After a mid-connection
  CLOSED the reopened chunks follow the chunk rules again.

### Mid-connection fallback

A CLOSED after the stream EOSE in `live`/`gated`, other than an overflow
or `lagged` (those wait for the socket to close, above)
(`midConnectionClosed`):

1. Reopen chunks for every handed room at once (plus the extras and
   desktop's kept rooms, already on chunks).
2. Catch the store up for the carried rooms that have coverage, from their
   resume points (the stream's coverage end), batched and sorted as below,
   with L = the time of the CLOSED. The reopened chunks then follow the
   chunk rules from their own EOSE.
3. Desktop also runs the listener repair for every handed room, from the
   stream's last-seen time minus 1865 s, with each room's listener floor at
   its live start minus 5 s ([Reconnect](#reconnect)), alongside step 2 and
   the room on screen first. Its walks start no page once the socket ends.

New chunks would not repair a gap before their own EOSE, so these steps are
not optional. The stream's coverage spans stop growing at its CLOSED
(`spanStop`): chunk frames never extend them, and the reopened chunks
record their own spans from their EOSE.

### Membership notices in `live`/`gated`

44100/44101 ride the addressed account bundle as before; the stream hooks
into that listener (`membership`, `routing`).

- **44100** after the stream EOSE for a room the stream does not carry: the
  relay has already added it, so the room becomes carried, with a coverage
  span at the notice's receive time. A 44100 before the stream EOSE, or in
  `shadow`, changes nothing carried: the snapshot or the chunks cover the
  room.
- **44101:** the room stops being carried. A later stream frame for it whose
  `created_at` is after the 44101's is stored but not shown, until a 44100
  for the room that is not older than the 44101. Same-second frames are
  shown. A notice older than the last applied notice for that room is
  ignored. This rule is for rendering only; catch-up after a rejoin repairs
  the store.
- **Rooms not in the member list:** an archived room's frames are stored
  and reach an open room, never the all-rooms consumers. A room the list
  does not know is stored and triggers a debounced list refresh: one per
  burst (1 s), at most once per 10 s per room, and at most one every 5 s
  over all rooms. Desktop: the burst debounce is the room list's own
  (500 ms, trailing), behind the 10 s / 5 s throttle.

### Watermark (`room_seq`)

`room_seq(room, seq, at)` is a freshness token for the room's coverage head,
for the [coverage](#coverage) kinds only: "a walk that started after the
relay's seq for this room was `seq` completed up to `at`". It is not a
completeness proof.

**Rule W (`ruleW`).** A row `(S, L)` is written only when all hold:

1. S comes from the stream snapshot of connection G, and the catch-up that
   vouches for it sent its first page after that stream's EOSE, on G.
   Otherwise nothing is written and `seqWriteSkippedOrder` counts the room.
   In `shadow` a chunk-EOSE catch-up qualifies only when the stream EOSE came
   first. A later CLOSED of that stream does not undo it.
2. The walk completed (at most 4 pages, at most 7 days); L is its live time.
3. The write rides that catch-up's own span record in the write queue, under
   its capture and scope, and is refused exactly when the span is refused.
4. The write carries the epoch its catch-up started under; the writer
   refuses it when `sync_state` holds another epoch at apply time. A walk
   from an earlier connection still writes when the epoch is unchanged.
5. The writer never lowers a stored seq: a write with S below the stored
   seq is dropped (walks can end out of order). An equal S moves `at`.
6. Every path that clears coverage clears `room_seq` in the same writer
   transaction, and refuses the rule-W writes in flight across it
   (`clearing`):

| Event | Desktop | Mobile | Rows cleared; writes refused |
|---|---|---|---|
| Full write queue | writer loss note → file `lossy` | `writer.lossy` | all; writes whose walk started at or before the loss |
| Failed write transaction | `note_lossy` | `lossy` persisted | all; as a full queue |
| Memory guard (50000 waiting events) | — | as a full queue | all; as a full queue |
| Forgetting a room (incl. a rejoin forget) | `purge.rs` | `purge.dart` | that room; its writes whose walk started before the forget |
| Coverage version change, sign-out, scope end | as coverage | as coverage | all; every write of the old version or scope |
| Epoch change | `sync_state` epoch | same | all; every write tagged with the old epoch |

A row for a room missing from the snapshot is left alone: only room lists
forget rooms.

### Catch-up at the stream EOSE

At the stream EOSE (time L), each snapshot room with seq S is classed
(`snapshot`). Its coverage here is what it had **before this connection**
(`connectionSpans`): each connection records its start time before any
chunk REQ is answered (mobile: at AUTH OK, before its first REQ), and
catch-up resume points and the new-row base use only coverage that began
before that start time. An EOSE span `[C, C]` from a chunk on the same
connection is ignored, even when it came before the stream EOSE, and so is
the stream's own `[L, L]`. A span begun before the start and grown on this
connection (a chunk catch-up that finished before the stream EOSE) counts
up to its end. The mid-connection fallback keeps L = the CLOSED time as
its bound: there the stream's own span is the resume point.

- **uncovered:** no coverage that began before the connection start. Not
  fetched, as today.
- **equal:** covered and stored seq = S.
- **behind:** covered, and stored seq < S or no row.
- **ahead:** covered and stored seq > S (the relay DB was restored): the row
  is deleted, the room is walked like a behind one, and one line is logged.

`live` walks equal, behind and ahead rooms and counts equal ones as
`predictedSkips`; `gated` skips equal rooms and walks the rest; `shadow`
walks nothing itself (chunk catch-ups run as today) and counts
`predictedSkips`. The fixture lists these room sets by id.

- **Batches (`batching`):** first take out every stale room, one whose own
  resume point minus 1865 s is more than 7 days before L: it is not walked
  and records nothing, left for opening the room, as today. Sort the rest
  by resume point, then id, and split them 128 per batch; each batch walks
  from its oldest resume point minus 1865 s. One stale room never holds
  back fresh ones. Stream, audit and fallback catch-ups all batch this way.
- **Connection end and rate limits:** a stream catch-up starts no new batch
  or page once its connection ended (pages already written stay; rooms not
  reached count in no counter), and each page waits out the session's
  rate-limit window first. A connection's catch-up starts only after every
  earlier connection's catch-up has ended, so they never overlap. Desktop:
  the same holds for the audit, listener repair and fallback walks; the
  fallback's walks stop only when the socket ends.

A room's stream coverage span is written to the store only after that room's catch-up batch has completed (walked, or verified equal and skipped by gated). If the catch-up stops early (connection ended), rooms whose batch didn't complete get no stream span, so the next connection resumes them from their pre-existing coverage.

- **A skip records nothing (`skipCoverage`):** no span beyond the stream's
  own `[L, L]`, and `room_seq.at` does not move. The gap from the room's last
  `hi` to L stays uncovered, so opening or preloading the room fetches it,
  and desktop preload does not count the room as certain across it. R2's
  saving in this release is reconnect traffic only.
- **Desktop listener repair:** rooms walked at the stream EOSE (in `gated`:
  non-skipped plus the audit sample, drawn when the rooms are classed) also
  get the repair walk from the stream's last-seen time minus 1865 s; rows
  newer than each room's live start minus 5 s go to listeners, quiet rooms
  to the quiet listeners. The repair starts as soon as the rooms are
  classed, alongside the catch-up rather than after it, with the room on
  screen first on its own, then the rest 128 per walk. The last-seen time
  is the newest stream frame `created_at` or accepted live/gated EOSE time,
  whichever is later, as a chunk's cursor starts at its creation, so a
  connection that saw no frame still repairs the next one. A connection
  whose rooms rode chunks (stream off, refused, backed off, or ended before
  its EOSE) moves it to that connection's AUTH OK time when later, which is
  also the live start of its rooms that had none. It lives in memory: the
  first connect after launch, and the first after an account or community
  switch, has none and runs no listener repair. Rooms that were
  not handed get their chunk's repair instead ([Handoff](#handoff)).
- **Mobile:** store catch-up batched and sorted, plus the open-room refetch.
  The reconnect unread catch-up runs from read markers; a chunk re-sync
  that retires it before it lands (the hand-off at the stream EOSE, a
  44100/44101) runs it again.
  The quiet-room backfill after a drop waits for the stream EOSE instead of
  the chunks'.

### Counters

Counters are kept per account and server, persisted cumulatively in
`sync_state` with a report sequence number. A line `[user-stream] report
#<seq> <counters as JSON>` is logged when a counter changed by at least 10%
since the last line (any change from 0), or 10 min after the last line if
anything changed.

- **Already stored (`newRow`):** when the gate counters (`falseSkip`,
  `liveMissed`, `skipAuditNew`) are computed, a row stored locally at or
  after this connection's start time does not count as "already stored",
  whichever path stored it (chunk catch-up, unread refresh, room reload, live
  frame).
  - Desktop: keeps `events.stored_at` (ms, this device's clock; rows from
    before the column have none and count as stored before) and compares it
    with the connection start taken at AUTH OK. After a walk it asks only
    about the candidate rows (in the window, not received live), after the
    write queue drained. "Received live" means socket frames only: a row a
    repair or catch-up walk delivered is not. When it cannot tell (no
    segments or stored-at answer from a drained store, or too many live ids
    to track), the batch verifies nothing: its rooms count in none of
    `verifiedSkips`, `falseSkip`, `liveRoomsChecked`, `skipAudited` or
    `skipAuditNew` (an audited batch counts in `skipAuditFailed`), and
    `newRowUncounted` counts its rooms.
  - Mobile: takes a stored mark at the connection start (the newest
    `events` rowid once every earlier write is done); a row with a higher
    rowid was stored on this connection. The mark always answers, so mobile
    has no `newRowUncounted`. When the relay refused the stream on the last
    connection ([Handoff](#handoff)) the connect takes no mark; if this
    relay then gives the stream, the mark is taken at its EOSE instead.
- **New-row rule (`newRow`):** a row counts as new only if it was not
  already stored (above), it was not also received live on this connection
  (on the stream or a chunk, before or after the stream EOSE, including the
  frames the relay held until EOSE), and its `created_at` is at or before L
  and at or after the start of the room's newest coverage segment that
  began before the connection start (the one the walk resumes from; an
  EOSE span `[C, C]` from a chunk on the same connection and the stream's
  own `[L, L]` never count, so a gap row is never left out by
  construction).
- **Shadow comparison (`comparator`):** starts at the stream EOSE. A room is
  compared when a live chunk carries it and the stream carries it (snapshot
  or 44100). Each event id is classified once, 5 s (inclusive) after it was
  first seen on either side: seen on both → compared; chunk only →
  `chunkOnly`; stream only → `streamOnlyUnexpected` if the room is not in the
  snapshot and no 44100 for it arrived on this connection, else
  `streamOnlyExpected`. `eventsCompared` counts the ids of compared rooms.
  Windows still open when the stream CLOSEs or the connection ends are
  dropped, and frames after a stream CLOSED are ignored.
- **Skip verification (`skipVerification`):** a predicted skip whose batch
  completed is `verifiedSkips` with no new row, `falseSkip` with one. In
  `shadow`, the rooms are classed at the stream EOSE (against coverage from
  before the connection, after queueing the epoch) and counted in
  `predictedSkips`; a chunk catch-up that starts after that EOSE on the same
  connection writes the rule-W rows with its span and verifies the
  predicted rooms it walked, each room once per connection. Desktop chunk
  reconnect repairs start before the stream EOSE and verify nothing.
- **Skip audit (`gated`, `auditSampling`, `auditVerdicts`):** each reconnect
  samples uniformly up to 128 skipped rooms whose own walk start (resume
  point minus 1865 s) is within 7 days of L (all of them when there are 128
  or fewer), and walks them as ordinary catch-ups (they record spans and
  rows like any walk). A room of a completed batch counts in
  `skipAudited`, and in `skipAuditNew` when it got a new row; a room of a
  failed batch counts in `skipAuditFailed`.
- **`liveMissed` (`live`/`gated`, `liveMissed`, `lastFrame`):** for rooms in
  the previous connection's live set whose batch completed on this one
  (`liveRoomsChecked`), new rows with `created_at` from `prevEose + 5 s` to
  `prevLastFrame − 900 s` (both inclusive), leaving out kind 9005 signed by
  the relay (the key that signed part 1). `prevEose` is the previous part 1's
  `created_at`; `prevLastFrame` is the newest `min(created_at, receive time)`
  over that connection's stream frames, so a future-dated event cannot widen
  the window. The live set (carried rooms) and both bounds are persisted in
  `sync_state` at the EOSE and with each 60 s span extension; they are read
  before the new connection's EOSE replaces them. No previous live
  connection: nothing is checked.

| Counter | Mode | Must be |
|---|---|---|
| `eventsCompared` | shadow | denominator |
| `chunkOnly` | shadow | 0 |
| `streamOnlyUnexpected` / `streamOnlyExpected` | shadow | 0 / info |
| `predictedSkips` / `verifiedSkips` | shadow, live | denominator |
| `falseSkip` | shadow, live | 0 |
| `liveRoomsChecked` / `liveMissed` | live, gated | denominator / 0 |
| `skipAudited` / `skipAuditNew` / `skipAuditFailed` | gated | — / 0 / info |
| `seqWriteSkippedOrder`, `closed{key}`, `fallbacks{key}`, `reconnects`, `snapshotMs` | all | info |
| `handoffKept`, `newRowUncounted` (desktop only) | live, gated | info |

`handoffKept` and `newRowUncounted` are desktop only: mobile hands every
snapshot room to the stream and its stored mark always answers, so both
would stay 0.

`closed{key}` and `fallbacks{key}` use the keys of `closedReasons`
(`overflow` splits into `shadow_full` and `full`; an unknown reason is
`other`).

- **iOS report (mobile, iOS only):** iOS has no readable log, so each report
  line is also POSTed (NIP-98, the `/query` auth) to
  `/api/user-stream/shadow-report`, at most once every 5 min, and only
  while the connection's stream was accepted (`shadow`, `live` or `gated`),
  never with the stream off: `{"platform":
  "ios", "app_version": "<semver>", "seq": n, "counters": {<every counter
  above but the desktop-only ones>, "snapshotMs"}, "closed": {<every
  closedReasons key>}}`, every number a u32, at most 4096 bytes. Best effort: an error or a 404 (relay
  without the endpoint) is logged and not retried.

### Limits

- Rows changed without a new seq (soft-delete updates, visibility or
  history-policy changes, expiry) are missed by catch-up today; under
  `gated` the miss lasts until the room is opened or gets a new commit. R4
  deletion records cover non-author deletes.
- 40009 has a seq but is not a coverage kind; offline renames come back
  through the thread-name batch. 39005 is never stored.
- Backdated `created_at` is bounded at ±900 s by the relay. R4 backfill
  records carry the time they were written.
- The R4 backfill can write many 9005 rows with `created_at` near now in a
  few rooms; more than 2000 in one 128-room batch fails that catch-up
  (4 pages of 500).
- Frames carry no seq, so the 44101 rendering rule compares `created_at`
  with the notice's: a frame created before the removal but delivered after
  the notice is still shown.

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
  replies are drawn. Mobile keeps a thread's list hidden while it opens for
  at most about 1.2 s, the 1 s read state wait included (also when the
  target's time is not known yet); then the replies at hand show at the
  newest, and a position found later moves the list once, only if the reader
  has not scrolled meanwhile. A list with no replies is never hidden, and a
  linked reply that arrives later (a push inbox drain, the relay) still opens
  there.
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
