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
}

/// Drawing a room's saved rows before the connection, the room's live REQ or
/// the relay's window: they are shown as not yet confirmed, and the relay's
/// window replaces them when it lands
/// (docs/message-sync.md#opening-a-room-from-the-store).
extension _StoredPaint on ChannelMessagesNotifier {
  /// Looks up the device store and the head cache at once. Whichever answers
  /// first draws; the store's window then replaces a cached head drawn
  /// before it, since the store holds every row the device received.
  Future<void> _restoreSavedRows(ChannelHeadScope scope) async {
    final filter = _channelWindowFilter(null);
    final session = _summarySession;
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
        stored.then((events) => storeDrew = _paintSaved(scope, events)),
        cached.then((events) => storeDrew ? null : _paintSaved(scope, events)),
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
      for (final event in _windowStore.liveOverlay)
        if (event.threadReference.parentId == null) event,
      // Saved rows shown while a reconnect waits for the relay's window.
      if (_windowStore.allPages.isEmpty)
        for (final event in _lastKnownMessages ?? const <NostrEvent>[])
          if (_cachedHeadIds.contains(event.id)) event,
    ];
    final target = rows.where((event) => event.id == id).firstOrNull;
    if (target == null) return false;
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
      _foldPagesAround(around.pages, around.version, anchor.id);
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
