part of 'channels_provider_test.dart';

void _roomStoreFeedTests() {
  group('live feed into room stores', () {
    late Directory root;

    setUp(() async {
      root = await Directory.systemTemp.createTemp('channels_room_store');
    });

    tearDown(() async {
      if (await root.exists()) await root.delete(recursive: true);
    });

    test('a closed room\'s live events land in its stored tail', () async {
      final cache = ChannelHeadCache(() async => root);
      final roomStore = ChannelRoomStore(cache);
      final session = _FakeRelaySession(
        memberships: [_membership(_channelA, 'me')],
        metadata: [_meta(id: _channelA, name: 'room')],
      );
      final container = _buildContainer(
        session: session,
        overrides: [channelRoomStoreProvider.overrideWithValue(roomStore)],
      );
      addTearDown(container.dispose);
      await container.read(channelsProvider.future);
      await _waitUntil(() => session.activeSubscriptionCount == 1);
      expect(
        session.subscribeFilters
            .singleWhere((filter) => filter.tags['#h'] != null)
            .kinds,
        contains(EventKind.channelThreadSummary),
      );
      final heard = <NostrEvent>[];
      container.read(channelLiveEventsProvider).listen(heard.add);

      final message = _liveMessage(_channelA);
      final summary = NostrEvent(
        id: 'summary',
        pubkey: 'relay',
        createdAt: 21,
        kind: EventKind.channelThreadSummary,
        tags: [
          ['h', _channelA],
          ['d', message.id],
          ['e', message.id],
        ],
        content: jsonEncode({'reply_count': 1, 'descendant_count': 1}),
        sig: 'sig',
      );
      session.emit(message);
      session.emit(summary);
      await roomStore.flush();

      final scope = container.read(channelHeadScopeProvider)!;
      final tail = await cache.loadTail(scope, _channelA);
      expect(tail!.rows.map((event) => event.id), [message.id]);
      expect(tail.aux.map((event) => event.id), ['summary']);
      // Summaries feed the store only, not the list's readers.
      expect(heard.map((event) => event.id), [message.id]);
      // No room notifier was created for the closed room.
      expect(container.exists(channelMessagesProvider(_channelA)), isFalse);
    });
  });
}
