import 'package:flutter/foundation.dart';
import 'package:sqlite3/sqlite3.dart';

/// Every change to what the message database claims is complete goes through
/// this file: coverage ranges, room live ranges and per-server sync state.
/// Keeping them together is what lets a later deletion stage trim ranges in
/// the same place it removes rows.
///
/// Coverage is written only from explicit fetch claims (a page's bounds or a
/// complete scan), never because an event arrived.

/// A position in a room's timeline, in the relay's paging order: newest
/// `created_at` first, ascending id within a second. Chronologically, a
/// later position has a larger [createdAt] or, within the same second, a
/// smaller [id].
@immutable
class CoverageCursor implements Comparable<CoverageCursor> {
  const CoverageCursor(this.createdAt, this.id);

  /// Before every message: a range that starts here reaches the room's (or
  /// thread's) first message. Real events never have a negative time.
  static const start = CoverageCursor(-1, '');

  /// The latest position within [createdAt]: every event of that second is
  /// at or before it. A complete scan ends here, since it holds every event
  /// of its newest second.
  const CoverageCursor.endOfSecond(this.createdAt) : id = '';

  final int createdAt;
  final String id;

  /// Chronological order: negative when this is earlier than [other].
  @override
  int compareTo(CoverageCursor other) {
    final byTime = createdAt.compareTo(other.createdAt);
    return byTime != 0 ? byTime : other.id.compareTo(id);
  }

  bool operator <(CoverageCursor other) => compareTo(other) < 0;
  bool operator <=(CoverageCursor other) => compareTo(other) <= 0;

  @override
  bool operator ==(Object other) =>
      other is CoverageCursor && other.createdAt == createdAt && other.id == id;

  @override
  int get hashCode => Object.hash(createdAt, id);

  @override
  String toString() => '($createdAt, $id)';
}

CoverageCursor _min(CoverageCursor a, CoverageCursor b) => a <= b ? a : b;
CoverageCursor _max(CoverageCursor a, CoverageCursor b) => a <= b ? b : a;

/// A closed range [lo, hi]: every event of its stream at or between the two
/// positions is stored.
@immutable
class CoverageRange {
  const CoverageRange(this.lo, this.hi);

  final CoverageCursor lo;
  final CoverageCursor hi;

  bool get isValid => lo <= hi;

  /// Whether the two ranges overlap or share an endpoint. Ranges that only
  /// come close are kept apart: the hole between them may hold events.
  bool connects(CoverageRange other) => lo <= other.hi && other.lo <= hi;

  @override
  bool operator ==(Object other) =>
      other is CoverageRange && other.lo == lo && other.hi == hi;

  @override
  int get hashCode => Object.hash(lo, hi);

  @override
  String toString() => '[$lo, $hi]';
}

/// Coverage keys. A thread window pages only [threadWindowKinds]-style
/// conversation kinds, so its claims are kept apart from a complete scan of
/// the thread, which covers every timeline kind.
abstract final class CoverageStream {
  /// The room's main timeline: top-level rows and their overlays.
  static const main = '';

  /// Every kind in the room (reserved for catch-up).
  static const all = '*';

  /// Every reply of a thread, from a complete scan.
  static String thread(String rootId) => rootId;

  /// The conversation replies a thread window returns.
  static String threadWindow(String rootId) => '$rootId:window';
}

/// One fetch's claim: [range] of [stream] in [channelId] is complete.
@immutable
class CoverageClaim {
  const CoverageClaim({
    required this.channelId,
    required this.stream,
    required this.range,
  });

  final String channelId;
  final String stream;
  final CoverageRange range;

  @override
  String toString() => 'CoverageClaim($channelId, "$stream", $range)';
}

/// The claim a window page proves, or null when it proves nothing.
///
/// [rows] are the page's row positions in any order; [start] is the request
/// cursor (null for a room's newest page). An older page covers from its
/// oldest row (or the start, when [hasMore] is false) up to [start]; a newer
/// page covers from [start] up to its newest row. A newest page claims no
/// further than its newest row: an event can still arrive with an earlier
/// timestamp than the moment the page was served.
CoverageClaim? windowPageClaim({
  required String channelId,
  required String stream,
  required Iterable<CoverageCursor> rows,
  required CoverageCursor? start,
  required bool newer,
  required bool hasMore,
}) {
  CoverageCursor? oldest;
  CoverageCursor? newest;
  for (final row in rows) {
    if (oldest == null || row < oldest) oldest = row;
    if (newest == null || newest < row) newest = row;
  }
  final CoverageRange range;
  if (newer) {
    if (start == null || newest == null) return null;
    range = CoverageRange(start, newest);
  } else {
    final lo = hasMore ? oldest : CoverageCursor.start;
    final hi = start ?? newest;
    if (lo == null || hi == null) return null;
    range = CoverageRange(lo, hi);
  }
  if (!range.isValid) return null;
  return CoverageClaim(channelId: channelId, stream: stream, range: range);
}

