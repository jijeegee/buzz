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

/// One log line per comparison.
void logStoreComparison(
  String scope,
  List<NostrEvent> stored,
  List<NostrEvent> relay,
) {
  final difference = storeRelayDifference(stored, relay);
  if (difference == null) return;
  final (:compared, :missing, :extra) = difference;
  if (missing.isEmpty && extra.isEmpty) {
    debugPrint('[message-store] $scope: store matches relay ($compared)');
    return;
  }
  debugPrint(
    '[message-store] $scope: store differs from relay '
    '(compared $compared, missing ${missing.length} ${missing.take(5)}, '
    'extra ${extra.length} ${extra.take(5)})',
  );
}
