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

## Message store

Every room event the device receives is kept on the device, with the spans
known complete ([Coverage](#coverage)). Opening a room reads its newest
window from the store when it is certain
([Opening a room from the store](#opening-a-room-from-the-store)), and
opening a thread its newest replies
([Opening a thread from the store](#opening-a-thread-from-the-store)); older
pages still load from the relay.

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

### Opening a room from the store

On a room's first fetch, the client reads the room's newest window from the
store and uses it as the relay's channel-window answer when it is certain;
otherwise it asks the relay as before. A first fetch is one for a room this
run has not yet fetched from the store or the relay, the background preload
included. Desktop tracks this per query cache (a community switch starts a
new one); mobile per room notifier, which lives for the run, and for the
preload per account and server. A stored window only draws the first frame: once that fetch has
settled, the client asks the relay once (on desktop, the refresh after the
room's live subscription joins that request rather than sending another) and
its window replaces the stored rows (stored rows inside its span that the relay no longer has are dropped; older rows that join it stay, see [Opening at a position](#opening-at-a-position); live arrivals stay).
This extra request is a safety net until the store-vs-relay log shows no
differences (stage 5 then skips it for certain rooms). Later refreshes always
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
- **Certain** when all of: the window has 50 rows; a live REQ carrying the
  room reached EOSE at L on the current connection and has not been closed
  since (the client passes L; none means not certain); one coverage span
  has `lo ≤ created_at of the 50th row − 1865` and `hi ≥ L`; and the room
  has never stored a job row (43001–43006). Job rows show in the window, but
  live REQs do not carry them (that set is frozen), so coverage cannot vouch
  for them. A room's first job row is still missed until something stores
  one; the relay request after a stored open (above) corrects that frame.
- **Written first:** the read waits (at most 0.5 s) until every event
  received before it is written. A writer that is gone, still busy after the
  wait, or holding a loss not yet written down means not certain.

### Opening a thread from the store

On a thread's first load (no reply page held for it: desktop's thread
window state, mobile's thread window; both are released some time after the
thread closes, and the next open counts as first again), the client reads the
thread's newest replies from the store and uses them as the relay's
thread-window answer when it is certain; otherwise it asks the relay as
before. As with rooms, the thread then asks the relay once and folds its
newest page in. On desktop, a load that must include a linked reply skips
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
  current connection and has not been closed since, and one coverage span has
  `hi ≥ L` and `lo ≤` (full window: the oldest reply's `created_at`; fewer
  replies: the root's `created_at`, which must be stored in the room) `− 1865`.
- **Written first:** as for rooms.

### Limits

- **since_join rooms:** the relay hides rows from before the reader joined.
  The store keeps only what the relay sent this account, so it holds no such
  rows, but rows kept from an earlier membership (left, then re-added) stay
  and can open in a stored window until the relay's answer replaces it.
- **Deletions without an event:** the store drops a row only for a removal
  (9005) or author deletion (5) it has received. A row the relay deletes
  without publishing such an event (an admin deletion today) cannot be seen
  from the store, so a stored window may show it until the relay's answer
  replaces it. The relay request after every stored open covers this until
  the relay keeps a deletion record clients can fetch (a later stage).
- **Job rows:** see the room's certain rule above.
- **Scope capture (desktop):** a span's scope is read just after its EOSE,
  not at it; an account switch on the same server in that moment could
  file the span under the new account. Not yet closed.
- **Build switch:** `VITE_KUZZ_STORE_OPEN=off` (desktop) or
  `--dart-define=KUZZ_STORE_OPEN=off` (mobile) opens every room and thread
  from the relay. Events are still stored and covered.
- **Head cache:** only a relay window is saved as a room's head cache, never
  a window opened from the store.

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

- **Decided on a current window (desktop):** the position is chosen only on
  a window fetched during this visit (from the relay, or a certain window
  from the store), or one fetched earlier whose newest row is at least as new
  as the channel's last message. A head cache from an earlier run, the fetch
  that only hands it over, and rows merged live never count as fetched: they
  can miss newer unread rows, so the skeleton stays until the relay
  answers, at most 1.5 s, after which the rows held decide. A visit that
  cannot have unread rows (last message not after the read frontier) and has
  no remembered row opens at the newest at once. A window counts once its
  newest row has reached the rows drawn, not when it is merely announced.
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
- **Waiting for the read state:** first unread and the remembered row are
  only known with the read state. A room or thread opened before its first
  load stays hidden for up to 1 s, decides once when it is ready, and
  otherwise opens at the newest without trusting a remembered row or (for
  a thread) its unread replies. A read state that is stuck loading costs one
  wait per session, not one per room. With no read state at all (no account
  signed in) nothing waits. Opening does not mark the room read before
  that decision. A link does not wait.
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
  new page delivers, desktop takes the page's summary and mobile keeps a live
  one newer than it (as before); desktop drops the rest unless the root is a
  live overlay row. A mobile window opened at a
  message starts over at the newest page. Cases:
  `test-fixtures/channel-window-replace.json`.
- **Limit:** a kept older row keeps what it had: if it was deleted, edited,
  reacted to or replied to while the client was disconnected (desktop's
  reconnect repair stores such events but does not hand older ones to the
  room), that shows only once the room is fetched again from the newest page
  with no older rows kept, or the row is paged in again. Mobile's screen list
  also keeps rows its window drops (as before), so after a replace that does
  not join, desktop and mobile can show different older rows until reopened.
