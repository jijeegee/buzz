import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import '../relay/nostr_models.dart';

/// Comparing a window opened from the device store with the relay's answer
/// that replaced it (docs/message-sync.md#store-vs-relay-log). The log is the
/// evidence for later skipping the relay when the store is certain.
typedef StoreRelayDifference = ({
  int compared,
  List<String> missing,
  List<String> extra,
});

/// Edits, reactions and removals of rows both answers hold, and the thread
/// roots among them whose reply count differs.
typedef StoreRelayOverlayDifference = ({
  List<String> overlaysMissing,
  List<String> overlaysExtra,
  List<String> summaries,
});

final _timelineKinds = EventKind.channelTimelineContentKinds.toSet();

/// Differences inside the time range both answers cover: from the newer of
/// the two oldest rows up to the store's newest row. Rows that arrived live
/// after the store answered, or that fell off the end of the relay's page,
/// are not differences.
StoreRelayDifference? storeRelayDifference(
  List<NostrEvent> stored,
  List<NostrEvent> relay,
) {
  if (stored.isEmpty || relay.isEmpty) return null;
  int oldest(List<NostrEvent> rows) =>
      rows.map((row) => row.createdAt).reduce(math.min);
  final from = math.max(oldest(stored), oldest(relay));
  final to = stored.map((row) => row.createdAt).reduce(math.max);
  Set<String> inRange(List<NostrEvent> rows) => {
    for (final row in rows)
      if (row.createdAt >= from && row.createdAt <= to) row.id,
  };
  final storedIds = inRange(stored);
  final relayIds = inRange(relay);
  return (
    compared: storedIds.length,
    missing: [...relayIds.where((id) => !storedIds.contains(id))],
    extra: [...storedIds.where((id) => !relayIds.contains(id))],
  );
}

/// A thread's reply count as one answer saw it, and when that was.
typedef ReplyCount = ({int count, int at});

const _deletionKinds = {5, 9005};

/// The newest summary's `reply_count` per root, from 39005 events.
Map<String, ReplyCount> replyCountsOf(List<NostrEvent> events) {
  final byRoot = <String, ReplyCount>{};
  for (final event in events) {
    if (event.kind != EventKind.channelThreadSummary) continue;
    final root = event.tags
        .where((tag) => tag.length > 1 && tag[0] == 'e')
        .firstOrNull?[1];
    if (root == null) continue;
    final int count;
    try {
      final value = (jsonDecode(event.content) as Map)['reply_count'];
      if (value is! num) continue;
      count = value.toInt();
    } catch (_) {
      continue;
    }
    final kept = byRoot[root];
    if (kept == null || kept.at < event.createdAt) {
      byRoot[root] = (count: count, at: event.createdAt);
    }
  }
  return byRoot;
}

/// What both answers say about the rows they share: the other events aimed
/// at them (`e` tag) and their thread reply counts. Only what both could
/// know is compared: nothing newer than the store's newest event ([fence]),
/// and no overlay that its own answer also removes (the relay leaves those
/// out). Counts, not whole summaries: a reader's blocks change the rest.
StoreRelayOverlayDifference storeRelayOverlayDifference(
  List<NostrEvent> stored,
  List<NostrEvent> relay,
  Set<String> shared,
  Map<String, ReplyCount> storedCounts,
  Map<String, ReplyCount> relayCounts,
  int fence,
) {
  Set<String> overlays(List<NostrEvent> events) {
    final removed = {
      for (final event in events)
        if (_deletionKinds.contains(event.kind))
          for (final tag in event.tags)
            if (tag.length > 1 && tag[0] == 'e') tag[1],
    };
    return {
      for (final event in events)
        if (event.createdAt <= fence &&
            !removed.contains(event.id) &&
            !_timelineKinds.contains(event.kind) &&
            event.kind != EventKind.channelThreadSummary &&
            event.tags.any(
              (tag) =>
                  tag.length > 1 && tag[0] == 'e' && shared.contains(tag[1]),
            ))
          event.id,
    };
  }

  final storedOverlays = overlays(stored);
  final relayOverlays = overlays(relay);
  final roots = {
    ...storedCounts.keys,
    ...relayCounts.keys,
  }.where(shared.contains);
  return (
    overlaysMissing: [
      ...relayOverlays.where((id) => !storedOverlays.contains(id)),
    ],
    overlaysExtra: [
      ...storedOverlays.where((id) => !relayOverlays.contains(id)),
    ],
    summaries: [
      for (final root in roots)
        if ((storedCounts[root]?.at ?? 0) <= fence &&
            (relayCounts[root]?.at ?? 0) <= fence &&
            (storedCounts[root]?.count ?? 0) != (relayCounts[root]?.count ?? 0))
          root,
    ],
  );
}

