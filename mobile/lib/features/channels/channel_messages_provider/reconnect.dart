part of '../channel_messages_provider.dart';

/// Connection changes do not rebuild a room. On reconnect, a room the channel
/// list's live feed covers keeps its rows and takes what it missed from that
/// feed's single catch-up, reloading only when the catch-up (or the message
/// database) says rows may be missing. Other rooms reload their newest page.
extension _RoomReconnect on ChannelMessagesNotifier {
  void _listenForReconnects() {
    final ref = _storeRef;
    ref.listen(relaySessionProvider.select((session) => session.status), (
      previous,
      next,
    ) {
      if (next != SessionStatus.connected) {
        if (previous == SessionStatus.connected) _onDisconnected();
      } else if (previous != SessionStatus.connected) {
        _onReconnected();
      }
    });
    ref.onDispose(
      ref.read(channelRoomStoreProvider).listenCatchUps(_onCatchUp),
    );
  }

  bool get _isFedByChannelList => _storeRef
      .read(channelRoomStoreProvider)
      .feeds(_readHeadScope(), channelId);

  /// Fences an in-flight load; it reruns once the relay is back.
  void _stopInit() {
    if (_initInFlight) _needsInit = true;
    _initVersion++;
    _initInFlight = false;
    _initialWindowQueryInFlight = false;
    _liveSummaryRootsDuringInitialWindowQuery.clear();
  }

  void _onDisconnected() {
    _stopInit();
    _awaitingCatchUp = false;
    _deletionOwnerQueue.pause();
    _summaryRefreshes.pause();
    // A room that reloads on reconnect subscribes again then. A covered
    // room keeps its subscription, which the session replays.
    if (!_isFedByChannelList) _clearSubscription();
  }

  void _onReconnected() {
    final ref = _storeRef;
    final me = ref.read(myPubkeyProvider);
    Future.microtask(() => restoreLocalSends(ref, this, me));
    if (_needsInit || _unsubscribe == null || !_isFedByChannelList) {
      _restart();
      return;
    }
    _awaitingCatchUp = true;
    _summaryRefreshes.resume();
    _deletionOwnerQueue.resume();
  }

  void _onCatchUp(RoomCatchUp catchUp) {
    if (!_awaitingCatchUp ||
        !catchUp.rooms.contains(channelId) ||
        catchUp.scope != _readHeadScope()) {
      return;
    }
    _awaitingCatchUp = false;
    for (final event in catchUp.events[channelId] ?? const <NostrEvent>[]) {
      _handleLiveEvent(event);
    }
    if (catchUp.gaps.contains(channelId)) {
      _restart();
    } else {
      unawaited(_restartIfRoomHasGap(catchUp.scope));
    }
  }

  /// The message database ends a room's live range when rows may be missing
  /// that no catch-up could bring (an interrupted feed, a failed write).
  Future<void> _restartIfRoomHasGap(MessageScope scope) async {
    final initVersion = _initVersion;
    final bool gap;
    try {
      gap = await _storeRef
          .read(channelRoomStoreProvider)
          .hasGap(scope, channelId);
    } catch (error) {
      debugPrint(
        '[ChannelMessagesNotifier] live range unknown for $channelId: $error',
      );
      return;
    }
    if (!_summaryMounted ||
        initVersion != _initVersion ||
        _readHeadScope() != scope ||
        _storeRef.read(relaySessionProvider).status !=
            SessionStatus.connected) {
      return;
    }
    if (gap) _restart();
  }

  /// Loads the newest page as opening the room does; a window opened at a
  /// message pages on from its newest row instead.
  void _restart() {
    final ref = _storeRef;
    _awaitingCatchUp = false;
    if (_storedWindow != _StoredWindow.none) {
      // Painted from the database and not settled yet: keep the rows and
      // ask only for what is missing.
      _initFuture = _init(fromStore: true);
      return;
    }
    if (!_usingChannelWindow || !_windowStore.isMiddle) {
      _reachedOldest = false;
      _windowStore = const ChannelWindowStore.empty();
      // A reconnect window can lag a confirmed thread query and contains only
      // top-level rows. Keep known replies as summary evidence, including
      // their deletion markers so a reset cannot resurrect a deleted reply.
      for (final event in _lastKnownMessages ?? const <NostrEvent>[]) {
        if (event.threadReference.parentId != null ||
            event.kind == EventKind.deletion ||
            event.kind == EventKind.nip29DeleteEvent) {
          _mergeWindowEventIntoStore(event);
        }
      }
      // Unconfirmed own sends (in flight or failed) are in no relay page yet.
      final localSends = splitLocalSendsBySigner(
        ref.read(pendingLocalMessagesProvider(channelId)),
        ref.read(myPubkeyProvider),
      );
      for (final event in localSends.own) {
        if (event.threadReference.parentId == null &&
            EventKind.channelTimelineContentKinds.contains(event.kind)) {
          _windowStore = mergeLiveChannelWindowEvent(
            _windowStore,
            event,
            isTimelineRow: true,
          );
        }
      }
      _usingChannelWindow = false;
    }
    _initialWindowQueryInFlight = false;
    _liveSummaryRootsDuringInitialWindowQuery.clear();
    _initFuture = _init();
  }
}
