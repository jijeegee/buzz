import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:sqlite3/sqlite3.dart';

import '../relay/relay.dart';
import 'coverage.dart';

/// Small indexed reads of the message database, run on the UI isolate's
/// read-only connection so a room or thread can paint its first frame from
/// them.
///
/// A room's main timeline is complete wherever a closed `''` or `'*'` range
/// or the room's open live range covers it (see coverage.dart). A thread
/// (named by its root id) is complete wherever one of its own ranges, a
/// `'*'` range or the room's open live range covers it. Reads return rows
/// from one complete range only, so a hole is never painted over: rows on
/// the far side of it come from the server.

/// After every event: the end of an open live range.
const _openEnd = CoverageCursor(1 << 53, '');

/// Rows a room reads per page, as a server window page holds.
const storedPageRows = 50;

/// The rows a room's main timeline (or one thread) has stored at one place,
/// in the shape of a server window page before it is parsed.
@immutable
class StoredTimelinePage {
  const StoredTimelinePage({
    required this.rows,
    required this.overlays,
    required this.reachesStart,
    required this.reachesLive,
  });

  /// Rows in the relay's order: newest first, ascending id within a second.
  /// A newer page ([readNewerStoredPage]) runs the other way: oldest first.
  final List<NostrEvent> rows;

  /// The rows' edits and reactions, and for main-timeline rows each one's
  /// newest thread summary.
  final List<NostrEvent> overlays;

  /// No older row exists: the range reaches the room's (or thread's) first
  /// message.
  final bool reachesStart;

  /// The range runs on into the room's open live range: nothing newer is
  /// missing while the live feed keeps it open. On a newer page, also that
  /// the page holds every row up to it.
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
}) => storedCoverage(db, channelId, liveFrom: liveFrom);

