part of 'relay_session.dart';

/// Reconnect replay and CLOSED retry for live REQs
/// (docs/message-sync.md#reconnect).
extension RelaySessionReplay on RelaySessionNotifier {
  /// Replay all live subscriptions after a reconnect, with a time skew to
  /// catch events that occurred during the disconnect.
  Future<void> _replayLiveSubscriptions(int generation) async {
    if (_rateLimitGate.isActive) await _rateLimitGate.wait();
    if (!_isActiveConnection(generation)) return;

    final entries = _liveSubscriptions.entries.toList();
    final visibleChannelId = _visibleChannelsByOwner.isEmpty
        ? null
        : _visibleChannelsByOwner.values.last;
    if (visibleChannelId != null) {
      entries.sort((left, right) {
        final leftVisible =
            left.value.filter.tags['#h']?.contains(visibleChannelId) ?? false;
        final rightVisible =
            right.value.filter.tags['#h']?.contains(visibleChannelId) ?? false;
        if (leftVisible == rightVisible) return 0;
        return leftVisible ? -1 : 1;
      });
    }

    await _sendReplayBatches(entries, generation);
  }

  Future<void> _replayPendingClosedRetries(int generation) async {
    if (!_isActiveConnection(generation)) return;
    final entries = _pendingClosedRetries.entries
        .where((entry) => entry.value.generation == generation)
        .map(
          (entry) => MapEntry<String, _LiveSubscription>(
            entry.key,
            entry.value.subscription,
          ),
        )
        .toList();
    await _sendReplayBatches(entries, generation, pendingClosedRetries: true);
  }

  Future<void> _sendReplayBatches(
    List<MapEntry<String, _LiveSubscription>> entries,
    int generation, {
    bool pendingClosedRetries = false,
  }) async {
    for (
      var i = 0;
      i < entries.length;
      i += RelaySessionNotifier._replayBatchSize
    ) {
      if (_rateLimitGate.isActive) await _rateLimitGate.wait();
      if (!_isActiveConnection(generation)) return;
      final batch = entries.sublist(
        i,
        min(i + RelaySessionNotifier._replayBatchSize, entries.length),
      );
      for (final entry in batch) {
        if (_liveSubscriptions[entry.key] != entry.value) continue;
        if (pendingClosedRetries) {
          final pendingRetry = _pendingClosedRetries[entry.key];
          if (pendingRetry?.subscription != entry.value ||
              pendingRetry?.generation != generation) {
            continue;
          }
          _pendingClosedRetries.remove(entry.key);
        }
        _sendLiveReq(entry.key, entry.value, _replayFilter(entry.value));
      }
      if (i + RelaySessionNotifier._replayBatchSize < entries.length) {
        await _replayDelay(RelaySessionNotifier._replayInterBatchDelay);
      }
    }
  }

  NostrFilter _replayFilter(_LiveSubscription subscription) {
    final since = subscription.lastSeenCreatedAt;
    return since == null || subscription.filter.limit == 0
        ? subscription.filter
        : subscription.filter.copyWithSince(
            max(0, since - RelaySessionNotifier._reconnectReplaySkewSeconds),
          );
  }

  /// A live REQ the relay CLOSED: terminal ends it (`onClosed`), anything
  /// else retries the same filter under backoff.
  void _handleLiveClosed(
    String subId,
    _LiveSubscription liveSub,
    String message,
  ) {
    final closedClass = classifyRelayClosed(message);
    _storeCoverage.noteClosed(liveSub);
    final readyCompleter = liveSub.readyCompleter;
    // A deadline is terminal for one-shot history, but a live subscription
    // retries it: same filter under backoff, which can keep retrying at the
    // cap. `since` is not advanced, so events missed while closed aren't
    // skipped.
    if (closedClass == RelayClosedClass.terminal &&
        !isRelayDeadlineError(message)) {
      if (readyCompleter != null && !readyCompleter.isCompleted) {
        readyCompleter.completeError(Exception(message));
      }
      liveSub.onClosed?.call(message);
      _removeLiveSubscription(subId, liveSub);
      return;
    }
    // Its retry is the next REQ in flight and carries the newest filters.
    liveSub.multi?.lost();
    if (readyCompleter != null && !readyCompleter.isCompleted) {
      readyCompleter.complete();
      liveSub.readyCompleter = null;
    }
    liveSub.onStatusChanged?.call(RelaySubscriptionStatus.retrying);
    if (liveSub.closedRetryTimer != null) return;

    final attempt = liveSub.closedRetryAttempt;
    final backoffMs = attempt >= 5
        ? RelaySessionNotifier._maxReconnectDelayMs
        : RelaySessionNotifier._baseReconnectDelayMs * (1 << attempt);
    var delayMs = backoffMs;
    if (closedClass == RelayClosedClass.rateLimited) {
      final retrySeconds = parseRateLimitRetrySeconds(message);
      _rateLimitGate.activate(retrySeconds);
      final fallbackMs =
          (retrySeconds != null
              ? min(retrySeconds, RelayRateLimitGate.maxRetrySeconds)
              : RelayRateLimitGate.defaultRetrySeconds) *
          1000;
      delayMs = max(
        backoffMs,
        _rateLimitGate.remainingMs() == 0
            ? fallbackMs
            : _rateLimitGate.remainingMs(),
      );
    }

    liveSub.closedRetryAttempt = attempt + 1;
    final retryGeneration = _connectionGeneration;
    liveSub.closedRetryTimer = _retryTimerFactory(
      Duration(milliseconds: delayMs),
      () async {
        liveSub.closedRetryTimer = null;
        if (!_isActiveConnection(retryGeneration) ||
            _liveSubscriptions[subId] != liveSub) {
          return;
        }
        if (_rateLimitGate.isActive) await _rateLimitGate.wait();
        if (!_isActiveConnection(retryGeneration) ||
            _liveSubscriptions[subId] != liveSub ||
            !_socketConnected) {
          return;
        }
        _pendingClosedRetries[subId] = _ClosedRetry(
          subscription: liveSub,
          generation: retryGeneration,
        );
        _scheduleClosedRetryReplay(retryGeneration);
      },
    );
  }

  void _scheduleClosedRetryReplay(int generation) {
    if (_closedRetryReplayScheduled) return;
    _closedRetryReplayScheduled = true;
    scheduleMicrotask(() async {
      try {
        await _replayPendingClosedRetries(generation);
      } finally {
        _closedRetryReplayScheduled = false;
        _pendingClosedRetries.removeWhere(
          (_, retry) => retry.generation != _connectionGeneration,
        );
        if (_pendingClosedRetries.values.any(
          (retry) => retry.generation == _connectionGeneration,
        )) {
          _scheduleClosedRetryReplay(_connectionGeneration);
        }
      }
    });
  }

  void _resetClosedRetry(_LiveSubscription subscription) {
    subscription.closedRetryAttempt = 0;
    subscription.closedRetryTimer?.cancel();
    subscription.closedRetryTimer = null;
  }

  void _cancelAllClosedRetries() {
    _pendingClosedRetries.clear();
    for (final subscription in _liveSubscriptions.values) {
      subscription.closedRetryTimer?.cancel();
      subscription.closedRetryTimer = null;
    }
  }

  void _resetAllClosedRetries() {
    _pendingClosedRetries.clear();
    for (final subscription in _liveSubscriptions.values) {
      _resetClosedRetry(subscription);
    }
  }
}
