import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:path_provider/path_provider.dart';

import '../../shared/relay/relay.dart';
import 'channel_room_store.dart';

/// Identity a persisted channel head belongs to. Heads are never shared across
/// accounts or relays.
@immutable
class ChannelHeadScope {
  const ChannelHeadScope({required this.relayUrl, required this.pubkey});

  final String relayUrl;
  final String pubkey;

  @override
  bool operator ==(Object other) =>
      other is ChannelHeadScope &&
      other.relayUrl == relayUrl &&
      other.pubkey == pubkey;

  @override
  int get hashCode => Object.hash(relayUrl, pubkey);
}

/// Persists each channel's newest window response so reopening the app can
/// paint a channel before the relay answers, and next to it the room's
/// [RoomTail], which live events keep current for every joined room.
///
/// The head is a paint accelerator only: the relay's response always replaces
/// the cached page. The store is fully derived from relay events, so it is
/// discarded rather than migrated when [schemaVersion] changes. It lives in
/// the platform cache directory, which Android Auto Backup and iOS backups
/// exclude and the OS may evict.
class ChannelHeadCache {
  ChannelHeadCache(this._root, {this.maxChannels = maxChannelsPerScope});

  /// 2 added the room tail.
  static const schemaVersion = 2;

  /// Room for the inbox preload's rooms (up to 48 rows) next to visited ones.
  static const maxChannelsPerScope = 64;
  static const maxEntryBytes = 1024 * 1024;

  final Future<Directory> Function() _root;
  final int maxChannels;

  // File work runs one operation at a time, so a clear cannot interleave with
  // a write that would recreate the directory it just removed.
  Future<void> _tail = Future.value();
  int _clearGeneration = 0;

  Future<T> _serial<T>(Future<T> Function() task) {
    final result = _tail.then((_) => task());
    _tail = result.then<void>((_) {}, onError: (_) {});
    return result;
  }

  /// Counts clears; a write requested under an older count is dropped.
  int get clearGeneration => _clearGeneration;

  /// The stored head, or null when the room has none (a room known only from
  /// live events has a tail but no head).
  Future<List<NostrEvent>?> load(ChannelHeadScope scope, String channelId) =>
      _serial(() async {
        final path = _entryPath(await _scopeDir(scope), channelId);
        return _readInIsolate(path, channelId);
      });

  /// The room's stored tail, or null when nothing is stored for it.
  Future<RoomTail?> loadTail(ChannelHeadScope scope, String channelId) =>
      _serial(() async {
        final path = _entryPath(await _scopeDir(scope), channelId);
        return _readTailInIsolate(path, channelId);
      });

  /// Applies live [events] to the room's tail, keeping its head. Dropped when
  /// a clear was requested after [generation] was read.
  Future<void> applyLive(
    ChannelHeadScope scope,
    String channelId,
    List<RoomLiveEvent> events, {
    required int generation,
    bool markGap = false,
  }) => _serial(() async {
    if (generation != _clearGeneration) return;
    final dir = await _scopeDir(scope);
    await _applyLiveInIsolate(dir, channelId, events, markGap, maxChannels);
  });

  /// When [channelId]'s head was last written, or null when there is none or
  /// it predates this field. Reads do not change it (unlike the file's
  /// modification time, which marks recent use for eviction).
  Future<DateTime?> storedAt(ChannelHeadScope scope, String channelId) =>
      _serial(() async {
        final path = _entryPath(await _scopeDir(scope), channelId);
        return _readStoredAt(path);
      });

  /// Writes the relay's newest page and rebuilds the tail from it. Writes are
  /// dropped when a clear was requested after this call.
  Future<void> store(
    ChannelHeadScope scope,
    String channelId,
    List<NostrEvent> events,
  ) {
    final generation = _clearGeneration;
    return _serial(() async {
      if (generation != _clearGeneration) return;
      final dir = await _scopeDir(scope);
      await _writeInIsolate(dir, channelId, events, maxChannels);
    });
  }

