import 'dart:async';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';

import 'relay_session.dart';

/// Tracks the app lifecycle state and drives websocket connect/disconnect
/// behavior for mobile battery efficiency.
class AppLifecycleNotifier extends Notifier<AppLifecycleState> {
  AppLifecycleListener? _listener;
  StreamSubscription<List<ConnectivityResult>>? _connectivitySub;
  final _networkChange = NetworkChangeFilter();

  @override
  AppLifecycleState build() {
    _listener = AppLifecycleListener(onStateChange: _onStateChange);

    // Watch connectivity changes to trigger reconnect on network restore.
    _connectivitySub = Connectivity().onConnectivityChanged.listen((results) {
      final hasNetwork = results.any((r) => r != ConnectivityResult.none);
      final session = ref.read(relaySessionProvider.notifier);
      // A new network ends the user stream's overflow backoff.
      if (_networkChange.changed(results)) {
        session.noteUserStreamNetworkChange();
      }
      if (hasNetwork && state == AppLifecycleState.resumed) {
        session.onAppResumed();
      }
    });

    ref.onDispose(() {
      _listener?.dispose();
      _connectivitySub?.cancel();
    });

    return AppLifecycleState.resumed;
  }

  void _onStateChange(AppLifecycleState newState) {
    state = newState;

    final session = ref.read(relaySessionProvider.notifier);
    switch (newState) {
      case AppLifecycleState.resumed:
        session
          ..noteUserStreamResume()
          ..onAppResumed();
      case AppLifecycleState.paused:
      case AppLifecycleState.detached:
        session.onAppPaused();
      case AppLifecycleState.inactive:
      case AppLifecycleState.hidden:
        break; // Brief transition states — no action.
    }
  }
}

final appLifecycleProvider =
    NotifierProvider<AppLifecycleNotifier, AppLifecycleState>(
      AppLifecycleNotifier.new,
    );

/// Which connectivity events are a real network change for the user
/// stream's overflow backoff (docs/message-sync.md#relay-answers): never
/// the first event after start (Android emits the current state when the
/// listener is registered, so an app restart would end the backoff), never
/// a repeat of the same network types (capability updates), never "no
/// network".
class NetworkChangeFilter {
  Set<ConnectivityResult>? _last;

  bool changed(List<ConnectivityResult> results) {
    final now = results.where((r) => r != ConnectivityResult.none).toSet();
    final last = _last;
    _last = now;
    if (last == null || now.isEmpty) return false;
    return !setEquals(last, now);
  }
}
