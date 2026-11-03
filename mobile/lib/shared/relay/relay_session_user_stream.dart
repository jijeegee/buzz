part of 'relay_session.dart';

final _userStreams = Expando<UserStream>();

/// The longest the first chunk replay waits for the stream REQ.
const _userStreamFirstWait = Duration(milliseconds: 300);

/// The session's side of the user stream (docs/message-sync.md#user-stream):
/// the stream sub never enters [_liveSubscriptions], so no generic path
/// (replay, coverage, CLOSED retry) sees `$joined` as a room.
extension RelaySessionUserStream on RelaySessionNotifier {
  UserStream get _userStream => _userStreams[this] ??= _newUserStream();

  UserStream _newUserStream() {
    final stream = UserStream(
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
        queueEpoch: (epoch) =>
            _messageStore?.queueStreamEpoch(_storeLease, epoch),
        capture: () => _messageStore?.capture(_storeLease)?.sequence,
        storedMark: () =>
            _messageStore?.storedMark(_storeLease) ?? Future.value(),
        extendCoverage: (rooms, from, to, captured) => _messageStore
            ?.extendCoverage(_storeLease, rooms, from, to, captured: captured),
        catchUp: _runUserStreamCatchUp,
        readLastResult: () =>
            switch ((_userStreamPrefs, _userStreamResultKey)) {
              (final prefs?, final key?) => prefs.getString(key),
              _ => null,
            },
        writeLastResult: (result) {
          final (prefs, key) = (_userStreamPrefs, _userStreamResultKey);
          if (prefs == null || key == null) return;
          unawaited(
            result == null ? prefs.remove(key) : prefs.setString(key, result),
          );
        },
        reportPlatform: defaultTargetPlatform == TargetPlatform.iOS
            ? 'ios'
            : null,
        postReport: (counters) => unawaited(_postUserStreamReport(counters)),
      ),
    );
    _storeCoverage.hooks = stream.hooks;
    return stream;
  }

  /// Preferences are already loaded at startup, so the last result is read
  /// without the store (whose first open is what the wait is about).
  SharedPreferences? get _userStreamPrefs => _prefs;

  /// One key per account and relay.
  String? get _userStreamResultKey => switch (_storeLease) {
    final lease? => 'user_stream_last_result:${lease.stem}:${lease.account}',
    null => null,
  };

  Future<UserStreamCatchUpResult?> _runUserStreamCatchUp(
    UserStreamCatchUpJob job,
    bool Function(String id) receivedLive,
  ) async {
    final store = _messageStore;
    final lease = _storeLease;
    if (store == null || lease == null) return null;
    bool stopped() => job.stopped?.call() ?? false;
    return UserStreamCatchUp(
      scope: (store: store, lease: lease),
      // Each page waits out the session's rate-limit window, and none goes
      // out once the catch-up's connection ended.
      query: (filters) async {
        if (_rateLimitGate.isActive) await _rateLimitGate.wait();
        if (stopped()) throw StateError('user stream catch-up stopped');
        return queryRelayJson(filters);
      },
      switched: () => _storeLease != lease,
      receivedLive: receivedLive,
    ).run(job);
  }

  /// iOS has no log anyone can read: its report lines go to the relay
  /// (plan U1d, `POST /api/user-stream/shadow-report`). Best effort: a relay
  /// without the endpoint answers 404 and nothing is retried.
  Future<void> _postUserStreamReport(UserStreamCounters counters) async {
    if (_disposed || _ageRestricted) return;
    try {
      final (config, tokens) = _httpAuth;
      final url = Uri.parse(
        config.baseUrl,
      ).resolve(userStreamReportPath).toString();
      final bodyBytes = utf8.encode(
        jsonEncode(
          userStreamReportBody(
            counters,
            platform: 'ios',
            appVersion: await appVersion(),
          ),
        ),
      );
      if (bodyBytes.length > userStreamReportMaxBytes || _disposed) return;
      final response = await sendRelayAuthorized(
        config,
        tokens,
        (authorization) => _httpQueryClient.post(
          Uri.parse(url),
          headers: {
            'Authorization': authorization,
            'Content-Type': 'application/json',
          },
          body: bodyBytes,
          timeout: const Duration(seconds: 8),
        ),
        nip98: () => buildNip98AuthHeader(
          method: 'POST',
          url: url,
          bodyBytes: bodyBytes,
          nsec: config.nsec,
        ),
      );
      if (response.statusCode >= 300) {
        debugPrint('[user-stream] report POST: ${response.statusCode}');
      }
    } catch (error) {
      debugPrint('[user-stream] report POST failed: $error');
    }
  }

  /// The stream REQ goes ahead of the chunk replay: while the first
  /// connect's state read is still running, the replay waits for it at most
  /// [_userStreamFirstWait]. With the stream switched off, or refused by
  /// this relay on the last connection, nothing waits.
  Future<void> _userStreamReqFirst(Future<void> connected) async {
    if (!_userStream.loadingState || _userStream.relayKnownOff) return;
    await connected.timeout(_userStreamFirstWait, onTimeout: () {});
  }

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
    if (data.length >= 3 &&
        data[0] == 'EVENT' &&
        data[2] is Map<String, dynamic>) {
      final subscription = _liveSubscriptions[data[1]];
      if (subscription != null &&
          storeCoveredRooms(subscription.filter) != null) {
        final event = data[2] as Map<String, dynamic>;
        // The new-row rule leaves out rows received live (any mode).
        stream.noteLiveEvent(event['id']);
        if (stream.phase == UserStreamPhase.shadow) {
          stream.noteChunkEvent('${event['id']}', userStreamRoomOf(event));
        }
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