  /// Removes every account's heads for [relayUrl].
  Future<void> clearRelay(String relayUrl) {
    _clearGeneration++;
    return _serial(() async {
      final dir = Directory(
        '${(await _root()).path}/${_hash(storageScopeFor(relayUrl))}',
      );
      if (await dir.exists()) await dir.delete(recursive: true);
    });
  }

  Future<void> clearAll() {
    _clearGeneration++;
    return _serial(() async {
      final dir = await _root();
      if (await dir.exists()) await dir.delete(recursive: true);
    });
  }

  Future<String> _scopeDir(ChannelHeadScope scope) async =>
      '${(await _root()).path}/${_hash(storageScopeFor(scope.relayUrl))}/'
      '${_hash(scope.pubkey)}';
}

// Top-level so the isolate closures capture only their arguments, not the
// cache instance and its pending futures.
Future<List<NostrEvent>?> _readInIsolate(String path, String channelId) =>
    Isolate.run(() => _readEntry(path, channelId));

Future<RoomTail?> _readTailInIsolate(String path, String channelId) =>
    Isolate.run(() => _readStored(path, channelId)?.tail);

Future<void> _applyLiveInIsolate(
  String dir,
  String channelId,
  List<RoomLiveEvent> events,
  bool markGap,
  int maxChannels,
) => Isolate.run(
  () => _applyLiveEntry(dir, channelId, events, markGap, maxChannels),
);

/// A stored entry; [events] stays encoded so tail writes pass it through.
typedef _StoredEntry = ({List<dynamic>? events, int? storedAt, RoomTail tail});

/// The entry at [path], or null when there is none or it cannot be used (an
/// older schema, another channel, a torn write).
_StoredEntry? _readStored(String path, String channelId) {
  final file = File(path);
  if (!file.existsSync()) return null;
  try {
    final decoded = jsonDecode(file.readAsStringSync());
    if (decoded is! Map<String, dynamic> ||
        decoded['v'] != ChannelHeadCache.schemaVersion ||
        decoded['channel'] != channelId) {
      return null;
    }
    final events = decoded['events'] as List<dynamic>?;
    return (
      events: events,
      storedAt: (decoded['stored_at'] as num?)?.toInt(),
      tail: RoomTail.fromJson(decoded, stored: _byId(events)),
    );
  } catch (_) {
    return null;
  }
}

Map<String, Map<String, dynamic>> _byId(List<dynamic>? events) => {
  for (final event in (events ?? const []).cast<Map<String, dynamic>>())
    if (event['id'] case final String id) id: event,
};

void _applyLiveEntry(
  String dir,
  String channelId,
  List<RoomLiveEvent> events,
  bool markGap,
  int maxChannels,
) {
  final stored = _readStored(_entryPath(dir, channelId), channelId);
  _writeEncodedEntry(
    dir,
    channelId,
    events: stored?.events,
    storedAt: stored?.storedAt,
    tail: applyLiveEvents(
      stored?.tail ?? RoomTail.empty,
      events,
      markGap: markGap,
    ),
    maxChannels: maxChannels,
  );
}

Future<void> _writeInIsolate(
  String dir,
  String channelId,
  List<NostrEvent> events,
  int maxChannels,
) => Isolate.run(() => _writeEntry(dir, channelId, events, maxChannels));

String _hash(String value) => sha256.convert(utf8.encode(value)).toString();

String _entryPath(String dir, String channelId) =>
    '$dir/${_hash(channelId)}.json';

List<NostrEvent>? _readEntry(String path, String channelId) {
  final file = File(path);
  if (!file.existsSync()) return null;
  try {
    final decoded = jsonDecode(file.readAsStringSync());
    if (decoded is! Map<String, dynamic> ||
        decoded['v'] != ChannelHeadCache.schemaVersion ||
        decoded['channel'] != channelId) {
      file.deleteSync();
      return null;
    }
    final head = decoded['events'] as List?;
    if (head == null) return null;
    final events = [
      for (final json in head)
        NostrEvent.fromJson(json as Map<String, dynamic>),
    ];
    // Mark the entry as recently used for eviction.
    file.setLastModifiedSync(DateTime.now());
    return events;
  } catch (_) {
    // A torn or corrupt entry is only a missed paint; drop it.
    try {
      file.deleteSync();
    } catch (_) {}
    return null;
  }
}

