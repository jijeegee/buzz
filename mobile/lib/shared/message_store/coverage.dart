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

/// Records that every event of [rooms] in `[from, to]` is stored.
Future<void> extendCoverage(
  DatabaseExecutor db,
  Iterable<String> rooms,
  int from,
  int to,
) async {
  for (final room in rooms) {
    final rows = await db.rawQuery(
      'SELECT lo_at, hi_at FROM coverage WHERE room = ? AND stream = ? ORDER BY lo_at',
      [room, _roomStream],
    );
    final next = mergeCoverage(
      [for (final row in rows) (row['lo_at']! as int, row['hi_at']! as int)],
      from,
      to,
    );
    await db.rawDelete('DELETE FROM coverage WHERE room = ? AND stream = ?', [
      room,
      _roomStream,
    ]);
    for (final (lo, hi) in next) {
      await db.rawInsert(
        "INSERT INTO coverage(room, stream, lo_at, lo_id, hi_at, hi_id) VALUES (?, ?, ?, '', ?, '')",
        [room, _roomStream, lo, hi],
      );
    }
  }
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
  for (final room in rooms) {
    final rows = await db.rawQuery(
      'SELECT max(hi_at) AS hi FROM coverage '
      'WHERE room = ? AND stream = ? AND lo_at < ?',
      [room, _roomStream, before],
    );
    final hi = rows.isEmpty ? null : rows.first['hi'];
    if (hi is int) points[room] = hi;
  }
  return points;
}
