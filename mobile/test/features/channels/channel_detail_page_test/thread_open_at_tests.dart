part of '../channel_detail_page_test.dart';

final _openAtRootId = fixtureId(1);

/// The thread root of the open-at thread.
final _openAtRoot = NostrEvent(
  id: _openAtRootId,
  pubkey: 'b' * 64,
  createdAt: 900,
  kind: EventKind.streamMessageV2,
  tags: const [
    ['h', _channelId],
  ],
  content: 'thread root',
  sig: '',
);

/// Reply [n] of the open-at thread, one a second like [_openAtRow].
NostrEvent _openAtReply(int n) => fixtureRow(
  _channelId,
  n,
  1000 + n,
  extraTags: [
    ['e', _openAtRootId, '', 'reply'],
  ],
);

/// Serves the thread's NIP-CW window queries over its replies.
class _ThreadWindowRelaySession extends _IdentityUpdateRelaySession {
  _ThreadWindowRelaySession(this.replies);

  final List<NostrEvent> replies;

  /// Holds newer pages until completed, once set.
  Completer<void>? newerGate;
  int newerQueries = 0;

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) async {
    final filter = filters.single;
    if (filter.extensions['thread_window'] != true) return const [];
    if (filter.extensions['after_id'] != null) {
      newerQueries += 1;
      await newerGate?.future;
    }
    return serveThreadWindow(_channelId, _openAtRootId, replies, filter);
  }
}

