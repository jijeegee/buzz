import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';

/// Room metadata the bundle carries for every covered room (thread names).
/// Delivered only to metadata listeners ([RoomLiveRouter.listenMetadata]):
/// never to room listeners, unread, the chats list or the store. Mirrors
/// desktop's `ROOM_METADATA_KINDS`.
const roomMetadataKinds = [EventKind.threadName];

/// Kinds a room's live stream carries: the room's own events, the relay's
/// live thread-summary recounts and room metadata. Mirrors desktop's
/// `ROOM_LIVE_KINDS`.
const roomLiveEventKinds = [
  ...EventKind.channelEventKinds,
  EventKind.channelThreadSummary,
  ...roomMetadataKinds,
];

/// Whether a bundle event reaches the all-rooms consumers (unread, chats
/// list): thread summaries and room metadata never do.
bool reachesAllRoomListeners(NostrEvent event) =>
    event.kind != EventKind.channelThreadSummary &&
    !roomMetadataKinds.contains(event.kind);

/// The longest [RoomLiveRouter.whenRoomCovered] waits.
const roomCoverageWaitLimit = Duration(milliseconds: 1500);

/// Events remembered for duplicate suppression across overlapping deliveries.
const _seenEventLimit = 20000;

/// Event id → room, so h-less reactions and removals find their room.
const _eventRoomLimit = 50000;

/// How far before a room started riding the bundle a backfilled event may
/// be, for clock skew between the relay stamp and this device.
const quietBackfillSkewSeconds = 5;

typedef RoomLiveListener = void Function(NostrEvent event);

/// Gets a quiet room's event and the room it belongs to (h-less reactions
/// and removals carry no `h` tag).
typedef QuietRoomListener = void Function(NostrEvent event, String roomId);

/// Opens one room's own live subscription and returns its cleanup.
typedef OpenRoomLiveSubscription =
    Future<void Function()> Function(
      NostrFilter filter,
      void Function(NostrEvent) onEvent,
    );

/// Routes the shared room-bundle subscriptions to the open rooms, instead of
/// each open room holding a REQ of its own. Mirrors desktop's `RoomLiveBundle`
/// routing rules:
///
/// - Every event reaches the listeners of its room once: overlapping chunks
///   (while one replaces another) and a room's own fallback subscription can
///   carry the same event.
/// - Reactions and their removals carry no `h` tag. A one-room chunk names the
///   room; otherwise the room comes from the event they reference, as seen
///   here earlier or as held by an open room. An event whose room cannot be
///   told is dropped: it references nothing on screen.
/// - Thread summaries (39005) reach room listeners only.
/// - Room metadata ([roomMetadataKinds]) reaches metadata listeners only.
/// - Quiet rooms (hidden DMs) ride the bundle without being listed: their
///   events reach room listeners and quiet listeners, never the all-rooms
///   consumers (the chunk owner keeps those to listed rooms).
///
/// The bundle covers member rooms once their chunk is installed. A room opened
/// before that (cold start) or without membership (a browsed open channel)
/// gets its own one-room subscription, closed once the bundle covers it.
class RoomLiveRouter {
  RoomLiveRouter(
    this._openRoomSubscription, {
    Duration coverageWaitLimit = roomCoverageWaitLimit,
    int Function()? nowSeconds,
  }) : _coverageWaitLimit = coverageWaitLimit,
       _nowSeconds = nowSeconds ?? _wallSeconds;

  final OpenRoomLiveSubscription _openRoomSubscription;
  final Duration _coverageWaitLimit;
  final int Function() _nowSeconds;
  final _rooms = <String, _ListenedRoom>{};
  Set<String> _coveredRooms = const {};
  Set<String> _bundleRooms = const {};
  Set<String> _quietRooms = const {};

  /// Room → when it started riding the bundle (bundle ∪ quiet), in seconds.
  final _carriedSince = <String, int>{};
  final _metadataListeners = <RoomLiveListener>[];
  final _quietListeners = <QuietRoomListener>[];
  final _passiveListeners = <String, List<RoomLiveListener>>{};
  final _coverWaiters = <_CoverWaiter>[];
  final _seen = <String>{};
  final _eventRooms = <String, String>{};

  /// Room → [_readyTick] at its chunk's latest ready (EOSE after the REQ was
  /// sent or re-sent).
  final _readyTicks = <String, int>{};
  int _readyTick = 0;
  final _readyWaiters = <_ReadyWaiter>[];
  bool _disposed = false;

