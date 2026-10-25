import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:sqlite3/sqlite3.dart';

import '../relay/relay.dart';
import 'coverage.dart';

/// Small indexed reads of the message database, run on the UI isolate's
/// read-only connection so a room can paint its first frame from them.
///
/// A room's main timeline is complete wherever a closed `''` or `'*'` range
/// or the room's open live range covers it (see coverage.dart). Reads return
/// rows from one complete range only, so a hole is never painted over: rows
/// on the far side of it come from the server.

/// After every event: the end of an open live range.
const _openEnd = CoverageCursor(1 << 53, '');

/// Rows a room reads per page, as a server window page holds.
const storedPageRows = 50;

/// The rows a room's main timeline has stored at one place, in the shape of a
/// server window page before it is parsed.
@immutable
class StoredTimelinePage {
  const StoredTimelinePage({
    required this.rows,
    required this.overlays,
    required this.reachesStart,
    required this.reachesLive,
  });

  /// Top-level rows in the relay's order: newest first, ascending id within
  /// a second.
  final List<NostrEvent> rows;

  /// The rows' edits, reactions and newest thread summaries.
  final List<NostrEvent> overlays;

  /// No older row exists: the range reaches the room's first message.
  final bool reachesStart;

  /// The range runs on into the room's open live range: nothing newer is
  /// missing while the live feed keeps it open.
  final bool reachesLive;
}

/// The live range opened at [liveFrom] as a coverage range: every event from
/// that second on.
CoverageRange liveCoverageRange(int liveFrom) =>
    CoverageRange(CoverageCursor.endOfSecond(liveFrom - 1), _openEnd);

/// The ranges where [channelId]'s main timeline is complete, merged, oldest
/// first. [liveFrom] is the room's open live range, if any.
List<CoverageRange> mainTimelineCoverage(
  Database db,
  String channelId, {
  int? liveFrom,
}) {
  final pending = <CoverageRange>[
    ...readCoverage(db, channelId, CoverageStream.main),
    ...readCoverage(db, channelId, CoverageStream.all),
    if (liveFrom != null) liveCoverageRange(liveFrom),
  ];
  final merged = <CoverageRange>[];
  while (pending.isNotEmpty) {
    final next = pending.removeLast();
    final result = mergeCoverage(pending, next);
    pending.removeWhere(result.removed.contains);
    merged.add(result.merged);
  }
  return merged..sort((a, b) => a.lo.compareTo(b.lo));
}

/// The newest stored page of [channelId]'s main timeline: the newest
/// complete range that holds rows, read down from its newest row. Null when
/// nothing complete is stored, apart from a room known to be empty (an empty
/// page that reaches the start).
StoredTimelinePage? readNewestStoredPage(
  Database db,
  String channelId, {
  int? liveFrom,
  int limit = storedPageRows,
}) {
  final ranges = mainTimelineCoverage(db, channelId, liveFrom: liveFrom);
  for (final range in ranges.reversed) {
    final rows = _rowsIn(db, channelId, range, before: null, limit: limit);
    if (rows.isEmpty) continue;
    return _page(db, rows, range, limit);
  }
  if (ranges.length == 1 &&
      ranges.single.lo == CoverageCursor.start &&
      _rowsIn(db, channelId, null, before: null, limit: 1).isEmpty) {
    return StoredTimelinePage(
      rows: const [],
      overlays: const [],
      reachesStart: true,
      reachesLive: ranges.single.hi == _openEnd,
    );
  }
  return null;
}

/// The stored rows just older than [before] (exclusive), when the range
/// holding [before] continues below it. Null when the rows below are not
/// known complete: they come from the server.
StoredTimelinePage? readOlderStoredPage(
  Database db,
  String channelId,
  CoverageCursor before, {
  int? liveFrom,
  int limit = storedPageRows,
}) {
  for (final range in mainTimelineCoverage(db, channelId, liveFrom: liveFrom)) {
    if (!(range.lo < before && before <= range.hi)) continue;
    final rows = _rowsIn(db, channelId, range, before: before, limit: limit);
    if (rows.isEmpty && range.lo != CoverageCursor.start) return null;
    return _page(db, rows, range, limit);
  }
  return null;
}

