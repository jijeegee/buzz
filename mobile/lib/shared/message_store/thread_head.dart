/// Opening a thread from the store: its newest replies by the relay's
/// thread-window rules, and whether that window is certain. Mirrors desktop
/// `message_store/read_thread.rs`. Rules:
/// docs/message-sync.md#opening-a-thread-from-the-store; shared cases:
/// test-fixtures/message-store-thread-head.json.
library;

import 'dart:convert';

import 'package:sqflite/sqflite.dart';

import 'room_head.dart';

/// A thread's newest replies as the store has them.
class StoredThreadHead {
  StoredThreadHead({
    required this.certain,
    required this.rows,
    required this.hasMore,
    required this.nextCursor,
  });

  /// Every reply of the window is known stored and the room's live REQ is
  /// still receiving, so the window equals what the relay would send.
  final bool certain;

  /// Newest first.
  final List<Map<String, dynamic>> rows;

  /// Older replies may exist: the window is full.
  final bool hasMore;

  /// `(created_at, id)` of the oldest reply, when [hasMore].
  final (int, String)? nextCursor;

  /// The window in the shape of a relay thread-window response: the rows
  /// and one `39007` bounds event for [room]'s thread [root].
  List<Map<String, dynamic>> toResponse(String room, String root) {
    final cursor = nextCursor;
    return [
      ...rows,
      {
        'id': '0' * 64,
        'pubkey': '0' * 64,
        'sig': '0' * 128,
        'kind': 39007,
        'created_at': 0,
        'tags': [
          ['h', room],
          ['e', root],
        ],
        'content': jsonEncode({
          'version': 1,
          'direction': 'older',
          'has_more': hasMore,
          'next_cursor': cursor == null
              ? null
              : {'created_at': cursor.$1, 'id': cursor.$2},
        }),
      },
    ];
  }
}

Map<String, dynamic>? _parse(Object? raw) {
  if (raw is! String) return null;
  try {
    final decoded = jsonDecode(raw);
    return decoded is Map<String, dynamic> ? decoded : null;
  } catch (_) {
    return null;
  }
}

Future<bool> _covered(
  DatabaseExecutor db,
  String room,
  int from,
  int liveAt,
) async => (await db.rawQuery(
  'SELECT 1 FROM coverage WHERE room = ? AND stream = ? '
  'AND lo_at <= ? AND hi_at >= ? LIMIT 1',
  [room, 'room', from, liveAt],
)).isNotEmpty;

/// The newest [limit] replies of [kinds] at any depth under [root] in
/// [room]. [liveAt] is when the room's live REQ reached EOSE on the current
/// connection, if it is live.
Future<StoredThreadHead> readThreadHead(
  DatabaseExecutor db, {
  required String room,
  required String root,
  required List<int> kinds,
  required int limit,
  required int? liveAt,
}) async {
  final rows = <Map<String, dynamic>>[];
  (int, String)? oldest;
  var hasMore = false;
  if (kinds.isNotEmpty && limit > 0) {
    // Read in pages: deleted replies are skipped as found.
    const page = 200;
    var offset = 0;
    while (!hasMore) {
      final candidates = await db.rawQuery(
        'SELECT id, pubkey, created_at, raw FROM events '
        'WHERE room = ? AND root = ? AND kind IN (${kinds.join(', ')}) '
        'ORDER BY created_at DESC, id ASC LIMIT ? OFFSET ?',
        [room, root, page, offset],
      );
      offset += candidates.length;
      for (final candidate in candidates) {
        final id = candidate['id']! as String;
        final event = _parse(candidate['raw']);
        if (event == null) continue;
        if (await isStoredDeleted(db, id, candidate['pubkey']! as String)) {
          continue;
        }
        oldest = (candidate['created_at']! as int, id);
        rows.add(event);
        if (rows.length == limit) {
          hasMore = true;
          break;
        }
      }
      if (candidates.length < page) break;
    }
  }

  var certain = false;
  if (liveAt != null) {
    if (hasMore) {
      certain = await _covered(
        db,
        room,
        oldest!.$1 - rowReceiveMarginSecs,
        liveAt,
      );
    } else {
      // Fewer replies than asked: only certain when every reply since the
      // root is known stored.
      final rootRow = await db.rawQuery(
        'SELECT created_at FROM events WHERE id = ? AND room = ?',
        [root, room],
      );
      if (rootRow.isNotEmpty) {
        certain = await _covered(
          db,
          room,
          (rootRow.single['created_at']! as int) - rowReceiveMarginSecs,
          liveAt,
        );
      }
    }
  }
  return StoredThreadHead(
    certain: certain,
    rows: rows,
    hasMore: hasMore,
    nextCursor: hasMore ? oldest : null,
  );
}
