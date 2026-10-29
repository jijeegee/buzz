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
| Agent observer filter | `desktop/src/shared/api/observerLiveFilter.ts` | `observer_subscription.dart` (`observerLive*`) |

The account bundle's cases live in
[`test-fixtures/me-live-bundle.json`](../test-fixtures/me-live-bundle.json),
the message store's in
[`test-fixtures/message-store.json`](../test-fixtures/message-store.json).

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

## Reconnect

- Desktop re-sends each chunk unchanged and repairs the missed window over
  HTTP: one keyset walk per chunk covering all its rooms, from the chunk's
  last-seen event time (or its start) minus the relay's created-at skew
  allowance.
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
  connection dropped minus the relay's created-at skew allowance, up to 500
  events. A failed catch-up owes its window to the next reconnect.
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

## Message store

Every room event the device receives is kept on the device, with the spans
known complete ([Coverage](#coverage)). Nothing reads the store yet; rooms and
threads still load from the relay.

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
  batched (up to 512 rows per transaction). A full queue or a failed write
  drops events and marks the file `lossy`, so a later catch-up does not take
  it for complete.
- **Sign-out** deletes every file of that account on this device and ends its
  scope: answers to requests sent before the sign-out write nothing. Other
  accounts' files are untouched.
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
  keyset walk (`#h`, repair kinds, `since`, `until` L, `before_id`, 500 per
  page) into the store only; nothing is shown. When it ends within 4 pages it
  records `[since, L]`. A longer or more than 7-day gap records nothing and
  is left for opening the room. Rooms without coverage are never fetched.
- **Order and loss:** spans go through the same write queue as events, so a
  span is recorded only after the events received before it are written. A
  dropped event or failed write clears every span of the file, and any later
  span starting at or before the time of the loss is not recorded.
- **Sign-out** deletes coverage with the file.