/// The ranges where [channelId]'s main timeline, or with [rootId] that
/// thread's replies, are complete, merged, oldest first. [liveFrom] is the
/// room's open live range, if any: the live feed delivers every event of the
/// room, thread replies included.
List<CoverageRange> storedCoverage(
  Database db,
  String channelId, {
  String? rootId,
  int? liveFrom,
}) {
  final pending = <CoverageRange>[
    if (rootId == null)
      ...readCoverage(db, channelId, CoverageStream.main)
    else ...[
      ...readCoverage(db, channelId, CoverageStream.thread(rootId)),
      ...readCoverage(db, channelId, CoverageStream.threadWindow(rootId)),
    ],
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

/// The newest stored page of [channelId]'s main timeline (or with [rootId],
/// of that thread's replies): the newest complete range that holds rows,
/// read down from its newest row. Null when nothing complete is stored,
/// apart from a room or thread known to be empty (an empty page that reaches
/// the start).
StoredTimelinePage? readNewestStoredPage(
  Database db,
  String channelId, {
  String? rootId,
  int? liveFrom,
  int limit = storedPageRows,
}) {
  final stream = rootId ?? CoverageStream.main;
  final ranges = storedCoverage(
    db,
    channelId,
    rootId: rootId,
    liveFrom: liveFrom,
  );
  for (final range in ranges.reversed) {
    final rows = _rowsIn(db, channelId, stream, range, limit: limit);
    if (rows.isEmpty) continue;
    return _page(db, rows, range, limit, threadRows: rootId != null);
  }
  if (ranges.length == 1 &&
      ranges.single.lo == CoverageCursor.start &&
      _rowsIn(db, channelId, stream, null, limit: 1).isEmpty) {
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
  String? rootId,
  int? liveFrom,
  int limit = storedPageRows,
}) {
  final stream = rootId ?? CoverageStream.main;
  for (final range in storedCoverage(
    db,
    channelId,
    rootId: rootId,
    liveFrom: liveFrom,
  )) {
    if (!(range.lo < before && before <= range.hi)) continue;
    final rows = _rowsIn(
      db,
      channelId,
      stream,
      range,
      before: before,
      limit: limit,
    );
    if (rows.isEmpty && range.lo != CoverageCursor.start) return null;
    return _page(db, rows, range, limit, threadRows: rootId != null);
  }
  return null;
}

/// The stored rows just newer than [after] (exclusive), oldest first, when
/// the range holding [after] continues above it. Null when the rows above
/// are not known complete. [StoredTimelinePage.reachesLive] says the page
/// holds every row up to the room's open live range.
StoredTimelinePage? readNewerStoredPage(
  Database db,
  String channelId,
  CoverageCursor after, {
  String? rootId,
  int? liveFrom,
  int limit = storedPageRows,
}) {
  final stream = rootId ?? CoverageStream.main;
  for (final range in storedCoverage(
    db,
    channelId,
    rootId: rootId,
    liveFrom: liveFrom,
  )) {
    if (!(range.lo <= after && after < range.hi)) continue;
    final rows = _rowsIn(
      db,
      channelId,
      stream,
      range,
      after: after,
      limit: limit,
    );
    final reachesLive = rows.length < limit && range.hi == _openEnd;
    if (rows.isEmpty && !reachesLive) return null;
    return StoredTimelinePage(
      rows: rows,
      overlays: _overlaysOf(db, rows, threadRows: rootId != null),
      reachesStart: false,
      reachesLive: reachesLive,
    );
  }
  return null;
}

/// What the stored main timeline proves about [channelId]'s first row after
/// [readAt] (a read marker, in seconds) that [counts]: [StoredFirstAfter.known]
/// when the range holding the marker runs complete up to that row, or up to
/// the open live range with no such row ([StoredFirstAfter.row] null).
StoredFirstAfter readFirstStoredAfter(
  Database db,
  String channelId,
  int readAt,
  bool Function(NostrEvent row) counts, {
  int? liveFrom,
  int limit = 200,
}) {
  final after = CoverageCursor.endOfSecond(readAt);
  for (final range in storedCoverage(db, channelId, liveFrom: liveFrom)) {
    if (!(range.lo <= after && after < range.hi)) continue;
    final rows = _rowsIn(
      db,
      channelId,
      CoverageStream.main,
      range,
      after: after,
      limit: limit,
    );
    for (final row in rows) {
      if (counts(row)) return (known: true, row: row);
    }
    final complete = rows.length < limit && range.hi == _openEnd;
    return (known: complete, row: null);
  }
  return (known: false, row: null);
}

/// See [readFirstStoredAfter].
typedef StoredFirstAfter = ({bool known, NostrEvent? row});

/// When stored main-timeline row [id] of [channelId] was sent, or null when
/// it is not stored (or deleted).
int? readStoredCreatedAt(Database db, String channelId, String id) {
  final result = db.select(
    'SELECT created_at FROM events WHERE id = ? AND channel_id = ? '
    "AND stream = '' AND deleted = 0",
    [id, channelId],
  );
  return result.isEmpty ? null : result.single['created_at'] as int;
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
  int limit, {
  required bool threadRows,
}) => StoredTimelinePage(
  rows: rows,
  overlays: _overlaysOf(db, rows, threadRows: threadRows),
  reachesStart: rows.length < limit && range.lo == CoverageCursor.start,
  reachesLive: range.hi == _openEnd,
);

/// Rows of [stream] at or inside [range] (any, when null): strictly older
/// than [before], newest first in relay order, or strictly newer than
/// [after], oldest first.
List<NostrEvent> _rowsIn(
  Database db,
  String channelId,
  String stream,
  CoverageRange? range, {
  CoverageCursor? before,
  CoverageCursor? after,
  required int limit,
}) {
  final where = StringBuffer('channel_id = ? AND stream = ? AND deleted = 0');
  final args = <Object?>[channelId, stream];
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
  if (after != null) {
    where.write(' AND (created_at > ? OR (created_at = ? AND id < ?))');
    args.addAll([after.createdAt, after.createdAt, after.id]);
  }
  final order = after != null
      ? 'created_at ASC, id DESC'
      : 'created_at DESC, id';
  final result = db.select(
    'SELECT raw FROM events WHERE $where ORDER BY $order LIMIT ?',
    [...args, limit],
  );
  return [for (final row in result) _decode(row['raw'] as String)];
}

/// The live edits and reactions of [rows], and for main-timeline rows each
/// one's newest thread summary (a thread shows its nested replies itself).
List<NostrEvent> _overlaysOf(
  Database db,
  List<NostrEvent> rows, {
  bool threadRows = false,
}) {
  if (rows.isEmpty) return const [];
  final marks = List.filled(rows.length, '?').join(', ');
  final kinds = [
    EventKind.streamMessageEdit,
    EventKind.reaction,
    if (!threadRows) EventKind.channelThreadSummary,
  ].join(', ');
  final result = db.select(
    'SELECT raw FROM events WHERE target_id IN ($marks) AND deleted = 0 '
    'AND kind IN ($kinds) ORDER BY created_at, id',
    [for (final row in rows) row.id],
  );
  return [for (final row in result) _decode(row['raw'] as String)];
}

NostrEvent _decode(String raw) =>
    NostrEvent.fromJson(jsonDecode(raw) as Map<String, dynamic>);
