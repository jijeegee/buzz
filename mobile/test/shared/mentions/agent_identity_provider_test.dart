import 'dart:convert';

import 'package:fake_async/fake_async.dart';

import 'package:flutter_test/flutter_test.dart';
import 'package:hooks_riverpod/hooks_riverpod.dart';
import 'package:kuzz/features/channels/agent_activity/working_bots_provider.dart';
import 'package:kuzz/features/channels/channel_management_provider.dart';
import 'package:kuzz/features/channels/room_live_router.dart';
import 'package:kuzz/shared/mentions/agent_identity_provider.dart';
import 'package:kuzz/shared/relay/relay.dart';

void main() {
  test('refreshes channel bot roles from membership notices on the room '
      'bundle, with no REQ', () async {
    final relaySession = _MembershipRelaySessionNotifier([
      _membershipEvent(role: 'bot'),
      _membershipEvent(role: 'member'),
    ]);
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => relaySession)],
    );
    addTearDown(container.dispose);
    final keepAlive = container.listen(
      channelBotPubkeysProvider(_channelId),
      (_, _) {},
      fireImmediately: true,
    );
    addTearDown(keepAlive.close);

    expect(await container.read(channelBotPubkeysProvider(_channelId).future), {
      _agentPubkey,
    });

    _routeNotice(container, 'member_left');
    await _pumpEventQueue();

    expect(
      await container.read(channelBotPubkeysProvider(_channelId).future),
      isEmpty,
    );
    expect(relaySession.liveFilters, isEmpty);
  });

  test('refreshes channel members from membership notices', () async {
    final relaySession = _MembershipRelaySessionNotifier([
      _membershipEvent(role: 'bot'),
      _membershipEvent(role: 'member'),
    ]);
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => relaySession)],
    );
    addTearDown(container.dispose);
    final keepAlive = container.listen(
      channelMembersProvider(_channelId),
      (_, _) {},
      fireImmediately: true,
    );
    addTearDown(keepAlive.close);

    expect(
      (await container.read(
        channelMembersProvider(_channelId).future,
      )).single.role,
      'bot',
    );

    _routeNotice(container, 'member_joined');
    await _pumpEventQueue();

    expect(
      (await container.read(
        channelMembersProvider(_channelId).future,
      )).single.role,
      'member',
    );
    expect(relaySession.liveFilters, isEmpty);
  });

  for (final type in membershipNoticeTypes) {
    test('a $type notice moves the version at 0, 1 and 4 s', () {
      fakeAsync((async) {
        final container = _coveredContainer();
        final keepAlive = container.listen(
          channelMembershipUpdateProvider(_channelId),
          (_, _) {},
          fireImmediately: true,
        );
        int version() =>
            container.read(channelMembershipUpdateProvider(_channelId)).version;

        _routeNotice(container, type);
        async.flushMicrotasks();
        expect(version(), 1);
        async.elapse(const Duration(milliseconds: 999));
        expect(version(), 1);
        async.elapse(const Duration(milliseconds: 1));
        expect(version(), 2);
        async.elapse(const Duration(seconds: 3));
        expect(version(), 3);
        async.elapse(const Duration(seconds: 10));
        expect(version(), 3);

        keepAlive.close();
        container.dispose();
      });
    });
  }

  test('other notices, other kinds and other rooms leave the version', () {
    fakeAsync((async) {
      final container = _coveredContainer();
      final keepAlive = container.listen(
        channelMembershipUpdateProvider(_channelId),
        (_, _) {},
        fireImmediately: true,
      );
      final router = container.read(roomLiveRouterProvider);
      router
        ..routeBundleEvent(_notice('topic_changed'), {_channelId})
        ..routeBundleEvent(_notice('not json', raw: true), {_channelId})
        ..routeBundleEvent(_notice('member_joined', kind: 9), {_channelId})
        ..routeBundleEvent(_notice('member_joined', roomId: _otherChannelId), {
          _otherChannelId,
        });
      async.elapse(const Duration(seconds: 5));
      expect(
        container.read(channelMembershipUpdateProvider(_channelId)).version,
        0,
      );
      keepAlive.close();
      container.dispose();
    });
  });

  test('isReady follows the room coverage', () async {
    final relaySession = _MembershipRelaySessionNotifier([]);
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => relaySession)],
    );
    addTearDown(container.dispose);
    final router = container.read(roomLiveRouterProvider)
      ..setBundleRooms({_channelId});
    final keepAlive = container.listen(
      channelMembershipUpdateProvider(_channelId),
      (_, _) {},
      fireImmediately: true,
    );
    addTearDown(keepAlive.close);

    await _pumpEventQueue();
    expect(
      container.read(channelMembershipUpdateProvider(_channelId)).isReady,
      isFalse,
    );

    router.setCoveredRooms({_channelId});
    await _pumpEventQueue();
    final state = container.read(channelMembershipUpdateProvider(_channelId));
    expect(state.isReady, isTrue);
    expect(state.error, isNull);
    expect(relaySession.liveFilters, isEmpty);
  });

  test('stops listening and cancels settle timers without consumers', () {
    fakeAsync((async) {
      final container = _coveredContainer();
      final keepAlive = container.listen(
        channelMembershipUpdateProvider(_channelId),
        (_, _) {},
        fireImmediately: true,
      );
      _routeNotice(container, 'member_joined');
      async.flushMicrotasks();
      expect(async.pendingTimers, isNotEmpty);

      keepAlive.close();
      // Riverpod disposes an unlistened provider on its next scheduler turn.
      async.elapse(Duration.zero);
      expect(async.pendingTimers, isEmpty);
      container.dispose();
    });
  });

  test('working bots open no REQ', () async {
    final relaySession = _MembershipRelaySessionNotifier([
      _membershipEvent(role: 'bot'),
    ]);
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => relaySession)],
    );
    addTearDown(container.dispose);
    final keepAlive = container.listen(
      workingBotPubkeysProvider(_channelId),
      (_, _) {},
      fireImmediately: true,
    );
    addTearDown(keepAlive.close);
    await _pumpEventQueue();

    expect(relaySession.liveFilters, isEmpty);
  });

  test('budget probe: 3 search hits from 3 rooms open no live REQ', () async {
    final relaySession = _MembershipRelaySessionNotifier([]);
    final container = ProviderContainer(
      overrides: [relaySessionProvider.overrideWith(() => relaySession)],
    );
    addTearDown(container.dispose);
    const rooms = [_channelId, _otherChannelId, 'room-3'];
    // What a search result row watches for its room (results.dart).
    for (final room in rooms) {
      final keepAlive = container.listen(
        agentMentionPubkeysProvider(room),
        (_, _) {},
        fireImmediately: true,
      );
      addTearDown(keepAlive.close);
    }
    await _pumpEventQueue();
    await container.pump();

    expect(relaySession.liveFilters, isEmpty);
  });

  test('blank profile labels defer to the directory label', () {
    const pubkey = 'deadbeef0123456789';

    expect(
      mentionNamesWithDirectoryLabels(
        mentionPubkeys: const [pubkey],
        profileMentionNames: const {pubkey: '  '},
        directoryDisplayNames: const {pubkey: 'Directory bot'},
        agentMentionPubkeys: const {pubkey},
      ),
      const {pubkey: 'Directory bot'},
    );
  });
}