/// Whether [channelId]'s newest message, sent at [latestAt], is stored with
/// a complete range under it.
bool isStoredThroughIn(
  Database db,
  String channelId,
  int latestAt, {
  int? liveFrom,
}) {
  if (liveFrom != null && liveFrom <= latestAt) return true;
  return mainTimelineCoverage(
    db,
    channelId,
  ).any((range) => range.hi.createdAt >= latestAt);
}

/// Up to [limit] stored rows of [channelId] sent at or before [before],
/// newest first: main-timeline rows, or with [rootId] the thread's root and
/// replies. Coverage is not consulted.
List<NostrEvent> readStoredRowsBefore(
  Database db,
  String channelId, {
  required int before,
  String? rootId,
  int limit = 100,
}) {
  final result = db.select(
    'SELECT raw FROM events WHERE channel_id = ? AND deleted = 0 '
    'AND created_at <= ? AND '
    '${rootId == null ? "stream = ''" : '(stream = ? OR id = ?)'} '
    'ORDER BY created_at DESC, id LIMIT ?',
    [channelId, before, ?rootId, ?rootId, limit],
  );
  return [for (final row in result) _decode(row['raw'] as String)];
}

StoredTimelinePage _page(
  Database db,
  List<NostrEvent> rows,
  CoverageRange range,
  int limit,
) => StoredTimelinePage(
  rows: rows,
  overlays: _overlaysOf(db, rows),
  reachesStart: rows.length < limit && range.lo == CoverageCursor.start,
  reachesLive: range.hi == _openEnd,
);

/// Main-timeline rows at or inside [range] (any, when null) and strictly
/// older than [before], newest first in relay order.
List<NostrEvent> _rowsIn(
  Database db,
  String channelId,
  CoverageRange? range, {
  required CoverageCursor? before,
  required int limit,
}) {
  final where = StringBuffer("channel_id = ? AND stream = '' AND deleted = 0");
  final args = <Object?>[channelId];
  // Within a second a smaller id is later (see CoverageCursor).
  void atOrBefore(CoverageCursor cursor) {
    where.write(' AND (created_at < ? OR (created_at = ? AND id >= ?))');
    args.addAll([cursor.createdAt, cursor.createdAt, cursor.id]);
  }

  if (range != null) {
    atOrBefore(range.hi);
    if (range.lo != CoverageCursor.start) {
      where.write(' AND (created_at > ? OR (created_at = ? AND id <= ?))');
      args.addAll([range.lo.createdAt, range.lo.createdAt, range.lo.id]);
    }
  }
  if (before != null) {
    where.write(' AND (created_at < ? OR (created_at = ? AND id > ?))');
    args.addAll([before.createdAt, before.createdAt, before.id]);
  }
  final result = db.select(
    'SELECT raw FROM events WHERE $where '
    'ORDER BY created_at DESC, id LIMIT ?',
    [...args, limit],
  );
  return [for (final row in result) _decode(row['raw'] as String)];
}

/// The live edits and reactions of [rows], and each one's newest thread
/// summary.
List<NostrEvent> _overlaysOf(Database db, List<NostrEvent> rows) {
  if (rows.isEmpty) return const [];
  final marks = List.filled(rows.length, '?').join(', ');
  final result = db.select(
    'SELECT raw FROM events WHERE target_id IN ($marks) AND deleted = 0 '
    'AND kind IN (${EventKind.streamMessageEdit}, ${EventKind.reaction}, '
    '${EventKind.channelThreadSummary}) ORDER BY created_at, id',
    [for (final row in rows) row.id],
  );
  return [for (final row in result) _decode(row['raw'] as String)];
}

NostrEvent _decode(String raw) =>
    NostrEvent.fromJson(jsonDecode(raw) as Map<String, dynamic>);
