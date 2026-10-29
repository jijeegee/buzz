part of 'relay_session.dart';

/// A build with `--dart-define=KUZZ_STORE_OPEN=off` opens everything from the
/// relay.
const storeOpensEnabled = String.fromEnvironment('KUZZ_STORE_OPEN') != 'off';

/// Opening rooms and threads from the device store
/// (docs/message-sync.md#opening-a-room-from-the-store,
/// docs/message-sync.md#opening-a-thread-from-the-store).
extension RelaySessionStore on RelaySessionNotifier {
  /// Keeps a `/query` answer and compares the relay's clock with this one's.
  void _storeQueryAnswer(
    MessageStoreLease? lease,
    Object? decoded,
    String? date,
  ) {
    noteServerDate(date, DateTime.now().millisecondsSinceEpoch ~/ 1000);
    if (decoded is List) _messageStore?.submit(lease, decoded);
  }

  /// A capture of the store scope (server file and sign-in) events are kept
  /// under now. Taken before a membership fetch whose list may forget rooms
  /// (docs/message-sync.md#forgetting-a-room).
  StoreCapture? get storeScope => _messageStore?.capture(_storeLease);

  /// Applies [keep], a full membership list fetched from [scope] on, to the
  /// stored rooms; refused (false) when that scope is no longer current or
  /// the list is empty (docs/message-sync.md#forgetting-a-room).
  bool forgetStoredRoomsExcept(StoreCapture? scope, Set<String> keep) =>
      _messageStore?.forgetRoomsExcept(scope, _storeLease, keep) ?? false;

  /// Notes the signing key (NIP-11 `self`) of [scope]'s server, while that
  /// scope is still current: only its notices may forget a room.
  void noteStoreRelayKey(StoreCapture? scope, String? key) {
    if (scope == null || scope.lease != _storeLease) return;
    _messageStore?.noteRelayKey(scope.lease, key);
  }

  /// When [roomId]'s live REQ reached EOSE on this connection, if it has
  /// been receiving every event of the room since.
  int? _storeLiveAt(String roomId) {
    int? liveAt;
    for (final subscription in _liveSubscriptions.values) {
      if (!(storeCoveredRooms(subscription.filter)?.contains(roomId) ??
          false)) {
        continue;
      }
      final at = _storeCoverage.liveAtOf(subscription, _connectionGeneration);
      if (at != null && (liveAt == null || at < liveAt)) liveAt = at;
    }
    return liveAt;
  }

  /// [rootId]'s newest replies in [roomId] from the store, in the shape of a
  /// relay thread-window response, or `null` unless it is certain.
  Future<List<NostrEvent>?> certainThreadHead(
    String roomId,
    String rootId, {
    required List<int> kinds,
    required int limit,
  }) async {
    final store = _messageStore;
    final lease = _storeLease;
    final liveAt = _storeLiveAt(roomId);
    if (store == null || lease == null || liveAt == null) return null;
    if (!storeOpensEnabled || !clockTrusted) return null;
    final events = await store.certainThreadHead(
      lease,
      room: roomId,
      root: rootId,
      kinds: kinds,
      limit: limit,
      liveAt: liveAt,
    );
    return _storedEvents(events);
  }

  List<NostrEvent>? _storedEvents(List<Map<String, dynamic>>? events) {
    if (events == null) return null;
    try {
      return events.map(NostrEvent.fromJson).toList();
    } catch (_) {
      return null;
    }
  }

  /// [roomId]'s newest window from the store, in the shape of a relay
  /// channel-window response, or `null` unless it is certain: a live REQ
  /// carrying the room reached EOSE on this connection and the store covers
  /// every row since.
  Future<List<NostrEvent>?> certainRoomHead(
    String roomId, {
    required List<int> kinds,
    required int limit,
  }) async {
    final store = _messageStore;
    final lease = _storeLease;
    final liveAt = _storeLiveAt(roomId);
    if (store == null || lease == null || liveAt == null) return null;
    if (!storeOpensEnabled || !clockTrusted) return null;
    final events = await store.certainRoomHead(
      lease,
      room: roomId,
      kinds: kinds,
      limit: limit,
      liveAt: liveAt,
    );
    return _storedEvents(events);
  }

  /// [filters] sent through [queryRelay], recording the head span of each
  /// newest room or thread window they ask for whose room's live REQs were
  /// live as the request went out and still are when it is answered
  /// (docs/message-sync.md#head-spans). The request is marked in the store
  /// first, so rows written after it are never taken for deleted ones.
  Future<List<NostrEvent>> queryRelayWithStoreHeads(
    List<NostrFilter> filters,
  ) async {
    final sent = [for (final filter in filters) storeHeadFilter(filter)];
    final windows = [for (final filter in sent) ?storeHeadWindowOf(filter)];
    final generation = _connectionGeneration;
    final tokens = <String, List<(String, _LiveSubscription, Object)>>{};
    for (final roomId in {for (final window in windows) window.roomId}) {
      final token = [
        for (final MapEntry(key: subId, value: subscription)
            in _liveSubscriptions.entries)
          if (storeCoveredRooms(subscription.filter)?.contains(roomId) ?? false)
            if (_storeCoverage.liveTokenOf(subscription, generation)
                case final live?)
              (subId, subscription, live),
      ];
      if (token.isNotEmpty) tokens[roomId] = token;
    }
    final store = _messageStore;
    final capture = tokens.isEmpty
        ? null
        : store?.headSent(_storeLease, tokens.keys.toList());
    if (store == null || capture == null) return queryRelay(sent);
    var heads = <StoreHeadSpan>[];
    try {
      final events = await queryRelay(sent);
      bool holds(String roomId) =>
          generation == _connectionGeneration &&
          tokens[roomId]!.every(
            (part) =>
                _liveSubscriptions[part.$1] == part.$2 &&
                _storeCoverage.holds(part.$2, part.$3, generation),
          );
      heads = [
        for (final answer in storeHeadAnswers(windows, events))
          if (tokens.containsKey(answer.window.roomId) &&
              holds(answer.window.roomId))
            StoreHeadSpan(
              room: answer.window.roomId,
              root: answer.window.rootId,
              kinds: answer.window.kinds,
              liveAt: tokens[answer.window.roomId]!
                  .map((part) => _storeCoverage.liveAtOfToken(part.$3))
                  .reduce(max),
              oldest: answer.nextCursor,
              ids: answer.ids,
            ),
      ];
      return events;
    } finally {
      // An empty list only releases the mark.
      store.recordHeads(capture, _storeLease, heads);
    }
  }
}
