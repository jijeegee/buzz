part of '../channel_messages_provider.dart';

/// Reads a room's certain newest window from the device store; overridden in
/// tests.
final storedRoomHeadProvider = Provider<StoredRoomHeadReader>(
  (ref) =>
      (session, roomId, filter) => session.certainRoomHead(
        roomId,
        kinds: filter.kinds,
        limit: filter.limit,
      ),
);

typedef StoredRoomHeadReader =
    Future<List<NostrEvent>?> Function(
      RelaySessionNotifier session,
      String roomId,
      NostrFilter filter,
    );

/// Rooms whose newest window this run already took from the relay in the
/// background (the preloader); they skip the store, as on desktop. Reset when
/// the account or server changes.
final relayFetchedRoomsProvider = Provider<Set<String>>((ref) {
  ref.watch(channelHeadScopeProvider);
  return <String>{};
});

/// Opening the room from the device store when its newest window there is
/// certain (docs/message-sync.md#opening-a-room-from-the-store).
extension _StoreHead on ChannelMessagesNotifier {
  /// Loads the newest window and merges it into the room. A window that came
  /// from the store is drawn at once, then the relay is asked once and its
  /// window replaces the stored rows (live arrivals stay). Opening at a
  /// message does not wait for that answer, and a window it opened first is
  /// kept (docs/message-sync.md#opening-at-a-position).
  Future<void> _syncNewest(
    RelaySessionNotifier session,
    int initVersion,
  ) async {
    _storedHead = null;
    final List<NostrEvent>? stored;
    try {
      await _mergeNewest(session, initVersion, const {});
    } finally {
      stored = _storedHead;
      _storedHead = null;
    }
    if (stored == null || !_isCurrentInit(initVersion)) return;
    if (_usingChannelWindow && _windowStore.isMiddle) return;
    _followingStoredHead = true;
    _storedHeadShown?.complete();
    _storedHeadShown = null;
    try {
      await _mergeNewest(session, initVersion, {
        for (final event in stored) event.id,
      });
    } finally {
      if (_isCurrentInit(initVersion)) _followingStoredHead = false;
    }
    if (!_isCurrentInit(initVersion)) return;
    final newest = _windowStore.pages.firstOrNull;
    if (!_usingChannelWindow || newest == null || newest.startCursor != null) {
      return;
    }
    // The page folds 39005 into its rows; their counts stand in for them.
    logStoreComparison(
      'room $channelId',
      'room',
      stored,
      [for (final row in newest.rows) row.event, ...newest.aux],
      relayCounts: {
        for (final row in newest.rows)
          if ((row.thread, row.threadSummaryCreatedAt) case (
            final thread?,
            final at?,
          ))
            row.event.id: (count: thread.replyCount, at: at),
      },
    );
  }

  Future<void> _mergeNewest(
    RelaySessionNotifier session,
    int initVersion,
    Set<String> storedIds,
  ) async {
    final historyVersion = ++_threadQuerySerial;
    final history = await _fetchNewestHistory(session);
    if (history == null || !_isCurrentInit(initVersion)) return;
    // Only relay rows confirm a send; the window's live overlay also holds
    // the unconfirmed local rows themselves.
    _confirmLocalMessages(
      _usingChannelWindow
          ? [
              for (final page in _windowStore.allPages)
                for (final row in page.rows) row.event.id,
            ]
          : history.map((event) => event.id),
    );

    // The relay page is authoritative: drop cached and stored rows it no
    // longer has (e.g. deleted while the app was closed), keeping live
    // arrivals.
    final dropped = {..._cachedHeadIds, ...storedIds};
    _cachedHeadIds = const {};
    final existing = [
      for (final event in _shownMessages ?? const <NostrEvent>[])
        if (!dropped.contains(event.id)) event,
    ];
    final existingIds = existing.map((event) => event.id).toSet();
    final merged = _withDeepLinkEvents([
      ...existing,
      ...history.where((event) => existingIds.add(event.id)),
    ]);
    _lastKnownMessages = merged;
    _reconcileRetainedSummaryPayloads(merged);
    _reconcileRetainedAggregates(historyVersion);
    _publishSummaryChange();
    _summaryRefreshes.resume();
  }

  /// The newest window: on the room's first fetch, the store's window when it
  /// is certain; otherwise (and on every later fetch) the relay's.
  Future<List<NostrEvent>> _openWindowResponse(
    RelaySessionNotifier session,
  ) async {
    final filter = _channelWindowFilter(null);
    if (!_relayHistoryArrived && !_preloadedFromRelay) {
      final stored = await _storedRoomHead(session, channelId, filter);
      if (stored != null) {
        _storedHead = stored;
        return stored;
      }
    }
    return session.queryRelay([filter]);
  }
}
