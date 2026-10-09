import 'package:buzz/shared/relay/relay.dart';

/// A connected relay session that records subscriptions and publishes, and
/// lets a test deliver live events to every subscription.
class FakeOwnDeviceSession extends RelaySessionNotifier {
  final List<NostrFilter> filters = [];
  final List<void Function(NostrEvent)> _listeners = [];
  final List<NostrEvent> published = [];

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    filters.add(filter);
    _listeners.add(onEvent);
    return () => _listeners.remove(onEvent);
  }

  @override
  Future<NostrEvent> publish(
    NostrEvent event, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    published.add(event);
    return event;
  }

  void emit(NostrEvent event) {
    for (final listener in List.of(_listeners)) {
      listener(event);
    }
  }
}

/// A token community whose principal is [principal].
class FixedTokenRelayConfig extends RelayConfigNotifier {
  FixedTokenRelayConfig(this.principal);

  final String principal;

  @override
  RelayConfig build() => RelayConfig(
    baseUrl: 'https://relay.test',
    tokenAuth: true,
    principalId: principal,
  );
}
