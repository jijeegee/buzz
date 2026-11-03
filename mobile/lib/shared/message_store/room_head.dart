/// Opening a room from the store: its newest window of top-level rows, by
/// the relay's channel-window rules, and whether that window is certain.
/// Mirrors desktop `message_store/read.rs`. Rules:
/// docs/message-sync.md#opening-a-room-from-the-store; shared cases:
/// test-fixtures/message-store-room-head.json.
library;

import 'dart:convert';

import 'package:sqflite/sqflite.dart';

import 'head_coverage.dart';

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
  /// [contiguous] adds `contiguous_from` to the bounds (see
  /// [storedRowsContiguousFrom]) for a window drawn before the relay's.
  List<Map<String, dynamic>> toResponse(
    String room, {
    ({int? from})? contiguous,
  }) {
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
          if (contiguous != null) 'contiguous_from': contiguous.from,
        }),
      },
    ];
  }
}

/// The ids among [candidates] (`id`, `pubkey` rows) that a stored deletion
/// removes: an admin removal (9005, accepted by the relay only when allowed)
/// or the author's own deletion (5). One query for the whole page.
Future<Set<String>> storedDeletedIds(
  DatabaseExecutor db,
  List<Map<String, Object?>> candidates,
) async {
  if (candidates.isEmpty) return {};
  final authors = {
    for (final candidate in candidates)
      candidate['id']! as String: candidate['pubkey']! as String,
  };
  final removals = await db.rawQuery(
    'SELECT t.target, e.kind, e.pubkey FROM targets t '
    'JOIN events e ON e.id = t.id '
    'WHERE t.target IN (${_marks(authors.length)}) AND e.kind IN (5, 9005)',
    authors.keys.toList(),
  );
  return {
    for (final removal in removals)
      if (removal['kind'] == 9005 ||
          removal['pubkey'] == authors[removal['target']])
        removal['target']! as String,
  };
}

/// Raw events for [ids], parsed, by id. One query.
Future<Map<String, Map<String, dynamic>>> storedEventsById(
  DatabaseExecutor db,
  List<String> ids,
) async {
  if (ids.isEmpty) return {};
  final rows = await db.rawQuery(
    'SELECT id, raw FROM events WHERE id IN (${_marks(ids.length)})',
    ids,
  );
  return {for (final row in rows) row['id']! as String: ?_parse(row['raw'])};
}