/// [existing] after [claim] joins it: every range that connects to the claim
/// merges into one; the rest are unchanged. Order is not kept.
({List<CoverageRange> removed, CoverageRange merged}) mergeCoverage(
  Iterable<CoverageRange> existing,
  CoverageRange claim,
) {
  var merged = claim;
  final removed = <CoverageRange>[];
  // A merged range can grow to reach a range it skipped; repeat until stable.
  final pending = existing.toList();
  var changed = true;
  while (changed) {
    changed = false;
    for (final range in List.of(pending)) {
      if (!range.connects(merged)) continue;
      merged = CoverageRange(
        _min(range.lo, merged.lo),
        _max(range.hi, merged.hi),
      );
      removed.add(range);
      pending.remove(range);
      changed = true;
    }
  }
  return (removed: removed, merged: merged);
}

/// Records [claim] inside the caller's transaction, merging it with the
/// stream's connected ranges.
void applyCoverageClaim(Database db, CoverageClaim claim) {
  if (!claim.range.isValid) return;
  final existing = [
    for (final row in db.select(
      'SELECT lo_at, lo_id, hi_at, hi_id FROM coverage '
      'WHERE channel_id = ? AND stream = ?',
      [claim.channelId, claim.stream],
    ))
      CoverageRange(
        CoverageCursor(row['lo_at'] as int, row['lo_id'] as String),
        CoverageCursor(row['hi_at'] as int, row['hi_id'] as String),
      ),
  ];
  final (:removed, :merged) = mergeCoverage(existing, claim.range);
  if (removed.length == 1 && removed.single == merged) return;
  for (final range in removed) {
    db.execute(
      'DELETE FROM coverage WHERE channel_id = ? AND stream = ? '
      'AND lo_at = ? AND lo_id = ?',
      [claim.channelId, claim.stream, range.lo.createdAt, range.lo.id],
    );
  }
  db.execute(
    'INSERT INTO coverage (channel_id, stream, lo_at, lo_id, hi_at, hi_id) '
    'VALUES (?, ?, ?, ?, ?, ?)',
    [
      claim.channelId,
      claim.stream,
      merged.lo.createdAt,
      merged.lo.id,
      merged.hi.createdAt,
      merged.hi.id,
    ],
  );
}

/// The stored ranges of one stream, oldest first.
List<CoverageRange> readCoverage(Database db, String channelId, String stream) {
  final ranges = [
    for (final row in db.select(
      'SELECT lo_at, lo_id, hi_at, hi_id FROM coverage '
      'WHERE channel_id = ? AND stream = ?',
      [channelId, stream],
    ))
      CoverageRange(
        CoverageCursor(row['lo_at'] as int, row['lo_id'] as String),
        CoverageCursor(row['hi_at'] as int, row['hi_id'] as String),
      ),
  ]..sort((a, b) => a.lo.compareTo(b.lo));
  return ranges;
}

/// Sets each room's open live range: a time starts a new one from it (any
/// earlier one ends, leaving a hole); null ends it.
void applyLiveRanges(Database db, Map<String, int?> liveFrom) {
  for (final MapEntry(key: channelId, value: from) in liveFrom.entries) {
    if (from == null) {
      db.execute('DELETE FROM room_state WHERE channel_id = ?', [channelId]);
    } else {
      db.execute(
        'INSERT INTO room_state (channel_id, live_from) VALUES (?, ?) '
        'ON CONFLICT (channel_id) DO UPDATE SET live_from = excluded.live_from',
        [channelId, from],
      );
    }
  }
}

/// What one write changes about a physical server's sync state. The
/// watermark only moves forward, and only in the transaction that stores the
/// events it vouches for.
@immutable
class SyncStateUpdate {
  const SyncStateUpdate({required this.origin, this.watermark, this.liveSince});

  /// The physical server origin (not the storage scope): replication lag
  /// makes one server's "since T" meaningless on another.
  final String origin;
  final int? watermark;

  /// When this device's live feed from [origin] last (re)connected.
  final int? liveSince;
}

void applySyncState(Database db, SyncStateUpdate update) {
  db.execute(
    'INSERT INTO sync_state (origin, watermark, live_since) VALUES (?, ?, ?) '
    'ON CONFLICT (origin) DO UPDATE SET '
    'watermark = CASE WHEN excluded.watermark IS NULL THEN watermark '
    'ELSE max(coalesce(watermark, 0), excluded.watermark) END, '
    'live_since = coalesce(excluded.live_since, live_since)',
    [update.origin, update.watermark, update.liveSince],
  );
}
