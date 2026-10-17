import 'dart:async';
import 'dart:convert';

import 'package:flutter/widgets.dart';
import 'package:flutter_hooks/flutter_hooks.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../shared/relay/relay.dart';
import '../../shared/theme/theme_provider.dart';
import 'channel_management_provider.dart';

/// Preference key prefix; the relay and account pubkey complete the key.
const roomPositionPrefsKey = 'kuzz-room-position.v1';

/// Rooms remembered at once; the longest-unsaved is dropped first.
const maxRoomPositions = 200;

/// How long scrolling must rest before its position is saved.
const roomPositionSaveDelay = Duration(milliseconds: 500);

/// The room key of a thread page, next to its channel's.
String threadRoomKey(String channelId, String headId) => '$channelId/$headId';

/// Where a reader left a room: the row they were reading, and where its
/// bottom edge sat as a fraction of the timeline's height from the bottom.
class RoomPosition {
  const RoomPosition({
    required this.id,
    required this.createdAt,
    required this.edge,
    required this.savedAt,
  });

  final String id;
  final int createdAt;
  final double edge;

  /// Milliseconds since the epoch.
  final int savedAt;

  Map<String, Object> toJson() => {
    'id': id,
    'createdAt': createdAt,
    'edge': edge,
    'savedAt': savedAt,
  };

  static RoomPosition? fromJson(Object? json) {
    if (json is! Map) return null;
    final id = json['id'];
    final createdAt = json['createdAt'];
    final edge = json['edge'];
    final savedAt = json['savedAt'];
    if (id is! String || createdAt is! int || edge is! num || savedAt is! int) {
      return null;
    }
    return RoomPosition(
      id: id,
      createdAt: createdAt,
      edge: edge.toDouble(),
      savedAt: savedAt,
    );
  }
}

/// What a timeline shows when its position is read: the newest row
/// ([atLatest], nothing to remember) or a row further up.
class RoomPositionReading {
  const RoomPositionReading.atLatest() : id = null, createdAt = 0, edge = 0;

  const RoomPositionReading.at({
    required String this.id,
    required this.createdAt,
    required this.edge,
  });

  final String? id;
  final int createdAt;
  final double edge;
}

/// Per-room reading positions of one account on one relay, kept on device.
class RoomPositionStore {
  RoomPositionStore(
    this._prefs, {
    required String relay,
    required String pubkey,
    int Function()? now,
  }) : prefsKey = '$roomPositionPrefsKey:$relay:$pubkey',
       _now = now ?? (() => DateTime.now().millisecondsSinceEpoch);

  final SharedPreferences _prefs;
  final String prefsKey;
  final int Function() _now;

  RoomPosition? read(String roomKey) => _readAll()[roomKey];

  /// Remembers [reading] for [roomKey]; a reading at the newest row forgets
  /// the room instead, so it opens at the newest row again.
  void record(String roomKey, RoomPositionReading reading) {
    final id = reading.id;
    if (id == null) return clear(roomKey);
    final positions = _readAll()
      ..[roomKey] = RoomPosition(
        id: id,
        createdAt: reading.createdAt,
        edge: reading.edge,
        savedAt: _now(),
      );
    while (positions.length > maxRoomPositions) {
      final oldest = positions.entries.reduce(
        (a, b) => b.value.savedAt < a.value.savedAt ? b : a,
      );
      positions.remove(oldest.key);
    }
    _writeAll(positions);
  }

  void clear(String roomKey) {
    final positions = _readAll();
    if (positions.remove(roomKey) == null) return;
    _writeAll(positions);
  }

  Map<String, RoomPosition> _readAll() {
    final raw = _prefs.getString(prefsKey);
    if (raw == null) return {};
    try {
      final decoded = jsonDecode(raw);
      if (decoded is! Map) return {};
      return {
        for (final entry in decoded.entries)
          if (entry.key is String && RoomPosition.fromJson(entry.value) != null)
            entry.key as String: RoomPosition.fromJson(entry.value)!,
      };
    } on FormatException {
      return {};
    }
  }

  void _writeAll(Map<String, RoomPosition> positions) {
    unawaited(
      _prefs.setString(
        prefsKey,
        jsonEncode({
          for (final entry in positions.entries)
            entry.key: entry.value.toJson(),
        }),
      ),
    );
  }
}

/// The signed-in account's room positions on the active relay; null without
/// an account. A page keeps the store it opened with, so a position is never
/// written under an account switched to meanwhile.
final roomPositionStoreProvider = Provider<RoomPositionStore?>((ref) {
  final pubkey = ref.watch(currentPubkeyProvider);
  if (pubkey == null || pubkey.isEmpty) return null;
  return RoomPositionStore(
    ref.watch(savedPrefsProvider),
    relay: ref.watch(relayConfigProvider).baseUrl,
    pubkey: pubkey,
  );
});

/// Keeps [roomKey]'s saved position current from [read], which returns null
/// while the timeline has nothing to report (still opening, not laid out).
/// Saves when the app is paused and when the caller goes away; returns the
/// callback that saves once scrolling has rested [roomPositionSaveDelay].
VoidCallback useRoomPositionSaving({
  required RoomPositionStore? store,
  required String roomKey,
  required RoomPositionReading? Function() read,
}) {
  final readRef = useRef(read)..value = read;
  final pending = useRef<Timer?>(null);

  void saveNow() {
    pending.value?.cancel();
    pending.value = null;
    if (store == null) return;
    final reading = readRef.value();
    if (reading != null) store.record(roomKey, reading);
  }

  useOnAppLifecycleStateChange((_, state) {
    if (state == AppLifecycleState.paused ||
        state == AppLifecycleState.hidden) {
      saveNow();
    }
  });
  useEffect(() => saveNow, [store, roomKey]);
  return () {
    pending.value?.cancel();
    pending.value = Timer(roomPositionSaveDelay, saveNow);
  };
}
