part of '../channel_messages_provider.dart';

/// Opening the channel window at a message instead of the newest row, and
/// paging from there toward the present (NIP-CW newer direction).
extension _OpenAtWindow on ChannelMessagesNotifier {
  Future<bool> _openAt({required int createdAt, String? eventId}) async {
    final anchor = windowAnchorCursor(createdAt: createdAt, eventId: eventId);
    final cursor = ChannelPageCursor(
      createdAt: anchor.createdAt,
      eventId: anchor.eventId,
    );
    // Rows stored around the message open the window without the relay,
    // also while a painted window is still being checked: what was sent
    // meanwhile joins it like live rows.
    if (await _openStoredAt(cursor, eventId)) return true;
    // A newest page still in flight: ask for the window around the message
    // alongside it rather than after it, so opening costs one round trip.
    // Kept only if that load finishes without being replaced.
    ({int initVersion, int pageVersion, Future<List<ChannelWindowPage>> pages})?
    early;
    if (_initInFlight && await _supportsNewerWindowsSoon()) {
      final pages = _fetchMiddlePages(_summarySession, cursor);
      // Awaited below, or dropped: never an unhandled error.
      pages.ignore();
      early = (
        initVersion: _initVersion,
        pageVersion: ++_threadQuerySerial,
        pages: pages,
      );
    }
    while (_initInFlight) {
      await _initFuture;
    }
    if (await _openStoredAt(cursor, eventId)) return true;
    if (!_summaryMounted || !_usingChannelWindow || isShowingCachedHead) {
      return false;
    }
    // Without newer pages the window could only show the anchor past a gap;
    // keep the newest window and let the caller page back as before.
    if (!await _relaySupportsNewerWindows()) return false;
    if (!_summaryMounted || !_usingChannelWindow || _initInFlight) return false;
    final initVersion = _initVersion;
    final opVersion = ++_windowOpVersion;
    final session = _summarySession;
    try {
      final reused = early != null && early.initVersion == initVersion
          ? early
          : null;
      final pageVersion = reused?.pageVersion ?? ++_threadQuerySerial;
      final pages = reused != null
          ? await reused.pages
          : await _fetchMiddlePages(session, cursor);
      if (!_isCurrentWindowOp(initVersion, opVersion)) return false;
      _windowStore = openMiddleChannelWindow(
        _windowStore,
        pages[0],
        pages[1],
        anchorId: eventId,
      );
      _windowGeneration++;
      _pruneOffWindowReplies();
      for (final page in pages) {
        _reconcilePageSummaries(page, pageVersion);
      }
      _reachedOldest = !channelWindowHasMore(_windowStore);
      _publishWindow(pages.expand((page) => page.rows));
      return true;
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] failed to open $channelId at $createdAt: $error',
      );
      return false;
    }
  }

  /// The pages older than [cursor] and from it toward the present.
  Future<List<ChannelWindowPage>> _fetchMiddlePages(
    RelaySessionNotifier session,
    ChannelPageCursor cursor,
  ) => Future.wait([
    _fetchWindowPage(session, cursor),
    _fetchWindowPage(session, cursor, direction: WindowPageDirection.newer),
  ]);

  /// Opens the window at [cursor] from stored rows when the database has
  /// both sides of it; false leaves the window as it is.
  Future<bool> _openStoredAt(ChannelPageCursor cursor, String? eventId) async {
    // A newest page in flight would replace the window; wait for it unless
    // the window is painted from the database, which it then only settles.
    if (_initInFlight && _storedWindow == _StoredWindow.none) return false;
    if (!_summaryMounted || !_usingChannelWindow) return false;
    final pages = _storedMiddlePages(cursor);
    if (pages == null) return false;
    // Newer pages past the stored ones come from the relay.
    if (pages.newer.hasMore && !await _supportsNewerWindowsSoon()) {
      return false;
    }
    if (!_summaryMounted ||
        !_usingChannelWindow ||
        (_initInFlight && _storedWindow == _StoredWindow.none)) {
      return false;
    }
    // The database may have changed during the check above.
    final current = _storedMiddlePages(cursor);
    if (current == null) return false;
    ++_windowOpVersion;
    try {
      _windowStore = openMiddleChannelWindow(
        _windowStore,
        current.older,
        current.newer,
        anchorId: eventId,
      );
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] stored window rejected for $channelId: $error',
      );
      return false;
    }
    _windowGeneration++;
    _pruneOffWindowReplies();
    final pageVersion = ++_threadQuerySerial;
    _reconcilePageSummaries(current.older, pageVersion);
    _reconcilePageSummaries(current.newer, pageVersion);
    _reachedOldest = !channelWindowHasMore(_windowStore);
    _publishWindow([...current.older.rows, ...current.newer.rows]);
    return true;
  }

  /// [_relaySupportsNewerWindows] without waiting when the relay document
  /// is already known.
  Future<bool> _supportsNewerWindowsSoon() async {
    final known = _storeRef.read(relayInfoProvider).value;
    if (known != null) return known.supports(newerWindowExtension);
    return _relaySupportsNewerWindows();
  }

  Future<bool> _canOpenAt() async {
    while (_initInFlight) {
      await _initFuture;
    }
    if (!_summaryMounted || !_usingChannelWindow) return false;
    return _relaySupportsNewerWindows();
  }

  Future<bool> _fetchNewer() async {
    if (_initInFlight || !_usingChannelWindow) return false;
    return _appendNewerPage(_summarySession, _initVersion);
  }

  /// Fetches the page after the newest loaded row and folds it in. Shared by
  /// scrolling down and by a reconnect, which pages on from the same edge.
  Future<bool> _appendNewerPage(
    RelaySessionNotifier session,
    int initVersion,
  ) async {
    final cursor = _windowStore.newerCursor;
    if (_loadingNewer || !_windowStore.hasNewer || cursor == null) {
      return false;
    }
    final opVersion = _windowOpVersion;
    _loadingNewer = true;
    try {
      final pageVersion = ++_threadQuerySerial;
      // Stored rows first, up to where the stored range ends.
      final page =
          _storedNewerPage(cursor) ??
          await _fetchWindowPage(
            session,
            cursor,
            direction: WindowPageDirection.newer,
          );
      if (!_isCurrentWindowOp(initVersion, opVersion)) return false;
      _windowStore = appendNewerChannelWindow(_windowStore, page);
      _reconcilePageSummaries(page, pageVersion);
      _publishWindow(page.rows);
      return true;
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] failed to fetch newer channel window page for $channelId: $error',
      );
      return false;
    } finally {
      _loadingNewer = false;
    }
  }

  Future<bool> _jumpToLatest() async {
    while (_initInFlight) {
      await _initFuture;
    }
    if (!_summaryMounted || !_usingChannelWindow || !_windowStore.isMiddle) {
      return false;
    }
    final initVersion = _initVersion;
    final opVersion = ++_windowOpVersion;
    final session = _summarySession;
    if (_storedNewestIfCovered() case final stored?) {
      _windowStore = replaceNewestChannelWindow(_windowStore, stored);
      _windowGeneration++;
      _pruneOffWindowReplies();
      _reconcilePageSummaries(stored, ++_threadQuerySerial);
      _reachedOldest = !channelWindowHasMore(_windowStore);
      _publishWindow(stored.rows);
      return true;
    }
    try {
      final pageVersion = ++_threadQuerySerial;
      final response = await session.queryRelay([_channelWindowFilter(null)]);
      final page = parseChannelWindowResponse(response, channelId, null);
      if (!_isCurrentWindowOp(initVersion, opVersion)) return false;
      _windowStore = replaceNewestChannelWindow(_windowStore, page);
      _windowGeneration++;
      _persistHead(response, page);
      _pruneOffWindowReplies();
      _reconcilePageSummaries(page, pageVersion);
      _reachedOldest = !channelWindowHasMore(_windowStore);
      _publishWindow(page.rows);
      return true;
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] failed to jump to the latest page of $channelId: $error',
      );
      return false;
    }
  }

  /// A reconnect keeps a window opened at a message and pages on from its
  /// newest loaded row, rather than replacing it with the newest page.
  Future<void> _refreshMiddleAfterReconnect(
    RelaySessionNotifier session,
    int initVersion,
  ) async {
    _windowStore = reopenNewerChannelWindowEdge(_windowStore);
    // A page in flight from before the reconnect is fenced out by its init
    // version; it must not also block this one.
    _loadingNewer = false;
    // A failed page leaves hasNewer set, so scrolling down retries it.
    await _appendNewerPage(session, initVersion);
    if (!_isCurrentInit(initVersion)) return;
    _publishSummaryChange();
    _summaryRefreshes.resume();
  }

  Future<bool> _relaySupportsNewerWindows() async =>
      (await _readRelayInfo())?.supports(newerWindowExtension) ?? false;

  bool _isCurrentWindowOp(int initVersion, int opVersion) =>
      _summaryMounted &&
      _usingChannelWindow &&
      _isCurrentInit(initVersion) &&
      opVersion == _windowOpVersion;

  void _publishWindow(Iterable<ChannelWindowRow> rows) {
    final flattened = _withDeepLinkEvents(
      flattenChannelWindowEvents(_windowStore),
    );
    _lastKnownMessages = flattened;
    _reconcileRetainedSummaryPayloads(rows.map((row) => row.event));
    _publishSummaryChange();
  }
}
