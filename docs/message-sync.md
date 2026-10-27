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

The account bundle's cases live in
[`test-fixtures/me-live-bundle.json`](../test-fixtures/me-live-bundle.json).

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

## Account bundle

Live-only subscriptions about the account itself (settings documents,
contact list, agent personas and teams, chat-list pings, sign-in notices)
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
- **Agent devices** (30180/30181) belong with the agent subscription.
- **Community-wide streams** (user status, presence, catalogs, custom emoji)
  are not about the account.
