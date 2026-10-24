part of '../channel_messages_provider.dart';

/// Rows a background check asks for at most; a room that missed more loads
/// its newest page instead, leaving a hole older pages fill.
const _storedCatchUpLimit = 500;

/// Where a room painted from the message database stands.
enum _StoredWindow {
  /// The window did not come from the database, or has been confirmed.
  none,

  /// Painted from the database; the newest rows are still being checked.
  unchecked,

  /// Painted from the database, and the live feed has kept it complete: the
  /// rows stand as they are.
  covered,
}

/// What this room keeps on the device. Every fetched page and live event is
/// written to the message database; the room paints its first frame from it
/// and asks the server only for what the database cannot prove it has.
extension _LocalStore on ChannelMessagesNotifier {
  /// The room's own live subscription: stored, then shown.
  void _onSubscriptionEvent(NostrEvent event) {
    recordLoadedEvents(_storeRef, [event]);
    _handleLiveEvent(event);
  }

  ChannelHeadScope? _readHeadScope() {
    try {
      return _storeRef.read(channelHeadScopeProvider);
    } catch (_) {
      return null;
    }
  }

  ChannelRoomStore get _roomStore => _storeRef.read(channelRoomStoreProvider);

  /// Installs the newest stored page, and the events still queued for the
  /// database, as the window. Synchronous, so the first build returns the
  /// rows. Returns whether anything was painted (an empty room the database
  /// knows is empty counts).
  bool _paintFromStore() {
    final scope = _readHeadScope();
    if (scope == null) return false;
    try {
      final stored = _roomStore.readNewest(scope, channelId);
      if (stored == null) return false;
      final page = _storedWindowPage(stored, null);
      _windowStore = replaceNewestChannelWindow(_windowStore, page);
      _usingChannelWindow = true;
      _storedReachesLive = stored.reachesLive;
      _storedNewest = stored.rows.firstOrNull;
      _reconcilePageSummaries(page, ++_threadQuerySerial);
      _reachedOldest = !channelWindowHasMore(_windowStore);
      for (final event in _roomStore.unflushed(scope, channelId)) {
        _mergeWindowEventIntoStore(event);
      }
      _storedWindow = _isStoredWindowCovered(scope)
          ? _StoredWindow.covered
          : _StoredWindow.unchecked;
      _lastKnownMessages = flattenChannelWindowEvents(_windowStore);
      return true;
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] stored rows unusable for $channelId: $error',
      );
      _windowStore = const ChannelWindowStore.empty();
      _usingChannelWindow = false;
      _storedWindow = _StoredWindow.none;
      return false;
    }
  }

  /// Whether the painted rows need nothing from the server: the channel
  /// list's live feed covers the room and has kept its range open up to the
  /// painted rows, and the relay is connected.
  bool _isStoredWindowCovered(ChannelHeadScope scope) =>
      _storedReachesLive &&
      _storeRef.read(relaySessionProvider).status == SessionStatus.connected &&
      _roomStore.feeds(scope, channelId) &&
      _roomStore.hasOpenLiveRange(scope, channelId);

  /// Brings a window painted from the database up to date, after the room's
  /// live subscription is in place. Returns false when the newest page must
  /// replace it (more than [_storedCatchUpLimit] events missed); the stored
  /// rows then make way for it like any replaced page.
  Future<bool> _settleStoredWindow(
    RelaySessionNotifier session,
    int initVersion,
  ) async {
    final scope = _readHeadScope();
    if (scope == null) return false;
    if (_isStoredWindowCovered(scope)) {
      // Whatever a reconnect catch-up still brings reaches this room.
      _awaitingCatchUp = true;
      _finishStoredWindow();
      return true;
    }
    // Everything sent since the newest painted row, every kind: new rows
    // join below it, and edits or deletions of stored rows apply in place.
    final newest = _storedNewest;
    final since = newest?.createdAt ?? 0;
    final events = await session.queryRelay([
      NostrFilter(
        kinds: _channelLiveEventKinds,
        tags: {
          '#h': [channelId],
        },
        since: since,
        limit: _storedCatchUpLimit,
      ),
    ]);
    if (!_isCurrentInit(initVersion)) return true;
    if (events.length >= _storedCatchUpLimit) return false;
    recordRoomScan(
      _storeRef,
      channelId,
      events,
      from: newest == null
          ? CoverageCursor.start
          : CoverageCursor(newest.createdAt, newest.id),
    );
    events.sort(compareChannelTimelineEventsChronologically);
    for (final event in events) {
      _handleLiveEvent(event);
    }
    _roomStore.clearGap(scope, channelId, at: since);
    _finishStoredWindow();
    return true;
  }

  void _finishStoredWindow() {
    _storedWindow = _StoredWindow.none;
    _needsInit = false;
    // Stored rows include own sends whose echo was stored.
    _confirmLocalMessages([
      for (final page in _windowStore.allPages)
        for (final row in page.rows) row.event.id,
    ]);
    _summaryRefreshes.resume();
    _publishSummaryChange();
  }

  /// The stored rows just older than [cursor], when the database knows them
  /// complete; null sends the page to the server.
  ChannelWindowPage? _storedOlderPage(ChannelPageCursor cursor) {
    final scope = _readHeadScope();
    if (scope == null) return null;
    try {
      final stored = _roomStore.readOlder(
        scope,
        channelId,
        CoverageCursor(cursor.createdAt, cursor.eventId),
      );
      return stored == null ? null : _storedWindowPage(stored, cursor);
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] stored older rows unusable for $channelId: $error',
      );
      return null;
    }
  }

  /// [stored] as the window page the relay would have served from [start].
  ChannelWindowPage _storedWindowPage(
    StoredTimelinePage stored,
    ChannelPageCursor? start,
  ) {
    final summaries = <String, NostrEvent>{};
    final aux = <NostrEvent>[];
    for (final event in stored.overlays) {
      if (event.kind != EventKind.channelThreadSummary) {
        aux.add(event);
      } else if (event.getTagValue('e') case final root?) {
        summaries[root] = event;
      }
    }
    final rows = [
      for (final event in stored.rows) _storedRow(event, summaries[event.id]),
    ];
    final hasMore = !stored.reachesStart && rows.isNotEmpty;
    return ChannelWindowPage(
      startCursor: start,
      rows: rows,
      aux: aux,
      nextCursor: hasMore ? ChannelPageCursor.of(rows.last.event) : null,
      hasMore: hasMore,
    );
  }

  ChannelWindowRow _storedRow(NostrEvent event, NostrEvent? summary) {
    if (summary == null) return ChannelWindowRow(event: event);
    try {
      return ChannelWindowRow(
        event: event,
        thread: parseChannelWindowThreadSummary(summary),
        threadSummaryCreatedAt: summary.createdAt,
      );
    } catch (_) {
      return ChannelWindowRow(event: event);
    }
  }

  /// Stores the newest page with the coverage it proves; a room the live
  /// feed covers keeps a live range from the page's newest row on.
  void _persistHead(List<NostrEvent> response, ChannelWindowPage page) {
    recordChannelWindowPage(_storeRef, channelId, response, page);
    final scope = _readHeadScope();
    if (scope == null) return;
    _roomStore.clearGap(
      scope,
      channelId,
      at: page.rows.firstOrNull?.event.createdAt,
    );
  }
}
