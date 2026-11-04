part of '../channel_messages_provider.dart';

/// Reads a room's newest window from the device store whether or not it is
/// certain, to draw before the relay answers; overridden in tests.
final paintableRoomHeadProvider = Provider<StoredRoomHeadReader>(
  (ref) =>
      (session, roomId, filter) => session.paintableRoomHead(
        roomId,
        kinds: filter.kinds,
        limit: filter.limit,
      ),
);

/// Whether a room's saved rows (the device store and the head cache) were
/// looked up this run. Until then a room page draws nothing rather than its
/// loading skeleton, which is only for a room with nothing saved.
final channelSavedRowsCheckedProvider =
    NotifierProvider.family<ChannelSavedRowsChecked, bool, String>(
      ChannelSavedRowsChecked.new,
    );

class ChannelSavedRowsChecked extends Notifier<bool> {
  ChannelSavedRowsChecked(this.channelId);

  final String channelId;

  @override
  bool build() => false;

  void markChecked() => state = true;

  /// Another account: its saved rows are looked up anew.
  void reset() => state = false;
}

/// Drawing a room's saved rows before the connection, the room's live REQ or
/// the relay's window: they are shown as not yet confirmed, and the relay's
/// window replaces them when it lands
/// (docs/message-sync.md#opening-a-room-from-the-store).
extension ChannelRowOpening on ChannelMessagesNotifier {
  /// Whether the rows shown reach [row] (saved, from the relay or live; not
  /// only a link target held beside them), so the room opens there without
  /// the relay's pages. An [unread] or pushed row the relay has yet to answer
  /// for opens where it is. Other saved rows must run without a gap from it;
  /// a first [unread] row also needs an older row shown, or no older history.
  bool openOnRowsAtHand(
    ({String id, int createdAt}) row, {
    bool unread = false,
  }) => _openOnRowsAtHand(row.id, createdAt: row.createdAt, unread: unread);

  /// Opens at [row]: on the rows at hand once a store read owed to this
  /// visit is in, else with [openAt].
  Future<bool> openAtRow(
    ({String id, int createdAt}) row, {
    bool unread = false,
  }) => _openAtRow(row.id, createdAt: row.createdAt, unread: unread);
}

