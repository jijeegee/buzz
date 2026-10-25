part of 'channels_provider_test.dart';

const _catchUpNow = 1000000;

NostrEvent _missed(
  String channelId,
  String id, {
  int createdAt = 999990,
  String pubkey = 'alice',
}) => NostrEvent(
  id: id,
  pubkey: pubkey,
  createdAt: createdAt,
  kind: EventKind.streamMessageV2,
  tags: [
    ['h', channelId],
  ],
  content: 'sent while offline',
  sig: 'sig',
);

/// Window loads of [channelId]'s room provider so far.
int _roomLoads(_FakeRelaySession session, String channelId) => session
    .queryBatches
    .where(
      (batch) => batch.any(
        (filter) =>
            filter.extensions['top_level'] == true &&
            (filter.tags['#h']?.contains(channelId) ?? false),
      ),
    )
    .length;

/// Waits in real time: catch-ups write the room stores off the main isolate.
Future<void> _eventually(bool Function() predicate) async {
  for (var i = 0; i < 500; i++) {
    if (predicate()) return;
    await Future<void>.delayed(const Duration(milliseconds: 10));
  }
  fail('Timed out waiting for the catch-up');
}

/// A message store whose writes can be made to fail.
class _FlakyMessageStore extends MessageStore {
  _FlakyMessageStore(super.root);

  bool failWrites = false;

  @override
  Future<void> write(
    MessageScope scope,
    MessageDbWrite write, {
    int? generation,
  }) => failWrites
      ? Future.error(const MessageDbWriteException('disk full'))
      : super.write(scope, write, generation: generation);
}

