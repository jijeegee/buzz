part of 'relay_session.dart';

/// Busy retries so far, per history request.
final _busyAttempts = Expando<int>();

/// The sub id a retried history request runs under now.
final _busyIds = Expando<String>();

/// `CLOSED "error: busy"` on a history REQ: the relay kept a live/gated
/// stream connection's send buffer from filling. Retried 1 s doubling to
/// 30 s, each time on a fresh sub id so no late frame of the refused REQ is
/// taken for the retry's, until the request's own timeout or the socket
/// ends (docs/message-sync.md#relay-answers, `busyRetry`).
extension RelaySessionBusyRetry on RelaySessionNotifier {
  /// The sub id [subscription], first sent as [first], runs under now.
  String _historyIdOf(_HistorySubscription subscription, String first) =>
      _busyIds[subscription] ?? first;

  /// Re-keys [subscription] for a later retry when [message] is a busy
  /// refusal; false otherwise.
  bool _retryBusyHistory(_HistorySubscription subscription, String message) {
    if (message != 'error: busy') return false;
    final attempt = _busyAttempts[subscription] ?? 0;
    const delays = userStreamBusyRetryDelaysSecs;
    final delay = delays[min(attempt, delays.length - 1)];
    _busyAttempts[subscription] = attempt + 1;
    final subId = _nextSubId('h');
    _busyIds[subscription] = subId;
    subscription
      ..sent = false
      ..events.clear();
    _historySubscriptions[subId] = subscription;
    final generation = _connectionGeneration;
    _retryTimerFactory(Duration(seconds: delay), () {
      if (!_isActiveConnection(generation) || !_socketConnected) return;
      if (_historySubscriptions[subId] != subscription || subscription.sent) {
        return;
      }
      _sendHistoryReq(subId, subscription);
    });
    return true;
  }
}
