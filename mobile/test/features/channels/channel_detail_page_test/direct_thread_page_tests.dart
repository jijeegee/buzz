part of '../channel_detail_page_test.dart';

/// A silent relay that notes the root of each thread query.
class _ThreadQueryRecordingSession extends _SilentRelaySession {
  final rootsAsked = <String>[];

  @override
  Future<List<NostrEvent>> queryRelay(
    List<NostrFilter> filters, {
    Duration timeout = const Duration(seconds: 8),
  }) {
    for (final filter in filters) {
      if (filter.extensions['thread_window'] == true) {
        rootsAsked.addAll(filter.tags['#e'] ?? const []);
      }
    }
    return super.queryRelay(filters, timeout: timeout);
  }
}

void directThreadPageTests() {
  final reveal = find.byKey(const ValueKey('thread-open-reveal'));
  final loading = find.byKey(const ValueKey('thread-replies-loading'));

  bool listVisible(WidgetTester tester) =>
      tester.widget<Opacity>(reveal).opacity > 0;

  void useTallView(WidgetTester tester) {
    tester.view.physicalSize = const Size(400, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
  }

  Override rootLookup({
    Future<List<NostrEvent>> Function()? relay,
    Future<({NostrEvent event, bool deleted})?> Function(String id)? stored,
  }) => threadRootSourcesProvider.overrideWithValue(
    ThreadRootSources(
      stored: (_, args) => stored?.call(args.rootId) ?? Future.value(),
      relay: (_, _) => relay?.call() ?? Completer<List<NostrEvent>>().future,
    ),
  );

  Override storedReplies(Future<List<NostrEvent>> Function() events) =>
      paintableThreadHeadProvider.overrideWithValue(
        (_, _) async => serveThreadWindow(
          _channelId,
          _openAtRootId,
          await events(),
          NostrFilter(kinds: const [], limit: 50),
        ),
      );

  Widget harness(
    Widget home, {
    required RelaySessionNotifier session,
    List<Override> overrides = const [],
    RelayInfo? relayInfo,
    _FakeMessagesNotifier? messagesNotifier,
  }) => _buildTestable(
    messages: const [],
    messagesNotifier: messagesNotifier,
    relaySessionNotifier: session,
    huddleCurrentPubkey: 'self',
    relayInfo: relayInfo,
    channelsNotifier: _FakeChannelsNotifier([_testChannel]),
    readStateNotifier: _SynchronousReadStateNotifier(
      const ReadStateState(
        isReady: true,
        pubkey: 'self',
        contexts: {_channelId: 5000},
        version: 0,
      ),
    ),
    overrides: overrides,
    home: home,
  );

  ThreadDetailPage threadPage({
    String? initialMessageId,
    int? initialMessageCreatedAt,
    bool awaitsLinkedReply = false,
    bool headMayBeNested = false,
  }) => ThreadDetailPage(
    headId: _openAtRootId,
    channelId: _channelId,
    currentPubkey: 'self',
    isMember: true,
    isArchived: false,
    channelBelow: false,
    initialMessageId: initialMessageId,
    initialMessageCreatedAt: initialMessageCreatedAt,
    awaitsLinkedReply: awaitsLinkedReply,
    headMayBeNested: headMayBeNested,
  );

  group('The thread page without its relay', () {
    final replies = [for (var n = 100; n < 160; n++) _openAtReply(n)];
    final windowRelay = RelayInfo(supportedExtensions: {newerWindowExtension});

    testWidgets('a short stored thread shows its name, never loading text', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          threadPage(),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(),
            storedReplies(() async => replies.take(3).toList()),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(findRichText('row 102'), findsOneWidget);
      expect(find.byKey(const ValueKey('thread-root-pending')), findsOneWidget);
      expect(loading, findsNothing);
    });

    testWidgets('loading text waits for the store, then shows at a link', (
      tester,
    ) async {
      final store = Completer<List<NostrEvent>>();
      await tester.pumpWidget(
        harness(
          threadPage(initialMessageId: fixtureId(120)),
          session: _SilentRelaySession(),
          relayInfo: windowRelay,
          overrides: [rootLookup(), storedReplies(() => store.future)],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(loading, findsNothing);

      store.complete(const []);
      await tester.pump();
      await tester.pump();

      expect(listVisible(tester), isTrue);
      expect(loading, findsOneWidget);
    });

    testWidgets('a linked reply a drain brings opens there', (tester) async {
      useTallView(tester);
      var stored = const <NostrEvent>[];
      await tester.pumpWidget(
        harness(
          threadPage(initialMessageId: fixtureId(120)),
          session: _SilentRelaySession(),
          relayInfo: windowRelay,
          overrides: [rootLookup(), storedReplies(() async => stored)],
        ),
      );
      await tester.pump();
      await tester.pump();
      expect(findRichText('row 120'), findsNothing);

      stored = replies;
      ProviderScope.containerOf(
        tester.element(find.byType(ThreadDetailPage)),
      ).read(pushInboxRevisionProvider.notifier).bump();
      await tester.pump();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      expect(listVisible(tester), isTrue);
      expect(findRichText('row 120'), findsOneWidget);
      expect(findRichText('row 159'), findsNothing);
    });

    testWidgets('offline, a store slower than the relay check still lands', (
      tester,
    ) async {
      useTallView(tester);
      final store = Completer<List<NostrEvent>>();
      await tester.pumpWidget(
        harness(
          threadPage(initialMessageId: fixtureId(120)),
          session: _SilentRelaySession(),
          overrides: [
            relayInfoProvider.overrideWith((ref) async => null),
            rootLookup(),
            storedReplies(() => store.future),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();
      await tester.pump();

      store.complete(replies);
      await tester.pump();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      expect(listVisible(tester), isTrue);
      expect(findRichText('row 120'), findsOneWidget);
      expect(findRichText('row 159'), findsNothing);
    });

    testWidgets('a draft head found in the store opens the composer', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          threadPage(headMayBeNested: true),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(
              stored: (id) async => (event: _openAtRoot, deleted: false),
            ),
            storedReplies(() async => const []),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(find.byType(ComposeBar), findsOneWidget);
    });

    testWidgets('a draft head waits for the lookup while it is out', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          threadPage(headMayBeNested: true),
          session: _SilentRelaySession(),
          overrides: [rootLookup(), storedReplies(() async => const [])],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(find.byType(ComposeBar), findsNothing);
    });

    testWidgets('a failed lookup never hides the composer', (tester) async {
      await tester.pumpWidget(
        harness(
          threadPage(headMayBeNested: true),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(relay: () async => throw TimeoutException('root')),
            storedReplies(() async => const []),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();
      await tester.pump();

      expect(find.byType(ComposeBar), findsOneWidget);
    });
  });

  group('A linked reply beyond the walked pages', () {
    final replies = [for (var n = 100; n < 900; n++) _openAtReply(n)];
    final target = findRichText('row 110');

    Widget linked(
      _ThreadWindowRelaySession session,
      ValueNotifier<int?> createdAt, {
      required bool awaitsWhileUnknown,
    }) => harness(
      ValueListenableBuilder(
        valueListenable: createdAt,
        builder: (_, at, _) => threadPage(
          initialMessageId: fixtureId(110),
          initialMessageCreatedAt: at,
          awaitsLinkedReply: awaitsWhileUnknown && at == null,
        ),
      ),
      session: session,
      relayInfo: RelayInfo(supportedExtensions: {newerWindowExtension}),
      overrides: [
        rootLookup(relay: () async => [_openAtRoot]),
        storedReplies(() async => const []),
      ],
    );

    testWidgets('waits out of sight for its time, then opens at it', (
      tester,
    ) async {
      useTallView(tester);
      final createdAt = ValueNotifier<int?>(null);
      addTearDown(createdAt.dispose);
      await tester.pumpWidget(
        linked(
          _ThreadWindowRelaySession(replies),
          createdAt,
          awaitsWhileUnknown: true,
        ),
      );
      for (var frame = 0; frame < 10; frame++) {
        await tester.pump(const Duration(milliseconds: 50));
      }
      expect(findRichText('row 899'), findsOneWidget);
      expect(listVisible(tester), isFalse);

      createdAt.value = 1110;
      final rects = <Rect?>[];
      for (var frame = 0; frame < 60; frame++) {
        await tester.pump(const Duration(milliseconds: 16));
        if (listVisible(tester)) {
          rects.add(
            target.evaluate().isEmpty ? null : tester.getRect(target.first),
          );
        }
      }
      await tester.pumpAndSettle();

      expect(rects, isNotEmpty);
      expect(rects, everyElement(tester.getRect(target)));
      expect(findRichText('row 899'), findsNothing);
    });

    testWidgets('a time arriving after it gave up still lands there', (
      tester,
    ) async {
      useTallView(tester);
      final createdAt = ValueNotifier<int?>(null);
      addTearDown(createdAt.dispose);
      await tester.pumpWidget(
        linked(
          _ThreadWindowRelaySession(replies),
          createdAt,
          awaitsWhileUnknown: false,
        ),
      );
      await tester.pumpAndSettle();
      expect(target, findsNothing);

      createdAt.value = 1110;
      await tester.pumpAndSettle();

      expect(target, findsOneWidget);
    });
  });

  group('A thread opened by a link', () {
    const parent = 'nested-parent';
    final replies = [for (var n = 100; n < 110; n++) _openAtReply(n)];
    final nested = _textMsg(
      id: 'nested-reply',
      pubkey: 'carol',
      content: 'Nested reply',
      createdAt: 1200,
      extraTags: [
        ['e', _openAtRootId, '', 'root'],
        const ['e', parent, '', 'reply'],
      ],
    );
    ThreadDetailPage shownPage(WidgetTester tester) =>
        tester.widget<ThreadDetailPage>(find.byType(ThreadDetailPage));

    Widget link({String? threadId, String? messageId, Channel? channel}) =>
        threadOrRoomPage(
          channel: channel ?? _testChannel,
          threadId: threadId ?? _openAtRootId,
          messageId: messageId,
        );

    Override storedByRoot(List<NostrEvent> rows, {List<String>? asked}) =>
        paintableThreadHeadProvider.overrideWithValue((_, args) async {
          asked?.add(args.rootId);
          return serveThreadWindow(
            _channelId,
            args.rootId,
            args.rootId == _openAtRootId ? rows : const [],
            NostrFilter(kinds: const [], limit: 50),
          );
        });

    Future<({NostrEvent event, bool deleted})?> Function(String) storedReply(
      NostrEvent reply,
    ) =>
        (id) async => id == reply.id ? (event: reply, deleted: false) : null;

    testWidgets('lands on a stored reply while nothing else answers', (
      tester,
    ) async {
      useTallView(tester);
      await tester.pumpWidget(
        harness(
          link(messageId: fixtureId(105)),
          session: _SilentRelaySession(),
          relayInfo: RelayInfo(supportedExtensions: {newerWindowExtension}),
          overrides: [
            rootLookup(stored: storedReply(replies[5])),
            storedReplies(() async => replies),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));

      expect(find.byType(ChannelDetailPage), findsNothing);
      expect(listVisible(tester), isTrue);
      expect(findRichText('row 105'), findsOneWidget);
      expect(loading, findsNothing);
      expect(shownPage(tester).headId, _openAtRootId);
      expect(shownPage(tester).initialMessageCreatedAt, replies[5].createdAt);
    });

    testWidgets('a link at a thread with no reply known opens by its id', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          link(messageId: 'unknown-reply'),
          session: _SilentRelaySession(),
          overrides: [rootLookup(), storedReplies(() async => const [])],
        ),
      );
      await tester.pump();

      expect(find.byType(ThreadDetailPage), findsOneWidget);
      expect(find.byType(ChannelDetailPage), findsNothing);
      expect(shownPage(tester).headId, _openAtRootId);
      expect(shownPage(tester).awaitsLinkedReply, isTrue);
    });

    testWidgets('a nested reply reads its root once it is known', (
      tester,
    ) async {
      final session = _ThreadQueryRecordingSession();
      final asked = <String>[];
      await tester.pumpWidget(
        harness(
          link(threadId: parent, messageId: nested.id),
          session: session,
          overrides: [
            rootLookup(stored: storedReply(nested)),
            storedByRoot([nested], asked: asked),
          ],
        ),
      );
      for (var frame = 0; frame < 5; frame++) {
        await tester.pump(const Duration(milliseconds: 20));
      }

      expect(shownPage(tester).headId, parent);
      expect(shownPage(tester).rootId, _openAtRootId);
      expect(asked, [parent, _openAtRootId]);
      expect(session.rootsAsked, [parent, _openAtRootId]);
      expect(find.text('Nested reply'), findsWidgets);
    });

    testWidgets('a hidden parent moves the page to the root in place', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          link(threadId: parent, messageId: nested.id),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(
              stored: storedReply(nested),
              relay: () async => const [],
            ),
            storedByRoot([nested]),
          ],
        ),
      );
      await tester.pumpAndSettle();

      expect(shownPage(tester).headId, _openAtRootId);
      expect(shownPage(tester).rootId, isNull);
      expect(find.text('Nested reply'), findsWidgets);
      final navigator = Navigator.of(
        tester.element(find.byType(ThreadDetailPage)),
      );
      expect(navigator.canPop(), isFalse);
    });

    testWidgets('a drain brings a reply not found at first', (tester) async {
      NostrEvent? stored;
      await tester.pumpWidget(
        harness(
          link(messageId: fixtureId(105)),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(
              stored: (id) async =>
                  stored == null ? null : (event: stored!, deleted: false),
              relay: () async => const [],
            ),
            storedReplies(() async => const []),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();
      expect(shownPage(tester).initialMessageCreatedAt, isNull);
      expect(shownPage(tester).awaitsLinkedReply, isFalse);

      stored = replies[5];
      ProviderScope.containerOf(
        tester.element(find.byType(ThreadDetailPage)),
      ).read(pushInboxRevisionProvider.notifier).bump();
      await tester.pump();
      await tester.pump();

      expect(shownPage(tester).initialMessageCreatedAt, replies[5].createdAt);
    });

    testWidgets('a reconnect asks again for a reply not found', (tester) async {
      final session = _SilentRelaySession();
      var answer = const <NostrEvent>[];
      await tester.pumpWidget(
        harness(
          link(messageId: fixtureId(105)),
          session: session,
          overrides: [
            rootLookup(relay: () async => answer),
            storedReplies(() async => const []),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();
      expect(shownPage(tester).initialMessageCreatedAt, isNull);

      answer = [replies[5]];
      session.drop();
      await tester.pump();
      session.comeBack();
      await tester.pump();
      await tester.pump();

      expect(shownPage(tester).initialMessageCreatedAt, replies[5].createdAt);
    });

    testWidgets('a reply of another room is refused', (tester) async {
      final elsewhere = fixtureRow('other-room', 105, 1105);
      await tester.pumpWidget(
        harness(
          link(messageId: elsewhere.id),
          session: _SilentRelaySession(),
          overrides: [
            rootLookup(
              stored: storedReply(elsewhere),
              relay: () async => const [],
            ),
            storedReplies(() async => const []),
          ],
        ),
      );
      await tester.pump();
      await tester.pump();

      expect(shownPage(tester).initialMessageCreatedAt, isNull);
    });

    testWidgets('own pending replies settle by a link', (tester) async {
      final messages = _UnconfirmedReplyMessagesNotifier();
      await tester.pumpWidget(
        harness(
          link(messageId: fixtureId(105)),
          session: _ThreadWindowRelaySession(replies),
          messagesNotifier: messages,
          overrides: [
            rootLookup(
              stored: storedReply(replies[5]),
              relay: () async => [_openAtRoot],
            ),
          ],
        ),
      );
      await tester.pumpAndSettle();

      expect(messages.asked, greaterThan(0));
    });

    testWidgets('Back returns to where the app was', (tester) async {
      await tester.pumpWidget(
        harness(
          Builder(
            builder: (context) => TextButton(
              onPressed: () => Navigator.of(context).push(
                MaterialPageRoute<void>(
                  builder: (_) => link(messageId: fixtureId(105)),
                ),
              ),
              child: const Text('Origin'),
            ),
          ),
          session: _SilentRelaySession(),
          overrides: [rootLookup(), storedReplies(() async => replies)],
        ),
      );
      await tester.tap(find.text('Origin'));
      await tester.pumpAndSettle();

      expect(find.byType(ThreadDetailPage), findsOneWidget);
      expect(find.byType(ChannelDetailPage), findsNothing);
      expect(shownPage(tester).channelBelow, isFalse);

      await tester.pageBack();
      await tester.pumpAndSettle();

      expect(find.text('Origin'), findsOneWidget);
      expect(find.byType(ThreadDetailPage), findsNothing);
    });

    test('a listed room keeps the channel page for its top level', () {
      final page = threadOrRoomPage(
        channel: _testChannel,
        messageId: 'message',
        highlightInitialMessage: true,
        ownerView: const OwnerView(
          agentPubkey: 'agent',
          agentName: 'Agent',
          dmChannelId: 'dm',
        ),
      );

      expect(page, isA<ChannelDetailPage>());
      page as ChannelDetailPage;
      expect(page.initialMessageId, 'message');
      expect(page.highlightInitialMessage, isTrue);
      expect(page.ownerView?.agentName, 'Agent');
    });

    test('a thread keeps its owner view', () {
      final page = threadOrRoomPage(
        channel: _testChannel,
        threadId: 'thread',
        ownerView: const OwnerView(
          agentPubkey: 'agent',
          agentName: 'Agent',
          dmChannelId: 'dm',
        ),
      );

      expect((page as DirectThreadPage).ownerView?.agentName, 'Agent');
    });

    test('a forum room opens its room page, never the thread', () {
      final forum = Channel(
        id: _channelId,
        name: 'design-forum',
        channelType: 'forum',
        visibility: 'open',
        description: '',
        createdBy: 'abc123',
        createdAt: DateTime(2025),
        memberCount: 5,
        isMember: true,
      );

      expect(
        threadOrRoomPage(channel: forum, threadId: 'post'),
        isA<ChannelDetailPage>(),
      );
    });

    testWidgets('the room as listed replaces the one it opened with', (
      tester,
    ) async {
      await tester.pumpWidget(
        harness(
          link(channel: _testChannel.copyWith(isMember: false)),
          session: _SilentRelaySession(),
          overrides: [rootLookup(), storedReplies(() async => const [])],
        ),
      );
      await tester.pump();

      expect(shownPage(tester).isMember, isTrue);
    });
  });
}