const _channelId = '11111111-1111-4111-8111-111111111111';
const _otherChannelId = '22222222-2222-4222-8222-222222222222';
const _agentPubkey =
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';

NostrEvent _membershipEvent({required String role}) => NostrEvent(
  id: 'membership-$role',
  pubkey: 'owner',
  createdAt: 1,
  kind: 39002,
  tags: [
    ['d', _channelId],
    ['h', _channelId],
    ['p', _agentPubkey, 'wss://relay.example', role],
  ],
  content: '',
  sig: 'sig',
);

NostrEvent _notice(
  String type, {
  int kind = EventKind.systemMessage,
  String roomId = _channelId,
  bool raw = false,
}) => NostrEvent(
  id: 'notice-$type-$kind-$roomId',
  pubkey: 'relay',
  createdAt: 2,
  kind: kind,
  tags: [
    ['h', roomId],
  ],
  content: raw ? type : jsonEncode({'type': type}),
  sig: 'sig',
);

/// Delivers a membership notice on the room bundle chunk carrying the room.
void _routeNotice(ProviderContainer container, String type) => container
    .read(roomLiveRouterProvider)
    .routeBundleEvent(_notice(type), {_channelId});

/// A container whose room is covered by the bundle (no coverage wait).
ProviderContainer _coveredContainer() {
  final container = ProviderContainer(
    overrides: [
      relaySessionProvider.overrideWith(
        () => _MembershipRelaySessionNotifier([]),
      ),
    ],
  );
  container.read(roomLiveRouterProvider)
    ..setBundleRooms({_channelId, _otherChannelId})
    ..setCoveredRooms({_channelId, _otherChannelId});
  return container;
}

Future<void> _pumpEventQueue() async {
  await Future<void>.delayed(Duration.zero);
  await Future<void>.delayed(Duration.zero);
}

class _MembershipRelaySessionNotifier extends RelaySessionNotifier {
  final List<NostrEvent> _memberships;
  final List<NostrFilter> liveFilters = [];
  var _membershipIndex = 0;

  _MembershipRelaySessionNotifier(this._memberships);

  @override
  SessionState build() => const SessionState(status: SessionStatus.connected);

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    if (_membershipIndex >= _memberships.length) return const [];
    return [_memberships[_membershipIndex++]];
  }

  @override
  Future<void Function()> subscribeWithStatus(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
    required void Function(RelaySubscriptionStatus status) onStatusChanged,
  }) async {
    liveFilters.add(filter);
    return () {};
  }

  @override
  Future<void Function()> subscribe(
    NostrFilter filter,
    void Function(NostrEvent) onEvent, {
    void Function(String message)? onClosed,
    void Function()? onGap,
  }) async {
    liveFilters.add(filter);
    return () {};
  }
}