extension _StoredPaint on ChannelMessagesNotifier {
  /// Looks up the device store and the head cache at once. Whichever answers
  /// first draws; the store's window then replaces a cached head drawn
  /// before it, since the store holds every row the device received.
  Future<void> _restoreSavedRows(ChannelHeadScope scope) async {
    final filter = _channelWindowFilter(null);
    final session = _summarySession;
    final read = ++_storeReads;
    _seenStoreRevision = _read(pushInboxRevisionProvider);
    final stored = _read(paintableRoomHeadProvider)(session, channelId, filter)
        .catchError((Object error) {
          debugPrint(
            '[ChannelMessagesNotifier] stored rows unavailable for $channelId: $error',
          );
          return null;
        });
    final cached = _read(channelHeadCacheProvider)
        .load(scope, channelId)
        .catchError((Object error) {
          debugPrint(
            '[ChannelMessagesNotifier] cached head unavailable for $channelId: $error',
          );
          return null;
        });
    var storeDrew = false;
    try {
      await Future.wait([
        stored.then((events) => storeDrew = _paintStored(scope, events, read)),
        // A store paint (this one or a later redraw) holds every cached row.
        cached.then(
          (events) => storeDrew || _paintedStoreRead > 0
              ? null
              : _paintSaved(scope, events),
        ),
      ]);
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] saved rows not drawn for $channelId: $error',
      );
    } finally {
      // The page waits for this before its skeleton or opening position.
      _markSavedRowsChecked();
    }
  }

  /// The read a room owes when someone comes back to it: the store changed
  /// since it last looked, and nobody was there to read it.
  void _showStoreIfChanged() {
    if (!_summaryMounted || !_hasListeners) return;
    if (_read(pushInboxRevisionProvider) != _seenStoreRevision) {
      _repaintStored();
    }
  }

  /// Reads the device store again after a push inbox drain wrote to it, the
  /// same read a first open makes
  /// (docs/message-sync.md#reading-the-store-again): saved rows still shown
  /// are drawn over, and rows held from the relay gain what lies past its
  /// cut.
  void _repaintStored() {
    final scope = _readHeadScope();
    if (!_hasListeners || !_headRestoreStarted || scope == null) return;
    final read = ++_storeReads;
    _seenStoreRevision = _read(pushInboxRevisionProvider);
    final filter = _channelWindowFilter(null);
    final stored = _read(paintableRoomHeadProvider)(
      _summarySession,
      channelId,
      filter,
    );
    final reading = stored
        .then((events) {
          if (!_paintStored(scope, events, read)) {
            _foldStoredRows(scope, events, read);
          }
        })
        .catchError((Object error) {
          debugPrint(
            '[ChannelMessagesNotifier] stored rows not redrawn for $channelId: $error',
          );
        });
    _storeRead = reading;
    unawaited(reading);
  }

  /// Adds the rows of a store read that lie past the relay cut, and their
  /// reply counts. They show as not yet confirmed: nothing is written, and
  /// a store read confirms no send and vouches for no gap. While newer pages
  /// remain they would sit past rows not loaded, so they wait for those.
  void _foldStoredRows(
    ChannelHeadScope scope,
    List<NostrEvent>? events,
    int read,
  ) {
    if (events == null ||
        !_summaryMounted ||
        read < _paintedStoreRead ||
        _windowStore.hasNewer ||
        _readHeadScope() != scope) {
      return;
    }
    final page = parseChannelWindowResponse(events, channelId, null);
    _paintedStoreRead = read;
    _storePushedAt = storedPushedAt(events, EventKind.channelWindowBounds);
    final shown = _lastKnownMessages ?? const <NostrEvent>[];
    final shownIds = {for (final event in shown) event.id};
    final windowRows = [
      for (final held in _windowStore.allPages)
        for (final row in held.rows) row.event,
    ];
    var newestHeld = 0;
    for (final event in windowRows.isEmpty ? shown : windowRows) {
      if (event.createdAt > newestHeld) newestHeld = event.createdAt;
    }
    final rows = [
      for (final row in page.rows)
        if (!shownIds.contains(row.event.id) &&
            !_processedDeletionTargets.contains(row.event.id) &&
            isPastRelayCut(
              _relayCut,
              createdAt: row.event.createdAt,
              fetchedAt: _storePushedAt[row.event.id],
              newestHeld: newestHeld,
            ))
          row.event,
    ];
    final counted = _noteStoreSummaries(page);
    if (rows.isEmpty && !counted) return;
    if (_usingChannelWindow) {
      _windowStore = foldStoredChannelRows(_windowStore, rows);
      _storeOnlyIds.addAll(_windowStore.storeRows.map((event) => event.id));
      _lastKnownMessages = _withDeepLinkEvents(
        flattenChannelWindowEvents(_windowStore),
      );
    } else {
      _storeOnlyIds.addAll(rows.map((event) => event.id));
      _lastKnownMessages = _withDeepLinkEvents(
        rows.fold(shown, ChannelMessagesNotifier._mergeEvent),
      );
    }
    _publishSummaryChange();
  }

  /// Keeps [page]'s reply counts past the relay cut, a paint's only: they
  /// count pushed replies by `created_at`, so one deleted on the relay does
  /// not come back with a later read. Returns whether any changed.
  bool _noteStoreSummaries(ChannelWindowPage page) {
    var changed = false;
    for (final row in page.rows) {
      final summary = row.thread;
      final lastReplyAt = summary?.lastReplyAt;
      if (summary == null ||
          lastReplyAt == null ||
          !isPastRelayCut(_relayCut, createdAt: lastReplyAt, newestHeld: 0)) {
        continue;
      }
      final root = row.event.id;
      if (_storeSummaries[root]?.lastReplyAt == lastReplyAt) continue;
      _storeSummaries[root] = summary;
      changed = true;
    }
    return changed;
  }

  /// A relay window landed: it ruled on everything up to its cut, so the
  /// rows and counts shown only from the store go, unless they are in it.
  void _foldedRelayRows(int askedAt, Iterable<NostrEvent> rows) {
    _relayCut = movedRelayCut(_relayCut, askedAt: askedAt, rows: rows);
    _storeOnlyIds.clear();
    _storeSummaries.clear();
    _paintedAnchor = null;
  }

  void _foldedRelayPages(int askedAt, List<ChannelWindowPage> pages) =>
      _foldedRelayRows(askedAt, [
        for (final page in pages)
          for (final row in page.rows) row.event,
      ]);

  /// Draws the store's window from read [read] unless a later read already
  /// drew: reads can answer out of order.
  bool _paintStored(
    ChannelHeadScope scope,
    List<NostrEvent>? events,
    int read,
  ) {
    if (read < _paintedStoreRead || !_paintSaved(scope, events)) return false;
    _paintedStoreRead = read;
    return true;
  }

  /// Draws [events] (a channel-window response) unless the relay's window or
  /// any other rows are already shown; saved rows replace saved rows only
  /// while nothing arrived live over them.
  bool _paintSaved(ChannelHeadScope scope, List<NostrEvent>? events) {
    final shown = _lastKnownMessages;
    if (events == null ||
        !_summaryMounted ||
        _relayHistoryArrived ||
        (shown != null &&
            (shown.length != _cachedHeadIds.length ||
                !shown.every((event) => _cachedHeadIds.contains(event.id)))) ||
        _readHeadScope() != scope) {
      return false;
    }
    final page = parseChannelWindowResponse(events, channelId, null);
    if (page.rows.isEmpty) return false;
    _storePushedAt = storedPushedAt(events, EventKind.channelWindowBounds);
    _windowStore = replaceNewestChannelWindow(_windowStore, page);
    final messages = flattenChannelWindowEvents(_windowStore);
    // Live arrivals and own sends are not saved rows: the relay keeps them.
    final live = {
      for (final event in [
        ..._windowStore.liveOverlay,
        ..._windowStore.heldLive,
      ])
        event.id,
    };
    _reachedOldest = !channelWindowHasMore(_windowStore);
    _paintedContiguousFrom = _contiguousFrom(events);
    _cachedHeadIds = {
      for (final event in messages)
        if (!live.contains(event.id)) event.id,
    };
    _lastKnownMessages = messages;
    _publishSummaryChange();
    return true;
  }

  /// From when [events]' rows run without a gap: the store says so in its
  /// bounds (`null`: from nowhere); a cached relay window is whole.
  int? _contiguousFrom(List<NostrEvent> events) {
    for (final event in events) {
      if (event.kind != EventKind.channelWindowBounds) continue;
      try {
        final bounds = jsonDecode(event.content);
        if (bounds is Map && bounds.containsKey('contiguous_from')) {
          final from = bounds['contiguous_from'];
          return from is int ? from : null;
        }
      } on FormatException {
        return null;
      }
    }
    return 0;
  }

  Future<bool> _openAtRow(
    String id, {
    required int createdAt,
    required bool unread,
  }) async {
    bool atHand() =>
        _openOnRowsAtHand(id, createdAt: createdAt, unread: unread);
    if (atHand()) return true;
    // A kept room reads the store as it comes back: the row may be in it.
    _showStoreIfChanged();
    await _storeRead;
    if (!_summaryMounted) return false;
    return atHand() || await _openAt(createdAt: createdAt, eventId: id);
  }

  bool _openOnRowsAtHand(
    String id, {
    required int createdAt,
    required bool unread,
  }) {
    // Saved rows not yet replaced by the relay's.
    final painted = _cachedHeadIds.isNotEmpty;
    final rows = [
      for (final page in _windowStore.allPages)
        for (final row in page.rows) row.event,
      ..._windowStore.storeRows,
      for (final event in _windowStore.liveOverlay)
        if (event.threadReference.parentId == null) event,
      // Saved rows shown while a reconnect waits for the relay's window.
      if (_windowStore.allPages.isEmpty)
        for (final event in _lastKnownMessages ?? const <NostrEvent>[])
          if (_cachedHeadIds.contains(event.id) ||
              _storeOnlyIds.contains(event.id))
            event,
    ];
    final target = rows.where((event) => event.id == id).firstOrNull;
    if (target == null) return false;
    // An unread or pushed row the relay has yet to answer for opens where it
    // is: the relay fills any gap above it, around it if its newest page
    // lacks the row (_openAroundPaintedAnchor).
    if ((painted || _storeOnlyIds.contains(id)) &&
        (unread || _storePushedAt.containsKey(id))) {
      // Only for the relay answer on its way: in a kept room none may come
      // until a reconnect, long after the reader left this row.
      if (painted || _initInFlight) {
        _paintedAnchor = (createdAt: createdAt, id: id);
      }
      return true;
    }
    final from = painted ? _paintedContiguousFrom : 0;
    if (from == null || target.createdAt < from) return false;
    final reaches =
        !unread ||
        (!painted && _reachedOldest) ||
        rows.any(
          (event) =>
              event.createdAt < target.createdAt && event.createdAt >= from,
        );
    // The relay's newest page may not hold it: its window then comes from
    // around it instead (_openAroundPaintedAnchor).
    if (reaches && painted) _paintedAnchor = (createdAt: createdAt, id: id);
    return reaches;
  }

  /// When the room opened at a saved row the relay's newest [page] does not
  /// hold, makes the relay's pages around that row the window instead, so
  /// the row being read stays. Returns whether it did.
  Future<bool> _openAroundPaintedAnchor(
    RelaySessionNotifier session,
    ChannelWindowPage page,
  ) async {
    final anchor = _paintedAnchor;
    _paintedAnchor = null;
    if (anchor == null || page.rows.any((row) => row.event.id == anchor.id)) {
      return false;
    }
    final initVersion = _initVersion;
    try {
      if (!await _relaySupportsNewerWindows()) return false;
      final around = await _fetchPagesAround(
        session,
        createdAt: anchor.createdAt,
        eventId: anchor.id,
      );
      if (!_summaryMounted || !_isCurrentInit(initVersion)) return false;
      _usingChannelWindow = true;
      _foldPagesAround(around.pages, around.version, anchor.id, around.askedAt);
      _openedAroundAnchor = true;
      return true;
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] no window around ${anchor.id} in $channelId: $error',
      );
      return false;
    }
  }

  void _markSavedRowsChecked() {
    if (!_summaryMounted) return;
    _read(channelSavedRowsCheckedProvider(channelId).notifier).markChecked();
  }
}
