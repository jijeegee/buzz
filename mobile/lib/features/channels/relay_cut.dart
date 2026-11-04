import 'dart:convert';
import 'dart:math';

import '../../shared/relay/relay.dart';

/// What the last relay window that landed has ruled on, for one rule across
/// rooms and threads: a stored row at or before the cut is in that window,
/// was delivered live or is gone; one after it may be added as not yet
/// confirmed (docs/message-sync.md#reading-the-store-again).
class RelayCut {
  const RelayCut({required this.askedAt, required this.newest});

  /// When that window was asked for: this device's clock, unix seconds.
  final int askedAt;

  /// The `created_at` of its newest row.
  final int newest;

  /// The cut after a relay window asked for at [askedAt] landed with
  /// [rows]. A cut never moves back.
  RelayCut moved({required int askedAt, required Iterable<NostrEvent> rows}) =>
      RelayCut(
        askedAt: max(this.askedAt, askedAt),
        newest: rows.fold(newest, (at, row) => max(at, row.createdAt)),
      );
}

/// [cut] moved by a relay window that landed, or the first one.
RelayCut movedRelayCut(
  RelayCut? cut, {
  required int askedAt,
  required Iterable<NostrEvent> rows,
}) => (cut ?? const RelayCut(askedAt: 0, newest: 0)).moved(
  askedAt: askedAt,
  rows: rows,
);

/// The time to note as a relay request goes out.
int relayAskTime() => DateTime.now().millisecondsSinceEpoch ~/ 1000;

/// Whether a stored row may be added to what the relay already answered. A
/// pushed row compares the time it was [fetchedAt] with the ask, both on
/// this device's clock, so a sender's clock does not matter; a row fetched
/// in the ask's own second counts as after it. Any other row, or a pushed
/// one of an older build, compares its `created_at`. With no [cut] yet
/// nothing was ruled on, and only rows past [newestHeld] (the window's
/// newest) join.
bool isPastRelayCut(
  RelayCut? cut, {
  required int createdAt,
  int? fetchedAt,
  required int newestHeld,
}) {
  if (cut == null) return createdAt > newestHeld;
  if (fetchedAt != null) return fetchedAt >= cut.askedAt;
  return createdAt > max(cut.askedAt, cut.newest);
}

/// The bounds' `pushed_at` of a store paint ([boundsKind] closes it): which
/// rows only the push inbox wrote, and when each was fetched.
Map<String, int?> storedPushedAt(List<NostrEvent> events, int boundsKind) {
  for (final event in events) {
    if (event.kind != boundsKind) continue;
    try {
      final bounds = jsonDecode(event.content);
      final pushed = bounds is Map ? bounds['pushed_at'] : null;
      if (pushed is Map) {
        return {
          for (final MapEntry(:key, :value) in pushed.entries)
            if (key is String) key: value is int ? value : null,
        };
      }
    } on FormatException {
      return const {};
    }
  }
  return const {};
}
