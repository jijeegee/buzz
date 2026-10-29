/// Head coverage: a newest window the relay answered while a live REQ
/// carrying its room was receiving holds every row of that window from its
/// oldest row on, and the live REQ has every row since. Mirrors desktop
/// `message_store/head.rs`. Rules: docs/message-sync.md#head-spans; shared
/// cases: test-fixtures/message-store-head-coverage.json.
library;

import 'dart:convert';

import 'package:sqflite/sqflite.dart';

import 'room_head.dart' show storedParentOf;

/// The stream a room window's head span is recorded under.
const headStream = 'head';

/// The stream thread [root]'s head span is recorded under.
String threadHeadStream(String root) => '$headStream:$root';

const _roomStream = 'room';

/// A newest window the relay answered under a live token.
class StoreHeadSpan {
  const StoreHeadSpan({
    required this.room,
    required this.root,
    required this.kinds,
    required this.liveAt,
    required this.oldest,
    required this.ids,
  });

  final String room;

  /// `null`: the room's window; else the window of this thread root.
  final String? root;

  /// The window's row kinds.
  final List<int> kinds;

  /// When the live REQ carrying the room reached EOSE (the token's L).
  final int liveAt;

  /// `(created_at, id)` of the answer's oldest row; `null` when the answer
  /// had no more (the whole history).
  final (int, String)? oldest;

  /// Every event id the answer held.
  final Set<String> ids;

  String get stream => root == null ? headStream : threadHeadStream(root!);
}

/// Window requests marked as they went out and not yet answered, at most.
const _maxMarks = 64;

/// Rows a mark remembers; beyond this its answer records nothing.
const _maxMarkRows = 4096;

/// A mark not answered within this is dropped: its answer records nothing.
const _markTtl = Duration(minutes: 2);

class _Mark {
  _Mark(this.rooms, this.opened);

  final Set<String> rooms;
  final DateTime opened;
  final Set<String> written = {};
  bool overflow = false;
}

/// What a closed mark vouches for: its rooms, and the ids of their rows
/// written since it.
typedef StoreHeadClosed = ({Set<String> rooms, Set<String> fresh});

/// Newest-window requests in flight, by the sequence number of the mark
/// queued as each went out, with the ids of their rooms' rows written since:
/// those may have reached the relay after its answer was read, so the answer
/// lacking them proves nothing.
class StoreHeadMarks {
  StoreHeadMarks({DateTime Function()? now}) : _now = now ?? DateTime.now;

  final DateTime Function() _now;
  final Map<int, _Mark> _open = {};

  bool _expired(_Mark mark) => _now().difference(mark.opened) >= _markTtl;

  void open(int at, Iterable<String> rooms) {
    _open.removeWhere((_, mark) => _expired(mark));
    if (_open.length >= _maxMarks) {
      _open.remove(_open.keys.reduce((a, b) => a < b ? a : b));
    }
    _open[at] = _Mark(rooms.toSet(), _now());
  }

  /// Row [id] of [room] is being written.
  void note(String room, String id) {
    for (final mark in _open.values) {
      if (!mark.rooms.contains(room)) continue;
      if (mark.written.length >= _maxMarkRows) {
        mark.overflow = true;
      } else {
        mark.written.add(id);
      }
    }
  }

  /// Closes mark [at], or `null` when it is unknown (dropped, or a plain
  /// capture), expired or overflowed.
  StoreHeadClosed? close(int at) {
    final mark = _open.remove(at);
    if (mark == null || mark.overflow || _expired(mark)) return null;
    return (rooms: mark.rooms, fresh: mark.written);
  }
}

/// Drops every head span of [room] (`head` and `head:<root>`): after a
/// history change the relay may show rows a head said were not there.
Future<void> forgetStoreHeads(DatabaseExecutor db, String room) => db.rawDelete(
  'DELETE FROM coverage WHERE room = ? AND (stream = ? OR stream LIKE ?)',
  [room, headStream, '$headStream:%'],
);

