import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'relay_session.dart';

/// Counts the relay connections that became usable (connected and
/// authenticated).
///
/// Providers that load once per connection watch this instead of the whole
/// session: the status steps at launch (disconnected, connecting,
/// connected), a drop, and every failed reconnect attempt each emit a new
/// session state, and each would otherwise fetch and subscribe again. The
/// count goes up only when a connection becomes usable, so a watcher loads
/// once per connection, including after the session restarts (its live
/// subscriptions are gone then).
final relayConnectionEpochProvider =
    NotifierProvider<RelayConnectionEpochNotifier, int>(
      RelayConnectionEpochNotifier.new,
    );

class RelayConnectionEpochNotifier extends Notifier<int> {
  @override
  int build() {
    ref.listen(relaySessionProvider, (previous, next) {
      if (next.status == SessionStatus.connected &&
          previous?.status != SessionStatus.connected) {
        state = state + 1;
      }
    });
    return ref.read(relaySessionProvider).status == SessionStatus.connected
        ? 1
        : 0;
  }
}
