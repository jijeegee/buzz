part of '../channel_messages_provider.dart';

/// Opening the channel window at a message instead of the newest row, and
/// paging from there toward the present (NIP-CW newer direction).
extension _OpenAtWindow on ChannelMessagesNotifier {
  Future<bool> _openAt({required int createdAt, String? eventId}) async {
    // Saved rows still shown are replaced by the window opened here.
    await _awaitOpenableWindow();
    if (!_summaryMounted || !_usingChannelWindow) return false;
    // Without newer pages the window could only show the anchor past a gap;
    // keep the newest window and let the caller page back as before.
    if (!await _relaySupportsNewerWindows()) return false;
    if (!_summaryMounted ||
        !_usingChannelWindow ||
        (_initInFlight && !_followingStoredHead)) {
      return false;
    }
    final initVersion = _initVersion;
    final opVersion = ++_windowOpVersion;
    try {
      final pages = await _fetchPagesAround(
        _summarySession,
        createdAt: createdAt,
        eventId: eventId,
      );
      if (!_isCurrentWindowOp(initVersion, opVersion)) return false;
      _foldPagesAround(pages.pages, pages.version, eventId);
      _windowGeneration++;
      _publishWindow(pages.pages.expand((page) => page.rows));
      return true;
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] failed to open $channelId at $createdAt: $error',
      );
      return false;
    }
  }

  /// The relay's page each way around a message.
  Future<({List<ChannelWindowPage> pages, int version})> _fetchPagesAround(
    RelaySessionNotifier session, {
    required int createdAt,
    String? eventId,
  }) async {
    final anchor = windowAnchorCursor(createdAt: createdAt, eventId: eventId);
    final cursor = ChannelPageCursor(
      createdAt: anchor.createdAt,
      eventId: anchor.eventId,
    );
    final version = ++_threadQuerySerial;
    final pages = await Future.wait([
      _fetchWindowPage(session, cursor),
      _fetchWindowPage(session, cursor, direction: WindowPageDirection.newer),
    ]);
    return (pages: pages, version: version);
  }

  /// Makes [pages] (from [_fetchPagesAround]) the window, in place of the
  /// rows shown.
  void _foldPagesAround(
    List<ChannelWindowPage> pages,
    int version,
    String? anchorId,
  ) {
    _windowStore = openMiddleChannelWindow(
      _windowStore,
      pages[0],
      pages[1],
      anchorId: anchorId,
    );
    _cachedHeadIds = const {};
    _pruneOffWindowReplies();
    for (final page in pages) {
      _reconcilePageSummaries(page, version);
    }
    _reachedOldest = !channelWindowHasMore(_windowStore);
  }

  Future<bool> _canOpenAt() async {
    await _awaitOpenableWindow();
    if (!_summaryMounted || !_usingChannelWindow) return false;
    return _relaySupportsNewerWindows();
  }

  /// Waits for the first window: the end of init, or a certain window from
  /// the device store whose relay follow-up is still out. Opening at a
  /// message fetches its own pages, so it need not wait for the newest one.
  Future<void> _awaitOpenableWindow() async {
    while (_initInFlight && !_followingStoredHead) {
      final shown = _storedHeadShown ??= Completer<void>();
      await Future.any([_initFuture ?? Future<void>.value(), shown.future]);
    }
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
      final page = await _fetchWindowPage(
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
    try {
      final pageVersion = ++_threadQuerySerial;
      final response = await session.queryRelay([_channelWindowFilter(null)]);
      final page = parseChannelWindowResponse(response, channelId, null);
      if (!_isCurrentWindowOp(initVersion, opVersion)) return false;
      _windowStore = replaceNewestChannelWindow(_windowStore, page);
      _windowGeneration++;
      _persistHead(response);
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
