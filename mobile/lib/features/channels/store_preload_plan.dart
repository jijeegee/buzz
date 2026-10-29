/// Which preload rooms need the relay (docs/message-sync.md#store-first-preload).
/// A room whose live REQ has reached EOSE (it has a token) is asked of the
/// store first; a certain one is left for its open to draw from the store.
/// Every other room is fetched. Mirrors desktop's `storePreloadPlan.ts`;
/// shared cases: test-fixtures/message-store-preload-plan.json.
library;

/// The rooms of one preload pass, by what happens to them.
typedef StorePreloadPlan = ({
  /// Rooms to ask the store about (those with a token), in order.
  List<String> check,

  /// Rooms not requested: an open draws them from the store.
  List<String> certain,

  /// Rooms requested from the relay, in order.
  List<String> fetch,

  /// The fetched rooms that had no token, so were never checked.
  List<String> noToken,
});

StorePreloadPlan planStorePreload(
  Iterable<String> rooms,
  Set<String> tokens,
  Set<String> certain,
) {
  final plan = (
    check: <String>[],
    certain: <String>[],
    fetch: <String>[],
    noToken: <String>[],
  );
  for (final room in rooms.toSet()) {
    if (!tokens.contains(room)) {
      plan.fetch.add(room);
      plan.noToken.add(room);
      continue;
    }
    plan.check.add(room);
    (certain.contains(room) ? plan.certain : plan.fetch).add(room);
  }
  return plan;
}

/// The one log line of a preload pass.
String storePreloadSummary(StorePreloadPlan plan) =>
    '[preload] certain ${plan.certain.length} / '
    'fetched ${plan.fetch.length} / '
    'skipped (no token) ${plan.noToken.length}';
