import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import '../../shared/relay/relay.dart';

/// Kinds a room's live stream carries: the room's own events plus the relay's
/// live thread-summary recounts. Mirrors desktop's `ROOM_LIVE_KINDS`.
const roomLiveEventKinds = [
  ...EventKind.channelEventKinds,
  EventKind.channelThreadSummary,
];

/// Events remembered for duplicate suppression across overlapping deliveries.
const _seenEventLimit = 20000;

/// Event id → room, so h-less reactions and removals find their room.
const _eventRoomLimit = 50000;

typedef RoomLiveListener = void Function(NostrEvent event);

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
///
/// The bundle covers member rooms once their chunk is installed. A room opened
/// before that (cold start) or without membership (a browsed open channel)
/// gets its own one-room subscription, closed once the bundle covers it.
class RoomLiveRouter {
  RoomLiveRouter(this._openRoomSubscription);

  final OpenRoomLiveSubscription _openRoomSubscription;
  final _rooms = <String, _ListenedRoom>{};
  Set<String> _coveredRooms = const {};
  final _seen = <String>{};
  final _eventRooms = <String, String>{};
  bool _disposed = false;

  /// Rooms whose bundle chunk is installed. Rooms listened to but not covered
  /// keep (or open) their own subscription; covered ones close it.
  void setCoveredRooms(Iterable<String> roomIds) {
    _coveredRooms = Set.unmodifiable(roomIds);
    for (final entry in _rooms.entries) {
      _reconcile(entry.key, entry.value);
    }
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
    final room = _rooms[roomId];
    if (room == null) return;
    for (final entry in List.of(room.listeners)) {
      entry.listener(event);
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

void _remember(Set<String> seen, String id, int limit) {
  seen.add(id);
  if (seen.length > limit) seen.remove(seen.first);
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