void threadOpenAtTests() {
  group('Opening a thread at a reply', () {
    final replies = [for (var n = 100; n < 300; n++) _openAtReply(n)];

    Widget testable(
      _ThreadWindowRelaySession session, {
      bool newerPages = true,
      String? initialMessageId,
      int? firstUnread,
      bool readReady = true,
      _SynchronousReadStateNotifier? readState,
    }) => _buildTestable(
      messages: [_openAtRoot],
      relaySessionNotifier: session,
      huddleCurrentPubkey: 'self',
      relayInfo: RelayInfo(
        supportedExtensions: newerPages ? {newerWindowExtension} : const {},
      ),
      channelsNotifier: _FakeChannelsNotifier(
        [_testChannel],
        observedUnread: {
          if (firstUnread != null)
            _channelId: [
              makeObservedUnreadEvent(
                id: fixtureId(firstUnread),
                createdAt: 1000 + firstUnread,
                rootId: _openAtRootId,
                highPriority: false,
                channelType: 'stream',
                isThreadedReply: true,
              ),
            ],
        },
      ),
      readStateNotifier:
          readState ??
          _SynchronousReadStateNotifier(
            ReadStateState(
              isReady: readReady,
              pubkey: 'self',
              contexts: {
                _channelId: firstUnread == null ? 2000 : 999 + firstUnread,
              },
              version: 0,
            ),
          ),
      home: ThreadDetailPage(
        threadHead: formatTimeline([_openAtRoot]).single,
        allMessages: formatTimeline([
          _openAtRoot,
          if (initialMessageId != null)
            ...replies.where((reply) => reply.id == initialMessageId),
        ]),
        channelId: _channelId,
        currentPubkey: 'self',
        isMember: true,
        isArchived: false,
        initialMessageId: initialMessageId,
      ),
    );

    void useTallView(WidgetTester tester) {
      tester.view.physicalSize = const Size(400, 800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
    }

    final messageList = find.byKey(const ValueKey('thread-message-list'));
    final divider = find.byKey(const ValueKey('thread-new-messages-divider'));

    bool listVisible(WidgetTester tester) {
      if (messageList.evaluate().isEmpty) return false;
      final reveal = find.byKey(const ValueKey('thread-open-reveal'));
      return tester.widget<Opacity>(reveal).opacity > 0;
    }

    /// Pumps frame by frame until the replies have shown and settled, and
    /// returns where [target] was in each frame they could be seen.
    Future<List<Rect?>> visibleRects(WidgetTester tester, Finder target) async {
      final rects = <Rect?>[];
      for (var frame = 0; frame < 300; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        if (!listVisible(tester)) continue;
        rects.add(
          target.evaluate().isEmpty ? null : tester.getRect(target.first),
        );
        if (!tester.binding.hasScheduledFrame) break;
      }
      await tester.pumpAndSettle();
      return rects;
    }

    ThreadWindowNotifier windowOf(WidgetTester tester) =>
        ProviderScope.containerOf(
          tester.element(find.byType(ThreadDetailPage)),
        ).read(
          threadWindowProvider(
            ThreadRepliesArgs(channelId: _channelId, rootId: _openAtRootId),
          ).notifier,
        );

    testWidgets('opens at the first unread reply from the first frame', (
      tester,
    ) async {
      useTallView(tester);
      await tester.pumpWidget(
        testable(_ThreadWindowRelaySession(replies), firstUnread: 150),
      );

      final rects = await visibleRects(tester, divider);

      expect(rects, isNotEmpty);
      final settled = tester.getRect(divider);
      // Every frame the replies could be seen showed the divider where it
      // ended up: nothing was drawn elsewhere and then moved.
      expect(rects, everyElement(settled));
      expect(find.text('New messages'), findsOneWidget);
      expect(
        settled.bottom,
        lessThanOrEqualTo(tester.getRect(findRichText('row 150')).top),
      );
      final appBar = tester.getRect(find.byType(FrostedAppBar).first);
      expect(settled.top, closeTo(appBar.bottom, 2));
      expect(findRichText('row 299'), findsNothing);
      expect(windowOf(tester).hasNewer, isTrue);
    });

    testWidgets('opens at a notification reply from the first frame', (
      tester,
    ) async {
      useTallView(tester);
      await tester.pumpWidget(
        testable(
          _ThreadWindowRelaySession(replies),
          initialMessageId: fixtureId(120),
        ),
      );

      final target = findRichText('row 120');
      final rects = await visibleRects(tester, target);

      expect(rects, isNotEmpty);
      expect(rects, everyElement(tester.getRect(target)));
      expect(findRichText('row 299'), findsNothing);
      expect(divider, findsNothing);
      expect(windowOf(tester).hasNewer, isTrue);
    });

    testWidgets('paging newer replies keeps the reply being read in place', (
      tester,
    ) async {
      useTallView(tester);
      final session = _ThreadWindowRelaySession(replies);
      await tester.pumpWidget(testable(session, firstUnread: 150));
      await tester.pumpAndSettle();
      expect(windowOf(tester).hasNewer, isTrue);

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
      // Reaching the newest loaded replies asks for the next page.
      expect(session.newerQueries, queriesBefore + 1);

      final reading = findRichText('row 196');
      final before = tester.getRect(reading);
      session.newerGate!.complete();
      for (var frame = 0; frame < 60; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        expect(tester.getRect(reading), before);
      }
      await tester.pumpAndSettle();

      final held = windowOf(tester);
      expect(held.hasNewer, isTrue);
      expect(
        ProviderScope.containerOf(tester.element(find.byType(ThreadDetailPage)))
            .read(
              threadWindowProvider(
                ThreadRepliesArgs(channelId: _channelId, rootId: _openAtRootId),
              ),
            )!
            .events
            .map((event) => event.id),
        contains(fixtureId(201)),
      );
      expect(tester.getRect(reading), before);
    });

    testWidgets('jump to latest replaces the window with the newest replies', (
      tester,
    ) async {
      useTallView(tester);
      await tester.pumpWidget(
        testable(_ThreadWindowRelaySession(replies), firstUnread: 150),
      );
      await tester.pumpAndSettle();

      final jump = find.byKey(const ValueKey('thread-jump-to-latest'));
      expect(jump, findsOneWidget);
      await tester.tap(jump);
      await tester.pumpAndSettle();

      expect(windowOf(tester).hasNewer, isFalse);
      expect(findRichText('row 150'), findsNothing);
      final newest = tester.getRect(findRichText('row 299'));
      expect(newest.bottom, greaterThan(600));
      expect(newest.bottom, lessThanOrEqualTo(800));
      expect(jump, findsNothing);
    });

    testWidgets('without newer pages it opens at the newest reply as before', (
      tester,
    ) async {
      useTallView(tester);
      await tester.pumpWidget(
        testable(
          _ThreadWindowRelaySession(replies),
          newerPages: false,
          firstUnread: 150,
        ),
      );
      await tester.pumpAndSettle();

      expect(findRichText('row 299'), findsOneWidget);
      expect(divider, findsNothing);
      expect(windowOf(tester).hasNewer, isFalse);
    });

    testWidgets('a linked reply still opens without newer pages', (
      tester,
    ) async {
      useTallView(tester);
      await tester.pumpWidget(
        testable(
          _ThreadWindowRelaySession(replies),
          newerPages: false,
          initialMessageId: fixtureId(280),
        ),
      );
      await tester.pumpAndSettle();

      expect(findRichText('row 280'), findsOneWidget);
      expect(windowOf(tester).hasNewer, isFalse);
    });

    testWidgets('reopening a thread restores where the reader left it', (
      tester,
    ) async {
      useTallView(tester);
      await tester.pumpWidget(testable(_ThreadWindowRelaySession(replies)));
      await tester.pumpAndSettle();
      expect(findRichText('row 299'), findsOneWidget);
      for (var i = 0; i < 3; i++) {
        await tester.drag(messageList, const Offset(0, 300));
        await tester.pumpAndSettle();
      }
      await tester.pump(roomPositionSaveDelay);
      final store = ProviderScope.containerOf(
        tester.element(find.byType(ThreadDetailPage)),
      ).read(roomPositionStoreProvider)!;
      final saved = store.read(threadRoomKey(_channelId, _openAtRootId));
      expect(saved, isNotNull);
      final reading = findRichText('row ${int.parse(saved!.id, radix: 16)}');
      final before = tester.getRect(reading);

      await tester.pumpWidget(const SizedBox());
      await tester.pumpWidget(testable(_ThreadWindowRelaySession(replies)));
      final rects = await visibleRects(tester, reading);

      expect(rects, isNotEmpty);
      for (final rect in rects) {
        expect(rect, isNotNull);
        expect(rect!.top, closeTo(before.top, 1));
      }
      expect(findRichText('row 299'), findsNothing);

      // Back at the newest reply, leaving forgets the position.
      await tester.tap(find.byKey(const ValueKey('thread-jump-to-latest')));
      await tester.pumpAndSettle();
      await tester.pumpWidget(const SizedBox());
      expect(store.read(threadRoomKey(_channelId, _openAtRootId)), isNull);
    });

    testWidgets('a late read state still opens at the first unread reply', (
      tester,
    ) async {
      useTallView(tester);
      final readState = _LateReadStateNotifier(
        ReadStateState(
          isReady: false,
          pubkey: 'self',
          contexts: const {_channelId: 999 + 150},
          version: 0,
        ),
      );
      await tester.pumpWidget(
        testable(
          _ThreadWindowRelaySession(replies),
          firstUnread: 150,
          readState: readState,
        ),
      );
      await tester.pump(const Duration(milliseconds: 400));
      expect(listVisible(tester), isFalse);

      readState.becomeReady(
        const ReadStateState(
          isReady: true,
          pubkey: 'self',
          contexts: {_channelId: 999 + 150},
          version: 0,
        ),
      );
      await tester.pumpAndSettle();
      expect(divider, findsOneWidget);
      expect(findRichText('row 150'), findsOneWidget);
      expect(findRichText('row 299'), findsNothing);
    });

    testWidgets(
      'a read marker from another device keeps the thread divider and position',
      (tester) async {
        useTallView(tester);
        final readState = _LateReadStateNotifier(
          const ReadStateState(
            isReady: true,
            pubkey: 'self',
            contexts: {_channelId: 999 + 150},
            version: 0,
          ),
        );
        await tester.pumpWidget(
          testable(
            _ThreadWindowRelaySession(replies),
            firstUnread: 150,
            readState: readState,
          ),
        );
        await tester.pumpAndSettle();
        expect(divider, findsOneWidget);
        final before = tester.getRect(findRichText('row 150').first);

        // Another device reads the whole thread while it is open here.
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

    testWidgets(
      'without the read state in a second a thread opens at the newest',
      (tester) async {
        useTallView(tester);
        await tester.pumpWidget(
          testable(
            _ThreadWindowRelaySession(replies),
            firstUnread: 150,
            readReady: false,
          ),
        );
        await tester.pump(readStateOpenWait ~/ 2);
        expect(listVisible(tester), isFalse);
        await tester.pump(readStateOpenWait);
        await tester.pumpAndSettle();
        expect(listVisible(tester), isTrue);
        expect(findRichText('row 299'), findsOneWidget);
      },
    );
  });
}
