part of '../channel_detail_page_test.dart';

/// Row [n] of the open-at channel: one a second, three-digit numbers so a
/// row's text never contains another's.
NostrEvent _openAtRow(int n) => fixtureRow(_channelId, n, 1000 + n);

/// Serves the channel as the relay's NIP-CW window queries do, and by id.
class _WindowRelaySession extends _IdentityUpdateRelaySession {
  _WindowRelaySession(this.rows);

  final List<NostrEvent> rows;

  /// Holds newer pages until completed, once set.
  Completer<void>? newerGate;
  int newerQueries = 0;

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final filter = filters.single;
    if (filter.extensions['after_id'] != null) {
      newerQueries += 1;
      await newerGate?.future;
    }
    return serveChannelWindow(_channelId, rows, filter);
  }

  @override
  Future<List<NostrEvent>> fetchHistory(
    NostrFilter filter, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final ids = filter.ids;
    if (ids != null) {
      return [
        for (final row in rows)
          if (ids.contains(row.id)) row,
      ];
    }
    return super.fetchHistory(filter, timeout: timeout);
  }
}

void openAtTests() {
  group('Opening at a message', () {
    final rows = [for (var n = 100; n < 300; n++) _openAtRow(n)];

    ReadStateState readAt(int? firstUnread, {bool isReady = true}) =>
        ReadStateState(
          isReady: isReady,
          pubkey: 'self',
          contexts: {
            _channelId: firstUnread == null ? 2000 : 999 + firstUnread,
          },
          version: 0,
        );

    Widget testable(
      _WindowRelaySession session, {
      bool newerPages = true,
      String? initialMessageId,
      int? firstUnread,
      Widget? home,
      _SynchronousReadStateNotifier? readState,
    }) => _buildTestable(
      home: home,
      huddleCurrentPubkey: 'self',
      messages: const [],
      realMessages: true,
      relaySessionNotifier: session,
      relayInfo: RelayInfo(
        supportedExtensions: newerPages ? {newerWindowExtension} : const {},
      ),
      initialMessageId: initialMessageId,
      channelsNotifier: _FakeChannelsNotifier(
        [_testChannel],
        observedUnread: {
          if (firstUnread != null)
            _channelId: [
              makeObservedUnreadEvent(
                id: fixtureId(firstUnread),
                createdAt: 1000 + firstUnread,
                rootId: null,
                highPriority: false,
                channelType: 'stream',
                isThreadedReply: false,
              ),
            ],
        },
      ),
      readStateNotifier:
          readState ?? _SynchronousReadStateNotifier(readAt(firstUnread)),
    );

    void useTallView(WidgetTester tester) {
      tester.view.physicalSize = const Size(400, 800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
    }

    final messageList = find.byKey(const ValueKey('channel-message-list'));

    bool timelineVisible(WidgetTester tester) {
      if (messageList.evaluate().isEmpty) return false;
      final content = find.ancestor(
        of: messageList,
        matching: find.byKey(const Key('skeleton-reveal-content')),
      );
      return tester.widget<Opacity>(content.first).opacity > 0;
    }

    /// Pumps frame by frame until the timeline has shown and settled, and
    /// returns where [target] was in each frame it could be seen.
    Future<List<Rect?>> visibleRects(WidgetTester tester, Finder target) async {
      final rects = <Rect?>[];
      for (var frame = 0; frame < 300; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        if (!timelineVisible(tester)) continue;
        rects.add(
          target.evaluate().isEmpty ? null : tester.getRect(target.first),
        );
        if (!tester.binding.hasScheduledFrame) break;
      }
      await tester.pumpAndSettle();
      return rects;
    }

    /// The signed-in account's saved positions, as the page reads them.
    Future<RoomPositionStore> positionStore(WidgetTester tester) async {
      await tester.pumpWidget(
        testable(_WindowRelaySession(rows), home: const SizedBox()),
      );
      final store = ProviderScope.containerOf(
        tester.element(find.byType(SizedBox).first),
      ).read(roomPositionStoreProvider)!;
      await tester.pumpWidget(const SizedBox());
      return store;
    }

    void saveRow(RoomPositionStore store, int n, {double edge = 0.5}) =>
        store.record(
          _channelId,
          RoomPositionReading.at(
            id: fixtureId(n),
            createdAt: 1000 + n,
            edge: edge,
          ),
        );

    ChannelMessagesNotifier notifierOf(WidgetTester tester) =>
        ProviderScope.containerOf(
          tester.element(find.byType(ChannelDetailPage)),
        ).read(channelMessagesProvider(_channelId).notifier);

    testWidgets(
      'opens at the first unread with its divider from the first frame',
      (tester) async {
        useTallView(tester);
        final session = _WindowRelaySession(rows);
        await tester.pumpWidget(testable(session, firstUnread: 150));

        final divider = find.byKey(
          const ValueKey('channel-new-messages-divider'),
        );
        final rects = await visibleRects(tester, divider);

        expect(rects, isNotEmpty);
        final settled = tester.getRect(divider);
        // Every frame the timeline could be seen showed the divider where it
        // ended up: nothing was drawn elsewhere and then moved.
        expect(rects, everyElement(settled));
        expect(find.text('New messages'), findsOneWidget);
        final firstUnread = tester.getRect(findRichText('row 150'));
        expect(settled.bottom, lessThanOrEqualTo(firstUnread.top));
        // The divider sits at the top of the timeline, below the app bar.
        final appBar = tester.getRect(find.byType(FrostedAppBar).first);
        expect(settled.top, closeTo(appBar.bottom, 2));
        expect(findRichText('row 149'), findsOneWidget);
        expect(findRichText('row 299'), findsNothing);
        expect(
          find.byKey(const ValueKey('channel-jump-to-oldest-unread')),
          findsNothing,
        );
        expect(notifierOf(tester).isInMiddleWindow, isTrue);
      },
    );

    testWidgets('opens at a notification target from the first frame', (
      tester,
    ) async {
      useTallView(tester);
      final session = _WindowRelaySession(rows);
      await tester.pumpWidget(
        testable(session, initialMessageId: fixtureId(120)),
      );

      final target = findRichText('row 120');
      final rects = await visibleRects(tester, target);

      expect(rects, isNotEmpty);
      expect(rects, everyElement(tester.getRect(target)));
      expect(findRichText('row 299'), findsNothing);
      expect(
        find.byKey(const ValueKey('channel-new-messages-divider')),
        findsNothing,
      );
      expect(notifierOf(tester).isInMiddleWindow, isTrue);
    });

    testWidgets('paging newer rows keeps the row being read in place', (
      tester,
    ) async {
      useTallView(tester);
      final session = _WindowRelaySession(rows);
      await tester.pumpWidget(testable(session, firstUnread: 150));
      await tester.pumpAndSettle();
      final notifier = notifierOf(tester);
      expect(notifier.hasNewer, isTrue);

      session.newerGate = Completer<void>();
      final queriesBefore = session.newerQueries;
      for (
        var i = 0;
        i < 20 && findRichText('row 200').evaluate().isEmpty;
        i++
      ) {
        await tester.drag(messageList, const Offset(0, -300));
        await tester.pumpAndSettle();
      }
      expect(findRichText('row 200'), findsOneWidget);
      // Reaching the newest loaded rows asks for the next page.
      expect(session.newerQueries, queriesBefore + 1);

      final reading = findRichText('row 196');
      final before = tester.getRect(reading);
      session.newerGate!.complete();
      for (var frame = 0; frame < 60; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        expect(tester.getRect(reading), before);
      }
      await tester.pumpAndSettle();

      final shown = ProviderScope.containerOf(
        tester.element(find.byType(ChannelDetailPage)),
      ).read(channelMessagesProvider(_channelId)).value!.map((e) => e.id);
      expect(shown, contains(fixtureId(201)));
      expect(tester.getRect(reading), before);
    });

    testWidgets('jump to latest replaces the window with the newest rows', (
      tester,
    ) async {
      useTallView(tester);
      final session = _WindowRelaySession(rows);
      await tester.pumpWidget(testable(session, firstUnread: 150));
      await tester.pumpAndSettle();

      final jump = find.byKey(const ValueKey('channel-jump-to-latest'));
      expect(jump, findsOneWidget);
      await tester.tap(jump);
      await tester.pumpAndSettle();

      final notifier = notifierOf(tester);
      expect(notifier.isInMiddleWindow, isFalse);
      expect(notifier.hasNewer, isFalse);
      expect(findRichText('row 299'), findsOneWidget);
      expect(findRichText('row 150'), findsNothing);
      // The newest row sits at the bottom of the timeline.
      final newest = tester.getRect(findRichText('row 299'));
      expect(newest.bottom, greaterThan(600));
      expect(newest.bottom, lessThanOrEqualTo(800));
      expect(findRichText('row 298'), findsOneWidget);
    });

    testWidgets('without newer pages it opens at the latest as before', (
      tester,
    ) async {
      useTallView(tester);
      final session = _WindowRelaySession(rows);
      await tester.pumpWidget(
        testable(session, newerPages: false, firstUnread: 150),
      );
      await tester.pumpAndSettle();

      expect(findRichText('row 299'), findsOneWidget);
      expect(
        find.byKey(const ValueKey('channel-new-messages-divider')),
        findsNothing,
      );
      expect(
        find.byKey(const ValueKey('channel-jump-to-oldest-unread')),
        findsOneWidget,
      );
      expect(notifierOf(tester).isInMiddleWindow, isFalse);
    });

    testWidgets('reopening restores the saved position from the first frame', (
      tester,
    ) async {
      useTallView(tester);
      final store = await positionStore(tester);
      await tester.pumpWidget(testable(_WindowRelaySession(rows)));
      await tester.pumpAndSettle();
      for (var i = 0; i < 4; i++) {
        await tester.drag(messageList, const Offset(0, 300));
        await tester.pumpAndSettle();
      }
      // Scrolling rests, then the position is saved.
      await tester.pump(roomPositionSaveDelay);
      final saved = store.read(_channelId);
      expect(saved, isNotNull);
      final reading = findRichText('row ${int.parse(saved!.id, radix: 16)}');
      final before = tester.getRect(reading);

      await tester.pumpWidget(const SizedBox());
      await tester.pumpWidget(testable(_WindowRelaySession(rows)));
      final rects = await visibleRects(tester, reading);

      expect(rects, isNotEmpty);
      // From the first frame the row is back where the reader left it.
      for (final rect in rects) {
        expect(rect, isNotNull);
        expect(rect!.top, closeTo(before.top, 1));
      }
      expect(findRichText('row 299'), findsNothing);
      expect(notifierOf(tester).isInMiddleWindow, isTrue);
    });

    testWidgets('unread messages win over a saved position', (tester) async {
      useTallView(tester);
      saveRow(await positionStore(tester), 250);
      await tester.pumpWidget(
        testable(_WindowRelaySession(rows), firstUnread: 150),
      );
      await tester.pumpAndSettle();

      expect(
        find.byKey(const ValueKey('channel-new-messages-divider')),
        findsOneWidget,
      );
      expect(findRichText('row 150'), findsOneWidget);
      expect(findRichText('row 250'), findsNothing);
    });

    testWidgets('a linked message wins over a saved position', (tester) async {
      useTallView(tester);
      saveRow(await positionStore(tester), 250);
      await tester.pumpWidget(
        testable(_WindowRelaySession(rows), initialMessageId: fixtureId(120)),
      );
      await tester.pumpAndSettle();

      expect(findRichText('row 120'), findsOneWidget);
      expect(findRichText('row 250'), findsNothing);
    });

    testWidgets('leaving at the newest row forgets the saved position', (
      tester,
    ) async {
      useTallView(tester);
      final store = await positionStore(tester);
      saveRow(store, 150);
      await tester.pumpWidget(testable(_WindowRelaySession(rows)));
      await tester.pumpAndSettle();
      expect(findRichText('row 150'), findsOneWidget);

      await tester.tap(find.byKey(const ValueKey('channel-jump-to-latest')));
      await tester.pumpAndSettle();
      expect(findRichText('row 299'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());

      expect(store.read(_channelId), isNull);
    });

    testWidgets('a saved row deleted since is forgotten', (tester) async {
      useTallView(tester);
      final store = await positionStore(tester);
      store.record(
        _channelId,
        const RoomPositionReading.at(id: 'f00d', createdAt: 1150, edge: 0.5),
      );
      await tester.pumpWidget(testable(_WindowRelaySession(rows)));
      await tester.pumpAndSettle();

      // It still opens where the row was.
      expect(findRichText('row 150'), findsOneWidget);
      expect(store.read(_channelId), isNull);
    });

    testWidgets(
      'a read state ready within a second opens at the first unread',
      (tester) async {
        useTallView(tester);
        final readState = _LateReadStateNotifier(readAt(150, isReady: false));
        await tester.pumpWidget(
          testable(
            _WindowRelaySession(rows),
            firstUnread: 150,
            readState: readState,
          ),
        );
        await tester.pump(const Duration(milliseconds: 400));
        // Nothing is drawn at the newest rows while the read state loads.
        expect(timelineVisible(tester), isFalse);

        readState.becomeReady(readAt(150));
        await tester.pumpAndSettle();
        expect(
          find.byKey(const ValueKey('channel-new-messages-divider')),
          findsOneWidget,
        );
        expect(findRichText('row 150'), findsOneWidget);
        expect(findRichText('row 299'), findsNothing);
        expect(notifierOf(tester).isInMiddleWindow, isTrue);
      },
    );

    testWidgets(
      'a read marker from another device keeps the divider and position',
      (tester) async {
        useTallView(tester);
        final readState = _LateReadStateNotifier(readAt(150));
        await tester.pumpWidget(
          testable(
            _WindowRelaySession(rows),
            firstUnread: 150,
            readState: readState,
          ),
        );
        await tester.pumpAndSettle();
        final divider = find.byKey(
          const ValueKey('channel-new-messages-divider'),
        );
        expect(divider, findsOneWidget);
        final before = tester.getRect(findRichText('row 150').first);

        // Another device reads the whole room while it is open here.
        readState.becomeReady(
          const ReadStateState(
            isReady: true,
            pubkey: 'self',
            contexts: {_channelId: 2000},
            version: 1,
          ),
        );
        await tester.pumpAndSettle();
        expect(divider, findsOneWidget);
        expect(tester.getRect(findRichText('row 150').first), before);
      },
    );

    testWidgets('without the read state in a second it opens at the newest', (
      tester,
    ) async {
      useTallView(tester);
      final store = await positionStore(tester);
      saveRow(store, 150);
      await tester.pumpWidget(
        testable(
          _WindowRelaySession(rows),
          readState: _LateReadStateNotifier(readAt(null, isReady: false)),
        ),
      );
      await tester.pump(readStateOpenWait ~/ 2);
      expect(timelineVisible(tester), isFalse);
      await tester.pump(readStateOpenWait);
      await tester.pumpAndSettle();

      expect(timelineVisible(tester), isTrue);
      expect(findRichText('row 299'), findsOneWidget);
      expect(notifierOf(tester).isInMiddleWindow, isFalse);
    });
  });
}

/// A read state that is not ready until the test says so.
class _LateReadStateNotifier extends _SynchronousReadStateNotifier {
  _LateReadStateNotifier(super.initial);

  void becomeReady(ReadStateState ready) => state = ready;
}

/// A signed-in test page gets a read state that is already loaded, so it
/// does not wait out readStateOpenWait for a relay that never answers.
ReadStateNotifier? _readyFor(String? pubkey) => pubkey == null
    ? null
    : _SynchronousReadStateNotifier(
        ReadStateState(
          isReady: true,
          pubkey: pubkey,
          contexts: const {},
          version: 0,
        ),
      );