void _roomCatchUpTests() {
  group('reconnect catch-up', () {
    late Directory root;
    late _FlakyMessageStore messages;
    late ChannelRoomStore roomStore;
    late List<RoomCatchUp> catchUps;
    var now = _catchUpNow;

    void openStores() {
      messages = _FlakyMessageStore(() async => '${root.path}/messages');
      roomStore = ChannelRoomStore(messages: messages);
      roomStore.listenCatchUps(catchUps.add);
    }

    setUp(() async {
      root = await Directory.systemTemp.createTemp('channels_catch_up');
      catchUps = [];
      openStores();
      now = _catchUpNow;
      roomCatchUpClock = () => now;
    });

    /// When [room]'s live range starts in the database, or null: a hole.
    Future<int?> liveFrom(MessageScope scope, String room) =>
        messages.read<int?>(
          MessageScope(relayUrl: scope.relayUrl, pubkey: scope.pubkey),
          LiveFromRead(room),
        );

    tearDown(() async {
      roomCatchUpClock = () => DateTime.now().millisecondsSinceEpoch ~/ 1000;
      messages.failWrites = false;
      await roomStore.flush();
      await messages.close();
      try {
        if (await root.exists()) await root.delete(recursive: true);
      } on FileSystemException {
        // A store write may still hold a file; the temp dir is disposable.
      }
    });

    Future<(ProviderContainer, _FakeRelaySession)> start(
      List<String> rooms, {
      List<Override> overrides = const [],
    }) async {
      final session = _FakeRelaySession(
        memberships: [for (final room in rooms) _membership(room, 'me')],
        metadata: [for (final room in rooms) _meta(id: room, name: room)],
      );
      final container = _buildContainer(
        session: session,
        overrides: [
          channelRoomStoreProvider.overrideWithValue(roomStore),
          ...overrides,
        ],
      );
      addTearDown(container.dispose);
      await container.read(channelsProvider.future);
      final scope = container.read(messageScopeProvider)!;
      await _eventually(() => rooms.every((id) => roomStore.feeds(scope, id)));
      return (container, session);
    }

    Future<void> reconnect(_FakeRelaySession session) async {
      final before = catchUps.length;
      session.setStatus(SessionStatus.reconnecting);
      await _settle();
      session.setStatus(SessionStatus.connected);
      await _eventually(() => catchUps.length > before);
      await _settle();
    }

    test('missed events of closed rooms arrive in one query', () async {
      final (container, session) = await start([
        _channelA,
        _channelB,
        _channelD,
      ]);
      session.catchUpResults = (_) => [
        _missed(_channelA, 'missed-a'),
        _missed(_channelB, 'missed-b'),
        _missed(_channelD, 'missed-d'),
      ];
      now += 30;

      await reconnect(session);

      final filter = session.catchUpFilters.single;
      expect(filter.tags['#h'], [_channelA, _channelB, _channelD]);
      expect(filter.since, _catchUpNow - 60);
      expect(filter.limit, 1000);
      expect(filter.kinds, contains(EventKind.channelThreadSummary));
      await roomStore.flush();
      final scope = container.read(messageScopeProvider)!;
      for (final (room, id) in [
        (_channelA, 'missed-a'),
        (_channelB, 'missed-b'),
        (_channelD, 'missed-d'),
      ]) {
        expect(
          messages
              .reader(scope)!
              .select('SELECT id FROM events WHERE channel_id = ?', [room])
              .map((row) => row['id']),
          [id],
        );
      }
      expect(catchUps.single.gaps, isEmpty);
      // No room reloaded: none was even created.
      for (final room in [_channelA, _channelB, _channelD]) {
        expect(container.exists(channelMessagesProvider(room)), isFalse);
      }
    });

    test(
      'an open covered room takes the catch-up instead of reloading',
      () async {
        final (container, session) = await start([_channelA]);
        container.listen(channelMessagesProvider(_channelA), (_, _) {});
        await _eventually(() => _roomLoads(session, _channelA) == 1);
        await _settle();
        final notifier = container.read(
          channelMessagesProvider(_channelA).notifier,
        );
        notifier.addLocalMessage(
          _missed(_channelA, 'my-pending', pubkey: 'me', createdAt: 999995),
        );
        session.catchUpResults = (_) => [_missed(_channelA, 'missed-a')];

        await reconnect(session);

        expect(_roomLoads(session, _channelA), 1);
        final ids = container
            .read(channelMessagesProvider(_channelA))
            .value!
            .map((event) => event.id);
        expect(ids, containsAll(['missed-a', 'my-pending']));
        // Its live range, started by its first page, continues.
        final scope = container.read(messageScopeProvider)!;
        expect(await roomStore.hasGap(scope, _channelA), isFalse);
        await roomStore.flush();
        expect(await liveFrom(scope, _channelA), isNotNull);
      },
    );

    test('a chunk that overflows marks its rooms and they reload', () async {
      final (container, session) = await start([_channelA, _channelB]);
      container.listen(channelMessagesProvider(_channelA), (_, _) {});
      await _eventually(() => _roomLoads(session, _channelA) == 1);
      await _settle();
      container
          .read(channelMessagesProvider(_channelA).notifier)
          .addLocalMessage(
            _missed(_channelA, 'my-pending', pubkey: 'me', createdAt: 999995),
          );
      final scope = container.read(messageScopeProvider)!;
      // Room B's newest page was fetched earlier: it has a live range.
      roomStore.clearGap(scope, _channelB);
      await roomStore.flush();
      expect(await liveFrom(scope, _channelB), isNotNull);
      var page = 0;
      session.catchUpResults = (_) {
        final base = 999000 - page++ * 1000;
        return [
          for (var i = 0; i < 1000; i++)
            _missed(_channelB, 'full-$base-$i', createdAt: base + i),
        ];
      };

      await reconnect(session);

      // Three full pages, then the rooms count as unfilled.
      expect(session.catchUpFilters, hasLength(3));
      expect(session.catchUpFilters[1].until, 999000);
      expect(
        session.catchUpFilters[1].extensions['before_id'],
        'full-999000-0',
      );
      expect(catchUps.single.gaps, {_channelA, _channelB});
      // The overflow left a hole: B's live range ended with the catch-up.
      expect(await liveFrom(scope, _channelB), isNull);
      expect(await roomStore.hasGap(scope, _channelB), isTrue);
      await _eventually(() => _roomLoads(session, _channelA) == 2);
      await _settle();
      expect(
        container
            .read(channelMessagesProvider(_channelA))
            .value!
            .map((event) => event.id),
        contains('my-pending'),
      );
    });

    test('a watermark older than a day marks every room instead', () async {
      final (container, session) = await start([_channelA, _channelB]);
      now += 24 * 60 * 60 + 1;

      await reconnect(session);

      expect(session.catchUpFilters, isEmpty);
      expect(catchUps.single.gaps, {_channelA, _channelB});
      final scope = container.read(messageScopeProvider)!;
      expect(await roomStore.hasGap(scope, _channelA), isTrue);
    });

    test('a future timestamp does not move the watermark past now', () async {
      final (_, session) = await start([_channelA]);
      session.emit(_missed(_channelA, 'future', createdAt: _catchUpNow + 5000));
      now += 10;

      await reconnect(session);

      // Clamped to when it arrived, not to its claimed future time.
      expect(session.catchUpFilters.single.since, _catchUpNow - 60);
    });

    test('an uncovered room still reloads on reconnect', () async {
      final (container, session) = await start([_channelA]);
      container.listen(channelMessagesProvider(_channelB), (_, _) {});
      await _eventually(() => _roomLoads(session, _channelB) == 1);

      await reconnect(session);

      await _eventually(() => _roomLoads(session, _channelB) == 2);
    });

    test('a restart catches up from the watermark in the database', () async {
      final (first, _) = await start([_channelA]);
      // The feed started now: its watermark is stored with the next write.
      await roomStore.flush();
      first.dispose();
      await messages.close();

      openStores();
      now += 300;
      final (_, session) = await start([_channelA]);
      await _eventually(() => catchUps.isNotEmpty);

      expect(session.catchUpFilters.single.since, _catchUpNow - 60);
      expect(catchUps.single.gaps, isEmpty);
    });

    test('a catch-up whose commit fails leaves every room a gap and the '
        'watermark where it was', () async {
      final (container, session) = await start([_channelA, _channelB]);
      await roomStore.flush();
      final scope = container.read(messageScopeProvider)!;
      expect(await roomStore.loadWatermark(scope), _catchUpNow);
      session.catchUpResults = (_) => [_missed(_channelA, 'missed-a')];
      messages.failWrites = true;
      now += 30;

      await reconnect(session);

      expect(catchUps.single.gaps, {_channelA, _channelB});
      expect(await roomStore.loadWatermark(scope), _catchUpNow);
      messages.failWrites = false;
      expect(await roomStore.hasGap(scope, _channelA), isTrue);
    });
  });
}
