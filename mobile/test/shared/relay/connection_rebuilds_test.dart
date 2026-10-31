import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/activity/activity_provider.dart';
import 'package:kuzz/features/activity/reminders_provider.dart';
import 'package:kuzz/features/channels/channel.dart';
import 'package:kuzz/features/channels/channels_provider.dart';
import 'package:kuzz/shared/community/community_provider.dart';
import 'package:kuzz/shared/read_state/read_state_manager.dart';
import 'package:kuzz/shared/read_state/read_state_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';
import 'package:kuzz/shared/relay/relay_connection_epoch.dart';
import 'package:kuzz/shared/theme/theme_provider.dart';
import 'package:nostr/nostr.dart' as nostr;
import 'package:shared_preferences/shared_preferences.dart';

/// Launch and reconnect must load read state, the activity feed and
/// reminders once per usable connection, not once per session status step.
///
/// The session is driven through the states the real session emits: at
/// launch disconnected (build), connecting, connected (after AUTH); on a
/// drop reconnecting(1), the same state again when the attempt starts, a
/// failed attempt reconnecting(2) (and again), then connected.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late String nsec;
  late String myPk;
  late SharedPreferences prefs;

  setUp(() async {
    SharedPreferences.setMockInitialValues({});
    prefs = await SharedPreferences.getInstance();
    final keys = nostr.Keys.generate();
    nsec = keys.nsec;
    myPk = keys.public;
  });

  ProviderContainer containerFor(_SteppedSession session) {
    final container = ProviderContainer(
      overrides: [
        savedPrefsProvider.overrideWithValue(prefs),
        relayConfigProvider.overrideWith(() => _Config(nsec)),
        relaySessionProvider.overrideWith(() => session),
        activeCommunityProvider.overrideWith((ref) async => null),
        appLifecycleProvider.overrideWith(_Resumed.new),
        channelsProvider.overrideWith(_NoChannels.new),
      ],
    );
    addTearDown(container.dispose);
    return container;
  }

  Future<void> settle() async {
    for (var i = 0; i < 20; i++) {
      await Future<void>.delayed(Duration.zero);
    }
  }

  Future<void> step(_SteppedSession session, SessionState next) async {
    session.step(next);
    await settle();
  }

  // New instances, as the real session builds them: an equal state set
  // again must not count as a change.
  SessionState state(SessionStatus status, [int attempt = 0]) =>
      SessionState(status: status, reconnectAttempt: attempt);

  Future<void> launch(_SteppedSession session) async {
    await step(session, state(SessionStatus.connecting));
    await step(session, state(SessionStatus.connected));
  }

  Future<void> reconnect(_SteppedSession session) async {
    await step(session, state(SessionStatus.reconnecting, 1));
    await step(session, state(SessionStatus.reconnecting, 1));
    await step(session, state(SessionStatus.reconnecting, 2));
    await step(session, state(SessionStatus.reconnecting, 2));
    await step(session, state(SessionStatus.connected));
  }

  test('equal session states are equal', () {
    expect(
      const SessionState(
        status: SessionStatus.reconnecting,
        reconnectAttempt: 2,
      ),
      const SessionState(
        status: SessionStatus.reconnecting,
        reconnectAttempt: 2,
      ),
    );
    expect(
      const SessionState(
        status: SessionStatus.reconnecting,
        reconnectAttempt: 1,
      ),
      isNot(
        const SessionState(
          status: SessionStatus.reconnecting,
          reconnectAttempt: 2,
        ),
      ),
    );
  });

  test('the connection epoch rises once per usable connection', () async {
    final session = _SteppedSession();
    final container = containerFor(session);
    final epochs = <int>[];
    container.listen(
      relayConnectionEpochProvider,
      (_, next) => epochs.add(next),
      fireImmediately: true,
    );

    await launch(session);
    await reconnect(session);

    expect(epochs, [0, 1, 2]);
  });

  test('read state fetches and subscribes once per connection', () async {
    final session = _SteppedSession();
    final container = containerFor(session);
    container.listen(readStateProvider, (_, _) {});
    await container.read(activeCommunityProvider.future);
    await settle();

    // Not connected yet: a fetch would be dropped; the read state is local.
    expect(session.readStateFetches, 0);
    expect(container.read(readStateProvider).isReady, isTrue);

    await launch(session);
    expect(session.readStateFetches, 1);
    expect(session.readStateSubscribes, 1);
    expect(session.liveReadStateSubscriptions, 1);

    await reconnect(session);
    expect(session.readStateFetches, 2);
    expect(session.readStateSubscribes, 2);
    expect(session.liveReadStateSubscriptions, 1);
  });

  test('read state keeps forced-unread flags across a reconnect', () async {
    final session = _SteppedSession();
    final container = containerFor(session);
    container.listen(readStateProvider, (_, _) {});
    await container.read(activeCommunityProvider.future);
    await launch(session);

    container
        .read(readStateProvider.notifier)
        .markContextUnread('chan-1', channelId: 'chan-1');
    await reconnect(session);

    expect(container.read(readStateProvider).isForcedUnread('chan-1'), isTrue);
  });

  test('a reconnect overlapping the first load keeps one live '
      'read-state subscription', () async {
    final session = _SteppedSession(
      initial: const SessionState(status: SessionStatus.connected),
    );
    final gate = Completer<void>();
    session.readStateFetchGate = gate;
    final crypto = ReadStateCrypto.tryCreate(nsec: nsec, pubkey: myPk)!;
    final manager = ReadStateManager(
      pubkey: myPk,
      prefs: prefs,
      crypto: crypto,
      relaySession: session,
      signedEventRelay: null,
      remoteEnabled: true,
      onChanged: () {},
    );
    addTearDown(manager.dispose);

    final first = manager.initialize();
    await settle();
    final second = manager.reinitializeRemote();
    await settle();
    session.readStateFetchGate = null;
    gate.complete();
    await Future.wait([first, second]);
    await settle();

    expect(session.readStateFetches, 2);
    expect(session.liveReadStateSubscriptions, 1);
  });

  test('activity fetches before connecting, then once per connection, '
      'and subscribes once per connection', () async {
    final session = _SteppedSession();
    final container = containerFor(session);
    container.listen(activityProvider, (_, _) {});
    await container.read(channelsProvider.future);
    await settle();
    // Before connecting it loads over HTTP (again when the channel list
    // resolves, a real change), without live subscriptions.
    final beforeConnecting = session.activityQueries;
    expect(beforeConnecting, greaterThan(0));
    expect(session.activitySubscribes, 0);

    await launch(session);
    expect(session.activityQueries, beforeConnecting + 1);
    expect(session.activitySubscribes, 1);
    expect(session.liveActivitySubscriptions, 1);

    await reconnect(session);
    expect(session.activityQueries, beforeConnecting + 2);
    expect(session.activitySubscribes, 2);
    expect(session.liveActivitySubscriptions, 1);
  });

  test('reminders fetch once per connection', () async {
    final session = _SteppedSession();
    final container = containerFor(session);
    container.listen(remindersProvider, (_, _) {});
    await settle();
    expect(session.reminderFetches, 1);

    await launch(session);
    expect(session.reminderFetches, 2);

    await reconnect(session);
    expect(session.reminderFetches, 3);
  });
}

