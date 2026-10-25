part of '../channel_detail_page_test.dart';

/// Serves the open-at channel and counts its window queries; the first
/// [failures] queries from a cursor fail as a relay that does not answer
/// would.
class _CountingWindowRelaySession extends _WindowRelaySession {
  _CountingWindowRelaySession(super.rows, {this.failures = 0});

  int failures;
  int windowQueries = 0;

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) {
    // Window pages and catch-up scans; other features' queries aside.
    if (filters.any(
      (filter) =>
          filter.extensions['top_level'] == true || filter.since != null,
    )) {
      windowQueries += 1;
    }
    final fromCursor = filters.any(
      (filter) =>
          filter.extensions['before_id'] != null ||
          filter.extensions['after_id'] != null,
    );
    if (fromCursor && failures > 0) {
      failures -= 1;
      return Future.error(TimeoutException('no answer'));
    }
    return super.queryRelay(filters, timeout: timeout);
  }
}

/// A thread relay whose first [failures] window queries from a cursor fail.
class _FailingThreadRelaySession extends _ThreadWindowRelaySession {
  _FailingThreadRelaySession(super.replies, {required this.failures});

  int failures;

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) {
    final filter = filters.single;
    final fromCursor =
        filter.extensions['before_id'] != null ||
        filter.extensions['after_id'] != null;
    if (filter.extensions['thread_window'] == true &&
        fromCursor &&
        failures > 0) {
      failures -= 1;
      return Future.error(TimeoutException('no answer'));
    }
    return super.queryRelay(filters, timeout: timeout);
  }
}

