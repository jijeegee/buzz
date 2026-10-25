part of '../channel_detail_page_test.dart';

const _storedScope = MessageScope(
  relayUrl: 'https://relay.test',
  pubkey: 'self',
);

/// Counts the thread window queries a thread page sends.
class _CountingThreadRelaySession extends _ThreadWindowRelaySession {
  _CountingThreadRelaySession(super.replies);

  int threadQueries = 0;

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) {
    if (filters.any((filter) => filter.extensions['thread_window'] == true)) {
      threadQueries += 1;
    }
    return super.queryRelay(filters, timeout: timeout);
  }
}

void threadStoredOpenTests() {
  group('Opening a thread stored in the message database', () {
    final replies = [for (var n = 100; n < 180; n++) _openAtReply(n)];
    late Directory root;
    late MessageStore messages;

    setUp(() {
      root = Directory.systemTemp.createTempSync('thread_stored_open_test');
      messages = MessageStore(
        () async => '${root.path}/messages',
        rootPath: '${root.path}/messages',
      );
    });

    Future<void> seed(WidgetTester tester, {required bool live}) =>
        tester.runAsync(() async {
          final newest = replies.last;
          await messages.write(
            _storedScope,
            MessageDbWrite(
              rows: messageRowsFor(replies, receivedAt: 1),
              claims: [
                CoverageClaim(
                  channelId: _channelId,
                  stream: CoverageStream.thread(_openAtRootId),
                  range: CoverageRange(
                    CoverageCursor.start,
                    CoverageCursor(newest.createdAt, newest.id),
                  ),
                ),
              ],
              liveFrom: {if (live) _channelId: newest.createdAt},
            ),
          );
          await messages.settled();
        });

    Future<void> closeStore(WidgetTester tester) async {
      await tester.pumpWidget(const SizedBox());
      await tester.runAsync(() async {
        await messages.close();
        root.deleteSync(recursive: true);
      });
    }

    Widget testable(_CountingThreadRelaySession session) => _buildTestable(
      messages: [_openAtRoot],
      relaySessionNotifier: session,
      huddleCurrentPubkey: 'self',
      relayInfo: RelayInfo(supportedExtensions: {newerWindowExtension}),
      readStateNotifier: _SynchronousReadStateNotifier(
        ReadStateState(
          isReady: true,
          pubkey: 'self',
          contexts: {_channelId: 5000},
          version: 0,
        ),
      ),
      overrides: [
        messageScopeProvider.overrideWithValue(_storedScope),
        messageStoreProvider.overrideWithValue(messages),
        channelRoomStoreProvider.overrideWith(
          (ref) => ChannelRoomStore(messages: messages)
            ..markLive()
            ..setFedRooms(_storedScope, {_channelId}),
        ),
      ],
      home: ThreadDetailPage(
        threadHead: formatTimeline([_openAtRoot]).single,
        allMessages: formatTimeline([_openAtRoot]),
        channelId: _channelId,
        currentPubkey: 'self',
        isMember: true,
        isArchived: false,
      ),
    );

    void useTallView(WidgetTester tester) {
      tester.view.physicalSize = const Size(400, 800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
    }

    final loading = find.byKey(const ValueKey('thread-replies-loading'));

    bool listVisible(WidgetTester tester) {
      final reveal = find.byKey(const ValueKey('thread-open-reveal'));
      if (find
          .byKey(const ValueKey('thread-message-list'))
          .evaluate()
          .isEmpty) {
        return false;
      }
      return reveal.evaluate().isEmpty ||
          tester.widget<Opacity>(reveal).opacity > 0;
    }

    testWidgets('paints the newest stored replies on the first frame', (
      tester,
    ) async {
      useTallView(tester);
      await seed(tester, live: true);
      final session = _CountingThreadRelaySession(replies);
      await tester.pumpWidget(testable(session));

      // Replies from the first frame on (at most one layout frame out of
      // sight while the composer dock is measured), never a loading row,
      // and every frame they show is at the final position.
      final newest = findRichText('row 179');
      expect(loading, findsNothing);
      expect(newest, findsOneWidget);
      final rects = <Rect>[];
      var hiddenFrames = listVisible(tester) ? 0 : 1;
      if (hiddenFrames == 0) rects.add(tester.getRect(newest));
      for (var frame = 0; frame < 30; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        expect(loading, findsNothing);
        if (!listVisible(tester)) {
          hiddenFrames += 1;
          continue;
        }
        rects.add(tester.getRect(newest));
      }
      await tester.pumpAndSettle();
      expect(hiddenFrames, lessThanOrEqualTo(1));
      expect(rects, everyElement(tester.getRect(newest)));
      expect(findRichText('row 100'), findsNothing);
      expect(session.threadQueries, 0);
      await closeStore(tester);
    });

    testWidgets('a stored thread paints while refreshing its unverified head', (
      tester,
    ) async {
      useTallView(tester);
      await seed(tester, live: false);
      final session = _CountingThreadRelaySession(replies);
      await tester.pumpWidget(testable(session));

      expect(loading, findsNothing);
      expect(findRichText('row 179'), findsOneWidget);
      await tester.pumpAndSettle();
      expect(findRichText('row 179'), findsOneWidget);
      expect(session.threadQueries, greaterThan(0));
      await closeStore(tester);
    });
  });
}