class _Config extends RelayConfigNotifier {
  _Config(this.nsec);

  final String nsec;

  @override
  RelayConfig build() => RelayConfig(baseUrl: 'http://localhost:1', nsec: nsec);
}

class _Resumed extends AppLifecycleNotifier {
  @override
  AppLifecycleState build() => AppLifecycleState.resumed;
}

class _NoChannels extends ChannelsNotifier {
  @override
  bool get hasLoaded => true;

  @override
  Set<String> get hiddenDmIds => const {};

  @override
  Future<List<Channel>> build() async => const [];
}

/// Session whose state the test steps; counts the requests each provider
/// sends and the live subscriptions still open.
class _SteppedSession extends RelaySessionNotifier {
  _SteppedSession({
    this.initial = const SessionState(status: SessionStatus.disconnected),
  });

  final SessionState initial;
  Completer<void>? readStateFetchGate;
  int readStateFetches = 0;
  int readStateSubscribes = 0;
  int liveReadStateSubscriptions = 0;
  int activityQueries = 0;
  int activitySubscribes = 0;
  int liveActivitySubscriptions = 0;
  int reminderFetches = 0;

  @override
  SessionState build() => initial;

  void step(SessionState next) => state = next;

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    if (filter.kinds.contains(EventKind.readState)) {
      readStateFetches++;
      final gate = readStateFetchGate;
      if (gate != null) await gate.future;
    } else if (filter.kinds.contains(kindEventReminder)) {
      reminderFetches++;
    }
    return const [];
  }

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    activityQueries++;
    return const [];
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
  }) async {
    if (filter.kinds.contains(EventKind.readState)) {
      readStateSubscribes++;
      liveReadStateSubscriptions++;
      var open = true;
      return () {
        if (open) liveReadStateSubscriptions--;
        open = false;
      };
    }
    if (filter.tags.containsKey('#p')) {
      activitySubscribes++;
      liveActivitySubscriptions++;
      var open = true;
      return () {
        if (open) liveActivitySubscriptions--;
        open = false;
      };
    }
    return () {};
  }
}