/// One stored event [id] of [room] for drawing, and whether a stored removal
/// (9005) or its author's deletion (5) targets it; `null` when it is not
/// stored. Rows only the push inbox wrote count: like a thread paint, the
/// relay's copy replaces them
/// (docs/message-sync.md#opening-a-thread-from-its-root-id).
Future<({Map<String, dynamic> event, bool deleted})?> readPaintableEvent(
  DatabaseExecutor db, {
  required String room,
  required String id,
}) async {
  final rows = await db.rawQuery(
    'SELECT id, pubkey, raw FROM events WHERE id = ? AND room = ?',
    [id, room],
  );
  if (rows.isEmpty) return null;
  final event = _parse(rows.single['raw']);
  if (event == null) return null;
  final deleted = await storedDeletedIds(db, rows);
  return (event: event, deleted: deleted.contains(id));
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
String? storedParentOf(Map<String, dynamic> event) => _parentOf(event);

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

/// Whether a thread summary (39005) counts at least one reply: the relay
/// sends no summary for a thread without replies.
bool _hasReplies(Map<String, dynamic> summary) {
  final content = _parse(summary['content']);
  final count = content?['reply_count'];
  return count is int && count > 0;
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
/// on the current connection, if it is live. [includePushed] (a paint only)
/// also takes rows only the push inbox wrote, and counts pushed replies
/// newer than a row's stored summary into it; the relay's window replaces
/// both, and nothing is written.
Future<StoredRoomHead> readRoomHead(
  DatabaseExecutor db, {
  required String room,
  required List<int> kinds,
  required int limit,
  required int? liveAt,
  bool includePushed = false,
}) async {
  final rows = <Map<String, dynamic>>[];
  final rowIds = <String>[];
  (int, String)? nextCursor;
  if (kinds.isNotEmpty && limit > 0) {
    // Read in pages: deleted rows and deep replies are skipped as found.
    const page = 200;
    var offset = 0;
    while (rows.length < limit) {
      // Ids first; raw only for the rows kept.
      final candidates = await db.rawQuery(
        'SELECT id, pubkey, created_at, root FROM events '
        'WHERE room = ? AND kind IN (${kinds.join(', ')}) '
        'AND (root IS NULL OR broadcast = 1) '
        // Rows only the push inbox wrote claim no coverage: a certain window
        // leaves them out; a paint draws them until the relay's copy.
        '${includePushed ? '' : 'AND id NOT IN (SELECT id FROM pushed_rows) '}'
        'ORDER BY created_at DESC, id ASC LIMIT ? OFFSET ?',
        [room, page, offset],
      );
      offset += candidates.length;
      final deleted = await storedDeletedIds(db, candidates);
      final kept = [
        for (final candidate in candidates)
          if (!deleted.contains(candidate['id'])) candidate,
      ];
      var next = 0;
      while (next < kept.length && rows.length < limit) {
        final chunk = kept
            .skip(next)
            .take(limit - rows.length)
            .toList(growable: false);
        next += chunk.length;
        final events = await storedEventsById(db, [
          for (final candidate in chunk) candidate['id']! as String,
        ]);
        for (final candidate in chunk) {
          final id = candidate['id']! as String;
          final root = candidate['root'] as String?;
          final event = events[id];
          if (event == null) continue;
          // A broadcast reply is a row only as a direct reply to the root.
          if (root != null && _parentOf(event) != root) continue;
          nextCursor = (candidate['created_at']! as int, id);
          rowIds.add(id);
          rows.add(event);
        }
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

  var stored = rowIds.isEmpty
      ? <Map<String, dynamic>>[]
      : (await db.rawQuery(
          'SELECT raw FROM summaries WHERE root IN (${_marks(rowIds.length)}) '
          'ORDER BY root',
          rowIds,
        )).map((row) => _parse(row['raw'])).nonNulls.toList();
  if (includePushed && rowIds.isNotEmpty) {
    stored = await _withPushedReplies(db, room, rowIds, stored);
  }
  final summaries = stored.where(_hasReplies).toList();

  var certain = false;
  final cursor = nextCursor;
  final full = rows.length == limit;
  if (liveAt != null) {
    final received =
        cursor != null &&
        full &&
        (await db.rawQuery(
          'SELECT 1 FROM coverage WHERE room = ? AND stream = ? '
          'AND lo_at <= ? AND hi_at >= ? LIMIT 1',
          [room, _roomStream, cursor.$1 - rowReceiveMarginSecs, liveAt],
        )).isNotEmpty;
    // Or a head span reaches the window's oldest row
    // (docs/message-sync.md#head-spans).
    final covered =
        received ||
        await storeHeadVouches(
          db,
          room: room,
          stream: headStream,
          oldest: cursor,
          full: full,
          liveAt: liveAt,
        );
    // Job rows (43001-43006) show in the window but no live REQ carries
    // them, so coverage never vouches for them: a room that ever stored one
    // is opened from the relay.
    final jobRows = await db.rawQuery(
      'SELECT 1 FROM events WHERE room = ? '
      'AND kind BETWEEN 43001 AND 43006 LIMIT 1',
      [room],
    );
    certain = covered && jobRows.isEmpty;
  }
  return StoredRoomHead(
    certain: certain,
    rows: rows,
    aux: aux,
    summaries: summaries,
    nextCursor: nextCursor,
  );
}

/// [summaries] of [roots] with the replies only the push inbox wrote counted
/// in: each pushed reply newer than a root's `last_reply_at` adds to
/// `descendant_count`, and to `reply_count` when it replies to the root
/// itself. A root with no summary gets one. Deleted replies do not count.
Future<List<Map<String, dynamic>>> _withPushedReplies(
  DatabaseExecutor db,
  String room,
  List<String> roots,
  List<Map<String, dynamic>> summaries,
) async {
  final candidates = await db.rawQuery(
    'SELECT id, pubkey, created_at, root FROM events '
    'WHERE room = ? AND root IN (${_marks(roots.length)}) '
    'AND kind IN (9, 40002) AND id IN (SELECT id FROM pushed_rows) '
    'ORDER BY created_at, id',
    [room, ...roots],
  );
  if (candidates.isEmpty) return summaries;
  final deleted = await storedDeletedIds(db, candidates);
  final events = await storedEventsById(db, [
    for (final candidate in candidates)
      if (!deleted.contains(candidate['id'])) candidate['id']! as String,
  ]);
  final byRoot = <String, Map<String, dynamic>>{
    for (final summary in summaries)
      ?_summaryRoot(summary): summary,
  };
  for (final root in roots) {
    final summary = byRoot[root];
    final content = summary == null ? null : _parse(summary['content']);
    if (summary != null && content == null) continue;
    final lastReplyAt = content?['last_reply_at'];
    final newer = [
      for (final candidate in candidates)
        if (candidate['root'] == root &&
            (lastReplyAt is! int ||
                (candidate['created_at']! as int) > lastReplyAt))
          ?events[candidate['id']],
    ];
    if (newer.isEmpty) continue;
    int count(String key) => switch (content?[key]) {
      final int value => value,
      _ => 0,
    };
    final participants = <String>[
      ...?(content?['participants'] as List?)?.whereType<String>(),
    ];
    for (final reply in newer) {
      final author = reply['pubkey'];
      if (author is String && !participants.contains(author)) {
        participants.add(author);
      }
    }
    final newest = newer.last['created_at']! as int;
    final createdAt = summary?['created_at'];
    byRoot[root] = {
      'id': summary?['id'] ?? '0' * 64,
      'pubkey': summary?['pubkey'] ?? '0' * 64,
      'sig': summary?['sig'] ?? '0' * 128,
      'kind': 39005,
      'created_at': createdAt is int && createdAt > newest ? createdAt : newest,
      'tags':
          summary?['tags'] ??
          [
            ['e', root],
            ['d', root],
            ['h', room],
          ],
      'content': jsonEncode({
        ...?content,
        'reply_count':
            count('reply_count') +
            newer.where((reply) => _parentOf(reply) == root).length,
        'descendant_count': count('descendant_count') + newer.length,
        'last_reply_at': newest,
        'participants': participants,
      }),
    };
  }
  return byRoot.values.toList();
}

String? _summaryRoot(Map<String, dynamic> summary) {
  final tags = summary['tags'];
  if (tags is! List) return null;
  for (final tag in tags) {
    if (tag is List && tag.length > 1 && tag[0] == 'e' && tag[1] is String) {
      return tag[1] as String;
    }
  }
  return null;
}

/// The oldest `created_at` from which [room]'s stored rows run without a gap
/// up to [newest] (the newest stored row's): a room span alone, or a head
/// span (docs/message-sync.md#head-spans) reaching [newest] itself or by a
/// room span. `null` when no span vouches for any of them.
Future<int?> storedRowsContiguousFrom(
  DatabaseExecutor db, {
  required String room,
  required int newest,
}) async {
  final spans = await db.rawQuery(
    'SELECT stream, lo_at, lo_id, hi_at FROM coverage '
    'WHERE room = ? AND stream IN (?, ?)',
    [room, _roomStream, headStream],
  );
  final roomSpans = [
    for (final span in spans)
      if (span['stream'] == _roomStream)
        (span['lo_at']! as int, span['hi_at']! as int),
  ];
  int? from;
  void take(int at) => from = from == null || at < from! ? at : from;
  for (final (lo, hi) in roomSpans) {
    if (hi >= newest) take(lo + rowReceiveMarginSecs);
  }
  for (final span in spans) {
    if (span['stream'] != headStream) continue;
    final hi = span['hi_at']! as int;
    if (hi < newest && !roomSpans.any((r) => r.$1 <= hi && r.$2 >= newest)) {
      continue;
    }
    // Rows of the oldest row's second with a greater id may be missing.
    final loId = span['lo_id']! as String;
    take(loId.isEmpty ? 0 : (span['lo_at']! as int) + 1);
  }
  return from;
}
