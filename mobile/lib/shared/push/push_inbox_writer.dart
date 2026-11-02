import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'push_inbox.dart' show isPushInboxToken, writePushInboxCursor;

/// The writing side of the push inbox (docs/message-sync.md#push-inbox):
/// the Android background push handler keeps what it fetched, in the same
/// format the iPhone notification extension writes (KuzzPushKit
/// `PushInbox.swift`), for the app's drain to take into its store.
///
///     <root>/<token>/<unixms>-<uuid>.json   one fetch
///     <root>/<token>/cursor                 newest created_at fetched
///
/// Files are written to a `.tmp` name and renamed, so readers only see
/// whole files.
class PushInboxLimits {
  final int maxAgeMilliseconds;
  final int maxEvents;
  final int maxBytes;

  const PushInboxLimits({
    required this.maxAgeMilliseconds,
    required this.maxEvents,
    required this.maxBytes,
  });

  /// 7 days, 500 events, 2 MB per token.
  static const standard = PushInboxLimits(
    maxAgeMilliseconds: 7 * 86400000,
    maxEvents: 500,
    maxBytes: 2097152,
  );
}

class PushInboxWriter {
  static const fileVersion = 1;

  /// The `push-inbox` folder.
  final String root;
  final PushInboxLimits limits;
  final DateTime Function() _now;
  final String Function() _makeId;

  PushInboxWriter(
    this.root, {
    this.limits = PushInboxLimits.standard,
    DateTime Function()? now,
    String Function()? makeId,
  }) : _now = now ?? DateTime.now,
       _makeId = makeId ?? _randomId;

  String? folder(String token) =>
      isPushInboxToken(token) ? '$root/$token' : null;

  /// Keeps one fetch and prunes the token's folder. Returns the new file,
  /// or null when there was nothing to keep.
  Future<File?> write({
    required String token,
    required String relayUrl,
    required String account,
    required int fetchedAt,
    required List<Map<String, dynamic>> events,
  }) async {
    final path = folder(token);
    if (events.isEmpty || path == null) return null;
    await Directory(path).create(recursive: true);
    final data = utf8.encode(
      jsonEncode({
        'v': fileVersion,
        'relayUrl': relayUrl,
        'account': account,
        'fetchedAt': fetchedAt,
        'events': [for (final event in events) _eventJson(event)],
      }),
    );
    final target = File(
      '$path/${_now().millisecondsSinceEpoch}-${_makeId()}.json',
    );
    final temporary = File('${target.path}.${_makeId()}.tmp');
    try {
      await temporary.writeAsBytes(data, flush: true);
      await temporary.rename(target.path);
    } catch (_) {
      if (await temporary.exists()) await temporary.delete();
      rethrow;
    }
    await prune(token);
    return target;
  }

  /// The token's fetch cursor (unix seconds), or null when absent or damaged.
  Future<int?> readCursor(String token) async {
    final path = folder(token);
    if (path == null) return null;
    try {
      return int.tryParse((await File('$path/cursor').readAsString()).trim());
    } catch (_) {
      return null;
    }
  }

  /// Replaces the token's cursor atomically; the last writer wins.
  Future<void> writeCursor(String token, int value) =>
      writePushInboxCursor(root, token, value);

  /// Drops files older than the age limit, then the oldest files while the
  /// folder holds more events or bytes than allowed. `.tmp` files and the
  /// cursor are neither counted nor removed.
  Future<void> prune(String token) async {
    final path = folder(token);
    if (path == null) return;
    final directory = Directory(path);
    if (!await directory.exists()) return;
    final entries = <({int millis, String name})>[];
    await for (final entity in directory.list(followLinks: false)) {
      if (entity is! File) continue;
      final name = entity.uri.pathSegments.last;
      final millis = fetchMillis(name);
      if (millis != null) entries.add((millis: millis, name: name));
    }
    entries.sort(
      (a, b) => a.millis == b.millis
          ? a.name.compareTo(b.name)
          : a.millis.compareTo(b.millis),
    );

    final nowMillis = _now().millisecondsSinceEpoch;
    while (entries.isNotEmpty &&
        nowMillis - entries.first.millis > limits.maxAgeMilliseconds) {
      await _delete('$path/${entries.removeAt(0).name}');
    }

    final sizes = <({int events, int bytes})>[];
    for (final entry in entries) {
      try {
        final data = await File('$path/${entry.name}').readAsBytes();
        sizes.add((events: eventCount(data), bytes: data.length));
      } catch (_) {
        sizes.add((events: 0, bytes: 0));
      }
    }
    var events = sizes.fold<int>(0, (sum, size) => sum + size.events);
    var bytes = sizes.fold<int>(0, (sum, size) => sum + size.bytes);
    while (entries.isNotEmpty &&
        (events > limits.maxEvents || bytes > limits.maxBytes)) {
      await _delete('$path/${entries.removeAt(0).name}');
      final size = sizes.removeAt(0);
      events -= size.events;
      bytes -= size.bytes;
    }
  }

  /// `<unixms>` of a finished fetch file name, or null for anything else.
  static int? fetchMillis(String name) {
    if (!name.endsWith('.json')) return null;
    final dash = name.indexOf('-');
    if (dash <= 0) return null;
    return int.tryParse(name.substring(0, dash));
  }

  static int eventCount(List<int> data) {
    try {
      final object = jsonDecode(utf8.decode(data));
      final events = object is Map ? object['events'] : null;
      return events is List ? events.length : 0;
    } catch (_) {
      return 0;
    }
  }

  static Future<void> _delete(String path) async {
    try {
      await File(path).delete();
    } catch (_) {
      // Already gone; the next prune sees the folder as it is.
    }
  }

  static Map<String, dynamic> _eventJson(Map<String, dynamic> event) => {
    'id': event['id'],
    'pubkey': event['pubkey'],
    'created_at': event['created_at'],
    'kind': event['kind'],
    'tags': event['tags'],
    'content': event['content'],
    'sig': event['sig'],
  };

  static String _randomId() {
    final random = Random.secure();
    return List.generate(
      16,
      (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
    ).join();
  }
}

/// The fetch window and cursor rules (fixture section `cursor`; Swift
/// `PushInboxCursor`).
abstract final class PushInboxCursor {
  /// Rows can land on the relay up to this long after their `created_at`.
  static const lookbackSeconds = 1865;

  /// A fetch never reaches further back than this.
  static const windowSeconds = 259200;

  static int since({required int? cursor, required int now}) {
    final floor = now - windowSeconds;
    if (cursor == null) return floor;
    return max(cursor - lookbackSeconds, floor);
  }

  /// The cursor after a fetch. A truncated answer still advances to the
  /// newest returned row: pushed rows claim no completeness and the app's
  /// catch-up fills gaps. The notification fallback query never moves it.
  static int? next({
    required int? current,
    required List<int> returned,
    required bool fallback,
  }) {
    if (fallback || returned.isEmpty) return current;
    return returned.reduce(max);
  }
}