/// Records [head] in transaction [txn]. The stored rows of its window from
/// its oldest row on (every one, for a whole history) up to `liveAt` that
/// the answer lacks were deleted on the relay and are dropped, unless
/// written since the request went out ([fresh]). Then the stream's span
/// becomes `[oldest, liveAt]` (`lo_at` 0 and an empty `lo_id` for a whole
/// history), unless the span already there reaches further back and is
/// joined to `liveAt` by a room span: that one keeps its `lo`.
Future<void> recordStoreHead(
  Transaction txn,
  StoreHeadSpan head,
  Set<String> fresh,
) async {
  if (head.kinds.isEmpty) return;
  final kinds = head.kinds.join(', ');
  final oldest = head.oldest;
  // Relay order is `created_at DESC, id ASC`.
  final newer = oldest == null
      ? 'created_at <= ?'
      : '(created_at > ? OR (created_at = ? AND id <= ?)) AND created_at <= ?';
  final bounds = oldest == null
      ? <Object>[head.liveAt]
      : [oldest.$1, oldest.$1, oldest.$2, head.liveAt];
  final root = head.root;
  final candidates = root == null
      ? await txn.rawQuery(
          'SELECT id, root, raw FROM events WHERE room = ? AND kind IN ($kinds) '
          'AND (root IS NULL OR broadcast = 1) AND $newer',
          [head.room, ...bounds],
        )
      : await txn.rawQuery(
          'SELECT id, NULL AS root, NULL AS raw FROM events WHERE room = ? '
          'AND kind IN ($kinds) AND root = ? AND $newer',
          [head.room, root, ...bounds],
        );
  final doomed = <String>[];
  for (final candidate in candidates) {
    final id = candidate['id']! as String;
    if (head.ids.contains(id) || fresh.contains(id)) continue;
    // A broadcast reply is a window row only as a direct reply.
    final replyTo = candidate['root'] as String?;
    if (replyTo != null) {
      final event = _parse(candidate['raw']);
      if (event == null || storedParentOf(event) != replyTo) continue;
    }
    doomed.add(id);
  }
  final span =
      await _widerSpan(txn, head) ??
      (oldest?.$1 ?? 0, oldest?.$2 ?? '', head.liveAt);
  final batch = txn.batch();
  for (final id in doomed) {
    batch.rawDelete('DELETE FROM events WHERE id = ?', [id]);
    batch.rawDelete('DELETE FROM targets WHERE id = ?', [id]);
    batch.rawDelete('DELETE FROM summaries WHERE root = ?', [id]);
  }
  batch.rawDelete('DELETE FROM coverage WHERE room = ? AND stream = ?', [
    head.room,
    head.stream,
  ]);
  batch.rawInsert(
    'INSERT INTO coverage(room, stream, lo_at, lo_id, hi_at, hi_id) '
    "VALUES (?, ?, ?, ?, ?, '')",
    [head.room, head.stream, span.$1, span.$2, span.$3],
  );
  await batch.commit(noResult: true);
}

/// The span already recorded for [head]'s stream, widened to its L, when it
/// reaches further back than [head] (or is the whole history while [head]
/// is not) and one room span joins its end to [head]'s L.
Future<(int, String, int)?> _widerSpan(
  DatabaseExecutor db,
  StoreHeadSpan head,
) async {
  final oldest = head.oldest;
  if (oldest == null) return null;
  final rows = await db.rawQuery(
    'SELECT lo_at, lo_id, hi_at FROM coverage WHERE room = ? AND stream = ?',
    [head.room, head.stream],
  );
  if (rows.isEmpty) return null;
  final loAt = rows.first['lo_at']! as int;
  final loId = rows.first['lo_id']! as String;
  final hiAt = rows.first['hi_at']! as int;
  // Relay order is `created_at DESC, id ASC`: further back is older, or the
  // same second with a greater id.
  final further =
      loId.isEmpty ||
      loAt < oldest.$1 ||
      (loAt == oldest.$1 && loId.compareTo(oldest.$2) > 0);
  if (!further) return null;
  final joined = await db.rawQuery(
    'SELECT 1 FROM coverage WHERE room = ? AND stream = ? '
    'AND lo_at <= ? AND hi_at >= ? LIMIT 1',
    [head.room, _roomStream, hiAt, head.liveAt],
  );
  if (joined.isEmpty) return null;
  return (loAt, loId, hiAt > head.liveAt ? hiAt : head.liveAt);
}

/// Whether a head span of [stream] vouches for a window whose oldest row is
/// [oldest] ([full]: the window holds as many rows as asked) while the live
/// REQ carrying the room reached EOSE at [liveAt]: the span reaches that row
/// (or is the whole history), and one room span begun at or before the
/// head's L runs to [liveAt].
Future<bool> storeHeadVouches(
  DatabaseExecutor db, {
  required String room,
  required String stream,
  required (int, String)? oldest,
  required bool full,
  required int liveAt,
}) async {
  final heads = await db.rawQuery(
    'SELECT lo_at, lo_id, hi_at FROM coverage WHERE room = ? AND stream = ?',
    [room, stream],
  );
  for (final head in heads) {
    final loAt = head['lo_at']! as int;
    final loId = head['lo_id']! as String;
    final headAt = head['hi_at']! as int;
    // The head holds every row at or before its oldest one in relay order.
    final reaches =
        loId.isEmpty ||
        (full &&
            oldest != null &&
            (oldest.$1 > loAt ||
                (oldest.$1 == loAt && oldest.$2.compareTo(loId) <= 0)));
    if (!reaches) continue;
    final joined = await db.rawQuery(
      'SELECT 1 FROM coverage WHERE room = ? AND stream = ? '
      'AND lo_at <= ? AND hi_at >= ? LIMIT 1',
      [room, _roomStream, headAt, liveAt],
    );
    if (joined.isNotEmpty) return true;
  }
  return false;
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
