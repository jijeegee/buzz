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

    Widget testable(
      _WindowRelaySession session, {
      bool newerPages = true,
      String? initialMessageId,
      int? firstUnread,
    }) => _buildTestable(
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
      readStateNotifier: _SynchronousReadStateNotifier(
        ReadStateState(
          isReady: true,
          pubkey: 'self',
          contexts: {
            _channelId: firstUnread == null ? 2000 : 999 + firstUnread,
          },
          version: 0,
        ),
      ),
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
  });
}
