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
}
