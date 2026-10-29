/// Opening a room from the store: its newest window of top-level rows, by
/// the relay's channel-window rules, and whether that window is certain.
/// Mirrors desktop `message_store/read.rs`. Rules:
/// docs/message-sync.md#opening-a-room-from-the-store; shared cases:
/// test-fixtures/message-store-room-head.json.
library;

import 'dart:convert';

import 'package:sqflite/sqflite.dart';

/// Receive time can trail `created_at` by this much (relay future tolerance
/// plus storage fence), so coverage must begin this far before a row.
const rowReceiveMarginSecs = 900 + 960 + 5;

const _roomStream = 'room';

/// A room's newest window as the store has it.
class StoredRoomHead {
  StoredRoomHead({
    required this.certain,
    required this.rows,
    required this.aux,
    required this.summaries,
    required this.nextCursor,
  });

  /// Every event of the window is known stored and the room's live REQ is
  /// still receiving, so the window equals what the relay would send.
  final bool certain;
  final List<Map<String, dynamic>> rows;
  final List<Map<String, dynamic>> aux;
  final List<Map<String, dynamic>> summaries;

  /// `(created_at, id)` of the oldest row.
  final (int, String)? nextCursor;

  /// The window in the shape of a relay channel-window response: rows,
  /// overlays, summaries and one `39006` bounds event for [room]'s head.
  List<Map<String, dynamic>> toResponse(String room) {
    final cursor = nextCursor;
    return [
      ...rows,
      ...aux,
      ...summaries,
      {
        'id': '0' * 64,
        'pubkey': '0' * 64,
        'sig': '0' * 128,
        'kind': 39006,
        'created_at': 0,
        'tags': [
          ['d', '${room.toLowerCase()}:head'],
          ['h', room],
        ],
        'content': jsonEncode({
          'has_more': cursor != null,
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

/// The parent of a reply: its last `reply`-marked `e` tag.
String? _parentOf(Map<String, dynamic> event) {
  final tags = event['tags'];
  if (tags is! List) return null;
  for (final tag in tags.reversed) {
    if (tag is List &&
        tag.length > 3 &&
        tag[0] == 'e' &&
        tag[3] == 'reply' &&
        tag[1] is String) {
      return tag[1] as String;
    }
  }
  return null;
}

String _marks(int count) => List.filled(count, '?').join(', ');

Future<List<Map<String, dynamic>>> _overlays(
  DatabaseExecutor db,
  List<String> targets,
  String kinds,
) async {
  if (targets.isEmpty) return [];
  final rows = await db.rawQuery(
    'SELECT DISTINCT e.id, e.raw, e.created_at FROM targets t '
    'JOIN events e ON e.id = t.id '
    'WHERE t.target IN (${_marks(targets.length)}) AND e.kind IN ($kinds) '
    'ORDER BY e.created_at, e.id',
    targets,
  );
  return rows.map((row) => _parse(row['raw'])).nonNulls.toList();
}

/// [room]'s newest [limit] top-level rows of [kinds], with their overlays
/// and thread summaries. [liveAt] is when the room's live REQ reached EOSE
/// on the current connection, if it is live.
Future<StoredRoomHead> readRoomHead(
  DatabaseExecutor db, {
  required String room,
  required List<int> kinds,
  required int limit,
  required int? liveAt,
}) async {
  final rows = <Map<String, dynamic>>[];
  final rowIds = <String>[];
  (int, String)? nextCursor;
  if (kinds.isNotEmpty && limit > 0) {
    // Read in pages: deleted rows and deep replies are skipped as found.
    const page = 200;
    var offset = 0;
    while (rows.length < limit) {
      final candidates = await db.rawQuery(
        'SELECT id, pubkey, created_at, root, raw FROM events '
        'WHERE room = ? AND kind IN (${kinds.join(', ')}) '
        'AND (root IS NULL OR broadcast = 1) '
        'ORDER BY created_at DESC, id ASC LIMIT ? OFFSET ?',
        [room, page, offset],
      );
      offset += candidates.length;
      for (final candidate in candidates) {
        final id = candidate['id']! as String;
        final root = candidate['root'] as String?;
        final event = _parse(candidate['raw']);
        if (event == null) continue;
        // A broadcast reply is a row only as a direct reply to the root.
        if (root != null && _parentOf(event) != root) continue;
        // An admin removal (9005, accepted only when allowed) or the
        // author's own deletion (5) takes the row out.
        final deleted = await db.rawQuery(
          'SELECT 1 FROM targets t JOIN events e ON e.id = t.id '
          'WHERE t.target = ? '
          'AND (e.kind = 9005 OR (e.kind = 5 AND e.pubkey = ?)) LIMIT 1',
          [id, candidate['pubkey']],
        );
        if (deleted.isNotEmpty) continue;
        nextCursor = (candidate['created_at']! as int, id);
        rowIds.add(id);
        rows.add(event);
        if (rows.length == limit) break;
      }
      if (candidates.length < page) break;
    }
  }

  final aux = await _overlays(db, rowIds, '5, 7, 9005, 40003');
  final auxIds = [for (final event in aux) event['id'] as String? ?? ''];
  final seen = auxIds.toSet();
  for (final removal in await _overlays(db, auxIds, '5, 9005')) {
    if (seen.add(removal['id'] as String? ?? '')) aux.add(removal);
  }

  final summaries = rowIds.isEmpty
      ? <Map<String, dynamic>>[]
      : (await db.rawQuery(
          'SELECT raw FROM summaries WHERE root IN (${_marks(rowIds.length)}) '
          'ORDER BY root',
          rowIds,
        )).map((row) => _parse(row['raw'])).nonNulls.toList();

  var certain = false;
  final cursor = nextCursor;
  if (liveAt != null && cursor != null && rows.length == limit) {
    final covered = await db.rawQuery(
      'SELECT 1 FROM coverage WHERE room = ? AND stream = ? '
      'AND lo_at <= ? AND hi_at >= ? LIMIT 1',
      [room, _roomStream, cursor.$1 - rowReceiveMarginSecs, liveAt],
    );
    certain = covered.isNotEmpty;
  }
  return StoredRoomHead(
    certain: certain,
    rows: rows,
    aux: aux,
    summaries: summaries,
    nextCursor: nextCursor,
  );
}