  /// Rooms whose bundle chunk is installed. Rooms listened to but not covered
  /// keep (or open) their own subscription; covered ones close it.
  void setCoveredRooms(Iterable<String> roomIds) {
    _coveredRooms = Set.unmodifiable(roomIds);
    for (final entry in _rooms.entries) {
      _reconcile(entry.key, entry.value);
    }
    _settleCoverWaiters();
  }

  /// Rooms the bundle is meant to carry (member and quiet rooms). Empty means
  /// not known yet (cold start, signed out).
  void setBundleRooms(Iterable<String> roomIds) {
    _bundleRooms = Set.unmodifiable(roomIds);
    _trackCarried();
    _settleCoverWaiters();
  }

  /// Rooms the bundle carries without listing them (hidden DMs). Their events
  /// also reach [listenQuiet] listeners.
  void setQuietRooms(Iterable<String> roomIds) {
    _quietRooms = Set.unmodifiable(roomIds);
    _trackCarried();
    _settleCoverWaiters();
  }

  bool isQuietRoom(String roomId) => _quietRooms.contains(roomId);

  /// Listens to quiet rooms' events (thread summaries and metadata
  /// excluded). Returns the function that stops listening.
  void Function() listenQuiet(QuietRoomListener listener) {
    _quietListeners.add(listener);
    return () => _quietListeners.remove(listener);
  }

  /// Hears [roomId]'s bundle events without asking for the room: opens no
  /// subscription and does not make the room wanted. Returns the function
  /// that stops listening.
  void Function() listenRoomEvents(String roomId, RoomLiveListener listener) {
    final listeners = _passiveListeners.putIfAbsent(roomId, () => []);
    listeners.add(listener);
    return () {
      listeners.remove(listener);
      if (listeners.isEmpty &&
          identical(_passiveListeners[roomId], listeners)) {
        _passiveListeners.remove(roomId);
      }
    };
  }

  /// Delivers quiet rooms' events read from history (after a reconnect) as
  /// if they came live: once per event, through the same duplicate check.
  /// Events older than the room's start on the bundle (less
  /// [quietBackfillSkewSeconds]) are dropped, so a room that just turned
  /// quiet does not replay its past. Returns the events delivered.
  List<NostrEvent> routeQuietBackfill(Iterable<NostrEvent> events) {
    final delivered = <NostrEvent>[];
    for (final event in events) {
      final roomId = event.channelId;
      if (roomId == null || !_quietRooms.contains(roomId)) continue;
      if (_seen.contains(event.id)) continue;
      final since = _carriedSince[roomId];
      if (since != null && event.createdAt < since - quietBackfillSkewSeconds) {
        continue;
      }
      _deliver(event, roomId);
      delivered.add(event);
    }
    return delivered;
  }

  /// When [roomId] started riding the bundle, or null when it does not.
  int? carriedSince(String roomId) => _carriedSince[roomId];

  /// Moves on at every chunk ready ([markRoomsReady]). Read it when the
  /// connection drops, then wait with [whenRoomsReadySince].
  int get readyTick => _readyTick;

  /// The chunk carrying [roomIds] is ready: its REQ (sent or re-sent after a
  /// reconnect) reached EOSE, so live events of these rooms arrive from here.
  void markRoomsReady(Iterable<String> roomIds) {
    final tick = ++_readyTick;
    for (final roomId in roomIds) {
      _readyTicks[roomId] = tick;
    }
    for (final waiter in List.of(_readyWaiters)) {
      if (_readySince(waiter.roomIds, waiter.tick)) waiter.settle(true);
    }
  }

  /// `true` once every room in [roomIds] had a chunk ready after [tick];
  /// `false` when [limit] passed first or the router is disposed.
  Future<bool> whenRoomsReadySince(
    Iterable<String> roomIds,
    int tick, {
    Duration limit = roomCoverageWaitLimit,
  }) {
    final rooms = roomIds.toSet();
    if (_readySince(rooms, tick)) return Future.value(true);
    if (_disposed) return Future.value(false);
    final waiter = _ReadyWaiter(rooms, tick, _readyWaiters.remove);
    _readyWaiters.add(waiter);
    waiter.timer = Timer(limit, () => waiter.settle(false));
    return waiter.done.future;
  }

  bool _readySince(Set<String> roomIds, int tick) =>
      roomIds.every((roomId) => (_readyTicks[roomId] ?? 0) > tick);

