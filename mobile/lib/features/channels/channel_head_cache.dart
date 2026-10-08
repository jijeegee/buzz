import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:path_provider/path_provider.dart';

import '../../shared/relay/relay.dart';

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
/// paint a channel before the relay answers.
///
/// This is a paint accelerator only: the relay's response always replaces the
/// cached page. The store is fully derived from relay events, so it is
/// discarded rather than migrated when [schemaVersion] changes. It lives in
/// the platform cache directory, which Android Auto Backup and iOS backups
/// exclude and the OS may evict.
class ChannelHeadCache {
  ChannelHeadCache(this._root, {this.maxChannels = maxChannelsPerScope});

  static const schemaVersion = 1;
  static const maxChannelsPerScope = 32;
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

  Future<List<NostrEvent>?> load(ChannelHeadScope scope, String channelId) =>
      _serial(() async {
        final path = _entryPath(await _scopeDir(scope), channelId);
        return _readInIsolate(path, channelId);
      });

  /// Writes are dropped when a clear was requested after this call.
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
      final dir = Directory('${(await _root()).path}/${_hash(relayUrl)}');
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
      '${(await _root()).path}/${_hash(scope.relayUrl)}/${_hash(scope.pubkey)}';
}

// Top-level so the isolate closures capture only their arguments, not the
// cache instance and its pending futures.
Future<List<NostrEvent>?> _readInIsolate(String path, String channelId) =>
    Isolate.run(() => _readEntry(path, channelId));

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
    final events = [
      for (final json in decoded['events'] as List)
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

void _writeEntry(
  String dir,
  String channelId,
  List<NostrEvent> events,
  int maxChannels,
) {
  final directory = Directory(dir)..createSync(recursive: true);
  final path = _entryPath(dir, channelId);
  final encoded = jsonEncode({
    'v': ChannelHeadCache.schemaVersion,
    'channel': channelId,
    'events': [for (final event in events) event.toJson()],
  });
  if (utf8.encode(encoded).length > ChannelHeadCache.maxEntryBytes) {
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
