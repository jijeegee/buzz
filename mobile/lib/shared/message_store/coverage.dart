/// Coverage: the spans of time over which every event of a room is in the
/// store. Rules and shared cases: docs/message-sync.md#coverage,
/// test-fixtures/message-store-coverage.json.
library;

import 'package:sqflite/sqflite.dart';

/// The stream a room's own timeline is recorded under.
const _roomStream = 'room';

/// What a span promises. Raised when that changes (2: job rows, clock check,
/// scope pinning); spans an older build wrote are then dropped. Mirrors
/// desktop `COVERAGE_VERSION`.
const coverageVersion = 2;

/// Drops spans written under another [coverageVersion] (on open).
Future<void> dropStaleCoverage(Database db) async {
  final rows = await db.rawQuery(
    "SELECT value FROM sync_state WHERE key = 'coverage_version'",
  );
  if (rows.isNotEmpty && rows.first['value'] == '$coverageVersion') return;
  await db.transaction((txn) async {
    await txn.rawDelete('DELETE FROM coverage');
    await txn.rawDelete('DELETE FROM room_seq');
    await txn.rawInsert(
      'INSERT OR REPLACE INTO sync_state(key, value) '
      "VALUES ('coverage_version', ?)",
      ['$coverageVersion'],
    );
  });
}

/// [segments] (sorted by lo) after the span `[from, to]` is known complete:
/// segments it overlaps or touches merge into it.
List<(int, int)> mergeCoverage(List<(int, int)> segments, int from, int to) {
  if (from > to) return List.of(segments);
  var lo = from;
  var hi = to;
  final merged = <(int, int)>[];
  for (final (segLo, segHi) in segments) {
    if (segHi < lo || segLo > hi) {
      merged.add((segLo, segHi));
    } else {
      if (segLo < lo) lo = segLo;
      if (segHi > hi) hi = segHi;
    }
  }
  merged.add((lo, hi));
  merged.sort(
    (a, b) => a.$1 != b.$1 ? a.$1.compareTo(b.$1) : a.$2.compareTo(b.$2),
  );
  return merged;
}

/// Rooms one `IN (...)` read names at most (SQLite's variable limit).
const _roomsPerRead = 500;

/// [rooms] in parts of [_roomsPerRead], with the `IN` placeholders of each.
Iterable<(List<String>, String)> _roomParts(Iterable<String> rooms) sync* {
  final all = rooms.toSet().toList();
  for (var i = 0; i < all.length; i += _roomsPerRead) {
    final end = i + _roomsPerRead < all.length ? i + _roomsPerRead : all.length;
    final part = all.sublist(i, end);
    yield (part, List.filled(part.length, '?').join(', '));
  }
}

/// Every room span of [rooms], sorted by lo: one read per 500 rooms.
Future<Map<String, List<(int, int)>>> _segmentsOf(
  DatabaseExecutor db,
  Iterable<String> rooms,
) async {
  final segments = <String, List<(int, int)>>{};
  for (final (part, marks) in _roomParts(rooms)) {
    final rows = await db.rawQuery(
      'SELECT room, lo_at, hi_at FROM coverage '
      'WHERE stream = ? AND room IN ($marks) ORDER BY room, lo_at, hi_at',
      [_roomStream, ...part],
    );
    for (final row in rows) {
      if (row case {
        'room': final String room,
        'lo_at': final int lo,
        'hi_at': final int hi,
      }) {
        segments.putIfAbsent(room, () => []).add((lo, hi));
      }
    }
  }
  return segments;
}

/// Records that every event of [rooms] in `[from, to]` is stored. One read
/// per 500 rooms and one batch of writes, for the rooms whose spans changed
/// only (a 750-room `[L, L]` or a 60 s extension costs a few calls).
Future<void> extendCoverage(
  DatabaseExecutor db,
  Iterable<String> rooms,
  int from,
  int to,
) async {
  if (from > to) return;
  final current = await _segmentsOf(db, rooms);
  final batch = db.batch();
  var changed = 0;
  for (final room in rooms.toSet()) {
    final before = current[room] ?? const <(int, int)>[];
    final next = mergeCoverage(before, from, to);
    if (_sameSegments(before, next)) continue;
    changed++;
    batch.rawDelete('DELETE FROM coverage WHERE room = ? AND stream = ?', [
      room,
      _roomStream,
    ]);
    for (final (lo, hi) in next) {
      batch.rawInsert(
        "INSERT INTO coverage(room, stream, lo_at, lo_id, hi_at, hi_id) VALUES (?, ?, ?, '', ?, '')",
        [room, _roomStream, lo, hi],
      );
    }
  }
  if (changed > 0) await batch.commit(noResult: true);
}

bool _sameSegments(List<(int, int)> a, List<(int, int)> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

/// Each room's newest span that began before [before]: `(lo, hi)`. The
/// walk resumes from its `hi`; its `lo` is the base of the new-row rule
/// (docs/message-sync.md#counters).
Future<Map<String, (int, int)>> coverageHeads(
  DatabaseExecutor db,
  Iterable<String> rooms,
  int before,
) async {
  final heads = <String, (int, int)>{};
  for (final MapEntry(key: room, value: segments) in (await _segmentsOf(
    db,
    rooms,
  )).entries) {
    (int, int)? head;
    for (final segment in segments) {
      if (segment.$1 >= before) continue;
      if (head == null || segment.$2 > head.$2) head = segment;
    }
    if (head != null) heads[room] = head;
  }
  return heads;
}

/// Forgets every span: some received event may not have been written.
Future<void> clearCoverage(DatabaseExecutor db) =>
    db.rawDelete('DELETE FROM coverage');

/// The end of each room's latest span that began before [before], for rooms
/// that have one. A span opened at [before] itself (the live REQ that is
/// about to be caught up) may already be written and must not hide the gap.
Future<Map<String, int>> coverageResumePoints(
  DatabaseExecutor db,
  Iterable<String> rooms,
  int before,
) async {
  final points = <String, int>{};
  for (final (part, marks) in _roomParts(rooms)) {
    final rows = await db.rawQuery(
      'SELECT room, max(hi_at) AS hi FROM coverage '
      'WHERE stream = ? AND lo_at < ? AND room IN ($marks) GROUP BY room',
      [_roomStream, before, ...part],
    );
    for (final row in rows) {
      if (row case {'room': final String room, 'hi': final int hi}) {
        points[room] = hi;
      }
    }
  }
  return points;
}