/// This run's comparison counts, logged at most once a minute.
const _tallyKeys = [
  'room.matches',
  'room.differs',
  'thread.matches',
  'thread.differs',
  'missing',
  'extra',
  'overlays',
  'summaries',
];

@visibleForTesting
final storeComparisonTally = {for (final key in _tallyKeys) key: 0};
DateTime? _tallyLoggedAt;

@visibleForTesting
void resetStoreComparisonTally() {
  for (final key in _tallyKeys) {
    storeComparisonTally[key] = 0;
  }
  _tallyLoggedAt = null;
}

/// One log line per comparison, and this run's counts at most once a minute.
/// [stored] and [relay] are whole answers: timeline rows plus what is aimed
/// at them. [stream] is `room` or `thread`.
void logStoreComparison(
  String scope,
  String stream,
  List<NostrEvent> stored,
  List<NostrEvent> relay, {
  DateTime? now,
  Map<String, ReplyCount>? relayCounts,
}) {
  List<NostrEvent> rows(List<NostrEvent> events) => [
    for (final event in events)
      if (_timelineKinds.contains(event.kind)) event,
  ];
  final difference = storeRelayDifference(rows(stored), rows(relay));
  if (difference == null) return;
  final (:compared, :missing, :extra) = difference;
  final relayIds = {for (final event in relay) event.id};
  final shared = {
    for (final event in rows(stored))
      if (relayIds.contains(event.id)) event.id,
  };
  final (
    :overlaysMissing,
    :overlaysExtra,
    :summaries,
  ) = storeRelayOverlayDifference(
    stored,
    relay,
    shared,
    replyCountsOf(stored),
    relayCounts ?? replyCountsOf(relay),
    stored.map((event) => event.createdAt).reduce(math.max),
  );
  final overlays = overlaysMissing.length + overlaysExtra.length;
  final same =
      missing.isEmpty && extra.isEmpty && overlays == 0 && summaries.isEmpty;
  void add(String key, int count) =>
      storeComparisonTally[key] = (storeComparisonTally[key] ?? 0) + count;
  add('$stream.${same ? 'matches' : 'differs'}', 1);
  add('missing', missing.length);
  add('extra', extra.length);
  add('overlays', overlays);
  add('summaries', summaries.length);
  if (same) {
    debugPrint('[message-store] $scope: store matches relay ($compared)');
  } else {
    debugPrint(
      '[message-store] $scope: store differs from relay '
      '(compared $compared, missing ${missing.length} ${missing.take(5)}, '
      'extra ${extra.length} ${extra.take(5)}, '
      'overlays +${overlaysMissing.length} -${overlaysExtra.length}, '
      'summaries ${summaries.take(5)})',
    );
  }
  final at = now ?? DateTime.now();
  final last = _tallyLoggedAt;
  // The first counts a minute after the first comparison, then each minute.
  if (last == null) {
    _tallyLoggedAt = at;
  } else if (at.difference(last) >= const Duration(minutes: 1)) {
    _tallyLoggedAt = at;
    debugPrint('[message-store] comparisons this run $storeComparisonTally');
  }
}