final _storedAtPattern = RegExp(r'"stored_at":(\d+)');

Future<DateTime?> _readStoredAt(String path) async {
  final RandomAccessFile file;
  try {
    file = await File(path).open();
  } on FileSystemException {
    return null;
  }
  try {
    final head = utf8.decode(await file.read(64), allowMalformed: true);
    final seconds = _storedAtPattern.firstMatch(head)?.group(1);
    return seconds == null
        ? null
        : DateTime.fromMillisecondsSinceEpoch(int.parse(seconds) * 1000);
  } on FileSystemException {
    return null;
  } finally {
    await file.close();
  }
}

void _writeEntry(
  String dir,
  String channelId,
  List<NostrEvent> events,
  int maxChannels,
) {
  final now = DateTime.now().millisecondsSinceEpoch ~/ 1000;
  final stored = _readStored(_entryPath(dir, channelId), channelId);
  _writeEncodedEntry(
    dir,
    channelId,
    events: [for (final event in events) event.toJson()],
    storedAt: now,
    tail: roomTailFromHead(events, stored?.tail ?? RoomTail.empty, now: now),
    maxChannels: maxChannels,
  );
}

/// Writes one entry; [storedAt] is when [events] (the head) was fetched, and
/// both are absent for a room known only from live events.
void _writeEncodedEntry(
  String dir,
  String channelId, {
  required List<dynamic>? events,
  required int? storedAt,
  required RoomTail tail,
  required int maxChannels,
}) {
  final directory = Directory(dir)..createSync(recursive: true);
  final path = _entryPath(dir, channelId);
  // `stored_at` stays near the start so [_readStoredAt] reads a few bytes.
  // Tail rows that are also head rows are stored once.
  String encode({required bool withHead}) => jsonEncode({
    'v': ChannelHeadCache.schemaVersion,
    if (withHead && storedAt != null) 'stored_at': storedAt,
    'channel': channelId,
    if (withHead && events != null) 'events': events,
    ...tail.toJson(storedIds: withHead ? _byId(events).keys.toSet() : const {}),
  });
  bool fits(String encoded) =>
      utf8.encode(encoded).length <= ChannelHeadCache.maxEntryBytes;
  var encoded = encode(withHead: true);
  // An oversized head is dropped rather than kept stale; the tail is smaller.
  if (!fits(encoded)) encoded = encode(withHead: false);
  if (!fits(encoded)) {
    final stale = File(path);
    if (stale.existsSync()) stale.deleteSync();
    return;
  }
  final temp = File('$path.tmp')..writeAsStringSync(encoded, flush: true);
  temp.renameSync(path);

  final entries = <(File, DateTime)>[];
  for (final file in directory.listSync().whereType<File>()) {
    try {
      if (file.path.endsWith('.json')) {
        entries.add((file, file.lastModifiedSync()));
      } else {
        // Operations are serialized, so any temp file is from an interrupted
        // write.
        file.deleteSync();
      }
    } on FileSystemException {
      // Already gone; nothing to evict.
    }
  }
  entries.sort((a, b) => b.$2.compareTo(a.$2));
  for (final (file, _) in entries.skip(maxChannels)) {
    file.deleteSync();
  }
}

final channelHeadCacheProvider = Provider<ChannelHeadCache>(
  (ref) => ChannelHeadCache(
    () async => Directory(
      '${(await getApplicationCacheDirectory()).path}/channel_heads',
    ),
  ),
);

/// The active account's head scope, or null when no identity is available.
final channelHeadScopeProvider = Provider<ChannelHeadScope?>((ref) {
  final pubkey = ref.watch(myPubkeyProvider);
  if (pubkey == null) return null;
  return ChannelHeadScope(
    relayUrl: ref.watch(relayConfigProvider).baseUrl,
    pubkey: pubkey,
  );
});
