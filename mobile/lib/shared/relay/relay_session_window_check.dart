part of 'relay_session.dart';

/// Room and thread windows opened from the store and checked against the
/// relay (window_check.dart).
extension RelaySessionWindowCheck on RelaySessionNotifier {
  /// Opens [page] (a room window, or a thread window of [root], below
  /// [before] or the newest) through the check, or `null` when it opens as
  /// before. The open's `query` stands in for [queryRelay] (pass it as
  /// `via` to [queryRelayWithStoreHeads] for a newest window).
  CheckedWindowOpen? openWindow(
    NostrFilter page, {
    required String room,
    String? root,
    (int, String)? before,
  }) {
    final store = _messageStore;
    final lease = _storeLease;
    if (store == null || lease == null || !storeOpensEnabled) return null;
    final window = (room: room, root: root, kinds: page.kinds, before: before);
    Future<StoredCheckWindow?> read(int limit) =>
        store.checkWindow(lease, window, limit: limit);
    return openWindowCheck(
      gate: _windowChecks,
      key: _windowCheckKey(room, root),
      label:
          '${root == null ? 'room $room' : 'thread $root'}'
          '${before == null ? '' : ' below ${before.$1}'}',
      kinds: page.kinds,
      limit: page.limit,
      readStored: () async {
        final stored = await read(page.limit);
        if (stored == null || !stored.fullyStored) return null;
        return CheckedStoreWindow(
          facts: (oldestId: stored.oldest?.$2, count: stored.rows.length),
          response: ({required hasMore}) =>
              _storedEvents(stored.toResponse(hasMore: hasMore)) ?? const [],
        );
      },
      check: (sent) => queryRelay([windowCheckFilter(sent)]),
      beginReplace: () {
        final capture = store.headSent(lease, [room]);
        return (events) async {
          if (events == null) {
            store.recordHeads(capture, lease, const []);
            return null;
          }
          final fetched = pageWindowFacts(events, page.kinds);
          final queued = store.replaceWindow(capture, _storeLease, (
            window: window,
            ids: fetched.ids,
            oldest: fetched.oldest,
            hasMore: fetched.hasMore,
          ));
          if (!queued) return null;
          // The stored window as the page's: as many rows as it held.
          final again = await read(
            fetched.hasMore ? fetched.facts.count : page.limit,
          );
          if (again == null) return null;
          return (oldestId: again.oldest?.$2, count: again.rows.length);
        };
      },
      liveRows: () => _liveRowsByRoom[room] ?? 0,
      fetch: queryRelay,
    );
  }
}

String _windowCheckKey(String room, String? root) =>
    root == null ? room : '$room\u0000$root';

/// Counts each event frame's room, for [decideWindowCheck]'s `liveRows`.
void _noteLiveRow(Map<String, int> counts, List<dynamic> data) {
  if (data.length < 3 || data[0] != 'EVENT') return;
  final event = data[2];
  if (event is! Map<String, dynamic>) return;
  final room = userStreamRoomOf(event);
  if (room != null) counts[room] = (counts[room] ?? 0) + 1;
}
