part of 'relay_session.dart';

final _userStreams = Expando<UserStream>();

/// The session's side of the user stream (docs/message-sync.md#user-stream):
/// the stream sub never enters [_liveSubscriptions], so no generic path
/// (replay, coverage, CLOSED retry) sees `$joined` as a room.
extension RelaySessionUserStream on RelaySessionNotifier {
  UserStream get _userStream => _userStreams[this] ??= UserStream(
    UserStreamHost(
      nextSubId: () => _nextSubId('l'),
      send: (frame) => _socket?.send(frame),
      isActive: (generation) =>
          _isActiveConnection(generation) && _socketConnected,
      store: (event) => _messageStore?.submit(_storeLease, [event]),
      closeSocket: _closeSocketForUserStream,
      scope: () => _storeLease,
      readState: (keys) async =>
          await _messageStore?.readSyncState(_storeLease, keys) ?? const {},
      writeState: (values) =>
          unawaited(_messageStore?.writeSyncState(_storeLease, values)),
      chunkRooms: _userStreamChunkRooms,
      nowMs: () => _now().millisecondsSinceEpoch,
      timer: _retryTimerFactory,
    ),
  );

  /// The stream state, for the room list and tests.
  UserStream get userStream => _userStream;

  /// The reconnect delay, with the `lagged` jitter once after one.
  int get _nextDelayMs =>
      _reconnectDelayMs + _userStream.takeReconnectJitterMs();

  /// A relay frame: true when it is the stream's, which nothing else sees
  /// (not even the store, which the stream decides for itself).
  bool _userStreamOwns(List<dynamic> data) {
    final stream = _userStream;
    if (stream.onMessage(data)) return true;
    if (stream.phase == UserStreamPhase.shadow &&
        data.length >= 3 &&
        data[0] == 'EVENT' &&
        data[2] is Map<String, dynamic>) {
      final subscription = _liveSubscriptions[data[1]];
      if (subscription != null &&
          storeCoveredRooms(subscription.filter) != null) {
        final event = data[2] as Map<String, dynamic>;
        stream.noteChunkEvent('${event['id']}', userStreamRoomOf(event));
      }
    }
    return false;
  }

  Set<String> _userStreamChunkRooms() => {
    for (final subscription in _liveSubscriptions.values)
      ...?storeCoveredRooms(subscription.filter),
  };

  /// After an overflow or `lagged` CLOSED the relay cancels the socket; if
  /// it is still open after the grace the client closes it and reconnects.
  void _closeSocketForUserStream(int generation) {
    if (!_isActiveConnection(generation)) return;
    final socket = _socket;
    _handleDisconnected(generation, null);
    unawaited(socket?.disconnect());
  }

  /// The network changed: the stream's overflow backoff ends.
  void noteUserStreamNetworkChange() => _userStream.networkChanged();

  /// Call before [onAppResumed]: a resume after an hour in the background
  /// ends the overflow backoff.
  void noteUserStreamResume() {
    final since = _backgroundedAt;
    if (since == null) return;
    _userStream.resumed(_now().difference(since).inSeconds);
  }
}