void roomStoredOpenTests() {
  group('Opening a room at a message stored in the database', () {
    final rows = [for (var n = 100; n < 300; n++) _openAtRow(n)];
    late Directory root;
    late MessageStore messages;

    setUp(() {
      root = Directory.systemTemp.createTempSync('room_stored_open_test');
      messages = MessageStore(
        () async => '${root.path}/messages',
        rootPath: '${root.path}/messages',
      );
    });

    Future<void> seed(WidgetTester tester) => tester.runAsync(() async {
      final newest = rows.last;
      await messages.write(
        _storedScope,
        MessageDbWrite(
          rows: messageRowsFor(rows, receivedAt: 1),
          claims: [
            CoverageClaim(
              channelId: _channelId,
              stream: CoverageStream.main,
              range: CoverageRange(
                CoverageCursor.start,
                CoverageCursor(newest.createdAt, newest.id),
              ),
            ),
          ],
          liveFrom: {_channelId: newest.createdAt},
        ),
      );
      await messages.settled();
    });

    Future<void> closeStore(WidgetTester tester) async {
      await tester.pumpWidget(const SizedBox());
      // Pending database writes and read-marker updates run out.
      await tester.pump(const Duration(seconds: 10));
      await tester.runAsync(() async {
        await messages.close();
        root.deleteSync(recursive: true);
      });
    }

    Widget testable(
      _CountingWindowRelaySession session, {
      required int readAt,
      bool readStateReady = true,
      int? observedUnread,
    }) => _buildTestable(
      huddleCurrentPubkey: 'self',
      messages: const [],
      realMessages: true,
      relaySessionNotifier: session,
      relayInfo: RelayInfo(supportedExtensions: {newerWindowExtension}),
      channelsNotifier: _FakeChannelsNotifier(
        [_testChannel],
        observedUnread: {
          if (observedUnread != null)
            _channelId: [
              makeObservedUnreadEvent(
                id: fixtureId(observedUnread),
                createdAt: 1000 + observedUnread,
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
          isReady: readStateReady,
          pubkey: 'self',
          contexts: {_channelId: readAt},
          version: 0,
        ),
      ),
      overrides: [
        // Not the rows' author, so they count as unread.
        myPubkeyProvider.overrideWithValue('b' * 64),
        messageScopeProvider.overrideWithValue(_storedScope),
        messageStoreProvider.overrideWithValue(messages),
        channelRoomStoreProvider.overrideWith(
          (ref) => ChannelRoomStore(messages: messages)
            ..markLive()
            ..setFedRooms(_storedScope, {_channelId}),
        ),
      ],
    );

    void useTallView(WidgetTester tester) {
      tester.view.physicalSize = const Size(400, 800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
    }

    final messageList = find.byKey(const ValueKey('channel-message-list'));
    final divider = find.byKey(const ValueKey('channel-new-messages-divider'));

    bool timelineVisible(WidgetTester tester) {
      if (messageList.evaluate().isEmpty) return false;
      final content = find.ancestor(
        of: messageList,
        matching: find.byKey(const Key('skeleton-reveal-content')),
      );
      return tester.widget<Opacity>(content.first).opacity > 0;
    }

    /// Every frame the timeline is seen in: where [target] was, and whether
    /// the newest row showed.
    Future<List<(Rect?, bool)>> visibleFrames(
      WidgetTester tester,
      Finder target,
    ) async {
      final frames = <(Rect?, bool)>[];
      for (var frame = 0; frame < 300; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        if (!timelineVisible(tester)) continue;
        frames.add((
          target.evaluate().isEmpty ? null : tester.getRect(target.first),
          findRichText('row 299').evaluate().isNotEmpty,
        ));
        if (!tester.binding.hasScheduledFrame) break;
      }
      await tester.pumpAndSettle();
      return frames;
    }

    testWidgets('opens at the first unread with no request, final from the '
        'first visible frame', (tester) async {
      useTallView(tester);
      await seed(tester);
      final session = _CountingWindowRelaySession(rows);
      await tester.pumpWidget(
        testable(session, readAt: 1149, observedUnread: 150),
      );

      final frames = await visibleFrames(tester, divider);
      expect(frames, isNotEmpty);
      final settled = tester.getRect(divider);
      expect(frames, everyElement((settled, false)));
      expect(findRichText('row 150'), findsOneWidget);
      expect(session.windowQueries, 0);
      await closeStore(tester);
    });

    testWidgets('the database finds the first unread before the read state '
        'is confirmed and before the channel list has seen it', (tester) async {
      useTallView(tester);
      await seed(tester);
      final session = _CountingWindowRelaySession(rows);
      await tester.pumpWidget(
        testable(session, readAt: 1149, readStateReady: false),
      );

      final frames = await visibleFrames(tester, divider);
      expect(frames, isNotEmpty);
      expect(frames, everyElement((tester.getRect(divider), false)));
      final firstUnread = tester.getRect(findRichText('row 150'));
      expect(
        tester.getRect(divider).bottom,
        lessThanOrEqualTo(firstUnread.top),
      );
      expect(session.windowQueries, 0);
      await closeStore(tester);
    });
  });

  group('Opening a room at a message the relay does not answer for', () {
    final rows = [for (var n = 100; n < 300; n++) _openAtRow(n)];

    testWidgets('stays out of sight and asks again; never the newest rows '
        'first', (tester) async {
      tester.view.physicalSize = const Size(400, 800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      // Both pages of the first open fail.
      final session = _CountingWindowRelaySession(rows, failures: 2);
      await tester.pumpWidget(
        _buildTestable(
          huddleCurrentPubkey: 'self',
          messages: const [],
          realMessages: true,
          relaySessionNotifier: session,
          relayInfo: RelayInfo(supportedExtensions: {newerWindowExtension}),
          channelsNotifier: _FakeChannelsNotifier(
            [_testChannel],
            observedUnread: {
              _channelId: [
                makeObservedUnreadEvent(
                  id: fixtureId(150),
                  createdAt: 1150,
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
              contexts: {_channelId: 1149},
              version: 0,
            ),
          ),
        ),
      );
      final messageList = find.byKey(const ValueKey('channel-message-list'));
      for (var frame = 0; frame < 120; frame++) {
        await tester.pump(const Duration(milliseconds: 50));
        expect(findRichText('row 299'), findsNothing);
      }
      expect(messageList, findsNothing);

      await tester.pump(const Duration(seconds: 8));
      await tester.pumpAndSettle();
      expect(
        find.byKey(const ValueKey('channel-new-messages-divider')),
        findsOneWidget,
      );
      expect(findRichText('row 150'), findsOneWidget);
      expect(findRichText('row 299'), findsNothing);
    });
  });

  group('Opening a thread at a reply the relay does not answer for', () {
    final replies = [for (var n = 100; n < 300; n++) _openAtReply(n)];

    testWidgets('stays out of sight and asks again', (tester) async {
      tester.view.physicalSize = const Size(400, 800);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final session = _FailingThreadRelaySession(replies, failures: 2);
      await tester.pumpWidget(
        _buildTestable(
          messages: [_openAtRoot],
          relaySessionNotifier: session,
          huddleCurrentPubkey: 'self',
          relayInfo: RelayInfo(supportedExtensions: {newerWindowExtension}),
          readStateNotifier: _SynchronousReadStateNotifier(
            ReadStateState(
              isReady: true,
              pubkey: 'self',
              contexts: {_channelId: 2000},
              version: 0,
            ),
          ),
          home: ThreadDetailPage(
            threadHead: formatTimeline([_openAtRoot]).single,
            allMessages: formatTimeline([
              _openAtRoot,
              ...replies.where((reply) => reply.id == fixtureId(120)),
            ]),
            channelId: _channelId,
            currentPubkey: 'self',
            isMember: true,
            isArchived: false,
            initialMessageId: fixtureId(120),
          ),
        ),
      );
      bool visible(String text) {
        final reveal = find.byKey(const ValueKey('thread-open-reveal'));
        return findRichText(text).evaluate().isNotEmpty &&
            (reveal.evaluate().isEmpty ||
                tester.widget<Opacity>(reveal).opacity > 0);
      }

      for (var frame = 0; frame < 120; frame++) {
        await tester.pump(const Duration(milliseconds: 50));
        expect(visible('row 299'), isFalse);
        expect(visible('row 120'), isFalse);
      }

      await tester.pump(const Duration(seconds: 8));
      await tester.pumpAndSettle();
      expect(visible('row 120'), isTrue);
      expect(visible('row 299'), isFalse);
    });
  });
}
