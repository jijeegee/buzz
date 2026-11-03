part of 'channels_provider_test.dart';

/// Push inbox folds into the channel list (docs/message-sync.md#push-inbox).
void _pushFoldTests() {
  final me = 'a' * 64;
  const channelC = '33333333-3333-4333-8333-333333333333';

  NostrEvent pushed(String id, String room, int at) => NostrEvent(
    id: id,
    pubkey: 'b' * 64,
    createdAt: at,
    kind: EventKind.streamMessageV2,
    tags: [
      ['h', room],
    ],
    content: id,
    sig: 'sig',
  );

  group('push inbox fold', () {
    late ProviderContainer container;
    late _GatedReadState readState;

    Future<void> load() async {
      readState = _GatedReadState();
      final session = _FakeRelaySession(
        memberships: [
          _membership(_channelA, me),
          _membership(_channelB, me),
          _membership(channelC, me),
        ],
        metadata: [
          _meta(id: _channelA, name: 'general'),
          _meta(id: _channelB, name: 'Bob', channelType: 'dm'),
          _meta(id: channelC, name: 'old', archived: true),
        ],
        hiddenDmEvents: [
          _hiddenDms([_channelB], pubkey: me),
        ],
      );
      container = _buildContainer(session: session, readState: () => readState);
      addTearDown(container.dispose);
      container.read(_testPubkeyProvider.notifier).set(me);
      await container.read(channelsProvider.future);
      await _settle();
    }

    MessageStoreLease lease() => container
        .read(messageStoreProvider)
        .lease(container.read(relayConfigProvider).baseUrl, me)!;

    Channel? channel(String id) => container
        .read(channelsProvider)
        .value!
        .where((c) => c.id == id)
        .firstOrNull;

    Map<String, Map<String, ObservedUnreadEvent>> observed() =>
        container.read(channelsProvider.notifier).observedUnreadEventsByChannel;

    test('listed rooms move at once; unread waits for read state, '
        'then records once', () async {
      await load();
      final notifier = container.read(channelsProvider.notifier);
      expect(notifier.pushFoldRoomIds, {_channelA});
      final events = [
        pushed('a1', _channelA, 50),
        pushed('b1', _channelB, 60),
        pushed('c1', channelC, 70),
      ];
      final fold = notifier.foldPushed(lease(), events);
      await _settle();
      expect(
        channel(_channelA)!.lastMessageAt,
        DateTime.fromMillisecondsSinceEpoch(50 * 1000, isUtc: true),
      );
      expect(observed(), isEmpty, reason: 'read state is not loaded yet');

      readState.load({});
      await fold;
      await notifier.foldPushed(lease(), events);
      expect(observed().keys, [_channelA]);
      expect(observed()[_channelA]!.keys, ['a1']);
      expect(channel(_channelB), isNull, reason: 'a hidden DM stays hidden');
      final archived = channel(channelC)!;
      expect(
        archived.lastMessageAt,
        isNot(DateTime.fromMillisecondsSinceEpoch(70 * 1000, isUtc: true)),
      );
    });

    test('a fold whose lease ended changes nothing', () async {
      await load();
      readState.load({});
      final ended = lease();
      // Signed out and back in: same server and account, a new lease.
      await container
          .read(messageStoreProvider)
          .forget(container.read(relayConfigProvider).baseUrl, me);
      expect(lease(), isNot(ended));
      await container.read(channelsProvider.notifier).foldPushed(ended, [
        pushed('a1', _channelA, 50),
      ]);
      expect(observed(), isEmpty);
      expect(channel(_channelA)!.lastMessageAt, isNull);
    });
  });
}
