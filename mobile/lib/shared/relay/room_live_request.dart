import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'nostr_models.dart';
import 'relay_provider.dart';
import 'relay_session.dart';

/// Goal tree heads (kuzz-core `KIND_GOAL_TREE`).
const roomLiveGoalTreeKind = 40111;

/// The kinds an open room's own live REQ carries, one filter each, always all
/// of them: a listener coming or going never re-sends the REQ. Shared with
/// desktop through `test-fixtures/room-live-request.json`
/// (docs/message-sync.md#open-room-req).
const roomLiveRequestKinds = [EventKind.typingIndicator, roomLiveGoalTreeKind];

/// The open-room REQ's filters for [roomId]: live only, one per kind.
List<NostrFilter> roomLiveRequestFilters(String roomId) => [
  for (final kind in roomLiveRequestKinds)
    NostrFilter(
      kinds: [kind],
      tags: {
        '#h': [roomId],
      },
      limit: 0,
    ),
];

/// Opens one room's REQ; [onGone] runs when the relay drops it for good.
typedef OpenRoomLiveRequest =
    Future<void Function()> Function(
      List<NostrFilter> filters,
      void Function(NostrEvent) onEvent,
      void Function() onGone,
    );

class _Listener {
  _Listener(this.kind, this.onEvent, this.onResync, this.onGone);

  final int kind;
  final void Function(NostrEvent) onEvent;
  final void Function()? onResync;
  final void Function()? onGone;
}

class _Entry {
  _Entry(this.roomId);

  final String roomId;
  final listeners = <_Listener>[];
  void Function()? close;
  bool closing = false;
  bool gone = false;
}

/// One live REQ per open room for the room's typing indicators and goal tree,
/// shared by every listener of that room ([roomLiveRequestKinds]). Mirrors
/// desktop's `RoomLiveRegistry`:
///
/// - The first listener opens it; it closes one microtask after the last
///   listener leaves, so a rebuild in the same task keeps it.
/// - [listen] returns its stop function at once.
/// - A terminal CLOSED calls every listener's `onGone` and forgets the room,
///   so a later listener opens a fresh REQ.
/// - [resync] (after a reconnect) calls `onResync` once per listener. The
///   session's replay re-sends the REQ itself; nothing here reopens it.
class RoomLiveRegistry {
  RoomLiveRegistry(this._open, {void Function(void Function())? defer})
    : _defer = defer ?? scheduleMicrotask;

  final OpenRoomLiveRequest _open;
  final void Function(void Function()) _defer;
  final _entries = <String, _Entry>{};
  bool _disposed = false;

  void Function() listen(
    String roomId,
    int kind,
    void Function(NostrEvent) onEvent, {
    void Function()? onResync,
    void Function()? onGone,
  }) {
    if (!roomLiveRequestKinds.contains(kind)) {
      throw ArgumentError('Kind $kind does not ride the open-room REQ.');
    }
    final entry = _entries[roomId] ?? _openEntry(roomId);
    final listener = _Listener(kind, onEvent, onResync, onGone);
    entry.listeners.add(listener);
    entry.closing = false;
    var stopped = false;
    return () {
      if (stopped) return;
      stopped = true;
      entry.listeners.remove(listener);
      if (entry.listeners.isNotEmpty || entry.closing) return;
      entry.closing = true;
      _defer(() {
        if (!entry.closing || entry.listeners.isNotEmpty) return;
        _closeEntry(entry);
      });
    };
  }

  /// The session reconnected: every listener reads what it may have missed.
  void resync() {
    for (final entry in List.of(_entries.values)) {
      for (final listener in List.of(entry.listeners)) {
        listener.onResync?.call();
      }
    }
  }

  /// Rooms with a REQ open or opening.
  @visibleForTesting
  List<String> get openRooms => List.unmodifiable(_entries.keys);

  void dispose() {
    _disposed = true;
    for (final entry in List.of(_entries.values)) {
      _closeEntry(entry);
    }
  }

  _Entry _openEntry(String roomId) {
    final entry = _Entry(roomId);
    _entries[roomId] = entry;
    void gone() {
      if (entry.gone) return;
      entry.gone = true;
      if (identical(_entries[roomId], entry)) _entries.remove(roomId);
      for (final listener in List.of(entry.listeners)) {
        listener.onGone?.call();
      }
    }

    if (_disposed) return entry;
    _open(roomLiveRequestFilters(roomId), (event) {
      if (entry.gone) return;
      for (final listener in List.of(entry.listeners)) {
        if (listener.kind == event.kind) listener.onEvent(event);
      }
    }, gone).then(
      (close) {
        if (!identical(_entries[roomId], entry)) {
          close();
          return;
        }
        entry.close = close;
      },
      onError: (Object error) {
        debugPrint('[RoomLiveRegistry] room REQ failed for $roomId: $error');
        gone();
      },
    );
    return entry;
  }

  void _closeEntry(_Entry entry) {
    if (identical(_entries[entry.roomId], entry)) {
      _entries.remove(entry.roomId);
    }
    entry.gone = true;
    final close = entry.close;
    entry.close = null;
    close?.call();
  }
}

/// The active relay session's open-room REQs. Rebuilt with the relay or
/// account. A reconnect calls [RoomLiveRegistry.resync]; the session's replay
/// re-sends the REQs.
final roomLiveRegistryProvider = Provider<RoomLiveRegistry>((ref) {
  ref.watch(relayConfigProvider.select((config) => config.baseUrl));
  ref.watch(myPubkeyProvider);
  final registry = RoomLiveRegistry(
    (filters, onEvent, onGone) => ref
        .read(relaySessionProvider.notifier)
        .subscribeRoomFilters(filters, onEvent, onClosed: (_) => onGone()),
  );
  // Only a reconnect resyncs, not the first connect.
  var connectedBefore =
      ref.read(relaySessionProvider).status == SessionStatus.connected;
  ref.listen(relaySessionProvider.select((session) => session.status), (
    previous,
    next,
  ) {
    if (next != SessionStatus.connected || previous == next) return;
    if (connectedBefore) registry.resync();
    connectedBefore = true;
  });
  ref.onDispose(registry.dispose);
  return registry;
});