  /// Listens to room metadata ([roomMetadataKinds]) of every room this router
  /// sees: bundle chunks and the rooms' own subscriptions. Returns the
  /// function that stops listening.
  void Function() listenMetadata(RoomLiveListener listener) {
    _metadataListeners.add(listener);
    return () => _metadataListeners.remove(listener);
  }

  /// Completes once live events of [roomId] reach this router, so a history
  /// read started afterwards misses nothing. Completes at the first of:
  /// the room is covered (its bundle chunk or its own subscription is up);
  /// the bundle's rooms are known, this room is not one of them and nothing
  /// else carries it (its live events never come); [roomCoverageWaitLimit].
  Future<void> whenRoomCovered(String roomId) {
    if (_disposed || _roomCovered(roomId)) return Future.value();
    final waiter = _CoverWaiter(roomId);
    _coverWaiters.add(waiter);
    waiter.timer = Timer(_coverageWaitLimit, () => _settle(waiter));
    return waiter.done.future;
  }

  /// Listens to [roomId]'s live events, thread summaries included. Completes
  /// once the room is covered by the bundle or its own subscription is set up,
  /// with the function that stops listening.
  Future<void Function()> listenRoom(
    String roomId,
    RoomLiveListener listener, {
    bool Function(String eventId)? holdsEvent,
  }) async {
    final room = _rooms.putIfAbsent(roomId, _ListenedRoom.new);
    final entry = _RoomListener(listener, holdsEvent);
    room.listeners.add(entry);
    _reconcile(roomId, room);
    var stopped = false;
    void stop() {
      if (stopped) return;
      stopped = true;
      room.listeners.remove(entry);
      if (room.listeners.isEmpty && identical(_rooms[roomId], room)) {
        _rooms.remove(roomId);
        _closeOwn(room);
      }
    }

    await room.ownReady;
    return stop;
  }

  /// Routes one event from a bundle chunk covering [chunkRooms] to its room's
  /// listeners (once per event) and returns that room, or null when the room
  /// cannot be told. The all-rooms consumers (unread, chats list) keep their
  /// own contract on every delivery, so a duplicate still returns its room.
  String? routeBundleEvent(NostrEvent event, Set<String> chunkRooms) {
    final roomId = _roomOf(event, chunkRooms);
    if (roomId == null) return null;
    if (!_seen.contains(event.id)) _deliver(event, roomId);
    return roomId;
  }

  void dispose() {
    _disposed = true;
    for (final room in _rooms.values) {
      _closeOwn(room);
    }
    _rooms.clear();
    _metadataListeners.clear();
    _quietListeners.clear();
    _passiveListeners.clear();
    for (final waiter in List.of(_coverWaiters)) {
      _settle(waiter);
    }
    for (final waiter in List.of(_readyWaiters)) {
      waiter.settle(false);
    }
  }

  bool _roomCovered(String roomId) {
    if (_coveredRooms.contains(roomId)) return true;
    final room = _rooms[roomId];
    if (room != null) return room.ownUnsubscribe != null;
    return _bundleRooms.isNotEmpty &&
        !_bundleRooms.contains(roomId) &&
        !_quietRooms.contains(roomId);
  }

  void _trackCarried() {
    final carried = {..._bundleRooms, ..._quietRooms};
    _carriedSince.removeWhere((roomId, _) => !carried.contains(roomId));
    final now = _nowSeconds();
    for (final roomId in carried) {
      _carriedSince.putIfAbsent(roomId, () => now);
    }
  }

  void _settleCoverWaiters() {
    for (final waiter in List.of(_coverWaiters)) {
      if (_roomCovered(waiter.roomId)) _settle(waiter);
    }
  }

  void _settle(_CoverWaiter waiter) {
    waiter.timer?.cancel();
    if (!_coverWaiters.remove(waiter)) return;
    waiter.done.complete();
  }

  String? _roomOf(NostrEvent event, Set<String> chunkRooms) {
    final tagged = event.channelId;
    if (tagged != null) return tagged;
    if (chunkRooms.length == 1) return chunkRooms.first;
    final referenced = [
      for (final tag in event.tags)
        if (tag.length > 1 && tag[0] == 'e') tag[1],
    ].reversed;
    for (final eventId in referenced) {
      final known = _eventRooms[eventId];
      if (known != null && chunkRooms.contains(known)) return known;
      for (final MapEntry(key: roomId, value: room) in _rooms.entries) {
        if (chunkRooms.contains(roomId) && room.holds(eventId)) return roomId;
      }
    }
    return null;
  }

  void _deliver(NostrEvent event, String roomId) {
    _remember(_seen, event.id, _seenEventLimit);
    _eventRooms.remove(event.id);
    _eventRooms[event.id] = roomId;
    if (_eventRooms.length > _eventRoomLimit) {
      _eventRooms.remove(_eventRooms.keys.first);
    }
    if (roomMetadataKinds.contains(event.kind)) {
      for (final listener in List.of(_metadataListeners)) {
        listener(event);
      }
      return;
    }
    final room = _rooms[roomId];
    if (room != null) {
      for (final entry in List.of(room.listeners)) {
        entry.listener(event);
      }
    }
    for (final listener in List.of(_passiveListeners[roomId] ?? const [])) {
      listener(event);
    }
    if (event.kind == EventKind.channelThreadSummary ||
        !_quietRooms.contains(roomId)) {
      return;
    }
    for (final listener in List.of(_quietListeners)) {
      listener(event, roomId);
    }
  }

  void _reconcile(String roomId, _ListenedRoom room) {
    if (_coveredRooms.contains(roomId)) {
      _closeOwn(room);
      return;
    }
    if (room.ownUnsubscribe != null || room.opening) return;
    _openOwn(roomId, room);
  }

  void _openOwn(String roomId, _ListenedRoom room) {
    if (_disposed) return;
    room.opening = true;
    final ready = Completer<void>();
    room.ownReady = ready.future;
    _openRoomSubscription(
      NostrFilter(
        kinds: roomLiveEventKinds,
        tags: {
          '#h': [roomId],
        },
        // Live only: missed history comes from the room's own refresh.
        limit: 0,
      ),
      (event) {
        if (!identical(_rooms[roomId], room) || _seen.contains(event.id)) {
          return;
        }
        // A one-room subscription names the room of an h-less event.
        _deliver(event, event.channelId ?? roomId);
      },
    ).then(
      (unsubscribe) {
        room.opening = false;
        ready.complete();
        final stillWanted =
            !_disposed &&
            identical(_rooms[roomId], room) &&
            !_coveredRooms.contains(roomId);
        if (stillWanted) {
          room.ownUnsubscribe = unsubscribe;
          _settleCoverWaiters();
        } else {
          unsubscribe();
        }
      },
      onError: (Object error) {
        room.opening = false;
        ready.complete();
        debugPrint(
          '[RoomLiveRouter] live subscription failed for $roomId: $error',
        );
      },
    );
  }

  void _closeOwn(_ListenedRoom room) {
    final unsubscribe = room.ownUnsubscribe;
    room.ownUnsubscribe = null;
    unsubscribe?.call();
  }
}

int _wallSeconds() => DateTime.now().millisecondsSinceEpoch ~/ 1000;

void _remember(Set<String> seen, String id, int limit) {
  seen.add(id);
  if (seen.length > limit) seen.remove(seen.first);
}

class _CoverWaiter {
  _CoverWaiter(this.roomId);

  final String roomId;
  final done = Completer<void>();
  Timer? timer;
}

class _ReadyWaiter {
  _ReadyWaiter(this.roomIds, this.tick, this._forget);

  final Set<String> roomIds;
  final int tick;
  final bool Function(_ReadyWaiter) _forget;
  final done = Completer<bool>();
  Timer? timer;

  void settle(bool ready) {
    timer?.cancel();
    _forget(this);
    if (!done.isCompleted) done.complete(ready);
  }
}

class _RoomListener {
  _RoomListener(this.listener, this.holdsEvent);

  final RoomLiveListener listener;
  final bool Function(String eventId)? holdsEvent;
}

class _ListenedRoom {
  final listeners = <_RoomListener>[];
  void Function()? ownUnsubscribe;
  bool opening = false;
  Future<void> ownReady = Future.value();

  bool holds(String eventId) =>
      listeners.any((entry) => entry.holdsEvent?.call(eventId) ?? false);
}

/// The active relay session's room router. Rebuilt with the relay or account,
/// so remembered events never cross scopes.
final roomLiveRouterProvider = Provider<RoomLiveRouter>((ref) {
  ref.watch(relayConfigProvider.select((config) => config.baseUrl));
  ref.watch(myPubkeyProvider);
  final router = RoomLiveRouter(
    (filter, onEvent) =>
        ref.read(relaySessionProvider.notifier).subscribe(filter, onEvent),
  );
  ref.onDispose(router.dispose);
  return router;
});
